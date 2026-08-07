use crate::view_models::{
    PromptEvaluationAttemptState, PromptEvolutionReadModel, PromptGenomeRecord, PromptRolloutState,
};
use orchestrator::{
    sha256_hex, AgentPolicy, PromptEvaluationAttemptStatus, PromptEvolutionObservation,
    PromptLearningCohortV1,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const PROMPT_EVALUATION_ATTEMPT_RETENTION: usize = 1_024;
pub(crate) const PROMPT_EVIDENCE_SCOPE_SEPARATOR: &str = "::";
const PROMPT_GENOME_HOT_RETENTION_PER_SCOPE: usize = 64;
const PROMPT_OBSERVATION_HOT_RETENTION_PER_SCOPE: usize = 2_048;
const PROMPT_FAILURE_CURRICULUM_HOT_RETENTION_PER_SCOPE: usize = 128;

type PromptGenomeKey = (String, String, String);
type PromptScopeEffort = (String, String);
const PROMPT_GENOME_IDENTITY_CONFLICT: &str = "conflict";

pub(crate) fn scoped_prompt_evaluation_id(scope: &str, evaluation_id: &str) -> String {
    let scope = scope.trim();
    let scope = if scope.is_empty() { "global" } else { scope };
    format!("{scope}{PROMPT_EVIDENCE_SCOPE_SEPARATOR}{evaluation_id}")
}

pub(crate) fn prompt_rollout_key(scope: &str, effort: &str) -> String {
    scoped_prompt_evaluation_id(scope, effort)
}

pub(crate) fn prompt_observation_matches_scope(
    observation: &PromptEvolutionObservation,
    scope: &str,
) -> bool {
    observation
        .evaluation_id
        .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        .is_some_and(|(observed_scope, _)| observed_scope == scope)
}

pub(crate) fn prompt_genome_key(record: &PromptGenomeRecord) -> PromptGenomeKey {
    (
        record.scope.clone(),
        record.effort.clone(),
        record.genome.id.clone(),
    )
}

pub(crate) fn prompt_genome_records_match(
    left: &PromptGenomeRecord,
    right: &PromptGenomeRecord,
) -> bool {
    left.scope == right.scope
        && left.effort == right.effort
        && left.genome == right.genome
        && left.evolution_method == right.evolution_method
}

fn prompt_genome_identity_key(record: &PromptGenomeRecord) -> String {
    serde_json::to_string(&prompt_genome_key(record))
        .expect("prompt genome identity strings must serialize")
}

fn prompt_genome_record_fingerprint(record: &PromptGenomeRecord) -> String {
    sha256_hex(&serde_json::to_vec(record).expect("validated prompt genome records must serialize"))
}

fn prompt_genome_key_from_identity(encoded: &str) -> Option<PromptGenomeKey> {
    serde_json::from_str(encoded).ok()
}

pub(crate) fn prompt_model_genome_conflict_keys(
    model: &PromptEvolutionReadModel,
) -> BTreeSet<PromptGenomeKey> {
    let mut conflicts = prompt_genome_conflict_keys(&model.genomes);
    conflicts.extend(
        model
            .genome_identity_fingerprints
            .iter()
            .filter(|(_, fingerprint)| fingerprint.as_str() == PROMPT_GENOME_IDENTITY_CONFLICT)
            .filter_map(|(identity, _)| prompt_genome_key_from_identity(identity)),
    );
    conflicts
}

pub(crate) fn prompt_genome_identities_are_valid(model: &PromptEvolutionReadModel) -> bool {
    if model
        .genome_identity_fingerprints
        .iter()
        .any(|(identity, fingerprint)| {
            prompt_genome_key_from_identity(identity).is_none()
                || (fingerprint != PROMPT_GENOME_IDENTITY_CONFLICT
                    && (fingerprint.len() != 64
                        || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())))
        })
    {
        return false;
    }
    model.genomes.iter().all(|record| {
        model
            .genome_identity_fingerprints
            .get(&prompt_genome_identity_key(record))
            .is_some_and(|fingerprint| {
                fingerprint != PROMPT_GENOME_IDENTITY_CONFLICT
                    && fingerprint == &prompt_genome_record_fingerprint(record)
            })
    })
}

pub(crate) fn upsert_prompt_genome(
    model: &mut PromptEvolutionReadModel,
    record: PromptGenomeRecord,
) {
    let record_key = prompt_genome_key(&record);
    let identity = prompt_genome_identity_key(&record);
    let fingerprint = prompt_genome_record_fingerprint(&record);
    match model.genome_identity_fingerprints.get(&identity) {
        Some(existing) if existing == PROMPT_GENOME_IDENTITY_CONFLICT => {
            model
                .genomes
                .retain(|existing| prompt_genome_key(existing) != record_key);
        }
        Some(existing) if existing == &fingerprint => {}
        Some(_) => {
            model
                .genome_identity_fingerprints
                .insert(identity, PROMPT_GENOME_IDENTITY_CONFLICT.to_string());
            model
                .genomes
                .retain(|existing| prompt_genome_key(existing) != record_key);
        }
        None => {
            let matching = model
                .genomes
                .iter()
                .filter(|existing| prompt_genome_key(existing) == record_key)
                .collect::<Vec<_>>();
            if matching
                .iter()
                .any(|existing| !prompt_genome_records_match(existing, &record))
            {
                model
                    .genome_identity_fingerprints
                    .insert(identity, PROMPT_GENOME_IDENTITY_CONFLICT.to_string());
                model
                    .genomes
                    .retain(|existing| prompt_genome_key(existing) != record_key);
            } else if matching.is_empty() {
                model
                    .genome_identity_fingerprints
                    .insert(identity, fingerprint);
                model.genomes.push(record);
            } else {
                model
                    .genome_identity_fingerprints
                    .insert(identity, fingerprint);
            }
        }
    }
}

pub(crate) fn prompt_genome_conflict_keys(
    records: &[PromptGenomeRecord],
) -> BTreeSet<PromptGenomeKey> {
    let mut first = BTreeMap::<PromptGenomeKey, &PromptGenomeRecord>::new();
    let mut conflicts = BTreeSet::new();
    for record in records {
        let key = prompt_genome_key(record);
        if first
            .get(&key)
            .is_some_and(|existing| !prompt_genome_records_match(existing, record))
        {
            conflicts.insert(key);
        } else {
            first.entry(key).or_insert(record);
        }
    }
    conflicts
}

fn prompt_rollout_profile_ids(rollout: &PromptRolloutState) -> BTreeSet<String> {
    let mut ids = BTreeSet::from([rollout.stable_profile_id.clone()]);
    ids.extend(rollout.canary_profile_id.iter().cloned());
    ids.extend(rollout.quarantined_profile_ids.iter().cloned());
    if let Some(lease) = rollout.distillation_lease.as_ref() {
        ids.insert(lease.stable_profile_id.clone());
        ids.insert(lease.candidate_profile_id.clone());
    }
    if let Some(snapshot) = rollout.frozen_profile.as_ref() {
        ids.insert(snapshot.genome.id.clone());
        ids.insert(snapshot.stable_profile_id.clone());
        if let Some(evidence) = snapshot.auto_teacher_evidence.as_ref() {
            ids.insert(evidence.source_profile_id.clone());
        }
        if let Some(evidence) = snapshot.pro_teacher_evidence.as_ref() {
            ids.insert(evidence.auto_parent_profile_id.clone());
            ids.insert(evidence.auto_child_profile_id.clone());
            ids.insert(evidence.teacher.teacher_profile_id.clone());
        }
    }
    ids
}

fn prompt_rollout_scope_effort(key: &str) -> PromptScopeEffort {
    key.split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        .map(|(scope, effort)| (scope.to_string(), effort.to_string()))
        .unwrap_or_else(|| ("global".to_string(), key.to_string()))
}

fn prompt_rollout_profile_references(
    model: &PromptEvolutionReadModel,
) -> BTreeMap<PromptScopeEffort, BTreeSet<String>> {
    let mut references = BTreeMap::<PromptScopeEffort, BTreeSet<String>>::new();
    for (key, rollout) in &model.rollouts {
        references
            .entry(prompt_rollout_scope_effort(key))
            .or_default()
            .extend(prompt_rollout_profile_ids(rollout));
    }
    references
}

pub(crate) fn prompt_rollout_references_conflicted_genome(
    rollout: &PromptRolloutState,
    scope: &str,
    effort: &str,
    conflicts: &BTreeSet<PromptGenomeKey>,
) -> bool {
    prompt_rollout_profile_ids(rollout)
        .into_iter()
        .any(|profile_id| conflicts.contains(&(scope.to_string(), effort.to_string(), profile_id)))
}

pub(crate) fn prompt_observation_matches_persisted_attempt(
    observation: &PromptEvolutionObservation,
    attempts: &BTreeMap<String, PromptEvaluationAttemptState>,
    cohorts: &BTreeMap<String, PromptLearningCohortV1>,
) -> bool {
    let Some(identity) = observation.provenance.matched_evaluation.as_ref() else {
        return observation.is_trusted_live_assignment();
    };
    cohorts.get(&identity.cohort_sha256).is_some_and(|cohort| {
        crate::prompt_attempt_runtime::prompt_matched_identity_belongs_to_cohort(identity, cohort)
            && attempts
                .get(&identity.evaluation_id)
                .is_some_and(|attempt| {
                    attempt.started.identity == *identity
                        && crate::prompt_attempt_runtime::prompt_observation_matches_attempt(
                            observation,
                            &attempt.started,
                        )
                        && attempt.terminal.as_ref().is_some_and(|terminal| {
                            terminal.identity == *identity
                                && terminal.status == PromptEvaluationAttemptStatus::CompletedPair
                        })
                })
    })
}

fn prompt_evaluation_id_matches_scope(evaluation_id: &str, scope: &str) -> bool {
    evaluation_id
        .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        .is_some_and(|(observed_scope, _)| observed_scope == scope)
}

pub(crate) fn prompt_evolution_read_model_for_scope(
    model: &PromptEvolutionReadModel,
    scope: &str,
) -> PromptEvolutionReadModel {
    let conflicts = prompt_model_genome_conflict_keys(model);
    let genomes = model
        .genomes
        .iter()
        .filter(|record| record.scope == scope && !conflicts.contains(&prompt_genome_key(record)))
        .cloned()
        .collect();
    let datasets = model
        .datasets
        .iter()
        .filter(|(_, dataset)| dataset.project_id == scope)
        .map(|(key, dataset)| (key.clone(), dataset.clone()))
        .collect();
    let attempts = model
        .attempts
        .iter()
        .filter(|(_, attempt)| {
            prompt_evaluation_id_matches_scope(&attempt.started.identity.evaluation_id, scope)
        })
        .map(|(key, attempt)| (key.clone(), attempt.clone()))
        .collect::<BTreeMap<_, _>>();
    let cohort_ids = attempts
        .values()
        .map(|attempt| attempt.started.identity.cohort_sha256.clone())
        .collect::<BTreeSet<_>>();
    let cohorts = model
        .cohorts
        .iter()
        .filter(|(cohort_sha256, _)| cohort_ids.contains(*cohort_sha256))
        .map(|(key, cohort)| (key.clone(), cohort.clone()))
        .collect::<BTreeMap<_, _>>();
    let cohort_sequences = model
        .cohort_sequences
        .iter()
        .filter(|(cohort_sha256, _)| cohort_ids.contains(*cohort_sha256))
        .map(|(key, sequence)| (key.clone(), *sequence))
        .collect();
    let observations = model
        .observations
        .iter()
        .filter(|(_, observation)| {
            prompt_observation_matches_scope(observation, scope)
                && prompt_observation_matches_persisted_attempt(observation, &attempts, &cohorts)
        })
        .cloned()
        .collect();
    let failure_curricula = model
        .failure_curricula
        .iter()
        .filter(|record| record.scope == scope)
        .cloned()
        .collect();
    let rollouts = ["fast", "auto", "pro"]
        .into_iter()
        .filter_map(|effort| {
            let rollout = model
                .rollouts
                .get(&prompt_rollout_key(scope, effort))
                .or_else(|| {
                    (scope == "global")
                        .then(|| model.rollouts.get(effort))
                        .flatten()
                })?;
            (!prompt_rollout_references_conflicted_genome(rollout, scope, effort, &conflicts))
                .then(|| (effort.to_string(), rollout.clone()))
        })
        .collect();
    let genome_identity_fingerprints = model
        .genome_identity_fingerprints
        .iter()
        .filter(|(identity, _)| {
            prompt_genome_key_from_identity(identity)
                .is_some_and(|(identity_scope, _, _)| identity_scope == scope)
        })
        .map(|(identity, fingerprint)| (identity.clone(), fingerprint.clone()))
        .collect();
    PromptEvolutionReadModel {
        schema: model.schema.clone(),
        projection_version: model.projection_version,
        revision: model.revision,
        event_count: model.event_count,
        genomes,
        genome_identity_fingerprints,
        observations,
        failure_curricula,
        attempts,
        cohorts,
        cohort_sequences,
        rollouts,
        datasets,
    }
}

fn prompt_policy_effort(policy: AgentPolicy) -> &'static str {
    match policy {
        AgentPolicy::Fast => "fast",
        AgentPolicy::Auto => "auto",
        AgentPolicy::Pro => "pro",
    }
}

fn prompt_active_cohort_references(model: &PromptEvolutionReadModel) -> BTreeSet<String> {
    let mut active_by_pair = BTreeMap::<(String, String, String, String), (u64, String)>::new();
    for attempt in model.attempts.values() {
        let digest = &attempt.started.identity.cohort_sha256;
        let Some(cohort) = model.cohorts.get(digest) else {
            continue;
        };
        let Some((scope, _)) = attempt
            .started
            .identity
            .evaluation_id
            .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        else {
            continue;
        };
        let sequence = model
            .cohort_sequences
            .get(digest)
            .copied()
            .unwrap_or_default();
        let mut profiles = attempt
            .started
            .treatments
            .iter()
            .map(|treatment| treatment.profile_id.clone())
            .collect::<Vec<_>>();
        profiles.sort();
        let key = (
            scope.to_string(),
            prompt_policy_effort(cohort.execution.policy).to_string(),
            profiles[0].clone(),
            profiles[1].clone(),
        );
        let candidate = (sequence, digest.clone());
        if active_by_pair
            .get(&key)
            .is_none_or(|current| candidate > *current)
        {
            active_by_pair.insert(key, candidate);
        }
    }
    let mut active = active_by_pair
        .into_values()
        .map(|(_, digest)| digest)
        .collect::<BTreeSet<_>>();
    let mut latest_scientific = BTreeSet::<(String, String, bool)>::new();
    for (effort, observation) in model.observations.iter().rev() {
        let Some(identity) = observation.provenance.matched_evaluation.as_ref() else {
            continue;
        };
        let Some((scope, _)) = identity
            .evaluation_id
            .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        else {
            continue;
        };
        let transfer = observation.is_source_attested_transfer_evidence();
        let key = (scope.to_string(), effort.clone(), transfer);
        if observation.is_scientific_evidence() && latest_scientific.insert(key) {
            active.insert(identity.cohort_sha256.clone());
        }
    }
    for rollout in model.rollouts.values() {
        if let Some(lease) = rollout.distillation_lease.as_ref() {
            active.insert(lease.cohort_sha256.clone());
        }
        if let Some(snapshot) = rollout.frozen_profile.as_ref() {
            if let Some(cohort_sha256) = snapshot
                .auto_teacher_evidence
                .as_ref()
                .and_then(|evidence| evidence.cohort_sha256.as_ref())
            {
                active.insert(cohort_sha256.clone());
            }
            if let Some(evidence) = snapshot.pro_teacher_evidence.as_ref() {
                active.insert(evidence.cohort_sha256.clone());
                active.insert(evidence.teacher.auto_transfer_cohort_sha256.clone());
            }
        }
    }
    active.extend(
        model
            .attempts
            .values()
            .filter(|attempt| attempt.terminal.is_none())
            .map(|attempt| attempt.started.identity.cohort_sha256.clone()),
    );
    active
}

fn prompt_observation_scope_effort(
    effort: &str,
    observation: &PromptEvolutionObservation,
) -> PromptScopeEffort {
    let scope = observation
        .evaluation_id
        .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        .map(|(scope, _)| scope)
        .unwrap_or("global");
    (scope.to_string(), effort.to_string())
}

fn compact_prompt_observations(
    model: &mut PromptEvolutionReadModel,
    active_cohorts: &BTreeSet<String>,
    limit_per_scope: usize,
) {
    let rollout_profiles = prompt_rollout_profile_references(model);
    let unfinished = model
        .attempts
        .values()
        .filter(|attempt| attempt.terminal.is_none())
        .map(|attempt| attempt.started.identity.evaluation_id.clone())
        .collect::<BTreeSet<_>>();
    let mut retained_unreferenced = BTreeMap::<PromptScopeEffort, usize>::new();
    let mut keep = vec![false; model.observations.len()];
    for (index, (effort, observation)) in model.observations.iter().enumerate().rev() {
        let scope_effort = prompt_observation_scope_effort(effort, observation);
        let referenced_profile = rollout_profiles.get(&scope_effort).is_some_and(|profiles| {
            profiles.contains(&observation.profile_id)
                || observation
                    .opponent_profile_id
                    .as_ref()
                    .is_some_and(|profile| profiles.contains(profile))
        });
        let referenced_evaluation = observation
            .provenance
            .matched_evaluation
            .as_ref()
            .is_some_and(|identity| {
                active_cohorts.contains(&identity.cohort_sha256)
                    || unfinished.contains(&identity.evaluation_id)
            });
        let retained = retained_unreferenced.entry(scope_effort).or_default();
        if referenced_profile || referenced_evaluation || *retained < limit_per_scope {
            keep[index] = true;
            if !referenced_profile && !referenced_evaluation {
                *retained += 1;
            }
        }
    }
    let mut index = 0usize;
    model.observations.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
}

fn compact_prompt_failure_curricula(model: &mut PromptEvolutionReadModel, limit_per_scope: usize) {
    let mut retained = BTreeMap::<PromptScopeEffort, usize>::new();
    let mut keep = vec![false; model.failure_curricula.len()];
    for (index, record) in model.failure_curricula.iter().enumerate().rev() {
        let count = retained
            .entry((record.scope.clone(), record.effort.clone()))
            .or_default();
        if *count < limit_per_scope {
            keep[index] = true;
            *count += 1;
        }
    }
    let mut index = 0usize;
    model.failure_curricula.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
}

fn compact_prompt_attempts_and_cohorts(
    model: &mut PromptEvolutionReadModel,
    active_cohorts: &BTreeSet<String>,
    attempt_limit: usize,
    cohort_limit: usize,
) {
    let referenced_evaluations = model
        .observations
        .iter()
        .filter_map(|(_, observation)| observation.provenance.matched_evaluation.as_ref())
        .map(|identity| identity.evaluation_id.clone())
        .collect::<BTreeSet<_>>();
    let excess_attempts = model.attempts.len().saturating_sub(attempt_limit);
    let mut removable_attempts = model
        .attempts
        .iter()
        .filter(|(_, attempt)| attempt.terminal.is_some())
        .filter(|(key, attempt)| {
            !active_cohorts.contains(&attempt.started.identity.cohort_sha256)
                && !referenced_evaluations.contains(*key)
        })
        .map(|(key, attempt)| {
            (
                model
                    .cohort_sequences
                    .get(&attempt.started.identity.cohort_sha256)
                    .copied()
                    .unwrap_or_default(),
                key.clone(),
            )
        })
        .collect::<Vec<_>>();
    removable_attempts.sort();
    for (_, key) in removable_attempts.into_iter().take(excess_attempts) {
        model.attempts.remove(&key);
    }

    let mut referenced_cohorts = active_cohorts.clone();
    referenced_cohorts.extend(
        model
            .attempts
            .values()
            .map(|attempt| attempt.started.identity.cohort_sha256.clone()),
    );
    referenced_cohorts.extend(model.observations.iter().filter_map(|(_, observation)| {
        observation
            .provenance
            .matched_evaluation
            .as_ref()
            .map(|identity| identity.cohort_sha256.clone())
    }));
    let excess_cohorts = model.cohorts.len().saturating_sub(cohort_limit);
    let mut removable_cohorts = model
        .cohorts
        .keys()
        .filter(|digest| !referenced_cohorts.contains(*digest))
        .map(|digest| {
            (
                model
                    .cohort_sequences
                    .get(digest)
                    .copied()
                    .unwrap_or_default(),
                digest.clone(),
            )
        })
        .collect::<Vec<_>>();
    removable_cohorts.sort();
    for (_, digest) in removable_cohorts.into_iter().take(excess_cohorts) {
        model.cohorts.remove(&digest);
        model.cohort_sequences.remove(&digest);
    }
    model
        .cohort_sequences
        .retain(|digest, _| model.cohorts.contains_key(digest));
}

fn compact_prompt_genomes(model: &mut PromptEvolutionReadModel, limit_per_scope: usize) {
    let conflicts = prompt_model_genome_conflict_keys(model);
    let mut referenced = prompt_rollout_profile_references(model);
    for (effort, observation) in &model.observations {
        let key = prompt_observation_scope_effort(effort, observation);
        let ids = referenced.entry(key).or_default();
        ids.insert(observation.profile_id.clone());
        ids.extend(observation.opponent_profile_id.iter().cloned());
    }
    for attempt in model.attempts.values() {
        let Some((scope, _)) = attempt
            .started
            .identity
            .evaluation_id
            .split_once(PROMPT_EVIDENCE_SCOPE_SEPARATOR)
        else {
            continue;
        };
        let effort = prompt_policy_effort(
            model
                .cohorts
                .get(&attempt.started.identity.cohort_sha256)
                .map(|cohort| cohort.execution.policy)
                .unwrap_or_default(),
        );
        referenced
            .entry((scope.to_string(), effort.to_string()))
            .or_default()
            .extend(
                attempt
                    .started
                    .treatments
                    .iter()
                    .map(|treatment| treatment.profile_id.clone()),
            );
    }
    let mut retained_unreferenced = BTreeMap::<PromptScopeEffort, usize>::new();
    let mut keep = vec![false; model.genomes.len()];
    for (index, record) in model.genomes.iter().enumerate().rev() {
        let key = (record.scope.clone(), record.effort.clone());
        let conflict = conflicts.contains(&prompt_genome_key(record));
        let referenced = referenced
            .get(&key)
            .is_some_and(|ids| ids.contains(&record.genome.id));
        let retained = retained_unreferenced.entry(key).or_default();
        if conflict || referenced || *retained < limit_per_scope {
            keep[index] = true;
            if !conflict && !referenced {
                *retained += 1;
            }
        }
    }
    let mut index = 0usize;
    model.genomes.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
}

pub(crate) fn compact_prompt_evolution_hot_state(model: &mut PromptEvolutionReadModel) {
    compact_prompt_evolution_hot_state_to_limits(
        model,
        PROMPT_EVALUATION_ATTEMPT_RETENTION,
        crate::prompt_attempt_runtime::PROMPT_LEARNING_COHORT_RETENTION,
        PROMPT_GENOME_HOT_RETENTION_PER_SCOPE,
        PROMPT_OBSERVATION_HOT_RETENTION_PER_SCOPE,
    );
}

pub(crate) fn compact_prompt_evolution_hot_state_to_limits(
    model: &mut PromptEvolutionReadModel,
    attempt_limit: usize,
    cohort_limit: usize,
    genome_limit_per_scope: usize,
    observation_limit_per_scope: usize,
) {
    let active_cohorts = prompt_active_cohort_references(model);
    compact_prompt_observations(model, &active_cohorts, observation_limit_per_scope);
    compact_prompt_failure_curricula(model, PROMPT_FAILURE_CURRICULUM_HOT_RETENTION_PER_SCOPE);
    compact_prompt_attempts_and_cohorts(model, &active_cohorts, attempt_limit, cohort_limit);
    compact_prompt_genomes(model, genome_limit_per_scope);
}

#[cfg(test)]
pub(crate) fn compact_prompt_learning_hot_state_to_limits(
    model: &mut PromptEvolutionReadModel,
    attempt_limit: usize,
    cohort_limit: usize,
) {
    let active_cohorts = prompt_active_cohort_references(model);
    compact_prompt_attempts_and_cohorts(model, &active_cohorts, attempt_limit, cohort_limit);
}
