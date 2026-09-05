//! Headless product-path harness — the Phase 4 fidelity clause's acceptance
//! proof. This drives the SHIPPING run command
//! (`run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate`)
//! end to end without Tauri: a composed `AppState` (the same
//! `compose_app_state` the real startup uses), a headless `AgentRunHost`
//! (recording sink, owned-state detached jobs), and a local fake
//! OpenAI-compatible server. What runs is the real preparation, the real model
//! loop over real HTTP transport, real tool execution with permission policy,
//! the real durable event lineage, and the real terminal/delivery path — the
//! reduced-kernel-loop gap the frozen Phase 4 protocol refuses to accept.

use super::*;
use crate::app_composition::{compose_app_state, DesktopComposition};
use crate::configuration_models::{ProjectSessionConfig, ProviderConfig};
use crate::desktop_event_sink::{AgentRunHost, DesktopEventSink, DetachedStateJob};
use crate::runtime_values::phase16_task_id;
use crate::schedule::ScheduleConfig;
use crate::view_models::{AgentTaskInput, ModelStreamDelta, RagOperationProgress};
use agent_core::EventKind;
use agent_runtime::AgentRunControl;
use agent_storage::SqliteStore;
use std::io::{BufRead, BufReader, Read as IoRead, Write as IoWrite};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The headless host: records emissions, counts background scheduling, and
/// runs detached jobs against the harness-owned state.
struct HeadlessRunHost {
    state: Arc<AppState>,
    events: Mutex<Vec<String>>,
    memory_refreshes: AtomicUsize,
    title_refinements: AtomicUsize,
}

impl HeadlessRunHost {
    fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            events: Mutex::new(Vec::new()),
            memory_refreshes: AtomicUsize::new(0),
            title_refinements: AtomicUsize::new(0),
        }
    }

    fn recorded(&self) -> Vec<String> {
        self.events.lock().expect("host events lock").clone()
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
        _workspace_root: std::path::PathBuf,
        _config: ProviderConfig,
        _run_context: agent_core::Metadata,
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

/// A local fake OpenAI-compatible provider: first chat request answers with
/// one `file.read` tool call, the second with the final text. Serves both the
/// streaming (SSE) and non-streaming (JSON) shapes from the same script.
struct FakeProvider {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    _server: std::thread::JoinHandle<()>,
}

fn sse(events: &[&str]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(event);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn tool_call_script(streaming: bool) -> String {
    if streaming {
        sse(&[
            r#"{"id":"fake-1","model":"fake-model","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call-read-1","type":"function","function":{"name":"file.read","arguments":"{\"path\":\"README.md\"}"}}]},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":40,"completion_tokens":10,"total_tokens":50}}"#,
        ])
    } else {
        r#"{"id":"fake-1","model":"fake-model","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call-read-1","type":"function","function":{"name":"file.read","arguments":"{\"path\":\"README.md\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":40,"completion_tokens":10,"total_tokens":50}}"#.to_string()
    }
}

fn final_answer_script(streaming: bool) -> String {
    if streaming {
        sse(&[
            r#"{"id":"fake-2","model":"fake-model","choices":[{"index":0,"delta":{"content":"README says line one"},"finish_reason":null}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":60,"completion_tokens":6,"total_tokens":66}}"#,
        ])
    } else {
        r#"{"id":"fake-2","model":"fake-model","choices":[{"index":0,"message":{"role":"assistant","content":"README says line one"},"finish_reason":"stop"}],"usage":{"prompt_tokens":60,"completion_tokens":6,"total_tokens":66}}"#.to_string()
    }
}

fn start_fake_provider() -> FakeProvider {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fake provider should bind");
    let port = listener.local_addr().expect("listener address").port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let server = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(30))).ok();
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(clone) => clone,
                Err(_) => continue,
            });
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
                if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
                    saw_content_length = true;
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            if !saw_content_length {
                // Fail loudly instead of silently scripting against an empty
                // body: reqwest always sets Content-Length for these requests,
                // so its absence means the harness no longer understands the
                // client and the request-count assertion must break.
                {
                    let mut requests = recorded.lock().expect("fake provider request log");
                    requests.push("missing-content-length".to_string());
                }
                let response = "HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
                continue;
            }
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            let body_text = String::from_utf8_lossy(&body).to_string();
            let request_index = {
                let mut requests = recorded.lock().expect("fake provider request log");
                requests.push(body_text.clone());
                requests.len()
            };
            let streaming = body_text.replace(' ', "").contains("\"stream\":true");
            let payload = if request_index == 1 {
                tool_call_script(streaming)
            } else {
                final_answer_script(streaming)
            };
            let content_type = if streaming {
                "text/event-stream"
            } else {
                "application/json"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            if stream.write_all(response.as_bytes()).is_err() {
                continue;
            }
            let _ = stream.flush();
        }
    });
    FakeProvider {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        requests,
        _server: server,
    }
}

/// The fidelity gate: the shipping run path, driven headlessly, completes a
/// tool-using turn and leaves the full durable lineage behind.
#[test]
fn shipping_run_path_completes_headlessly_with_durable_lineage() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("README.md"), "line one\nline two\n")
        .expect("seed README");
    let fake = start_fake_provider();
    let store = SqliteStore::open(workspace.path().join("state.sqlite3"))
        .expect("harness store should open");
    let session_config = ProjectSessionConfig::default_for_root(workspace.path());
    let session_id = session_config.active_session_id.clone();
    let composition = DesktopComposition {
        provider_config: ProviderConfig {
            base_url: fake.base_url.clone(),
            api_key: "fake-key".to_string(),
            model: "fake-model".to_string(),
            executor_model: "fake-model".to_string(),
            ..Default::default()
        },
        mcp_catalog: McpCatalogService::load(
            workspace.path().join("mcp.json"),
            workspace.path().join("mcp-cache.json"),
        ),
        workspace_config: WorkspaceConfig {
            root: workspace.path().to_path_buf(),
        },
        sidecar_config: SidecarConfig::default(),
        web_search_config: WebSearchConfig::default(),
        project_session_config: session_config,
        schedule_config: ScheduleConfig::default(),
        schedule_last_error: None,
        recovered_memory_refreshes: Vec::new(),
    };
    let state = Arc::new(compose_app_state(composition, store));
    let host = HeadlessRunHost::new(state.clone());

    let input = AgentTaskInput {
        prompt: "Read README.md and answer with its first line.".to_string(),
        session_id: session_id.clone(),
        current_time: "2026-09-05 12:00".to_string(),
        queue_id: None,
        // Fast keeps the deterministic knowledge plan (no memory/recall or
        // workspace retrieval), so the only provider traffic is the two
        // scripted chat turns — the loop, tools, and lineage are the subject.
        effort: "fast".to_string(),
        attachments: Vec::new(),
        plan_mode: false,
    };
    let cancellation = Arc::new(AgentRunControl::new("fast"));

    let result = run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate(
        &host,
        &state,
        input,
        &cancellation,
        crate::agent_execution_constraint::AgentExecutionConstraint::Native,
        AgentMemoryEvaluationConstraint::Native,
        false,
        &mut None,
    );

    let agent = result.expect("the shipping run path should complete headlessly");
    assert_eq!(
        agent.status, "completed",
        "run should complete; last error: {:?}",
        agent.last_error
    );
    assert!(
        agent
            .messages
            .iter()
            .any(|message| message.role == "assistant"
                && message.content.contains("README says line one")),
        "the delivered answer should reach the projected state"
    );

    // The durable lineage a real run leaves behind: the tool call started and
    // finished under the run's identity, and the task reached its terminal.
    let events = {
        let store = state.store.lock().expect("store lock");
        crate::agent_read_model::agent_events_for_session(
            &store,
            &phase16_task_id(),
            Some(&session_id),
        )
        .expect("session events should load")
    };
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::ToolCallStarted
                && event.metadata.get("tool").map(String::as_str) == Some("file.read")),
        "the delegated read must appear as a durable started event"
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::ToolCallFinished
                && event.metadata.get("tool").map(String::as_str) == Some("file.read")
                && event.metadata.get("status").map(String::as_str) == Some("succeeded")),
        "the delegated read must appear as a durable succeeded event"
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::TaskStatusChanged
                && event.summary == "Agent task completed"),
        "the run must reach its durable terminal"
    );

    // The UI-facing side effects flowed through the host seam: the stream
    // closed with a done delta, and the fake provider saw exactly the two
    // scripted chat turns (tool call, then final answer).
    assert!(
        host.recorded()
            .iter()
            .any(|event| event == "emit_model_stream_delta done=true"),
        "the run must close its model stream through the host sink"
    );
    assert_eq!(
        fake.requests
            .lock()
            .expect("fake provider request log")
            .len(),
        2,
        "exactly two provider calls: the tool turn and the answer turn"
    );
    // The completion must schedule the post-run background work through the
    // host seam — one of the two behaviors the seam exists for.
    assert_eq!(
        host.memory_refreshes.load(Ordering::SeqCst),
        1,
        "completion must schedule the semantic-memory refresh through the host seam"
    );
}

/// The second seam behavior: detached state jobs run against the host-owned
/// state and report through the returned channel, with the caller keeping its
/// timeout semantics (the guardian-review shape).
#[test]
fn headless_host_runs_detached_state_jobs_against_the_composed_state() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let store = SqliteStore::open(workspace.path().join("state.sqlite3"))
        .expect("harness store should open");
    let composition = DesktopComposition {
        provider_config: ProviderConfig::default(),
        mcp_catalog: McpCatalogService::load(
            workspace.path().join("mcp.json"),
            workspace.path().join("mcp-cache.json"),
        ),
        workspace_config: WorkspaceConfig {
            root: workspace.path().to_path_buf(),
        },
        sidecar_config: SidecarConfig::default(),
        web_search_config: WebSearchConfig::default(),
        project_session_config: ProjectSessionConfig::default_for_root(workspace.path()),
        schedule_config: ScheduleConfig::default(),
        schedule_last_error: None,
        recovered_memory_refreshes: Vec::new(),
    };
    let host = HeadlessRunHost::new(Arc::new(compose_app_state(composition, store)));

    let receiver = host.spawn_detached_state_job(Box::new(|state| {
        // The job sees the composed state: its workspace root is the harness's.
        let root = state
            .workspace_config
            .lock()
            .map_err(|error| error.to_string())?
            .root
            .display()
            .to_string();
        Ok(root)
    }));
    let reported = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("detached job should report through the channel")
        .expect("detached job should succeed");
    assert_eq!(reported, workspace.path().display().to_string());
}
