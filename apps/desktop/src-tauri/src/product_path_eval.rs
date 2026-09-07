//! The headless product-path harness shared by two consumers: the
//! `agent-product-path-contract` gate test (always compiled under `cfg(test)`)
//! and the feature-gated Phase 4 evaluation driver (`product-eval`, never part
//! of a shipping build).
//!
//! It owns the two test doubles the seam needs — a headless [`AgentRunHost`]
//! and a local fake OpenAI-compatible server — and the single function that
//! drives the shipping run entry against a composed [`AppState`].
#![cfg(any(test, feature = "product-eval"))]

/// The Phase 4 driver lives beside the harness it drives; both are excluded
/// from every shipping build.
#[cfg(feature = "product-eval")]
#[path = "product_eval_driver.rs"]
pub(crate) mod driver;

use crate::agent_commands::task::run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate;
use crate::agent_execution_constraint::AgentExecutionConstraint;
use crate::agent_preparation_runtime::AgentMemoryEvaluationConstraint;
use crate::app_composition::{compose_app_state, DesktopComposition};
use crate::app_state::AppState;
use crate::configuration_models::{ProjectSessionConfig, ProviderConfig};
use crate::desktop_event_sink::{AgentRunHost, DesktopEventSink, DetachedStateJob};
use crate::runtime_values::phase16_task_id;
use crate::schedule::ScheduleConfig;
use crate::view_models::{AgentTaskInput, ModelStreamDelta, RagOperationProgress};
use agent_core::{EventKind, Metadata};
use agent_runtime::AgentRunControl;
use agent_storage::SqliteStore;
use std::io::{BufRead, BufReader, Read as IoRead, Write as IoWrite};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The headless host: records emissions, counts background scheduling, and
/// runs detached jobs against the harness-owned state.
pub(crate) struct HeadlessRunHost {
    state: Arc<AppState>,
    events: Mutex<Vec<String>>,
    memory_refreshes: AtomicUsize,
    title_refinements: AtomicUsize,
}

impl HeadlessRunHost {
    pub(crate) fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            events: Mutex::new(Vec::new()),
            memory_refreshes: AtomicUsize::new(0),
            title_refinements: AtomicUsize::new(0),
        }
    }

    pub(crate) fn recorded(&self) -> Vec<String> {
        self.events.lock().expect("host events lock").clone()
    }

    pub(crate) fn memory_refresh_count(&self) -> usize {
        self.memory_refreshes.load(Ordering::SeqCst)
    }
}

impl DesktopEventSink for HeadlessRunHost {
    fn emit_model_stream_delta(&self, payload: ModelStreamDelta) {
        self.events
            .lock()
            .expect("host events lock")
            .push(format!("emit_model_stream_delta done={}", payload.done));
    }

    fn emit_rag_operation_progress(&self, _payload: RagOperationProgress) {
        self.events
            .lock()
            .expect("host events lock")
            .push("emit_rag_operation_progress".to_string());
    }

    fn emit_session_title_updated(&self, _session_id: String) {
        self.events
            .lock()
            .expect("host events lock")
            .push("emit_session_title_updated".to_string());
    }
}

impl AgentRunHost for HeadlessRunHost {
    fn schedule_semantic_memory_refresh(
        &self,
        _workspace_root: PathBuf,
        _config: ProviderConfig,
        _run_context: Metadata,
    ) {
        self.memory_refreshes.fetch_add(1, Ordering::SeqCst);
    }

    fn spawn_detached_state_job(
        &self,
        job: DetachedStateJob,
    ) -> std::sync::mpsc::Receiver<Result<String, String>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let state = self.state.clone();
        std::thread::spawn(move || {
            let _ = sender.send(job(&state));
        });
        receiver
    }

    fn spawn_session_title_refinement(
        &self,
        _refinement: crate::session_title_service::SessionTitleRefinement,
    ) {
        self.title_refinements.fetch_add(1, Ordering::SeqCst);
    }
}

/// Request counters of the fake provider, split by endpoint so an evaluation
/// can price chat turns and embedding calls separately.
#[derive(Debug, Default, Clone)]
pub(crate) struct FakeProviderCounters {
    pub(crate) chat: usize,
    pub(crate) embeddings: usize,
}

/// A local fake OpenAI-compatible provider. Its accept thread lives until the
/// process exits (fine for tests and the driver, which are short-lived
/// dedicated processes). Chat routing is content-driven
/// and stateless, so any number of sequential runs can share one server: a
/// request whose messages already contain a tool observation receives the
/// final answer; otherwise it receives one `file.read` tool call. The
/// embeddings endpoint answers with a fixed small vector (the index records
/// the served dimension itself).
pub(crate) struct FakeOpenAiServer {
    pub(crate) base_url: String,
    counters: Arc<Mutex<FakeProviderCounters>>,
    _server: std::thread::JoinHandle<()>,
}

impl FakeOpenAiServer {
    pub(crate) fn start(final_answer: &str) -> Self {
        Self::start_inner(final_answer, false)
    }

    /// The same double with the first chat request answered by the exact
    /// provider 400 that rejects `thinking_budget` (the shape seen in the
    /// consumed Phase 4 run 1). This arms the provider's compatibility
    /// fallback inside the harness so tests can prove the suppression fact
    /// reaches the durable model-turn events — the key the evaluation driver
    /// counts into `thinking_suppressed_calls` — instead of staying
    /// provider-internal and silently voiding an effort arm's treatment.
    #[cfg(test)]
    pub(crate) fn start_with_thinking_rejection(final_answer: &str) -> Self {
        Self::start_inner(final_answer, true)
    }

    fn start_inner(final_answer: &str, thinking_rejection: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake provider should bind");
        let port = listener.local_addr().expect("listener address").port();
        let counters = Arc::new(Mutex::new(FakeProviderCounters::default()));
        let server_counters = counters.clone();
        let rejection_armed = Arc::new(AtomicBool::new(thinking_rejection));
        let server_rejection = rejection_armed.clone();
        let answer = final_answer.to_string();
        let server = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
                stream.set_write_timeout(Some(Duration::from_secs(60))).ok();
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(clone) => clone,
                    Err(_) => continue,
                });
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut content_length = 0usize;
                let mut saw_content_length = false;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some(value) =
                        trimmed.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        saw_content_length = true;
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                if !saw_content_length {
                    // Fail loudly instead of silently scripting against an
                    // empty body: reqwest always sets Content-Length here, so
                    // its absence means the harness no longer understands the
                    // client and the run's request accounting must break.
                    let response = "HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(response.as_bytes());
                    continue;
                }
                let mut body = vec![0u8; content_length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let body_text = String::from_utf8_lossy(&body).to_string();
                let streaming = body_text.replace(' ', "").contains("\"stream\":true");
                let embeddings = request_line.contains("embeddings");
                let rejected = !embeddings && server_rejection.swap(false, Ordering::SeqCst);
                let payload = if rejected {
                    {
                        let mut counters = server_counters.lock().expect("fake counters");
                        counters.chat += 1;
                    }
                    "{\"error\":{\"message\":\"InternalError.Algo.InvalidParameter: Parameter thinking_budget is not supported.\",\"type\":\"invalid_request_error\"}}".to_string()
                } else if embeddings {
                    {
                        let mut counters = server_counters.lock().expect("fake counters");
                        counters.embeddings += 1;
                    }
                    embeddings_response()
                } else {
                    let saw_tool_observation = body_text.contains("\"role\":\"tool\"")
                        || body_text.contains("\"role\": \"tool\"");
                    {
                        let mut counters = server_counters.lock().expect("fake counters");
                        counters.chat += 1;
                    }
                    if saw_tool_observation {
                        chat_response(streaming, None, &answer)
                    } else {
                        chat_response(
                            streaming,
                            Some(("call-read-eval", "file.read", "{\"path\":\"README.md\"}")),
                            "",
                        )
                    }
                };
                let content_type = if streaming && !embeddings && !rejected {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let status_line = if rejected {
                    "HTTP/1.1 400 Bad Request"
                } else {
                    "HTTP/1.1 200 OK"
                };
                let response = format!(
                    "{status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                if stream.write_all(response.as_bytes()).is_err() {
                    continue;
                }
                let _ = stream.flush();
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            counters,
            _server: server,
        }
    }

    pub(crate) fn counters(&self) -> FakeProviderCounters {
        self.counters.lock().expect("fake counters").clone()
    }
}

fn sse(events: &[String]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(event);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// One chat response in both wire shapes. `tool_call` = (id, name, args);
/// `None` produces a plain content turn with `finish_reason: stop`.
fn chat_response(streaming: bool, tool_call: Option<(&str, &str, &str)>, content: &str) -> String {
    let (delta_payload, finish) = match tool_call {
        Some((id, name, args)) => (
            format!(
                r#"{{"role":"assistant","tool_calls":[{{"index":0,"id":"{id}","type":"function","function":{{"name":"{name}","arguments":{args_json}}}}}]}}"#,
                args_json = serde_json::Value::String(args.to_string())
            ),
            "tool_calls",
        ),
        None => (
            format!(
                r#"{{"role":"assistant","content":{content}}}"#,
                content = serde_json::Value::String(content.to_string())
            ),
            "stop",
        ),
    };
    if streaming {
        sse(&[
            format!(
                r#"{{"id":"fake-eval","model":"fake-model","choices":[{{"index":0,"delta":{delta_payload},"finish_reason":null}}]}}"#
            ),
            format!(
                r#"{{"choices":[{{"index":0,"delta":{{}},"finish_reason":"{finish}"}}],"usage":{{"prompt_tokens":30,"completion_tokens":8,"total_tokens":38}}}}"#
            ),
        ])
    } else {
        format!(
            r#"{{"id":"fake-eval","model":"fake-model","choices":[{{"index":0,"message":{message_body},"finish_reason":"{finish}"}}],"usage":{{"prompt_tokens":30,"completion_tokens":8,"total_tokens":38}}}}"#,
            message_body = match tool_call {
                Some((id, name, args)) => format!(
                    r#"{{"role":"assistant","content":null,"tool_calls":[{{"id":"{id}","type":"function","function":{{"name":"{name}","arguments":{args_json}}}}}]}}"#,
                    args_json = serde_json::Value::String(args.to_string())
                ),
                None => format!(
                    r#"{{"role":"assistant","content":{content}}}"#,
                    content = serde_json::Value::String(content.to_string())
                ),
            }
        )
    }
}

fn embeddings_response() -> String {
    let vector = vec![0.01f32; 64];
    format!(
        r#"{{"object":"list","model":"fake-model","data":[{{"object":"embedding","index":0,"embedding":{vector}}}],"usage":{{"prompt_tokens":1,"total_tokens":1}}}}"#,
        vector = serde_json::to_string(&vector).expect("vector serializes")
    )
}

/// Darwin suspension clocks: `CLOCK_MONOTONIC_RAW` (4) keeps ticking through
/// system sleep while `CLOCK_UPTIME_RAW` (8) pauses, so their per-span deltas
/// give the milliseconds the host actually slept — independent of
/// `std::time::Instant`'s platform-varying sleep semantics, which is why the
/// wall-minus-monotonic drift alone can be structurally dead on Apple
/// silicon. Constants verified against this host (clock probe, 2026-09-06);
/// the sampler sanity-checks the pair at every read and yields `None` when
/// the relationship does not hold.
#[cfg(target_os = "macos")]
pub(crate) fn host_sleep_clocks_ns() -> Option<(u64, u64)> {
    extern "C" {
        fn clock_gettime_nsec_np(clock_id: u32) -> u64;
    }
    const CLOCK_MONOTONIC_RAW: u32 = 4;
    const CLOCK_UPTIME_RAW: u32 = 8;
    let continuous = unsafe { clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW) };
    let uptime = unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) };
    (continuous > 0 && uptime > 0 && continuous >= uptime).then_some((continuous, uptime))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn host_sleep_clocks_ns() -> Option<(u64, u64)> {
    None
}

/// One headless drive of the shipping run entry.
pub(crate) struct ProductPathRunRequest {
    pub(crate) prompt: String,
    /// Effort tier label exactly as the composer sends it.
    pub(crate) effort: String,
    /// Matched single-agent arm: filter and deny the `task` delegation surface.
    pub(crate) no_delegation: bool,
    pub(crate) provider_config: ProviderConfig,
    /// Fixture-materialized workspace the run executes against.
    pub(crate) workspace_root: PathBuf,
    /// Where the run's SQLite event store lives (per-cell isolation).
    pub(crate) store_path: PathBuf,
}

/// Everything a matched-arm cell report needs from one run, taken from the
/// run's own durable outputs: the projected state, the shipping telemetry
/// receipt, and the event log — never from harness-side guesswork.
// Some fields are read only by the fidelity-gate tests (host_events,
// memory_refreshes, state, session_id) and not by the feature-gated driver's
// lib build; both consumers share this outcome by design.
#[allow(dead_code)]
pub(crate) struct ProductPathRunOutcome {
    pub(crate) status: String,
    pub(crate) final_answer: String,
    pub(crate) session_id: String,
    /// Wall clock across the run, as the protocol's latency accounting asks.
    pub(crate) wall_ms: u64,
    /// Monotonic clock across the same span; a large wall-minus-monotonic
    /// drift means the host slept and the cell must be censored.
    pub(crate) monotonic_ms: u64,
    pub(crate) telemetry: Option<agent_application::RunTelemetryReceiptV1>,
    pub(crate) network_tool_calls: usize,
    /// The entry's own error, when it returned `Err`.
    pub(crate) run_error: Option<String>,
    /// Milliseconds the host slept during the run per the darwin clock pair;
    /// falls back to wall-minus-monotonic drift where the pair is unavailable.
    pub(crate) host_slept_ms: u64,
    /// What flowed through the host seam during the run (the fidelity gate
    /// asserts on it; the driver ignores it).
    pub(crate) host_events: Vec<String>,
    pub(crate) memory_refreshes: usize,
    /// The composed state the run used, kept so callers can query the durable
    /// event log the run wrote.
    pub(crate) state: Option<Arc<AppState>>,
}

pub(crate) fn drive_product_path_run(request: ProductPathRunRequest) -> ProductPathRunOutcome {
    let session_config = ProjectSessionConfig::default_for_root(&request.workspace_root);
    let session_id = session_config.active_session_id.clone();
    let composition_result = (|| -> Result<(Arc<AppState>, String), String> {
        let store = SqliteStore::open(&request.store_path)
            .map_err(|error| format!("cell store failed to open: {error}"))?;
        let composition = DesktopComposition {
            provider_config: request.provider_config.clone(),
            mcp_catalog: agent_mcp::McpCatalogService::load(
                request.workspace_root.join("mcp.json"),
                request.workspace_root.join("mcp-cache.json"),
            ),
            workspace_config: crate::configuration_models::WorkspaceConfig {
                root: request.workspace_root.clone(),
            },
            sidecar_config: crate::configuration_models::SidecarConfig::default(),
            web_search_config: tools::WebSearchConfig::default(),
            project_session_config: session_config,
            schedule_config: ScheduleConfig::default(),
            schedule_last_error: None,
            recovered_memory_refreshes: Vec::new(),
        };
        Ok((
            Arc::new(compose_app_state(composition, store)),
            session_id.clone(),
        ))
    })();
    let (state, session_id) = match composition_result {
        Ok(composed) => composed,
        Err(error) => {
            return ProductPathRunOutcome {
                status: "error".to_string(),
                final_answer: String::new(),
                session_id,
                wall_ms: 0,
                monotonic_ms: 0,
                telemetry: None,
                network_tool_calls: 0,
                run_error: Some(error),
                host_slept_ms: 0,
                host_events: Vec::new(),
                memory_refreshes: 0,
                state: None,
            }
        }
    };
    let host = HeadlessRunHost::new(state.clone());
    let effort_policy = agent_core::AgentPolicy::parse_ingress(&request.effort);
    let input = AgentTaskInput {
        prompt: request.prompt.clone(),
        session_id: session_id.clone(),
        current_time: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        queue_id: None,
        effort: effort_policy.label().to_string(),
        attachments: Vec::new(),
        plan_mode: false,
    };
    let cancellation = Arc::new(AgentRunControl::new(effort_policy.label()));
    let constraint = if request.no_delegation {
        AgentExecutionConstraint::EvalNoDelegation
    } else {
        AgentExecutionConstraint::Native
    };

    let wall_start = SystemTime::now();
    let monotonic_start = Instant::now();
    let sleep_clocks_start = host_sleep_clocks_ns();
    let result = run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate(
        &host,
        &state,
        input,
        &cancellation,
        constraint,
        AgentMemoryEvaluationConstraint::Native,
        false,
        &mut None,
    );
    let monotonic_ms = monotonic_start.elapsed().as_millis() as u64;
    let host_slept_ms = match (sleep_clocks_start, host_sleep_clocks_ns()) {
        (Some((cont_start, up_start)), Some((cont_end, up_end))) => {
            cont_end
                .saturating_sub(cont_start)
                .saturating_sub(up_end.saturating_sub(up_start))
                / 1_000_000
        }
        _ => 0,
    };
    let wall_ms = wall_start
        .elapsed()
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(monotonic_ms);

    let (status, final_answer, run_error) = match result {
        Ok(agent) => {
            let answer = agent
                .latest_answer
                .clone()
                .filter(|answer| !answer.trim().is_empty())
                .or_else(|| {
                    agent.messages.iter().rev().find_map(|message| {
                        (message.role == "assistant" && !message.content.trim().is_empty())
                            .then(|| message.content.clone())
                    })
                })
                .unwrap_or_default();
            (agent.status.clone(), answer, None)
        }
        Err(error) => ("error".to_string(), String::new(), Some(error)),
    };

    // The shipping telemetry receipt for exactly this run, read back from the
    // journal the run itself wrote (isolated by CINDX_DATA_DIR in the driver).
    let telemetry = crate::run_telemetry_runtime::load_run_telemetry_receipts(
        &crate::run_telemetry_runtime::run_telemetry_journal_path(),
    )
    .ok()
    .and_then(|receipts| {
        receipts
            .into_iter()
            .rev()
            .find(|receipt| receipt.session_id == session_id)
    });

    let network_tool_calls = {
        let store = state.store.lock().expect("store lock");
        crate::agent_read_model::agent_events_for_session(
            &store,
            &phase16_task_id(),
            Some(&session_id),
        )
        .map(|events| {
            events
                .iter()
                .filter(|event| event.kind == EventKind::ToolCallStarted)
                .filter(|event| {
                    matches!(
                        event.metadata.get("tool").map(String::as_str),
                        Some("web.search") | Some("web.fetch")
                    )
                })
                .count()
        })
        .unwrap_or(0)
    };

    ProductPathRunOutcome {
        status,
        final_answer,
        session_id,
        wall_ms,
        monotonic_ms,
        telemetry,
        network_tool_calls,
        run_error,
        host_slept_ms,
        host_events: host.recorded(),
        memory_refreshes: host.memory_refresh_count(),
        state: Some(state),
    }
}

/// Materializes one case fixture set into a fresh cell workspace.
#[cfg(feature = "product-eval")]
pub(crate) fn materialize_fixture(workspace_root: &std::path::Path, files: &[(String, String)]) {
    for (path, content) in files {
        let target = workspace_root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent should be created");
        }
        std::fs::write(&target, content).expect("fixture file should be written");
    }
}
