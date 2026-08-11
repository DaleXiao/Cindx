use super::*;

pub(super) struct AdaptiveCollaborationSetup {
    pub(super) workflow_started_at_ms: u64,
    pub(super) conductor_model: String,
    pub(super) role_hints: ConductorRoleHints,
    pub(super) effort: String,
    pub(super) policy: String,
    pub(super) execution_contract: ConductorExecutionContract,
    pub(super) resume_key: String,
    pub(super) workflow_checkpoint: Option<WorkflowExecutionCheckpoint>,
    pub(super) checkpoint_resumable: bool,
    pub(super) resumed_from_workflow_id: Option<String>,
    pub(super) resumed_from_checkpoint: bool,
    pub(super) prior: Option<WorkflowTopologyPrior>,
    pub(super) route_workflow_proposal: Option<WorkflowPlanProposal>,
    pub(super) selection_mode: String,
    pub(super) prompt_genome: ConductorPromptGenome,
    pub(super) prompt_genome_json: String,
    pub(super) shared_memory: String,
    pub(super) cancellation: Option<Arc<AgentRunControl>>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_adaptive_collaboration(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    prompt: &str,
    history: &[Message],
    models: &[String],
    agent_budget: usize,
) -> Result<AdaptiveCollaborationSetup, String> {
    if models.is_empty() {
        return Err("adaptive collaboration has no configured worker models".to_string());
    }
    let workflow_started_at_ms = current_time_millis();
    let conductor_model = config.model_for_conductor();
    let role_hints = collaboration_role_hints(config, models);
    let validation_models = crate::agent_conductor_runtime::unique_configured_models(
        &crate::workflow_routing_runtime::model_candidates_for_config(config),
    );
    let effort = run_context
        .get("agent_effort")
        .cloned()
        .unwrap_or_else(|| "auto".to_string());
    let policy = run_context
        .get("collaboration_policy")
        .cloned()
        .unwrap_or_else(|| "best_of_n".to_string());
    let base_execution_contract = run_context
        .get("conductor_contract")
        .map(String::as_str)
        .map(ConductorExecutionContract::from_json)
        .transpose()?
        .unwrap_or_else(|| {
            ConductorExecutionContract::from_routing(
                &RoutingContext::from_prompt(prompt, Vec::new()),
                &effort,
                parse_policy(&policy).unwrap_or(OrchestrationPolicy::AutoRouter),
            )
        });
    let (resume_key, loaded_checkpoint) = load_workflow_checkpoint_for_run(
        state,
        task_id,
        run_context,
        prompt,
        &effort,
        &policy,
        &validation_models,
    )?;
    let checkpoint_resumable = loaded_checkpoint
        .as_ref()
        .is_some_and(|loaded| loaded.resumable);
    let mut workflow_checkpoint = loaded_checkpoint.map(|loaded| loaded.checkpoint);
    let checkpoint_loaded = workflow_checkpoint.is_some();
    let resumed_from_workflow_id = workflow_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.plan.workflow_id.clone());
    if checkpoint_resumable {
        let checkpoint = workflow_checkpoint
            .as_mut()
            .expect("resumable workflow checkpoint must be loaded");
        let prior_workflow_id = checkpoint.plan.workflow_id.clone();
        rebind_checkpoint_grounding_provenance(checkpoint, &prior_workflow_id, collaboration_id)?;
        checkpoint.plan.workflow_id = collaboration_id.to_string();
        let additional_turns = checkpoint.plan.budget.max_model_turns_per_step;
        checkpoint.continue_with_budget(additional_turns, workflow_started_at_ms);
    }
    let resumed_from_checkpoint = checkpoint_resumable;
    let prior = if checkpoint_loaded {
        None
    } else {
        workflow_prior_for_run(state, run_context, models, agent_budget)?
    };
    let route_workflow_proposal = route_workflow_proposal_from_context(
        run_context,
        checkpoint_loaded,
        &validation_models,
    )?;
    let strategy_genome = (!checkpoint_loaded)
        .then(|| run_context.get("prompt_genome"))
        .flatten()
        .and_then(|encoded| serde_json::from_str::<ConductorPromptGenome>(encoded).ok());
    let selection_mode = if resumed_from_checkpoint {
        "checkpoint_resume".to_string()
    } else if checkpoint_loaded {
        "checkpoint_owner_handoff".to_string()
    } else if strategy_genome.is_some() {
        run_context
            .get("prompt_profile_source")
            .cloned()
            .unwrap_or_else(|| "run_strategy".to_string())
    } else {
        "baseline".to_string()
    };
    let mut prompt_genome = workflow_checkpoint
        .as_ref()
        .and_then(|checkpoint| {
            serde_json::from_str::<ConductorPromptGenome>(&checkpoint.prompt_genome_json).ok()
        })
        .or(strategy_genome)
        .unwrap_or_else(|| ConductorPromptGenome::seed_for_effort(&effort));
    if resumed_from_checkpoint {
        if let Some(profile) = workflow_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.plan.prompt_profile.clone())
        {
            prompt_genome.id = profile;
        }
    }
    let selected_prompt_genome = prompt_genome.clone();
    prompt_genome = prompt_genome
        .with_effort_delivery_contract(&effort)
        .with_verification_requirement(base_execution_contract.verification_required);
    let prompt_delivery_contract_applied = prompt_genome != selected_prompt_genome;
    let execution_contract =
        base_execution_contract.with_prompt_commit_strategy(prompt_genome.commit_strategy);
    let prompt_genome_json = serde_json::to_string(&prompt_genome)
        .map_err(|error| format!("failed to serialize prompt genome: {error}"))?;
    record_prompt_profile_selection(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        &effort,
        &resume_key,
        resumed_from_checkpoint,
        &selection_mode,
        prompt_delivery_contract_applied,
        &prompt_genome,
        &prompt_genome_json,
    )?;

    let shared_memory = collaboration_context_for_genome(history, prompt_genome.context_policy);
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    if collaboration_steer_pending(cancellation.as_ref()) {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    Ok(AdaptiveCollaborationSetup {
        workflow_started_at_ms,
        conductor_model,
        role_hints,
        effort,
        policy,
        execution_contract,
        resume_key,
        workflow_checkpoint,
        checkpoint_resumable,
        resumed_from_workflow_id,
        resumed_from_checkpoint,
        prior,
        route_workflow_proposal,
        selection_mode,
        prompt_genome,
        prompt_genome_json,
        shared_memory,
        cancellation,
    })
}

pub(super) fn route_workflow_proposal_from_context(
    run_context: &Metadata,
    resumed_from_checkpoint: bool,
    allowed_models: &[String],
) -> Result<Option<WorkflowPlanProposal>, String> {
    if resumed_from_checkpoint {
        return Ok(None);
    }
    let Some(encoded) = run_context.get("conductor_workflow_proposal") else {
        return Ok(None);
    };
    let expected_sha256 = sha256_hex(encoded.as_bytes());
    if run_context
        .get("conductor_workflow_proposal_sha256")
        .map(String::as_str)
        != Some(expected_sha256.as_str())
        || run_context
            .get("conductor_workflow_plan_source")
            .map(String::as_str)
            != Some("run_decision")
    {
        return Err("run-decision workflow proposal receipt is inconsistent".to_string());
    }
    let decision = serde_json::from_str::<AgentRunDecision>(
        run_context
            .get("run_decision")
            .ok_or_else(|| "run-decision workflow proposal is missing its decision".to_string())?,
    )
    .map_err(|error| format!("run-decision workflow proposal has an invalid decision: {error}"))?;
    decision.validate(allowed_models, MAX_ADAPTIVE_WORKFLOW_AGENTS)?;
    let proposal = serde_json::from_str::<WorkflowPlanProposal>(encoded)
        .map_err(|error| format!("run-decision workflow proposal is invalid: {error}"))?;
    proposal.validate(&decision, allowed_models)?;
    Ok(Some(proposal))
}

#[allow(clippy::too_many_arguments)]
fn record_prompt_profile_selection(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    effort: &str,
    resume_key: &str,
    resumed_from_checkpoint: bool,
    selection_mode: &str,
    prompt_delivery_contract_applied: bool,
    prompt_genome: &ConductorPromptGenome,
    prompt_genome_json: &str,
) -> Result<(), String> {
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
                ("collaboration_id".to_string(), collaboration_id.to_string()),
                ("prompt_profile".to_string(), prompt_genome.id.clone()),
                ("prompt_effort".to_string(), effort.to_string()),
                (
                    "prompt_generation".to_string(),
                    prompt_genome.generation.to_string(),
                ),
                ("prompt_genome".to_string(), prompt_genome_json.to_string()),
                (
                    "prompt_selection_mode".to_string(),
                    selection_mode.to_string(),
                ),
                (
                    "prompt_delivery_contract_applied".to_string(),
                    prompt_delivery_contract_applied.to_string(),
                ),
                ("workflow_resume_key".to_string(), resume_key.to_string()),
                (
                    "prompt_evolution_status".to_string(),
                    if resumed_from_checkpoint {
                        "checkpoint_resume".to_string()
                    } else {
                        run_context
                            .get("prompt_rollout_status")
                            .cloned()
                            .unwrap_or_else(|| "unavailable".to_string())
                    },
                ),
                ("prompt_champion".to_string(), String::new()),
                (
                    "prompt_evolution_enabled".to_string(),
                    config.prompt_evolution_enabled.to_string(),
                ),
                (
                    "prompt_objective".to_string(),
                    run_context
                        .get("effective_prompt_objective")
                        .or_else(|| run_context.get("prompt_objective"))
                        .cloned()
                        .unwrap_or_default(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}
