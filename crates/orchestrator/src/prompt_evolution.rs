use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PROMPT_GENOME_SCHEMA: &str = "cindx.prompt-genome.v1";
const FITNESS_WINDOW_PER_SPLIT: usize = 12;
const PROMOTION_WILSON_Z: f64 = 1.96;
const PROMOTION_MIN_LOWER_BOUND: f64 = 0.50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptGraphDepth {
    Lean,
    Balanced,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptVerification {
    Minimal,
    Evidence,
    Adversarial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptContextPolicy {
    Recent,
    Relevant,
    Comprehensive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptToolPolicy {
    Disabled,
    EvidenceOnly,
    ReadOnlyExploration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptRetryPolicy {
    FailFast,
    SameModel,
    AlternateModel,
}

fn default_prompt_tool_policy() -> PromptToolPolicy {
    PromptToolPolicy::EvidenceOnly
}

fn default_prompt_retry_policy() -> PromptRetryPolicy {
    PromptRetryPolicy::AlternateModel
}

fn default_max_step_attempts() -> usize {
    2
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConductorPromptGenome {
    pub schema: String,
    pub id: String,
    pub generation: u32,
    #[serde(default)]
    pub parents: Vec<String>,
    pub graph_depth: PromptGraphDepth,
    pub verification: PromptVerification,
    pub context_policy: PromptContextPolicy,
    pub max_parallel_branches: usize,
    #[serde(default = "default_prompt_tool_policy")]
    pub tool_policy: PromptToolPolicy,
    #[serde(default = "default_prompt_retry_policy")]
    pub retry_policy: PromptRetryPolicy,
    #[serde(default = "default_max_step_attempts")]
    pub max_step_attempts: usize,
    pub require_final_synthesis: bool,
    #[serde(default)]
    pub custom_directive: String,
}

impl ConductorPromptGenome {
    pub fn seed_for_effort(effort: &str) -> Self {
        match effort {
            "fast" => Self::seed(
                "seed-fast-v1",
                PromptGraphDepth::Lean,
                PromptVerification::Minimal,
                PromptContextPolicy::Recent,
                1,
                PromptToolPolicy::Disabled,
                PromptRetryPolicy::FailFast,
                1,
            ),
            "pro" => Self::seed(
                "seed-pro-v1",
                PromptGraphDepth::Deep,
                PromptVerification::Adversarial,
                PromptContextPolicy::Comprehensive,
                3,
                PromptToolPolicy::ReadOnlyExploration,
                PromptRetryPolicy::AlternateModel,
                3,
            ),
            _ => Self::seed(
                "seed-auto-v1",
                PromptGraphDepth::Balanced,
                PromptVerification::Evidence,
                PromptContextPolicy::Relevant,
                2,
                PromptToolPolicy::EvidenceOnly,
                PromptRetryPolicy::AlternateModel,
                2,
            ),
        }
    }

    fn seed(
        id: &str,
        graph_depth: PromptGraphDepth,
        verification: PromptVerification,
        context_policy: PromptContextPolicy,
        max_parallel_branches: usize,
        tool_policy: PromptToolPolicy,
        retry_policy: PromptRetryPolicy,
        max_step_attempts: usize,
    ) -> Self {
        Self {
            schema: PROMPT_GENOME_SCHEMA.to_string(),
            id: id.to_string(),
            generation: 0,
            parents: Vec::new(),
            graph_depth,
            verification,
            context_policy,
            max_parallel_branches,
            tool_policy,
            retry_policy,
            max_step_attempts,
            require_final_synthesis: true,
            custom_directive: String::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_GENOME_SCHEMA {
            return Err(format!("unsupported prompt genome schema: {}", self.schema));
        }
        if self.id.trim().is_empty() {
            return Err("prompt genome id is empty".to_string());
        }
        if !(1..=3).contains(&self.max_parallel_branches) {
            return Err("prompt genome branch budget must be between 1 and 3".to_string());
        }
        if !(1..=4).contains(&self.max_step_attempts) {
            return Err("prompt genome step attempts must be between 1 and 4".to_string());
        }
        if self.custom_directive.chars().count() > 1_200 {
            return Err("prompt genome custom directive exceeds 1200 characters".to_string());
        }
        Ok(())
    }

    pub fn conductor_directive(&self) -> String {
        let graph = match self.graph_depth {
            PromptGraphDepth::Lean => {
                "Prefer one direct worker. Add another step only when it removes a concrete risk."
            }
            PromptGraphDepth::Balanced => {
                "Choose the smallest graph that covers materially different approaches or verification needs."
            }
            PromptGraphDepth::Deep => {
                "Use independent specialist branches for genuinely separable uncertainty, then reconcile them."
            }
        };
        let verification = match self.verification {
            PromptVerification::Minimal => {
                "Do not add a verifier when the result is directly checkable by the executor."
            }
            PromptVerification::Evidence => {
                "Add verification when claims depend on workspace or external evidence."
            }
            PromptVerification::Adversarial => {
                "For consequential uncertainty, assign a verifier to challenge assumptions and failed branches."
            }
        };
        let context = match self.context_policy {
            PromptContextPolicy::Recent => "Use only recent context needed to resolve references.",
            PromptContextPolicy::Relevant => {
                "Use relevant context, ignoring unrelated history even when it is available."
            }
            PromptContextPolicy::Comprehensive => {
                "Preserve relevant constraints, decisions, failures, and artifacts from the full supplied memory."
            }
        };
        let tools = match self.tool_policy {
            PromptToolPolicy::Disabled => {
                "Workers must not use tools; they may only reason from supplied context."
            }
            PromptToolPolicy::EvidenceOnly => {
                "Expose read-only tools only when a step names concrete evidence it must verify."
            }
            PromptToolPolicy::ReadOnlyExploration => {
                "Allow read-only exploration for uncertain workspace or research claims, while keeping writes in the main executor."
            }
        };
        let retries = match self.retry_policy {
            PromptRetryPolicy::FailFast => {
                "Fail a broken branch immediately and preserve the failure for synthesis."
            }
            PromptRetryPolicy::SameModel => {
                "Retry a failed branch once with a corrected instruction on the same model."
            }
            PromptRetryPolicy::AlternateModel => {
                "Replan a failed branch and retry it with a different configured model."
            }
        };
        let custom = self.custom_directive.trim();
        format!(
            "Prompt profile {} (generation {}). {} {} {} {} {} Never create more than {} independent branches or more than {} attempts per step.{}",
            self.id,
            self.generation,
            graph,
            verification,
            context,
            tools,
            retries,
            self.max_parallel_branches,
            self.max_step_attempts,
            if custom.is_empty() {
                String::new()
            } else {
                format!(" Additional evolved directive: {custom}")
            }
        )
    }

    pub fn workflow_tool_policy(&self, role: &str) -> crate::WorkflowToolPolicy {
        match self.tool_policy {
            PromptToolPolicy::Disabled => crate::WorkflowToolPolicy::None,
            PromptToolPolicy::EvidenceOnly
                if matches!(role, "worker" | "verifier") =>
            {
                crate::WorkflowToolPolicy::ReadOnlyEvidence
            }
            PromptToolPolicy::EvidenceOnly => crate::WorkflowToolPolicy::None,
            PromptToolPolicy::ReadOnlyExploration if role != "synthesizer" => {
                crate::WorkflowToolPolicy::ReadOnlyExploration
            }
            PromptToolPolicy::ReadOnlyExploration => crate::WorkflowToolPolicy::None,
        }
    }

    pub fn mutation_prompt(&self, evaluation_feedback: &str) -> String {
        format!(
            concat!(
                "You are evolving a Cindx Conductor prompt genome from measured end-to-end agent outcomes. ",
                "Return one strict JSON object matching the parent schema and no commentary. ",
                "Change one or two mutable genes only: graph_depth, verification, context_policy, max_parallel_branches, tool_policy, retry_policy, max_step_attempts, or custom_directive. ",
                "Keep max_parallel_branches between 1 and 3, max_step_attempts between 1 and 4, custom_directive under 1200 characters, ",
                "and do not embed user requests, secrets, benchmark answers, or model names. Optimize the feedback while preserving generality.\n\n",
                "Parent genome:\n{}\n\nEvaluation feedback:\n{}"
            ),
            serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string()),
            evaluation_feedback
        )
    }

    pub fn learned_mutation_from_response(
        &self,
        response: &str,
        id: impl Into<String>,
    ) -> Result<Self, String> {
        let start = response
            .find('{')
            .ok_or_else(|| "prompt mutation did not return a JSON object".to_string())?;
        let end = response
            .rfind('}')
            .filter(|end| *end >= start)
            .ok_or_else(|| "prompt mutation returned incomplete JSON".to_string())?;
        let mut mutation = serde_json::from_str::<Self>(&response[start..=end])
            .map_err(|error| format!("prompt mutation JSON is invalid: {error}"))?;
        mutation.schema = PROMPT_GENOME_SCHEMA.to_string();
        mutation.id = id.into();
        mutation.generation = self.generation.saturating_add(1);
        mutation.parents = vec![self.id.clone()];
        mutation.require_final_synthesis = self.require_final_synthesis;
        mutation.validate()?;

        let changed_genes = usize::from(mutation.graph_depth != self.graph_depth)
            + usize::from(mutation.verification != self.verification)
            + usize::from(mutation.context_policy != self.context_policy)
            + usize::from(mutation.max_parallel_branches != self.max_parallel_branches)
            + usize::from(mutation.tool_policy != self.tool_policy)
            + usize::from(mutation.retry_policy != self.retry_policy)
            + usize::from(mutation.max_step_attempts != self.max_step_attempts)
            + usize::from(mutation.custom_directive.trim() != self.custom_directive.trim());
        if !(1..=2).contains(&changed_genes) {
            return Err(format!(
                "prompt mutation must change one or two genes, changed {changed_genes}"
            ));
        }
        Ok(mutation)
    }

    pub fn mutations(&self) -> Vec<Self> {
        let next_generation = self.generation.saturating_add(1);
        let mut variants = Vec::new();
        for (suffix, graph_depth) in [
            ("lean", PromptGraphDepth::Lean),
            ("balanced", PromptGraphDepth::Balanced),
            ("deep", PromptGraphDepth::Deep),
        ] {
            if graph_depth != self.graph_depth {
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.graph_depth = graph_depth;
                variants.push(variant);
            }
        }
        for (suffix, verification) in [
            ("minimal", PromptVerification::Minimal),
            ("evidence", PromptVerification::Evidence),
            ("adversarial", PromptVerification::Adversarial),
        ] {
            if verification != self.verification {
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.verification = verification;
                variants.push(variant);
            }
        }
        for (suffix, context_policy) in [
            ("recent", PromptContextPolicy::Recent),
            ("relevant", PromptContextPolicy::Relevant),
            ("comprehensive", PromptContextPolicy::Comprehensive),
        ] {
            if context_policy != self.context_policy {
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.context_policy = context_policy;
                variants.push(variant);
            }
        }
        for branches in 1..=3 {
            if branches != self.max_parallel_branches {
                let mut variant =
                    self.child(format!("{}-g{}-b{branches}", self.id, next_generation));
                variant.max_parallel_branches = branches;
                variants.push(variant);
            }
        }
        for (suffix, tool_policy) in [
            ("tools-off", PromptToolPolicy::Disabled),
            ("tools-evidence", PromptToolPolicy::EvidenceOnly),
            ("tools-explore", PromptToolPolicy::ReadOnlyExploration),
        ] {
            if tool_policy != self.tool_policy {
                let mut variant =
                    self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.tool_policy = tool_policy;
                variants.push(variant);
            }
        }
        for (suffix, retry_policy) in [
            ("retry-none", PromptRetryPolicy::FailFast),
            ("retry-same", PromptRetryPolicy::SameModel),
            ("retry-alt", PromptRetryPolicy::AlternateModel),
        ] {
            if retry_policy != self.retry_policy {
                let mut variant =
                    self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.retry_policy = retry_policy;
                variants.push(variant);
            }
        }
        for attempts in 1..=4 {
            if attempts != self.max_step_attempts {
                let mut variant =
                    self.child(format!("{}-g{}-a{attempts}", self.id, next_generation));
                variant.max_step_attempts = attempts;
                variants.push(variant);
            }
        }
        variants
    }

    pub fn crossover(id: impl Into<String>, left: &Self, right: &Self) -> Result<Self, String> {
        left.validate()?;
        right.validate()?;
        let child = Self {
            schema: PROMPT_GENOME_SCHEMA.to_string(),
            id: id.into(),
            generation: left.generation.max(right.generation).saturating_add(1),
            parents: vec![left.id.clone(), right.id.clone()],
            graph_depth: left.graph_depth,
            verification: right.verification,
            context_policy: right.context_policy,
            max_parallel_branches: left.max_parallel_branches.min(right.max_parallel_branches),
            tool_policy: right.tool_policy,
            retry_policy: left.retry_policy,
            max_step_attempts: left.max_step_attempts.min(right.max_step_attempts),
            require_final_synthesis: left.require_final_synthesis || right.require_final_synthesis,
            custom_directive: String::new(),
        };
        child.validate()?;
        Ok(child)
    }

    fn child(&self, id: String) -> Self {
        let mut child = self.clone();
        child.id = id;
        child.generation = self.generation.saturating_add(1);
        child.parents = vec![self.id.clone()];
        child.custom_directive.clear();
        child
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvaluationSplit {
    Train,
    Holdout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvaluationMode {
    Live,
    PairedShadow,
    ReplayHoldout,
    PairedExecution,
    ReplayExecution,
}

impl PromptEvaluationMode {
    pub fn is_paired(self) -> bool {
        matches!(self, Self::PairedShadow | Self::PairedExecution)
    }

    pub fn is_replay(self) -> bool {
        matches!(self, Self::ReplayHoldout | Self::ReplayExecution)
    }

    pub fn is_execution(self) -> bool {
        matches!(self, Self::PairedExecution | Self::ReplayExecution)
    }
}

fn default_evaluation_mode() -> PromptEvaluationMode {
    PromptEvaluationMode::Live
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptStepCredit {
    pub step_id: String,
    pub role: String,
    pub succeeded: bool,
    pub attempts: usize,
    pub evidence_count: usize,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub credit: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptEvolutionObservation {
    pub profile_id: String,
    #[serde(default)]
    pub evaluation_id: String,
    #[serde(default)]
    pub opponent_profile_id: Option<String>,
    pub task_class: String,
    pub split: PromptEvaluationSplit,
    #[serde(default = "default_evaluation_mode")]
    pub mode: PromptEvaluationMode,
    #[serde(default = "default_true")]
    pub format_valid: bool,
    pub succeeded: bool,
    pub quality_score: f64,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub estimated_cost_microusd: u64,
    pub safety_violations: u64,
    #[serde(default)]
    pub relative_reward: Option<f64>,
    #[serde(default)]
    pub step_credits: Vec<PromptStepCredit>,
}

fn default_true() -> bool {
    true
}

impl PromptEvolutionObservation {
    pub fn reward(&self) -> f64 {
        if !self.format_valid || self.safety_violations > 0 {
            return 0.0;
        }
        let quality = self.quality_score.clamp(0.0, 1.0);
        if self.succeeded {
            0.5 + quality * 0.5
        } else {
            quality * 0.5
        }
    }

    pub fn group_relative_reward(&self) -> f64 {
        if !self.format_valid || self.safety_violations > 0 {
            return -1.0;
        }
        self.relative_reward.unwrap_or_default().clamp(-1.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptFitness {
    pub runs: usize,
    pub paired_runs: usize,
    pub replay_runs: usize,
    pub execution_runs: usize,
    pub average_reward: f64,
    pub average_relative_reward: f64,
    pub average_step_credit: f64,
    pub format_valid_rate: f64,
    pub success_rate: f64,
    pub average_quality: f64,
    pub average_latency_ms: f64,
    pub average_total_tokens: f64,
    pub average_cost_microusd: f64,
    pub safety_violations: u64,
    pub task_class_coverage: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptPromotionConfidence {
    pub comparisons: usize,
    pub wins: usize,
    pub losses: usize,
    pub ties: usize,
    pub observed_win_rate: f64,
    pub wilson_lower_bound: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptParetoCandidate {
    pub genome: ConductorPromptGenome,
    pub train: PromptFitness,
    pub holdout: PromptFitness,
    pub quality_generalization_gap: f64,
    pub confidence: PromptPromotionConfidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptParetoArchive {
    pub candidates: Vec<PromptParetoCandidate>,
    pub rejected_profiles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptEvolutionConvergence {
    pub frozen: bool,
    pub reason: Option<String>,
    pub champion: Option<PromptParetoCandidate>,
    pub best_score: f64,
    pub stagnant_generations: usize,
    pub evaluated_generations: usize,
}

impl PromptParetoArchive {
    pub fn build(
        genomes: &[ConductorPromptGenome],
        observations: &[PromptEvolutionObservation],
        minimum_train_runs: usize,
        minimum_holdout_runs: usize,
    ) -> Result<Self, String> {
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
            }));
            let holdout = summarize(observations.iter().filter(|observation| {
                observation.profile_id == genome.id
                    && observation.split == PromptEvaluationSplit::Holdout
                    && observation.mode.is_replay()
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
            if gap > 0.15
                || holdout.format_valid_rate < 1.0
                || holdout.safety_violations > 0
            {
                rejected_profiles.push(genome.id.clone());
                continue;
            }
            let confidence = prompt_promotion_confidence(
                observations
                    .iter()
                    .filter(|observation| observation.profile_id == genome.id),
            );
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

    pub fn next_generation(&self, population_limit: usize) -> Vec<ConductorPromptGenome> {
        if population_limit == 0 {
            return Vec::new();
        }
        let mut population = self
            .candidates
            .iter()
            .map(|candidate| candidate.genome.clone())
            .collect::<Vec<_>>();
        for pair in self.candidates.windows(2) {
            if let Ok(child) = ConductorPromptGenome::crossover(
                format!(
                    "cross-g{}-{}-{}",
                    pair[0].genome.generation.max(pair[1].genome.generation) + 1,
                    pair[0].genome.id,
                    pair[1].genome.id
                ),
                &pair[0].genome,
                &pair[1].genome,
            ) {
                population.push(child);
            }
        }
        for candidate in &self.candidates {
            population.extend(candidate.genome.mutations());
        }
        let mut ids = BTreeSet::new();
        population.retain(|genome| ids.insert(genome.id.clone()));
        population.truncate(population_limit);
        population
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
    let mut wins = 0usize;
    let mut losses = 0usize;
    let mut ties = 0usize;
    for reward in observations
        .filter(|observation| observation.mode.is_execution())
        .filter_map(|observation| observation.relative_reward)
    {
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
    let margin = PROMOTION_WILSON_Z
        * ((rate * (1.0 - rate) + z2 / (4.0 * n)) / n).sqrt();
    PromptPromotionConfidence {
        comparisons,
        wins,
        losses,
        ties,
        observed_win_rate: rate,
        wilson_lower_bound: ((center - margin) / denominator).clamp(0.0, 1.0),
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
    let full_archive = PromptParetoArchive::build(
        genomes,
        observations,
        minimum_train_runs,
        minimum_holdout_runs,
    )?;
    let champion = full_archive.champion().cloned();
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
        let archive = PromptParetoArchive::build(
            &generation_genomes,
            &generation_observations,
            minimum_train_runs,
            minimum_holdout_runs,
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

fn summarize<'a>(
    observations: impl Iterator<Item = &'a PromptEvolutionObservation>,
) -> PromptFitness {
    let mut entries = observations.collect::<Vec<_>>();
    if entries.len() > FITNESS_WINDOW_PER_SPLIT {
        entries = entries.split_off(entries.len() - FITNESS_WINDOW_PER_SPLIT);
    }
    let runs = entries.len();
    let divisor = runs.max(1) as f64;
    let paired = entries
        .iter()
        .filter(|entry| entry.mode != PromptEvaluationMode::Live)
        .collect::<Vec<_>>();
    let paired_divisor = paired.len().max(1) as f64;
    let step_credits = entries
        .iter()
        .flat_map(|entry| entry.step_credits.iter())
        .map(|step| step.credit.clamp(0.0, 1.0))
        .collect::<Vec<_>>();
    PromptFitness {
        runs,
        paired_runs: paired.len(),
        replay_runs: entries
            .iter()
            .filter(|entry| entry.mode.is_replay())
            .count(),
        execution_runs: entries
            .iter()
            .filter(|entry| entry.mode.is_execution())
            .count(),
        average_reward: entries.iter().map(|entry| entry.reward()).sum::<f64>() / divisor,
        average_relative_reward: paired
            .iter()
            .map(|entry| entry.group_relative_reward())
            .sum::<f64>()
            / paired_divisor,
        average_step_credit: step_credits.iter().sum::<f64>()
            / step_credits.len().max(1) as f64,
        format_valid_rate: entries.iter().filter(|entry| entry.format_valid).count() as f64
            / divisor,
        success_rate: entries.iter().filter(|entry| entry.succeeded).count() as f64 / divisor,
        average_quality: entries
            .iter()
            .map(|entry| entry.quality_score.clamp(0.0, 1.0))
            .sum::<f64>()
            / divisor,
        average_latency_ms: entries.iter().map(|entry| entry.latency_ms).sum::<u64>() as f64
            / divisor,
        average_total_tokens: entries.iter().map(|entry| entry.total_tokens).sum::<u64>() as f64
            / divisor,
        average_cost_microusd: entries
            .iter()
            .map(|entry| entry.estimated_cost_microusd)
            .sum::<u64>() as f64
            / divisor,
        safety_violations: entries.iter().map(|entry| entry.safety_violations).sum(),
        task_class_coverage: entries
            .iter()
            .map(|entry| entry.task_class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
    }
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
        && left.average_total_tokens <= right.average_total_tokens
        && left.average_cost_microusd <= right.average_cost_microusd
        && left.safety_violations <= right.safety_violations
        && left.task_class_coverage >= right.task_class_coverage;
    let strictly_better = left.success_rate > right.success_rate
        || left.average_reward > right.average_reward
        || left.average_relative_reward > right.average_relative_reward
        || left.average_step_credit > right.average_step_credit
        || left.average_quality > right.average_quality
        || left.average_latency_ms < right.average_latency_ms
        || left.average_total_tokens < right.average_total_tokens
        || left.average_cost_microusd < right.average_cost_microusd
        || left.safety_violations < right.safety_violations
        || left.task_class_coverage > right.task_class_coverage;
    no_worse && strictly_better
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(
        profile_id: &str,
        split: PromptEvaluationSplit,
        quality: f64,
        latency_ms: u64,
        tokens: u64,
    ) -> PromptEvolutionObservation {
        PromptEvolutionObservation {
            profile_id: profile_id.to_string(),
            evaluation_id: format!("eval-{profile_id}-{latency_ms}-{tokens}"),
            opponent_profile_id: Some("baseline".to_string()),
            task_class: "coding".to_string(),
            split,
            mode: match split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedExecution,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayExecution,
            },
            format_valid: true,
            succeeded: true,
            quality_score: quality,
            latency_ms,
            total_tokens: tokens,
            estimated_cost_microusd: tokens * 2,
            safety_violations: 0,
            relative_reward: Some(quality - 0.75),
            step_credits: vec![PromptStepCredit {
                step_id: "final".to_string(),
                role: "synthesizer".to_string(),
                succeeded: true,
                attempts: 1,
                evidence_count: 1,
                latency_ms,
                total_tokens: tokens,
                credit: quality,
            }],
        }
    }

    #[test]
    fn prompt_genomes_mutate_and_cross_without_unbounded_fields() {
        let auto = ConductorPromptGenome::seed_for_effort("auto");
        let pro = ConductorPromptGenome::seed_for_effort("pro");
        let mutations = auto.mutations();
        assert!(mutations.len() >= 8);
        assert!(mutations
            .iter()
            .any(|genome| genome.context_policy != auto.context_policy));
        assert!(mutations
            .iter()
            .any(|genome| genome.tool_policy != auto.tool_policy));
        assert!(mutations
            .iter()
            .any(|genome| genome.retry_policy != auto.retry_policy));
        assert!(mutations
            .iter()
            .any(|genome| genome.max_step_attempts != auto.max_step_attempts));
        let child = ConductorPromptGenome::crossover("child", &auto, &pro).unwrap();
        assert_eq!(child.parents, vec![auto.id, pro.id]);
        assert_eq!(child.generation, 1);
        child.validate().unwrap();
    }

    #[test]
    fn legacy_genomes_receive_safe_harness_gene_defaults() {
        let legacy = serde_json::json!({
            "schema": PROMPT_GENOME_SCHEMA,
            "id": "legacy-auto",
            "generation": 0,
            "parents": [],
            "graph_depth": "balanced",
            "verification": "evidence",
            "context_policy": "relevant",
            "max_parallel_branches": 2,
            "require_final_synthesis": true,
            "custom_directive": ""
        });
        let genome: ConductorPromptGenome = serde_json::from_value(legacy).unwrap();

        assert_eq!(genome.tool_policy, PromptToolPolicy::EvidenceOnly);
        assert_eq!(genome.retry_policy, PromptRetryPolicy::AlternateModel);
        assert_eq!(genome.max_step_attempts, 2);
        genome.validate().unwrap();
    }

    #[test]
    fn learned_mutation_changes_only_bounded_genes() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let response = serde_json::json!({
            "schema": PROMPT_GENOME_SCHEMA,
            "id": "ignored",
            "generation": 99,
            "parents": [],
            "graph_depth": "deep",
            "verification": "evidence",
            "context_policy": "relevant",
            "max_parallel_branches": 2,
            "require_final_synthesis": false,
            "custom_directive": "Prefer independent hypotheses when the task is ambiguous."
        })
        .to_string();
        let mutation = parent
            .learned_mutation_from_response(&response, "learned-auto-g1")
            .unwrap();

        assert_eq!(mutation.id, "learned-auto-g1");
        assert_eq!(mutation.generation, 1);
        assert_eq!(mutation.parents, vec![parent.id]);
        assert!(mutation.require_final_synthesis);
        assert_eq!(mutation.graph_depth, PromptGraphDepth::Deep);
        assert!(!mutation.custom_directive.is_empty());
    }

    #[test]
    fn malformed_or_unsafe_workflows_receive_zero_reward() {
        let mut entry = observation("profile", PromptEvaluationSplit::Holdout, 1.0, 1_000, 1_000);
        assert_eq!(entry.reward(), 1.0);
        entry.format_valid = false;
        assert_eq!(entry.reward(), 0.0);
        entry.format_valid = true;
        entry.safety_violations = 1;
        assert_eq!(entry.reward(), 0.0);
    }

    #[test]
    fn live_only_runs_cannot_promote_without_paired_and_replay_evidence() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Holdout, 1.0, 100, 100),
        ];
        for observation in &mut observations {
            observation.mode = PromptEvaluationMode::Live;
            observation.opponent_profile_id = None;
            observation.relative_reward = None;
        }

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 1, 1)
                .unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn plan_only_comparisons_cannot_promote_without_execution_evidence() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                1.0,
                100,
                100,
            ),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                1.0,
                100,
                100,
            ),
        ];
        for observation in &mut observations {
            observation.mode = match observation.split {
                PromptEvaluationSplit::Train => PromptEvaluationMode::PairedShadow,
                PromptEvaluationSplit::Holdout => PromptEvaluationMode::ReplayHoldout,
            };
        }

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 2, 2)
                .unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn promotion_confidence_requires_more_than_two_lucky_wins() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let sparse = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 100, 100),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                1.0,
                100,
                100,
            ),
        ];
        let sparse_confidence = prompt_promotion_confidence(sparse.iter());
        assert_eq!(sparse_confidence.comparisons, 2);
        assert!(sparse_confidence.wilson_lower_bound < PROMOTION_MIN_LOWER_BOUND);

        let repeated = (0..4)
            .map(|round| {
                let split = if round % 2 == 0 {
                    PromptEvaluationSplit::Train
                } else {
                    PromptEvaluationSplit::Holdout
                };
                observation(&genome.id, split, 1.0, 100 + round, 100)
            })
            .collect::<Vec<_>>();
        let repeated_confidence = prompt_promotion_confidence(repeated.iter());
        assert_eq!(repeated_confidence.comparisons, 4);
        assert!(repeated_confidence.wilson_lower_bound >= PROMOTION_MIN_LOWER_BOUND);
    }

    #[test]
    fn same_task_relative_winner_improves_robust_score() {
        let winner = ConductorPromptGenome::seed_for_effort("auto");
        let loser = ConductorPromptGenome {
            id: "relative-loser".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let mut observations = Vec::new();
        for split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
            for round in 0..2 {
                let mut winning = observation(&winner.id, split, 0.9, 1_000, 1_000);
                winning.evaluation_id = format!("pair-{split:?}-{round}");
                winning.opponent_profile_id = Some(loser.id.clone());
                winning.relative_reward = Some(0.3);
                let mut losing = observation(&loser.id, split, 0.9, 1_000, 1_000);
                losing.evaluation_id = winning.evaluation_id.clone();
                losing.opponent_profile_id = Some(winner.id.clone());
                losing.relative_reward = Some(-0.3);
                observations.extend([winning, losing]);
            }
        }

        let archive = PromptParetoArchive::build(
            &[winner.clone(), loser],
            &observations,
            1,
            1,
        )
        .unwrap();

        assert_eq!(archive.champion().unwrap().genome.id, winner.id);
        assert!(archive.champion().unwrap().holdout.average_relative_reward > 0.0);
    }

    #[test]
    fn pareto_archive_keeps_quality_and_efficiency_tradeoffs() {
        let fast = ConductorPromptGenome::seed_for_effort("fast");
        let pro = ConductorPromptGenome::seed_for_effort("pro");
        let dominated = ConductorPromptGenome {
            id: "dominated".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let genomes = vec![fast.clone(), pro.clone(), dominated.clone()];
        let mut observations = Vec::new();
        for split in [PromptEvaluationSplit::Train, PromptEvaluationSplit::Holdout] {
            observations.push(observation(&fast.id, split, 0.82, 1_000, 1_000));
            observations.push(observation(&fast.id, split, 0.82, 1_000, 1_000));
            observations.push(observation(&pro.id, split, 0.96, 4_000, 3_000));
            observations.push(observation(&pro.id, split, 0.96, 4_000, 3_000));
            observations.push(observation(&dominated.id, split, 0.75, 5_000, 4_000));
            observations.push(observation(&dominated.id, split, 0.75, 5_000, 4_000));
        }
        let archive = PromptParetoArchive::build(&genomes, &observations, 2, 2).unwrap();
        let ids = archive
            .candidates
            .iter()
            .map(|candidate| candidate.genome.id.as_str())
            .collect::<BTreeSet<_>>();
        assert!(ids.contains(fast.id.as_str()));
        assert!(ids.contains(pro.id.as_str()));
        assert!(!ids.contains(dominated.id.as_str()));
        assert!(!archive.next_generation(8).is_empty());
    }

    #[test]
    fn holdout_gap_and_safety_block_promotion() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 1_000, 1_000),
            observation(&genome.id, PromptEvaluationSplit::Train, 1.0, 1_000, 1_000),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.5,
                1_000,
                1_000,
            ),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.5,
                1_000,
                1_000,
            ),
        ];
        observations[3].safety_violations = 1;
        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 2, 2)
                .unwrap();
        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn invalid_holdout_workflow_cannot_become_champion() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let mut observations = vec![
            observation(&genome.id, PromptEvaluationSplit::Train, 0.9, 1_000, 1_000),
            observation(&genome.id, PromptEvaluationSplit::Train, 0.9, 1_000, 1_000),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.9,
                1_000,
                1_000,
            ),
            observation(
                &genome.id,
                PromptEvaluationSplit::Holdout,
                0.9,
                1_000,
                1_000,
            ),
        ];
        observations[3].format_valid = false;

        let archive =
            PromptParetoArchive::build(std::slice::from_ref(&genome), &observations, 2, 2)
                .unwrap();

        assert!(archive.candidates.is_empty());
        assert_eq!(archive.rejected_profiles, vec![genome.id]);
    }

    #[test]
    fn recent_outcomes_automatically_roll_back_a_degraded_champion() {
        let previous = ConductorPromptGenome::seed_for_effort("auto");
        let fallback = ConductorPromptGenome {
            id: "stable-fallback".to_string(),
            ..ConductorPromptGenome::seed_for_effort("auto")
        };
        let mut observations = vec![
            observation(
                &previous.id,
                PromptEvaluationSplit::Train,
                0.4,
                1_000,
                1_000,
            ),
            observation(
                &previous.id,
                PromptEvaluationSplit::Train,
                0.4,
                1_000,
                1_000,
            ),
            observation(
                &fallback.id,
                PromptEvaluationSplit::Train,
                0.8,
                1_000,
                1_000,
            ),
            observation(
                &fallback.id,
                PromptEvaluationSplit::Train,
                0.8,
                1_000,
                1_000,
            ),
        ];
        for _ in 0..12 {
            observations.push(observation(
                &previous.id,
                PromptEvaluationSplit::Holdout,
                1.0,
                1_000,
                1_000,
            ));
        }
        for _ in 0..12 {
            observations.push(observation(
                &previous.id,
                PromptEvaluationSplit::Holdout,
                0.4,
                1_000,
                1_000,
            ));
            observations.push(observation(
                &fallback.id,
                PromptEvaluationSplit::Holdout,
                0.8,
                1_000,
                1_000,
            ));
        }

        let archive = PromptParetoArchive::build(
            &[previous, fallback.clone()],
            &observations,
            2,
            2,
        )
        .unwrap();

        assert_eq!(archive.champion().unwrap().genome.id, fallback.id);
    }

    #[test]
    fn evolution_freezes_after_stagnant_generations() {
        let seed = ConductorPromptGenome::seed_for_effort("auto");
        let mut genomes = vec![seed.clone()];
        let mut parent = seed;
        for generation in 1..=3 {
            let mut child = parent.clone();
            child.id = format!("generation-{generation}");
            child.generation = generation;
            child.parents = vec![parent.id.clone()];
            genomes.push(child.clone());
            parent = child;
        }
        let mut observations = Vec::new();
        for genome in &genomes {
            for _ in 0..2 {
                observations.push(observation(
                    &genome.id,
                    PromptEvaluationSplit::Train,
                    0.9,
                    1_000,
                    1_000,
                ));
                observations.push(observation(
                    &genome.id,
                    PromptEvaluationSplit::Holdout,
                    0.9,
                    1_000,
                    1_000,
                ));
            }
        }
        let convergence =
            evaluate_prompt_convergence(&genomes, &observations, 1, 1, 3, 0.02, 12).unwrap();

        assert!(convergence.frozen);
        assert_eq!(convergence.reason.as_deref(), Some("converged"));
        assert_eq!(convergence.stagnant_generations, 3);
    }
}
