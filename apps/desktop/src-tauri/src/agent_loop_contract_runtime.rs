use crate::desktop_prelude::*;
use crate::{
    agent_grounding_policy::{tool_matches_evidence_scope, PromptEvidenceScope},
    app_state::AppState,
    event_persistence::append_event,
    runtime_values::{agent_runtime_context_for_run, run_context_steer_epoch},
};

#[cfg(test)]
use crate::agent_grounding_policy::{pin_prompt_evidence_tools, prompt_evidence_scopes};

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

#[cfg(test)]
pub(crate) fn apply_run_task_contract(
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    tools: &[ToolSpec],
    collaboration: Option<&AgentCollaboration>,
) -> Result<(), String> {
    let evidence_scopes = prompt_evidence_scopes(run_context);
    apply_run_task_contract_with_evidence_scopes(
        runtime,
        run_context,
        tools,
        collaboration,
        &evidence_scopes,
    )
}

pub(crate) fn apply_run_task_contract_with_evidence_scopes(
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    tools: &[ToolSpec],
    collaboration: Option<&AgentCollaboration>,
    evidence_scopes: &BTreeSet<PromptEvidenceScope>,
) -> Result<(), String> {
    AgentKernel::new(runtime, tools).merge_workspace_verification_policy(
        workspace_verification_policy_for_run_context(run_context)?,
    );
    let steer_epoch = run_context_steer_epoch(run_context);
    let prompt_contract_epoch = run_context
        .get("prompt_contract_epoch")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(steer_epoch);
    let prompt_required_tools = if run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true")
    {
        ["image.generate"].as_slice()
    } else {
        &[]
    };
    AgentKernel::new(runtime, tools).replace_prompt_required_tool_successes(
        prompt_contract_epoch,
        prompt_required_tools.iter().copied(),
    );

    let evidence_requirements = evidence_scopes
        .iter()
        .map(|scope| {
            (
                scope.requirement_id().to_string(),
                tools
                    .iter()
                    .filter(|tool| tool_matches_evidence_scope(tool, *scope))
                    .map(|tool| tool.name.clone())
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    AgentKernel::new(runtime, tools)
        .replace_prompt_evidence_requirements(prompt_contract_epoch, evidence_requirements.clone());
    if evidence_scopes.contains(&PromptEvidenceScope::Workspace) {
        if let Some((receipt, observation)) =
            workspace_knowledge_receipt(&mut runtime.messages, prompt_contract_epoch)
        {
            AgentKernel::new(runtime, tools).record_prompt_context_evidence_for_requirement_at(
                prompt_contract_epoch,
                PromptEvidenceScope::Workspace.requirement_id(),
                "knowledge_context",
                &receipt,
                &observation,
            );
        }
    }
    for (requirement_id, source, receipt, observation) in persisted_collaboration_receipts(
        &runtime.messages,
        prompt_contract_epoch,
        &evidence_requirements,
        collaboration.map(|collaboration| collaboration.id.as_str()),
    ) {
        AgentKernel::new(runtime, tools).record_prompt_evidence_for_requirement_at(
            prompt_contract_epoch,
            &requirement_id,
            &source,
            &receipt,
            &observation,
        );
    }
    Ok(())
}

fn workspace_knowledge_receipt(
    messages: &mut [Message],
    prompt_contract_epoch: u64,
) -> Option<(String, String)> {
    messages.iter_mut().rev().find_map(|message| {
        let selected_count = message
            .metadata
            .get("selected_count")?
            .parse::<usize>()
            .ok()?;
        let trusted = message.role == MessageRole::Reviewer
            && message.metadata.get("internal").map(String::as_str) == Some("true")
            && message.metadata.get("kind").map(String::as_str) == Some("knowledge_context")
            && message
                .metadata
                .get("context_source_schema")
                .map(String::as_str)
                == Some(agent_runtime::CONTEXT_SOURCE_SCHEMA)
            && selected_count > 0;
        trusted.then(|| {
            message
                .metadata
                .insert("required_grounding".to_string(), "true".to_string());
            message.metadata.insert(
                "prompt_contract_epoch".to_string(),
                prompt_contract_epoch.to_string(),
            );
            message.metadata.insert(
                "requirement_id".to_string(),
                PromptEvidenceScope::Workspace.requirement_id().to_string(),
            );
            message.metadata.insert(
                "requirement_ids_json".to_string(),
                r#"["workspace_grounding"]"#.to_string(),
            );
            (
                format!("selected_count={selected_count}"),
                message.content.clone(),
            )
        })
    })
}

fn persisted_collaboration_receipts(
    messages: &[Message],
    prompt_contract_epoch: u64,
    requirements: &BTreeMap<String, BTreeSet<String>>,
    expected_collaboration_id: Option<&str>,
) -> Vec<(String, String, String, String)> {
    let mut credited = BTreeSet::new();
    let mut receipts = Vec::new();
    for message in messages.iter().rev() {
        let epoch = message
            .metadata
            .get("prompt_contract_epoch")
            .and_then(|value| value.parse::<u64>().ok());
        let Some(epoch) = epoch else {
            continue;
        };
        if message.role != MessageRole::Reviewer
            || message.metadata.get("internal").map(String::as_str) != Some("true")
            || message.metadata.get("kind").map(String::as_str)
                != Some("collaboration_tool_evidence")
            || message.metadata.get("evidence_schema").map(String::as_str)
                != Some(crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA)
            || message
                .metadata
                .get("grounding_candidate")
                .map(String::as_str)
                != Some("true")
            || epoch != prompt_contract_epoch
        {
            continue;
        }
        let Some(tools) = message
            .metadata
            .get("grounding_tools_json")
            .and_then(|value| serde_json::from_str::<Vec<String>>(value).ok())
        else {
            continue;
        };
        let Some(observations) = serde_json::from_str::<serde_json::Value>(&message.content)
            .ok()
            .and_then(|value| {
                value
                    .get("observations")
                    .and_then(|value| value.as_array())
                    .cloned()
            })
        else {
            continue;
        };
        let Some(collaboration_id) = message
            .metadata
            .get("collaboration_id")
            .map(|value| value.trim().to_string())
        else {
            continue;
        };
        if collaboration_id.is_empty()
            || expected_collaboration_id.is_some_and(|expected| expected != collaboration_id)
        {
            continue;
        }
        for (requirement_id, allowed_tools) in requirements {
            if credited.contains(requirement_id) {
                continue;
            }
            let Some((tool, observation)) = observations.iter().find_map(|observation| {
                let tool = observation.get("tool")?.as_str()?;
                let output = observation.get("observation")?.as_str()?;
                (tools.iter().any(|candidate| candidate == tool)
                    && allowed_tools.contains(tool)
                    && !output.trim().is_empty())
                .then_some((tool, output))
            }) else {
                continue;
            };
            credited.insert(requirement_id.clone());
            receipts.push((
                requirement_id.clone(),
                format!("collaboration:{collaboration_id}"),
                format!(
                    "schema={};tool={tool}",
                    crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA
                ),
                observation.to_string(),
            ));
        }
    }
    receipts
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
#[path = "agent_loop_contract_runtime_tests.rs"]
mod tests;
