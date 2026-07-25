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
    pub(super) resumed_from_workflow_id: Option<String>,
    pub(super) resumed_from_checkpoint: bool,
    pub(super) prior: Option<WorkflowTopologyPrior>,
    pub(super) evolution: Option<PromptEvolutionEvaluation>,
    pub(super) selection_mode: String,
    pub(super) prompt_genome: ConductorPromptGenome,
    pub(super) prompt_genome_json: String,
    pub(super) shared_memory: String,
    pub(super) anchor_spec: AdaptiveCollaborationSpec,
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
    let (resume_key, mut workflow_checkpoint) = load_workflow_checkpoint_for_run(
        state,
        task_id,
        run_context,
        prompt,
        &effort,
        &policy,
        models,
    )?;
    let resumed_from_workflow_id = workflow_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.plan.workflow_id.clone());
    if let Some(checkpoint) = workflow_checkpoint.as_mut() {
        checkpoint.plan.workflow_id = collaboration_id.to_string();
        let additional_turns = checkpoint.plan.budget.max_model_turns_per_step;
        checkpoint.continue_with_budget(additional_turns, workflow_started_at_ms);
    }
    let resumed_from_checkpoint = workflow_checkpoint.is_some();
    let prior = if resumed_from_checkpoint {
        None
    } else {
        workflow_prior_for_run(state, run_context, models, agent_budget)?
    };
    let evolution = if !resumed_from_checkpoint && config.prompt_evolution_enabled {
        Some(prompt_evolution_evaluation_for_run(
            state,
            &effort,
            run_context,
        )?)
    } else {
        None
    };
    let selection_mode = if resumed_from_checkpoint {
        "checkpoint_resume".to_string()
    } else {
        evolution
            .as_ref()
            .map(|evaluation| evaluation.next_mode.clone())
            .unwrap_or_else(|| "baseline".to_string())
    };
    let mut prompt_genome = workflow_checkpoint
        .as_ref()
        .and_then(|checkpoint| {
            serde_json::from_str::<ConductorPromptGenome>(&checkpoint.prompt_genome_json).ok()
        })
        .or_else(|| {
            evolution
                .as_ref()
                .map(|evaluation| evaluation.next_profile.clone())
        })
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
    prompt_genome = prompt_genome.with_effort_capability_floor(&effort);
    let prompt_capability_floor_applied = prompt_genome != selected_prompt_genome;
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
        prompt_capability_floor_applied,
        &prompt_genome,
        &prompt_genome_json,
        evolution.as_ref(),
    )?;

    let shared_memory = collaboration_context_for_genome(history, prompt_genome.context_policy);
    let anchor_spec = direct_anchor_spec(&role_hints, prompt, &shared_memory);
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
        resumed_from_workflow_id,
        resumed_from_checkpoint,
        prior,
        evolution,
        selection_mode,
        prompt_genome,
        prompt_genome_json,
        shared_memory,
        anchor_spec,
        cancellation,
    })
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
    prompt_capability_floor_applied: bool,
    prompt_genome: &ConductorPromptGenome,
    prompt_genome_json: &str,
    evolution: Option<&PromptEvolutionEvaluation>,
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
                    "prompt_capability_floor_applied".to_string(),
                    prompt_capability_floor_applied.to_string(),
                ),
                ("workflow_resume_key".to_string(), resume_key.to_string()),
                (
                    "prompt_evolution_status".to_string(),
                    if resumed_from_checkpoint {
                        "checkpoint_resume".to_string()
                    } else {
                        evolution
                            .map(|evaluation| evaluation.status.clone())
                            .unwrap_or_else(|| "disabled".to_string())
                    },
                ),
                (
                    "prompt_champion".to_string(),
                    evolution
                        .and_then(|evaluation| evaluation.champion_id.clone())
                        .unwrap_or_default(),
                ),
                (
                    "prompt_evolution_enabled".to_string(),
                    config.prompt_evolution_enabled.to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}
