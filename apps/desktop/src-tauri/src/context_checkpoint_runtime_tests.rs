use super::test_support::temp_test_root;
use super::test_support::test_message;
use super::*;

#[test]
fn context_estimate_accounts_for_multibyte_text_and_prompt_reserve() {
    let ascii = test_message(MessageRole::User, "abcdefgh");
    let chinese = test_message(MessageRole::User, "你好世界你好世界");
    assert!(estimate_message_tokens(&chinese) > estimate_message_tokens(&ascii));

    let mut image_message = test_message(MessageRole::User, "inspect these images");
    image_message.metadata.insert(
        "image_paths".to_string(),
        "/tmp/one.png\n/tmp/two.png".to_string(),
    );
    assert!(
        estimate_message_tokens(&image_message)
            >= estimate_text_tokens_for_context("inspect these images") + 2_048
    );

    // Token usage is counted with a real BPE tokenizer, which compresses
    // repeated characters heavily; use prose-like text so the history still
    // presses the context window.
    let large_history = vec![test_message(
        MessageRole::User,
        "lorem ipsum dolor sit amet ".repeat(14_000),
    )];
    let plan = session_compaction_plan(&large_history, 100_000);
    assert!(plan.should_compact);
    assert!(plan.estimated_request_tokens > plan.estimated_history_tokens);
}

#[test]
fn context_checkpoint_reuse_has_bounded_hysteresis() {
    let mut history = (0..40)
        .map(|index| {
            test_message(
                if index % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                format!("message {index}"),
            )
        })
        .collect::<Vec<_>>();
    let checkpoint = ValidatedContextCheckpoint {
        text: "verified checkpoint".to_string(),
        covered_messages: 8,
    };
    let plan = session_compaction_plan(&history, 32_000);
    assert!(context_checkpoint_is_within_reuse_window(
        &checkpoint,
        &history,
        plan,
        32_000,
    ));

    history.extend(
        (40..58).map(|index| test_message(MessageRole::Assistant, format!("message {index}"))),
    );
    let plan = session_compaction_plan(&history, 32_000);
    assert!(!context_checkpoint_is_within_reuse_window(
        &checkpoint,
        &history,
        plan,
        32_000,
    ));
}

#[test]
fn recent_context_starts_on_a_complete_user_turn() {
    let history = vec![
        test_message(MessageRole::User, "old request"),
        test_message(MessageRole::Assistant, "old answer"),
        test_message(MessageRole::Tool, "old tool evidence"),
        test_message(MessageRole::User, "latest request"),
        test_message(MessageRole::Assistant, "latest answer"),
    ];
    let budget = estimate_message_tokens(&history[3]) + estimate_message_tokens(&history[4]);
    let (start, tokens) = recent_history_start(&history, budget);

    assert_eq!(start, 3);
    assert!(is_user_turn_start(&history[start]));
    assert_eq!(tokens, budget);

    let (narrow_start, _) = recent_history_start(
        &history,
        estimate_message_tokens(history.last().expect("latest message")),
    );
    assert_eq!(narrow_start, 3);
}

#[test]
fn context_checkpoint_events_stop_at_the_covered_message_prefix() {
    let session_id = "session-prefix";
    let event = |sequence: u64, kind: EventKind, role: Option<&str>, content: Option<&str>| {
        let mut metadata = [("session_id".to_string(), session_id.to_string())]
            .into_iter()
            .collect::<Metadata>();
        if let Some(role) = role {
            metadata.insert("role".to_string(), role.to_string());
        }
        if let Some(content) = content {
            metadata.insert("content".to_string(), content.to_string());
        }
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: phase16_task_id(),
            timestamp_ms: sequence,
            sequence,
            kind,
            summary: format!("event {sequence}"),
            metadata,
        }
    };
    let events = vec![
        event(1, EventKind::MessageAdded, Some("user"), Some("first")),
        event(2, EventKind::ToolCallFinished, None, None),
        event(
            3,
            EventKind::MessageAdded,
            Some("assistant"),
            Some("answer"),
        ),
        event(4, EventKind::TaskStatusChanged, None, None),
        event(5, EventKind::MessageAdded, Some("user"), Some("retained")),
    ];

    let covered = context_events_for_covered_history_prefix(&events, 2);

    assert_eq!(covered.len(), 3);
    assert_eq!(covered.last().map(|event| event.sequence), Some(3));
    assert!(covered
        .iter()
        .all(|event| event.metadata.get("content").map(String::as_str) != Some("retained")));
}

#[test]
fn context_state_builds_and_persists_restore_pack() {
    let root = temp_test_root("phase15-context");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    append_message_event(
        &mut store,
        &phase4_task_id(),
        MessageRole::User,
        "Continue the MVP context manager",
    )
    .expect("message should append");
    append_event(
        &mut store,
        &phase7_task_id(),
        EventKind::RetrievalPerformed,
        "RAG search completed",
        [
            ("action".to_string(), "search".to_string()),
            ("query".to_string(), "context compression".to_string()),
            ("selected_count".to_string(), "2".to_string()),
            (
                "retrieval_mode".to_string(),
                "four_way_parallel".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("retrieval should append");

    let run_context = Metadata::new();
    let preview =
        context_state(&store, &root, &run_context, None, None).expect("state should load");
    let checkpoint = preview.checkpoint.expect("checkpoint should exist");

    assert_eq!(
        checkpoint.current_goal.as_deref(),
        Some("Continue the MVP context manager")
    );
    assert!(checkpoint.path.is_none());
    assert!(checkpoint.restore_pack.contains("## Current Goal"));
    assert!(checkpoint.restore_pack.contains("four_way_parallel"));

    let events = collect_context_events(&store, &run_context).expect("events should collect");
    let pack = build_restore_context_pack(build_session_checkpoint_at(
        &events,
        CheckpointOptions::default(),
        777,
    ));
    let checkpoint_path =
        write_context_checkpoint(&root, None, &pack.text, None).expect("checkpoint should write");
    append_event(
        &mut store,
        &phase15_task_id(),
        EventKind::TaskStatusChanged,
        "Context checkpoint compacted",
        [
            ("checkpoint_id".to_string(), pack.checkpoint.id.clone()),
            (
                "context_checkpoint_path".to_string(),
                checkpoint_path.display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .expect("compact event should append");

    let compacted =
        context_state(&store, &root, &run_context, Some(pack), None).expect("state should load");
    let checkpoint = compacted.checkpoint.expect("checkpoint should exist");
    let expected_path = checkpoint_path.display().to_string();

    assert_eq!(checkpoint.path.as_deref(), Some(expected_path.as_str()));
    assert!(fs::read_to_string(checkpoint_path)
        .expect("checkpoint should be readable")
        .contains("Cindx Context Checkpoint"));
    assert!(compacted
        .timeline
        .iter()
        .any(|entry| entry.detail.contains("Context checkpoint compacted")));
}

#[test]
fn context_events_and_checkpoint_paths_are_session_scoped() {
    let root = temp_test_root("phase15-session-scope");
    fs::create_dir_all(&root).expect("temp root should exist");
    let mut store = SqliteStore::in_memory().expect("store should open");
    for (session_id, content) in [("session-a", "alpha"), ("session-b", "beta")] {
        append_message_event_with_metadata(
            &mut store,
            &phase16_task_id(),
            MessageRole::User,
            content,
            [
                ("project_id".to_string(), "project-a".to_string()),
                ("session_id".to_string(), session_id.to_string()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("message should append");
    }
    let run_context = [
        ("project_id".to_string(), "project-a".to_string()),
        ("session_id".to_string(), "session-a".to_string()),
    ]
    .into_iter()
    .collect();

    let events = collect_context_events(&store, &run_context).expect("events should collect");

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].metadata.get("content").map(String::as_str),
        Some("alpha")
    );
    assert_ne!(
        context_checkpoint_path_for_session(&root, Some("session-a")),
        context_checkpoint_path_for_session(&root, Some("session-b"))
    );
}

#[test]
fn context_checkpoint_coverage_restores_only_the_verified_prefix() {
    let root = temp_test_root("context-checkpoint-coverage");
    let history = vec![
        test_message(MessageRole::User, "first requirement"),
        test_message(MessageRole::Assistant, "first answer"),
        test_message(MessageRole::User, "second requirement"),
        test_message(MessageRole::Assistant, "second answer"),
    ];
    let path = write_context_checkpoint(
        &root,
        Some("session-a"),
        "verified checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 2,
        }),
    )
    .expect("checkpoint should write");

    let checkpoint = read_validated_context_checkpoint(&root, Some("session-a"), &history)
        .expect("coverage should validate");
    assert_eq!(checkpoint.covered_messages, 2);
    let restored = history_with_context_checkpoint(&history, checkpoint, &path)
        .expect("history should restore");

    assert_eq!(restored.len(), 3);
    assert_eq!(restored[1].content, "second requirement");
    assert_eq!(restored[2].content, "second answer");
    assert_eq!(
        restored[0]
            .metadata
            .get("covered_messages")
            .map(String::as_str),
        Some("2")
    );
}

#[test]
fn legacy_context_checkpoint_never_truncates_history() {
    let root = temp_test_root("legacy-context-checkpoint");
    let history = vec![test_message(MessageRole::User, "keep this verbatim")];
    write_context_checkpoint(&root, Some("session-a"), "legacy checkpoint", None)
        .expect("legacy checkpoint should write");

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history,).is_none());
}

#[test]
fn context_checkpoint_rejects_changed_history_prefix() {
    let root = temp_test_root("stale-context-prefix");
    let history = vec![
        test_message(MessageRole::User, "original requirement"),
        test_message(MessageRole::Assistant, "original answer"),
        test_message(MessageRole::User, "uncovered tail"),
    ];
    write_context_checkpoint(
        &root,
        Some("session-a"),
        "checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 2,
        }),
    )
    .expect("checkpoint should write");
    let mut changed = history.clone();
    changed[0].content = "edited requirement".to_string();

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &changed,).is_none());
    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history[..2],).is_none());
}

#[test]
fn context_checkpoint_rejects_manifest_content_mismatch() {
    let root = temp_test_root("stale-context-content");
    let history = vec![test_message(MessageRole::User, "requirement")];
    let path = write_context_checkpoint(
        &root,
        Some("session-a"),
        "checkpoint",
        Some(ContextCheckpointCoverage {
            history: &history,
            covered_messages: 1,
        }),
    )
    .expect("checkpoint should write");
    fs::write(path, "different checkpoint").expect("checkpoint should mutate");

    assert!(read_validated_context_checkpoint(&root, Some("session-a"), &history,).is_none());
}

#[test]
fn context_governor_preserves_canonical_history_and_latest_tool_round() {
    let tools = vec![ToolSpec::builtin(
        "file.read",
        "file",
        "Read a workspace file",
        ToolRisk::ReadOnly,
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    )];
    let mut runtime = start_agent_loop(
        TaskId("context-governor-test".to_string()),
        "current goal: finish the verified implementation",
        AgentRuntimeConfig { max_turns: 24 },
    );
    let current_user = runtime.messages.pop().expect("current user should exist");
    runtime.messages.push(Message {
        role: MessageRole::System,
        content: "artifact ".repeat(8_000),
        metadata: [("kind".to_string(), "artifact_manifest".to_string())]
            .into_iter()
            .collect(),
    });
    runtime.messages.push(test_message(
        MessageRole::User,
        "旧的中文需求".repeat(5_000),
    ));
    runtime.messages.push(test_message(
        MessageRole::Assistant,
        "old answer ".repeat(5_000),
    ));
    runtime.messages.push(current_user);
    for index in 0..5 {
        runtime.messages.push(Message {
                role: MessageRole::Assistant,
                content: format!("tool round {index}"),
                metadata: [(
                    "raw_tool_calls_json".to_string(),
                    format!(
                        r#"[{{"id":"call-{index}","type":"function","function":{{"name":"file_read","arguments":"{{\"path\":\"file-{index}\"}}"}}}}]"#
                    ),
                )]
                .into_iter()
                .collect(),
            });
        runtime.messages.push(Message {
            role: MessageRole::Tool,
            content: if index == 4 {
                "latest verified evidence".to_string()
            } else {
                "older evidence ".repeat(4_000)
            },
            metadata: [("tool_call_id".to_string(), format!("call-{index}"))]
                .into_iter()
                .collect(),
        });
    }
    let canonical_history = runtime.messages.clone();

    let (request, report) =
        model_request_for_turn_with_context_budget(&mut runtime, &tools, None, None, 16_384, 2_048);

    assert!(report.applied);
    assert!(report.hard_limit_satisfied);
    assert!(report.omitted_messages > 0);
    assert!(request.messages.iter().any(|message| {
        message
            .content
            .contains("current goal: finish the verified implementation")
    }));
    assert!(request.messages.iter().any(|message| {
        message.metadata.get("kind").map(String::as_str) == Some("artifact_manifest")
    }));
    let assistant_index = request
        .messages
        .iter()
        .position(|message| {
            message
                .metadata
                .get("raw_tool_calls_json")
                .is_some_and(|value| value.contains("call-4"))
        })
        .expect("latest assistant tool call should remain");
    let tool_index = request
        .messages
        .iter()
        .position(|message| {
            message.metadata.get("tool_call_id").map(String::as_str) == Some("call-4")
        })
        .expect("latest tool evidence should remain");
    assert!(assistant_index < tool_index);
    assert_eq!(runtime.messages, canonical_history);
}
