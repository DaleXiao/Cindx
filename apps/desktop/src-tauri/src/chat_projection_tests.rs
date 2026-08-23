use super::*;

#[test]
fn runtime_snapshot_matches_the_redacted_durable_projection() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "api_key=super-secret".to_string(),
        AgentRuntimeConfig::default(),
    );
    let effective_objective =
        "Initial request:\napi_key=super-secret\n\nAccepted steering 1:\nInspect token=private-value";
    let prepared_context = [
        (
            "effective_prompt_objective".to_string(),
            effective_objective.to_string(),
        ),
        ("steer_epoch".to_string(), "2".to_string()),
        ("prompt_contract_epoch".to_string(), "1".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    runtime.replace_prepared_task_state(agent_runtime::PreparedTaskState::from_run_context(
        &prepared_context,
        &runtime.user_prompt,
        agent_runtime::prompt_completion_intent(&prepared_context),
    ));
    runtime.messages.push(Message {
        role: MessageRole::Assistant,
        content: "<think>private scratchpad</think>\nFinished".to_string(),
        metadata: Metadata::new(),
    });

    let snapshot = capture_persistable_agent_task_state(&runtime);
    let mut persisted_messages = runtime.messages.clone();
    for message in &mut persisted_messages {
        if message.role == MessageRole::Assistant {
            message.content = sanitize_assistant_content(&message.content);
        }
        message.content = redact_sensitive_text(&message.content);
        message.metadata = redact_metadata(&message.metadata);
    }

    assert!(snapshot
        .restore_with_effective_objective(
            redact_sensitive_text(&runtime.user_prompt),
            persisted_messages,
            redact_sensitive_text(effective_objective),
        )
        .is_ok());
    assert!(snapshot
        .restore(runtime.user_prompt.clone(), runtime.messages.clone())
        .is_err());
}

#[test]
fn assistant_reasoning_control_only_message_is_sanitized_for_chat() {
    let event = Event {
        id: EventId("assistant-reasoning-control".to_string()),
        task_id: phase16_task_id(),
        sequence: 10,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "assistant message".to_string(),
        metadata: [
            ("role".to_string(), "assistant".to_string()),
            ("content".to_string(), "</think>".to_string()),
            ("raw_tool_calls_json".to_string(), "[]".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    let chat_message = message_view_from_event(&event).expect("message should project");
    assert_eq!(chat_message.content, "");
    let transcript_message = message_from_event(&event).expect("tool turn should remain");
    assert_eq!(transcript_message.content, "");
    assert!(transcript_message
        .metadata
        .contains_key("raw_tool_calls_json"));
}

#[test]
fn assistant_dsml_tool_protocol_is_sanitized_for_chat_and_transcript() {
    let event = Event {
        id: EventId("assistant-dsml-tool-protocol".to_string()),
        task_id: phase16_task_id(),
        sequence: 11,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "assistant message".to_string(),
        metadata: [
            ("role".to_string(), "assistant".to_string()),
            (
                "content".to_string(),
                concat!(
                    "<｜DSML｜tool_calls>",
                    "<｜DSML｜invoke name=\"shell_run\">",
                    "<｜DSML｜parameter name=\"command\" string=\"true\">pwd</｜DSML｜parameter>",
                    "</｜DSML｜invoke>",
                    "</｜DSML｜tool_calls>"
                )
                .to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    };

    let chat_message = message_view_from_event(&event).expect("message should project");
    assert_eq!(chat_message.content, "");
    let transcript_message = message_from_event(&event).expect("message should remain");
    assert_eq!(transcript_message.content, "");
}

#[test]
fn durable_internal_instruction_is_hidden_from_chat_but_restored_for_runtime() {
    let event = Event {
        id: EventId("internal-verification-instruction".to_string()),
        task_id: phase16_task_id(),
        sequence: 12,
        timestamp_ms: 100,
        kind: EventKind::MessageAdded,
        summary: "system message".to_string(),
        metadata: [
            ("role".to_string(), "system".to_string()),
            ("content".to_string(), "verify the mutation".to_string()),
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), "completion_verification".to_string()),
        ]
        .into_iter()
        .collect(),
    };

    assert!(message_from_event(&event).is_none());
    let runtime_message =
        runtime_message_from_event(&event).expect("runtime instruction should restore");
    assert_eq!(runtime_message.content, "verify the mutation");
    assert_eq!(
        runtime_message.metadata.get("kind").map(String::as_str),
        Some("completion_verification")
    );
}
