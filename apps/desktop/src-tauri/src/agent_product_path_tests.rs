//! Headless product-path harness — the Phase 4 fidelity clause's acceptance
//! proof. This drives the SHIPPING run command
//! (`run_agent_task_blocking_inner_with_evaluation_constraints_and_start_gate`)
//! end to end without Tauri, through the shared doubles in
//! `crate::product_path_eval`: a composed `AppState` (the same
//! `compose_app_state` the real startup uses), a headless `AgentRunHost`, and
//! a local fake OpenAI-compatible server. What runs is the real preparation,
//! the real model loop over real HTTP transport, real tool execution with
//! permission policy, the real durable event lineage, and the real
//! terminal/delivery path — the reduced-kernel-loop gap the frozen Phase 4
//! protocol refuses to accept.

use super::*;
use crate::product_path_eval::{drive_product_path_run, FakeOpenAiServer, ProductPathRunRequest};
use crate::runtime_values::phase16_task_id;
use agent_core::EventKind;

fn fake_eval_config(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        base_url: base_url.to_string(),
        api_key: "fake-key".to_string(),
        model: "fake-model".to_string(),
        executor_model: "fake-model".to_string(),
        ..Default::default()
    }
}

/// The fidelity gate: the shipping run path, driven headlessly, completes a
/// tool-using turn and leaves the full durable lineage behind.
#[test]
fn shipping_run_path_completes_headlessly_with_durable_lineage() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("README.md"), "line one\nline two\n")
        .expect("seed README");
    let server = FakeOpenAiServer::start("README says line one");

    // Fast keeps the deterministic knowledge plan (no memory/recall or
    // workspace retrieval), so the only provider traffic is the two scripted
    // chat turns — the loop, tools, and lineage are the subject.
    let outcome = drive_product_path_run(ProductPathRunRequest {
        prompt: "Read README.md and answer with its first line.".to_string(),
        effort: "fast".to_string(),
        no_delegation: false,
        provider_config: fake_eval_config(&server.base_url),
        workspace_root: workspace.path().to_path_buf(),
        store_path: workspace.path().join("state.sqlite3"),
    });

    assert_eq!(
        outcome.status, "completed",
        "run should complete; error: {:?}",
        outcome.run_error
    );
    assert!(
        outcome.final_answer.contains("README says line one"),
        "the delivered answer should reach the projected outcome"
    );

    // The durable lineage a real run leaves behind: the tool call started and
    // finished under the run's identity, and the task reached its terminal.
    let state = outcome.state.as_ref().expect("composed state");
    let events = {
        let store = state.store.lock().expect("store lock");
        crate::agent_read_model::agent_events_for_session(
            &store,
            &phase16_task_id(),
            Some(&outcome.session_id),
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
    // closed with a done delta, the completion scheduled the post-run
    // semantic-memory refresh, and the fake provider saw exactly the two
    // scripted chat turns (tool call, then final answer).
    assert!(
        outcome
            .host_events
            .iter()
            .any(|event| event == "emit_model_stream_delta done=true"),
        "the run must close its model stream through the host sink"
    );
    assert_eq!(
        outcome.memory_refreshes, 1,
        "completion must schedule the semantic-memory refresh through the host seam"
    );
    let counters = server.counters();
    assert_eq!(
        counters.chat, 2,
        "exactly two chat calls: the tool turn and the answer turn"
    );
    assert_eq!(counters.embeddings, 0, "fast performs no retrieval");
}

/// The treatment-delivered measurement chain: when the provider rejects the
/// thinking parameters, the shipping run recovers via the compatibility
/// fallback AND stamps the durable suppression fact onto its model-turn
/// events — the exact key the evaluation driver counts into
/// `thinking_suppressed_calls`. Without this, an effort arm whose thinking
/// budget was stripped stays silently treatment-undelivered (the run-2
/// confound this instrumentation exists to prevent).
#[test]
fn thinking_rejection_leaves_a_durable_suppression_fact_on_model_turns() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("README.md"), "line one\nline two\n")
        .expect("seed README");
    let server = FakeOpenAiServer::start_with_thinking_rejection("README says line one");

    // The tier model must be thinking-default family (qwen/kimi/...), or the
    // request carries no thinking params, there is nothing to strip, and the
    // fallback correctly declines to retry — the mirror of production, where
    // the tier models are exactly the families that send thinking params.
    // `fast_model` is pinned because an empty pin falls back to the provider
    // catalog's hardcoded fast default, which is not a thinking family.
    let provider_config = ProviderConfig {
        model: "kimi-k3".to_string(),
        executor_model: "kimi-k3".to_string(),
        fast_model: "kimi-k3".to_string(),
        ..fake_eval_config(&server.base_url)
    };
    let outcome = drive_product_path_run(ProductPathRunRequest {
        prompt: "Read README.md and answer with its first line.".to_string(),
        effort: "fast".to_string(),
        no_delegation: false,
        provider_config,
        workspace_root: workspace.path().to_path_buf(),
        store_path: workspace.path().join("state.sqlite3"),
    });

    let state = outcome.state.as_ref().expect("composed state");
    let events = {
        let store = state.store.lock().expect("store lock");
        crate::agent_read_model::agent_events_for_session(
            &store,
            &phase16_task_id(),
            Some(&outcome.session_id),
        )
        .expect("session events should load")
    };
    assert_eq!(
        outcome.status, "completed",
        "the stripped retry should recover the run; error: {:?}",
        outcome.run_error
    );
    // The same predicate the evaluation driver counts (ModelRequestFinished
    // events carrying the key — assistant message events copy the same
    // response metadata and must not double-count a call), so this assertion
    // is the receipt's durable-fact pipeline itself, not a parallel copy. It
    // runs over EVERY model turn of the run: the retry-recovered first turn
    // and every later turn served under prepare-time suppression. That the
    // later turns carry the fact pins the provider instance (and its
    // suppression state) persisting across the run — the exact mechanism
    // that silently voided run 2's high/xhigh tiers.
    let model_turns = events
        .iter()
        .filter(|event| event.kind == EventKind::ModelRequestFinished)
        .count();
    let suppressed_turns = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::ModelRequestFinished
                && event
                    .metadata
                    .contains_key(model_provider::THINKING_SUPPRESSED_METADATA_KEY)
        })
        .count();
    assert!(
        model_turns > 0 && suppressed_turns == model_turns,
        "every model turn served under suppression must carry the durable fact \
         ({suppressed_turns} stamped of {model_turns} turns)"
    );
    assert_eq!(
        server.counters().chat, 3,
        "the rejected first request, its stripped retry (the tool-call turn), \
         and the final-answer turn"
    );
}

/// The second seam behavior: detached state jobs run against the host-owned
/// state and report through the returned channel, with the caller keeping its
/// timeout semantics (the guardian-review shape).
#[test]
fn headless_host_runs_detached_state_jobs_against_the_composed_state() {
    use crate::app_composition::{compose_app_state, DesktopComposition};
    use crate::product_path_eval::HeadlessRunHost;
    use agent_storage::SqliteStore;
    use std::sync::Arc;

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
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("detached job should report through the channel")
        .expect("detached job should succeed");
    assert_eq!(reported, workspace.path().display().to_string());
}
