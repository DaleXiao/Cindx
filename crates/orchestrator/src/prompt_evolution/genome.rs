use crate::AgentEvaluationReflectionPacket;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PROMPT_GENOME_SCHEMA: &str = "cindx.prompt-genome.v1";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptTopologyStrategy {
    Serial,
    AdaptiveDag,
    ParallelDeliberation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptRoleStrategy {
    Flexible,
    Specialists,
    DiverseSpecialists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCommitStrategy {
    Adaptive,
    Quorum,
    Exhaustive,
}

fn default_prompt_tool_policy() -> PromptToolPolicy {
    PromptToolPolicy::EvidenceOnly
}

fn default_prompt_retry_policy() -> PromptRetryPolicy {
    PromptRetryPolicy::AlternateModel
}

fn default_prompt_topology_strategy() -> PromptTopologyStrategy {
    PromptTopologyStrategy::AdaptiveDag
}

fn default_prompt_role_strategy() -> PromptRoleStrategy {
    PromptRoleStrategy::Specialists
}

fn default_prompt_commit_strategy() -> PromptCommitStrategy {
    PromptCommitStrategy::Adaptive
}

fn default_direct_finalizer_verification() -> PromptVerification {
    PromptVerification::Evidence
}

fn direct_finalizer_verification_is_default(value: &PromptVerification) -> bool {
    *value == default_direct_finalizer_verification()
}

fn default_max_step_attempts() -> usize {
    2
}

fn default_max_model_turns_per_step() -> usize {
    0
}

fn default_max_tool_calls_per_step() -> usize {
    0
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
    #[serde(default = "default_prompt_topology_strategy")]
    pub topology_strategy: PromptTopologyStrategy,
    #[serde(default = "default_prompt_role_strategy")]
    pub role_strategy: PromptRoleStrategy,
    #[serde(default = "default_prompt_commit_strategy")]
    pub commit_strategy: PromptCommitStrategy,
    #[serde(
        default = "default_direct_finalizer_verification",
        skip_serializing_if = "direct_finalizer_verification_is_default"
    )]
    pub direct_finalizer_verification: PromptVerification,
    #[serde(default = "default_max_step_attempts")]
    pub max_step_attempts: usize,
    #[serde(default = "default_max_model_turns_per_step")]
    pub max_model_turns_per_step: usize,
    #[serde(default = "default_max_tool_calls_per_step")]
    pub max_tool_calls_per_step: usize,
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
                PromptTopologyStrategy::Serial,
                PromptRoleStrategy::Flexible,
                1,
                1,
                0,
            ),
            "pro" => Self::seed(
                "seed-pro-v1",
                PromptGraphDepth::Deep,
                PromptVerification::Adversarial,
                PromptContextPolicy::Comprehensive,
                3,
                PromptToolPolicy::ReadOnlyExploration,
                PromptRetryPolicy::AlternateModel,
                PromptTopologyStrategy::AdaptiveDag,
                PromptRoleStrategy::DiverseSpecialists,
                3,
                3,
                6,
            ),
            _ => Self::seed(
                "seed-auto-v1",
                PromptGraphDepth::Balanced,
                PromptVerification::Evidence,
                PromptContextPolicy::Relevant,
                2,
                PromptToolPolicy::EvidenceOnly,
                PromptRetryPolicy::AlternateModel,
                PromptTopologyStrategy::AdaptiveDag,
                PromptRoleStrategy::Specialists,
                2,
                2,
                4,
            ),
        }
    }

    pub fn with_effort_delivery_contract(mut self, effort: &str) -> Self {
        if matches!(effort.trim().to_ascii_lowercase().as_str(), "auto" | "pro") {
            self.require_final_synthesis = true;
        }
        self
    }

    pub fn with_verification_requirement(mut self, verification_required: bool) -> Self {
        if verification_required && self.verification == PromptVerification::Minimal {
            self.verification = PromptVerification::Evidence;
        }
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn seed(
        id: &str,
        graph_depth: PromptGraphDepth,
        verification: PromptVerification,
        context_policy: PromptContextPolicy,
        max_parallel_branches: usize,
        tool_policy: PromptToolPolicy,
        retry_policy: PromptRetryPolicy,
        topology_strategy: PromptTopologyStrategy,
        role_strategy: PromptRoleStrategy,
        max_step_attempts: usize,
        max_model_turns_per_step: usize,
        max_tool_calls_per_step: usize,
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
            topology_strategy,
            role_strategy,
            commit_strategy: PromptCommitStrategy::Adaptive,
            direct_finalizer_verification: default_direct_finalizer_verification(),
            max_step_attempts,
            max_model_turns_per_step,
            max_tool_calls_per_step,
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
        if !(1..=4).contains(&self.effective_max_model_turns_per_step()) {
            return Err("prompt genome model turns must be between 1 and 4".to_string());
        }
        let tool_calls = self.effective_max_tool_calls_per_step();
        if tool_calls > 8 || (self.tool_policy != PromptToolPolicy::Disabled && tool_calls == 0) {
            return Err(
                "prompt genome tool calls must be between 1 and 8 when tools are enabled"
                    .to_string(),
            );
        }
        if self.custom_directive.chars().count() > 1_200 {
            return Err("prompt genome custom directive exceeds 1200 characters".to_string());
        }
        Ok(())
    }

    pub fn effective_max_model_turns_per_step(&self) -> usize {
        let declared_turns = if self.max_model_turns_per_step == 0 {
            self.max_step_attempts.clamp(1, 4)
        } else {
            self.max_model_turns_per_step
        };
        self.workflow_tool_ceiling()
            .effective_model_turn_budget(declared_turns)
    }

    pub fn effective_max_tool_calls_per_step(&self) -> usize {
        let declared_calls = if self.tool_policy == PromptToolPolicy::Disabled {
            0
        } else if self.max_tool_calls_per_step == 0 {
            4
        } else {
            self.max_tool_calls_per_step
        };
        self.workflow_tool_ceiling()
            .effective_tool_call_budget(declared_calls)
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
        let topology = match self.topology_strategy {
            PromptTopologyStrategy::Serial => {
                "Use one dependency chain; do not create parallel root branches."
            }
            PromptTopologyStrategy::AdaptiveDag => {
                "Choose serial or parallel dependencies from the query's actual uncertainty."
            }
            PromptTopologyStrategy::ParallelDeliberation => {
                "Start with independent root branches and reconcile them only after they produce separate work."
            }
        };
        let roles = match self.role_strategy {
            PromptRoleStrategy::Flexible => {
                "Assign the fewest useful roles; generalists are acceptable."
            }
            PromptRoleStrategy::Specialists => {
                "Give each independent root a distinct specialist subtask. Select models by capability fit and evidence; reuse is allowed."
            }
            PromptRoleStrategy::DiverseSpecialists => {
                "Use cross-functional independent roots with complementary roles and distinct subtasks. Do not force model diversity without evidence of benefit."
            }
        };
        let commit = match self.commit_strategy {
            PromptCommitStrategy::Adaptive => {
                "Use the effort execution contract's verified stopping policy."
            }
            PromptCommitStrategy::Quorum => {
                "Commit once the required quorum has produced usable independent results; preserve failed branches for synthesis without blocking the executor."
            }
            PromptCommitStrategy::Exhaustive => {
                "After quorum, give every remaining branch its bounded completion window before committing the best verified result."
            }
        };
        let custom = self.custom_directive.trim();
        format!(
            "Prompt profile {} (generation {}). {} {} {} {} {} {} {} {} Never create more than {} independent branches, {} attempts, {} model turns, or {} read-only tool calls per step.{}",
            self.id,
            self.generation,
            graph,
            verification,
            context,
            tools,
            retries,
            topology,
            roles,
            commit,
            self.max_parallel_branches,
            self.max_step_attempts,
            self.effective_max_model_turns_per_step(),
            self.effective_max_tool_calls_per_step(),
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
            PromptToolPolicy::EvidenceOnly if matches!(role, "worker" | "verifier") => {
                crate::WorkflowToolPolicy::ReadOnlyEvidence
            }
            PromptToolPolicy::EvidenceOnly => crate::WorkflowToolPolicy::None,
            PromptToolPolicy::ReadOnlyExploration if role != "synthesizer" => {
                crate::WorkflowToolPolicy::ReadOnlyExploration
            }
            PromptToolPolicy::ReadOnlyExploration => crate::WorkflowToolPolicy::None,
        }
    }

    pub fn workflow_tool_ceiling(&self) -> crate::WorkflowToolPolicy {
        match self.tool_policy {
            PromptToolPolicy::Disabled => crate::WorkflowToolPolicy::None,
            PromptToolPolicy::EvidenceOnly => crate::WorkflowToolPolicy::ReadOnlyEvidence,
            PromptToolPolicy::ReadOnlyExploration => crate::WorkflowToolPolicy::ReadOnlyExploration,
        }
    }

    pub fn mutation_prompt(&self, evaluation_feedback: &str) -> String {
        format!(
            concat!(
                "You are evolving a Cindx Conductor prompt genome from measured end-to-end agent outcomes. ",
                "Return one strict JSON object matching the parent schema and no commentary. ",
                "Change one or two mutable genes only: graph_depth, verification, context_policy, max_parallel_branches, tool_policy, retry_policy, topology_strategy, role_strategy, commit_strategy, max_step_attempts, max_model_turns_per_step, max_tool_calls_per_step, or custom_directive. ",
                "Keep max_parallel_branches between 1 and 3, max_step_attempts and max_model_turns_per_step between 1 and 4, max_tool_calls_per_step between 0 and 8, custom_directive under 1200 characters, ",
                "and use only these exact enum values: graph_depth=lean|balanced|deep, verification=minimal|evidence|adversarial, context_policy=recent|relevant|comprehensive, tool_policy=disabled|evidence_only|read_only_exploration, retry_policy=fail_fast|same_model|alternate_model, topology_strategy=serial|adaptive_dag|parallel_deliberation, role_strategy=flexible|specialists|diverse_specialists, commit_strategy=adaptive|quorum|exhaustive. ",
                "and do not embed user requests, secrets, benchmark answers, or model names. Optimize the feedback while preserving generality.\n\n",
                "Parent genome:\n{}\n\nEvaluation feedback:\n{}"
            ),
            serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string()),
            evaluation_feedback
        )
    }

    pub fn reflective_mutation_prompt(
        &self,
        trajectories: &[AgentEvaluationReflectionPacket],
    ) -> Result<String, String> {
        self.validate()?;
        if trajectories.is_empty() {
            return Err(
                "reflective mutation requires at least one feedback trajectory".to_string(),
            );
        }
        let trajectories = serde_json::to_string_pretty(trajectories)
            .map_err(|error| format!("could not serialize reflection trajectories: {error}"))?;
        Ok(format!(
            concat!(
                "You are applying GEPA-style reflective evolution to a Cindx Conductor prompt genome. ",
                "Read every full execution trajectory, including module inputs, outputs, tool results, errors, deterministic checks, and actionable side information. ",
                "When two trajectories share suite_id, case_id, and run_id but have different candidate_id values, treat them as one matched Auto/Pro comparison: learn only the general strategy contrast, never copy case content. ",
                "A trajectory with suite_id=cindx.prompt-failure-curriculum.v1 is a negative failure seed only: contrast it with the successful anchor, never treat it as a teacher, success, permission to retry, or reason to widen authority or budget. ",
                "Diagnose which parent instruction or harness gene caused each failure, preserve behavior that passed, and generalize across examples rather than memorizing answers. ",
                "Return one strict JSON object matching the parent genome schema and no commentary. ",
                "Change one or two mutable genes only: graph_depth, verification, context_policy, max_parallel_branches, tool_policy, retry_policy, topology_strategy, role_strategy, commit_strategy, max_step_attempts, max_model_turns_per_step, max_tool_calls_per_step, or custom_directive. ",
                "Keep max_parallel_branches between 1 and 3, max_step_attempts and max_model_turns_per_step between 1 and 4, max_tool_calls_per_step between 0 and 8, custom_directive under 1200 characters, ",
                "and use only these exact enum values: graph_depth=lean|balanced|deep, verification=minimal|evidence|adversarial, context_policy=recent|relevant|comprehensive, tool_policy=disabled|evidence_only|read_only_exploration, retry_policy=fail_fast|same_model|alternate_model, topology_strategy=serial|adaptive_dag|parallel_deliberation, role_strategy=flexible|specialists|diverse_specialists, commit_strategy=adaptive|quorum|exhaustive. ",
                "and never embed user requests, secrets, benchmark answers, case ids, or model names.\n\n",
                "Parent genome:\n{}\n\nFeedback trajectories:\n{}"
            ),
            serde_json::to_string_pretty(self)
                .map_err(|error| format!("could not serialize parent genome: {error}"))?,
            trajectories,
        ))
    }

    pub fn mutation_repair_prompt(&self, invalid_response: &str, error: &str) -> String {
        format!(
            concat!(
                "Repair a rejected Cindx prompt-genome mutation. Return one strict JSON object and no commentary. ",
                "Preserve the intended one-or-two-gene improvement, but correct only schema, enum, bound, identity, or changed-gene-count errors. ",
                "Use exact enum values: graph_depth=lean|balanced|deep, verification=minimal|evidence|adversarial, context_policy=recent|relevant|comprehensive, ",
                "tool_policy=disabled|evidence_only|read_only_exploration, retry_policy=fail_fast|same_model|alternate_model, topology_strategy=serial|adaptive_dag|parallel_deliberation, role_strategy=flexible|specialists|diverse_specialists, commit_strategy=adaptive|quorum|exhaustive. ",
                "Never add user data, secrets, benchmark answers, case ids, or model names.\n\n",
                "Parent genome:\n{}\n\nValidation error:\n{}\n\nRejected mutation:\n{}"
            ),
            serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string()),
            error,
            invalid_response,
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
        if mutation.direct_finalizer_verification != self.direct_finalizer_verification {
            return Err(
                "workflow prompt mutation must not change the direct finalizer gene".to_string(),
            );
        }

        // Count the genes the response explicitly changed before policy-derived
        // budget floors are materialized. A tool-policy upgrade is one gene;
        // the larger executable budget it requires is part of that phenotype,
        // not two additional model-authored mutations.
        let mut budget_comparison = mutation.clone();
        budget_comparison.tool_policy = self.tool_policy;
        let changed_genes = usize::from(mutation.graph_depth != self.graph_depth)
            + usize::from(mutation.verification != self.verification)
            + usize::from(mutation.context_policy != self.context_policy)
            + usize::from(mutation.max_parallel_branches != self.max_parallel_branches)
            + usize::from(mutation.tool_policy != self.tool_policy)
            + usize::from(mutation.retry_policy != self.retry_policy)
            + usize::from(mutation.topology_strategy != self.topology_strategy)
            + usize::from(mutation.role_strategy != self.role_strategy)
            + usize::from(mutation.commit_strategy != self.commit_strategy)
            + usize::from(mutation.max_step_attempts != self.max_step_attempts)
            + usize::from(
                budget_comparison.effective_max_model_turns_per_step()
                    != self.effective_max_model_turns_per_step(),
            )
            + usize::from(
                budget_comparison.effective_max_tool_calls_per_step()
                    != self.effective_max_tool_calls_per_step(),
            )
            + usize::from(mutation.custom_directive.trim() != self.custom_directive.trim());
        if !(1..=2).contains(&changed_genes) {
            return Err(format!(
                "prompt mutation must change one or two genes, changed {changed_genes}"
            ));
        }
        mutation.normalize_behavioral_budgets();
        mutation.validate()?;
        if mutation.execution_phenotype() == self.execution_phenotype() {
            return Err("prompt mutation must change the execution phenotype".to_string());
        }
        Ok(mutation)
    }

    pub fn learned_reflective_mutation_from_response(
        &self,
        response: &str,
        id: impl Into<String>,
        trajectories: &[AgentEvaluationReflectionPacket],
    ) -> Result<Self, String> {
        let mutation = self.learned_mutation_from_response(response, id)?;
        if mutation.custom_directive.trim() == self.custom_directive.trim() {
            return Ok(mutation);
        }
        validate_reflective_directive(&mutation.custom_directive, trajectories)?;
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
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
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
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.retry_policy = retry_policy;
                variants.push(variant);
            }
        }
        for (suffix, topology_strategy) in [
            ("topology-serial", PromptTopologyStrategy::Serial),
            ("topology-adaptive", PromptTopologyStrategy::AdaptiveDag),
            (
                "topology-parallel",
                PromptTopologyStrategy::ParallelDeliberation,
            ),
        ] {
            if topology_strategy != self.topology_strategy {
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.topology_strategy = topology_strategy;
                variants.push(variant);
            }
        }
        for (suffix, role_strategy) in [
            ("roles-flexible", PromptRoleStrategy::Flexible),
            ("roles-specialists", PromptRoleStrategy::Specialists),
            ("roles-diverse", PromptRoleStrategy::DiverseSpecialists),
        ] {
            if role_strategy != self.role_strategy {
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.role_strategy = role_strategy;
                variants.push(variant);
            }
        }
        for (suffix, commit_strategy) in [
            ("commit-adaptive", PromptCommitStrategy::Adaptive),
            ("commit-quorum", PromptCommitStrategy::Quorum),
            ("commit-exhaustive", PromptCommitStrategy::Exhaustive),
        ] {
            if commit_strategy != self.commit_strategy {
                let mut variant = self.child(format!("{}-g{}-{suffix}", self.id, next_generation));
                variant.commit_strategy = commit_strategy;
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
        for turns in 1..=4 {
            let mut variant = self.child(format!("{}-g{}-t{turns}", self.id, next_generation));
            variant.max_model_turns_per_step = turns;
            variant.normalize_behavioral_budgets();
            if variant.max_model_turns_per_step != self.effective_max_model_turns_per_step() {
                variant.id = format!(
                    "{}-g{}-t{}",
                    self.id, next_generation, variant.max_model_turns_per_step
                );
                variants.push(variant);
            }
        }
        if self.tool_policy != PromptToolPolicy::Disabled {
            for tool_calls in [1, 2, 4, 6, 8] {
                let mut variant =
                    self.child(format!("{}-g{}-tc{tool_calls}", self.id, next_generation));
                variant.max_tool_calls_per_step = tool_calls;
                variant.normalize_behavioral_budgets();
                if variant.max_tool_calls_per_step != self.effective_max_tool_calls_per_step() {
                    variant.id = format!(
                        "{}-g{}-tc{}",
                        self.id, next_generation, variant.max_tool_calls_per_step
                    );
                    variants.push(variant);
                }
            }
        }
        for variant in &mut variants {
            variant.normalize_behavioral_budgets();
        }
        let parent_phenotype = self.execution_phenotype();
        let mut unique_phenotypes = BTreeSet::new();
        variants.retain(|variant| {
            let phenotype = variant.execution_phenotype();
            phenotype != parent_phenotype && unique_phenotypes.insert(phenotype)
        });
        variants
    }

    pub fn crossover(id: impl Into<String>, left: &Self, right: &Self) -> Result<Self, String> {
        left.validate()?;
        right.validate()?;
        let mut child = Self {
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
            topology_strategy: left.topology_strategy,
            role_strategy: right.role_strategy,
            commit_strategy: right.commit_strategy,
            direct_finalizer_verification: left.direct_finalizer_verification,
            max_step_attempts: left.max_step_attempts.min(right.max_step_attempts),
            max_model_turns_per_step: left
                .effective_max_model_turns_per_step()
                .min(right.effective_max_model_turns_per_step()),
            max_tool_calls_per_step: left
                .effective_max_tool_calls_per_step()
                .min(right.effective_max_tool_calls_per_step()),
            require_final_synthesis: left.require_final_synthesis || right.require_final_synthesis,
            custom_directive: String::new(),
        };
        child.normalize_behavioral_budgets();
        child.validate()?;
        Ok(child)
    }

    pub fn system_aware_merge(
        id: impl Into<String>,
        ancestor: &Self,
        left: &Self,
        right: &Self,
    ) -> Result<Self, String> {
        ancestor.validate()?;
        left.validate()?;
        right.validate()?;
        if !left.parents.iter().any(|parent| parent == &ancestor.id)
            || !right.parents.iter().any(|parent| parent == &ancestor.id)
        {
            return Err("system-aware merge requires a shared direct ancestor".to_string());
        }
        let left_changes = left.changed_genes_from(ancestor);
        let right_changes = right.changed_genes_from(ancestor);
        if left_changes.is_empty() || right_changes.is_empty() {
            return Err("system-aware merge candidates must both change the ancestor".to_string());
        }
        if !left_changes.is_disjoint(&right_changes) {
            return Err("system-aware merge candidates changed conflicting genes".to_string());
        }
        let mut child = ancestor.clone();
        child.id = id.into();
        child.generation = left.generation.max(right.generation).saturating_add(1);
        child.parents = vec![left.id.clone(), right.id.clone()];
        for gene in &left_changes {
            child.copy_gene_from(*gene, left);
        }
        for gene in &right_changes {
            child.copy_gene_from(*gene, right);
        }
        child.normalize_behavioral_budgets();
        child.validate()?;
        Ok(child)
    }

    fn changed_genes_from(&self, ancestor: &Self) -> BTreeSet<PromptGenomeGene> {
        let mut changes = BTreeSet::new();
        if self.graph_depth != ancestor.graph_depth {
            changes.insert(PromptGenomeGene::GraphDepth);
        }
        if self.verification != ancestor.verification {
            changes.insert(PromptGenomeGene::Verification);
        }
        if self.context_policy != ancestor.context_policy {
            changes.insert(PromptGenomeGene::ContextPolicy);
        }
        if self.max_parallel_branches != ancestor.max_parallel_branches {
            changes.insert(PromptGenomeGene::MaxParallelBranches);
        }
        if self.tool_policy != ancestor.tool_policy {
            changes.insert(PromptGenomeGene::ToolPolicy);
        }
        if self.retry_policy != ancestor.retry_policy {
            changes.insert(PromptGenomeGene::RetryPolicy);
        }
        if self.topology_strategy != ancestor.topology_strategy {
            changes.insert(PromptGenomeGene::TopologyStrategy);
        }
        if self.role_strategy != ancestor.role_strategy {
            changes.insert(PromptGenomeGene::RoleStrategy);
        }
        if self.commit_strategy != ancestor.commit_strategy {
            changes.insert(PromptGenomeGene::CommitStrategy);
        }
        if self.max_step_attempts != ancestor.max_step_attempts {
            changes.insert(PromptGenomeGene::MaxStepAttempts);
        }
        if self.effective_max_model_turns_per_step()
            != ancestor.effective_max_model_turns_per_step()
        {
            changes.insert(PromptGenomeGene::MaxModelTurnsPerStep);
        }
        if self.effective_max_tool_calls_per_step() != ancestor.effective_max_tool_calls_per_step()
        {
            changes.insert(PromptGenomeGene::MaxToolCallsPerStep);
        }
        if self.custom_directive.trim() != ancestor.custom_directive.trim() {
            changes.insert(PromptGenomeGene::CustomDirective);
        }
        changes
    }

    fn copy_gene_from(&mut self, gene: PromptGenomeGene, source: &Self) {
        match gene {
            PromptGenomeGene::GraphDepth => self.graph_depth = source.graph_depth,
            PromptGenomeGene::Verification => self.verification = source.verification,
            PromptGenomeGene::ContextPolicy => self.context_policy = source.context_policy,
            PromptGenomeGene::MaxParallelBranches => {
                self.max_parallel_branches = source.max_parallel_branches;
            }
            PromptGenomeGene::ToolPolicy => self.tool_policy = source.tool_policy,
            PromptGenomeGene::RetryPolicy => self.retry_policy = source.retry_policy,
            PromptGenomeGene::TopologyStrategy => {
                self.topology_strategy = source.topology_strategy;
            }
            PromptGenomeGene::RoleStrategy => self.role_strategy = source.role_strategy,
            PromptGenomeGene::CommitStrategy => self.commit_strategy = source.commit_strategy,
            PromptGenomeGene::MaxStepAttempts => {
                self.max_step_attempts = source.max_step_attempts;
            }
            PromptGenomeGene::MaxModelTurnsPerStep => {
                self.max_model_turns_per_step = source.effective_max_model_turns_per_step();
            }
            PromptGenomeGene::MaxToolCallsPerStep => {
                self.max_tool_calls_per_step = source.effective_max_tool_calls_per_step();
            }
            PromptGenomeGene::CustomDirective => {
                self.custom_directive.clone_from(&source.custom_directive);
            }
        }
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

fn normalized_leakage_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn contains_case_specific_overlap(directive: &str, source: &str) -> bool {
    const MIN_OVERLAP_CHARS: usize = 32;
    let directive = normalized_leakage_text(directive)
        .chars()
        .collect::<Vec<_>>();
    let source = normalized_leakage_text(source).chars().collect::<Vec<_>>();
    if directive.len() < MIN_OVERLAP_CHARS || source.len() < MIN_OVERLAP_CHARS {
        return false;
    }
    let directive_windows = directive
        .windows(MIN_OVERLAP_CHARS)
        .map(|window| window.iter().collect::<String>())
        .collect::<BTreeSet<_>>();
    source
        .windows(MIN_OVERLAP_CHARS)
        .any(|window| directive_windows.contains(&window.iter().collect::<String>()))
}

fn validate_reflective_directive(
    directive: &str,
    trajectories: &[AgentEvaluationReflectionPacket],
) -> Result<(), String> {
    let normalized_directive = normalized_leakage_text(directive);
    for trajectory in trajectories {
        let mut identities = vec![
            trajectory.suite_id.as_str(),
            trajectory.case_id.as_str(),
            trajectory.run_id.as_str(),
            trajectory.candidate_id.as_str(),
            trajectory.candidate_fingerprint.as_str(),
        ];
        identities.extend(trajectory.model_fingerprints.values().map(String::as_str));
        identities.extend(trajectory.steps.iter().map(|step| step.model.as_str()));
        if identities.into_iter().any(|identity| {
            let identity = normalized_leakage_text(identity);
            identity.chars().count() >= 6 && normalized_directive.contains(&identity)
        }) {
            return Err(
                "reflective mutation must not embed case ids or participant model names"
                    .to_string(),
            );
        }

        let mut sources = vec![
            trajectory.input.as_str(),
            trajectory.final_output.as_str(),
            trajectory.actionable_feedback.summary.as_str(),
        ];
        sources.extend(
            trajectory
                .verifier
                .checks
                .iter()
                .map(|check| check.detail.as_str()),
        );
        sources.extend(
            trajectory
                .actionable_feedback
                .passed_constraints
                .iter()
                .chain(&trajectory.actionable_feedback.failed_constraints)
                .chain(&trajectory.actionable_feedback.errors)
                .chain(&trajectory.actionable_feedback.suggested_changes)
                .map(String::as_str),
        );
        for step in &trajectory.steps {
            sources.extend([step.prompt.as_str(), step.output.as_str()]);
            for tool_call in &step.tool_calls {
                sources.extend([tool_call.request.as_str(), tool_call.response.as_str()]);
            }
        }
        if sources
            .into_iter()
            .any(|source| contains_case_specific_overlap(directive, source))
        {
            return Err(
                "reflective mutation must generalize feedback instead of copying case content"
                    .to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod verification_requirement_tests {
    use super::*;

    #[test]
    fn required_verification_upgrades_minimal_to_evidence() {
        let genome =
            ConductorPromptGenome::seed_for_effort("fast").with_verification_requirement(true);

        assert_eq!(genome.verification, PromptVerification::Evidence);
    }

    #[test]
    fn optional_verification_preserves_minimal() {
        let genome =
            ConductorPromptGenome::seed_for_effort("fast").with_verification_requirement(false);

        assert_eq!(genome.verification, PromptVerification::Minimal);
    }

    #[test]
    fn required_verification_preserves_stronger_policies() {
        for effort in ["auto", "pro"] {
            let selected = ConductorPromptGenome::seed_for_effort(effort);
            let expected = selected.verification;
            let effective = selected.with_verification_requirement(true);

            assert_eq!(effective.verification, expected);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PromptGenomeGene {
    GraphDepth,
    Verification,
    ContextPolicy,
    MaxParallelBranches,
    ToolPolicy,
    RetryPolicy,
    TopologyStrategy,
    RoleStrategy,
    CommitStrategy,
    MaxStepAttempts,
    MaxModelTurnsPerStep,
    MaxToolCallsPerStep,
    CustomDirective,
}
