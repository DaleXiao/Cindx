use super::*;
use crate::agent_terminal_commit_runtime::persist_agent_terminal_once;
use crate::run_telemetry_runtime::{
    load_run_telemetry_receipts, project_run_telemetry_receipt, record_run_telemetry_terminal_to,
    run_telemetry_journal_path_for, RunTelemetryTerminalFacts, RUN_TELEMETRY_JOURNAL_CAPACITY,
};
use agent_application::RunTelemetryTerminalPathV1;
use agent_core::TaskId;

fn telemetry_context() -> Metadata {
    [
        ("project_id".to_string(), "project-telemetry".to_string()),
        ("session_id".to_string(), "session-telemetry".to_string()),
        (
            "agent_run_id".to_string(),
            format!("run-telemetry-{}", unique_id("case")),
        ),
        ("steer_epoch".to_string(), "0".to_string()),
        ("agent_effort".to_string(), "default".to_string()),
    ]
    .into_iter()
    .collect()
}

fn exercised_control() -> Arc<AgentRunControl> {
    let control = Arc::new(AgentRunControl::new("auto"));
    control.begin_model_call("act").expect("model call begins");
    std::thread::sleep(std::time::Duration::from_millis(2));
    control.finish_model_call();
    let attempt = control
        .begin_physical_model_attempt("model-a", 10, 5, RunStageClass::Worker)
        .expect("physical attempt reserves");
    assert!(control.finish_physical_model_attempt(
        attempt,
        Some(agent_runtime::ModelAttemptUsage::new(
            10,
            5,
            15,
            agent_runtime::ModelUsageSource::Provider,
        )),
    ));
    control
        .begin_tool_call("scope", "file.read", r#"{"path":"a.md"}"#)
        .expect("tool call begins");
    control.finish_tool_call();
    control.record_agent_turn("act").expect("turn records");
    control.record_context_compaction();
    control.record_rolling_summary();
    control.record_retrieval(120, 3, 9);
    control
}

fn telemetry_journal_path() -> PathBuf {
    std::env::temp_dir()
        .join(unique_id("run-telemetry"))
        .join("journal.jsonl")
}

fn facts_for<'a>(
    run_context: &'a Metadata,
    control: &'a AgentRunControl,
) -> RunTelemetryTerminalFacts<'a> {
    RunTelemetryTerminalFacts {
        run_context,
        control,
        terminal_path: RunTelemetryTerminalPathV1::Direct,
        stop_reason: "completed",
    }
}

#[test]
fn run_telemetry_projection_fills_fields_from_real_run_counters() {
    let control = exercised_control();
    let run_context = telemetry_context();
    let receipt =
        project_run_telemetry_receipt(&facts_for(&run_context, &control)).expect("projection");

    assert_eq!(receipt.schema, agent_application::RUN_TELEMETRY_SCHEMA);
    assert_eq!(
        receipt.agent_run_id,
        run_context.get("agent_run_id").cloned().unwrap()
    );
    assert_eq!(receipt.session_id, "session-telemetry");
    assert_eq!(receipt.effort, "default");
    assert_eq!(receipt.terminal_path, RunTelemetryTerminalPathV1::Direct);
    assert_eq!(receipt.stop_reason, "completed");
    assert_eq!(receipt.model_calls, 1);
    assert!(receipt.model_wait_ms >= 2);
    assert_eq!(receipt.tool_calls, 1);
    assert_eq!(receipt.agent_turns, 1);
    assert_eq!(receipt.context_compactions, 1);
    assert_eq!(receipt.rolling_summaries, 1);
    assert_eq!(receipt.retrieval_ms, 120);
    assert_eq!(receipt.retrieval_channels, 3);
    assert_eq!(receipt.retrieval_channel_hits, 9);
    assert_eq!(receipt.prompt_tokens, 10);
    assert_eq!(receipt.completion_tokens, 5);
    assert_eq!(receipt.usage_provider_attempts, 1);
    assert_eq!(receipt.usage_estimated_attempts, 0);
    assert_eq!(receipt.usage_unknown_attempts, 0);
}

#[test]
fn run_telemetry_projection_fails_closed_without_run_identity() {
    let control = exercised_control();
    let mut run_context = telemetry_context();
    run_context.remove("agent_run_id");
    let path = telemetry_journal_path();
    let error = record_run_telemetry_terminal_to(&path, &facts_for(&run_context, &control))
        .expect_err("missing run identity must fail closed");
    assert!(error.contains("agent run id"));
    assert!(!path.exists());
}

#[test]
fn run_telemetry_journal_lives_beside_the_app_data_root() {
    let root = std::env::temp_dir().join(unique_id("run-telemetry-root"));
    let path = run_telemetry_journal_path_for(&root);
    assert_eq!(path.parent(), Some(root.as_path()));
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("run-telemetry.journal.jsonl")
    );
}

#[test]
fn run_telemetry_journal_round_trips_through_the_capped_private_file() {
    let control = exercised_control();
    let run_context = telemetry_context();
    let path = telemetry_journal_path();
    let receipt = record_run_telemetry_terminal_to(&path, &facts_for(&run_context, &control))
        .expect("recording");
    let receipts = load_run_telemetry_receipts(&path).expect("journal loads");
    assert_eq!(receipts, vec![receipt]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path)
            .expect("journal metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "journal must stay private");
    }
}

#[test]
fn run_telemetry_journal_rotation_keeps_the_newest_receipts() {
    let path = telemetry_journal_path();
    let mut expected_last_run_id = String::new();
    for index in 0..(RUN_TELEMETRY_JOURNAL_CAPACITY + 5) {
        let control = exercised_control();
        let mut run_context = telemetry_context();
        let run_id = format!("run-telemetry-rotation-{index}");
        run_context.insert("agent_run_id".to_string(), run_id.clone());
        expected_last_run_id = run_id;
        record_run_telemetry_terminal_to(&path, &facts_for(&run_context, &control))
            .expect("append");
    }
    let receipts = load_run_telemetry_receipts(&path).expect("journal loads");
    assert_eq!(receipts.len(), RUN_TELEMETRY_JOURNAL_CAPACITY);
    assert_eq!(
        receipts
            .first()
            .map(|receipt| receipt.agent_run_id.as_str()),
        Some("run-telemetry-rotation-5")
    );
    assert_eq!(
        receipts.last().map(|receipt| receipt.agent_run_id.as_str()),
        Some(expected_last_run_id.as_str())
    );
}

fn persist_completed_terminal(
    store: &mut SqliteStore,
    identity: &agent_application::AgentTerminalCommitIdentity,
    context: &Metadata,
) -> Result<AgentState, StorageError> {
    append_event(
        store,
        &TaskId("agent".to_string()),
        EventKind::TaskStatusChanged,
        "Agent task completed",
        metadata_with_context(identity.metadata(), context),
    )?;
    agent_state_for_session(store, None, Some("session-telemetry"))
}

#[test]
fn run_telemetry_terminal_records_exactly_once_per_durable_terminal_commit() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = telemetry_context();
    let path = telemetry_journal_path();

    let first_control = exercised_control();
    let first_lease = match first_control.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("first execution lease unavailable: {outcome:?}"),
    };
    let first = first_control
        .commit_terminal_result_with(first_lease, || {
            persist_agent_terminal_once(
                &mut store,
                &TaskId("agent".to_string()),
                &run_context,
                0,
                |store, identity| persist_completed_terminal(store, identity, &run_context),
            )
        })
        .expect("first terminal commit should persist");
    let agent_runtime::RunTerminalCommit::Committed(persisted) = first else {
        panic!("first terminal commit should commit");
    };
    assert!(persisted.inserted);
    if persisted.inserted {
        record_run_telemetry_terminal_to(&path, &facts_for(&run_context, &first_control))
            .expect("first terminal records telemetry");
    }

    let replay_control = AgentRunControl::new("auto");
    let replay_lease = match replay_control.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        outcome => panic!("replay execution lease unavailable: {outcome:?}"),
    };
    let replay = replay_control
        .commit_terminal_result_with(replay_lease, || {
            persist_agent_terminal_once(
                &mut store,
                &TaskId("agent".to_string()),
                &run_context,
                0,
                |store, identity| persist_completed_terminal(store, identity, &run_context),
            )
        })
        .expect("durable replay should load the existing terminal state");
    let agent_runtime::RunTerminalCommit::Committed(persisted) = replay else {
        panic!("replay should resolve the committed terminal");
    };
    assert!(!persisted.inserted);
    if persisted.inserted {
        record_run_telemetry_terminal_to(&path, &facts_for(&run_context, &replay_control))
            .expect("replay must not reach this recording gate");
    }

    let receipts = load_run_telemetry_receipts(&path).expect("journal loads");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].model_calls, 1);
    assert_eq!(receipts[0].tool_calls, 1);
}

#[test]
fn run_telemetry_journal_has_no_production_reader_or_consumer() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    // The journal path and its reader stay confined to the shadow module and
    // its tests; no production code can address or read the journal. The one
    // addition is the Phase 4 product-path harness/driver, which reads
    // receipts as its per-run evidence: `product_path_eval.rs` is compiled
    // only under `cfg(any(test, feature = "product-eval"))`, and `product-eval`
    // is a non-default feature excluded from every shipping build, so the
    // no-production-reader invariant is unchanged.
    let mut journal_files = Vec::new();
    collect_source_references(&source_root, "run_telemetry_journal", &mut journal_files);
    for file in &journal_files {
        assert!(
            file.ends_with("run_telemetry_runtime.rs")
                || file.ends_with("run_telemetry_runtime_tests.rs")
                || file.ends_with("product_path_eval.rs"),
            "unexpected telemetry journal reference in {}",
            file.display()
        );
    }
    let mut reader_files = Vec::new();
    collect_source_references(
        &source_root,
        "load_run_telemetry_receipts",
        &mut reader_files,
    );
    for file in &reader_files {
        assert!(
            file.ends_with("run_telemetry_runtime.rs")
                || file.ends_with("run_telemetry_runtime_tests.rs")
                || file.ends_with("product_path_eval.rs"),
            "production file {} must not read the telemetry journal",
            file.display()
        );
    }

    // The producer entry point is wired only at terminal commit sites.
    let mut producer_files = Vec::new();
    collect_source_references(
        &source_root,
        "record_run_telemetry_terminal",
        &mut producer_files,
    );
    let allowed_producers = [
        "run_telemetry_runtime.rs",
        "run_telemetry_runtime_tests.rs",
        "agent_completion_runtime.rs",
        "agent_failure_terminal_runtime.rs",
        "task.rs",
    ];
    for file in &producer_files {
        assert!(
            allowed_producers.iter().any(|name| file.ends_with(name)),
            "unexpected telemetry producer wiring in {}",
            file.display()
        );
    }
}

fn collect_source_references(directory: &Path, needle: &str, matches: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .expect("source directory should read")
        .collect::<Result<Vec<_>, _>>()
        .expect("source directory entries should read");
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_source_references(&path, needle, matches);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            && fs::read_to_string(&path)
                .map(|content| content.contains(needle))
                .unwrap_or(false)
        {
            matches.push(path);
        }
    }
}
