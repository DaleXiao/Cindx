use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_read_model::load_prompt_evolution_read_model;
use crate::runtime_values::phase16_task_id;
use crate::view_models::PromptEvaluationAttemptState;
use agent_core::{Event, EventKind, Metadata, TaskId};
use agent_storage::SqliteStore;
use orchestrator::{
    sha256_hex, PromptEvaluationAttemptEventV1, PromptEvaluationAttemptStatus,
    PromptEvolutionObservation, PromptLearningCohortV1, PromptMatchedEvaluationIdentityV1,
};
use std::collections::BTreeMap;
use tauri::Manager;

pub(crate) const PROMPT_EVALUATION_ATTEMPT_EVENT: &str =
    "Conductor prompt evaluation attempt";
pub(crate) const PROMPT_EVALUATION_ATTEMPT_METADATA_KEY: &str =
    "prompt_evaluation_attempt_v1";
pub(crate) const PROMPT_LEARNING_COHORT_EVENT: &str = "Conductor prompt learning cohort";
pub(crate) const PROMPT_LEARNING_COHORT_METADATA_KEY: &str = "prompt_learning_cohort_v1";
pub(crate) const PROMPT_LEARNING_COHORT_RETENTION: usize = 256;

pub(crate) fn prompt_matched_identity_belongs_to_cohort(
    identity: &PromptMatchedEvaluationIdentityV1,
    cohort: &PromptLearningCohortV1,
) -> bool {
    cohort.cohort_sha256 == identity.cohort_sha256
        && cohort.dataset.dataset_sha256 == identity.dataset_sha256
        && cohort.dataset.case(&identity.case_id).is_some_and(|case| {
            case.objective_sha256 == identity.objective_sha256 && case.split == identity.split
        })
}

pub(crate) fn prompt_observation_matches_attempt(
    observation: &PromptEvolutionObservation,
    started: &PromptEvaluationAttemptEventV1,
) -> bool {
    observation.provenance.matched_evaluation.as_ref() == Some(&started.identity)
        && started.treatments.iter().any(|candidate| {
            candidate.profile_id == observation.profile_id
                && candidate.prompt_sha256 == observation.provenance.candidate_prompt_sha256
                && started.treatments.iter().any(|opponent| {
                    Some(opponent.profile_id.as_str())
                        == observation.opponent_profile_id.as_deref()
                        && opponent.prompt_sha256
                            == observation.provenance.opponent_prompt_sha256
                        && opponent.profile_id != candidate.profile_id
                })
        })
}

pub(crate) fn append_prompt_learning_cohort_if_missing(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    cohort: &PromptLearningCohortV1,
) -> Result<(), String> {
    cohort.validate()?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let model = load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
    if let Some(existing) = model.cohorts.get(&cohort.cohort_sha256) {
        return (existing == cohort)
            .then_some(())
            .ok_or_else(|| "prompt learning cohort digest collision".to_string());
    }
    let encoded = serde_json::to_string(cohort)
        .map_err(|error| format!("prompt learning cohort serialization failed: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        PROMPT_LEARNING_COHORT_EVENT,
        metadata_with_context(
            [
                ("background_evaluation".to_string(), "true".to_string()),
                ("prompt_effort".to_string(), effort.to_string()),
                (
                    "prompt_learning_cohort_sha256".to_string(),
                    cohort.cohort_sha256.clone(),
                ),
                (PROMPT_LEARNING_COHORT_METADATA_KEY.to_string(), encoded),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

pub(crate) fn prompt_learning_cohort_from_event(
    event: &Event,
) -> Option<PromptLearningCohortV1> {
    if event.summary != PROMPT_LEARNING_COHORT_EVENT {
        return None;
    }
    let cohort = event
        .metadata
        .get(PROMPT_LEARNING_COHORT_METADATA_KEY)
        .and_then(|encoded| serde_json::from_str::<PromptLearningCohortV1>(encoded).ok())?;
    cohort.validate().ok()?;
    let project_id = event.metadata.get("project_id")?;
    (event.metadata.get("prompt_learning_cohort_sha256") == Some(&cohort.cohort_sha256)
        && cohort.dataset.scope_sha256 == sha256_hex(project_id.as_bytes()))
    .then_some(cohort)
}

pub(crate) fn append_prompt_evaluation_attempt_event(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    event: &PromptEvaluationAttemptEventV1,
) -> Result<(), String> {
    event.validate()?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    if event.status == PromptEvaluationAttemptStatus::Started {
        let model =
            load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
        prompt_evaluation_attempt_start_is_admitted(&model.attempts, event)?;
    }
    append_prompt_evaluation_attempt_event_to_store(
        &mut store,
        task_id,
        run_context,
        effort,
        event,
    )
}

fn prompt_evaluation_attempt_start_is_admitted(
    attempts: &BTreeMap<String, PromptEvaluationAttemptState>,
    event: &PromptEvaluationAttemptEventV1,
) -> Result<(), String> {
    let new_attempt = !attempts.contains_key(&event.identity.evaluation_id);
    let retention_full = attempts.len()
        >= crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION;
    let no_terminal_can_be_evicted = attempts
        .values()
        .all(|attempt| attempt.terminal.is_none());
    if new_attempt && retention_full && no_terminal_can_be_evicted {
        return Err("prompt evaluation attempt retention is full".to_string());
    }
    Ok(())
}

fn append_prompt_evaluation_attempt_event_to_store(
    store: &mut SqliteStore,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    event: &PromptEvaluationAttemptEventV1,
) -> Result<(), String> {
    event.validate()?;
    let encoded = serde_json::to_string(event)
        .map_err(|error| format!("prompt evaluation attempt serialization failed: {error}"))?;
    append_event(
        store,
        task_id,
        EventKind::TaskStatusChanged,
        PROMPT_EVALUATION_ATTEMPT_EVENT,
        metadata_with_context(
            [
                ("background_evaluation".to_string(), "true".to_string()),
                ("prompt_effort".to_string(), effort.to_string()),
                (
                    "prompt_evaluation_id".to_string(),
                    event.identity.evaluation_id.clone(),
                ),
                (
                    PROMPT_EVALUATION_ATTEMPT_METADATA_KEY.to_string(),
                    encoded,
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

fn prompt_evaluation_recovery_terminals(
    attempts: &BTreeMap<String, PromptEvaluationAttemptState>,
) -> Result<Vec<PromptEvaluationAttemptEventV1>, String> {
    attempts
        .values()
        .take(crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION)
        .filter(|attempt| attempt.terminal.is_none())
        .map(|attempt| {
            PromptEvaluationAttemptEventV1::terminal(
                &attempt.started,
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                [true; 2],
                "recovered_incomplete_attempt",
            )
        })
        .collect()
}

pub(crate) fn reconcile_incomplete_prompt_evaluation_attempts(
    state: &tauri::State<'_, AppState>,
) -> Result<usize, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let model = load_prompt_evolution_read_model(&mut store).map_err(|error| error.to_string())?;
    let terminals = prompt_evaluation_recovery_terminals(&model.attempts)?;
    for terminal in &terminals {
        let scope = terminal
            .identity
            .evaluation_id
            .split_once("::")
            .map(|(scope, _)| scope)
            .filter(|scope| !scope.trim().is_empty())
            .unwrap_or("global");
        let effort = model
            .cohorts
            .get(&terminal.identity.cohort_sha256)
            .map(|cohort| cohort.execution.policy.label())
            .unwrap_or("unknown");
        let run_context = [("project_id".to_string(), scope.to_string())]
            .into_iter()
            .collect::<Metadata>();
        append_prompt_evaluation_attempt_event_to_store(
            &mut store,
            &phase16_task_id(),
            &run_context,
            effort,
            terminal,
        )?;
    }
    Ok(terminals.len())
}

pub(crate) fn recover_prompt_evaluation_attempts_at_worker_start(
    app: &tauri::AppHandle,
) -> bool {
    let state = app.state::<AppState>();
    if let Err(error) = reconcile_incomplete_prompt_evaluation_attempts(&state) {
        eprintln!("prompt evaluation attempt recovery failed: {error}");
        return false;
    }
    true
}

pub(crate) fn prompt_evaluation_attempt_from_event(
    event: &Event,
) -> Option<PromptEvaluationAttemptEventV1> {
    if event.summary != PROMPT_EVALUATION_ATTEMPT_EVENT {
        return None;
    }
    let attempt = event
        .metadata
        .get(PROMPT_EVALUATION_ATTEMPT_METADATA_KEY)
        .and_then(|encoded| serde_json::from_str::<PromptEvaluationAttemptEventV1>(encoded).ok())?;
    attempt.validate().ok()?;
    (event.metadata.get("prompt_evaluation_id") == Some(&attempt.identity.evaluation_id))
        .then_some(attempt)
}

pub(crate) struct PromptEvaluationAttemptGuard<'a, 'state> {
    state: &'a tauri::State<'state, AppState>,
    task_id: &'a TaskId,
    run_context: &'a Metadata,
    effort: &'a str,
    started: PromptEvaluationAttemptEventV1,
    treatment_failures: [bool; 2],
    terminal: bool,
}

impl<'a, 'state> PromptEvaluationAttemptGuard<'a, 'state> {
    pub(crate) fn start(
        state: &'a tauri::State<'state, AppState>,
        task_id: &'a TaskId,
        run_context: &'a Metadata,
        effort: &'a str,
        cohort: &PromptLearningCohortV1,
        started: PromptEvaluationAttemptEventV1,
    ) -> Result<Self, String> {
        if started.status != PromptEvaluationAttemptStatus::Started {
            return Err("prompt evaluation attempt guard requires a start".to_string());
        }
        if started.identity.cohort_sha256 != cohort.cohort_sha256 {
            return Err("prompt evaluation attempt guard cohort is mismatched".to_string());
        }
        append_prompt_learning_cohort_if_missing(
            state,
            task_id,
            run_context,
            effort,
            cohort,
        )?;
        append_prompt_evaluation_attempt_event(state, task_id, run_context, effort, &started)?;
        Ok(Self {
            state,
            task_id,
            run_context,
            effort,
            started,
            treatment_failures: [false; 2],
            terminal: false,
        })
    }

    pub(crate) fn mark_treatment_failures(&mut self, failures: [bool; 2]) {
        for (current, failed) in self.treatment_failures.iter_mut().zip(failures) {
            *current |= failed;
        }
    }

    pub(crate) fn finish(
        &mut self,
        status: PromptEvaluationAttemptStatus,
        reason_code: &str,
    ) -> Result<(), String> {
        if self.terminal {
            return Ok(());
        }
        let terminal = PromptEvaluationAttemptEventV1::terminal(
            &self.started,
            status,
            self.treatment_failures,
            reason_code,
        )?;
        append_prompt_evaluation_attempt_event(
            self.state,
            self.task_id,
            self.run_context,
            self.effort,
            &terminal,
        )?;
        self.terminal = true;
        Ok(())
    }
}

impl Drop for PromptEvaluationAttemptGuard<'_, '_> {
    fn drop(&mut self) {
        if self.terminal {
            return;
        }
        let Ok(terminal) = PromptEvaluationAttemptEventV1::terminal(
            &self.started,
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            [true; 2],
            "attempt_abandoned",
        ) else {
            return;
        };
        let _ = append_prompt_evaluation_attempt_event(
            self.state,
            self.task_id,
            self.run_context,
            self.effort,
            &terminal,
        );
        self.terminal = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, EventKind};
    use orchestrator::{
        AgentPolicy, PromptDatasetCaseIdentityV1, PromptDatasetIdentityV1, PromptEvaluationMode,
        PromptEvaluationSplit, PromptExecutionContextV1, PromptLearningCohortV1,
        PromptMatchedEvaluationIdentityV1, PromptTreatmentIdentityV1,
        PROMPT_EXECUTION_CONTEXT_SCHEMA_V1,
    };

    fn cohort_fixture() -> PromptLearningCohortV1 {
        let dataset = PromptDatasetIdentityV1::new(
            "scope",
            1,
            vec![PromptDatasetCaseIdentityV1 {
                case_id: "case".to_string(),
                objective_sha256: "c".repeat(64),
                task_family_sha256: "f".repeat(64),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap();
        let context = PromptExecutionContextV1 {
            schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            harness_sha256: "3".repeat(64),
            system_prompt_sha256: "4".repeat(64),
            policy: AgentPolicy::Auto,
            policy_sha256: "5".repeat(64),
            budget_sha256: "6".repeat(64),
            tool_contract_sha256: "7".repeat(64),
            source_revision_sha256: "8".repeat(64),
            workspace_revision_sha256: "9".repeat(64),
        };
        PromptLearningCohortV1::new(dataset, context).unwrap()
    }

    fn attempt_fixture(
        cohort: &PromptLearningCohortV1,
        index: usize,
    ) -> PromptEvaluationAttemptEventV1 {
        PromptEvaluationAttemptEventV1::started(
            PromptMatchedEvaluationIdentityV1::new(
                format!("scope::attempt-{index:04}"),
                cohort,
                "case",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
            )
            .unwrap(),
            cohort,
            [
                PromptTreatmentIdentityV1 {
                    profile_id: "a".to_string(),
                    prompt_sha256: "d".repeat(64),
                },
                PromptTreatmentIdentityV1 {
                    profile_id: "b".to_string(),
                    prompt_sha256: "e".repeat(64),
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn malformed_attempt_event_is_not_projected() {
        let dataset = PromptDatasetIdentityV1::new(
            "scope",
            1,
            vec![PromptDatasetCaseIdentityV1 {
                case_id: "case".to_string(),
                objective_sha256: "c".repeat(64),
                task_family_sha256: "f".repeat(64),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap();
        let context = PromptExecutionContextV1 {
            schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            harness_sha256: "3".repeat(64),
            system_prompt_sha256: "4".repeat(64),
            policy: AgentPolicy::Auto,
            policy_sha256: "5".repeat(64),
            budget_sha256: "6".repeat(64),
            tool_contract_sha256: "7".repeat(64),
            source_revision_sha256: "8".repeat(64),
            workspace_revision_sha256: "9".repeat(64),
        };
        let cohort = PromptLearningCohortV1::new(dataset, context).unwrap();
        let identity = PromptMatchedEvaluationIdentityV1::new(
            "scope::attempt",
            &cohort,
            "case",
            PromptEvaluationSplit::Train,
            PromptEvaluationMode::PairedExecution,
        )
        .unwrap();
        let attempt = PromptEvaluationAttemptEventV1::started(
            identity,
            &cohort,
            [
                PromptTreatmentIdentityV1 {
                    profile_id: "a".to_string(),
                    prompt_sha256: "d".repeat(64),
                },
                PromptTreatmentIdentityV1 {
                    profile_id: "b".to_string(),
                    prompt_sha256: "e".repeat(64),
                },
            ],
        )
        .unwrap();
        let mut metadata = Metadata::new();
        metadata.insert(
            PROMPT_EVALUATION_ATTEMPT_METADATA_KEY.to_string(),
            serde_json::to_string(&attempt).unwrap(),
        );
        metadata.insert(
            "prompt_evaluation_id".to_string(),
            "different".to_string(),
        );
        let event = Event {
            id: EventId("attempt".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: PROMPT_EVALUATION_ATTEMPT_EVENT.to_string(),
            metadata,
        };
        assert!(prompt_evaluation_attempt_from_event(&event).is_none());
    }

    #[test]
    fn cohort_event_is_bound_to_its_project_scope() {
        let dataset = PromptDatasetIdentityV1::new(
            "scope",
            1,
            vec![PromptDatasetCaseIdentityV1 {
                case_id: "case".to_string(),
                objective_sha256: "c".repeat(64),
                task_family_sha256: "f".repeat(64),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap();
        let context = PromptExecutionContextV1 {
            schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            harness_sha256: "3".repeat(64),
            system_prompt_sha256: "4".repeat(64),
            policy: AgentPolicy::Auto,
            policy_sha256: "5".repeat(64),
            budget_sha256: "6".repeat(64),
            tool_contract_sha256: "7".repeat(64),
            source_revision_sha256: "8".repeat(64),
            workspace_revision_sha256: "9".repeat(64),
        };
        let cohort = PromptLearningCohortV1::new(dataset, context).unwrap();
        let event = Event {
            id: EventId("cohort".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: PROMPT_LEARNING_COHORT_EVENT.to_string(),
            metadata: [
                ("project_id".to_string(), "scope".to_string()),
                (
                    "prompt_learning_cohort_sha256".to_string(),
                    cohort.cohort_sha256.clone(),
                ),
                (
                    PROMPT_LEARNING_COHORT_METADATA_KEY.to_string(),
                    serde_json::to_string(&cohort).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(prompt_learning_cohort_from_event(&event), Some(cohort));
        let mut wrong_scope = event;
        wrong_scope
            .metadata
            .insert("project_id".to_string(), "other".to_string());
        assert!(prompt_learning_cohort_from_event(&wrong_scope).is_none());
    }

    #[test]
    fn startup_recovery_terminates_only_incomplete_retained_attempts_once() {
        let cohort = cohort_fixture();
        let treatments = [
            PromptTreatmentIdentityV1 {
                profile_id: "a".to_string(),
                prompt_sha256: "d".repeat(64),
            },
            PromptTreatmentIdentityV1 {
                profile_id: "b".to_string(),
                prompt_sha256: "e".repeat(64),
            },
        ];
        let incomplete = PromptEvaluationAttemptEventV1::started(
            PromptMatchedEvaluationIdentityV1::new(
                "scope::incomplete",
                &cohort,
                "case",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
            )
            .unwrap(),
            &cohort,
            treatments.clone(),
        )
        .unwrap();
        let completed = PromptEvaluationAttemptEventV1::started(
            PromptMatchedEvaluationIdentityV1::new(
                "scope::completed",
                &cohort,
                "case",
                PromptEvaluationSplit::Train,
                PromptEvaluationMode::PairedExecution,
            )
            .unwrap(),
            &cohort,
            treatments,
        )
        .unwrap();
        let completed_terminal = PromptEvaluationAttemptEventV1::terminal(
            &completed,
            PromptEvaluationAttemptStatus::CompletedPair,
            [false; 2],
            "",
        )
        .unwrap();
        let attempts = BTreeMap::from([
            (
                incomplete.identity.evaluation_id.clone(),
                PromptEvaluationAttemptState {
                    started: incomplete.clone(),
                    terminal: None,
                },
            ),
            (
                completed.identity.evaluation_id.clone(),
                PromptEvaluationAttemptState {
                    started: completed,
                    terminal: Some(completed_terminal),
                },
            ),
        ]);

        let recovered = prompt_evaluation_recovery_terminals(&attempts).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].identity, incomplete.identity);
        assert_eq!(
            recovered[0].status,
            PromptEvaluationAttemptStatus::InfrastructureInvalid
        );
        assert_eq!(recovered[0].treatment_failures, [true; 2]);
        assert!(recovered[0].enters_effect_denominator());
        let recovered_event = Event {
            id: EventId("recovered-attempt".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 2,
            kind: EventKind::TaskStatusChanged,
            summary: PROMPT_EVALUATION_ATTEMPT_EVENT.to_string(),
            metadata: [
                (
                    "prompt_evaluation_id".to_string(),
                    recovered[0].identity.evaluation_id.clone(),
                ),
                (
                    PROMPT_EVALUATION_ATTEMPT_METADATA_KEY.to_string(),
                    serde_json::to_string(&recovered[0]).unwrap(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(
            prompt_evaluation_attempt_from_event(&recovered_event),
            Some(recovered[0].clone())
        );

        let projected = BTreeMap::from([(
            incomplete.identity.evaluation_id.clone(),
            PromptEvaluationAttemptState {
                started: incomplete,
                terminal: Some(recovered[0].clone()),
            },
        )]);
        assert!(prompt_evaluation_recovery_terminals(&projected)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn terminal_attempt_retention_evicts_before_admitting_the_1025th() {
        let cohort = cohort_fixture();
        let mut attempts = BTreeMap::new();
        for index in 0..=crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION {
            let started = attempt_fixture(&cohort, index);
            let terminal = PromptEvaluationAttemptEventV1::terminal(
                &started,
                PromptEvaluationAttemptStatus::CompletedPair,
                [false; 2],
                "",
            )
            .unwrap();
            crate::prompt_evolution_read_model::upsert_prompt_evaluation_attempt(
                &mut attempts,
                started,
            );
            crate::prompt_evolution_read_model::upsert_prompt_evaluation_attempt(
                &mut attempts,
                terminal,
            );
        }

        assert_eq!(
            attempts.len(),
            crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION
        );
        assert!(!attempts.contains_key("scope::attempt-0000"));
        assert!(attempts.contains_key("scope::attempt-1024"));
    }

    #[test]
    fn incomplete_attempt_retention_rejects_and_drops_the_1025th_start() {
        let cohort = cohort_fixture();
        let mut attempts = BTreeMap::new();
        for index in 0..crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION {
            crate::prompt_evolution_read_model::upsert_prompt_evaluation_attempt(
                &mut attempts,
                attempt_fixture(&cohort, index),
            );
        }
        let overflow = attempt_fixture(
            &cohort,
            crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION,
        );

        assert!(prompt_evaluation_attempt_start_is_admitted(&attempts, &overflow).is_err());
        crate::prompt_evolution_read_model::upsert_prompt_evaluation_attempt(
            &mut attempts,
            overflow,
        );
        assert_eq!(
            attempts.len(),
            crate::prompt_evolution_read_model::PROMPT_EVALUATION_ATTEMPT_RETENTION
        );
        assert!(!attempts.contains_key("scope::attempt-1024"));
    }
}
