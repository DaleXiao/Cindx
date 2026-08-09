use super::memory_receipts::memory_evaluation_receipt_from_events;
use super::receipts::{
    is_receipt_bearing_event, model_receipts_from_metadata, resolved_budget_from_events,
    strategy_receipt_from_events_with_constraint, successful_response_count,
};
use super::tool_receipts::{project_tool_receipts, ToolReceiptStatus};
use super::{metadata_u64, EventMetrics, PermissionPolicy, ProductRun, Treatment};
use crate::agent_execution_constraint::{AgentExecutionConstraint, MatchedRoutePlanAnchor};
use crate::agent_preparation_runtime::AgentMemoryEvaluationConstraint;
use crate::{
    begin_agent_run_control_for_effort, begin_agent_run_control_with_budget, phase16_task_id,
    resolve_agent_permission_blocking, retry_agent_task_blocking,
    run_agent_task_blocking_inner_with_evaluation_constraints, AgentState, AgentTaskInput,
    AppState, EventKind, FrozenPromptProfileSnapshot, SessionActionInput,
};
use agent_runtime::RunBudget;
use std::path::Path;

const MAX_DRIVER_ROUNDS: usize = 24;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_product_task_with_execution_constraint(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    prompt: &str,
    treatment: Treatment,
    permission_policy: PermissionPolicy,
    run_budget: Option<RunBudget>,
    execution_constraint_override: Option<AgentExecutionConstraint>,
    matched_route_plan_anchor: Option<&MatchedRoutePlanAnchor>,
) -> ProductRun {
    let effort = treatment
        .product_effort()
        .expect("product runner cannot execute an oracle reference");
    let execution_constraint = execution_constraint_override.unwrap_or_else(|| {
        if treatment.is_memory_evaluation() {
            AgentExecutionConstraint::MatchedMemoryEffect
        } else if treatment.is_grounded_direct() {
            AgentExecutionConstraint::GroundedDirect
        } else {
            AgentExecutionConstraint::Native
        }
    });
    let memory_constraint = match treatment {
        Treatment::MemoryOn => AgentMemoryEvaluationConstraint::MemoryOn,
        Treatment::MemoryOff => AgentMemoryEvaluationConstraint::MemoryOff,
        _ => AgentMemoryEvaluationConstraint::Native,
    };
    let initial = (|| -> Result<AgentState, String> {
        let lease = if let Some(budget) = run_budget {
            begin_agent_run_control_with_budget(state, session_id, budget)?
        } else {
            begin_agent_run_control_for_effort(state, session_id, effort, None)?
        };
        let control = lease.control();
        let result = run_agent_task_blocking_inner_with_evaluation_constraints(
            app,
            state.clone(),
            AgentTaskInput {
                prompt: prompt.to_string(),
                session_id: session_id.to_string(),
                current_time: chrono::Utc::now().to_rfc3339(),
                queue_id: None,
                effort: effort.to_string(),
                attachments: Vec::new(),
            },
            &control,
            execution_constraint,
            memory_constraint,
            matched_route_plan_anchor,
        );
        drop(lease);
        result
    })();
    let mut current = match initial {
        Ok(state) => state,
        Err(error) => {
            return ProductRun {
                state: empty_agent_state(session_id, &error),
                permission_requests: 0,
                denied_permissions: 0,
                error: Some(error),
            }
        }
    };
    let mut permission_requests = 0usize;
    let mut denied_permissions = 0usize;
    let mut error = None;
    for _ in 0..MAX_DRIVER_ROUNDS {
        if let Some(approval) = current.pending_approvals.first() {
            permission_requests += 1;
            let decision = match permission_policy {
                PermissionPolicy::AllowOnce => "allow_once",
                PermissionPolicy::DenyMutations => {
                    denied_permissions += 1;
                    "deny"
                }
            };
            match resolve_agent_permission_blocking(
                app,
                state.clone(),
                approval.request_id.clone(),
                decision.to_string(),
                session_id.to_string(),
            ) {
                Ok(next) => current = next,
                Err(resolve_error) => {
                    error = Some(resolve_error);
                    break;
                }
            }
            continue;
        }
        if current.status == "paused" && current.can_continue {
            match retry_agent_task_blocking(
                app,
                state.clone(),
                SessionActionInput {
                    session_id: session_id.to_string(),
                },
            ) {
                Ok(next) => current = next,
                Err(resume_error) => {
                    error = Some(resume_error);
                    break;
                }
            }
            continue;
        }
        break;
    }
    if (!current.pending_approvals.is_empty()
        || (current.status == "paused" && current.can_continue))
        && error.is_none()
    {
        error = Some(format!(
            "evaluation driver exceeded {MAX_DRIVER_ROUNDS} permission or continuation rounds"
        ));
    }
    ProductRun {
        state: current,
        permission_requests,
        denied_permissions,
        error,
    }
}

fn empty_agent_state(session_id: &str, error: &str) -> AgentState {
    AgentState {
        task_id: String::new(),
        project_id: None,
        project_name: None,
        session_id: Some(session_id.to_string()),
        session_name: None,
        status: "failed".to_string(),
        turn_count: 0,
        max_turns: 0,
        transcript_messages: 0,
        context_tokens_used: 0,
        context_window_tokens: 0,
        context_remaining_percent: 0.0,
        context_usage_estimated: true,
        run_started_at_ms: 0,
        run_budget_ms: 0,
        run_model_call_budget: 0,
        run_tool_call_budget: 0,
        can_cancel: false,
        can_retry: false,
        can_continue: false,
        event_count: 0,
        latest_sequence: 0,
        oldest_sequence: 0,
        has_older_history: false,
        timeline: Vec::new(),
        messages: Vec::new(),
        pending_approvals: Vec::new(),
        queued_messages: Vec::new(),
        latest_answer: None,
        last_error: Some(error.to_string()),
    }
}

pub(super) fn collect_event_metrics(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
    treatment: Treatment,
    frozen_profile: Option<&FrozenPromptProfileSnapshot>,
    sequence_floor: u64,
    workspace_root: &Path,
    run_budget: Option<RunBudget>,
    expected_execution_constraint: Option<AgentExecutionConstraint>,
) -> Result<EventMetrics, String> {
    let events = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .list_by_task_and_metadata_after(
                &phase16_task_id(),
                "session_id",
                session_id,
                sequence_floor,
            )
            .map_err(|error| error.to_string())?
    };
    let mut metrics = EventMetrics::default();
    for event in &events {
        match event.kind {
            EventKind::ModelRequestStarted => metrics.model_calls += 1,
            EventKind::ModelRequestFinished => {
                metrics.prompt_tokens = metrics
                    .prompt_tokens
                    .saturating_add(metadata_u64(&event.metadata, "prompt_tokens"));
                metrics.completion_tokens = metrics
                    .completion_tokens
                    .saturating_add(metadata_u64(&event.metadata, "completion_tokens"));
                metrics.total_tokens = metrics
                    .total_tokens
                    .saturating_add(metadata_u64(&event.metadata, "total_tokens"));
                let expected_responses = successful_response_count(event);
                metrics.model_responses =
                    metrics.model_responses.saturating_add(expected_responses);
                if is_receipt_bearing_event(event) {
                    match model_receipts_from_metadata(&event.metadata) {
                        Ok(receipts) => {
                            for receipt in &receipts {
                                if receipt.receipt_status != "observed" {
                                    metrics.evidence_errors.push(format!(
                                        "provider identity evidence is {} at event {}",
                                        receipt.receipt_status, event.sequence
                                    ));
                                }
                            }
                            metrics.model_receipts.extend(receipts);
                        }
                        Err(error) => metrics.evidence_errors.push(format!(
                            "provider receipt at event {} is invalid: {error}",
                            event.sequence
                        )),
                    }
                } else if expected_responses > 0 {
                    metrics.evidence_errors.push(format!(
                        "provider receipt is missing for successful event {}",
                        event.sequence
                    ));
                }
            }
            EventKind::PermissionRequested => metrics.permission_requests += 1,
            EventKind::PermissionResolved
                if event.metadata.get("decision").map(String::as_str) == Some("deny") =>
            {
                metrics.denied_permissions += 1;
            }
            _ => {}
        }
        if event.metadata.contains_key("recovery_state")
            || event.metadata.contains_key("recovery_resume_key")
        {
            metrics.recovery_events += 1;
        }
    }
    let projection = project_tool_receipts(&events, workspace_root);
    metrics.tool_calls = projection.receipts.len();
    for receipt in &projection.receipts {
        match receipt.status {
            ToolReceiptStatus::Succeeded => {
                metrics.tool_succeeded += 1;
                metrics.tools.insert(receipt.tool.clone());
            }
            ToolReceiptStatus::Failed => metrics.tool_failed += 1,
            ToolReceiptStatus::Cancelled => metrics.tool_cancelled += 1,
            ToolReceiptStatus::Denied => metrics.tool_denied += 1,
            ToolReceiptStatus::Incomplete => metrics.tool_incomplete += 1,
            ToolReceiptStatus::Superseded => metrics.tool_superseded += 1,
            ToolReceiptStatus::Invalid => metrics.tool_invalid += 1,
        }
    }
    metrics.tool_receipts = projection.receipts;
    metrics.evidence_errors.extend(projection.errors);
    metrics.model_calls = metrics.model_calls.max(metrics.model_responses);
    if metrics.model_receipts.len() != metrics.model_responses {
        metrics.evidence_errors.push(format!(
            "provider receipt coverage is {}/{} successful responses",
            metrics.model_receipts.len(),
            metrics.model_responses
        ));
    }
    match resolved_budget_from_events(&events, treatment, run_budget) {
        Ok(receipt) => metrics.resolved_budget = Some(receipt),
        Err(error) => metrics.evidence_errors.push(error),
    }
    match strategy_receipt_from_events_with_constraint(
        &events,
        treatment,
        frozen_profile,
        expected_execution_constraint,
    ) {
        Ok(receipt) => metrics.strategy_receipt = receipt,
        Err(error) => metrics.evidence_errors.push(error),
    }
    match memory_evaluation_receipt_from_events(&events, treatment) {
        Ok(receipt) => metrics.memory_evaluation_receipt = receipt,
        Err(error) => metrics.evidence_errors.push(error),
    }
    if metrics.total_tokens == 0 {
        metrics.total_tokens = metrics
            .prompt_tokens
            .saturating_add(metrics.completion_tokens);
    }
    Ok(metrics)
}

pub(super) fn event_sequence_floor(state: &tauri::State<'_, AppState>) -> Result<u64, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .latest_sequence(&phase16_task_id())
        .map_err(|error| error.to_string())
}
