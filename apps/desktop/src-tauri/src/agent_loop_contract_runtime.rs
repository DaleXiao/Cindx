use crate::desktop_prelude::*;
use crate::{
    app_state::AppState,
    event_persistence::append_event,
    runtime_values::{agent_runtime_context_for_run, run_context_steer_epoch},
};
use agent_core::ToolEffectSemantics;
use agent_runtime::{
    pin_evidence_scope_tools, prompt_completion_intent, tool_matches_evidence_scope,
    EvidenceTargetAnchor, PromptCompletionIntent, PromptEvidenceScope, PromptToolRequirement,
};

pub(crate) fn planned_agent_tools(
    registry: &ToolRegistry,
    run_context: &Metadata,
    prompt: &str,
    context_window: u64,
) -> (Vec<ToolSpec>, PromptCompletionIntent) {
    let catalog = registry.specs();
    let completion_intent = prompt_completion_intent(run_context);
    let intent = tool_exposure_intent(
        run_context,
        &completion_intent.evidence_scopes,
        completion_intent.tool_requirement,
    );
    let mut tools = registry
        .exposure_plan_with_intent(prompt, context_window, &intent)
        .inline;
    pin_evidence_scope_tools(&completion_intent.evidence_scopes, &catalog, &mut tools);
    (tools, completion_intent)
}

fn tool_exposure_intent(
    run_context: &Metadata,
    evidence_scopes: &BTreeSet<PromptEvidenceScope>,
    completion_requirement: PromptToolRequirement,
) -> ToolExposureIntent {
    let mut intent = ToolExposureIntent::default();
    match run_context.get("task_class").map(String::as_str) {
        Some("coding") => {
            intent
                .preferred_namespaces
                .extend(["file", "shell"].map(str::to_string));
        }
        Some("research" | "retrieval") => {
            intent
                .preferred_namespaces
                .extend(["web", "file"].map(str::to_string));
        }
        Some("browser") => {
            intent.preferred_namespaces.insert("browser".to_string());
            intent.deferred_namespaces.insert("computer".to_string());
        }
        Some("computer") => {
            intent.preferred_namespaces.insert("computer".to_string());
            intent.deferred_namespaces.insert("browser".to_string());
        }
        _ => {}
    }
    for scope in evidence_scopes {
        match scope {
            PromptEvidenceScope::Workspace => {
                intent.preferred_namespaces.insert("file".to_string());
            }
            PromptEvidenceScope::External => {
                intent.preferred_namespaces.insert("web".to_string());
            }
            PromptEvidenceScope::Browser => {
                intent.preferred_namespaces.insert("browser".to_string());
                intent.deferred_namespaces.insert("computer".to_string());
            }
            PromptEvidenceScope::Visual => {
                intent
                    .preferred_namespaces
                    .extend(["browser", "computer"].map(str::to_string));
            }
        }
    }
    match merged_tool_requirement(run_context, completion_requirement) {
        PromptToolRequirement::ReadOnly => intent.prefer_read_only = true,
        PromptToolRequirement::Effects => intent.prefer_effects = true,
        PromptToolRequirement::None => {}
    }
    if run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true")
    {
        intent.required_tools.insert("image.generate".to_string());
    }
    intent
}

#[cfg(test)]
pub(crate) fn workspace_verification_policy_for_run_context(
    run_context: &Metadata,
) -> Result<WorkspaceVerificationPolicy, String> {
    workspace_verification_policy_for_run_context_with_requirement(
        run_context,
        prompt_completion_intent(run_context).tool_requirement,
    )
}

fn workspace_verification_policy_for_run_context_with_requirement(
    run_context: &Metadata,
    completion_requirement: PromptToolRequirement,
) -> Result<WorkspaceVerificationPolicy, String> {
    let configured = if let Some(serialized) = run_context.get("conductor_contract") {
        let contract = ConductorExecutionContract::from_json(serialized)?;
        if contract.verification_required {
            WorkspaceVerificationPolicy::RequiredAfterMutation
        } else {
            WorkspaceVerificationPolicy::NotRequired
        }
    } else if run_context
        .get("verification_required")
        .is_some_and(|value| value == "true")
    {
        WorkspaceVerificationPolicy::RequiredAfterMutation
    } else {
        WorkspaceVerificationPolicy::NotRequired
    };
    Ok(
        if configured.is_required() || completion_requirement == PromptToolRequirement::Effects {
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
    let completion_intent = prompt_completion_intent(run_context);
    apply_run_task_contract_with_completion_intent(
        runtime,
        run_context,
        tools,
        collaboration,
        &completion_intent,
    )
}

pub(crate) fn apply_run_task_contract_with_completion_intent(
    runtime: &mut agent_runtime::AgentLoopState,
    run_context: &Metadata,
    tools: &[ToolSpec],
    collaboration: Option<&AgentCollaboration>,
    completion_intent: &PromptCompletionIntent,
) -> Result<(), String> {
    let evidence_scopes = &completion_intent.evidence_scopes;
    AgentKernel::new(runtime, tools).merge_workspace_verification_policy(
        workspace_verification_policy_for_run_context_with_requirement(
            run_context,
            completion_intent.tool_requirement,
        )?,
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

    let prompt_capability_requirements =
        prompt_capability_requirements(run_context, tools, completion_intent.tool_requirement);
    AgentKernel::new(runtime, tools).replace_prompt_required_any_tool_successes(
        prompt_contract_epoch,
        prompt_capability_requirements,
    );

    let mut active_evidence_scopes = evidence_scopes.clone();
    if run_context.get("vision_required").map(String::as_str) == Some("true") {
        active_evidence_scopes.insert(PromptEvidenceScope::Visual);
    }
    let evidence_requirements = active_evidence_scopes
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
    let anchors = &completion_intent.target_anchors;
    let evidence_targets = active_evidence_scopes
        .iter()
        .map(|scope| {
            (
                scope.requirement_id().to_string(),
                anchors
                    .iter()
                    .filter(|anchor| evidence_anchor_matches_scope(anchor, *scope))
                    .cloned()
                    .collect(),
            )
        })
        .collect();
    AgentKernel::new(runtime, tools)
        .bind_prompt_evidence_targets(prompt_contract_epoch, evidence_targets);
    if active_evidence_scopes.contains(&PromptEvidenceScope::Workspace) {
        if let Some((message_index, receipt, observation)) =
            workspace_knowledge_receipt(&mut runtime.messages, prompt_contract_epoch)
        {
            let recorded = AgentKernel::new(runtime, tools)
                .record_prompt_context_evidence_for_requirement_at(
                    prompt_contract_epoch,
                    PromptEvidenceScope::Workspace.requirement_id(),
                    "knowledge_context",
                    &receipt,
                    &observation,
                );
            if recorded {
                if let Some(sequence) = runtime.task_contract.prompt_evidence_sequence(
                    prompt_contract_epoch,
                    PromptEvidenceScope::Workspace.requirement_id(),
                ) {
                    runtime.messages[message_index].metadata.insert(
                        "contract_evidence_sequence".to_string(),
                        sequence.to_string(),
                    );
                    runtime.messages[message_index].metadata.insert(
                        agent_runtime::CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY.to_string(),
                        serde_json::json!([sequence]).to_string(),
                    );
                }
            }
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

fn prompt_capability_requirements(
    run_context: &Metadata,
    tools: &[ToolSpec],
    completion: PromptToolRequirement,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut requirements = BTreeMap::new();
    let configured = configured_tool_requirement(run_context);
    match merged_tool_requirement(run_context, completion) {
        PromptToolRequirement::ReadOnly => {
            if configured != PromptToolRequirement::ReadOnly {
                return requirements;
            }
            let tools = tools
                .iter()
                .filter(|tool| substantive_read_tool(tool))
                .map(|tool| tool.name.clone())
                .collect::<BTreeSet<_>>();
            if !tools.is_empty() {
                requirements.insert("conductor_read_evidence".to_string(), tools);
            }
        }
        PromptToolRequirement::Effects => {
            let tools = tools
                .iter()
                .filter(|tool| {
                    tool.namespace != "meta"
                        && tool.effect_semantics != ToolEffectSemantics::ReadOnly
                })
                .map(|tool| tool.name.clone())
                .collect::<BTreeSet<_>>();
            if !tools.is_empty() {
                let id = if configured == PromptToolRequirement::Effects {
                    "conductor_effect"
                } else {
                    "prompt_effect"
                };
                requirements.insert(id.to_string(), tools);
            }
        }
        PromptToolRequirement::None => {}
    }
    requirements
}

fn merged_tool_requirement(
    run_context: &Metadata,
    completion_requirement: PromptToolRequirement,
) -> PromptToolRequirement {
    let configured = configured_tool_requirement(run_context);
    match (configured, completion_requirement) {
        (PromptToolRequirement::Effects, _) | (_, PromptToolRequirement::Effects) => {
            PromptToolRequirement::Effects
        }
        (PromptToolRequirement::ReadOnly, _) | (_, PromptToolRequirement::ReadOnly) => {
            PromptToolRequirement::ReadOnly
        }
        _ => PromptToolRequirement::None,
    }
}

fn configured_tool_requirement(run_context: &Metadata) -> PromptToolRequirement {
    match run_context.get("tool_requirement").map(String::as_str) {
        Some("effects") => PromptToolRequirement::Effects,
        Some("read_only") => PromptToolRequirement::ReadOnly,
        _ => PromptToolRequirement::None,
    }
}

fn evidence_anchor_matches_scope(
    anchor: &EvidenceTargetAnchor,
    scope: PromptEvidenceScope,
) -> bool {
    matches!(
        (anchor, scope),
        (
            EvidenceTargetAnchor::Workspace(_),
            PromptEvidenceScope::Workspace
        ) | (
            EvidenceTargetAnchor::ExternalUrl(_),
            PromptEvidenceScope::External
        ) | (
            EvidenceTargetAnchor::ExternalSubject(_),
            PromptEvidenceScope::External
        ) | (
            EvidenceTargetAnchor::ExternalUrl(_),
            PromptEvidenceScope::Browser
        )
    )
}

fn substantive_read_tool(tool: &ToolSpec) -> bool {
    tool.namespace != "meta"
        && tool.effect_semantics == ToolEffectSemantics::ReadOnly
        && !matches!(tool.name.as_str(), "file.list" | "browser.tabs")
}

fn workspace_knowledge_receipt(
    messages: &mut [Message],
    prompt_contract_epoch: u64,
) -> Option<(usize, String, String)> {
    messages
        .iter_mut()
        .enumerate()
        .rev()
        .find_map(|(message_index, message)| {
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
                    message_index,
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
