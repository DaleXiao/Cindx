use crate::desktop_prelude::*;
use crate::{
    adaptive_collaboration_execution::run_adaptive_collaboration,
    agent_query_commands::{active_agent_run_control, agent_run_should_stop},
    app_state::AppState,
    collaboration_execution::{
        collaboration_candidate_models, collaboration_run_should_interrupt,
        prioritize_collaboration_model, record_collaboration_stage_started,
        CollaborationCandidateSpec,
    },
    collaboration_stage_runtime::{record_collaboration_stage_finished, run_collaboration_stage},
    collaboration_worker_runtime::complete_collaboration_worker_with_tools,
    configuration_models::ProviderConfig,
    event_persistence::append_event,
    project_session_persistence::metadata_with_context,
    runtime_constants::COLLABORATION_MAX_OUTPUT_TOKENS,
    runtime_values::{run_context_steer_epoch, unique_id},
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_collaboration_candidates(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    collaboration_id: &str,
    prompt: &str,
    history: &[Message],
    models: &[String],
    allow_tools: bool,
    prompt_profile: Option<&ConductorPromptGenome>,
) -> Result<String, String> {
    let recent_context = collaboration_recent_context(history);
    let conductor_directive = prompt_profile.map(ConductorPromptGenome::conductor_directive);
    let declared_model_turns = prompt_profile
        .map(ConductorPromptGenome::effective_max_model_turns_per_step)
        .unwrap_or(DEFAULT_COLLABORATION_WORKER_TURNS);
    let max_model_turns = if allow_tools {
        WorkflowToolPolicy::ReadOnlyEvidence.effective_model_turn_budget(declared_model_turns)
    } else {
        declared_model_turns.max(1)
    };
    let declared_tool_calls = prompt_profile
        .map(ConductorPromptGenome::effective_max_tool_calls_per_step)
        .unwrap_or(MAX_COLLABORATION_WORKER_TOOL_CALLS);
    let max_tool_calls = if allow_tools {
        WorkflowToolPolicy::ReadOnlyEvidence.effective_tool_call_budget(declared_tool_calls)
    } else {
        0
    };
    let specs = models
        .iter()
        .enumerate()
        .map(|(index, model)| CollaborationCandidateSpec {
            stage: format!("candidate_{}", index + 1),
            model: model.clone(),
            prompt: build_collaboration_candidate_prompt(
                prompt,
                &recent_context,
                index,
                conductor_directive.as_deref(),
            ),
            request_id: unique_id("collaboration-model"),
        })
        .collect::<Vec<_>>();
    for spec in &specs {
        record_collaboration_stage_started(
            state,
            task_id,
            run_context,
            collaboration_id,
            &spec.stage,
            &ModelRole::Planner,
            &spec.model,
            &spec.request_id,
            &Metadata::new(),
        )?;
    }

    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    let jobs = specs
        .iter()
        .map(|spec| {
            let app = app.clone();
            let config = config.clone();
            let task_id = task_id.clone();
            let workspace_root = workspace_root.to_path_buf();
            let run_context = run_context.clone();
            let collaboration_id = collaboration_id.to_string();
            let stage = spec.stage.clone();
            let model = spec.model.clone();
            let prompt = spec.prompt.clone();
            let cancellation = cancellation.clone();
            let worker_access = CollaborationWorkerAccess::new(
                stage.clone(),
                if allow_tools {
                    WorkflowToolPolicy::ReadOnlyExploration
                } else {
                    WorkflowToolPolicy::None
                },
            );
            Box::new(move |branch_cancellation| {
                complete_collaboration_worker_with_tools(
                    app,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    stage,
                    ModelRole::Planner,
                    model,
                    prompt,
                    worker_access,
                    max_model_turns,
                    max_tool_calls,
                    COLLABORATION_MAX_OUTPUT_TOKENS,
                    cancellation,
                    Some(branch_cancellation),
                )
            }) as CancellableParallelJob<CollaborationCompletion>
        })
        .collect::<Vec<_>>();
    let effort = run_context
        .get("agent_effort")
        .map(String::as_str)
        .unwrap_or("auto");
    let execution_contract = run_context
        .get("conductor_contract")
        .map(String::as_str)
        .and_then(|contract| ConductorExecutionContract::from_json(contract).ok())
        .map(|contract| match prompt_profile {
            Some(profile) => contract.with_prompt_commit_strategy(profile.commit_strategy),
            None => contract,
        });
    let required_successes = execution_contract
        .as_ref()
        .map(|contract| contract.required_successes_for_layer(specs.len()))
        .unwrap_or_else(|| collaboration_candidate_quorum(specs.len(), effort));
    let quorum_grace = execution_contract
        .as_ref()
        .map(|contract| Duration::from_millis(contract.quorum_grace_ms()))
        .unwrap_or_else(|| collaboration_candidate_quorum_grace(effort));
    let execution = run_model_jobs_until_quorum_interruptible(
        "candidate-worker",
        jobs,
        InterruptibleQuorumPolicy::new(required_successes, quorum_grace, Duration::from_millis(20)),
        |completion| {
            completion
                .content
                .as_ref()
                .is_some_and(|content| !content.trim().is_empty())
        },
        || {
            cancellation
                .as_ref()
                .is_some_and(collaboration_run_should_interrupt)
        },
    );
    let interrupted_for_steer = execution.interrupted
        && cancellation
            .as_ref()
            .is_some_and(|control| control.has_pending_steer());
    let execution = execution.execution;
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration candidate quorum resolved",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "quorum_required".to_string(),
                        required_successes.to_string(),
                    ),
                    (
                        "quorum_successful".to_string(),
                        execution.successful.to_string(),
                    ),
                    (
                        "quorum_reached".to_string(),
                        execution.quorum_reached.to_string(),
                    ),
                    (
                        "cancelled_stragglers".to_string(),
                        execution.cancelled_stragglers.to_string(),
                    ),
                    (
                        "quorum_grace_ms".to_string(),
                        quorum_grace.as_millis().to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
    if interrupted_for_steer
        || cancellation
            .as_ref()
            .is_some_and(|control| control.has_pending_steer())
    {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let completions = execution
        .results
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|error| {
                CollaborationCompletion::failed(format!(
                    "collaboration candidate worker failed: {error}"
                ))
            })
        })
        .collect::<Vec<_>>();
    if cancellation.as_ref().is_some_and(agent_run_should_stop) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }

    let mut candidates = Vec::new();
    for (spec, completion) in specs.iter().zip(&completions) {
        record_collaboration_stage_finished(
            state,
            task_id,
            run_context,
            collaboration_id,
            &spec.stage,
            &ModelRole::Planner,
            &spec.model,
            &spec.request_id,
            completion,
            &Metadata::new(),
        )?;
        if let Some(content) = completion
            .content
            .as_ref()
            .filter(|content| !content.trim().is_empty())
        {
            candidates.push((
                spec.model.clone(),
                collaboration_step_result(&spec.stage, &spec.model, content, &completion.evidence),
            ));
        }
    }

    if candidates.is_empty() {
        return Err("all collaboration candidates were unavailable".to_string());
    }
    if candidates.len() == 1 {
        return Ok(candidates.remove(0).1);
    }
    let arbiter_result = run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        "arbiter",
        ModelRole::Reviewer,
        &config.model_for_role(&ModelRole::Reviewer),
        build_collaboration_arbiter_prompt(prompt, &candidates, conductor_directive.as_deref()),
    );
    match arbiter_result {
        Ok(guidance) => Ok(guidance),
        Err(error) => {
            if let Ok(mut store) = state.store.lock() {
                let _ = append_event(
                    &mut store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Collaboration arbiter degraded",
                    metadata_with_context(
                        [
                            ("collaboration_id".to_string(), collaboration_id.to_string()),
                            ("status".to_string(), "degraded".to_string()),
                            ("candidate_count".to_string(), candidates.len().to_string()),
                            (
                                "error".to_string(),
                                truncate_for_collaboration(&error, 2_000),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                );
            }
            Ok(collaboration_candidate_handoff(prompt, &candidates, &error))
        }
    }
}

pub(crate) fn collaboration_candidate_quorum(candidate_count: usize, effort: &str) -> usize {
    match candidate_count {
        0 => 0,
        1 => 1,
        _ if effort == "fast" => 1,
        count if effort == "pro" => count.saturating_mul(2).saturating_add(2) / 3,
        count => count.saturating_sub(1).max(2).min(count),
    }
}

pub(crate) fn collaboration_candidate_quorum_grace(effort: &str) -> Duration {
    match effort {
        "fast" => Duration::from_millis(100),
        "pro" => Duration::from_millis(1_500),
        _ => Duration::from_millis(400),
    }
}

pub(crate) fn collaboration_candidate_handoff(
    prompt: &str,
    candidates: &[(String, String)],
    arbiter_error: &str,
) -> String {
    let candidate_reports = candidates
        .iter()
        .enumerate()
        .map(|(index, (_, report))| {
            format!(
                "Candidate {}:\n{}",
                index + 1,
                truncate_for_collaboration(report, 12_000)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "INTERNAL TEAM HANDOFF: Independent candidate work completed, but the arbiter was unavailable. The downstream executor must reconcile conflicts, verify claims and artifacts, and produce the final answer itself. Do not expose this internal note to the user.\n\nUser objective:\n{}\n\nArbiter failure:\n{}\n\nCandidate work:\n{}",
        truncate_for_collaboration(prompt, 4_000),
        truncate_for_collaboration(arbiter_error, 1_000),
        truncate_for_collaboration(&candidate_reports, 30_000),
    )
}

pub(crate) fn collaboration_error_blocks_executor(error: &str) -> bool {
    error.starts_with(WORKFLOW_SAFETY_ERROR_PREFIX) || error == COLLABORATION_STEER_INTERRUPTED
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_collaboration(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    policy: &OrchestrationPolicy,
    prompt: &str,
    history: &[Message],
) -> Result<Option<AgentCollaboration>, String> {
    let OrchestrationPolicy::BestOfN { candidates } = policy else {
        return Ok(None);
    };
    let id = unique_id("collab");
    let agent_budget = collaboration_agent_budget(*candidates);
    let mut models = collaboration_candidate_models(config, agent_budget);
    prioritize_collaboration_model(
        &mut models,
        run_context.get("agent_model").map(String::as_str),
        agent_budget,
    );
    let fallback_models = collaboration_fallback_models(&models, agent_budget);
    let bounded = run_context.get("collaboration_profile").map(String::as_str) == Some("bounded");
    let effort = run_context
        .get("agent_effort")
        .cloned()
        .unwrap_or_else(|| "auto".to_string());
    let bounded_profile = (bounded && config.prompt_evolution_enabled && effort != "fast")
        .then(|| run_context.get("prompt_genome"))
        .flatten()
        .and_then(|encoded| serde_json::from_str::<ConductorPromptGenome>(encoded).ok())
        .map(|profile| profile.with_effort_delivery_contract(&effort));
    if let Some(profile) = bounded_profile.as_ref() {
        let prompt_genome = serde_json::to_string(profile)
            .map_err(|error| format!("failed to serialize prompt genome: {error}"))?;
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Conductor prompt profile selected",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), id.clone()),
                    ("prompt_profile".to_string(), profile.id.clone()),
                    ("prompt_effort".to_string(), effort.clone()),
                    (
                        "prompt_generation".to_string(),
                        profile.generation.to_string(),
                    ),
                    ("prompt_genome".to_string(), prompt_genome),
                    (
                        "prompt_selection_mode".to_string(),
                        run_context
                            .get("prompt_profile_source")
                            .cloned()
                            .unwrap_or_else(|| "run_strategy".to_string()),
                    ),
                    (
                        "prompt_evolution_status".to_string(),
                        run_context
                            .get("prompt_rollout_status")
                            .cloned()
                            .unwrap_or_else(|| "unavailable".to_string()),
                    ),
                    ("prompt_champion".to_string(), String::new()),
                    ("prompt_evolution_enabled".to_string(), "true".to_string()),
                    ("collaboration_profile".to_string(), "bounded".to_string()),
                    (
                        "prompt_objective".to_string(),
                        truncate_for_collaboration(prompt, 6_000),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    let outcome_result = if bounded {
        run_collaboration_candidates(
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            &id,
            prompt,
            history,
            &fallback_models,
            false,
            bounded_profile.as_ref(),
        )
        .map(AdaptiveCollaborationOutcome::direct)
    } else {
        run_adaptive_collaboration(
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            &id,
            prompt,
            history,
            &models,
            agent_budget,
        )
        .or_else(|error| {
            if error == COLLABORATION_STEER_INTERRUPTED {
                return Err(error);
            }
            if collaboration_error_blocks_executor(&error) {
                return Err(error);
            }
            let control =
                active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
            if control
                .as_ref()
                .is_some_and(|control| control.should_stop())
            {
                return Err(error);
            }
            let fallback_allowed = control
                .as_ref()
                .is_none_or(|control| control.progress().remaining.as_secs() >= 90);
            if let Ok(mut store) = state.store.lock() {
                let _ = append_event(
                    &mut store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Collaboration workflow failed",
                    metadata_with_context(
                        [
                            ("collaboration_id".to_string(), id.clone()),
                            (
                                "workflow_schema".to_string(),
                                WORKFLOW_IR_SCHEMA.to_string(),
                            ),
                            ("status".to_string(), "failed".to_string()),
                            ("fallback_used".to_string(), fallback_allowed.to_string()),
                            (
                                "error".to_string(),
                                truncate_for_collaboration(&error, 2_000),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                );
            }
            if !fallback_allowed {
                return Err(format!(
                    "{error}; adaptive fallback skipped because less than 90 seconds remain"
                ));
            }
            run_collaboration_candidates(
                app,
                state,
                config,
                task_id,
                workspace_root,
                run_context,
                &id,
                prompt,
                history,
                &fallback_models,
                true,
                None,
            )
            .map(AdaptiveCollaborationOutcome::direct)
        })
    };
    let mut outcome = outcome_result?;
    let steer_epoch = run_context_steer_epoch(run_context);
    outcome.grounding_receipts.retain(|receipt| {
        receipt.steer_epoch == steer_epoch
            && receipt.collaboration_id == id
            && !receipt.observation.trim().is_empty()
    });
    outcome
        .grounding_receipts
        .truncate(crate::collaboration_service::COLLABORATION_GROUNDING_RECEIPT_MAX_ENTRIES);
    Ok(Some(AgentCollaboration {
        id,
        policy: policy.label().to_string(),
        guidance: outcome.guidance,
        execution_contract: outcome.execution_contract,
        evidence_packet: outcome.evidence_packet,
        grounding_receipts: outcome.grounding_receipts,
        candidate_models: models,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_collaboration_or_degrade(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    policy: &OrchestrationPolicy,
    prompt: &str,
    history: &[Message],
) -> Result<Option<AgentCollaboration>, String> {
    match prepare_agent_collaboration(
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        policy,
        prompt,
        history,
    ) {
        Ok(collaboration) => Ok(collaboration),
        Err(error) if error == COLLABORATION_STEER_INTERRUPTED => Ok(None),
        Err(error) if collaboration_error_blocks_executor(&error) => Err(error),
        Err(error) => {
            let control =
                active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
            if control.as_ref().is_some_and(agent_run_should_stop) {
                return Err(error);
            }
            let best_known = control
                .as_ref()
                .and_then(|control| control.best_guidance_result());
            let guidance = best_known
                .as_ref()
                .filter(|result| !result.content.trim().is_empty())
                .map(|result| {
                    format!(
                        "INTERNAL DEGRADED COLLABORATION HANDOFF: Team orchestration did not finish, but useful work was preserved from stage {} with quality {}. Independently verify it before use and do not expose this note to the user.\n\n{}",
                        result.stage,
                        result.quality.as_str(),
                        result.content,
                    )
                });
            if let Ok(mut store) = state.store.lock() {
                let _ = append_event(
                    &mut store,
                    task_id,
                    EventKind::TaskStatusChanged,
                    "Collaboration degraded to executor",
                    metadata_with_context(
                        [
                            ("status".to_string(), "degraded".to_string()),
                            ("fallback_used".to_string(), "executor".to_string()),
                            ("policy".to_string(), policy.label().to_string()),
                            (
                                "best_known_stage".to_string(),
                                best_known
                                    .as_ref()
                                    .map(|result| result.stage.clone())
                                    .unwrap_or_default(),
                            ),
                            (
                                "error".to_string(),
                                truncate_for_collaboration(&error, 2_000),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                        run_context,
                    ),
                );
            }
            Ok(guidance.map(|guidance| AgentCollaboration {
                id: unique_id("collab-degraded"),
                policy: policy.label().to_string(),
                guidance,
                execution_contract: None,
                evidence_packet: None,
                grounding_receipts: Vec::new(),
                candidate_models: Vec::new(),
            }))
        }
    }
}

pub(crate) fn append_agent_collaboration_context(
    history: &mut Vec<Message>,
    collaboration: &AgentCollaboration,
) {
    let mut guidance = agent_runtime::AgentExecutionGuidance::new(
        collaboration.id.clone(),
        collaboration.guidance.clone(),
        collaboration.execution_contract.clone(),
    );
    if let Some(packet) = collaboration.evidence_packet.clone() {
        guidance = guidance.with_evidence_packet(packet);
    }
    guidance.append_to_history(history);
    if !collaboration.grounding_receipts.is_empty() {
        let content = serde_json::json!({
            "type": "collaboration_tool_evidence",
            "trust": "untrusted_tool_data",
            "observations": collaboration
                .grounding_receipts
                .iter()
                .map(|receipt| serde_json::json!({
                    "step": receipt.source_step,
                    "callId": receipt.tool_call_id,
                    "tool": receipt.tool_name,
                    "inputFingerprint": receipt.input_fingerprint,
                    "observation": receipt.observation,
                }))
                .collect::<Vec<_>>(),
        })
        .to_string();
        let grounding_epoch = collaboration.grounding_receipts[0].steer_epoch;
        let grounding_tools = collaboration
            .grounding_receipts
            .iter()
            .map(|receipt| receipt.tool_name.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        history.push(Message {
            role: MessageRole::Reviewer,
            content,
            metadata: [
                ("internal".to_string(), "true".to_string()),
                (
                    "kind".to_string(),
                    "collaboration_tool_evidence".to_string(),
                ),
                (
                    "evidence_schema".to_string(),
                    crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA.to_string(),
                ),
                ("grounding_candidate".to_string(), "true".to_string()),
                (
                    "prompt_contract_epoch".to_string(),
                    grounding_epoch.to_string(),
                ),
                (
                    "grounding_tools_json".to_string(),
                    serde_json::to_string(&grounding_tools).unwrap_or_else(|_| "[]".to_string()),
                ),
                ("collaboration_id".to_string(), collaboration.id.clone()),
            ]
            .into_iter()
            .collect(),
        });
    }
}

#[cfg(test)]
#[path = "agent_collaboration_runtime_tests.rs"]
mod collaboration_context_tests;
