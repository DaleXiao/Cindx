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
    AgentPolicy, FrozenPromptProfileSnapshot, OrchestrationPolicy, ProTeacherAttestationV1,
};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use tauri::Manager;

const ROLLOUT_EVENT: &str = "Conductor prompt rollout updated";
const REQUEST_EVENT: &str = "Conductor prompt evaluation requested";
const DISPATCHED_EVENT: &str = "Conductor Pro distillation dispatched";
const INTENT_ID_KEY: &str = "prompt_pro_distillation_intent_id";

#[derive(Clone)]
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
        let intent_id = format!("prompt-pro-distillation-{digest}");
        let run_context = persistent_context(&event.metadata, project_id);
        Some(Self {
            request_id: format!("prompt-evaluation-{intent_id}"),
            intent_id,
            run_context,
            snapshot,
            attestation,
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
    let events = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .list_by_task_and_metadata(&phase16_task_id(), "prompt_effort", "pro")
            .map_err(|error| error.to_string())?
    };
    let dispatched = events
        .iter()
        .filter(|event| event.summary == DISPATCHED_EVENT)
        .filter_map(|event| event.metadata.get(INTENT_ID_KEY).cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let mut intents = BTreeMap::new();
    for event in &events {
        if let Some(intent) = PromptProDistillationIntent::from_rollout(event) {
            intents.insert(intent.intent_id.clone(), intent);
        }
    }
    for (intent_id, intent) in intents {
        if dispatched.contains(&intent_id) {
            continue;
        }
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
        if !request_exists {
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
    }
    Ok(())
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
        phase16_task_id, Event, EventKind, Metadata, PromptProDistillationIntent, ROLLOUT_EVENT,
    };

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
}
