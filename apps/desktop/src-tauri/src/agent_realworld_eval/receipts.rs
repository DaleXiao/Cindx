use super::{metadata_u64, Treatment};
use crate::agent_execution_constraint::{AgentExecutionConstraint, MatchedRoutePlanAnchor};
use crate::*;
use agent_application::{
    AgentRunEvent, AgentStrategyDecisionReceipt, AgentTerminalCommitIdentity,
    AgentTerminalCommitState, AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY,
    AGENT_TERMINAL_COMMIT_SCHEMA, AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY,
};
use agent_core::{decode_event_type, DecodedEventType, Event, EventTypeV1};
use orchestrator::{ExecutionPlan, WorkflowPlanIr};
use std::collections::{BTreeMap, BTreeSet};

const PROVIDER_RESPONSE_ID_DOMAIN: &str = "cindx.provider-response-id.v1\0";
const PROVIDER_FINGERPRINT_DOMAIN: &str = "cindx.provider-system-fingerprint.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ResolvedBudgetReceipt {
    pub(super) max_duration_ms: u64,
    pub(super) model_call_timeout_ms: u64,
    pub(super) tool_call_timeout_ms: u64,
    pub(super) initial_model_calls: usize,
    pub(super) max_model_calls: usize,
    pub(super) initial_tool_calls: usize,
    pub(super) max_tool_calls: usize,
    pub(super) no_progress_timeout_ms: u64,
    pub(super) max_total_tokens: u64,
    pub(super) max_physical_model_attempts: usize,
    pub(super) terminal_token_reserve: u64,
    pub(super) terminal_physical_model_attempt_reserve: usize,
}

impl ResolvedBudgetReceipt {
    pub(super) fn for_treatment(treatment: Treatment) -> Self {
        let effort = treatment.product_effort().unwrap_or("fast");
        Self::from_budget(RunBudget::for_effort(effort))
    }

    pub(super) fn from_budget(budget: RunBudget) -> Self {
        Self {
            max_duration_ms: duration_ms(budget.max_duration),
            model_call_timeout_ms: duration_ms(budget.model_call_timeout),
            tool_call_timeout_ms: duration_ms(budget.tool_call_timeout),
            initial_model_calls: budget.initial_model_calls,
            max_model_calls: budget.max_model_calls,
            initial_tool_calls: budget.initial_tool_calls,
            max_tool_calls: budget.max_tool_calls,
            no_progress_timeout_ms: duration_ms(budget.no_progress_timeout),
            max_total_tokens: budget.max_total_tokens,
            max_physical_model_attempts: budget.max_physical_model_attempts,
            terminal_token_reserve: budget.terminal_token_reserve,
            terminal_physical_model_attempt_reserve: budget.terminal_physical_model_attempt_reserve,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(super) struct TreatmentExposureReceipt {
    pub(super) logical_model_calls: usize,
    pub(super) worker_model_calls: usize,
    pub(super) successful_owner_model_calls: usize,
    pub(super) successful_specialist_model_calls: usize,
    pub(super) successful_independent_verifier_model_calls: usize,
    pub(super) successful_conductor_model_calls: usize,
    pub(super) successful_workflow_specialist_model_calls: usize,
    pub(super) successful_workflow_verifier_model_calls: usize,
    pub(super) successful_workflow_specialist_models: BTreeSet<String>,
    pub(super) successful_workflow_verifier_models: BTreeSet<String>,
    pub(super) direct_anchor_competition_calls: usize,
    pub(super) non_owner_permission_gated_calls: usize,
    pub(super) workflow_planned: bool,
    pub(super) workflow_completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct StrategyReceipt {
    #[serde(skip)]
    pub(super) matched_route_plan_anchor: Option<MatchedRoutePlanAnchor>,
    pub(super) requested_policy: String,
    pub(super) effective_policy: String,
    pub(super) execution_mode: String,
    pub(super) decision_source: String,
    pub(super) execution_constraint: String,
    pub(super) decision_sha256: String,
    pub(super) conductor_candidate_sha256: Option<String>,
    pub(super) execution_plan_sha256: Option<String>,
    pub(super) execution_plan_semantic_sha256: Option<String>,
    pub(super) execution_plan_authority: Option<String>,
    pub(super) workflow_execution_profile_sha256: Option<String>,
    pub(super) workflow_proposal_sha256: Option<String>,
    pub(super) workflow_plan_source: Option<String>,
    pub(super) workflow_plan_sha256: Option<String>,
    pub(super) workflow_step_count: usize,
    pub(super) workflow_root_roles: Vec<String>,
    pub(super) workflow_verifier_steps: usize,
    pub(super) workflow_synthesis_steps: usize,
    pub(super) workflow_execution_completed: bool,
    #[serde(skip)]
    pub(super) treatment_exposure: Option<TreatmentExposureReceipt>,
    pub(super) routing_signature_sha256: String,
    pub(super) profile_source: String,
    pub(super) profile_id: String,
    pub(super) profile_sha256: String,
    pub(super) route_profile_sha256: String,
    pub(super) route_profile_semantics_exercised: bool,
    pub(super) profile_generation: u32,
    pub(super) parent_profile_ids: Vec<String>,
    pub(super) learned_artifact_sha256: Option<String>,
    pub(super) learned_method: Option<String>,
    pub(super) stable_profile_id: Option<String>,
    pub(super) dataset_sha256: Option<String>,
    pub(super) paired_evidence_sha256: Option<String>,
    pub(super) promotion_gate_protocol: Option<String>,
    pub(super) workflow_profile_exercised: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ModelReceipt {
    pub(crate) configured_model: String,
    pub(crate) request_payload_sha256: String,
    pub(crate) provider_response_model: Option<String>,
    pub(crate) provider_response_id_sha256: Option<String>,
    pub(crate) provider_system_fingerprint_sha256: Option<String>,
    pub(crate) receipt_status: String,
    pub(crate) response_semantic_sha256: String,
}

pub(super) fn resolved_budget_from_events(
    events: &[Event],
    treatment: Treatment,
    run_budget: Option<RunBudget>,
) -> Result<ResolvedBudgetReceipt, String> {
    let expected = run_budget
        .map(ResolvedBudgetReceipt::from_budget)
        .unwrap_or_else(|| ResolvedBudgetReceipt::for_treatment(treatment));
    if treatment.is_oracle_reference() {
        return Ok(expected);
    }
    let event = events
        .iter()
        .find(|event| event.summary == "Agent task started")
        .ok_or_else(|| "agent task start budget receipt is missing".to_string())?;
    let observed = ResolvedBudgetReceipt {
        max_duration_ms: required_u64(&event.metadata, "run_budget_ms")?,
        model_call_timeout_ms: required_u64(&event.metadata, "run_model_timeout_ms")?,
        tool_call_timeout_ms: required_u64(&event.metadata, "run_tool_timeout_ms")?,
        initial_model_calls: required_usize(&event.metadata, "run_initial_model_calls")?,
        max_model_calls: required_usize(&event.metadata, "run_model_call_budget")?,
        initial_tool_calls: required_usize(&event.metadata, "run_initial_tool_calls")?,
        max_tool_calls: required_usize(&event.metadata, "run_tool_call_budget")?,
        no_progress_timeout_ms: required_u64(&event.metadata, "run_no_progress_ms")?,
        max_total_tokens: required_u64(&event.metadata, "run_total_token_budget")?,
        max_physical_model_attempts: required_usize(
            &event.metadata,
            "run_physical_model_attempt_budget",
        )?,
        terminal_token_reserve: required_u64(&event.metadata, "run_terminal_token_reserve")?,
        terminal_physical_model_attempt_reserve: required_usize(
            &event.metadata,
            "run_terminal_physical_model_attempt_reserve",
        )?,
    };
    if observed != expected {
        return Err(
            "persisted run budget does not match the resolved treatment budget".to_string(),
        );
    }
    Ok(observed)
}

#[cfg(test)]
pub(super) fn strategy_receipt_from_events(
    events: &[Event],
    treatment: Treatment,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
) -> Result<Option<StrategyReceipt>, String> {
    strategy_receipt_from_events_with_constraint(events, treatment, frozen_profile, None)
}

pub(super) fn strategy_receipt_from_events_with_constraint(
    events: &[Event],
    treatment: Treatment,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
    expected_execution_constraint: Option<AgentExecutionConstraint>,
) -> Result<Option<StrategyReceipt>, String> {
    if treatment.is_oracle_reference() {
        return Ok(None);
    }
    let (decision_index, event) = verified_strategy_decision(events)?;
    let decision_json = required_metadata_any(&event.metadata, &["run_decision", "decision"])?;
    let decision = serde_json::from_str::<AgentRunDecision>(decision_json)
        .map_err(|error| format!("agent strategy decision receipt is invalid: {error}"))?;
    let execution_plan = event
        .metadata
        .get("execution_plan")
        .map(|encoded| {
            let plan = serde_json::from_str::<ExecutionPlan>(encoded)
                .map_err(|error| format!("agent execution-plan receipt is invalid: {error}"))?;
            plan.validate()?;
            if plan.action() != &decision {
                return Err(
                    "agent execution plan does not match the legacy run-decision projection"
                        .to_string(),
                );
            }
            let digest = plan.digest()?;
            let semantic_digest = plan.semantic_digest()?;
            if event.metadata.get("execution_plan_sha256") != Some(&digest)
                || event.metadata.get("execution_plan_semantic_sha256") != Some(&semantic_digest)
                || event
                    .metadata
                    .get("execution_plan_authority")
                    .map(String::as_str)
                    != Some(plan.authority.label())
            {
                return Err("agent execution-plan metadata digest is inconsistent".to_string());
            }
            Ok(plan)
        })
        .transpose()?;
    let execution_constraint = event
        .metadata
        .get("execution_constraint")
        .map(String::as_str)
        .unwrap_or("native");
    validate_execution_constraint_receipt(
        treatment,
        expected_execution_constraint,
        execution_constraint,
        decision.execution,
    )?;
    let genome_json = required_metadata(&event.metadata, "prompt_genome")?;
    let genome = serde_json::from_str::<ConductorPromptGenome>(genome_json)
        .map_err(|error| format!("agent strategy profile receipt is invalid: {error}"))?;
    genome.validate()?;
    let profile_sha256 = prompt_genome_sha256(&genome)?;
    let profile_effort = treatment
        .product_effort()
        .ok_or_else(|| "product strategy receipt is missing its route effort".to_string())?;
    let route_profile_sha256 =
        required_metadata(&event.metadata, "route_prompt_profile_sha256")?.to_string();
    let expected_route_profile_sha256 = genome.route_decision_profile_sha256(profile_effort)?;
    if route_profile_sha256 != expected_route_profile_sha256 {
        return Err("agent strategy route profile receipt does not match its genome".to_string());
    }
    let seed_route_profile_sha256 = ConductorPromptGenome::seed_for_effort(profile_effort)
        .route_decision_profile_sha256(profile_effort)?;
    let profile_source = required_metadata_any(
        &event.metadata,
        &["profile_source", "prompt_profile_source"],
    )?
    .to_string();
    let mut workflow_profile_exercised = false;
    let learned = match (frozen_profile, profile_source.as_str()) {
        (Some(snapshot), "evaluation_frozen_profile") => {
            snapshot.validate()?;
            if snapshot.effort != treatment.profile_effort().unwrap_or_default()
                || snapshot.genome.id != genome.id
                || snapshot.candidate_sha256 != profile_sha256
            {
                return Err(
                    "frozen profile receipt does not match the exercised strategy".to_string(),
                );
            }
            Some(snapshot)
        }
        (Some(_), _) => {
            return Err("frozen evaluation profile was configured but not exercised".to_string())
        }
        (None, "evaluation_frozen_profile") => {
            return Err(
                "strategy claims a frozen evaluation profile without an artifact".to_string(),
            )
        }
        (None, _) => None,
    };
    let learned_method = learned
        .map(|snapshot| serde_json::to_value(snapshot.evolution_method))
        .transpose()
        .map_err(|error| format!("failed to serialize learned profile method: {error}"))?
        .and_then(|value| value.as_str().map(str::to_string));
    let routing_signature = required_metadata(&event.metadata, "routing_signature")?;
    let workflow_proposal = event.metadata.get("conductor_workflow_proposal");
    let workflow_proposal_sha256 = event
        .metadata
        .get("conductor_workflow_proposal_sha256")
        .cloned();
    let declared_workflow_plan_source = event
        .metadata
        .get("conductor_workflow_plan_source")
        .cloned();
    let mut workflow_plan_source = None;
    let mut workflow_plan_sha256 = None;
    let mut workflow_step_count = 0;
    let mut workflow_root_roles = Vec::new();
    let mut workflow_verifier_steps = 0;
    let mut workflow_synthesis_steps = 0;
    let mut workflow_execution_completed = false;
    let mut matched_route_plan_anchor = None;
    let matched_direct_anchor =
        expected_execution_constraint == Some(AgentExecutionConstraint::MatchedDirect);
    if decision.execution == orchestrator::AgentExecutionMode::Workflow {
        let encoded = workflow_proposal
            .ok_or_else(|| "workflow strategy receipt is missing its route proposal".to_string())?;
        let proposal = serde_json::from_str::<WorkflowPlanProposal>(encoded)
            .map_err(|error| format!("workflow strategy proposal is invalid: {error}"))?;
        let proposal_models = proposal
            .steps
            .iter()
            .map(|step| step.model.trim().to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        proposal.validate_v1(&decision, &proposal_models)?;
        let expected_proposal_sha256 = sha256_hex(encoded.as_bytes());
        if workflow_proposal_sha256.as_deref() != Some(expected_proposal_sha256.as_str())
            || declared_workflow_plan_source.as_deref() != Some("run_decision")
        {
            return Err("workflow strategy proposal receipt is inconsistent".to_string());
        }
        if expected_execution_constraint.is_some_and(AgentExecutionConstraint::is_matched_route) {
            let plan = execution_plan.as_ref().ok_or_else(|| {
                "matched Workflow execution plan is missing from its strategy receipt".to_string()
            })?;
            matched_route_plan_anchor = Some(MatchedRoutePlanAnchor::new(
                plan.conductor_candidate.clone(),
                plan.compatibility_route.clone(),
                proposal.clone(),
            )?);
        }

        if let Some((planned_index, planned)) = events
            .iter()
            .enumerate()
            .skip(decision_index.saturating_add(1))
            .find(|(_, event)| event.summary == "Collaboration workflow planned")
        {
            let encoded_plan = required_metadata(&planned.metadata, "workflow_ir")?;
            let plan = serde_json::from_str::<WorkflowPlanIr>(encoded_plan)
                .map_err(|error| format!("materialized workflow receipt is invalid: {error}"))?;
            let plan_models = plan
                .steps
                .iter()
                .map(|step| step.model.trim().to_string())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            plan.validate(&plan_models)?;
            let source = required_metadata(&planned.metadata, "conductor_source")?;
            if source == "run_decision_proposal"
                && !workflow_plan_matches_proposal(&plan, &proposal)
            {
                return Err(
                    "materialized workflow does not match its run-decision proposal".to_string(),
                );
            }
            workflow_plan_source = Some(
                if source == "run_decision_proposal" {
                    "run_decision"
                } else {
                    source
                }
                .to_string(),
            );
            workflow_plan_sha256 = Some(sha256_hex(encoded_plan.as_bytes()));
            workflow_step_count = plan.steps.len();
            workflow_root_roles = plan
                .steps
                .iter()
                .filter(|step| step.access.is_empty())
                .map(|step| step.role.clone())
                .collect();
            workflow_verifier_steps = plan
                .steps
                .iter()
                .filter(|step| {
                    step.contract.output_kind == orchestrator::WorkflowOutputKind::Verification
                })
                .count();
            workflow_synthesis_steps = plan
                .steps
                .iter()
                .filter(|step| {
                    step.contract.output_kind == orchestrator::WorkflowOutputKind::Synthesis
                })
                .count();
            workflow_profile_exercised = planned.metadata.get("prompt_profile") == Some(&genome.id);
            let collaboration_id = planned.metadata.get("collaboration_id");
            workflow_execution_completed =
                events
                    .iter()
                    .skip(planned_index.saturating_add(1))
                    .any(|event| {
                        event.summary == "Collaboration workflow completed"
                            && event.metadata.get("collaboration_id") == collaboration_id
                    });
        }
    } else if matched_direct_anchor {
        let encoded = workflow_proposal.ok_or_else(|| {
            "matched Direct receipt is missing its shared workflow proposal anchor".to_string()
        })?;
        let proposal = serde_json::from_str::<WorkflowPlanProposal>(encoded)
            .map_err(|error| format!("matched Direct workflow anchor is invalid: {error}"))?;
        let candidate = execution_plan
            .as_ref()
            .map(|plan| &plan.conductor_candidate)
            .ok_or_else(|| "matched Direct execution plan is missing".to_string())?;
        if candidate.execution != orchestrator::AgentExecutionMode::Workflow {
            return Err("matched Direct conductor anchor is not a workflow".to_string());
        }
        let proposal_models = proposal
            .steps
            .iter()
            .map(|step| step.model.trim().to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        proposal.validate_v1(candidate, &proposal_models)?;
        let expected_proposal_sha256 = sha256_hex(encoded.as_bytes());
        if workflow_proposal_sha256.as_deref() != Some(expected_proposal_sha256.as_str())
            || declared_workflow_plan_source.as_deref() != Some("run_decision")
        {
            return Err("matched Direct workflow anchor receipt is inconsistent".to_string());
        }
        matched_route_plan_anchor = Some(MatchedRoutePlanAnchor::new(
            candidate.clone(),
            execution_plan
                .as_ref()
                .expect("matched Direct execution plan was required above")
                .compatibility_route
                .clone(),
            proposal,
        )?);
    } else if workflow_proposal.is_some()
        || workflow_proposal_sha256.is_some()
        || declared_workflow_plan_source.is_some()
    {
        return Err("direct strategy receipt claimed a workflow proposal".to_string());
    }
    let treatment_exposure = expected_execution_constraint
        .filter(|constraint| constraint.is_matched_route())
        .map(|_| treatment_exposure_from_events(events, decision_index))
        .transpose()?;
    if treatment_exposure
        .as_ref()
        .is_some_and(|exposure| exposure.workflow_completed != workflow_execution_completed)
    {
        return Err(
            "matched route treatment exposure disagrees with workflow completion evidence"
                .to_string(),
        );
    }
    Ok(Some(StrategyReceipt {
        matched_route_plan_anchor,
        requested_policy: required_metadata(&event.metadata, "requested_policy")?.to_string(),
        effective_policy: required_metadata(&event.metadata, "collaboration_policy")?.to_string(),
        execution_mode: match decision.execution {
            orchestrator::AgentExecutionMode::Direct => "direct",
            orchestrator::AgentExecutionMode::Workflow => "workflow",
        }
        .to_string(),
        decision_source: required_metadata(&event.metadata, "decision_source")?.to_string(),
        execution_constraint: execution_constraint.to_string(),
        decision_sha256: domain_hash("cindx.agent-run-decision.v1\0", decision_json),
        conductor_candidate_sha256: execution_plan
            .as_ref()
            .map(|plan| serde_json::to_string(&plan.conductor_candidate))
            .transpose()
            .map_err(|error| format!("conductor candidate serialization failed: {error}"))?
            .map(|candidate| domain_hash("cindx.agent-conductor-candidate.v1\0", &candidate)),
        execution_plan_sha256: execution_plan
            .as_ref()
            .map(ExecutionPlan::digest)
            .transpose()?,
        execution_plan_semantic_sha256: execution_plan
            .as_ref()
            .map(ExecutionPlan::semantic_digest)
            .transpose()?,
        execution_plan_authority: execution_plan
            .as_ref()
            .map(|plan| plan.authority.label().to_string()),
        workflow_execution_profile_sha256: execution_plan
            .as_ref()
            .and_then(|plan| plan.workflow_execution_profile_sha256.clone()),
        workflow_proposal_sha256,
        workflow_plan_source,
        workflow_plan_sha256,
        workflow_step_count,
        workflow_root_roles,
        workflow_verifier_steps,
        workflow_synthesis_steps,
        workflow_execution_completed,
        treatment_exposure,
        routing_signature_sha256: domain_hash(
            "cindx.agent-routing-signature.v1\0",
            routing_signature,
        ),
        profile_source,
        profile_id: genome.id.clone(),
        profile_sha256,
        route_profile_semantics_exercised: decision.execution
            == orchestrator::AgentExecutionMode::Workflow
            && route_profile_sha256 != seed_route_profile_sha256,
        route_profile_sha256,
        profile_generation: genome.generation,
        parent_profile_ids: genome.parents.clone(),
        learned_artifact_sha256: learned
            .map(FrozenPromptProfileSnapshot::artifact_sha256)
            .transpose()?,
        learned_method,
        stable_profile_id: learned.map(|snapshot| snapshot.stable_profile_id.clone()),
        dataset_sha256: learned.map(|snapshot| snapshot.dataset_sha256.clone()),
        paired_evidence_sha256: learned.map(|snapshot| snapshot.paired_evidence_sha256.clone()),
        promotion_gate_protocol: learned.map(|snapshot| snapshot.promotion_gate_protocol.clone()),
        workflow_profile_exercised,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelAttributionReceipt {
    actor: String,
    service: String,
    stage: String,
    effect_authority: String,
    component: String,
    model: String,
    collaboration_id: Option<String>,
}

#[derive(Debug, Clone)]
struct StartedModelCall {
    sequence: u64,
    attribution: ModelAttributionReceipt,
}

fn treatment_exposure_from_events(
    events: &[Event],
    decision_index: usize,
) -> Result<TreatmentExposureReceipt, String> {
    let planned_workflow = events
        .iter()
        .enumerate()
        .skip(decision_index.saturating_add(1))
        .find(|(_, event)| event.summary == "Collaboration workflow planned")
        .map(|(index, event)| {
            required_metadata(&event.metadata, "collaboration_id")
                .map(|collaboration_id| (index, collaboration_id.to_string()))
        })
        .transpose()?;
    let workflow_completed =
        planned_workflow
            .as_ref()
            .is_some_and(|(planned_index, collaboration_id)| {
                events
                    .iter()
                    .skip(planned_index.saturating_add(1))
                    .any(|event| {
                        event.summary == "Collaboration workflow completed"
                            && event.metadata.get("collaboration_id").map(String::as_str)
                                == Some(collaboration_id.as_str())
                    })
            });
    let workflow_collaboration_id = planned_workflow
        .as_ref()
        .map(|(_, collaboration_id)| collaboration_id.as_str());

    let mut starts = BTreeMap::new();
    for event in events
        .iter()
        .filter(|event| event.kind == EventKind::ModelRequestStarted)
    {
        let request_id = required_metadata(&event.metadata, "request_id")?.to_string();
        let call = StartedModelCall {
            sequence: event.sequence,
            attribution: model_attribution_receipt(event)?,
        };
        if starts.insert(request_id.clone(), call).is_some() {
            return Err(format!(
                "matched route treatment exposure has duplicate model start {request_id}"
            ));
        }
    }

    let mut receipt = TreatmentExposureReceipt {
        workflow_planned: planned_workflow.is_some(),
        workflow_completed,
        ..TreatmentExposureReceipt::default()
    };
    for call in starts.values() {
        if matches!(
            call.attribution.actor.as_str(),
            "specialist" | "independent_verifier"
        ) {
            receipt.worker_model_calls = receipt.worker_model_calls.saturating_add(1);
        }
        if call.attribution.component.starts_with("direct_anchor") {
            receipt.direct_anchor_competition_calls =
                receipt.direct_anchor_competition_calls.saturating_add(1);
        }
        if call.attribution.effect_authority == "permission_gated"
            && call.attribution.actor != "owner"
        {
            receipt.non_owner_permission_gated_calls =
                receipt.non_owner_permission_gated_calls.saturating_add(1);
        }
    }

    let mut finished = BTreeSet::new();
    let mut successful_responses = 0usize;
    for event in events
        .iter()
        .filter(|event| event.kind == EventKind::ModelRequestFinished)
    {
        let request_id = required_metadata(&event.metadata, "request_id")?;
        let started = starts.get(request_id).ok_or_else(|| {
            format!("matched route treatment exposure has an orphan model finish {request_id}")
        })?;
        if !finished.insert(request_id.to_string()) {
            return Err(format!(
                "matched route treatment exposure has duplicate model finish {request_id}"
            ));
        }
        if event.sequence <= started.sequence {
            return Err(format!(
                "matched route treatment exposure model finish precedes start {request_id}"
            ));
        }
        let finished_attribution = model_attribution_receipt(event)?;
        if finished_attribution != started.attribution {
            return Err(format!(
                "matched route treatment exposure attribution changed for {request_id}"
            ));
        }
        let response_count = successful_response_count(event);
        successful_responses = successful_responses.saturating_add(response_count);
        if response_count == 0 {
            continue;
        }
        let attribution = &started.attribution;
        match attribution.actor.as_str() {
            "owner" => {
                receipt.successful_owner_model_calls =
                    receipt.successful_owner_model_calls.saturating_add(1)
            }
            "specialist" => {
                receipt.successful_specialist_model_calls =
                    receipt.successful_specialist_model_calls.saturating_add(1)
            }
            "independent_verifier" => {
                receipt.successful_independent_verifier_model_calls = receipt
                    .successful_independent_verifier_model_calls
                    .saturating_add(1)
            }
            _ => {}
        }
        if attribution.service == "conductor" {
            receipt.successful_conductor_model_calls =
                receipt.successful_conductor_model_calls.saturating_add(1);
        }
        let is_workflow_step = workflow_collaboration_id.is_some_and(|collaboration_id| {
            attribution.collaboration_id.as_deref() == Some(collaboration_id)
                && attribution.effect_authority == "read_only"
                && !attribution.component.starts_with("direct_anchor")
        });
        if is_workflow_step {
            match attribution.actor.as_str() {
                "specialist" => {
                    receipt.successful_workflow_specialist_model_calls = receipt
                        .successful_workflow_specialist_model_calls
                        .saturating_add(1);
                    receipt
                        .successful_workflow_specialist_models
                        .insert(attribution.model.clone());
                }
                "independent_verifier" => {
                    receipt.successful_workflow_verifier_model_calls = receipt
                        .successful_workflow_verifier_model_calls
                        .saturating_add(1);
                    receipt
                        .successful_workflow_verifier_models
                        .insert(attribution.model.clone());
                }
                _ => {}
            }
        }
    }
    if finished.len() != starts.len() {
        return Err("matched route treatment exposure has an unfinished model call".to_string());
    }
    receipt.logical_model_calls = starts.len().max(successful_responses);
    Ok(receipt)
}

fn model_attribution_receipt(event: &Event) -> Result<ModelAttributionReceipt, String> {
    if event
        .metadata
        .get(agent_core::AGENT_MODEL_ATTRIBUTION_SCHEMA_METADATA_KEY)
        .map(String::as_str)
        != Some(agent_core::AGENT_MODEL_ATTRIBUTION_SCHEMA)
    {
        return Err("matched route model attribution schema is missing".to_string());
    }
    let actor = required_metadata(&event.metadata, agent_core::AGENT_ACTOR_METADATA_KEY)?;
    let service = required_metadata(&event.metadata, agent_core::AGENT_SERVICE_METADATA_KEY)?;
    let stage = required_metadata(&event.metadata, agent_core::AGENT_STAGE_METADATA_KEY)?;
    let profile = required_metadata(
        &event.metadata,
        agent_core::AGENT_MODEL_PROFILE_METADATA_KEY,
    )?;
    let output_trust =
        required_metadata(&event.metadata, agent_core::AGENT_OUTPUT_TRUST_METADATA_KEY)?;
    let effect_authority = required_metadata(
        &event.metadata,
        agent_core::AGENT_EFFECT_AUTHORITY_METADATA_KEY,
    )?;
    let component = required_metadata(
        &event.metadata,
        agent_core::AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY,
    )?;
    let model = required_metadata(
        &event.metadata,
        agent_core::AGENT_ATTRIBUTION_MODEL_METADATA_KEY,
    )?;
    required_metadata(
        &event.metadata,
        agent_core::AGENT_ATTRIBUTION_LEGACY_ROLE_METADATA_KEY,
    )?;

    if !matches!(
        actor,
        "none" | "owner" | "specialist" | "independent_verifier"
    ) || !matches!(service, "none" | "conductor" | "learning_utility")
        || (actor == "none") == (service == "none")
        || !matches!(stage, "plan" | "evidence" | "act" | "verify" | "finalize")
        || !matches!(profile, "primary" | "reasoning" | "verifier" | "utility")
        || output_trust != "untrusted_model_output"
        || !matches!(effect_authority, "none" | "read_only" | "permission_gated")
    {
        return Err("matched route model attribution is invalid".to_string());
    }
    Ok(ModelAttributionReceipt {
        actor: actor.to_string(),
        service: service.to_string(),
        stage: stage.to_string(),
        effect_authority: effect_authority.to_string(),
        component: component.to_string(),
        model: model.to_string(),
        collaboration_id: event.metadata.get("collaboration_id").cloned(),
    })
}

fn verified_strategy_decision(events: &[Event]) -> Result<(usize, &Event), String> {
    let mut terminal_events = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let lifecycle = AgentRunEvent::try_from_event(event)
            .map_err(|error| format!("agent strategy lifecycle event is invalid: {error}"))?;
        if lifecycle.is_some_and(|event| event.status().is_terminal()) {
            terminal_events.push((index, event));
        }
    }
    let [(_, terminal)] = terminal_events.as_slice() else {
        return Err(if terminal_events.is_empty() {
            "agent strategy lifecycle terminal receipt is missing".to_string()
        } else {
            "agent strategy lifecycle has duplicate terminal receipts".to_string()
        });
    };
    if terminal
        .metadata
        .get(AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY)
        .map(String::as_str)
        != Some(AGENT_TERMINAL_COMMIT_SCHEMA)
    {
        return Err("agent strategy lifecycle terminal receipt is missing".to_string());
    }
    let steer_epoch = terminal
        .metadata
        .get(AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "agent strategy lifecycle terminal epoch is invalid".to_string())?;
    let receipt = AgentStrategyDecisionReceipt::from_metadata(&terminal.metadata)
        .map_err(|error| format!("agent strategy lifecycle receipt is invalid: {error}"))?;
    let receipt = receipt.ok_or_else(|| "agent strategy receipt is missing".to_string())?;
    let identity =
        AgentTerminalCommitIdentity::new(&terminal.task_id, &terminal.metadata, steer_epoch)
            .map_err(|error| {
                format!("agent strategy lifecycle terminal identity is invalid: {error}")
            })?;
    match identity.inspect_events(events) {
        Ok(AgentTerminalCommitState::Committed) => {}
        Ok(AgentTerminalCommitState::Pending) => {
            return Err("agent strategy lifecycle terminal receipt is missing".to_string())
        }
        Err(error) => Err(format!(
            "agent strategy lifecycle terminal receipt is invalid: {error}"
        ))?,
    }
    let decisions = events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            event.metadata.get("agent_run_id").map(String::as_str) == Some(receipt.agent_run_id())
                && event
                    .metadata
                    .get("steer_epoch")
                    .and_then(|value| value.parse::<u64>().ok())
                    == Some(receipt.steer_epoch())
                && matches!(
                    decode_event_type(event),
                    DecodedEventType::V1(typed)
                        if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
                )
        })
        .collect::<Vec<_>>();
    let [(decision_index, event)] = decisions.as_slice() else {
        return Err(if decisions.is_empty() {
            "agent strategy receipt is missing".to_string()
        } else {
            "agent strategy lifecycle has duplicate decisions".to_string()
        });
    };
    if !receipt.matches_decision_event(event) {
        return Err("agent strategy lifecycle receipt does not match its decision".to_string());
    }
    Ok((*decision_index, *event))
}

fn validate_execution_constraint_receipt(
    treatment: Treatment,
    expected_override: Option<AgentExecutionConstraint>,
    observed: &str,
    execution: orchestrator::AgentExecutionMode,
) -> Result<(), String> {
    let expected = expected_override.unwrap_or_else(|| {
        if treatment.is_memory_evaluation() {
            AgentExecutionConstraint::MatchedMemoryEffect
        } else if treatment.is_grounded_direct() {
            AgentExecutionConstraint::GroundedDirect
        } else {
            AgentExecutionConstraint::Native
        }
    });
    if observed != expected.label() {
        return Err(format!(
            "execution constraint receipt mismatch: expected {}, observed {observed}",
            expected.label()
        ));
    }
    match expected {
        AgentExecutionConstraint::Native => Ok(()),
        AgentExecutionConstraint::GroundedDirect
        | AgentExecutionConstraint::MatchedMemoryEffect
        | AgentExecutionConstraint::MatchedDirect
            if execution == orchestrator::AgentExecutionMode::Direct =>
        {
            Ok(())
        }
        AgentExecutionConstraint::MatchedWorkflow
            if execution == orchestrator::AgentExecutionMode::Workflow =>
        {
            Ok(())
        }
        _ => Err("execution constraint receipt does not match the executed route".to_string()),
    }
}

fn workflow_plan_matches_proposal(plan: &WorkflowPlanIr, proposal: &WorkflowPlanProposal) -> bool {
    plan.steps.len() == proposal.steps.len()
        && plan
            .steps
            .iter()
            .zip(&proposal.steps)
            .all(|(materialized, proposed)| {
                materialized.id == proposed.id.trim()
                    && materialized.role == proposed.role.trim().to_ascii_lowercase()
                    && materialized.model == proposed.model.trim()
                    && materialized.subtask == proposed.subtask.trim()
                    && materialized.access
                        == proposed
                            .access
                            .iter()
                            .map(|dependency| dependency.trim().to_string())
                            .collect::<Vec<_>>()
                    && materialized.tool_policy == proposed.tool_policy
                    && materialized.contract.output_kind == proposed.output_kind
            })
}

pub(crate) fn model_receipts_from_metadata(
    metadata: &Metadata,
) -> Result<Vec<ModelReceipt>, String> {
    if let Some(encoded) = metadata.get("worker_provider_receipts") {
        let receipts = serde_json::from_str::<Vec<Metadata>>(encoded)
            .map_err(|error| format!("worker provider receipts are invalid: {error}"))?;
        if receipts.is_empty() {
            return Err("worker provider receipt list is empty".to_string());
        }
        return receipts.iter().map(model_receipt_from_metadata).collect();
    }
    Ok(vec![model_receipt_from_metadata(metadata)?])
}

fn model_receipt_from_metadata(metadata: &Metadata) -> Result<ModelReceipt, String> {
    let request_payload_sha256 = required_sha256(metadata, "request_payload_sha256")?;
    let response_semantic_sha256 = required_sha256(metadata, "response_semantic_sha256")?;
    let response_id = optional_nonempty(metadata, "provider_response_id");
    let receipt_status = required_metadata(metadata, "provider_receipt_status")?.to_string();
    match (receipt_status.as_str(), response_id) {
        ("observed", Some(_)) | ("identity_conflict", _) => {}
        ("provider_id_missing", None) => {}
        _ => {
            return Err(
                "provider receipt status is inconsistent with the provider response id".to_string(),
            )
        }
    }
    let configured_model = optional_nonempty(metadata, "model")
        .or_else(|| optional_nonempty(metadata, "agent_model"))
        .ok_or_else(|| "provider receipt configured model is missing".to_string())?;
    Ok(ModelReceipt {
        configured_model: configured_model.to_string(),
        request_payload_sha256,
        provider_response_model: optional_nonempty(metadata, "provider_response_model")
            .map(str::to_string),
        provider_response_id_sha256: response_id
            .map(|value| domain_hash(PROVIDER_RESPONSE_ID_DOMAIN, value)),
        provider_system_fingerprint_sha256: optional_nonempty(
            metadata,
            "provider_system_fingerprint",
        )
        .map(|value| domain_hash(PROVIDER_FINGERPRINT_DOMAIN, value)),
        receipt_status,
        response_semantic_sha256,
    })
}

pub(super) fn successful_response_count(event: &Event) -> usize {
    if event.kind != EventKind::ModelRequestFinished {
        return 0;
    }
    if let Some(responses) = event
        .metadata
        .get("worker_model_responses")
        .and_then(|value| value.parse::<usize>().ok())
    {
        return responses;
    }
    if event.metadata.contains_key("request_payload_sha256") {
        return 1;
    }
    usize::from(
        event.summary == "Agent model turn finished"
            || event.metadata.get("status").map(String::as_str) == Some("completed")
            || event.metadata.contains_key("output"),
    )
}

pub(super) fn is_receipt_bearing_event(event: &Event) -> bool {
    event.kind == EventKind::ModelRequestFinished
        && (event.metadata.contains_key("request_payload_sha256")
            || event.metadata.contains_key("worker_provider_receipts"))
}

fn required_metadata<'a>(metadata: &'a Metadata, key: &str) -> Result<&'a str, String> {
    optional_nonempty(metadata, key)
        .ok_or_else(|| format!("required receipt field {key} is missing"))
}

fn required_metadata_any<'a>(metadata: &'a Metadata, keys: &[&str]) -> Result<&'a str, String> {
    keys.iter()
        .find_map(|key| optional_nonempty(metadata, key))
        .ok_or_else(|| format!("required receipt field {} is missing", keys.join("/")))
}

fn optional_nonempty<'a>(metadata: &'a Metadata, key: &str) -> Option<&'a str> {
    metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_u64(metadata: &Metadata, key: &str) -> Result<u64, String> {
    let value = metadata_u64(metadata, key);
    if value == 0 && metadata.get(key).map(String::as_str) != Some("0") {
        return Err(format!("required numeric receipt field {key} is invalid"));
    }
    Ok(value)
}

fn required_usize(metadata: &Metadata, key: &str) -> Result<usize, String> {
    required_metadata(metadata, key)?
        .parse::<usize>()
        .map_err(|_| format!("required numeric receipt field {key} is invalid"))
}

fn required_sha256(metadata: &Metadata, key: &str) -> Result<String, String> {
    let value = required_metadata(metadata, key)?;
    if !is_sha256(value) {
        return Err(format!("required digest receipt field {key} is invalid"));
    }
    Ok(value.to_string())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn domain_hash(domain: &str, value: &str) -> String {
    sha256_hex(format!("{domain}{value}").as_bytes())
}

fn duration_ms(value: std::time::Duration) -> u64 {
    value.as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        insert_event_type_v1, AgentActor, AgentEffectAuthority, AgentModelAttribution,
        AgentModelProfile, AgentService, AgentStage, EventId, TaskId,
    };
    use orchestrator::{
        AgentExecutionMode, AgentVerificationPolicy, WorkflowBudget, WorkflowCompletionCriteria,
        WorkflowOutputKind, WorkflowPlanIr, WorkflowPlanProposal, WorkflowPlanProposalStep,
        WorkflowPlanStep, WorkflowStepContract, WorkflowToolPolicy, WORKFLOW_IR_SCHEMA,
    };

    fn event(summary: &str, kind: EventKind, metadata: Metadata) -> Event {
        Event {
            id: EventId("event-receipt".to_string()),
            task_id: TaskId("task-receipt".to_string()),
            sequence: 7,
            timestamp_ms: 1,
            kind,
            summary: summary.to_string(),
            metadata,
        }
    }

    fn attributed_model_events(
        request_id: &str,
        sequence: u64,
        attribution: AgentModelAttribution,
        model: &str,
        component: &str,
        collaboration_id: Option<&str>,
    ) -> [Event; 2] {
        let mut metadata = Metadata::from([("request_id".to_string(), request_id.to_string())]);
        if let Some(collaboration_id) = collaboration_id {
            metadata.insert("collaboration_id".to_string(), collaboration_id.to_string());
        }
        attribution
            .insert_into(&mut metadata, model, "legacy-role", component)
            .unwrap();
        let mut started = event(
            "Model request started",
            EventKind::ModelRequestStarted,
            metadata.clone(),
        );
        started.sequence = sequence;
        metadata.insert("status".to_string(), "completed".to_string());
        let mut finished = event(
            "Model request finished",
            EventKind::ModelRequestFinished,
            metadata,
        );
        finished.sequence = sequence.saturating_add(1);
        [started, finished]
    }

    fn with_completed_strategy_lifecycle(events: Vec<Event>) -> Vec<Event> {
        with_completed_strategy_lifecycle_for_epoch(events, 0)
    }

    fn with_completed_strategy_lifecycle_for_epoch(
        mut events: Vec<Event>,
        steer_epoch: u64,
    ) -> Vec<Event> {
        let decision_index = events
            .iter()
            .position(|event| event.summary == "Agent run decision selected")
            .expect("strategy fixture must contain a decision");
        let decision = &mut events[decision_index];
        decision
            .metadata
            .entry("session_id".to_string())
            .or_insert_with(|| "session-receipt".to_string());
        decision
            .metadata
            .entry("agent_run_id".to_string())
            .or_insert_with(|| "run-receipt".to_string());
        decision
            .metadata
            .insert("steer_epoch".to_string(), steer_epoch.to_string());
        insert_event_type_v1(
            &decision.kind,
            &mut decision.metadata,
            EventTypeV1::AgentRunDecisionSelected,
        )
        .unwrap();
        let plan_sha256 = decision
            .metadata
            .get("execution_plan_semantic_sha256")
            .cloned()
            .unwrap_or_else(|| "a".repeat(64));
        let receipt =
            AgentStrategyDecisionReceipt::new(&decision.task_id, &decision.metadata, &plan_sha256)
                .unwrap();
        receipt.insert_into(&mut decision.metadata).unwrap();
        let identity = AgentTerminalCommitIdentity::new(
            &decision.task_id,
            &decision.metadata,
            receipt.steer_epoch(),
        )
        .unwrap();
        let mut terminal_metadata = decision.metadata.clone();
        terminal_metadata.extend(identity.metadata());
        terminal_metadata.remove(agent_core::EVENT_TYPE_METADATA_KEY);
        insert_event_type_v1(
            &EventKind::TaskStatusChanged,
            &mut terminal_metadata,
            EventTypeV1::AgentRunCompleted,
        )
        .unwrap();
        let mut terminal = event(
            "Agent task completed",
            EventKind::TaskStatusChanged,
            terminal_metadata,
        );
        terminal.sequence = events
            .iter()
            .map(|event| event.sequence)
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        events.push(terminal);
        events
    }

    fn budget_metadata(budget: &ResolvedBudgetReceipt) -> Metadata {
        Metadata::from([
            (
                "run_budget_ms".to_string(),
                budget.max_duration_ms.to_string(),
            ),
            (
                "run_model_timeout_ms".to_string(),
                budget.model_call_timeout_ms.to_string(),
            ),
            (
                "run_tool_timeout_ms".to_string(),
                budget.tool_call_timeout_ms.to_string(),
            ),
            (
                "run_initial_model_calls".to_string(),
                budget.initial_model_calls.to_string(),
            ),
            (
                "run_model_call_budget".to_string(),
                budget.max_model_calls.to_string(),
            ),
            (
                "run_initial_tool_calls".to_string(),
                budget.initial_tool_calls.to_string(),
            ),
            (
                "run_tool_call_budget".to_string(),
                budget.max_tool_calls.to_string(),
            ),
            (
                "run_no_progress_ms".to_string(),
                budget.no_progress_timeout_ms.to_string(),
            ),
            (
                "run_total_token_budget".to_string(),
                budget.max_total_tokens.to_string(),
            ),
            (
                "run_physical_model_attempt_budget".to_string(),
                budget.max_physical_model_attempts.to_string(),
            ),
            (
                "run_terminal_token_reserve".to_string(),
                budget.terminal_token_reserve.to_string(),
            ),
            (
                "run_terminal_physical_model_attempt_reserve".to_string(),
                budget.terminal_physical_model_attempt_reserve.to_string(),
            ),
        ])
    }

    fn workflow_strategy_metadata() -> Metadata {
        let mut decision = AgentRunDecision::direct("configured-model");
        decision.execution = AgentExecutionMode::Workflow;
        decision.verification = AgentVerificationPolicy::SelfCheck;
        decision.max_parallelism = 2;
        decision.min_successful_branches = 2;
        decision.distinct_contributions = 2;
        decision.estimated_steps = 3;
        decision.expected_uplift_bps = 4_000;
        decision.confidence_bps = 8_000;
        decision.stop_policy = ConductorStopPolicy::Quorum;
        let proposal = WorkflowPlanProposal {
            steps: vec![
                WorkflowPlanProposalStep {
                    id: "analysis".to_string(),
                    role: "analyst".to_string(),
                    model: "configured-model".to_string(),
                    subtask: "derive the primary solution".to_string(),
                    access: Vec::new(),
                    output_kind: WorkflowOutputKind::Analysis,
                    tool_policy: WorkflowToolPolicy::None,
                },
                WorkflowPlanProposalStep {
                    id: "counterexample".to_string(),
                    role: "critic".to_string(),
                    model: "configured-model".to_string(),
                    subtask: "search for an independent counterexample".to_string(),
                    access: Vec::new(),
                    output_kind: WorkflowOutputKind::Verification,
                    tool_policy: WorkflowToolPolicy::None,
                },
                WorkflowPlanProposalStep {
                    id: "synthesis".to_string(),
                    role: "synthesizer".to_string(),
                    model: "configured-model".to_string(),
                    subtask: "reconcile both contributions".to_string(),
                    access: vec!["analysis".to_string(), "counterexample".to_string()],
                    output_kind: WorkflowOutputKind::Synthesis,
                    tool_policy: WorkflowToolPolicy::None,
                },
            ],
        };
        let encoded_proposal = serde_json::to_string(&proposal).unwrap();
        let genome = ConductorPromptGenome::seed_for_effort("pro");
        Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            (
                "route_prompt_profile_sha256".to_string(),
                genome.route_decision_profile_sha256("pro").unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "pro_router".to_string()),
            ("collaboration_policy".to_string(), "best_of_n".to_string()),
            (
                "decision_source".to_string(),
                "dynamic_conductor".to_string(),
            ),
            (
                "routing_signature".to_string(),
                decision.learning_signature(),
            ),
            (
                "conductor_workflow_proposal_sha256".to_string(),
                sha256_hex(encoded_proposal.as_bytes()),
            ),
            ("conductor_workflow_proposal".to_string(), encoded_proposal),
            (
                "conductor_workflow_plan_source".to_string(),
                "run_decision".to_string(),
            ),
        ])
    }

    fn workflow_strategy_events(metadata: &Metadata) -> Vec<Event> {
        workflow_strategy_events_for_epoch(metadata, 0)
    }

    fn workflow_strategy_events_for_epoch(metadata: &Metadata, steer_epoch: u64) -> Vec<Event> {
        let proposal = serde_json::from_str::<WorkflowPlanProposal>(
            metadata.get("conductor_workflow_proposal").unwrap(),
        )
        .unwrap();
        let genome =
            serde_json::from_str::<ConductorPromptGenome>(metadata.get("prompt_genome").unwrap())
                .unwrap();
        let plan = WorkflowPlanIr {
            schema: WORKFLOW_IR_SCHEMA.to_string(),
            workflow_id: "workflow-receipt-test".to_string(),
            objective: "test the bound workflow receipt".to_string(),
            effort: "pro".to_string(),
            policy: "best_of_n".to_string(),
            coordinator_model: "configured-model".to_string(),
            prompt_profile: genome.id.clone(),
            steps: proposal
                .steps
                .iter()
                .map(|step| WorkflowPlanStep {
                    id: step.id.clone(),
                    role: step.role.clone(),
                    model: step.model.clone(),
                    subtask: step.subtask.clone(),
                    access: step.access.clone(),
                    tool_policy: step.tool_policy,
                    contract: WorkflowStepContract {
                        input_steps: step.access.clone(),
                        output_kind: step.output_kind.clone(),
                        completion: WorkflowCompletionCriteria::default(),
                    },
                })
                .collect(),
            budget: WorkflowBudget {
                max_steps: 3,
                max_models: 1,
                max_model_turns_per_step: 1,
                max_tool_calls_per_step: 1,
                max_output_tokens_per_step: 1_024,
            },
        };
        with_completed_strategy_lifecycle_for_epoch(
            vec![
                event(
                    "Agent run decision selected",
                    EventKind::TaskStatusChanged,
                    metadata.clone(),
                ),
                event(
                    "Collaboration workflow planned",
                    EventKind::TaskStatusChanged,
                    Metadata::from([
                        ("workflow_ir".to_string(), plan.to_json().unwrap()),
                        (
                            "conductor_source".to_string(),
                            "run_decision_proposal".to_string(),
                        ),
                        ("prompt_profile".to_string(), genome.id),
                        (
                            "collaboration_id".to_string(),
                            "workflow-receipt-test".to_string(),
                        ),
                    ]),
                ),
                event(
                    "Collaboration workflow completed",
                    EventKind::TaskStatusChanged,
                    Metadata::from([(
                        "collaboration_id".to_string(),
                        "workflow-receipt-test".to_string(),
                    )]),
                ),
            ],
            steer_epoch,
        )
    }

    #[test]
    fn workflow_strategy_receipt_binds_the_route_proposal() {
        let metadata = workflow_strategy_metadata();
        let events = workflow_strategy_events(&metadata);
        let receipt = strategy_receipt_from_events(&events, Treatment::Pro, None)
            .expect("strategy receipt")
            .expect("product strategy");

        assert_eq!(receipt.execution_mode, "workflow");
        assert_eq!(
            receipt.workflow_plan_source.as_deref(),
            Some("run_decision")
        );
        assert_eq!(
            receipt.workflow_proposal_sha256.as_ref(),
            metadata.get("conductor_workflow_proposal_sha256")
        );
        assert!(receipt.workflow_plan_sha256.is_some());
        assert_eq!(receipt.workflow_step_count, 3);
        assert_eq!(receipt.workflow_root_roles, ["analyst", "critic"]);
        assert_eq!(receipt.workflow_verifier_steps, 1);
        assert_eq!(receipt.workflow_synthesis_steps, 1);
        assert!(receipt.workflow_execution_completed);
        assert!(receipt.workflow_profile_exercised);

        let mut fallback_events = events.clone();
        fallback_events[1].metadata.insert(
            "conductor_source".to_string(),
            "model_cold_start".to_string(),
        );
        let fallback_receipt = strategy_receipt_from_events(&fallback_events, Treatment::Pro, None)
            .expect("fallback strategy receipt")
            .expect("product strategy");
        assert_eq!(
            fallback_receipt.workflow_plan_source.as_deref(),
            Some("model_cold_start")
        );

        let mut tampered = metadata;
        tampered.insert(
            "conductor_workflow_proposal_sha256".to_string(),
            "0".repeat(64),
        );
        let tampered_events = with_completed_strategy_lifecycle(vec![event(
            "Agent run decision selected",
            EventKind::TaskStatusChanged,
            tampered,
        )]);
        assert!(
            strategy_receipt_from_events(&tampered_events, Treatment::Pro, None,)
                .expect_err("tampered proposal receipt must fail closed")
                .contains("proposal receipt is inconsistent")
        );
    }

    #[test]
    fn agent_strategy_lifecycle_contract_evidence_fails_closed() {
        let events = workflow_strategy_events(&workflow_strategy_metadata());

        let mut missing_terminal = events.clone();
        missing_terminal.pop();
        assert!(
            strategy_receipt_from_events(&missing_terminal, Treatment::Pro, None)
                .expect_err("missing terminal must invalidate strategy evidence")
                .contains("lifecycle terminal receipt is missing")
        );

        let mut mismatched_terminal = events.clone();
        mismatched_terminal.last_mut().unwrap().metadata.insert(
            agent_application::AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY.to_string(),
            "f".repeat(64),
        );
        assert!(
            strategy_receipt_from_events(&mismatched_terminal, Treatment::Pro, None)
                .expect_err("mismatched terminal must invalidate strategy evidence")
                .contains("strategy receipt does not match")
        );

        let mut duplicate_decision = events;
        duplicate_decision.insert(1, duplicate_decision[0].clone());
        assert!(
            strategy_receipt_from_events(&duplicate_decision, Treatment::Pro, None)
                .expect_err("duplicate decision must invalidate strategy evidence")
                .contains("multiple strategy decisions")
        );
    }

    #[test]
    fn agent_strategy_lifecycle_contract_scopes_decisions_to_the_terminal_epoch() {
        let metadata = workflow_strategy_metadata();
        let mut events = workflow_strategy_events_for_epoch(&metadata, 1);
        let previous_decision = workflow_strategy_events_for_epoch(&metadata, 0).remove(0);
        events.insert(0, previous_decision);
        strategy_receipt_from_events(&events, Treatment::Pro, None)
            .expect("a prior steer epoch must not invalidate the terminal epoch")
            .expect("product strategy");

        let mut duplicate_terminal = workflow_strategy_events(&metadata);
        let mut unlinked = duplicate_terminal.last().unwrap().clone();
        unlinked
            .metadata
            .remove(agent_application::AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY);
        unlinked
            .metadata
            .remove(agent_application::AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY);
        unlinked.sequence = unlinked.sequence.saturating_add(1);
        duplicate_terminal.push(unlinked);
        assert!(
            strategy_receipt_from_events(&duplicate_terminal, Treatment::Pro, None)
                .expect_err("an unlinked second terminal must invalidate evidence")
                .contains("duplicate terminal receipts")
        );

        let mut cross_run_terminal = workflow_strategy_events(&metadata);
        let mut other_run = cross_run_terminal.last().unwrap().clone();
        other_run
            .metadata
            .insert("agent_run_id".to_string(), "other-run".to_string());
        other_run
            .metadata
            .remove(agent_application::AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY);
        other_run
            .metadata
            .remove(agent_application::AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY);
        other_run.sequence = other_run.sequence.saturating_add(1);
        cross_run_terminal.push(other_run);
        assert!(
            strategy_receipt_from_events(&cross_run_terminal, Treatment::Pro, None)
                .expect_err("a second physical run terminal must invalidate case evidence")
                .contains("duplicate terminal receipts")
        );

        let mut out_of_order = workflow_strategy_events(&metadata);
        let decision_sequence = out_of_order[0].sequence;
        out_of_order.last_mut().unwrap().sequence = decision_sequence;
        assert!(
            strategy_receipt_from_events(&out_of_order, Treatment::Pro, None)
                .expect_err("terminal must follow its strategy decision")
                .contains("does not follow")
        );
    }

    #[test]
    fn provider_receipt_hashes_upstream_identity_without_serializing_the_raw_id() {
        let raw_id = "upstream-response-private-123";
        let receipt = model_receipts_from_metadata(
            &[
                ("model".to_string(), "configured-model".to_string()),
                ("request_payload_sha256".to_string(), "a".repeat(64)),
                ("provider_response_id".to_string(), raw_id.to_string()),
                (
                    "provider_response_model".to_string(),
                    "served-model".to_string(),
                ),
                ("response_semantic_sha256".to_string(), "b".repeat(64)),
                (
                    "provider_receipt_status".to_string(),
                    "observed".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        )
        .expect("receipt should validate")
        .remove(0);
        let encoded = serde_json::to_string(&receipt).expect("receipt should serialize");

        assert_eq!(receipt.configured_model, "configured-model");
        assert_eq!(
            receipt.provider_response_model.as_deref(),
            Some("served-model")
        );
        assert!(receipt
            .provider_response_id_sha256
            .as_deref()
            .is_some_and(is_sha256));
        assert!(!encoded.contains(raw_id));
    }

    #[test]
    fn missing_and_conflicting_provider_ids_remain_explicit() {
        for status in ["provider_id_missing", "identity_conflict"] {
            let metadata = [
                ("model".to_string(), "configured-model".to_string()),
                ("request_payload_sha256".to_string(), "a".repeat(64)),
                ("response_semantic_sha256".to_string(), "b".repeat(64)),
                ("provider_receipt_status".to_string(), status.to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            let receipt = model_receipts_from_metadata(&metadata)
                .expect("explicit incomplete receipt should parse")
                .remove(0);
            assert_eq!(receipt.receipt_status, status);
            assert!(receipt.provider_response_id_sha256.is_none());
        }
    }

    #[test]
    fn degraded_collaboration_with_provider_receipt_counts_as_a_response() {
        let event = event(
            "Collaboration verifier unavailable",
            EventKind::ModelRequestFinished,
            Metadata::from([
                ("status".to_string(), "degraded".to_string()),
                ("request_payload_sha256".to_string(), "a".repeat(64)),
                ("response_semantic_sha256".to_string(), "b".repeat(64)),
                (
                    "provider_receipt_status".to_string(),
                    "observed".to_string(),
                ),
            ]),
        );

        assert_eq!(successful_response_count(&event), 1);
    }

    #[test]
    fn degraded_collaboration_without_provider_receipt_does_not_count_as_a_response() {
        let event = event(
            "Collaboration verifier unavailable",
            EventKind::ModelRequestFinished,
            Metadata::from([("status".to_string(), "degraded".to_string())]),
        );

        assert_eq!(successful_response_count(&event), 0);
    }

    #[test]
    fn completed_event_without_provider_receipt_still_exposes_missing_evidence() {
        let event = event(
            "Collaboration verifier finished",
            EventKind::ModelRequestFinished,
            Metadata::from([("status".to_string(), "completed".to_string())]),
        );

        assert_eq!(successful_response_count(&event), 1);
    }

    #[test]
    fn batched_worker_event_counts_each_successful_model_response() {
        let event = event(
            "Collaboration specialist finished",
            EventKind::ModelRequestFinished,
            Metadata::from([("worker_model_responses".to_string(), "3".to_string())]),
        );

        assert_eq!(successful_response_count(&event), 3);
    }

    #[test]
    fn persisted_product_budget_must_match_the_resolved_treatment_budget() {
        let budget = ResolvedBudgetReceipt::for_treatment(Treatment::Auto);
        assert_eq!(
            ResolvedBudgetReceipt::for_treatment(Treatment::GroundedDirect),
            budget,
            "grounded direct must remain iso-budget with Auto"
        );
        let mut metadata = budget_metadata(&budget);
        let events = vec![event(
            "Agent task started",
            EventKind::TaskStatusChanged,
            metadata.clone(),
        )];
        assert_eq!(
            resolved_budget_from_events(&events, Treatment::Auto, None).expect("budget receipt"),
            budget
        );

        metadata.insert("run_model_call_budget".to_string(), "1".to_string());
        let drifted = vec![event(
            "Agent task started",
            EventKind::TaskStatusChanged,
            metadata,
        )];
        assert!(resolved_budget_from_events(&drifted, Treatment::Auto, None).is_err());

        let mut custom = RunBudget::for_effort("fast");
        custom.max_duration = std::time::Duration::from_secs(73);
        custom.initial_model_calls = 3;
        custom.max_model_calls = 5;
        custom.initial_tool_calls = 7;
        custom.max_tool_calls = 11;
        let custom_receipt = ResolvedBudgetReceipt::from_budget(custom);
        let custom_events = vec![event(
            "Agent task started",
            EventKind::TaskStatusChanged,
            budget_metadata(&custom_receipt),
        )];
        assert_eq!(
            resolved_budget_from_events(&custom_events, Treatment::Pro, Some(custom))
                .expect("custom campaign budget receipt"),
            custom_receipt
        );
    }

    #[test]
    fn strategy_receipt_binds_the_actual_decision_and_profile() {
        let decision = AgentRunDecision::direct("configured-model");
        let genome = ConductorPromptGenome::seed_for_effort("fast");
        let metadata = Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            (
                "route_prompt_profile_sha256".to_string(),
                genome.route_decision_profile_sha256("fast").unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "single".to_string()),
            ("collaboration_policy".to_string(), "single".to_string()),
            ("decision_source".to_string(), "fast_direct".to_string()),
            ("routing_signature".to_string(), "frozen-route".to_string()),
        ]);
        let events = with_completed_strategy_lifecycle(vec![event(
            "Agent run decision selected",
            EventKind::TaskStatusChanged,
            metadata,
        )]);
        let receipt = strategy_receipt_from_events(&events, Treatment::Fast, None)
            .expect("strategy receipt")
            .expect("product strategy");

        assert_eq!(receipt.profile_id, genome.id);
        assert_eq!(
            receipt.profile_sha256,
            prompt_genome_sha256(&genome).unwrap()
        );
        assert_eq!(receipt.execution_mode, "direct");
        assert!(receipt.learned_artifact_sha256.is_none());
        assert!(!serde_json::to_string(&receipt)
            .unwrap()
            .contains("direct_finalizer_execution"));
    }

    #[test]
    fn matched_route_receipt_requires_the_explicit_expected_constraint() {
        assert!(validate_execution_constraint_receipt(
            Treatment::Pro,
            Some(AgentExecutionConstraint::MatchedWorkflow),
            "matched_workflow",
            orchestrator::AgentExecutionMode::Workflow,
        )
        .is_ok());
        assert!(validate_execution_constraint_receipt(
            Treatment::Pro,
            Some(AgentExecutionConstraint::MatchedDirect),
            "matched_direct",
            orchestrator::AgentExecutionMode::Direct,
        )
        .is_ok());
        assert!(validate_execution_constraint_receipt(
            Treatment::Pro,
            None,
            "matched_workflow",
            orchestrator::AgentExecutionMode::Workflow,
        )
        .is_err());
        assert!(validate_execution_constraint_receipt(
            Treatment::Pro,
            Some(AgentExecutionConstraint::MatchedWorkflow),
            "matched_workflow",
            orchestrator::AgentExecutionMode::Direct,
        )
        .is_err());
    }

    #[test]
    fn agent_execution_graph_contract_counts_every_model_stage_without_reclassification() {
        let mut events = Vec::new();
        events.extend(attributed_model_events(
            "conductor",
            1,
            AgentModelAttribution::service(
                AgentService::Conductor,
                AgentStage::Plan,
                AgentModelProfile::Reasoning,
            ),
            "conductor-model",
            "conductor",
            None,
        ));
        let mut decision = event(
            "Agent run decision selected",
            EventKind::TaskStatusChanged,
            Metadata::new(),
        );
        decision.sequence = 3;
        events.push(decision);
        events.extend(attributed_model_events(
            "owner",
            4,
            AgentModelAttribution::actor(
                AgentActor::Owner,
                AgentStage::Act,
                AgentModelProfile::Primary,
                AgentEffectAuthority::PermissionGated,
            ),
            "owner-model",
            "foreground_agent",
            None,
        ));
        let mut planned = event(
            "Collaboration workflow planned",
            EventKind::TaskStatusChanged,
            Metadata::from([(
                "collaboration_id".to_string(),
                "workflow-exposure".to_string(),
            )]),
        );
        planned.sequence = 6;
        events.push(planned);
        let mut specialist_events = attributed_model_events(
            "specialist",
            7,
            AgentModelAttribution::actor(
                AgentActor::Specialist,
                AgentStage::Act,
                AgentModelProfile::Reasoning,
                AgentEffectAuthority::ReadOnly,
            ),
            "specialist-model",
            "analysis",
            Some("workflow-exposure"),
        );
        specialist_events[1]
            .metadata
            .insert("worker_model_responses".to_string(), "3".to_string());
        events.extend(specialist_events);
        events.extend(attributed_model_events(
            "verifier",
            9,
            AgentModelAttribution::actor(
                AgentActor::IndependentVerifier,
                AgentStage::Verify,
                AgentModelProfile::Verifier,
                AgentEffectAuthority::ReadOnly,
            ),
            "verifier-model",
            "verification",
            Some("workflow-exposure"),
        ));
        events.extend(attributed_model_events(
            "anchor",
            11,
            AgentModelAttribution::actor(
                AgentActor::Specialist,
                AgentStage::Act,
                AgentModelProfile::Reasoning,
                AgentEffectAuthority::None,
            ),
            "anchor-model",
            "direct_anchor",
            Some("workflow-exposure"),
        ));
        let mut completed = event(
            "Collaboration workflow completed",
            EventKind::TaskStatusChanged,
            Metadata::from([(
                "collaboration_id".to_string(),
                "workflow-exposure".to_string(),
            )]),
        );
        completed.sequence = 13;
        events.push(completed);

        let exposure = treatment_exposure_from_events(&events, 2).unwrap();
        assert_eq!(exposure.logical_model_calls, 7);
        assert_eq!(exposure.worker_model_calls, 3);
        assert_eq!(exposure.successful_conductor_model_calls, 1);
        assert_eq!(exposure.successful_owner_model_calls, 1);
        assert_eq!(exposure.successful_specialist_model_calls, 2);
        assert_eq!(exposure.successful_independent_verifier_model_calls, 1);
        assert_eq!(exposure.successful_workflow_specialist_model_calls, 1);
        assert_eq!(exposure.successful_workflow_verifier_model_calls, 1);
        assert_eq!(
            exposure.successful_workflow_specialist_models,
            BTreeSet::from(["specialist-model".to_string()])
        );
        assert_eq!(
            exposure.successful_workflow_verifier_models,
            BTreeSet::from(["verifier-model".to_string()])
        );
        assert_eq!(exposure.direct_anchor_competition_calls, 1);
        assert_eq!(exposure.non_owner_permission_gated_calls, 0);
        assert!(exposure.workflow_planned);
        assert!(exposure.workflow_completed);

        let mut changed_attribution = events.clone();
        changed_attribution
            .iter_mut()
            .find(|event| {
                event.kind == EventKind::ModelRequestFinished
                    && event.metadata.get("request_id").map(String::as_str) == Some("owner")
            })
            .unwrap()
            .metadata
            .insert(
                agent_core::AGENT_ATTRIBUTION_COMPONENT_METADATA_KEY.to_string(),
                "changed".to_string(),
            );
        assert!(treatment_exposure_from_events(&changed_attribution, 2).is_err());
    }

    #[test]
    fn grounded_direct_receipt_proves_the_execution_constraint() {
        let decision = AgentRunDecision::direct("configured-model");
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let metadata = Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            (
                "route_prompt_profile_sha256".to_string(),
                genome.route_decision_profile_sha256("auto").unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "auto_router".to_string()),
            ("collaboration_policy".to_string(), "single".to_string()),
            ("decision_source".to_string(), "fixture".to_string()),
            ("routing_signature".to_string(), "frozen-route".to_string()),
            (
                "execution_constraint".to_string(),
                "grounded_direct".to_string(),
            ),
        ]);
        let events = with_completed_strategy_lifecycle(vec![event(
            "Agent run decision selected",
            EventKind::TaskStatusChanged,
            metadata,
        )]);
        let receipt = strategy_receipt_from_events(&events, Treatment::GroundedDirect, None)
            .expect("strategy receipt")
            .expect("grounded strategy");

        assert_eq!(receipt.execution_constraint, "grounded_direct");
        assert_eq!(receipt.execution_mode, "direct");
    }

    #[test]
    fn memory_effect_receipt_proves_the_matched_constraint() {
        let mut decision = AgentRunDecision::direct("configured-model");
        decision.memory = orchestrator::MemoryRecallPlan {
            policy: orchestrator::MemoryRecallPolicy::Relevant,
            query: "durable project requirements and prior-session facts".to_string(),
        };
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let metadata = Metadata::from([
            (
                "run_decision".to_string(),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "prompt_genome".to_string(),
                serde_json::to_string(&genome).unwrap(),
            ),
            (
                "route_prompt_profile_sha256".to_string(),
                genome.route_decision_profile_sha256("auto").unwrap(),
            ),
            ("profile_source".to_string(), "seed_fallback".to_string()),
            ("requested_policy".to_string(), "auto_router".to_string()),
            ("collaboration_policy".to_string(), "single".to_string()),
            (
                "decision_source".to_string(),
                "matched_memory_evaluation".to_string(),
            ),
            ("routing_signature".to_string(), "frozen-route".to_string()),
            (
                "execution_constraint".to_string(),
                "matched_memory_effect".to_string(),
            ),
        ]);

        let events = with_completed_strategy_lifecycle(vec![event(
            "Agent run decision selected",
            EventKind::TaskStatusChanged,
            metadata,
        )]);
        let receipt = strategy_receipt_from_events(&events, Treatment::MemoryOn, None)
            .expect("strategy receipt")
            .expect("memory-effect strategy");

        assert_eq!(receipt.decision_source, "matched_memory_evaluation");
        assert_eq!(receipt.execution_constraint, "matched_memory_effect");
        assert_eq!(receipt.execution_mode, "direct");
    }
}
