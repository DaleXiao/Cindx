use crate::app_state::AppState;
use crate::collaboration_models::PromptOfflineCase;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_attempt_runtime::{
    prompt_matched_identity_belongs_to_cohort, prompt_observation_matches_attempt,
};
use crate::prompt_evolution_read_model::{
    load_prompt_evolution_read_model, prompt_evolution_read_model_for_scope,
};
use crate::prompt_rollout_runtime::{
    canonical_prompt_profile_fingerprint_by_id, stable_prompt_profile_fingerprint,
};
use crate::view_models::{PromptEvolutionReadModel, PromptRolloutState};
use agent_core::{Event, EventKind, Metadata, TaskId};
use orchestrator::{
    derive_pro_to_auto_distillation_child, prompt_genome_sha256, sha256_hex, ConductorPromptGenome,
    FrozenPromptProfileSnapshot, ProTeacherAttestationV1, PromptDatasetCaseIdentityV1,
    PromptDatasetIdentityV1, PromptEvaluationMode, PromptEvaluationSplit,
    PromptEvolutionObservation, PromptProToAutoDistillationProvenanceV1,
};
use std::collections::BTreeSet;

pub(crate) const DISTILLATION_EVALUATION_EVENT: &str =
    "Conductor Pro-to-Auto distillation evaluation";

pub(crate) fn prompt_distillation_provenance_from_event(
    event: &Event,
) -> Option<PromptProToAutoDistillationProvenanceV1> {
    if event.summary != DISTILLATION_EVALUATION_EVENT
        || event.metadata.get("mutation_strategy").map(String::as_str)
            != Some("pro_to_auto_distillation")
    {
        return None;
    }
    let mut unique = event
        .metadata
        .get("prompt_observations")
        .and_then(|encoded| serde_json::from_str::<Vec<PromptEvolutionObservation>>(encoded).ok())?
        .into_iter()
        .filter_map(|observation| observation.provenance.pro_to_auto_distillation)
        .filter(|provenance| provenance.validate().is_ok())
        .fold(Vec::new(), |mut unique, provenance| {
            if !unique.contains(&provenance) {
                unique.push(provenance);
            }
            unique
        });
    (unique.len() == 1).then(|| unique.remove(0))
}

#[derive(Clone)]
pub(crate) struct PromptDistillationEvaluation {
    pub(crate) snapshot: FrozenPromptProfileSnapshot,
    pub(crate) provenance: PromptProToAutoDistillationProvenanceV1,
    pub(crate) child: ConductorPromptGenome,
    pub(crate) dataset: PromptDatasetIdentityV1,
    cases: Vec<PromptOfflineCase>,
}

pub(crate) fn prepare_prompt_distillation_evaluation(
    model: &PromptEvolutionReadModel,
    auto_parent: &ConductorPromptGenome,
    snapshot: &FrozenPromptProfileSnapshot,
    cases: &[PromptOfflineCase],
) -> Result<PromptDistillationEvaluation, String> {
    let (canonical_auto_parent, auto_parent_sha256) =
        stable_prompt_profile_fingerprint(model, "auto")?;
    if canonical_auto_parent != *auto_parent {
        return Err("distillation request Auto parent is no longer stable".to_string());
    }
    let pro_rollout = canonical_pro_teacher_rollout(model, snapshot)?;
    let (defeated_stable_pro, _) =
        canonical_prompt_profile_fingerprint_by_id(model, "pro", &snapshot.stable_profile_id)?;
    let child = derive_pro_to_auto_distillation_child(
        auto_parent,
        snapshot,
        &defeated_stable_pro,
        &pro_rollout.stable_profile_id,
    )?;
    let child_sha256 = prompt_genome_sha256(&child)?;
    let teacher =
        ProTeacherAttestationV1::from_stable_snapshot(snapshot, &pro_rollout.stable_profile_id)?;
    let provenance = PromptProToAutoDistillationProvenanceV1::new(
        teacher,
        auto_parent.id.clone(),
        auto_parent_sha256,
        child.id.clone(),
        child_sha256,
    )?;
    validate_prompt_distillation_lineage(model, auto_parent, snapshot, &child, &provenance)?;
    let cases = fresh_prompt_distillation_cases(model, &provenance.teacher, cases)?;
    if !prompt_distillation_cases_are_sufficient(&cases) {
        return Err("distillation has insufficient fresh train/holdout cases".to_string());
    }
    let dataset = prompt_distillation_dataset_identity(&cases, &provenance, child.generation)?;
    if !provenance.permits_evaluation_digest_lineage(&dataset.dataset_sha256, None) {
        return Err("distillation dataset reuses Pro teacher source evidence".to_string());
    }
    Ok(PromptDistillationEvaluation {
        snapshot: snapshot.clone(),
        provenance,
        child,
        dataset,
        cases,
    })
}

pub(crate) fn prompt_distillation_cases_are_sufficient(cases: &[PromptOfflineCase]) -> bool {
    cases
        .iter()
        .filter(|case| case.split == PromptEvaluationSplit::Train)
        .count()
        >= crate::runtime_constants::PROMPT_EVOLUTION_MIN_TRAIN_RUNS
        && cases
            .iter()
            .filter(|case| case.split == PromptEvaluationSplit::Holdout)
            .count()
            >= crate::runtime_constants::PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS
}

pub(crate) fn fresh_prompt_distillation_cases(
    model: &PromptEvolutionReadModel,
    teacher: &ProTeacherAttestationV1,
    cases: &[PromptOfflineCase],
) -> Result<Vec<PromptOfflineCase>, String> {
    let (source_cases, mut source_objectives) = pro_teacher_source_manifest(model, teacher)?;
    source_objectives.extend(
        cases
            .iter()
            .filter(|case| source_cases.contains(&case.id))
            .map(|case| sha256_hex(case.objective.trim().as_bytes())),
    );
    Ok(cases
        .iter()
        .filter(|case| {
            !source_cases.contains(&case.id)
                && !source_objectives.contains(&sha256_hex(case.objective.trim().as_bytes()))
        })
        .cloned()
        .collect())
}

fn pro_teacher_source_manifest(
    model: &PromptEvolutionReadModel,
    teacher: &ProTeacherAttestationV1,
) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    teacher.validate()?;
    let mut required_datasets = BTreeSet::from([
        teacher.ordinary_dataset_sha256.clone(),
        teacher.auto_transfer_dataset_sha256.clone(),
    ]);
    required_datasets.extend(
        teacher
            .auto_source_lineage
            .ancestor_dataset_sha256
            .iter()
            .cloned(),
    );
    let mut source_cases = BTreeSet::new();
    let mut source_objectives = BTreeSet::new();
    for digest in teacher.excluded_evidence_sha256() {
        let mut resolved_dataset;
        let mut resolved_cohort = false;
        let mut identities = model
            .datasets
            .values()
            .filter_map(|dataset| dataset.identity.as_ref())
            .filter(|identity| identity.dataset_sha256 == digest)
            .collect::<Vec<_>>();
        resolved_dataset = !identities.is_empty();
        for cohort in model.cohorts.values() {
            if cohort.cohort_sha256 == digest {
                cohort.validate()?;
                resolved_cohort = true;
                identities.push(&cohort.dataset);
            } else if cohort.dataset.dataset_sha256 == digest {
                cohort.validate()?;
                resolved_dataset = true;
                identities.push(&cohort.dataset);
            }
        }
        if required_datasets.contains(&digest) && !resolved_dataset {
            return Err("Pro teacher source dataset manifest is unavailable".to_string());
        }
        if digest == teacher.auto_transfer_cohort_sha256 && !resolved_cohort {
            return Err("Pro teacher source cohort manifest is unavailable".to_string());
        }
        for identity in identities {
            identity.validate()?;
            for case in &identity.cases {
                source_cases.insert(case.case_id.clone());
                source_objectives.insert(case.objective_sha256.clone());
            }
        }
    }
    Ok((source_cases, source_objectives))
}

fn validate_prompt_distillation_lineage(
    model: &PromptEvolutionReadModel,
    auto_parent: &ConductorPromptGenome,
    snapshot: &FrozenPromptProfileSnapshot,
    child: &ConductorPromptGenome,
    provenance: &PromptProToAutoDistillationProvenanceV1,
) -> Result<(), String> {
    let rollout = canonical_pro_teacher_rollout(model, snapshot)?;
    let (canonical_auto_parent, parent_sha256) = stable_prompt_profile_fingerprint(model, "auto")?;
    if canonical_auto_parent != *auto_parent {
        return Err("distillation Auto parent is no longer canonical stable".to_string());
    }
    let (defeated_stable_pro, _) =
        canonical_prompt_profile_fingerprint_by_id(model, "pro", &snapshot.stable_profile_id)?;
    let expected_child = derive_pro_to_auto_distillation_child(
        auto_parent,
        snapshot,
        &defeated_stable_pro,
        &rollout.stable_profile_id,
    )?;
    let expected = PromptProToAutoDistillationProvenanceV1::new(
        ProTeacherAttestationV1::from_stable_snapshot(snapshot, &rollout.stable_profile_id)?,
        auto_parent.id.clone(),
        parent_sha256,
        expected_child.id.clone(),
        prompt_genome_sha256(&expected_child)?,
    )?;
    if expected_child != *child || expected != *provenance {
        return Err("distillation lineage does not match the canonical bounded child".to_string());
    }
    Ok(())
}

fn canonical_pro_teacher_rollout<'a>(
    model: &'a PromptEvolutionReadModel,
    snapshot: &FrozenPromptProfileSnapshot,
) -> Result<&'a PromptRolloutState, String> {
    let rollout = model
        .rollouts
        .get("pro")
        .ok_or_else(|| "canonical Pro rollout is unavailable".to_string())?;
    if !matches!(rollout.status.as_str(), "promoted" | "stable")
        || rollout.stable_profile_id != snapshot.genome.id
        || rollout.frozen_profile.as_ref() != Some(snapshot)
    {
        return Err("Pro teacher is not the canonical frozen stable champion".to_string());
    }
    ProTeacherAttestationV1::from_stable_snapshot(snapshot, &rollout.stable_profile_id)?;
    Ok(rollout)
}

pub(crate) fn replay_canonical_pro_teacher_snapshot(
    model: &PromptEvolutionReadModel,
    snapshot: &FrozenPromptProfileSnapshot,
) -> Result<(), String> {
    canonical_pro_teacher_rollout(model, snapshot)?;
    let replayed = crate::prompt_rollout_runtime::frozen_prompt_profile_for_promotion(
        model,
        "pro",
        &snapshot.genome.id,
        &snapshot.stable_profile_id,
    )?;
    if replayed != *snapshot {
        return Err(
            "Pro teacher frozen evidence does not replay from the canonical dual gate".to_string(),
        );
    }
    Ok(())
}

fn prompt_distillation_dataset_identity(
    cases: &[PromptOfflineCase],
    provenance: &PromptProToAutoDistillationProvenanceV1,
    generation: u32,
) -> Result<PromptDatasetIdentityV1, String> {
    provenance.validate()?;
    let project_id = cases
        .first()
        .map(|case| case.project_id.as_str())
        .ok_or_else(|| "distillation dataset is empty".to_string())?;
    if cases.iter().any(|case| case.project_id != project_id) {
        return Err("distillation dataset crosses project scopes".to_string());
    }
    let scope = format!(
        "pro_to_auto_distillation\n{project_id}\n{}",
        provenance.teacher.teacher_snapshot_sha256
    );
    PromptDatasetIdentityV1::new(
        &scope,
        generation,
        cases
            .iter()
            .map(|case| PromptDatasetCaseIdentityV1 {
                case_id: case.id.clone(),
                objective_sha256: sha256_hex(case.objective.trim().as_bytes()),
                task_family_sha256: sha256_hex(case.task_class.trim().as_bytes()),
                split: case.split,
            })
            .collect(),
    )
}

pub(crate) fn prompt_distillation_evidence_counts(
    observations: &[PromptEvolutionObservation],
    provenance: &PromptProToAutoDistillationProvenanceV1,
    dataset_sha256: &str,
) -> (usize, usize) {
    let mut train = BTreeSet::new();
    let mut holdout = BTreeSet::new();
    for observation in observations.iter().filter(|observation| {
        observation.is_strict_pro_to_auto_distillation_evidence()
            && observation.profile_id == provenance.auto_child_profile_id
            && observation.provenance.pro_to_auto_distillation.as_ref() == Some(provenance)
            && observation.provenance.dataset_sha256 == dataset_sha256
    }) {
        let target = match observation.split {
            PromptEvaluationSplit::Train => &mut train,
            PromptEvaluationSplit::Holdout => &mut holdout,
        };
        target.insert(observation.evidence_identity());
    }
    (train.len(), holdout.len())
}

pub(crate) fn select_prompt_distillation_case(
    observations: &[PromptEvolutionObservation],
    evaluation: &PromptDistillationEvaluation,
    split: PromptEvaluationSplit,
) -> Option<PromptOfflineCase> {
    evaluation
        .cases
        .iter()
        .filter(|case| case.split == split)
        .min_by_key(|case| {
            let repeats = observations
                .iter()
                .filter(|observation| {
                    observation.case_id == case.id
                        && observation.split == split
                        && observation.is_strict_pro_to_auto_distillation_evidence()
                        && observation.provenance.pro_to_auto_distillation.as_ref()
                            == Some(&evaluation.provenance)
                        && observation.provenance.dataset_sha256
                            == evaluation.dataset.dataset_sha256
                })
                .map(PromptEvolutionObservation::evidence_identity)
                .collect::<BTreeSet<_>>()
                .len();
            (repeats, case.id.as_str())
        })
        .cloned()
}

fn validate_distillation_observations(
    observations: [&PromptEvolutionObservation; 2],
    expected: &PromptProToAutoDistillationProvenanceV1,
) -> Result<(), String> {
    if !observations
        .iter()
        .all(|observation| observation.is_strict_pro_to_auto_distillation_evidence())
        || observations[0].provenance.pro_to_auto_distillation
            != observations[1].provenance.pro_to_auto_distillation
        || observations[0].provenance.pro_to_auto_distillation.as_ref() != Some(expected)
        || observations[0].profile_id
            != observations[1]
                .opponent_profile_id
                .as_deref()
                .unwrap_or_default()
        || observations[1].profile_id
            != observations[0]
                .opponent_profile_id
                .as_deref()
                .unwrap_or_default()
        || observations[0].evaluation_id != observations[1].evaluation_id
        || observations[0].case_id != observations[1].case_id
        || observations[0].split != observations[1].split
        || observations[0].mode != observations[1].mode
    {
        return Err("distillation observations are not strict reciprocal matched evidence".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_prompt_distillation_observations(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    mode: PromptEvaluationMode,
    observations: [&PromptEvolutionObservation; 2],
    genomes: [&ConductorPromptGenome; 2],
    evaluation: &PromptDistillationEvaluation,
) -> Result<(), String> {
    validate_distillation_observations(observations, &evaluation.provenance)?;
    crate::prompt_evolution_store_runtime::with_prompt_evolution_store(state, |store| {
        let model = load_prompt_evolution_read_model(store).map_err(|error| error.to_string())?;
        let project_id = run_context
            .get("project_id")
            .ok_or_else(|| "distillation project scope is missing".to_string())?;
        let scoped = prompt_evolution_read_model_for_scope(&model, project_id);
        validate_prompt_distillation_lineage(
            &scoped,
            genomes[0],
            &evaluation.snapshot,
            &evaluation.child,
            &evaluation.provenance,
        )?;
        let identity = observations[0]
            .provenance
            .matched_evaluation
            .as_ref()
            .ok_or_else(|| "distillation observations are missing matched identity".to_string())?;
        let cohort = model
            .cohorts
            .get(&identity.cohort_sha256)
            .ok_or_else(|| "distillation cohort is not persisted".to_string())?;
        let attempt = model
            .attempts
            .get(&identity.evaluation_id)
            .ok_or_else(|| "distillation attempt is not persisted".to_string())?;
        if cohort.dataset != evaluation.dataset
            || validate_distillation_dataset_is_fresh(
                &model,
                &evaluation.provenance.teacher,
                &cohort.dataset,
            )
            .is_err()
            || !prompt_matched_identity_belongs_to_cohort(identity, cohort)
            || !observations.iter().all(|observation| {
                prompt_observation_matches_attempt(observation, &attempt.started)
            })
        {
            return Err("distillation observations do not match persisted evidence".to_string());
        }
        Ok(())
    })?;
    let encoded_observations = serde_json::to_string(&observations)
        .map_err(|error| format!("distillation observations serialization failed: {error}"))?;
    let encoded_genomes = serde_json::to_string(&genomes)
        .map_err(|error| format!("distillation genomes serialization failed: {error}"))?;
    let encoded_snapshot = serde_json::to_string(&evaluation.snapshot)
        .map_err(|error| format!("Pro teacher snapshot serialization failed: {error}"))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        DISTILLATION_EVALUATION_EVENT,
        metadata_with_context(
            [
                ("background_evaluation".to_string(), "true".to_string()),
                (
                    "evaluation_id".to_string(),
                    observations[0].evaluation_id.clone(),
                ),
                ("prompt_effort".to_string(), "auto".to_string()),
                ("prompt_genomes".to_string(), encoded_genomes),
                ("prompt_observations".to_string(), encoded_observations),
                (
                    "mutation_strategy".to_string(),
                    "pro_to_auto_distillation".to_string(),
                ),
                ("pro_teacher_snapshot".to_string(), encoded_snapshot),
                (
                    "pro_teacher_attestation_sha256".to_string(),
                    evaluation.provenance.teacher_attestation_sha256.clone(),
                ),
                (
                    "evaluation_mode".to_string(),
                    match mode {
                        PromptEvaluationMode::PairedExecution => "paired_execution",
                        PromptEvaluationMode::ReplayExecution => "replay_execution",
                        PromptEvaluationMode::PairedShadow => "paired_shadow",
                        PromptEvaluationMode::ReplayHoldout => "replay_holdout",
                        PromptEvaluationMode::Live => "live",
                    }
                    .to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

fn validate_distillation_dataset_is_fresh(
    model: &PromptEvolutionReadModel,
    teacher: &ProTeacherAttestationV1,
    identity: &PromptDatasetIdentityV1,
) -> Result<(), String> {
    let (source_cases, source_objectives) = pro_teacher_source_manifest(model, teacher)?;
    if identity.cases.iter().any(|case| {
        source_cases.contains(&case.case_id) || source_objectives.contains(&case.objective_sha256)
    }) {
        return Err("distillation cohort reuses Pro teacher source cases".to_string());
    }
    Ok(())
}

pub(crate) fn prompt_distillation_observations_from_event(
    model: &PromptEvolutionReadModel,
    event: &Event,
) -> Vec<(String, PromptEvolutionObservation)> {
    if event.summary != DISTILLATION_EVALUATION_EVENT {
        return Vec::new();
    }
    let Some(project_id) = event.metadata.get("project_id") else {
        return Vec::new();
    };
    let Some(snapshot) = event
        .metadata
        .get("pro_teacher_snapshot")
        .and_then(|encoded| FrozenPromptProfileSnapshot::from_json_slice(encoded.as_bytes()).ok())
    else {
        return Vec::new();
    };
    let observations = event
        .metadata
        .get("prompt_observations")
        .and_then(|encoded| serde_json::from_str::<Vec<PromptEvolutionObservation>>(encoded).ok())
        .unwrap_or_default();
    let Some([parent, child]) = event
        .metadata
        .get("prompt_genomes")
        .and_then(|encoded| serde_json::from_str::<Vec<ConductorPromptGenome>>(encoded).ok())
        .and_then(|genomes| <[ConductorPromptGenome; 2]>::try_from(genomes).ok())
    else {
        return Vec::new();
    };
    let Some([first, second]) = observations.as_slice().first_chunk::<2>() else {
        return Vec::new();
    };
    let scoped = prompt_evolution_read_model_for_scope(model, project_id);
    let Some(provenance) = prompt_distillation_provenance_from_event(event) else {
        return Vec::new();
    };
    if validate_prompt_distillation_lineage(&scoped, &parent, &snapshot, &child, &provenance)
        .is_err()
    {
        return Vec::new();
    }
    let Some(identity) = first.provenance.matched_evaluation.as_ref() else {
        return Vec::new();
    };
    let Some(cohort) = model.cohorts.get(&identity.cohort_sha256) else {
        return Vec::new();
    };
    let Some(attempt) = model.attempts.get(&identity.evaluation_id) else {
        return Vec::new();
    };
    if observations.len() != 2
        || cohort.dataset.dataset_sha256 != first.provenance.dataset_sha256
        || validate_distillation_dataset_is_fresh(model, &provenance.teacher, &cohort.dataset)
            .is_err()
        || !prompt_matched_identity_belongs_to_cohort(identity, cohort)
        || !observations
            .iter()
            .all(|observation| prompt_observation_matches_attempt(observation, &attempt.started))
        || validate_distillation_observations([first, second], &provenance).is_err()
        || event.metadata.get("pro_teacher_attestation_sha256")
            != Some(&provenance.teacher_attestation_sha256)
    {
        return Vec::new();
    }
    observations
        .into_iter()
        .map(|observation| ("auto".to_string(), observation))
        .collect()
}

pub(crate) fn apply_prompt_distillation_event(model: &mut PromptEvolutionReadModel, event: &Event) {
    for (effort, observation) in prompt_distillation_observations_from_event(model, event) {
        crate::prompt_evolution_read_model::upsert_prompt_observation(
            &mut model.observations,
            effort,
            observation,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fresh_prompt_distillation_cases, prepare_prompt_distillation_evaluation,
        prompt_genome_sha256, replay_canonical_pro_teacher_snapshot, sha256_hex,
        ConductorPromptGenome, FrozenPromptProfileSnapshot, ProTeacherAttestationV1,
        PromptDatasetCaseIdentityV1, PromptDatasetIdentityV1, PromptEvaluationSplit,
        PromptEvolutionReadModel, PromptOfflineCase, PromptRolloutState,
        DISTILLATION_EVALUATION_EVENT,
    };
    use crate::prompt_evolution_read_model::build_prompt_evolution_read_model;
    use crate::runtime_values::phase16_task_id;
    use crate::view_models::PromptOfflineDatasetState;
    use agent_core::{Event, EventId, EventKind};
    use orchestrator::{
        AgentPolicy, FrozenPromptSourceProfileLineageV1, FrozenPromptTransferEvidence,
        PromptEvaluationMode, PromptEvaluationProvenance, PromptEvolutionMethod,
        PromptExecutionContextV1, PromptLearningCohortV1, PROMPT_AUTO_TRANSFER_GATE_PROTOCOL,
        PROMPT_EXECUTION_CONTEXT_SCHEMA_V1,
    };

    struct CertifiedFixture {
        model: PromptEvolutionReadModel,
        auto_parent: ConductorPromptGenome,
        snapshot: FrozenPromptProfileSnapshot,
        teacher: ProTeacherAttestationV1,
        ancestor_dataset_sha256: Option<String>,
    }

    fn offline_case(
        id: impl Into<String>,
        objective: impl Into<String>,
        split: PromptEvaluationSplit,
    ) -> PromptOfflineCase {
        PromptOfflineCase {
            id: id.into(),
            objective: objective.into(),
            task_class: "general".to_string(),
            project_id: "project-a".to_string(),
            source_run_id: "source-run".to_string(),
            split,
            learning_receipt: None,
            auto_teacher: None,
        }
    }

    fn dataset_identity(
        scope: &str,
        case_id: &str,
        objective_sha256: String,
    ) -> PromptDatasetIdentityV1 {
        PromptDatasetIdentityV1::new(
            scope,
            1,
            vec![PromptDatasetCaseIdentityV1 {
                case_id: case_id.to_string(),
                objective_sha256,
                task_family_sha256: sha256_hex(b"general"),
                split: PromptEvaluationSplit::Train,
            }],
        )
        .unwrap()
    }

    fn execution_context() -> PromptExecutionContextV1 {
        PromptExecutionContextV1 {
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
        }
    }

    fn dataset_state(effort: &str, identity: PromptDatasetIdentityV1) -> PromptOfflineDatasetState {
        PromptOfflineDatasetState {
            effort: effort.to_string(),
            project_id: "project-a".to_string(),
            digest: identity.dataset_sha256.clone(),
            generation: identity.generation,
            case_ids: identity
                .cases
                .iter()
                .map(|case| case.case_id.clone())
                .collect(),
            case_count: identity.cases.len(),
            train_count: identity
                .cases
                .iter()
                .filter(|case| case.split == PromptEvaluationSplit::Train)
                .count(),
            holdout_count: identity
                .cases
                .iter()
                .filter(|case| case.split == PromptEvaluationSplit::Holdout)
                .count(),
            selected_case_id: None,
            status: "ready".to_string(),
            updated_at_ms: 1,
            identity: Some(identity),
        }
    }

    fn certified_fixture(with_ancestor: bool) -> CertifiedFixture {
        let auto_parent = ConductorPromptGenome::seed_for_effort("auto");
        let auto_parent_sha256 = prompt_genome_sha256(&auto_parent).unwrap();
        let ordinary = dataset_identity(
            "ordinary",
            "source-ordinary",
            sha256_hex(b"ordinary objective"),
        );
        let transfer = dataset_identity(
            "auto-transfer",
            "source-transfer",
            sha256_hex(b"auto-transfer-attested-objective"),
        );
        let transfer_cohort =
            PromptLearningCohortV1::new(transfer.clone(), execution_context()).unwrap();
        let ancestor = with_ancestor.then(|| {
            dataset_identity(
                "ancestor-distillation",
                "source-ancestor",
                sha256_hex(b"ancestor objective"),
            )
        });
        let ancestor_cohort = ancestor.as_ref().map(|identity| {
            PromptLearningCohortV1::new(identity.clone(), execution_context()).unwrap()
        });
        let ancestor_digests = ancestor
            .as_ref()
            .map(|identity| vec![identity.dataset_sha256.clone()])
            .unwrap_or_default();
        let lineage = FrozenPromptSourceProfileLineageV1::new(
            auto_parent_sha256.clone(),
            ancestor_digests.clone(),
            ancestor_digests,
        )
        .unwrap();
        let pro = ConductorPromptGenome::seed_for_effort("pro")
            .mutations()
            .into_iter()
            .next()
            .unwrap();
        let snapshot = FrozenPromptProfileSnapshot::new_gepa(
            "pro",
            pro.clone(),
            ConductorPromptGenome::seed_for_effort("pro").id,
            ordinary.dataset_sha256.clone(),
            "a".repeat(64),
        )
        .unwrap()
        .with_auto_teacher_evidence(FrozenPromptTransferEvidence {
            source_effort: "auto".to_string(),
            source_profile_id: auto_parent.id.clone(),
            source_profile_sha256: auto_parent_sha256,
            dataset_sha256: transfer.dataset_sha256.clone(),
            cohort_sha256: Some(transfer_cohort.cohort_sha256.clone()),
            paired_evidence_sha256: "b".repeat(64),
            promotion_gate_protocol: PROMPT_AUTO_TRANSFER_GATE_PROTOCOL.to_string(),
            source_profile_lineage: Some(lineage),
        })
        .unwrap();
        let teacher =
            ProTeacherAttestationV1::from_stable_snapshot(&snapshot, &snapshot.genome.id).unwrap();
        let mut model = build_prompt_evolution_read_model(&[], 0, 0);
        model
            .datasets
            .insert("pro".to_string(), dataset_state("pro", ordinary));
        model
            .cohorts
            .insert(transfer_cohort.cohort_sha256.clone(), transfer_cohort);
        if let Some(cohort) = ancestor_cohort {
            model.cohorts.insert(cohort.cohort_sha256.clone(), cohort);
        }
        model.rollouts.insert(
            "pro".to_string(),
            PromptRolloutState {
                stable_profile_id: pro.id,
                canary_profile_id: None,
                canary_percent: 0,
                evidence_checkpoint: 0,
                live_checkpoint: 0,
                stable_live_checkpoint: 0,
                quarantined_profile_ids: Vec::new(),
                distillation_lease: None,
                rollback_count: 0,
                status: "stable".to_string(),
                last_reason: None,
                promotion_confidence: None,
                frozen_profile: Some(snapshot.clone()),
            },
        );
        CertifiedFixture {
            model,
            auto_parent,
            snapshot,
            teacher,
            ancestor_dataset_sha256: ancestor.map(|identity| identity.dataset_sha256),
        }
    }

    fn fresh_campaign_cases() -> Vec<PromptOfflineCase> {
        (0..crate::runtime_constants::PROMPT_EVOLUTION_MIN_TRAIN_RUNS)
            .map(|index| {
                offline_case(
                    format!("fresh-train-{index}"),
                    format!("fresh train objective {index}"),
                    PromptEvaluationSplit::Train,
                )
            })
            .chain(
                (0..crate::runtime_constants::PROMPT_EVOLUTION_MIN_HOLDOUT_RUNS).map(|index| {
                    offline_case(
                        format!("fresh-holdout-{index}"),
                        format!("fresh holdout objective {index}"),
                        PromptEvaluationSplit::Holdout,
                    )
                }),
            )
            .collect()
    }

    #[test]
    fn distillation_filters_current_and_ancestor_source_cases_and_objectives() {
        let fixture = certified_fixture(true);
        let mut cases = fresh_campaign_cases();
        cases.extend([
            offline_case(
                "source-ordinary",
                "ordinary objective",
                PromptEvaluationSplit::Train,
            ),
            offline_case(
                "ordinary-objective-copy",
                "ordinary objective",
                PromptEvaluationSplit::Holdout,
            ),
            offline_case(
                "source-transfer",
                "transfer raw objective",
                PromptEvaluationSplit::Train,
            ),
            offline_case(
                "transfer-objective-copy",
                "transfer raw objective",
                PromptEvaluationSplit::Holdout,
            ),
            offline_case(
                "source-ancestor",
                "ancestor objective",
                PromptEvaluationSplit::Train,
            ),
            offline_case(
                "ancestor-objective-copy",
                "ancestor objective",
                PromptEvaluationSplit::Holdout,
            ),
        ]);

        let fresh = fresh_prompt_distillation_cases(&fixture.model, &fixture.teacher, &cases)
            .expect("all typed source manifests are available");
        let evaluation = prepare_prompt_distillation_evaluation(
            &fixture.model,
            &fixture.auto_parent,
            &fixture.snapshot,
            &cases,
        )
        .expect("fresh isolated train and holdout cases should prepare");

        assert_eq!(fresh, fresh_campaign_cases());
        assert_eq!(evaluation.cases, fresh);
        for case in &evaluation.dataset.cases {
            let source = evaluation
                .cases
                .iter()
                .find(|candidate| candidate.id == case.case_id)
                .unwrap();
            assert_eq!(
                case.objective_sha256,
                sha256_hex(source.objective.trim().as_bytes())
            );
        }
    }

    #[test]
    fn typed_ancestor_dataset_without_manifest_fails_closed() {
        let mut fixture = certified_fixture(true);
        let ancestor = fixture.ancestor_dataset_sha256.take().unwrap();
        fixture
            .model
            .cohorts
            .retain(|_, cohort| cohort.dataset.dataset_sha256 != ancestor);

        let error = fresh_prompt_distillation_cases(
            &fixture.model,
            &fixture.teacher,
            &fresh_campaign_cases(),
        )
        .expect_err("typed ancestor datasets must remain resolvable");

        assert!(error.contains("source dataset manifest is unavailable"));
    }

    #[test]
    fn stale_or_rolled_back_pro_teacher_cannot_prepare() {
        let mut fixture = certified_fixture(false);
        let rollout = fixture.model.rollouts.get_mut("pro").unwrap();
        rollout.status = "rolled_back".to_string();

        let error = prepare_prompt_distillation_evaluation(
            &fixture.model,
            &fixture.auto_parent,
            &fixture.snapshot,
            &fresh_campaign_cases(),
        )
        .err()
        .expect("rolled back Pro cannot remain a teacher");

        assert!(error.contains("canonical frozen stable champion"));
    }

    #[test]
    fn synthetic_teacher_without_replayable_dual_gate_is_rejected_at_dispatch_boundary() {
        let fixture = certified_fixture(false);

        let error = replay_canonical_pro_teacher_snapshot(&fixture.model, &fixture.snapshot)
            .expect_err("dispatch must replay the ordinary and Auto-transfer gates");

        assert!(!error.trim().is_empty());
    }

    #[test]
    fn insufficient_fresh_split_cases_do_not_start_a_campaign() {
        let fixture = certified_fixture(false);
        let mut cases = fresh_campaign_cases();
        cases.retain(|case| {
            case.split == PromptEvaluationSplit::Holdout
                || case.id
                    != format!(
                        "fresh-train-{}",
                        crate::runtime_constants::PROMPT_EVOLUTION_MIN_TRAIN_RUNS - 1
                    )
        });

        let error = prepare_prompt_distillation_evaluation(
            &fixture.model,
            &fixture.auto_parent,
            &fixture.snapshot,
            &cases,
        )
        .err()
        .expect("an impossible campaign should not consume background evaluations");

        assert!(error.contains("insufficient fresh train/holdout cases"));
    }

    #[test]
    fn one_case_cannot_cross_train_and_holdout() {
        let fixture = certified_fixture(false);
        let mut cases = fresh_campaign_cases();
        let duplicate = cases[0].clone();
        cases.push(PromptOfflineCase {
            split: PromptEvaluationSplit::Holdout,
            ..duplicate
        });

        assert!(prepare_prompt_distillation_evaluation(
            &fixture.model,
            &fixture.auto_parent,
            &fixture.snapshot,
            &cases,
        )
        .is_err());
    }

    #[test]
    fn distillation_replay_projects_only_the_child_genome_method() {
        let fixture = certified_fixture(false);
        let evaluation = prepare_prompt_distillation_evaluation(
            &fixture.model,
            &fixture.auto_parent,
            &fixture.snapshot,
            &fresh_campaign_cases(),
        )
        .unwrap();
        let parent_event = Event {
            id: EventId("parent-event".to_string()),
            task_id: phase16_task_id(),
            sequence: 1,
            timestamp_ms: 1,
            kind: EventKind::TaskStatusChanged,
            summary: "Conductor prompt mutation generated".to_string(),
            metadata: [
                ("project_id".to_string(), "project-a".to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_genome".to_string(),
                    serde_json::to_string(&fixture.auto_parent).unwrap(),
                ),
                (
                    "mutation_strategy".to_string(),
                    "gepa_reflection".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let observation = orchestrator::PromptEvolutionObservation {
            profile_id: evaluation.child.id.clone(),
            evaluation_id: "evaluation".to_string(),
            case_id: "case".to_string(),
            opponent_profile_id: Some(fixture.auto_parent.id.clone()),
            task_class: "general".to_string(),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
            format_valid: true,
            succeeded: true,
            quality_score: 1.0,
            latency_ms: 1,
            total_tokens: 1,
            estimated_cost_microusd: 0,
            safety_violations: 0,
            relative_reward: Some(0.1),
            step_credits: Vec::new(),
            reflection_packet: None,
            provenance: PromptEvaluationProvenance::default()
                .with_pro_to_auto_distillation(evaluation.provenance.clone()),
        };
        let distillation_event = Event {
            id: EventId("distillation-event".to_string()),
            task_id: phase16_task_id(),
            sequence: 2,
            timestamp_ms: 2,
            kind: EventKind::TaskStatusChanged,
            summary: DISTILLATION_EVALUATION_EVENT.to_string(),
            metadata: [
                ("project_id".to_string(), "project-a".to_string()),
                ("prompt_effort".to_string(), "auto".to_string()),
                (
                    "prompt_genomes".to_string(),
                    serde_json::to_string(&vec![
                        fixture.auto_parent.clone(),
                        evaluation.child.clone(),
                    ])
                    .unwrap(),
                ),
                (
                    "prompt_observations".to_string(),
                    serde_json::to_string(&vec![observation]).unwrap(),
                ),
                (
                    "mutation_strategy".to_string(),
                    "pro_to_auto_distillation".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };

        let model = build_prompt_evolution_read_model(&[parent_event, distillation_event], 2, 2);
        let parent = model
            .genomes
            .iter()
            .find(|record| record.genome.id == fixture.auto_parent.id)
            .unwrap();
        let child = model
            .genomes
            .iter()
            .find(|record| record.genome.id == evaluation.child.id)
            .unwrap();

        assert_eq!(
            parent.evolution_method,
            Some(PromptEvolutionMethod::GepaReflectivePaired)
        );
        assert_eq!(
            child.evolution_method,
            Some(PromptEvolutionMethod::ProToAutoDistillation)
        );
    }
}
