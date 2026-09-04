//! How a delegated child's stop is classified, phrased, and recorded.
//!
//! The child loop produces facts; this module turns them into what the parent
//! model reads and what the durable transcript keeps — the stop reason behind a
//! refused model-call reservation, the answer text for each reason, the child's
//! reportable outcome, and the persisted `subagent_result` message. Keeping the
//! vocabulary in one place is what makes a steered child unable to answer with a
//! budget exhaustion it did not have.

use crate::agent_query_commands::agent_run_should_stop;
use crate::app_state::AppState;
use crate::project_session_persistence::metadata_with_context;
use agent_core::{Message, Metadata, TaskId};
use agent_runtime::{
    AgentRunControl, SubagentChildOutcome, SubagentStopReason, SUBAGENT_MAX_STEPS,
};
use std::sync::Arc;

/// The child loop's own report type lives in `agent_runtime::subagent` beside the
/// caps it reports against, so the durable record and the eval harness read the
/// same vocabulary; this helper only shortens the loop's seven stop sites.
pub(crate) fn child_outcome(
    description: &str,
    answer: String,
    stop_reason: SubagentStopReason,
    steps: usize,
    tool_calls: usize,
) -> SubagentChildOutcome {
    SubagentChildOutcome::new(description, answer, stop_reason, steps, tool_calls)
}

/// Whether the parent run's objective moved under this child: a steer is pending,
/// or the child can no longer act on the epoch it started with.
pub(crate) fn subagent_run_was_steered(cancellation: &Arc<AgentRunControl>) -> bool {
    cancellation.has_pending_steer()
        || !cancellation.objective_epoch_is_current(cancellation.steer_epoch())
}

/// Why a child's model-call reservation was refused. Cancellation wins, then a
/// steer that superseded the delegation, then the Worker stage budget.
pub(crate) fn subagent_stage_stop_reason(
    cancellation: &Arc<AgentRunControl>,
) -> SubagentStopReason {
    if agent_run_should_stop(cancellation) {
        SubagentStopReason::Cancelled
    } else if subagent_run_was_steered(cancellation) {
        SubagentStopReason::Steered
    } else {
        SubagentStopReason::StageBudget
    }
}

/// The answer text for a child that stopped before finishing. Each reason keeps
/// the wording the parent already receives for it, except `Steered`, which used
/// to be indistinguishable from a stage-budget exhaustion.
pub(crate) fn subagent_stop_answer(reason: SubagentStopReason, partial: &str) -> String {
    match reason {
        SubagentStopReason::Steered => subagent_steered_answer(partial),
        SubagentStopReason::Cancelled => subagent_stopped_answer(partial),
        SubagentStopReason::StepLimit => subagent_step_limit_answer(partial),
        _ => subagent_budget_answer(partial),
    }
}

pub(crate) fn subagent_steered_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent stopped: the run was steered before this delegation produced an answer."
            .to_string()
    } else {
        format!("{partial}\n\n[subagent stopped: superseded by user steering]")
    }
}

/// Persist a delegation's `subagent_result` message as a durable `MessageAdded`
/// event, with exactly the metadata the in-memory message carries plus the run
/// context every persisted message carries.
///
/// Best-effort like the progress events: a store failure must not lose the
/// in-memory result the parent is about to reason over.
pub(crate) fn persist_subagent_result_message(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    message: &Message,
) {
    let persisted = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))
        .and_then(|mut store| {
            crate::event_persistence::append_message_event_with_metadata(
                &mut store,
                task_id,
                message.role.clone(),
                &message.content,
                metadata_with_context(message.metadata.clone(), run_context),
            )
            .map_err(|error| format!("failed to persist the subagent result message: {error}"))
        });
    if let Err(error) = persisted {
        eprintln!("{error}");
    }
}

pub(crate) fn subagent_stopped_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent stopped before producing an answer.".to_string()
    } else {
        format!("{partial}\n\n[subagent stopped by run cancellation]")
    }
}

pub(crate) fn subagent_budget_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent exhausted its stage budget before producing an answer.".to_string()
    } else {
        format!("{partial}\n\n[subagent stopped: stage budget exhausted]")
    }
}

pub(crate) fn subagent_step_limit_answer(partial: &str) -> String {
    if partial.trim().is_empty() {
        "Subagent reached its step limit without a final answer.".to_string()
    } else {
        format!("{partial}\n\n[subagent stopped: step limit {SUBAGENT_MAX_STEPS} reached]")
    }
}
