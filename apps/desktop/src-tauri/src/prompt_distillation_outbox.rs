use crate::app_state::AppState;
use crate::background_work_runtime::foreground_agent_active;
use crate::collaboration_execution::collaboration_candidate_models;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_read_model::{
    load_prompt_evolution_read_model, prompt_evolution_read_model_for_scope,
};
use crate::prompt_rollout_runtime::stable_prompt_profile_fingerprint;
use crate::runtime_values::phase16_task_id;
use agent_core::{Event, EventKind};
use orchestrator::{
    prompt_learning_dispatch_recovery, AgentPolicy, OrchestrationPolicy, ProTeacherAttestationV1,
    PromptLearningDispatchRecovery, PromptLearningOutboxProjection, PromptProDistillationIntent,
};
use std::sync::atomic::Ordering;
use tauri::Manager;

const ROLLOUT_EVENT: &str = "Conductor prompt rollout updated";
const REQUEST_EVENT: &str = "Conductor prompt evaluation requested";
const DISPATCHED_EVENT: &str = "Conductor Pro distillation dispatched";
const INTENT_ID_KEY: &str = "prompt_pro_distillation_intent_id";

fn pro_distillation_intent_from_rollout(event: &Event) -> Option<PromptProDistillationIntent> {
    if event.summary != ROLLOUT_EVENT
        || event.metadata.get("prompt_effort").map(String::as_str) != Some("pro")
        || !matches!(
            event.metadata.get("rollout_status").map(String::as_str),
            Some("promoted" | "stable")
        )
    {
        return None;
    }
    let project_id = event
        .metadata
        .get("prompt_rollout_scope")
        .or_else(|| event.metadata.get("project_id"))?;
    let stable_profile_id = event.metadata.get("stable_profile")?;
    let encoded = event.metadata.get("frozen_prompt_profile")?;
    let snapshot =
        orchestrator::FrozenPromptProfileSnapshot::from_json_slice(encoded.as_bytes()).ok()?;
    PromptProDistillationIntent::new(project_id, stable_profile_id, &event.metadata, snapshot).ok()
}

pub(crate) fn dispatch_prompt_pro_distillation_intents(
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !prompt_distillation_outbox_is_idle(&state) {
        return Ok(());
    }
    let projection = {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)?
    };
    let intents = projection
        .pending_pro_distillation()
        .map(|(project_id, intent_id, payload)| {
            PromptProDistillationIntent::decode_for_project(project_id, intent_id, payload)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut dispatched_any = false;
    for intent in intents {
        if !prompt_distillation_outbox_is_idle(&state) {
            return Ok(());
        }
        let Some(project_id) = intent.project_id() else {
            continue;
        };
        let scoped_model =
            crate::prompt_evolution_store_runtime::with_prompt_evolution_store(&state, |store| {
                let model =
                    load_prompt_evolution_read_model(store).map_err(|error| error.to_string())?;
                Ok(prompt_evolution_read_model_for_scope(&model, project_id))
            })?;
        let Some(rollout) = scoped_model.rollouts.get("pro") else {
            continue;
        };
        if rollout.stable_profile_id != intent.snapshot().genome.id
            || rollout.frozen_profile.as_ref() != Some(intent.snapshot())
            || !matches!(rollout.status.as_str(), "promoted" | "stable")
            || ProTeacherAttestationV1::from_stable_snapshot(
                intent.snapshot(),
                &rollout.stable_profile_id,
            )
            .as_ref()
                != Ok(intent.attestation())
        {
            continue;
        }
        if crate::prompt_distillation_runtime::replay_canonical_pro_teacher_snapshot(
            &scoped_model,
            intent.snapshot(),
        )
        .is_err()
        {
            continue;
        }
        let (auto_profile, _) = stable_prompt_profile_fingerprint(&scoped_model, "auto")?;
        let config = state
            .provider_config
            .lock()
            .map_err(|error| format!("provider config lock poisoned: {error}"))?
            .clone();
        if !config.prompt_evolution_enabled || !config.is_ready() {
            continue;
        }
        if !prompt_distillation_outbox_is_idle(&state) {
            return Ok(());
        }
        let source_events = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?
            .list_by_task_and_metadata(&phase16_task_id(), "project_id", project_id)
            .map_err(|error| error.to_string())?;
        let auto_sha256 = orchestrator::prompt_genome_sha256(&auto_profile)?;
        let discovered = crate::prompt_evidence_runtime::prompt_learning_dataset(
            &source_events,
            project_id,
            Some((&auto_profile.id, &auto_sha256)),
        )
        .into_iter()
        .filter(|case| crate::prompt_learning_runtime::prompt_learning_case_is_safe(case, &config))
        .collect::<Vec<_>>();
        let fresh = crate::prompt_distillation_runtime::fresh_prompt_distillation_cases(
            &scoped_model,
            intent.attestation(),
            &discovered,
        )?;
        if !crate::prompt_distillation_runtime::prompt_distillation_cases_are_sufficient(&fresh) {
            continue;
        }
        let request_exists = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?
            .list_by_task_and_metadata(
                &phase16_task_id(),
                "prompt_evaluation_request_id",
                intent.request_id(),
            )
            .map_err(|error| error.to_string())?
            .iter()
            .any(|event| event.summary == REQUEST_EVENT);
        if prompt_learning_dispatch_recovery(request_exists)
            == PromptLearningDispatchRecovery::EnqueueThenMark
        {
            if !prompt_distillation_outbox_is_idle(&state) {
                return Ok(());
            }
            let agent_budget = AgentPolicy::Auto.max_parallelism();
            let worker_models = collaboration_candidate_models(&config, agent_budget);
            if worker_models.is_empty() {
                continue;
            }
            crate::prompt_evolution_worker::enqueue_prompt_pro_to_auto_distillation(
                app,
                &phase16_task_id(),
                intent.run_context(),
                intent.request_id().to_string(),
                OrchestrationPolicy::AutoRouter.label().to_string(),
                worker_models,
                agent_budget,
                auto_profile.with_effort_delivery_contract(AgentPolicy::Auto.label()),
                intent.snapshot().clone(),
            )?;
        }
        append_dispatched(&state, &intent)?;
        dispatched_any = true;
    }
    if dispatched_any {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)?;
    }
    Ok(())
}

pub(crate) fn project_pro_distillation_outbox_event(
    projection: &mut PromptLearningOutboxProjection,
    event: &Event,
) -> Result<(), String> {
    if event.summary == DISPATCHED_EVENT {
        let intent_id = event
            .metadata
            .get(INTENT_ID_KEY)
            .ok_or_else(|| "prompt Pro distillation dispatch intent id is missing".to_string())?;
        let project_id = event
            .metadata
            .get("project_id")
            .ok_or_else(|| "prompt Pro distillation dispatch project is missing".to_string())?;
        let effective_intent_id = projection
            .pending_pro_distillation()
            .find(|(pending_project_id, _, _)| *pending_project_id == project_id)
            .and_then(|(_, pending_intent_id, payload)| {
                let pending = PromptProDistillationIntent::decode_for_project(
                    project_id,
                    pending_intent_id,
                    payload,
                )
                .ok()?;
                pending
                    .matches_dispatch_marker(project_id, intent_id, &event.metadata)
                    .then(|| pending_intent_id.to_string())
            })
            .unwrap_or_else(|| intent_id.to_string());
        return projection.remove_pro_distillation(project_id, &effective_intent_id);
    }
    let Some(intent) = pro_distillation_intent_from_rollout(event) else {
        return Ok(());
    };
    projection.insert_pro_distillation_intent(event.sequence, &intent)
}

pub(crate) fn prompt_distillation_outbox_is_idle(state: &tauri::State<'_, AppState>) -> bool {
    distillation_outbox_scan_permitted(
        foreground_agent_active(state),
        state.allow_exit.load(Ordering::Relaxed),
    )
}

fn distillation_outbox_scan_permitted(
    foreground_active: Result<bool, String>,
    exiting: bool,
) -> bool {
    matches!(foreground_active, Ok(false)) && !exiting
}

fn append_dispatched(
    state: &tauri::State<'_, AppState>,
    intent: &PromptProDistillationIntent,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        DISPATCHED_EVENT,
        metadata_with_context(
            [
                (INTENT_ID_KEY.to_string(), intent.intent_id().to_string()),
                (
                    "prompt_pro_teacher_attestation_sha256".to_string(),
                    intent.attestation().digest()?,
                ),
                ("prompt_effort".to_string(), "pro".to_string()),
                ("background_evaluation".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            intent.run_context(),
        ),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        phase16_task_id, pro_distillation_intent_from_rollout, Event, EventKind,
        PromptProDistillationIntent, DISPATCHED_EVENT, INTENT_ID_KEY, ROLLOUT_EVENT,
    };
    use agent_core::{EventId, Metadata};
    use agent_storage::{EventStore, SqliteStore};
    use orchestrator::{
        ConductorPromptGenome, FrozenPromptProfileSnapshot, FrozenPromptSourceProfileLineageV1,
        FrozenPromptTransferEvidence, PROMPT_AUTO_TRANSFER_GATE_PROTOCOL,
    };

    fn certified_pro_snapshot(mutation_index: usize) -> FrozenPromptProfileSnapshot {
        let seed = ConductorPromptGenome::seed_for_effort("pro");
        let genome = seed
            .mutations()
            .into_iter()
            .nth(mutation_index)
            .expect("Pro seed should expose deterministic mutations");
        FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            genome,
            seed.id,
            "1".repeat(64),
            format!("{:064x}", mutation_index + 2),
        )
        .unwrap()
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: "auto-source".to_string(),
            source_profile_sha256: "3".repeat(64),
            dataset_sha256: "4".repeat(64),
            cohort_sha256: Some("5".repeat(64)),
            paired_evidence_sha256: "6".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(
                FrozenPromptSourceProfileLineageV1::undistilled("3".repeat(64)).unwrap(),
            ),
        })
        .unwrap()
    }

    fn rollout_event(
        sequence: u64,
        project_id: &str,
        snapshot: &FrozenPromptProfileSnapshot,
    ) -> Event {
        Event {
            id: EventId(format!("rollout-{sequence}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: sequence,
            kind: EventKind::TaskStatusChanged,
            summary: ROLLOUT_EVENT.to_string(),
            metadata: [
                ("project_id".to_string(), project_id.to_string()),
                ("prompt_rollout_scope".to_string(), project_id.to_string()),
                ("prompt_effort".to_string(), "pro".to_string()),
                ("rollout_status".to_string(), "stable".to_string()),
                ("stable_profile".to_string(), snapshot.genome.id.clone()),
                (
                    "frozen_prompt_profile".to_string(),
                    serde_json::to_string(snapshot).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn non_pro_rollout_never_becomes_a_distillation_intent() {
        let mut event = Event {
            id: agent_core::EventId("event-1".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: ROLLOUT_EVENT.to_string(),
            metadata: Metadata::new(),
        };
        event.metadata.insert("prompt_effort".into(), "auto".into());

        assert!(pro_distillation_intent_from_rollout(&event).is_none());
    }

    #[test]
    fn foreground_activity_or_registry_failure_blocks_every_outbox_scan() {
        assert!(!super::distillation_outbox_scan_permitted(Ok(true), false));
        assert!(!super::distillation_outbox_scan_permitted(
            Err("registry unavailable".to_string()),
            false,
        ));
        assert!(!super::distillation_outbox_scan_permitted(Ok(false), true));
        assert!(super::distillation_outbox_scan_permitted(Ok(false), false));
    }

    #[test]
    fn same_project_new_rollout_supersedes_old_pending_across_restart() {
        let first_snapshot = certified_pro_snapshot(0);
        let second_snapshot = certified_pro_snapshot(1);
        let first_event = rollout_event(1, "project", &first_snapshot);
        let second_event = rollout_event(2, "project", &second_snapshot);
        let first_intent = pro_distillation_intent_from_rollout(&first_event).unwrap();
        let second_intent = pro_distillation_intent_from_rollout(&second_event).unwrap();
        let mut store = SqliteStore::in_memory().unwrap();
        store.append(first_event).unwrap();
        store.append(second_event).unwrap();

        let projected =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        let pending = projected.pending_pro_distillation().collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "project");
        assert_eq!(pending[0].1, second_intent.intent_id());

        let restarted =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        assert_eq!(
            restarted
                .pending_pro_distillation()
                .map(|(_, intent_id, _)| intent_id)
                .collect::<Vec<_>>(),
            vec![second_intent.intent_id()]
        );

        store
            .append(Event {
                id: EventId("stale-dispatch".to_string()),
                task_id: phase16_task_id(),
                sequence: 3,
                timestamp_ms: 3,
                kind: EventKind::TaskStatusChanged,
                summary: DISPATCHED_EVENT.to_string(),
                metadata: [
                    ("project_id".to_string(), "project".to_string()),
                    (
                        INTENT_ID_KEY.to_string(),
                        first_intent.intent_id().to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            })
            .unwrap();
        let after_stale_dispatch =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        assert_eq!(
            after_stale_dispatch
                .pending_pro_distillation()
                .map(|(_, intent_id, _)| intent_id)
                .collect::<Vec<_>>(),
            vec![second_intent.intent_id()]
        );
    }

    #[test]
    fn repeated_profile_rollout_keeps_the_newest_valid_context() {
        let snapshot = certified_pro_snapshot(0);
        let mut first = rollout_event(1, "project", &snapshot);
        first
            .metadata
            .insert("agent_run_id".to_string(), "run-a".to_string());
        let mut second = rollout_event(2, "project", &snapshot);
        second
            .metadata
            .insert("agent_run_id".to_string(), "run-b".to_string());
        let expected = pro_distillation_intent_from_rollout(&second).unwrap();
        let first_intent = pro_distillation_intent_from_rollout(&first).unwrap();
        assert_ne!(first_intent.intent_id(), expected.intent_id());
        let mut store = SqliteStore::in_memory().unwrap();
        store.append(first).unwrap();
        store.append(second).unwrap();

        let projection =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        let (_, intent_id, payload) = projection.pending_pro_distillation().next().unwrap();
        assert_eq!(intent_id, expected.intent_id());
        assert_eq!(
            PromptProDistillationIntent::decode_for_project("project", intent_id, payload)
                .unwrap()
                .run_context()
                .get("agent_run_id")
                .map(String::as_str),
            Some("run-b")
        );
    }

    #[test]
    fn dispatch_marker_rejects_cross_project_scope() {
        let snapshot = certified_pro_snapshot(0);
        let rollout = rollout_event(1, "project-a", &snapshot);
        let intent = pro_distillation_intent_from_rollout(&rollout).unwrap();
        let mut store = SqliteStore::in_memory().unwrap();
        store.append(rollout).unwrap();
        crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store).unwrap();
        store
            .append(Event {
                id: EventId("dispatch".to_string()),
                task_id: phase16_task_id(),
                sequence: 2,
                timestamp_ms: 2,
                kind: EventKind::TaskStatusChanged,
                summary: DISPATCHED_EVENT.to_string(),
                metadata: [
                    ("project_id".to_string(), "project-b".to_string()),
                    (INTENT_ID_KEY.to_string(), intent.intent_id().to_string()),
                ]
                .into_iter()
                .collect(),
            })
            .unwrap();

        assert!(
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap_err()
                .contains("mismatched project scope")
        );
    }
}
