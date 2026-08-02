#[path = "agent_strategy_context.rs"]
mod context;
#[path = "agent_strategy_preparation.rs"]
mod preparation;

pub(crate) use self::preparation::{
    cumulative_effective_prompt_objective, effective_prompt_objective_for_messages,
};
use self::preparation::{ensure_planning_current, selected_strategy_profile};
use crate::agent_conductor_runtime::{
    attempt_conductor_decision, conductor_model_sequence, preferred_fallback_model,
    unique_configured_models,
};
use crate::agent_conductor_scheduler::{
    schedule_conductor_decision, ConductorDecisionOutcome, ConductorDecisionSchedule,
};
use crate::app_state::AppState;
use crate::collaboration_service::{collaboration_recent_context, truncate_for_collaboration};
use crate::collaboration_stage_runtime::CollaborationStageError;
use crate::conductor_health_runtime;
use crate::configuration_models::{AgentEffort, ProviderConfig};
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::workflow_routing_runtime::{
    append_router_decision_event, conductor_historical_evidence, model_candidates_for_config,
};
use agent_core::{EventKind, Message, Metadata, TaskId};
use agent_runtime::AgentRunControl;
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

pub(crate) struct AgentRunPlanningRequest<'a> {
    pub(crate) config: &'a ProviderConfig,
    pub(crate) task_id: &'a TaskId,
    pub(crate) prompt: &'a str,
    pub(crate) history: &'a [Message],
    pub(crate) effort: AgentEffort,
    pub(crate) cancellation: &'a AgentRunControl,
}

struct PlannedRunFinalizeInput {
    decision: AgentRunDecision,
    source: String,
    attempts: usize,
    prompt_genome: ConductorPromptGenome,
    effort: AgentEffort,
    degradation_reason: Option<String>,
    attempted_conductor_models: Vec<String>,
    selected_conductor_model: Option<String>,
}

pub(crate) fn plan_agent_run(
    state: &tauri::State<'_, AppState>,
    run_context: &mut Metadata,
    request: AgentRunPlanningRequest<'_>,
) -> Result<PlannedAgentRun, CollaborationStageError> {
    let AgentRunPlanningRequest {
        config,
        task_id,
        prompt,
        history,
        effort,
        cancellation,
    } = request;
    ensure_planning_current(cancellation)?;
    run_context.insert(
        "prompt_objective".to_string(),
        truncate_for_collaboration(prompt, 6_000),
    );
    let default_effective_objective = truncate_for_collaboration(
        run_context
            .get("initial_prompt_objective")
            .map(String::as_str)
            .unwrap_or(prompt),
        6_000,
    );
    run_context
        .entry("effective_prompt_objective".to_string())
        .or_insert(default_effective_objective);
    let candidates = model_candidates_for_config(config);
    let allowed_models = unique_configured_models(&candidates);
    let fallback_model = preferred_fallback_model(config, effort, &allowed_models);
    let (profile, profile_source) = selected_strategy_profile(state, config, effort, run_context);

    if effort == AgentEffort::Fast {
        conductor_health_runtime::record_conductor_fast_bypass(run_context);
        let planned = finalize_planned_run(
            prompt,
            candidates,
            PlannedRunFinalizeInput {
                decision: AgentRunDecision::direct(fallback_model),
                source: "fast_direct".to_string(),
                attempts: 0,
                prompt_genome: profile,
                effort,
                degradation_reason: None,
                attempted_conductor_models: Vec::new(),
                selected_conductor_model: None,
            },
        )
        .map_err(CollaborationStageError::Failed)?;
        ensure_planning_current(cancellation)?;
        planned
            .apply_to_context(run_context, effort)
            .map_err(CollaborationStageError::Failed)?;
        run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
        record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)
            .map_err(CollaborationStageError::Failed)?;
        return Ok(planned);
    }
    let max_parallelism = if effort == AgentEffort::Pro { 3 } else { 2 };
    let configured_conductor_models = conductor_model_sequence(config);
    let (provider_scope, health_generation, conductor_models) =
        conductor_health_runtime::route_conductor_models(
            &state.conductor_health,
            &config.base_url,
            &configured_conductor_models,
            run_context,
        );
    let (historical_evidence, matched_collaboration_evidence) =
        conductor_historical_evidence(state, &allowed_models).unwrap_or_default();
    let base_request = AgentRunDecisionRequest {
        objective: prompt.to_string(),
        recent_context: collaboration_recent_context(history),
        effort: effort.label().to_string(),
        conductor_model: conductor_models
            .first()
            .cloned()
            .unwrap_or_else(|| config.model_for_conductor()),
        allowed_models: allowed_models.clone(),
        model_candidates: candidates.clone(),
        max_parallelism,
        evolved_directive: profile.conductor_directive(),
        historical_evidence,
        matched_collaboration_evidence,
        execution_constraints: "The foreground executor may use permission-gated tools after user approval. Isolated workflow workers can use only exposed permissionless read-only evidence tools: they cannot operate browser/computer controls, mutate the workspace, execute shell commands, or request user approval. For interactive or effectful tasks, choose workflow only when bounded isolated analysis or verification adds independent value around foreground execution."
            .to_string(),
    };
    let decision_id = format!(
        "{}-run-decision",
        run_context
            .get("agent_run_id")
            .map(String::as_str)
            .unwrap_or("agent")
    );
    let mut attempts = 0usize;
    let schedule = schedule_conductor_decision(
        &conductor_models,
        &mut attempts,
        |model_index, conductor_model, has_alternate_model, attempts| {
            let mut request = base_request.clone();
            request.conductor_model = conductor_model.to_string();
            let harness = AgentRunDecisionHarness::new(request);
            let calls_before = *attempts;
            let result = attempt_conductor_decision(
                state,
                config,
                task_id,
                run_context,
                &decision_id,
                model_index,
                has_alternate_model,
                conductor_model,
                &harness,
                attempts,
            );
            conductor_health_runtime::record_conductor_attempt(
                &state.conductor_health,
                &provider_scope,
                health_generation,
                conductor_model,
                (*attempts).saturating_sub(calls_before),
                &result,
                run_context,
            );
            result
        },
    )?;
    let ConductorDecisionSchedule {
        outcome,
        attempted_models: attempted_conductor_models,
        selected_model: selected_conductor_model,
        failure_reasons,
    } = schedule;

    let (decision, source, degradation_reason) = match outcome {
        ConductorDecisionOutcome::Selected(decision) => {
            let source = if attempted_conductor_models.len() > 1 {
                "dynamic_conductor_replanned"
            } else {
                "dynamic_conductor_v2"
            };
            (*decision, source.to_string(), None)
        }
        ConductorDecisionOutcome::Exhausted => {
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
        prompt,
        candidates,
        PlannedRunFinalizeInput {
            decision,
            source,
            attempts,
            prompt_genome: profile,
            effort,
            degradation_reason,
            attempted_conductor_models,
            selected_conductor_model,
        },
    )
    .map_err(CollaborationStageError::Failed)?;
    ensure_planning_current(cancellation)?;
    planned
        .apply_to_context(run_context, effort)
        .map_err(CollaborationStageError::Failed)?;
    run_context.insert("prompt_profile_source".to_string(), profile_source.clone());
    record_planned_agent_run(state, task_id, run_context, &planned, &profile_source)
        .map_err(CollaborationStageError::Failed)?;
    Ok(planned)
}

fn finalize_planned_run(
    prompt: &str,
    candidates: Vec<ModelCandidate>,
    input: PlannedRunFinalizeInput,
) -> Result<PlannedAgentRun, String> {
    let PlannedRunFinalizeInput {
        decision,
        source,
        attempts,
        prompt_genome,
        effort,
        degradation_reason,
        attempted_conductor_models,
        selected_conductor_model,
    } = input;
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
