use crate::desktop_prelude::*;
use crate::{
    app_state::AppState,
    event_persistence::append_event,
    runtime_values::{agent_runtime_context_for_run, run_context_steer_epoch},
};

pub(crate) fn workspace_verification_policy_for_run_context(
    run_context: &Metadata,
) -> Result<WorkspaceVerificationPolicy, String> {
    if let Some(serialized) = run_context.get("conductor_contract") {
        let contract = ConductorExecutionContract::from_json(serialized)?;
        return Ok(if contract.verification_required {
            WorkspaceVerificationPolicy::RequiredAfterMutation
        } else {
            WorkspaceVerificationPolicy::NotRequired
        });
    }

    Ok(
        if run_context
            .get("verification_required")
            .is_some_and(|value| value == "true")
        {
            WorkspaceVerificationPolicy::RequiredAfterMutation
        } else {
            WorkspaceVerificationPolicy::NotRequired
        },
    )
}

pub(crate) fn apply_run_task_contract(
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
) -> Result<(), String> {
    AgentKernel::new(runtime, &[]).merge_workspace_verification_policy(
        workspace_verification_policy_for_run_context(run_context)?,
    );
    let steer_epoch = run_context_steer_epoch(run_context);
    let prompt_required_tools = if run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true")
    {
        ["image.generate"].as_slice()
    } else {
        &[]
    };
    AgentKernel::new(runtime, &[])
        .replace_prompt_required_tool_successes(steer_epoch, prompt_required_tools.iter().copied());
    Ok(())
}

pub(super) fn synchronize_noop_control_epoch_context(
    run_context: &mut Metadata,
    epoch: u64,
) -> Option<String> {
    run_context.insert("steer_epoch".to_string(), epoch.to_string());
    // A deleted/no-op steer advances control arbitration only. It must not
    // revise the task objective or erase already-satisfied tool evidence.
    agent_runtime_context_for_run(run_context)
}

pub(super) fn record_retained_agent_decision_after_noop_steer(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
) -> Result<(), String> {
    let mut metadata = run_context.clone();
    metadata.insert(
        "decision_source".to_string(),
        "retained_after_noop_steer".to_string(),
    );
    metadata.insert("decision_attempts".to_string(), "0".to_string());
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        metadata,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::TaskId;
    use agent_runtime::{start_agent_loop, AgentRuntimeConfig};

    #[test]
    fn resolved_noop_steer_synchronizes_execution_epoch_context() {
        let mut runtime = start_agent_loop(
            TaskId("noop-steer-epoch".to_string()),
            "Generate an image",
            AgentRuntimeConfig::default(),
        );
        let mut run_context = [
            ("steer_epoch".to_string(), "0".to_string()),
            ("image_generation_required".to_string(), "true".to_string()),
            (
                "configured_image_model".to_string(),
                "image-model".to_string(),
            ),
            (
                "configured_image_endpoint".to_string(),
                "https://example.invalid".to_string(),
            ),
        ]
        .into_iter()
        .collect::<Metadata>();

        apply_run_task_contract(&mut runtime, &run_context)
            .expect("initial image contract should apply");
        record_tool_outcome_with_risk(
            &mut runtime,
            "image.generate",
            r#"{"prompt":"lighthouse"}"#,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::UsesNetwork),
        );
        let runtime_context = synchronize_noop_control_epoch_context(&mut run_context, 4);

        assert_eq!(
            run_context.get("steer_epoch").map(String::as_str),
            Some("4")
        );
        assert!(runtime_context
            .as_deref()
            .is_some_and(|context| context.contains("image.generate")));
        let contract =
            serde_json::to_value(&runtime.task_contract).expect("task contract should serialize");
        assert_eq!(contract["promptRequirementEpoch"].as_u64(), Some(0));
        assert!(runtime
            .task_contract
            .required_tool_satisfied("image.generate"));
    }
}
