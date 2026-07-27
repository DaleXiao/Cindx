use crate::agent_conductor_runtime::{
    attempt_conductor_decision, conductor_model_sequence, preferred_fallback_model,
    unique_configured_models,
};
use crate::app_state::AppState;
use crate::collaboration_service::{
    collaboration_recent_context, truncate_for_collaboration, COLLABORATION_STEER_INTERRUPTED,
};
use crate::configuration_models::{AgentEffort, ProviderConfig};
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_runtime::prompt_evolution_evaluation_for_run;
use crate::workflow_routing_runtime::{
    append_router_decision_event, conductor_historical_evidence, model_candidates_for_config,
};
use agent_core::{EventKind, Message, Metadata, TaskId};
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::{
    AgentExecutionMode, AgentRunDecision, AgentRunDecisionHarness, AgentRunDecisionRequest,
    ConductorExecutionContract, ConductorPromptGenome, ModelCandidate, RoutingContext,
    RoutingDecision,
};

#[derive(Debug, Clone)]
pub(crate) struct PlannedAgentRun {
    pub(crate) decision: AgentRunDecision,
    pub(crate) routing_context: RoutingContext,
    pub(crate) routing_decision: RoutingDecision,
    pub(crate) execution_contract: ConductorExecutionContract,
    pub(crate) source: String,
    pub(crate) attempts: usize,
    pub(crate) prompt_genome: ConductorPromptGenome,
    pub(crate) degradation_reason: Option<String>,
    pub(crate) attempted_conductor_models: Vec<String>,
    pub(crate) selected_conductor_model: Option<String>,
}

impl PlannedAgentRun {
    pub(crate) fn apply_to_context(
        &self,
        run_context: &mut Metadata,
        effort: AgentEffort,
    ) -> Result<(), String> {
        let requested_policy = effort.requested_policy();
        let collaboration_policy = self.decision.policy();
        run_context.insert(
            "task_class".to_string(),
            self.decision.task_class.label().to_string(),
        );
        run_context.insert(
            "routing_signature".to_string(),
            self.decision.learning_signature(),
        );
        run_context.insert("agent_effort".to_string(), effort.label().to_string());
        run_context.insert(
            "requested_policy".to_string(),
            requested_policy.label().to_string(),
        );
        run_context.insert(
            "collaboration_policy".to_string(),
            collaboration_policy.label().to_string(),
        );
        run_context.insert(
            "collaboration_profile".to_string(),
            if self.decision.execution == AgentExecutionMode::Workflow {
                "adaptive"
            } else {
                "direct"
            }
            .to_string(),
        );
        run_context.insert(
            "conductor_contract".to_string(),
            self.execution_contract.to_json()?,
        );
        run_context.insert(
            "expected_collaboration_uplift_bps".to_string(),
            self.execution_contract.expected_uplift_bps.to_string(),
        );
        run_context.insert(
            "agent_model".to_string(),
            self.decision.primary_model.clone(),
        );
        run_context.insert(
            "router_model".to_string(),
            self.decision.primary_model.clone(),
        );
        run_context.insert("router_examples".to_string(), "0".to_string());
        run_context.insert("router_source".to_string(), self.source.clone());
        run_context.insert(
            "conductor_degraded".to_string(),
            self.degradation_reason.is_some().to_string(),
        );
        if let Some(reason) = &self.degradation_reason {
            run_context.insert(
                "conductor_failure".to_string(),
                truncate_for_collaboration(reason, 1_200),
            );
        }
        run_context.insert(
            "run_decision".to_string(),
            serde_json::to_string(&self.decision)
                .map_err(|error| format!("run decision serialization failed: {error}"))?,
        );
        run_context.insert(
            "run_decision_attempts".to_string(),
            self.attempts.to_string(),
        );
        run_context.insert(
            "conductor_models_attempted".to_string(),
            self.attempted_conductor_models.join(","),
        );
        run_context.insert(
            "conductor_selected_model".to_string(),
            self.selected_conductor_model.clone().unwrap_or_default(),
        );
        run_context.insert("prompt_profile".to_string(), self.prompt_genome.id.clone());
        run_context.insert(
            "prompt_genome".to_string(),
            serde_json::to_string(&self.prompt_genome)
                .map_err(|error| format!("prompt genome serialization failed: {error}"))?,
        );
        Ok(())
    }
}

pub(crate) fn plan_agent_run(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &mut Metadata,
    prompt: &str,
    history: &[Message],
    effort: AgentEffort,
) -> Result<PlannedAgentRun, String> {
    let candidates = model_candidates_for_config(config);
    let allowed_models = unique_configured_models(&candidates);
    let fallback_model = preferred_fallback_model(config, effort, &allowed_models);
    let (profile, profile_source) = selected_strategy_profile(state, config, effort, run_context);

    if effort == AgentEffort::Fast {
        let planned = finalize_planned_run(
            AgentRunDecision::direct(fallback_model),
            prompt,
            candidates,
            "fast_direct".to_string(),
            0,
            profile,
            effort,
            None,
            Vec::new(),
            None,
        )?;
        planned.apply_to_context(run_context, effort)?;
        run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
        record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)?;
        return Ok(planned);
    }

    let max_parallelism = match effort {
        AgentEffort::Fast => 1,
        AgentEffort::Auto => 2,
        AgentEffort::Pro => 3,
    };
    let conductor_models = conductor_model_sequence(config);
    let base_request = AgentRunDecisionRequest {
        objective: prompt.to_string(),
        recent_context: collaboration_recent_context(history),
        effort: effort.label().to_string(),
        conductor_model: conductor_models
            .first()
            .cloned()
            .unwrap_or_else(|| config.model_for_conductor()),
        allowed_models: allowed_models.clone(),
        max_parallelism,
        evolved_directive: profile.conductor_directive(),
        historical_evidence: conductor_historical_evidence(state, &allowed_models)
            .unwrap_or_default(),
    };
    let decision_id = format!(
        "{}-run-decision",
        run_context
            .get("agent_run_id")
            .map(String::as_str)
            .unwrap_or("agent")
    );
    let mut attempts = 0usize;
    let mut attempted_conductor_models = Vec::new();
    let mut failure_reasons = Vec::new();
    let mut selected_conductor_model = None;
    let mut planned_decision = None;

    for (model_index, conductor_model) in conductor_models.iter().enumerate() {
        attempted_conductor_models.push(conductor_model.clone());
        let mut request = base_request.clone();
        request.conductor_model = conductor_model.clone();
        let harness = AgentRunDecisionHarness::new(request);
        match attempt_conductor_decision(
            state,
            config,
            task_id,
            run_context,
            &decision_id,
            model_index,
            conductor_model,
            &harness,
            &mut attempts,
        ) {
            Ok(decision) => {
                selected_conductor_model = Some(conductor_model.clone());
                planned_decision = Some(decision);
                break;
            }
            Err(error) if error == MODEL_REQUEST_CANCELLED => return Err(error),
            Err(error) if error == COLLABORATION_STEER_INTERRUPTED => {
                return Err("conductor planning interrupted by a queued steer".to_string())
            }
            Err(error) => failure_reasons.push(format!("{conductor_model}: {error}")),
        }
    }

    let (decision, source, degradation_reason) = match planned_decision {
        Some(decision) => {
            let source = if attempted_conductor_models.len() > 1 {
                "dynamic_conductor_replanned"
            } else {
                "dynamic_conductor_v2"
            };
            (decision, source.to_string(), None)
        }
        None => {
            let reason = if failure_reasons.is_empty() {
                "no configured conductor model was available".to_string()
            } else {
                failure_reasons.join(" | ")
            };
            let decision = AgentRunDecision::degraded_conductor_fallback(
                fallback_model,
                effort.label(),
                allowed_models.len(),
                max_parallelism,
                &reason,
            );
            let source = if decision.execution == AgentExecutionMode::Workflow {
                "dynamic_conductor_degraded_workflow"
            } else {
                "dynamic_conductor_degraded_direct"
            };
            (decision, source.to_string(), Some(reason))
        }
    };
    let planned = finalize_planned_run(
        decision,
        prompt,
        candidates,
        source,
        attempts,
        profile,
        effort,
        degradation_reason,
        attempted_conductor_models,
        selected_conductor_model,
    )?;
    planned.apply_to_context(run_context, effort)?;
    run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
    record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)?;
    Ok(planned)
}

fn finalize_planned_run(
    decision: AgentRunDecision,
    prompt: &str,
    candidates: Vec<ModelCandidate>,
    source: String,
    attempts: usize,
    prompt_genome: ConductorPromptGenome,
    effort: AgentEffort,
    degradation_reason: Option<String>,
    attempted_conductor_models: Vec<String>,
    selected_conductor_model: Option<String>,
) -> Result<PlannedAgentRun, String> {
    let routing_context = decision.routing_context(prompt, candidates);
    let routing_decision = decision.routing_decision();
    let execution_contract = decision.execution_contract(effort.label());
    Ok(PlannedAgentRun {
        decision,
        routing_context,
        routing_decision,
        execution_contract,
        source,
        attempts,
        prompt_genome,
        degradation_reason,
        attempted_conductor_models,
        selected_conductor_model,
    })
}

fn selected_strategy_profile(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    effort: AgentEffort,
    run_context: &Metadata,
) -> (ConductorPromptGenome, String) {
    if config.prompt_evolution_enabled {
        if let Ok(evaluation) =
            prompt_evolution_evaluation_for_run(state, effort.label(), run_context)
        {
            return (evaluation.next_profile, evaluation.next_mode);
        }
    }
    (
        ConductorPromptGenome::seed_for_effort(effort.label()),
        "seed_fallback".to_string(),
    )
}

fn record_planned_agent_run(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    planned: &PlannedAgentRun,
    profile_source: &str,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_router_decision_event(
        &mut store,
        task_id,
        run_context,
        &planned.routing_context,
        &planned.routing_decision,
        0,
    )
    .map_err(|error| error.to_string())?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        metadata_with_context(
            [
                ("decision_source".to_string(), planned.source.clone()),
                (
                    "decision_attempts".to_string(),
                    planned.attempts.to_string(),
                ),
                (
                    "conductor_models_attempted".to_string(),
                    planned.attempted_conductor_models.join(","),
                ),
                (
                    "conductor_selected_model".to_string(),
                    planned.selected_conductor_model.clone().unwrap_or_default(),
                ),
                (
                    "decision".to_string(),
                    serde_json::to_string(&planned.decision).unwrap_or_default(),
                ),
                (
                    "prompt_profile".to_string(),
                    planned.prompt_genome.id.clone(),
                ),
                ("profile_source".to_string(), profile_source.to_string()),
                (
                    "conductor_degraded".to_string(),
                    planned.degradation_reason.is_some().to_string(),
                ),
                (
                    "conductor_failure".to_string(),
                    planned
                        .degradation_reason
                        .as_deref()
                        .map(|reason| truncate_for_collaboration(reason, 1_200))
                        .unwrap_or_default(),
                ),
                (
                    "decision_rationale".to_string(),
                    truncate_for_collaboration(&planned.decision.rationale, 1_200),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}
