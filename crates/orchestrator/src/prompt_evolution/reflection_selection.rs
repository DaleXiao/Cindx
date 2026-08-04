use super::{
    latest_scientific_training_dataset_digest, latest_scientific_transfer_dataset_digest,
    PromptEvaluationMode, PromptEvaluationSplit, PromptEvolutionObservation,
};
use crate::AgentEvaluationReflectionPacket;
use std::collections::{BTreeMap, BTreeSet};

pub const PROMPT_REFLECTION_SELECTOR_SCHEMA_V1: &str = "cindx.prompt-reflection-selector.v1";
const MIN_REFLECTION_INFORMATION_SCORE: u64 = 25_000;

pub fn prompt_reflection_packets(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    limit: usize,
) -> Vec<AgentEvaluationReflectionPacket> {
    if limit == 0 {
        return Vec::new();
    }
    let Some(active_dataset_sha256) = latest_scientific_training_dataset_digest(observations)
    else {
        return Vec::new();
    };
    let candidates = observations
        .iter()
        .filter(|observation| {
            observation.profile_id == profile_id
                && observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_scientific_evidence()
                && observation.scientific_cohort_sha256() == Some(active_dataset_sha256)
        })
        .filter(|observation| {
            observation
                .reflection_packet
                .as_ref()
                .is_some_and(|packet| packet.candidate_id == profile_id)
        })
        .filter(|observation| {
            reflection_information_score(observation) >= MIN_REFLECTION_INFORMATION_SCORE
        })
        .collect::<Vec<_>>();
    select_high_information_observations(candidates, limit)
        .into_iter()
        .filter_map(|observation| observation.reflection_packet.clone())
        .collect()
}

pub fn prompt_transfer_reflection_packets(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    limit: usize,
) -> Vec<AgentEvaluationReflectionPacket> {
    prompt_transfer_reflection_pairs(observations, profile_id, limit)
        .into_iter()
        .filter(|packet| packet.candidate_id == profile_id)
        .take(limit)
        .collect()
}

pub fn prompt_transfer_reflection_pairs(
    observations: &[PromptEvolutionObservation],
    profile_id: &str,
    pair_limit: usize,
) -> Vec<AgentEvaluationReflectionPacket> {
    if pair_limit == 0 {
        return Vec::new();
    }
    let Some(active_dataset_sha256) = latest_scientific_transfer_dataset_digest(observations)
    else {
        return Vec::new();
    };
    let eligible = observations
        .iter()
        .filter(|observation| {
            observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_strict_source_attested_transfer_evidence()
                && observation.scientific_cohort_sha256() == Some(active_dataset_sha256)
                && observation.reflection_packet.is_some()
        })
        .collect::<Vec<_>>();
    let mut candidates = BTreeMap::new();
    let mut teachers = BTreeMap::new();
    let mut conflicting_keys = BTreeSet::new();
    for observation in eligible {
        let Some(identity) = observation.provenance.matched_evaluation.as_ref() else {
            continue;
        };
        let key = (
            identity.cohort_sha256.clone(),
            observation.evaluation_id.clone(),
            observation.case_id.clone(),
        );
        let observations_for_side = if observation.profile_id == profile_id {
            &mut candidates
        } else if observation.opponent_profile_id.as_deref() == Some(profile_id) {
            &mut teachers
        } else {
            continue;
        };
        if observations_for_side
            .get(&key)
            .is_some_and(|existing| *existing != observation)
        {
            conflicting_keys.insert(key);
        } else {
            observations_for_side.entry(key).or_insert(observation);
        }
    }

    let mut pairs = candidates
        .into_iter()
        .filter_map(|(key, candidate)| {
            if conflicting_keys.contains(&key) {
                return None;
            }
            let teacher = teachers.get(&key).copied()?;
            let transfer = candidate.provenance.transfer.as_ref()?;
            let candidate_packet = candidate.reflection_packet.as_ref()?;
            let teacher_packet = teacher.reflection_packet.as_ref()?;
            (teacher.profile_id == transfer.source_profile_id
                && candidate.opponent_profile_id.as_deref() == Some(teacher.profile_id.as_str())
                && teacher.opponent_profile_id.as_deref() == Some(candidate.profile_id.as_str())
                && candidate.provenance.transfer == teacher.provenance.transfer
                && candidate.provenance.matched_evaluation == teacher.provenance.matched_evaluation
                && candidate_packet.candidate_id == candidate.profile_id
                && teacher_packet.candidate_id == teacher.profile_id)
                .then_some((candidate, teacher))
        })
        .collect::<Vec<_>>();
    pairs.retain(|(candidate, teacher)| {
        transfer_reflection_information_score(candidate, teacher)
            >= MIN_REFLECTION_INFORMATION_SCORE
    });
    pairs.sort_by(
        |(left_candidate, left_teacher), (right_candidate, right_teacher)| {
            transfer_reflection_information_score(right_candidate, right_teacher)
                .cmp(&transfer_reflection_information_score(
                    left_candidate,
                    left_teacher,
                ))
                .then_with(|| left_candidate.task_class.cmp(&right_candidate.task_class))
                .then_with(|| left_candidate.case_id.cmp(&right_candidate.case_id))
                .then_with(|| {
                    left_candidate
                        .evaluation_id
                        .cmp(&right_candidate.evaluation_id)
                })
        },
    );

    let mut selected = Vec::new();
    let mut selected_keys = BTreeSet::new();
    let mut diversity = BTreeSet::new();
    for (candidate, teacher) in &pairs {
        let signature = (
            candidate.task_class.clone(),
            candidate.succeeded,
            reflection_strategy_signature(candidate.reflection_packet.as_ref().unwrap()),
            reflection_strategy_signature(teacher.reflection_packet.as_ref().unwrap()),
        );
        if diversity.insert(signature) {
            selected.push((*candidate, *teacher));
            selected_keys.insert((candidate.evaluation_id.as_str(), candidate.case_id.as_str()));
            if selected.len() == pair_limit {
                break;
            }
        }
    }
    if selected.len() < pair_limit {
        for (candidate, teacher) in pairs {
            if selected_keys.insert((candidate.evaluation_id.as_str(), candidate.case_id.as_str()))
            {
                selected.push((candidate, teacher));
                if selected.len() == pair_limit {
                    break;
                }
            }
        }
    }

    selected
        .into_iter()
        .flat_map(|(candidate, teacher)| {
            [
                candidate.reflection_packet.clone().unwrap(),
                teacher.reflection_packet.clone().unwrap(),
            ]
        })
        .collect()
}

fn select_high_information_observations<'a>(
    mut observations: Vec<&'a PromptEvolutionObservation>,
    limit: usize,
) -> Vec<&'a PromptEvolutionObservation> {
    observations.sort_by(|left, right| {
        reflection_information_score(right)
            .cmp(&reflection_information_score(left))
            .then_with(|| left.task_class.cmp(&right.task_class))
            .then_with(|| left.case_id.cmp(&right.case_id))
            .then_with(|| left.evaluation_id.cmp(&right.evaluation_id))
    });
    let mut selected = Vec::new();
    let mut selected_keys = BTreeSet::new();
    let mut diversity = BTreeSet::new();
    for observation in &observations {
        let packet = observation.reflection_packet.as_ref().unwrap();
        let signature = (observation.task_class.as_str(), observation.succeeded);
        if diversity.insert(signature) {
            selected.push(*observation);
            selected_keys.insert((packet.run_id.as_str(), packet.case_id.as_str()));
            if selected.len() == limit {
                return selected;
            }
        }
    }
    for observation in observations {
        let packet = observation.reflection_packet.as_ref().unwrap();
        if selected_keys.insert((packet.run_id.as_str(), packet.case_id.as_str())) {
            selected.push(observation);
            if selected.len() == limit {
                break;
            }
        }
    }
    selected
}

fn transfer_reflection_information_score(
    candidate: &PromptEvolutionObservation,
    teacher: &PromptEvolutionObservation,
) -> u64 {
    let strategy_contrast = match (
        candidate.reflection_packet.as_ref(),
        teacher.reflection_packet.as_ref(),
    ) {
        (Some(candidate), Some(teacher)) => {
            let step_delta = candidate.steps.len().abs_diff(teacher.steps.len()) as u64;
            let role_delta = reflection_roles(candidate)
                .symmetric_difference(&reflection_roles(teacher))
                .count() as u64;
            step_delta.saturating_mul(500) + role_delta.saturating_mul(750)
        }
        _ => 0,
    };
    reflection_information_score(candidate)
        .max(reflection_information_score(teacher))
        .saturating_add(strategy_contrast.min(10_000))
}

fn reflection_information_score(observation: &PromptEvolutionObservation) -> u64 {
    let Some(packet) = observation.reflection_packet.as_ref() else {
        return 0;
    };
    let relative_signal = (observation
        .relative_reward
        .unwrap_or_default()
        .abs()
        .clamp(0.0, 1.0)
        * 100_000.0) as u64;
    let quality_gap = ((1.0 - observation.quality_score.clamp(0.0, 1.0)) * 50_000.0) as u64;
    let actionable = packet
        .actionable_feedback
        .failed_constraints
        .len()
        .saturating_add(packet.actionable_feedback.errors.len())
        .saturating_add(packet.actionable_feedback.suggested_changes.len())
        as u64;
    let runtime_failures = packet
        .steps
        .iter()
        .map(|step| {
            step.errors.len()
                + step
                    .tool_calls
                    .iter()
                    .filter(|call| call.error.is_some())
                    .count()
        })
        .sum::<usize>() as u64;
    u64::from(!observation.succeeded || !packet.verifier.passed)
        .saturating_mul(200_000)
        .saturating_add(relative_signal)
        .saturating_add(quality_gap)
        .saturating_add(actionable.min(32).saturating_mul(4_000))
        .saturating_add(runtime_failures.min(32).saturating_mul(6_000))
}

fn reflection_roles(packet: &AgentEvaluationReflectionPacket) -> BTreeSet<&str> {
    packet.steps.iter().map(|step| step.role.as_str()).collect()
}

fn reflection_strategy_signature(packet: &AgentEvaluationReflectionPacket) -> String {
    format!(
        "{}:{}",
        packet.steps.len(),
        reflection_roles(packet)
            .into_iter()
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActionableSideInformation, AgentEvaluationCheck, AgentEvaluationEvidenceSource,
        AgentEvaluationTraceStep, AgentEvaluationVerifierOutcome, AutoTeacherSourceContextV1,
        PromptEvaluationProvenance, PromptMatchedEvaluationIdentityV1, PromptTransferProvenance,
        AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1, PROMPT_MATCHED_EVALUATION_SCHEMA_V1,
    };

    fn source_context() -> AutoTeacherSourceContextV1 {
        AutoTeacherSourceContextV1 {
            schema: AUTO_TEACHER_SOURCE_CONTEXT_SCHEMA_V1.to_string(),
            provider_sha256: "1".repeat(64),
            model_pool_sha256: "2".repeat(64),
            system_prompt_sha256: "3".repeat(64),
            policy_sha256: "4".repeat(64),
            budget_sha256: "5".repeat(64),
            tool_contract_sha256: "6".repeat(64),
            source_revision_sha256: "7".repeat(64),
            workspace_revision_sha256: "8".repeat(64),
            evaluator_identity_sha256: "9".repeat(64),
            evaluator_receipt_sha256: "a".repeat(64),
            checkpoint_sha256: "b".repeat(64),
            learning_receipt_sha256: "c".repeat(64),
        }
    }

    fn reflection_packet(
        candidate_id: &str,
        run_id: &str,
        case_id: &str,
        passed: bool,
    ) -> AgentEvaluationReflectionPacket {
        AgentEvaluationReflectionPacket {
            suite_id: "runtime-prompt-evolution".to_string(),
            suite_version: 2,
            case_id: case_id.to_string(),
            category: "coding".to_string(),
            run_id: run_id.to_string(),
            seed: 0,
            candidate_id: candidate_id.to_string(),
            candidate_fingerprint: format!("fingerprint-{candidate_id}"),
            model_fingerprints: BTreeMap::new(),
            input: "verify a bounded change".to_string(),
            steps: vec![AgentEvaluationTraceStep {
                step_id: "worker".to_string(),
                role: "worker".to_string(),
                model: "configured-worker".to_string(),
                prompt: "inspect and verify".to_string(),
                output: "bounded result".to_string(),
                tool_calls: Vec::new(),
                errors: (!passed)
                    .then(|| "verification failed".to_string())
                    .into_iter()
                    .collect(),
                latency_ms: 10,
                total_tokens: 20,
            }],
            final_output: "bounded result".to_string(),
            verifier: AgentEvaluationVerifierOutcome {
                source: AgentEvaluationEvidenceSource::Judge,
                passed,
                score: if passed { 0.9 } else { 0.2 },
                checks: vec![AgentEvaluationCheck {
                    id: "pairwise_quality".to_string(),
                    passed,
                    detail: "matched quality".to_string(),
                }],
            },
            actionable_feedback: ActionableSideInformation {
                summary: "matched comparison".to_string(),
                failed_constraints: (!passed)
                    .then(|| "preserve verification".to_string())
                    .into_iter()
                    .collect(),
                suggested_changes: (!passed)
                    .then(|| "use an independent check".to_string())
                    .into_iter()
                    .collect(),
                ..ActionableSideInformation::default()
            },
        }
    }

    fn strict_transfer_pair(
        run_id: &str,
        case_id: &str,
        candidate_succeeded: bool,
    ) -> [PromptEvolutionObservation; 2] {
        let candidate_id = "pro-candidate";
        let teacher_id = "auto-teacher";
        let dataset_sha256 = "a".repeat(64);
        let candidate_sha256 = "e".repeat(64);
        let teacher_sha256 = "d".repeat(64);
        let transfer = PromptTransferProvenance::auto_to_pro(
            format!("source-{run_id}"),
            0,
            teacher_id,
            teacher_sha256.clone(),
            "c".repeat(64),
        )
        .with_source_context(&source_context())
        .unwrap();
        let matched = PromptMatchedEvaluationIdentityV1 {
            schema: PROMPT_MATCHED_EVALUATION_SCHEMA_V1.to_string(),
            evaluation_id: run_id.to_string(),
            cohort_sha256: "f".repeat(64),
            dataset_sha256: dataset_sha256.clone(),
            case_id: case_id.to_string(),
            objective_sha256: "b".repeat(64),
            split: PromptEvaluationSplit::Train,
            mode: PromptEvaluationMode::PairedExecution,
        };
        let observation = |profile_id: &str,
                           opponent_id: &str,
                           succeeded: bool,
                           quality_score: f64,
                           relative_reward: f64,
                           candidate_prompt_sha256: String,
                           opponent_prompt_sha256: String| {
            PromptEvolutionObservation {
                profile_id: profile_id.to_string(),
                evaluation_id: run_id.to_string(),
                case_id: case_id.to_string(),
                opponent_profile_id: Some(opponent_id.to_string()),
                task_class: "coding".to_string(),
                split: PromptEvaluationSplit::Train,
                mode: PromptEvaluationMode::PairedExecution,
                format_valid: true,
                succeeded,
                quality_score,
                latency_ms: 100,
                total_tokens: 100,
                estimated_cost_microusd: 0,
                safety_violations: 0,
                relative_reward: Some(relative_reward),
                step_credits: Vec::new(),
                reflection_packet: Some(reflection_packet(profile_id, run_id, case_id, succeeded)),
                provenance: PromptEvaluationProvenance::blind_pairwise_swap(
                    vec!["independent-reviewer".to_string()],
                    vec!["pro-worker".to_string(), "auto-worker".to_string()],
                    dataset_sha256.clone(),
                    candidate_prompt_sha256,
                    opponent_prompt_sha256,
                )
                .with_transfer(transfer.clone())
                .with_matched_evaluation(matched.clone()),
            }
        };
        [
            observation(
                candidate_id,
                teacher_id,
                candidate_succeeded,
                if candidate_succeeded { 0.85 } else { 0.2 },
                if candidate_succeeded { 0.25 } else { -0.7 },
                candidate_sha256.clone(),
                teacher_sha256.clone(),
            ),
            observation(
                teacher_id,
                candidate_id,
                true,
                0.9,
                if candidate_succeeded { -0.25 } else { 0.7 },
                teacher_sha256,
                candidate_sha256,
            ),
        ]
    }

    #[test]
    fn transfer_reflection_requires_complete_pair_and_includes_auto() {
        let pair = strict_transfer_pair("matched-1", "case-1", true);
        assert!(pair
            .iter()
            .all(PromptEvolutionObservation::is_strict_source_attested_transfer_evidence));
        let packets = prompt_transfer_reflection_pairs(&pair, "pro-candidate", 1);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].candidate_id, "pro-candidate");
        assert_eq!(packets[1].candidate_id, "auto-teacher");
        assert!(prompt_transfer_reflection_pairs(&pair[..1], "pro-candidate", 1).is_empty());
    }

    #[test]
    fn older_actionable_regret_beats_newer_low_information_deterministically() {
        let high_information = strict_transfer_pair("run-a", "case-a", false);
        let mut low_information = strict_transfer_pair("run-z", "case-z", true);
        for observation in &mut low_information {
            observation.quality_score = 1.0;
            observation.relative_reward = Some(0.0);
        }
        let observations = high_information
            .into_iter()
            .chain(low_information)
            .collect::<Vec<_>>();
        let selected = prompt_transfer_reflection_pairs(&observations, "pro-candidate", 1);
        let reversed = prompt_transfer_reflection_pairs(
            &observations.iter().cloned().rev().collect::<Vec<_>>(),
            "pro-candidate",
            1,
        );
        assert_eq!(selected, reversed);
        assert_eq!(selected[0].run_id, "run-a");
        assert_eq!(selected[1].run_id, "run-a");
    }

    #[test]
    fn low_information_pair_cannot_trigger_reflection() {
        let mut pair = strict_transfer_pair("matched-low", "case-low", true);
        for observation in &mut pair {
            observation.quality_score = 1.0;
            observation.relative_reward = Some(0.0);
        }

        assert!(prompt_transfer_reflection_pairs(&pair, "pro-candidate", 1).is_empty());
    }

    #[test]
    fn duplicate_replays_are_deterministic_and_conflicts_fail_closed() {
        let pair = strict_transfer_pair("matched-replay", "case-replay", false);
        let exact_replay = pair
            .iter()
            .cloned()
            .chain(pair.iter().cloned())
            .collect::<Vec<_>>();
        let selected = prompt_transfer_reflection_pairs(&exact_replay, "pro-candidate", 1);
        let reversed = prompt_transfer_reflection_pairs(
            &exact_replay.iter().cloned().rev().collect::<Vec<_>>(),
            "pro-candidate",
            1,
        );
        assert_eq!(selected, reversed);
        assert_eq!(selected.len(), 2);

        let mut conflicting_candidate = pair[0].clone();
        conflicting_candidate.quality_score = 0.99;
        let conflicting_replay = pair
            .iter()
            .cloned()
            .chain(std::iter::once(conflicting_candidate))
            .collect::<Vec<_>>();
        assert!(
            prompt_transfer_reflection_pairs(&conflicting_replay, "pro-candidate", 1).is_empty()
        );
        assert!(prompt_transfer_reflection_pairs(
            &conflicting_replay.iter().cloned().rev().collect::<Vec<_>>(),
            "pro-candidate",
            1
        )
        .is_empty());
    }
}
