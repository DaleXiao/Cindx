use agent_application::{
    insert_strategy_not_selected, strategy_receipt_is_explicitly_not_selected,
    AgentStrategyDecisionReceipt, AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY,
    AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY, AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY,
    AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY, AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY,
};
use agent_core::{
    decode_event_type, DecodedEventType, Event, EventKind, EventTypeV1, Metadata, TaskId,
    EVENT_TYPE_METADATA_KEY,
};
#[cfg(test)]
use agent_storage::EventStore;
use agent_storage::{SqliteStore, StorageError};

use crate::event_persistence::append_event;

pub(crate) fn bind_strategy_receipt_from_events(
    events: &[Event],
    run_context: &Metadata,
    metadata: &mut Metadata,
) -> Result<(), String> {
    let Some(run_id) = run_context.get("agent_run_id").map(String::as_str) else {
        return Ok(());
    };
    let steer_epoch = run_context
        .get("steer_epoch")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let decisions = events
        .iter()
        .filter(|event| {
            event.metadata.get("agent_run_id").map(String::as_str) == Some(run_id)
                && event
                    .metadata
                    .get("steer_epoch")
                    .and_then(|value| value.parse::<u64>().ok())
                    == Some(steer_epoch)
                && (event.summary == "Agent run decision selected"
                    || matches!(
                        decode_event_type(event),
                        DecodedEventType::V1(typed)
                            if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
                    ))
        })
        .collect::<Vec<_>>();
    let typed = decisions
        .iter()
        .copied()
        .filter(|event| {
            matches!(
                decode_event_type(event),
                DecodedEventType::V1(typed)
                    if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
            )
        })
        .collect::<Vec<_>>();
    if typed.len() > 1 {
        return Err("agent recovery found duplicate strategy decisions for one epoch".to_string());
    }
    if strategy_receipt_is_explicitly_not_selected(run_context) {
        if run_context
            .get(AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY)
            .and_then(|value| value.parse::<u64>().ok())
            != Some(steer_epoch)
        {
            return Err("not-selected recovery context has a mismatched epoch".to_string());
        }
        if decisions.is_empty() {
            return insert_strategy_not_selected(metadata, steer_epoch)
                .map_err(|error| error.to_string());
        }
        return Err("not-selected recovery context has a durable strategy decision".to_string());
    }
    let context_receipt = AgentStrategyDecisionReceipt::from_metadata(run_context)
        .map_err(|error| error.to_string())?;
    match (context_receipt, typed.first()) {
        (Some(receipt), Some(event)) if receipt.matches_decision_event(event) => receipt
            .insert_into(metadata)
            .map_err(|error| error.to_string()),
        (Some(_), _) => {
            Err("agent recovery strategy receipt does not match durable decision".to_string())
        }
        (None, Some(event)) => AgentStrategyDecisionReceipt::from_decision_event(event)
            .map_err(|error| error.to_string())?
            .insert_into(metadata)
            .map_err(|error| error.to_string()),
        (None, None) if decisions.is_empty() => {
            insert_strategy_not_selected(metadata, steer_epoch).map_err(|error| error.to_string())
        }
        (None, None) => Ok(()),
    }
}

pub(crate) fn persist_retained_strategy_decision(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    steer_epoch: u64,
) -> Result<Option<Metadata>, StorageError> {
    let current = AgentStrategyDecisionReceipt::from_metadata(run_context)
        .map_err(|error| StorageError::new(error.to_string()))?;
    let Some(current) = current else {
        return Ok(None);
    };
    let plan_sha256 = run_context
        .get("execution_plan_semantic_sha256")
        .ok_or_else(|| StorageError::new("retained strategy has no semantic plan digest"))?;
    if current.plan_sha256() != plan_sha256 {
        return Err(StorageError::new(
            "retained strategy receipt does not match its semantic plan digest",
        ));
    }
    let mut retained_context = run_context.clone();
    retained_context.insert("steer_epoch".to_string(), steer_epoch.to_string());
    for key in [
        AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY,
        AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY,
        AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY,
        AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY,
        AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY,
        EVENT_TYPE_METADATA_KEY,
    ] {
        retained_context.remove(key);
    }
    let receipt = AgentStrategyDecisionReceipt::new(task_id, &retained_context, plan_sha256)
        .map_err(|error| StorageError::new(error.to_string()))?;
    receipt
        .insert_into(&mut retained_context)
        .map_err(|error| StorageError::new(error.to_string()))?;

    let existing =
        store.list_by_task_and_metadata(task_id, "agent_run_id", receipt.agent_run_id())?;
    let decisions = existing
        .iter()
        .filter(|event| {
            matches!(
                decode_event_type(event),
                DecodedEventType::V1(typed)
                    if typed.event_type() == EventTypeV1::AgentRunDecisionSelected
            ) && event
                .metadata
                .get("steer_epoch")
                .and_then(|value| value.parse::<u64>().ok())
                == Some(steer_epoch)
        })
        .collect::<Vec<_>>();
    match decisions.as_slice() {
        [] => {}
        [event] if receipt.matches_decision_event(event) => return Ok(Some(retained_context)),
        _ => {
            return Err(StorageError::new(
                "retained strategy conflicts with a durable decision",
            ))
        }
    }

    let mut metadata = retained_context.clone();
    metadata.insert(
        "decision_source".to_string(),
        "retained_after_noop_steer".to_string(),
    );
    metadata.insert("decision_attempts".to_string(), "0".to_string());
    append_event(
        store,
        task_id,
        EventKind::TaskStatusChanged,
        "Agent run decision selected",
        metadata,
    )?;
    Ok(Some(retained_context))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_session_persistence::metadata_with_context;
    use agent_application::{AgentOutcomeLifecycleBindingV1, AgentTerminalCommitIdentity};

    #[test]
    fn noop_steer_rebinds_the_retained_plan_to_its_new_epoch_once() {
        let task_id = TaskId("agent-retained-plan".to_string());
        let plan_sha256 = "a".repeat(64);
        let mut context = Metadata::from([
            ("session_id".to_string(), "session-retained".to_string()),
            ("agent_run_id".to_string(), "run-retained".to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
            (
                "execution_plan_semantic_sha256".to_string(),
                plan_sha256.clone(),
            ),
        ]);
        let initial = AgentStrategyDecisionReceipt::new(&task_id, &context, &plan_sha256)
            .expect("initial receipt");
        initial
            .insert_into(&mut context)
            .expect("initial receipt metadata");
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &task_id,
            EventKind::TaskStatusChanged,
            "Agent run decision selected",
            context.clone(),
        )
        .expect("initial decision should persist");

        let retained = persist_retained_strategy_decision(&mut store, &task_id, &context, 1)
            .expect("retained decision should persist")
            .expect("selected plan should be retained");
        let rebound = AgentStrategyDecisionReceipt::from_metadata(&retained)
            .expect("rebound receipt should decode")
            .expect("rebound receipt should exist");
        assert_eq!(rebound.steer_epoch(), 1);
        assert_eq!(rebound.plan_sha256(), initial.plan_sha256());
        assert_ne!(rebound.key(), initial.key());

        persist_retained_strategy_decision(&mut store, &task_id, &context, 1)
            .expect("replayed retention should be idempotent");
        let events = store.list_by_task(&task_id).expect("decisions should load");
        assert_eq!(events.len(), 2);
        assert!(rebound.matches_decision_event(&events[1]));
    }

    #[test]
    fn agent_strategy_lifecycle_contract_predecision_terminal_projects_as_censor() {
        let task_id = TaskId("agent-predecision-censor".to_string());
        let context = Metadata::from([
            ("session_id".to_string(), "session-predecision".to_string()),
            ("agent_run_id".to_string(), "run-predecision".to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
        ]);
        let mut strategy_metadata = Metadata::new();
        bind_strategy_receipt_from_events(&[], &context, &mut strategy_metadata)
            .expect("pre-decision terminal should bind an explicit not-selected receipt");
        assert!(strategy_receipt_is_explicitly_not_selected(
            &strategy_metadata
        ));

        let terminal_context = metadata_with_context(strategy_metadata, &context);
        let identity = AgentTerminalCommitIdentity::new(&task_id, &terminal_context, 0)
            .expect("not-selected terminal identity should be valid");
        let terminal_metadata = metadata_with_context(identity.metadata(), &terminal_context);
        let mut store = SqliteStore::in_memory().expect("store should open");
        append_event(
            &mut store,
            &task_id,
            EventKind::Error,
            "Agent task failed",
            terminal_metadata,
        )
        .expect("pre-decision terminal should persist");

        let events = store
            .list_by_task(&task_id)
            .expect("terminal event should load");
        let error = AgentOutcomeLifecycleBindingV1::from_events(&events)
            .expect_err("pre-decision terminal must not become a valid outcome")
            .to_string();
        assert_eq!(
            error,
            "externally verified outcome is censored before strategy decision because no strategy was selected"
        );
    }
}
