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
use agent_core::{Event, EventKind, Metadata};
use orchestrator::{
    sha256_hex, AgentPolicy, FrozenPromptProfileSnapshot, OrchestrationPolicy,
    ProTeacherAttestationV1,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tauri::Manager;

use crate::prompt_learning_outbox_projection::{
    prompt_learning_dispatch_recovery, PromptLearningDispatchRecovery,
    PromptLearningOutboxProjection,
};

const ROLLOUT_EVENT: &str = "Conductor prompt rollout updated";
const REQUEST_EVENT: &str = "Conductor prompt evaluation requested";
const DISPATCHED_EVENT: &str = "Conductor Pro distillation dispatched";
const INTENT_ID_KEY: &str = "prompt_pro_distillation_intent_id";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PromptProDistillationIntent {
    intent_id: String,
    request_id: String,
    run_context: Metadata,
    snapshot: FrozenPromptProfileSnapshot,
    attestation: ProTeacherAttestationV1,
}

impl PromptProDistillationIntent {
    fn from_rollout(event: &Event) -> Option<Self> {
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
            .or_else(|| event.metadata.get("project_id"))?
            .trim();
        let stable_profile_id = event.metadata.get("stable_profile")?.trim();
        if project_id.is_empty() || stable_profile_id.is_empty() {
            return None;
        }
        let encoded = event.metadata.get("frozen_prompt_profile")?;
        let snapshot = FrozenPromptProfileSnapshot::from_json_slice(encoded.as_bytes()).ok()?;
        let attestation =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, stable_profile_id).ok()?;
        let digest = attestation.digest().ok()?;
        let run_context = persistent_context(&event.metadata, project_id);
        let intent_id = pro_distillation_intent_id(project_id, &digest, &run_context).ok()?;
        Some(Self {
            request_id: format!("prompt-evaluation-{intent_id}"),
            intent_id,
            run_context,
            snapshot,
            attestation,
        })
    }

    fn validate(&self) -> bool {
        let Some(project_id) = self.run_context.get("project_id") else {
            return false;
        };
        if project_id.trim().is_empty()
            || self.intent_id.trim().is_empty()
            || self.request_id != format!("prompt-evaluation-{}", self.intent_id)
        {
            return false;
        }
        let Ok(expected) =
            ProTeacherAttestationV1::from_stable_snapshot(&self.snapshot, &self.snapshot.genome.id)
        else {
            return false;
        };
        expected == self.attestation
            && self.attestation.digest().is_ok_and(|digest| {
                pro_distillation_intent_id(project_id, &digest, &self.run_context)
                    .is_ok_and(|expected_id| self.intent_id == expected_id)
            })
    }
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
            decode_pro_distillation_intent(project_id, intent_id, payload)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut dispatched_any = false;
    for (_intent_id, intent) in intents {
        if !prompt_distillation_outbox_is_idle(&state) {
            return Ok(());
        }
        let Some(project_id) = intent.run_context.get("project_id") else {
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
        if rollout.stable_profile_id != intent.snapshot.genome.id
            || rollout.frozen_profile.as_ref() != Some(&intent.snapshot)
            || !matches!(rollout.status.as_str(), "promoted" | "stable")
            || ProTeacherAttestationV1::from_stable_snapshot(
                &intent.snapshot,
                &rollout.stable_profile_id,
            )
            .as_ref()
                != Ok(&intent.attestation)
        {
            continue;
        }
        if crate::prompt_distillation_runtime::replay_canonical_pro_teacher_snapshot(
            &scoped_model,
            &intent.snapshot,
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
            &intent.attestation,
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
                &intent.request_id,
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
                &intent.run_context,
                intent.request_id.clone(),
                OrchestrationPolicy::AutoRouter.label().to_string(),
                worker_models,
                agent_budget,
                auto_profile.with_effort_delivery_contract(AgentPolicy::Auto.label()),
                intent.snapshot.clone(),
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
                let (_, pending) =
                    decode_pro_distillation_intent(project_id, pending_intent_id, payload).ok()?;
                let digest = pending.attestation.digest().ok()?;
                (legacy_pro_distillation_intent_id(&pending.attestation)
                    .ok()?
                    .as_str()
                    == intent_id
                    || previous_project_scoped_pro_distillation_intent_id(project_id, &digest)
                        == *intent_id)
                    .then(|| pending_intent_id.to_string())
            })
            .unwrap_or_else(|| intent_id.to_string());
        return projection.remove_pro_distillation(project_id, &effective_intent_id);
    }
    let Some(intent) = PromptProDistillationIntent::from_rollout(event) else {
        return Ok(());
    };
    let payload = serde_json::to_string(&intent)
        .map_err(|error| format!("prompt Pro distillation intent serialization failed: {error}"))?;
    let project_id = intent
        .run_context
        .get("project_id")
        .ok_or_else(|| "prompt Pro distillation project is missing".to_string())?;
    projection.insert_pro_distillation(project_id, &intent.intent_id, event.sequence, payload)
}

pub(crate) fn pro_distillation_intent_payload_is_valid(
    project_id: &str,
    intent_id: &str,
    payload: &str,
) -> bool {
    decode_pro_distillation_intent(project_id, intent_id, payload).is_ok()
}

fn decode_pro_distillation_intent(
    project_id: &str,
    intent_id: &str,
    payload: &str,
) -> Result<(String, PromptProDistillationIntent), String> {
    let intent = serde_json::from_str::<PromptProDistillationIntent>(payload)
        .map_err(|error| format!("prompt Pro distillation intent is invalid: {error}"))?;
    if !intent.validate()
        || intent.intent_id != intent_id
        || intent.run_context.get("project_id").map(String::as_str) != Some(project_id)
    {
        return Err("prompt Pro distillation intent identity is invalid".to_string());
    }
    Ok((intent_id.to_string(), intent))
}

fn pro_distillation_intent_id(
    project_id: &str,
    attestation_digest: &str,
    run_context: &Metadata,
) -> Result<String, String> {
    let context = serde_json::to_vec(run_context)
        .map_err(|error| format!("prompt Pro distillation context serialization failed: {error}"))?;
    let context_digest = sha256_hex(&context);
    let digest = sha256_hex(
        format!("{project_id}\n{attestation_digest}\n{context_digest}").as_bytes(),
    );
    Ok(format!("prompt-pro-distillation-{digest}"))
}

fn previous_project_scoped_pro_distillation_intent_id(
    project_id: &str,
    attestation_digest: &str,
) -> String {
    let digest = sha256_hex(format!("{project_id}\n{attestation_digest}").as_bytes());
    format!("prompt-pro-distillation-{digest}")
}

fn legacy_pro_distillation_intent_id(
    attestation: &ProTeacherAttestationV1,
) -> Result<String, String> {
    Ok(format!("prompt-pro-distillation-{}", attestation.digest()?))
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
                (INTENT_ID_KEY.to_string(), intent.intent_id.clone()),
                (
                    "prompt_pro_teacher_attestation_sha256".to_string(),
                    intent.attestation.digest()?,
                ),
                ("prompt_effort".to_string(), "pro".to_string()),
                ("background_evaluation".to_string(), "true".to_string()),
            ]
            .into_iter()
            .collect(),
            &intent.run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

fn persistent_context(metadata: &Metadata, project_id: &str) -> Metadata {
    let mut context = [
        "agent_run_id",
        "collaboration_policy",
        "project_root",
        "session_id",
        "task_class",
    ]
    .into_iter()
    .filter_map(|key| {
        metadata
            .get(key)
            .map(|value| (key.to_string(), value.clone()))
    })
    .collect::<Metadata>();
    context.insert("project_id".to_string(), project_id.to_string());
    context.insert("agent_effort".to_string(), "auto".to_string());
    context
}

#[cfg(test)]
mod tests {
    use super::{
        legacy_pro_distillation_intent_id, phase16_task_id,
        previous_project_scoped_pro_distillation_intent_id, Event, EventKind, Metadata,
        PromptProDistillationIntent, DISPATCHED_EVENT, INTENT_ID_KEY, ROLLOUT_EVENT,
    };
    use agent_core::EventId;
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

        assert!(PromptProDistillationIntent::from_rollout(&event).is_none());
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
    fn pro_intent_identity_is_bound_to_project_scope() {
        let snapshot = certified_pro_snapshot(0);
        let project_a =
            PromptProDistillationIntent::from_rollout(&rollout_event(1, "project-a", &snapshot))
                .unwrap();
        let project_b =
            PromptProDistillationIntent::from_rollout(&rollout_event(2, "project-b", &snapshot))
                .unwrap();

        assert_ne!(project_a.intent_id, project_b.intent_id);
        assert!(project_a.validate());
        assert!(project_b.validate());

        let mut cross_project = project_a;
        cross_project
            .run_context
            .insert("project_id".to_string(), "project-b".to_string());
        assert!(!cross_project.validate());
    }

    #[test]
    fn same_project_new_rollout_supersedes_old_pending_across_restart() {
        let first_snapshot = certified_pro_snapshot(0);
        let second_snapshot = certified_pro_snapshot(1);
        let first_event = rollout_event(1, "project", &first_snapshot);
        let second_event = rollout_event(2, "project", &second_snapshot);
        let first_intent = PromptProDistillationIntent::from_rollout(&first_event).unwrap();
        let second_intent = PromptProDistillationIntent::from_rollout(&second_event).unwrap();
        let mut store = SqliteStore::in_memory().unwrap();
        store.append(first_event).unwrap();
        store.append(second_event).unwrap();

        let projected =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        let pending = projected.pending_pro_distillation().collect::<Vec<_>>();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "project");
        assert_eq!(pending[0].1, second_intent.intent_id);

        let restarted =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        assert_eq!(
            restarted
                .pending_pro_distillation()
                .map(|(_, intent_id, _)| intent_id)
                .collect::<Vec<_>>(),
            vec![second_intent.intent_id.as_str()]
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
                    (INTENT_ID_KEY.to_string(), first_intent.intent_id),
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
            vec![second_intent.intent_id.as_str()]
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
        let expected = PromptProDistillationIntent::from_rollout(&second).unwrap();
        let first_intent = PromptProDistillationIntent::from_rollout(&first).unwrap();
        assert_ne!(first_intent.intent_id, expected.intent_id);
        let mut store = SqliteStore::in_memory().unwrap();
        store.append(first).unwrap();
        store.append(second).unwrap();

        let projection =
            crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                .unwrap();
        let (_, intent_id, payload) = projection.pending_pro_distillation().next().unwrap();
        assert_eq!(intent_id, expected.intent_id);
        assert_eq!(
            serde_json::from_str::<PromptProDistillationIntent>(payload)
                .unwrap()
                .run_context
                .get("agent_run_id")
                .map(String::as_str),
            Some("run-b")
        );
    }

    #[test]
    fn dispatch_marker_rejects_cross_project_scope() {
        let snapshot = certified_pro_snapshot(0);
        let rollout = rollout_event(1, "project-a", &snapshot);
        let intent = PromptProDistillationIntent::from_rollout(&rollout).unwrap();
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
                    (INTENT_ID_KEY.to_string(), intent.intent_id),
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

    #[test]
    fn legacy_dispatch_markers_prevent_duplicate_redispatch() {
        let snapshot = certified_pro_snapshot(0);
        let rollout = rollout_event(1, "project", &snapshot);
        let intent = PromptProDistillationIntent::from_rollout(&rollout).unwrap();
        let digest = intent.attestation.digest().unwrap();
        let legacy_ids = [
            legacy_pro_distillation_intent_id(&intent.attestation).unwrap(),
            previous_project_scoped_pro_distillation_intent_id("project", &digest),
        ];
        for (index, legacy_intent_id) in legacy_ids.into_iter().enumerate() {
            assert_ne!(legacy_intent_id, intent.intent_id);
            let mut store = SqliteStore::in_memory().unwrap();
            store.append(rollout.clone()).unwrap();
            store
                .append(Event {
                    id: EventId(format!("legacy-dispatch-{index}")),
                    task_id: phase16_task_id(),
                    sequence: 2,
                    timestamp_ms: 2,
                    kind: EventKind::TaskStatusChanged,
                    summary: DISPATCHED_EVENT.to_string(),
                    metadata: [
                        ("project_id".to_string(), "project".to_string()),
                        (INTENT_ID_KEY.to_string(), legacy_intent_id),
                    ]
                    .into_iter()
                    .collect(),
                })
                .unwrap();

            let projection =
                crate::prompt_evolution_transfer_outbox::load_prompt_learning_outbox(&mut store)
                    .unwrap();
            assert_eq!(projection.pending_pro_distillation().count(), 0);
        }
    }
}
