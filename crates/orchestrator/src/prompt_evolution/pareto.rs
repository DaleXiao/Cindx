use super::fitness::summarize;
use super::*;
use crate::{AgentEvaluationCaseScore, AgentEvaluationSplit};
use std::collections::{BTreeMap, BTreeSet};

const PROMOTION_WILSON_Z: f64 = 1.96;
pub(super) const PROMOTION_MIN_LOWER_BOUND: f64 = 0.55;

impl PromptParetoArchive {
    pub fn build(
        genomes: &[ConductorPromptGenome],
        observations: &[PromptEvolutionObservation],
        minimum_train_runs: usize,
        minimum_holdout_runs: usize,
    ) -> Result<Self, String> {
        let active_cohort_sha256 = latest_scientific_dataset_digest(observations);
        let mut genome_ids = BTreeSet::new();
        for genome in genomes {
            genome.validate()?;
            if !genome_ids.insert(genome.id.as_str()) {
                return Err(format!("duplicated prompt genome id: {}", genome.id));
            }
        }
        let by_id = genomes
            .iter()
            .map(|genome| (genome.id.as_str(), genome))
            .collect::<BTreeMap<_, _>>();
        if observations
            .iter()
            .any(|observation| !by_id.contains_key(observation.profile_id.as_str()))
        {
            return Err("prompt observation references an unknown profile".to_string());
        }

        let mut eligible = Vec::new();
        let mut rejected_profiles = Vec::new();
        for genome in genomes {
            let train = summarize(observations.iter().filter(|observation| {
                observation.profile_id == genome.id
                    && observation.split == PromptEvaluationSplit::Train
                    && observation.mode == PromptEvaluationMode::PairedExecution
                    && observation.is_scientific_evidence()
                    && active_cohort_sha256.is_some_and(|digest| {
                        observation.scientific_cohort_sha256() == Some(digest)
                    })
            }));
            let holdout = summarize(observations.iter().filter(|observation| {
                observation.profile_id == genome.id
                    && observation.split == PromptEvaluationSplit::Holdout
                    && observation.mode == PromptEvaluationMode::ReplayExecution
                    && observation.is_scientific_evidence()
                    && active_cohort_sha256.is_some_and(|digest| {
                        observation.scientific_cohort_sha256() == Some(digest)
                    })
            }));
            if train.paired_runs < minimum_train_runs
                || holdout.replay_runs < minimum_holdout_runs
                || train.execution_runs < minimum_train_runs
                || holdout.execution_runs < minimum_holdout_runs
            {
                rejected_profiles.push(genome.id.clone());
                continue;
            }
            let gap = (train.average_reward - holdout.average_reward).max(0.0);
            if gap > 0.15 || holdout.format_valid_rate < 1.0 || holdout.safety_violations > 0 {
                rejected_profiles.push(genome.id.clone());
                continue;
            }
            let confidence =
                prompt_promotion_confidence(observations.iter().filter(|observation| {
                    observation.profile_id == genome.id
                        && observation.split == PromptEvaluationSplit::Holdout
                        && observation.mode == PromptEvaluationMode::ReplayExecution
                        && observation.is_scientific_evidence()
                        && active_cohort_sha256.is_some_and(|digest| {
                            observation.scientific_cohort_sha256() == Some(digest)
                        })
                }));
            if confidence.wilson_lower_bound < PROMOTION_MIN_LOWER_BOUND {
                rejected_profiles.push(genome.id.clone());
                continue;
            }
            eligible.push(PromptParetoCandidate {
                genome: genome.clone(),
                train,
                holdout,
                quality_generalization_gap: gap,
                confidence,
            });
        }

        let candidates = eligible
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !eligible.iter().enumerate().any(|(other_index, other)| {
                    index != &other_index && dominates(other, candidate)
                })
            })
            .map(|(_, candidate)| candidate.clone())
            .collect::<Vec<_>>();
        Ok(Self {
            candidates,
            rejected_profiles,
        })
    }

    pub fn champion(&self) -> Option<&PromptParetoCandidate> {
        self.candidates.iter().max_by(|left, right| {
            left.robust_score()
                .total_cmp(&right.robust_score())
                .then_with(|| {
                    left.holdout
                        .average_quality
                        .total_cmp(&right.holdout.average_quality)
                })
                .then_with(|| left.holdout.runs.cmp(&right.holdout.runs))
                .then_with(|| right.genome.id.cmp(&left.genome.id))
        })
    }

    pub fn frontier_score(&self) -> f64 {
        self.champion()
            .map(PromptParetoCandidate::robust_score)
            .unwrap_or_default()
    }
}

impl PromptInstanceParetoArchive {
    pub fn build(
        genomes: &[ConductorPromptGenome],
        scores: &[AgentEvaluationCaseScore],
        minimum_repeats_per_case: usize,
    ) -> Result<Self, String> {
        if minimum_repeats_per_case == 0 {
            return Err("instance-wise Pareto requires a positive repeat minimum".to_string());
        }
        let mut profile_ids = BTreeSet::new();
        for genome in genomes {
            genome.validate()?;
            if !profile_ids.insert(genome.id.as_str()) {
                return Err(format!("duplicated prompt genome id: {}", genome.id));
            }
        }
        if scores
            .iter()
            .any(|score| score.split != AgentEvaluationSplit::Pareto)
        {
            return Err("instance-wise Pareto accepts only Pareto score records".to_string());
        }
        if scores
            .iter()
            .any(|score| !profile_ids.contains(score.candidate_id.as_str()))
        {
            return Err("instance-wise Pareto score references an unknown profile".to_string());
        }
        let suites = scores
            .iter()
            .map(|score| (score.suite_id.as_str(), score.suite_version))
            .collect::<BTreeSet<_>>();
        if suites.len() > 1 {
            return Err("instance-wise Pareto scores span multiple evaluation suites".to_string());
        }

        let mut grouped =
            BTreeMap::<(String, String), BTreeMap<String, &AgentEvaluationCaseScore>>::new();
        for score in scores {
            if !score.score.is_finite() || !(0.0..=1.0).contains(&score.score) {
                return Err(
                    "instance-wise Pareto score must be finite and between 0 and 1".to_string(),
                );
            }
            let repeat_identity = if score.run_id.trim().is_empty() {
                format!("seed:{}", score.seed)
            } else {
                format!("run:{}", score.run_id.trim())
            };
            let repeats = grouped
                .entry((score.candidate_id.clone(), score.case_id.clone()))
                .or_default();
            if let Some(existing) = repeats.get(&repeat_identity) {
                if *existing != score {
                    return Err(format!(
                        "conflicting instance-wise Pareto repeat: {} / {} / {}",
                        score.candidate_id, score.case_id, repeat_identity
                    ));
                }
                continue;
            }
            repeats.insert(repeat_identity, score);
        }

        let mut averages = BTreeMap::<(String, String), (f64, f64, usize)>::new();
        for ((profile_id, case_id), entries) in grouped {
            let entries = entries.into_values().collect::<Vec<_>>();
            if entries.len() < minimum_repeats_per_case
                || entries.iter().any(|entry| entry.safety_violations > 0)
            {
                continue;
            }
            let divisor = entries.len() as f64;
            averages.insert(
                (profile_id, case_id),
                (
                    entries.iter().map(|entry| entry.score).sum::<f64>() / divisor,
                    entries
                        .iter()
                        .filter(|entry| entry.verified_success)
                        .count() as f64
                        / divisor,
                    entries.len(),
                ),
            );
        }

        let case_ids = averages
            .keys()
            .map(|(_, case_id)| case_id.clone())
            .collect::<BTreeSet<_>>();
        let mut case_best_scores = BTreeMap::new();
        let mut leading_cases = BTreeMap::<String, Vec<String>>::new();
        for case_id in case_ids {
            let best = averages
                .iter()
                .filter(|((_, candidate_case), _)| candidate_case == &case_id)
                .map(|(_, (average, _, _))| *average)
                .max_by(f64::total_cmp)
                .unwrap_or_default();
            case_best_scores.insert(case_id.clone(), best);
            for ((profile_id, candidate_case), (average, _, _)) in &averages {
                if candidate_case == &case_id && (*average - best).abs() <= f64::EPSILON * 8.0 {
                    leading_cases
                        .entry(profile_id.clone())
                        .or_default()
                        .push(case_id.clone());
                }
            }
        }

        let mut profile_average_scores = BTreeMap::new();
        for genome in genomes {
            let profile_entries = averages
                .iter()
                .filter(|((profile_id, _), _)| profile_id == &genome.id)
                .map(|(_, value)| *value)
                .collect::<Vec<_>>();
            if !profile_entries.is_empty() {
                profile_average_scores.insert(
                    genome.id.clone(),
                    profile_entries
                        .iter()
                        .map(|(average, _, _)| *average)
                        .sum::<f64>()
                        / profile_entries.len() as f64,
                );
            }
        }

        let mut candidates = genomes
            .iter()
            .filter_map(|genome| {
                let mut cases = leading_cases.remove(&genome.id)?;
                cases.sort();
                let profile_entries = averages
                    .iter()
                    .filter(|((profile_id, _), _)| profile_id == &genome.id)
                    .map(|(_, value)| *value)
                    .collect::<Vec<_>>();
                let evaluations = profile_entries
                    .iter()
                    .map(|(_, _, evaluations)| *evaluations)
                    .sum();
                let verified_success_rate = profile_entries
                    .iter()
                    .map(|(_, success_rate, _)| *success_rate)
                    .sum::<f64>()
                    / profile_entries.len().max(1) as f64;
                Some(PromptInstanceParetoCandidate {
                    profile_id: genome.id.clone(),
                    leading_cases: cases,
                    average_score: profile_average_scores
                        .get(&genome.id)
                        .copied()
                        .unwrap_or_default(),
                    verified_success_rate,
                    evaluations,
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
        Ok(Self {
            candidates,
            case_best_scores,
            profile_average_scores,
        })
    }

    pub fn select_for_mutation(&self, sample: u64) -> Option<&PromptInstanceParetoCandidate> {
        let total_weight = self
            .candidates
            .iter()
            .map(|candidate| candidate.leading_cases.len())
            .sum::<usize>();
        if total_weight == 0 {
            return None;
        }
        let mut position = (sample as usize) % total_weight;
        for candidate in &self.candidates {
            let weight = candidate.leading_cases.len();
            if position < weight {
                return Some(candidate);
            }
            position -= weight;
        }
        None
    }

    pub fn best_aggregate(&self) -> Option<&PromptInstanceParetoCandidate> {
        self.candidates.iter().max_by(|left, right| {
            left.average_score
                .total_cmp(&right.average_score)
                .then_with(|| {
                    left.verified_success_rate
                        .total_cmp(&right.verified_success_rate)
                })
                .then_with(|| left.evaluations.cmp(&right.evaluations))
                .then_with(|| right.profile_id.cmp(&left.profile_id))
        })
    }

    pub fn merge_complementary(
        &self,
        id: impl Into<String>,
        ancestor: &ConductorPromptGenome,
        left: &ConductorPromptGenome,
        right: &ConductorPromptGenome,
    ) -> Result<ConductorPromptGenome, String> {
        let left_candidate = self
            .candidates
            .iter()
            .find(|candidate| candidate.profile_id == left.id)
            .ok_or_else(|| {
                "left merge candidate is not instance-wise Pareto-optimal".to_string()
            })?;
        let right_candidate = self
            .candidates
            .iter()
            .find(|candidate| candidate.profile_id == right.id)
            .ok_or_else(|| {
                "right merge candidate is not instance-wise Pareto-optimal".to_string()
            })?;
        let left_cases = left_candidate
            .leading_cases
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let right_cases = right_candidate
            .leading_cases
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if left_cases.difference(&right_cases).next().is_none()
            || right_cases.difference(&left_cases).next().is_none()
        {
            return Err("merge candidates do not lead complementary Pareto cases".to_string());
        }
        let ancestor_score = self
            .profile_average_scores
            .get(&ancestor.id)
            .copied()
            .ok_or_else(|| "merge ancestor has no complete Pareto evaluation".to_string())?;
        if left_candidate.average_score <= ancestor_score
            || right_candidate.average_score <= ancestor_score
        {
            return Err("both merge candidates must improve on their ancestor".to_string());
        }
        ConductorPromptGenome::system_aware_merge(id, ancestor, left, right)
    }
}

impl PromptParetoCandidate {
    pub fn robust_score(&self) -> f64 {
        if self.holdout.runs == 0
            || self.holdout.format_valid_rate < 1.0
            || self.holdout.safety_violations > 0
        {
            return 0.0;
        }
        let evidence = self.holdout.runs * self.holdout.task_class_coverage.max(1);
        let confidence_penalty = 0.08 / (evidence as f64).sqrt();
        let relative = (self.holdout.average_relative_reward + 1.0) * 0.5;
        (self.holdout.average_reward * 0.65
            + relative * 0.25
            + self.holdout.average_step_credit * 0.10
            - confidence_penalty
            - self.quality_generalization_gap * 0.5)
            .clamp(0.0, 1.0)
    }
}

pub fn prompt_promotion_confidence<'a>(
    observations: impl Iterator<Item = &'a PromptEvolutionObservation>,
) -> PromptPromotionConfidence {
    let mut seen = BTreeSet::new();
    prompt_promotion_confidence_from_relative_rewards(
        observations
            .filter(|observation| observation.is_scientific_evidence())
            .filter_map(|observation| {
                seen.insert(observation.evidence_identity())
                    .then_some(observation.relative_reward)
                    .flatten()
            }),
    )
}

pub fn prompt_promotion_confidence_from_relative_rewards(
    rewards: impl Iterator<Item = f64>,
) -> PromptPromotionConfidence {
    let mut wins = 0usize;
    let mut losses = 0usize;
    let mut ties = 0usize;
    for reward in rewards {
        if reward > 0.02 {
            wins += 1;
        } else if reward < -0.02 {
            losses += 1;
        } else {
            ties += 1;
        }
    }
    let comparisons = wins + losses + ties;
    if comparisons == 0 {
        return PromptPromotionConfidence {
            comparisons,
            wins,
            losses,
            ties,
            observed_win_rate: 0.0,
            wilson_lower_bound: 0.0,
        };
    }
    let n = comparisons as f64;
    let successes = wins as f64 + ties as f64 * 0.5;
    let rate = successes / n;
    let z2 = PROMOTION_WILSON_Z * PROMOTION_WILSON_Z;
    let denominator = 1.0 + z2 / n;
    let center = rate + z2 / (2.0 * n);
    let margin = PROMOTION_WILSON_Z * ((rate * (1.0 - rate) + z2 / (4.0 * n)) / n).sqrt();
    PromptPromotionConfidence {
        comparisons,
        wins,
        losses,
        ties,
        observed_win_rate: rate,
        wilson_lower_bound: ((center - margin) / denominator).clamp(0.0, 1.0),
    }
}

pub fn prompt_proposal_minibatch_decision(
    proposal: &ConductorPromptGenome,
    observations: &[PromptEvolutionObservation],
    minimum_comparisons: usize,
    minimum_relative_improvement: f64,
) -> Result<PromptProposalMinibatchDecision, String> {
    proposal.validate()?;
    if proposal.parents.is_empty() {
        return Ok(PromptProposalMinibatchDecision::NotRequired);
    }
    if minimum_comparisons == 0 {
        return Err("prompt proposal minibatch requires at least one comparison".to_string());
    }
    if !minimum_relative_improvement.is_finite()
        || !(-1.0..=1.0).contains(&minimum_relative_improvement)
    {
        return Err("prompt proposal minibatch improvement must be finite and bounded".to_string());
    }

    let parent_ids = proposal
        .parents
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let active_cohort_sha256 = latest_scientific_dataset_digest(observations);
    let mut parent_pairs = BTreeSet::new();
    for observation in observations.iter().filter(|observation| {
        parent_ids.contains(observation.profile_id.as_str())
            && observation.opponent_profile_id.as_deref() == Some(proposal.id.as_str())
            && observation.split == PromptEvaluationSplit::Train
            && observation.mode == PromptEvaluationMode::PairedExecution
            && observation.is_scientific_evidence()
            && active_cohort_sha256
                .is_some_and(|digest| observation.scientific_cohort_sha256() == Some(digest))
    }) {
        parent_pairs.insert((
            observation.evaluation_id.as_str(),
            observation.case_id.as_str(),
            observation.profile_id.as_str(),
        ));
    }

    let mut seen = BTreeSet::new();
    let mut paired = observations
        .iter()
        .filter(|observation| {
            observation.profile_id == proposal.id
                && observation
                    .opponent_profile_id
                    .as_deref()
                    .is_some_and(|opponent| parent_ids.contains(opponent))
                && observation.split == PromptEvaluationSplit::Train
                && observation.mode == PromptEvaluationMode::PairedExecution
                && observation.is_scientific_evidence()
                && active_cohort_sha256
                    .is_some_and(|digest| observation.scientific_cohort_sha256() == Some(digest))
        })
        .filter(|observation| {
            let Some(parent_id) = observation.opponent_profile_id.as_deref() else {
                return false;
            };
            parent_pairs.contains(&(
                observation.evaluation_id.as_str(),
                observation.case_id.as_str(),
                parent_id,
            ))
        })
        .filter(|observation| seen.insert(observation.evidence_identity()))
        .collect::<Vec<_>>();
    paired.sort_by(|left, right| {
        left.evaluation_id
            .cmp(&right.evaluation_id)
            .then_with(|| left.case_id.cmp(&right.case_id))
    });

    if paired
        .iter()
        .any(|observation| !observation.format_valid || observation.safety_violations > 0)
    {
        return Ok(PromptProposalMinibatchDecision::Rejected {
            comparisons: paired.len(),
            wins: 0,
            losses: 0,
            average_relative_reward: -1.0,
            reason: "format_or_safety_failure".to_string(),
        });
    }
    if paired.len() < minimum_comparisons {
        return Ok(PromptProposalMinibatchDecision::Pending {
            comparisons: paired.len(),
            required: minimum_comparisons,
        });
    }

    let paired = &paired[..minimum_comparisons];
    let rewards = paired
        .iter()
        .map(|observation| observation.group_relative_reward())
        .collect::<Vec<_>>();
    let wins = rewards.iter().filter(|reward| **reward > 0.02).count();
    let losses = rewards.iter().filter(|reward| **reward < -0.02).count();
    let average_relative_reward = rewards.iter().sum::<f64>() / rewards.len() as f64;
    if average_relative_reward > minimum_relative_improvement && wins > losses {
        Ok(PromptProposalMinibatchDecision::Accepted {
            comparisons: rewards.len(),
            wins,
            losses,
            average_relative_reward,
        })
    } else {
        Ok(PromptProposalMinibatchDecision::Rejected {
            comparisons: rewards.len(),
            wins,
            losses,
            average_relative_reward,
            reason: "no_measured_minibatch_improvement".to_string(),
        })
    }
}

pub fn evaluate_prompt_convergence(
    genomes: &[ConductorPromptGenome],
    observations: &[PromptEvolutionObservation],
    minimum_train_runs: usize,
    minimum_holdout_runs: usize,
    patience: usize,
    minimum_improvement: f64,
    maximum_generation: u32,
) -> Result<PromptEvolutionConvergence, String> {
    let search_archive = PromptSearchArchive::build(genomes, observations, minimum_train_runs)?;
    let champion = if let Some(nominee) = search_archive.champion() {
        let nominee_observations = observations
            .iter()
            .filter(|observation| observation.profile_id == nominee.genome.id)
            .cloned()
            .collect::<Vec<_>>();
        PromptParetoArchive::build(
            std::slice::from_ref(&nominee.genome),
            &nominee_observations,
            minimum_train_runs,
            minimum_holdout_runs,
        )?
        .champion()
        .cloned()
    } else {
        None
    };
    let mut generation_scores = Vec::new();
    let generations = genomes
        .iter()
        .map(|genome| genome.generation)
        .collect::<BTreeSet<_>>();
    for generation in generations {
        let generation_genomes = genomes
            .iter()
            .filter(|genome| genome.generation == generation)
            .cloned()
            .collect::<Vec<_>>();
        let generation_ids = generation_genomes
            .iter()
            .map(|genome| genome.id.as_str())
            .collect::<BTreeSet<_>>();
        let generation_observations = observations
            .iter()
            .filter(|observation| generation_ids.contains(observation.profile_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let archive = PromptSearchArchive::build(
            &generation_genomes,
            &generation_observations,
            minimum_train_runs,
        )?;
        if !archive.candidates.is_empty() {
            generation_scores.push((generation, archive.frontier_score()));
        }
    }

    let mut best_score = 0.0f64;
    let mut stagnant_generations = 0usize;
    let mut has_best = false;
    for (_, score) in &generation_scores {
        if !has_best || *score >= best_score + minimum_improvement {
            best_score = best_score.max(*score);
            stagnant_generations = 0;
            has_best = true;
        } else {
            stagnant_generations += 1;
        }
    }
    let latest_evaluated_generation = generation_scores
        .last()
        .map(|(generation, _)| *generation)
        .unwrap_or_default();
    let reached_generation_limit = latest_evaluated_generation >= maximum_generation;
    let converged = generation_scores.len() > patience && stagnant_generations >= patience;
    let reason = if reached_generation_limit {
        Some("generation_limit".to_string())
    } else if converged {
        Some("converged".to_string())
    } else {
        None
    };
    Ok(PromptEvolutionConvergence {
        frozen: reason.is_some() && champion.is_some(),
        reason,
        champion,
        best_score,
        stagnant_generations,
        evaluated_generations: generation_scores.len(),
    })
}

fn dominates(left: &PromptParetoCandidate, right: &PromptParetoCandidate) -> bool {
    let left = &left.holdout;
    let right = &right.holdout;
    let no_worse = left.success_rate >= right.success_rate
        && left.average_reward >= right.average_reward
        && left.average_relative_reward >= right.average_relative_reward
        && left.average_step_credit >= right.average_step_credit
        && left.average_quality >= right.average_quality
        && left.average_latency_ms <= right.average_latency_ms
        && left.safety_violations <= right.safety_violations
        && left.task_class_coverage >= right.task_class_coverage;
    let strictly_better = left.success_rate > right.success_rate
        || left.average_reward > right.average_reward
        || left.average_relative_reward > right.average_relative_reward
        || left.average_step_credit > right.average_step_credit
        || left.average_quality > right.average_quality
        || left.average_latency_ms < right.average_latency_ms
        || left.safety_violations < right.safety_violations
        || left.task_class_coverage > right.task_class_coverage;
    no_worse && strictly_better
}
