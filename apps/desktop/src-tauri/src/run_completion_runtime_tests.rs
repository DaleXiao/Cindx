use super::*;

#[test]
fn agent_runtime_never_completes_with_an_empty_model_answer() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "complete the task",
        AgentRuntimeConfig { max_turns: 6 },
    );
    let empty_response = || model_provider::ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: String::new(),
            metadata: Metadata::new(),
        },
        raw_tool_calls_json: None,
        tool_calls: Vec::new(),
        metadata: Metadata::new(),
    };

    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Retry { .. }
    ));
    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Retry { .. }
    ));
    assert!(matches!(
        advance_with_model_response(&mut runtime, empty_response(), &[]),
        AgentAdvance::Failed { .. }
    ));
}

#[test]
fn image_generation_run_cannot_complete_without_the_configured_tool() {
    let mut runtime = start_agent_loop(
        phase16_task_id(),
        "生成一张图片",
        AgentRuntimeConfig { max_turns: 6 },
    );
    let run_context = [("image_generation_required".to_string(), "true".to_string())]
        .into_iter()
        .collect();
    apply_run_task_contract(&mut runtime, &run_context, &[], None).expect("task contract applies");
    assert!(!runtime
        .task_contract
        .required_tool_satisfied("image.generate"));

    record_tool_outcome_with_risk(
        &mut runtime,
        "image.generate",
        r#"{"prompt":"cat"}"#,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::UsesNetwork),
    );

    assert!(runtime
        .task_contract
        .required_tool_satisfied("image.generate"));
}
