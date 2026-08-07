use super::*;
use crate::prompt_evolution_hot_state::{
    compact_prompt_evolution_hot_state, prompt_genome_conflict_keys,
    prompt_genome_identities_are_valid, prompt_genome_key, prompt_model_genome_conflict_keys,
    prompt_rollout_references_conflicted_genome, upsert_prompt_genome,
};
#[cfg(test)]
use crate::prompt_evolution_hot_state::{
    compact_prompt_evolution_hot_state_to_limits, compact_prompt_learning_hot_state_to_limits,
};
pub(crate) use crate::prompt_evolution_hot_state::{
    prompt_evolution_read_model_for_scope, prompt_observation_matches_scope, prompt_rollout_key,
    scoped_prompt_evaluation_id, PROMPT_EVALUATION_ATTEMPT_RETENTION,
    PROMPT_EVIDENCE_SCOPE_SEPARATOR,
};
use crate::prompt_evolution_projection_contract::{
    prompt_auto_teacher_source_key, prompt_transfer_matches_canonical_teacher,
    CanonicalPromptAutoTeacher,
};
use agent_storage::StoredReadModel;
#[cfg(test)]
use orchestrator::{IndependentQualitySource, LearningEvidenceV1, LearningUsageCompleteness};

fn is_agent_run_terminal(event: &Event) -> bool {
    AgentRunEvent::from_event(event).is_some_and(|event| event.status().is_terminal())
}

pub(crate) fn initial_prompt_population(effort: &str) -> Vec<ConductorPromptGenome> {
    let seed = ConductorPromptGenome::seed_for_effort(effort);
    let mutations = seed.mutations();
    let mut population = vec![seed.clone()];
    for variant in [
        mutations
            .iter()
            .find(|variant| variant.graph_depth != seed.graph_depth),
        mutations
            .iter()
            .find(|variant| variant.verification != seed.verification),
        mutations
            .iter()
            .find(|variant| variant.context_policy != seed.context_policy),
        mutations
            .iter()
            .find(|variant| variant.max_parallel_branches != seed.max_parallel_branches),
        mutations
            .iter()
            .find(|variant| variant.tool_policy != seed.tool_policy),
        mutations
            .iter()
            .find(|variant| variant.retry_policy != seed.retry_policy),
    ]
    .into_iter()
    .flatten()
    {
        population.push(variant.clone());
    }
    population.truncate(PROMPT_EVOLUTION_POPULATION_LIMIT);
    population
}

pub(crate) fn prompt_genomes_from_events(
    events: &[Event],
    effort: &str,
) -> Vec<ConductorPromptGenome> {
    let mut population = initial_prompt_population(effort);
    let records = events
        .iter()
        .flat_map(prompt_genome_records_from_event)
        .collect::<Vec<_>>();
    let conflicts = prompt_genome_conflict_keys(&records);
    for record in records {
        if record.effort == effort && !conflicts.contains(&prompt_genome_key(&record)) {
            population.push(record.genome);
        }
    }
    let mut ids = BTreeSet::new();
    population.retain(|genome| ids.insert(genome.id.clone()));
    population
}

fn prompt_agent_run_event_index(events: &[Event]) -> BTreeMap<&str, Vec<&Event>> {
    let mut index = BTreeMap::<&str, Vec<&Event>>::new();
    for event in events {
        if let Some(run_id) = event.metadata.get("agent_run_id") {
            index.entry(run_id.as_str()).or_default().push(event);
        }
    }
    index
}

fn prompt_evolution_observation_records_from_events(
    events: &[Event],
) -> Vec<(u64, String, PromptEvolutionObservation)> {
    let mut runs =
        crate::prompt_canary_outcome_projection::prompt_live_canary_outcome_records_from_events(
            events,
        );
    let events_by_agent_run = prompt_agent_run_event_index(events);
    let mut auto_teacher_cache = BTreeMap::<(String, String), Option<PromptAutoTeacherCase>>::new();
    for event in events {
        let source_key = prompt_auto_teacher_source_key(event);
        if let Some((project_id, source_run_id)) = source_key.as_ref() {
            auto_teacher_cache
                .entry((project_id.clone(), source_run_id.clone()))
                .or_insert_with(|| {
                    let source_events = events_by_agent_run
                        .get(source_run_id.as_str())
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    crate::prompt_evidence_runtime::reconstruct_prompt_auto_teacher_from_indexed_events(
                        source_events,
                        project_id,
                        source_run_id,
                    )
                });
        }
        let canonical_teacher = source_key
            .as_ref()
            .and_then(|key| auto_teacher_cache.get(key))
            .and_then(Option::as_ref)
            .map(CanonicalPromptAutoTeacher::from);
        runs.extend(
            prompt_observation_records_from_event_with_canonical_teacher(event, canonical_teacher)
                .into_iter()
                .map(|(effort, observation)| (event.sequence, effort, observation)),
        );
    }
    runs.sort_by_key(|(sequence, _, _)| *sequence);
    runs
}

pub(crate) fn prompt_evolution_observations_from_events(
    events: &[Event],
) -> Vec<(String, PromptEvolutionObservation)> {
    prompt_evolution_observation_records_from_events(events)
        .into_iter()
        .map(|(_, effort, observation)| (effort, observation))
        .collect()
}

pub(crate) fn prompt_genome_records_from_event(event: &Event) -> Vec<PromptGenomeRecord> {
    let Some(effort) = event.metadata.get("prompt_effort").cloned() else {
        return Vec::new();
    };
    let genomes = event
        .metadata
        .get("prompt_genomes")
        .and_then(|encoded| serde_json::from_str::<Vec<ConductorPromptGenome>>(encoded).ok())
        .or_else(|| {
            event
                .metadata
                .get("prompt_genome")
                .and_then(|encoded| serde_json::from_str::<ConductorPromptGenome>(encoded).ok())
                .map(|genome| vec![genome])
        })
        .unwrap_or_default();
    let mutation_strategy = event.metadata.get("mutation_strategy").map(String::as_str);
    let distillation_provenance = (mutation_strategy == Some("pro_to_auto_distillation"))
        .then_some(event)
        .and_then(crate::prompt_distillation_runtime::prompt_distillation_provenance_from_event);
    let scope = event
        .metadata
        .get("project_id")
        .or_else(|| event.metadata.get("prompt_rollout_scope"))
        .map(|scope| scope.trim())
        .filter(|scope| !scope.is_empty())
        .unwrap_or("global")
        .to_string();
    genomes
        .into_iter()
        .filter(|genome| genome.validate().is_ok())
        .filter_map(|genome| {
            let evolution_method = match mutation_strategy {
                Some("gepa_reflection") => Some(PromptEvolutionMethod::GepaReflectivePaired),
                Some("pro_to_auto_distillation") => {
                    let provenance = distillation_provenance.as_ref()?;
                    if genome.id != provenance.auto_child_profile_id
                        || prompt_genome_sha256(&genome).ok().as_ref()
                            != Some(&provenance.auto_child_profile_sha256)
                    {
                        return None;
                    }
                    Some(PromptEvolutionMethod::ProToAutoDistillation)
                }
                _ => None,
            };
            Some(PromptGenomeRecord {
                scope: scope.clone(),
                effort: effort.clone(),
                genome,
                evolution_method,
            })
        })
        .collect()
}

fn prompt_observation_records_from_event_with_canonical_teacher(
    event: &Event,
    canonical_teacher: Option<CanonicalPromptAutoTeacher<'_>>,
) -> Vec<(String, PromptEvolutionObservation)> {
    let is_transfer = match event.summary.as_str() {
        "Conductor pairwise evaluation" => false,
        "Conductor Auto transfer evaluation" => true,
        _ => return Vec::new(),
    };
    let Some(effort) = event.metadata.get("prompt_effort").cloned() else {
        return Vec::new();
    };
    let observations = event
        .metadata
        .get("prompt_observations")
        .and_then(|encoded| serde_json::from_str::<Vec<PromptEvolutionObservation>>(encoded).ok())
        .or_else(|| {
            event
                .metadata
                .get("prompt_observation")
                .and_then(|encoded| {
                    serde_json::from_str::<PromptEvolutionObservation>(encoded).ok()
                })
                .map(|observation| vec![observation])
        })
        .unwrap_or_default();
    if !is_transfer {
        let Some([candidate, opponent]) = observations.as_slice().first_chunk::<2>() else {
            return Vec::new();
        };
        let Some(project_id) = event.metadata.get("project_id") else {
            return Vec::new();
        };
        let matched_identity_present = candidate.provenance.matched_evaluation.is_some()
            || opponent.provenance.matched_evaluation.is_some();
        if observations.len() != 2
            || !candidate.is_scientific_evidence()
            || !opponent.is_scientific_evidence()
            || candidate.evaluation_id != opponent.evaluation_id
            || candidate.case_id != opponent.case_id
            || candidate.split != opponent.split
            || candidate.mode != opponent.mode
            || candidate.opponent_profile_id.as_deref() != Some(opponent.profile_id.as_str())
            || opponent.opponent_profile_id.as_deref() != Some(candidate.profile_id.as_str())
            || !prompt_observation_matches_scope(candidate, project_id)
            || !prompt_observation_matches_scope(opponent, project_id)
            || (matched_identity_present
                && (!candidate.is_strict_matched_evidence()
                    || !opponent.is_strict_matched_evidence()
                    || candidate.provenance.matched_evaluation
                        != opponent.provenance.matched_evaluation))
        {
            return Vec::new();
        }
    } else {
        let Some([candidate, teacher]) = observations.as_slice().first_chunk::<2>() else {
            return Vec::new();
        };
        if observations.len() != 2
            || effort != "pro"
            || !candidate.is_strict_source_attested_transfer_evidence()
            || !teacher.is_strict_source_attested_transfer_evidence()
            || candidate.evaluation_id != teacher.evaluation_id
            || candidate.case_id != teacher.case_id
            || candidate.split != teacher.split
            || candidate.mode != teacher.mode
            || candidate.opponent_profile_id.as_deref() != Some(teacher.profile_id.as_str())
            || teacher.opponent_profile_id.as_deref() != Some(candidate.profile_id.as_str())
            || candidate.provenance.transfer != teacher.provenance.transfer
            || candidate.provenance.matched_evaluation != teacher.provenance.matched_evaluation
        {
            return Vec::new();
        }
        let Some(transfer) = candidate.provenance.transfer.as_ref() else {
            return Vec::new();
        };
        let Some(canonical_teacher) = canonical_teacher else {
            return Vec::new();
        };
        let Some(project_id) = event.metadata.get("project_id") else {
            return Vec::new();
        };
        if !prompt_observation_matches_scope(candidate, project_id)
            || !prompt_observation_matches_scope(teacher, project_id)
            || event.metadata.get("auto_teacher_profile") != Some(&transfer.source_profile_id)
            || event.metadata.get("auto_teacher_run_id") != Some(&transfer.source_run_id)
            || teacher.profile_id != canonical_teacher.profile_id
            || !prompt_transfer_matches_canonical_teacher(transfer, canonical_teacher)
        {
            return Vec::new();
        }
    }
    observations
        .into_iter()
        .map(|observation| (effort.clone(), observation))
        .collect()
}

#[cfg(test)]
pub(crate) fn prompt_observation_records_from_event(
    event: &Event,
) -> Vec<(String, PromptEvolutionObservation)> {
    prompt_observation_records_from_event_with_canonical_teacher(event, None)
}

fn prompt_rollout_record_from_event(event: &Event) -> Option<(String, String, PromptRolloutState)> {
    if event.summary != "Conductor prompt rollout updated" {
        return None;
    }
    let effort = event.metadata.get("prompt_effort")?.clone();
    let rollout_key = event
        .metadata
        .get("prompt_rollout_scope")
        .filter(|scope| !scope.trim().is_empty())
        .map(|scope| prompt_rollout_key(scope, &effort))
        .unwrap_or_else(|| effort.clone());
    let stable_profile_id = event.metadata.get("stable_profile")?.clone();
    if effort.trim().is_empty() || stable_profile_id.trim().is_empty() {
        return None;
    }
    let optional_text = |key: &str| {
        event
            .metadata
            .get(key)
            .filter(|value| !value.trim().is_empty())
            .cloned()
    };
    let parse_usize = |key: &str| {
        event
            .metadata
            .get(key)
            .and_then(|value| value.parse::<usize>().ok())
    };
    let status = event
        .metadata
        .get("rollout_status")
        .filter(|status| {
            matches!(
                status.as_str(),
                "stable" | "evaluating" | "canary" | "rolled_back" | "promoted"
            )
        })?
        .clone();
    let frozen_profile = match optional_text("frozen_prompt_profile") {
        Some(value) => Some(
            serde_json::from_str::<FrozenPromptProfileSnapshot>(&value)
                .ok()
                .filter(|snapshot| {
                    snapshot.validate().is_ok()
                        && snapshot.effort == effort
                        && snapshot.genome.id == stable_profile_id
                })?,
        ),
        None => None,
    };
    let canary_profile_id = optional_text("canary_profile");
    let canary_percent = event.metadata.get("canary_percent")?.parse::<u8>().ok()?;
    let has_canary = status == "canary"
        && canary_profile_id
            .as_ref()
            .is_some_and(|canary| canary != &stable_profile_id)
        && (1..=50).contains(&canary_percent);
    let has_no_canary = status != "canary" && canary_profile_id.is_none() && canary_percent == 0;
    if !has_canary && !has_no_canary {
        return None;
    }
    let stable_live_checkpoint = event
        .metadata
        .get("stable_live_checkpoint")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let quarantined_profile_ids = match optional_text("quarantined_profiles") {
        Some(encoded) => serde_json::from_str::<Vec<String>>(&encoded)
            .ok()
            .filter(|profile_ids| prompt_quarantine_is_valid(profile_ids))?,
        None => Vec::new(),
    };
    let distillation_lease = match optional_text("distillation_canary_lease") {
        Some(encoded) => Some(
            serde_json::from_str::<PromptDistillationCanaryLeaseV1>(&encoded)
                .ok()
                .filter(|lease| {
                    lease.schema == PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1
                        && canary_profile_id.as_ref() == Some(&lease.candidate_profile_id)
                        && stable_profile_id == lease.stable_profile_id
                        && [
                            lease.candidate_profile_sha256.as_str(),
                            lease.stable_profile_sha256.as_str(),
                            lease.cohort_sha256.as_str(),
                            lease.paired_evidence_sha256.as_str(),
                        ]
                        .iter()
                        .all(|digest| {
                            digest.len() == 64
                                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                })?,
        ),
        None => None,
    };
    Some((
        rollout_key,
        effort,
        PromptRolloutState {
            stable_profile_id,
            canary_profile_id,
            canary_percent,
            evidence_checkpoint: parse_usize("evidence_checkpoint")?,
            live_checkpoint: parse_usize("live_checkpoint")?,
            stable_live_checkpoint,
            quarantined_profile_ids,
            distillation_lease,
            rollback_count: parse_usize("rollback_count")?,
            status,
            last_reason: optional_text("rollout_reason"),
            promotion_confidence: event
                .metadata
                .get("promotion_confidence")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite()),
            frozen_profile,
        },
    ))
}

fn prompt_rollout_transition_is_valid(
    effort: &str,
    previous: Option<&PromptRolloutState>,
    next: &PromptRolloutState,
) -> bool {
    let seed_id = ConductorPromptGenome::seed_for_effort(effort).id;
    let frozen_is_valid = |snapshot: &FrozenPromptProfileSnapshot| {
        snapshot.validate().is_ok()
            && snapshot.effort == effort
            && snapshot.genome.id == next.stable_profile_id
    };
    if !prompt_quarantine_is_valid(&next.quarantined_profile_ids)
        || next
            .quarantined_profile_ids
            .iter()
            .any(|profile_id| profile_id == &next.stable_profile_id)
    {
        return false;
    }
    let Some(previous) = previous else {
        return if next.status == "promoted" {
            next.stable_profile_id != seed_id
                && next.rollback_count == 0
                && next.quarantined_profile_ids.is_empty()
                && next.frozen_profile.as_ref().is_some_and(|snapshot| {
                    frozen_is_valid(snapshot) && snapshot.stable_profile_id == seed_id
                })
        } else {
            next.status != "rolled_back"
                && next.stable_profile_id == seed_id
                && next.frozen_profile.is_none()
                && next.rollback_count == 0
                && (next.status != "canary" || next.canary_percent == 10)
                && next.quarantined_profile_ids.is_empty()
        };
    };
    let expected_rollback_count = if next.status == "rolled_back" {
        previous.rollback_count.checked_add(1)
    } else {
        Some(previous.rollback_count)
    };
    if expected_rollback_count != Some(next.rollback_count) {
        return false;
    }
    let distillation_transition =
        previous.distillation_lease.is_some() || next.distillation_lease.is_some();
    if next.live_checkpoint < previous.live_checkpoint
        || (distillation_transition
            && next.stable_live_checkpoint < previous.stable_live_checkpoint)
    {
        return false;
    }
    if previous.canary_profile_id.is_some()
        && !matches!(next.status.as_str(), "canary" | "rolled_back" | "promoted")
    {
        return false;
    }
    if previous.canary_profile_id.is_none() && next.status == "rolled_back" {
        return false;
    }
    let expected_quarantine =
        if next.status == "rolled_back" && previous.distillation_lease.is_some() {
            let Some(previous_candidate) = previous.canary_profile_id.as_ref() else {
                return false;
            };
            let mut profile_ids = previous.quarantined_profile_ids.clone();
            if profile_ids
                .iter()
                .any(|profile_id| profile_id == previous_candidate)
                || profile_ids.len() >= PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES
            {
                return false;
            }
            profile_ids.push(previous_candidate.clone());
            profile_ids
        } else if next.status == "promoted" {
            Vec::new()
        } else {
            previous.quarantined_profile_ids.clone()
        };
    if next.quarantined_profile_ids != expected_quarantine
        || (next.status == "rolled_back" && next.distillation_lease.is_some())
    {
        return false;
    }
    if next.status == "canary" {
        let Some(next_candidate) = next.canary_profile_id.as_ref() else {
            return false;
        };
        if next
            .quarantined_profile_ids
            .iter()
            .any(|profile_id| profile_id == next_candidate)
        {
            return false;
        }
        if let Some(previous_candidate) = previous.canary_profile_id.as_ref() {
            let stage_advanced = matches!(
                (previous.canary_percent, next.canary_percent),
                (10, 25) | (25, 50)
            );
            let stage_unchanged = previous.canary_percent == next.canary_percent;
            if previous_candidate != next_candidate
                || (!stage_advanced && !stage_unchanged)
                || (stage_advanced
                    && (next.live_checkpoint <= previous.live_checkpoint
                        || (distillation_transition
                            && next.stable_live_checkpoint <= previous.stable_live_checkpoint)))
                || (stage_unchanged
                    && (next.live_checkpoint != previous.live_checkpoint
                        || (distillation_transition
                            && next.stable_live_checkpoint != previous.stable_live_checkpoint)))
                || previous.distillation_lease != next.distillation_lease
            {
                return false;
            }
        } else if next.canary_percent != 10 {
            return false;
        }
    }
    if next.status == "promoted" {
        previous.canary_profile_id.as_deref() == Some(next.stable_profile_id.as_str())
            && previous.canary_percent == 50
            && next.stable_profile_id != previous.stable_profile_id
            && next.live_checkpoint > previous.live_checkpoint
            && (!distillation_transition
                || next.stable_live_checkpoint > previous.stable_live_checkpoint)
            && next.distillation_lease.is_none()
            && next.quarantined_profile_ids.is_empty()
            && next.frozen_profile.as_ref().is_some_and(|snapshot| {
                frozen_is_valid(snapshot)
                    && snapshot.stable_profile_id == previous.stable_profile_id
            })
    } else {
        next.stable_profile_id == previous.stable_profile_id
            && next.frozen_profile == previous.frozen_profile
    }
}

fn prompt_quarantine_is_valid(profile_ids: &[String]) -> bool {
    profile_ids.len() <= PROMPT_ROLLOUT_MAX_QUARANTINED_PROFILES
        && profile_ids
            .iter()
            .all(|profile_id| !profile_id.trim().is_empty())
        && profile_ids.iter().collect::<BTreeSet<_>>().len() == profile_ids.len()
}

fn apply_prompt_rollout_event(model: &mut PromptEvolutionReadModel, event: &Event) {
    let Some((key, effort, rollout)) = prompt_rollout_record_from_event(event) else {
        return;
    };
    let previous = model.rollouts.get(&key).cloned();
    if !prompt_rollout_transition_is_valid(&effort, previous.as_ref(), &rollout) {
        return;
    }
    let scope = event
        .metadata
        .get("prompt_rollout_scope")
        .map(String::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .unwrap_or("global");
    let scoped = prompt_evolution_read_model_for_scope(model, scope);
    if crate::prompt_rollout_runtime::prompt_rollout_transition_has_canonical_evidence(
        &scoped,
        &effort,
        previous.as_ref(),
        &rollout,
    ) {
        model.rollouts.insert(key, rollout);
    }
}

pub(crate) fn upsert_prompt_evaluation_attempt(
    attempts: &mut BTreeMap<String, PromptEvaluationAttemptState>,
    event: PromptEvaluationAttemptEventV1,
) {
    let key = event.identity.evaluation_id.clone();
    if event.status == orchestrator::PromptEvaluationAttemptStatus::Started {
        attempts.entry(key).or_insert(PromptEvaluationAttemptState {
            started: event,
            terminal: None,
        });
    } else if let Some(state) = attempts.get_mut(&key) {
        if state.terminal.is_none()
            && state.started.identity == event.identity
            && state.started.treatments == event.treatments
        {
            state.terminal = Some(event);
        }
    }
}

fn upsert_prompt_learning_cohort(
    cohorts: &mut BTreeMap<String, PromptLearningCohortV1>,
    cohort_sequences: &mut BTreeMap<String, u64>,
    cohort: PromptLearningCohortV1,
    sequence: u64,
) {
    let digest = cohort.cohort_sha256.clone();
    cohorts.entry(digest.clone()).or_insert(cohort);
    cohort_sequences.insert(digest, sequence);
}

pub(crate) fn visible_prompt_rollout(
    model: &PromptEvolutionReadModel,
    effort: &str,
) -> Option<PromptRolloutState> {
    let conflicts = prompt_model_genome_conflict_keys(model);
    model
        .rollouts
        .get(effort)
        .filter(|rollout| {
            !prompt_rollout_references_conflicted_genome(rollout, "global", effort, &conflicts)
        })
        .cloned()
        .or_else(|| {
            let suffix = format!("{PROMPT_EVIDENCE_SCOPE_SEPARATOR}{effort}");
            model
                .rollouts
                .iter()
                .filter(|(key, _)| key.ends_with(&suffix))
                .filter(|(key, rollout)| {
                    key.strip_suffix(&suffix).is_some_and(|scope| {
                        !prompt_rollout_references_conflicted_genome(
                            rollout, scope, effort, &conflicts,
                        )
                    })
                })
                .map(|(_, rollout)| rollout)
                .max_by_key(|rollout| {
                    (
                        rollout.evidence_checkpoint,
                        rollout.live_checkpoint,
                        rollout.rollback_count,
                    )
                })
                .cloned()
        })
}

pub(crate) fn prompt_dataset_key(effort: &str, project_id: &str) -> String {
    format!("{effort}:{project_id}")
}

pub(crate) fn prompt_dataset_record_from_event(
    event: &Event,
) -> Option<(String, PromptOfflineDatasetState)> {
    if event.summary != "Conductor offline dataset selected" {
        return None;
    }
    let effort = event.metadata.get("prompt_effort")?.clone();
    let project_id = event.metadata.get("project_id")?.clone();
    let digest = event.metadata.get("dataset_sha256")?.clone();
    if effort.trim().is_empty() || project_id.trim().is_empty() || digest.trim().is_empty() {
        return None;
    }
    let parse_usize = |key: &str| {
        event
            .metadata
            .get(key)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default()
    };
    let selected_case_id = event
        .metadata
        .get("selected_case_id")
        .filter(|value| !value.trim().is_empty())
        .cloned();
    let case_ids = event
        .metadata
        .get("dataset_case_ids")
        .and_then(|encoded| serde_json::from_str::<Vec<String>>(encoded).ok())
        .unwrap_or_default();
    let key = prompt_dataset_key(&effort, &project_id);
    let identity = event
        .metadata
        .get("dataset_identity_v1")
        .and_then(|encoded| serde_json::from_str::<PromptDatasetIdentityV1>(encoded).ok())
        .filter(|identity| {
            identity.validate().is_ok()
                && identity.dataset_sha256 == digest
                && identity.scope_sha256 == sha256_hex(project_id.as_bytes())
        });
    Some((
        key,
        PromptOfflineDatasetState {
            effort,
            project_id,
            digest,
            identity,
            generation: event
                .metadata
                .get("dataset_generation")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or_default(),
            case_ids,
            case_count: parse_usize("dataset_case_count"),
            train_count: parse_usize("dataset_train_count"),
            holdout_count: parse_usize("dataset_holdout_count"),
            selected_case_id,
            status: event
                .metadata
                .get("dataset_status")
                .cloned()
                .unwrap_or_else(|| "ready".to_string()),
            updated_at_ms: event.timestamp_ms,
        },
    ))
}

pub(crate) fn upsert_prompt_observation(
    observations: &mut Vec<(String, PromptEvolutionObservation)>,
    effort: String,
    observation: PromptEvolutionObservation,
) {
    if observations.iter().any(|(existing_effort, existing)| {
        existing_effort == &effort
            && existing.evaluation_id == observation.evaluation_id
            && existing.profile_id == observation.profile_id
            && existing == &observation
    }) {
        return;
    }
    observations.push((effort, observation));
}

pub(crate) fn upsert_prompt_failure_curriculum(
    curricula: &mut Vec<PromptFailureCurriculumRecord>,
    record: PromptFailureCurriculumRecord,
) {
    if record.effort != "pro"
        || record.scope.trim().is_empty()
        || !record.receipt.matches_project(&record.scope)
    {
        return;
    }
    curricula.retain(|existing| {
        !(existing.scope == record.scope
            && existing.effort == record.effort
            && existing.receipt.run_sha256 == record.receipt.run_sha256
            && existing.receipt.profile_sha256 == record.receipt.profile_sha256
            && existing.receipt.steer_epoch == record.receipt.steer_epoch
            && existing.receipt.kind == record.receipt.kind
            && existing.receipt.failure_code == record.receipt.failure_code
            && existing.receipt.denial_kind == record.receipt.denial_kind
            && existing.receipt.tool_sha256 == record.receipt.tool_sha256
            && existing.receipt.input_fingerprint == record.receipt.input_fingerprint)
    });
    curricula.push(record);
}

pub(crate) fn replace_prompt_failure_curriculum_for_run(
    curricula: &mut Vec<PromptFailureCurriculumRecord>,
    run_id: &str,
    records: Vec<PromptFailureCurriculumRecord>,
) {
    curricula.retain(|record| !record.receipt.matches_run(run_id));
    for record in records {
        upsert_prompt_failure_curriculum(curricula, record);
    }
}

pub(crate) fn build_prompt_evolution_read_model(
    events: &[Event],
    revision: u64,
    event_count: u64,
) -> PromptEvolutionReadModel {
    let mut model = PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
        revision,
        event_count,
        genomes: Vec::new(),
        genome_identity_fingerprints: BTreeMap::new(),
        observations: Vec::new(),
        failure_curricula: Vec::new(),
        attempts: BTreeMap::new(),
        cohorts: BTreeMap::new(),
        cohort_sequences: BTreeMap::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    };
    let mut observations_by_sequence =
        BTreeMap::<u64, Vec<(String, PromptEvolutionObservation)>>::new();
    for (sequence, effort, observation) in prompt_evolution_observation_records_from_events(events)
    {
        observations_by_sequence
            .entry(sequence)
            .or_default()
            .push((effort, observation));
    }
    let mut curricula_by_sequence = BTreeMap::<u64, Vec<PromptFailureCurriculumRecord>>::new();
    for (sequence, record) in
        crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(
            events,
        )
    {
        curricula_by_sequence
            .entry(sequence)
            .or_default()
            .push(record);
    }
    let mut ordered_events = events.iter().collect::<Vec<_>>();
    ordered_events.sort_by_key(|event| event.sequence);
    for event in ordered_events {
        for record in prompt_genome_records_from_event(event) {
            upsert_prompt_genome(&mut model, record);
        }
        if let Some((key, dataset)) = prompt_dataset_record_from_event(event) {
            model.datasets.insert(key, dataset);
        }
        if let Some(attempt) =
            crate::prompt_attempt_runtime::prompt_evaluation_attempt_from_event(event)
        {
            upsert_prompt_evaluation_attempt(&mut model.attempts, attempt);
        }
        if let Some(cohort) =
            crate::prompt_attempt_runtime::prompt_learning_cohort_from_event(event)
        {
            upsert_prompt_learning_cohort(
                &mut model.cohorts,
                &mut model.cohort_sequences,
                cohort,
                event.sequence,
            );
        }
        if let Some(observations) = observations_by_sequence.remove(&event.sequence) {
            for (effort, observation) in observations {
                upsert_prompt_observation(&mut model.observations, effort, observation);
            }
        }
        if let Some(records) = curricula_by_sequence.remove(&event.sequence) {
            for record in records {
                upsert_prompt_failure_curriculum(&mut model.failure_curricula, record);
            }
        }
        crate::prompt_distillation_runtime::apply_prompt_distillation_event(&mut model, event);
        apply_prompt_rollout_event(&mut model, event);
    }
    compact_prompt_evolution_hot_state(&mut model);
    model
}

pub(crate) fn load_prompt_evolution_read_model(
    store: &mut SqliteStore,
) -> Result<PromptEvolutionReadModel, StorageError> {
    for publish_attempt in 0..2 {
        let observed = store.load_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
        )?;
        let (model, changed) = project_prompt_evolution_read_model(store, observed.as_ref())?;
        if !changed {
            return Ok(model);
        }
        if compare_exchange_prompt_evolution_read_model(store, observed.as_ref(), &model)? {
            return Ok(model);
        }
        if publish_attempt == 1 {
            return Err(StorageError::new(
                "prompt evolution snapshot publication conflicted twice".to_string(),
            ));
        }
    }
    unreachable!("prompt evolution publication attempts are bounded")
}

fn project_prompt_evolution_read_model(
    store: &mut SqliteStore,
    observed: Option<&StoredReadModel>,
) -> Result<(PromptEvolutionReadModel, bool), StorageError> {
    let task_id = phase16_task_id();
    let revision = store.event_revision(&task_id)?;
    let stored = observed.and_then(|stored| {
        serde_json::from_str::<PromptEvolutionReadModel>(&stored.payload)
            .ok()
            .filter(|model| {
                model.schema == PROMPT_EVOLUTION_READ_MODEL_NAMESPACE
                    && model.projection_version == PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION
                    && model.revision == stored.revision
                    && model.revision <= revision.latest_sequence
                    && model.event_count <= revision.event_count
                    && prompt_genome_scopes_are_valid(model)
            })
    });
    let had_stored_model = stored.is_some();
    let mut model = stored.unwrap_or_else(|| PromptEvolutionReadModel {
        schema: PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string(),
        projection_version: PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
        revision: 0,
        event_count: 0,
        genomes: Vec::new(),
        genome_identity_fingerprints: BTreeMap::new(),
        observations: Vec::new(),
        failure_curricula: Vec::new(),
        attempts: BTreeMap::new(),
        cohorts: BTreeMap::new(),
        cohort_sequences: BTreeMap::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    });
    let mut delta = store.list_by_task_after(&task_id, model.revision)?;
    let mut changed = !had_stored_model;
    if model.event_count.saturating_add(delta.len() as u64) != revision.event_count {
        changed = true;
        delta = store.list_by_task_after(&task_id, 0)?;
        model = build_prompt_evolution_read_model(
            &delta,
            revision.latest_sequence,
            revision.event_count,
        );
    } else if model.revision == 0 {
        changed = true;
        model = build_prompt_evolution_read_model(
            &delta,
            revision.latest_sequence,
            revision.event_count,
        );
    } else if !delta.is_empty() {
        changed = true;
        let mut auto_teacher_cache =
            BTreeMap::<(String, String), Option<PromptAutoTeacherCase>>::new();
        for event in &delta {
            for record in prompt_genome_records_from_event(event) {
                upsert_prompt_genome(&mut model, record);
            }
            if let Some((key, dataset)) = prompt_dataset_record_from_event(event) {
                model.datasets.insert(key, dataset);
            }
            if let Some(attempt) =
                crate::prompt_attempt_runtime::prompt_evaluation_attempt_from_event(event)
            {
                upsert_prompt_evaluation_attempt(&mut model.attempts, attempt);
            }
            if let Some(cohort) =
                crate::prompt_attempt_runtime::prompt_learning_cohort_from_event(event)
            {
                upsert_prompt_learning_cohort(
                    &mut model.cohorts,
                    &mut model.cohort_sequences,
                    cohort,
                    event.sequence,
                );
            }
            let source_key = prompt_auto_teacher_source_key(event);
            if let Some((project_id, source_run_id)) = source_key.as_ref() {
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    auto_teacher_cache.entry((project_id.clone(), source_run_id.clone()))
                {
                    let source_events =
                        store.list_by_task_and_metadata(&task_id, "agent_run_id", source_run_id)?;
                    let teacher = crate::prompt_evidence_runtime::reconstruct_prompt_auto_teacher_from_canonical_events(
                        &source_events,
                        project_id,
                        source_run_id,
                    );
                    entry.insert(teacher);
                }
            }
            let canonical_teacher = source_key
                .as_ref()
                .and_then(|key| auto_teacher_cache.get(key))
                .and_then(Option::as_ref)
                .map(CanonicalPromptAutoTeacher::from);
            for (effort, observation) in
                prompt_observation_records_from_event_with_canonical_teacher(
                    event,
                    canonical_teacher,
                )
            {
                upsert_prompt_observation(&mut model.observations, effort, observation);
            }
            crate::prompt_distillation_runtime::apply_prompt_distillation_event(&mut model, event);
            let terminal_scope = if is_agent_run_terminal(event)
                || AgentRunEvent::from_event(event) == Some(AgentRunEvent::Paused)
            {
                event
                    .metadata
                    .get("agent_run_id")
                    .map(|id| ("agent_run_id", id.as_str()))
            } else if matches!(
                event.summary.as_str(),
                "Collaboration workflow completed" | "Collaboration workflow failed"
            ) {
                event
                    .metadata
                    .get("collaboration_id")
                    .map(|id| ("collaboration_id", id.as_str()))
            } else {
                None
            };
            if let Some((scope, value)) = terminal_scope {
                let scoped_events = store.list_by_task_and_metadata(&task_id, scope, value)?;
                for (effort, observation) in
                    prompt_evolution_observations_from_events(&scoped_events)
                {
                    upsert_prompt_observation(&mut model.observations, effort, observation);
                }
                if scope == "agent_run_id" {
                    let records = crate::prompt_failure_curriculum_projection::prompt_failure_curriculum_records_from_events(&scoped_events)
                        .into_iter()
                        .map(|(_, record)| record)
                        .collect();
                    replace_prompt_failure_curriculum_for_run(
                        &mut model.failure_curricula,
                        value,
                        records,
                    );
                }
            }
            apply_prompt_rollout_event(&mut model, event);
        }
        compact_prompt_evolution_hot_state(&mut model);
        model.revision = revision.latest_sequence;
        model.event_count = revision.event_count;
    }
    Ok((model, changed))
}

fn prompt_genome_scopes_are_valid(model: &PromptEvolutionReadModel) -> bool {
    model
        .genomes
        .iter()
        .all(|record| !record.scope.trim().is_empty())
        && prompt_genome_identities_are_valid(model)
        && model.failure_curricula.iter().all(|record| {
            record.effort == "pro"
                && !record.scope.trim().is_empty()
                && record.receipt.matches_project(&record.scope)
        })
}

fn compare_exchange_prompt_evolution_read_model(
    store: &mut SqliteStore,
    expected: Option<&StoredReadModel>,
    model: &PromptEvolutionReadModel,
) -> Result<bool, StorageError> {
    let revision = store.event_revision(&phase16_task_id())?;
    if model.revision != revision.latest_sequence || model.event_count != revision.event_count {
        return Ok(false);
    }
    let payload = serde_json::to_string(model).map_err(|error| {
        StorageError::new(format!("prompt evolution serialization failed: {error}"))
    })?;
    store.compare_exchange_read_model(
        PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
        PROMPT_EVOLUTION_READ_MODEL_KEY,
        expected,
        model.revision,
        &payload,
    )
}

pub(crate) fn prompt_evolution_profile_events(model: &PromptEvolutionReadModel) -> Vec<Event> {
    let conflicts = prompt_model_genome_conflict_keys(model);
    model
        .genomes
        .iter()
        .enumerate()
        .filter(|(_, record)| !conflicts.contains(&prompt_genome_key(record)))
        .filter_map(|(index, record)| {
            Some(Event {
                id: EventId(format!("prompt-read-model-{index}")),
                task_id: phase16_task_id(),
                sequence: index as u64 + 1,
                timestamp_ms: 0,
                kind: EventKind::TaskStatusChanged,
                summary: "Conductor prompt profile indexed".to_string(),
                metadata: [
                    ("project_id".to_string(), record.scope.clone()),
                    ("prompt_effort".to_string(), record.effort.clone()),
                    (
                        "prompt_genome".to_string(),
                        serde_json::to_string(&record.genome).ok()?,
                    ),
                ]
                .into_iter()
                .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "prompt_evolution_read_model_tests.rs"]
mod tests;
