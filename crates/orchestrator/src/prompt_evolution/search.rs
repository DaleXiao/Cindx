use super::{fitness::summarize, *};
use std::collections::BTreeSet;

impl PromptSearchArchive {
    pub fn build(
        genomes: &[ConductorPromptGenome],
        observations: &[PromptEvolutionObservation],
        minimum_train_runs: usize,
    ) -> Result<Self, String> {
        let active_dataset_sha256 = latest_scientific_training_dataset_digest(observations);
        let mut genome_ids = BTreeSet::new();
        for genome in genomes {
            genome.validate()?;
            if !genome_ids.insert(genome.id.as_str()) {
                return Err(format!("duplicated prompt genome id: {}", genome.id));
            }
        }
        if observations
            .iter()
            .any(|observation| !genome_ids.contains(observation.profile_id.as_str()))
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
                    && active_dataset_sha256
                        .is_some_and(|digest| observation.provenance.dataset_sha256 == digest)
            }));
            if train.paired_runs < minimum_train_runs
                || train.execution_runs < minimum_train_runs
                || train.format_valid_rate < 1.0
                || train.safety_violations > 0
            {
                rejected_profiles.push(genome.id.clone());
                continue;
            }
            eligible.push(PromptSearchCandidate {
                genome: genome.clone(),
                train,
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
            .collect();
        Ok(Self {
            candidates,
            rejected_profiles,
        })
    }

    pub fn next_generation(&self, population_limit: usize) -> Vec<ConductorPromptGenome> {
        next_generation_from_genomes(
            self.candidates.iter().map(|candidate| &candidate.genome),
            population_limit,
        )
    }

    pub fn champion(&self) -> Option<&PromptSearchCandidate> {
        self.candidates.iter().max_by(|left, right| {
            left.search_score()
                .total_cmp(&right.search_score())
                .then_with(|| {
                    left.train
                        .average_quality
                        .total_cmp(&right.train.average_quality)
                })
                .then_with(|| left.train.runs.cmp(&right.train.runs))
                .then_with(|| {
                    right
                        .train
                        .average_latency_ms
                        .total_cmp(&left.train.average_latency_ms)
                })
                .then_with(|| right.genome.id.cmp(&left.genome.id))
        })
    }

    pub fn frontier_score(&self) -> f64 {
        self.champion()
            .map(PromptSearchCandidate::search_score)
            .unwrap_or_default()
    }
}

impl PromptSearchCandidate {
    pub fn search_score(&self) -> f64 {
        if self.train.runs == 0
            || self.train.format_valid_rate < 1.0
            || self.train.safety_violations > 0
        {
            return 0.0;
        }
        let evidence = self.train.runs * self.train.task_class_coverage.max(1);
        let confidence_penalty = 0.08 / (evidence as f64).sqrt();
        let relative = (self.train.average_relative_reward + 1.0) * 0.5;
        (self.train.average_reward * 0.35
            + self.train.success_rate * 0.25
            + self.train.average_quality * 0.20
            + relative * 0.15
            + self.train.average_step_credit * 0.05
            - confidence_penalty)
            .clamp(0.0, 1.0)
    }
}

pub(super) fn next_generation_from_genomes<'a>(
    parents: impl Iterator<Item = &'a ConductorPromptGenome>,
    population_limit: usize,
) -> Vec<ConductorPromptGenome> {
    if population_limit == 0 {
        return Vec::new();
    }
    let parents = parents.cloned().collect::<Vec<_>>();
    let mut population = parents.clone();
    for pair in parents.windows(2) {
        if let Ok(child) = ConductorPromptGenome::crossover(
            format!(
                "cross-g{}-{}-{}",
                pair[0].generation.max(pair[1].generation) + 1,
                pair[0].id,
                pair[1].id
            ),
            &pair[0],
            &pair[1],
        ) {
            population.push(child);
        }
    }
    for parent in &parents {
        population.extend(parent.mutations());
    }
    let mut ids = BTreeSet::new();
    population.retain(|genome| ids.insert(genome.id.clone()));
    population.truncate(population_limit);
    population
}

fn dominates(left: &PromptSearchCandidate, right: &PromptSearchCandidate) -> bool {
    let left = &left.train;
    let right = &right.train;
    let no_worse = left.success_rate >= right.success_rate
        && left.average_reward >= right.average_reward
        && left.average_relative_reward >= right.average_relative_reward
        && left.average_step_credit >= right.average_step_credit
        && left.average_quality >= right.average_quality
        && left.task_class_coverage >= right.task_class_coverage;
    let strictly_better = left.success_rate > right.success_rate
        || left.average_reward > right.average_reward
        || left.average_relative_reward > right.average_relative_reward
        || left.average_step_credit > right.average_step_credit
        || left.average_quality > right.average_quality
        || left.task_class_coverage > right.task_class_coverage;
    no_worse && strictly_better
}
