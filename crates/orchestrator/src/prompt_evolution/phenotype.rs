use super::*;
use serde::Serialize;

pub const DIRECT_FINALIZER_PHENOTYPE_SCHEMA: &str = "cindx.prompt-direct-finalizer-phenotype.v1";
const ROUTE_DECISION_PHENOTYPE_SCHEMA: &str = "cindx.prompt-route-decision-phenotype.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DirectFinalizerPromptPhenotype {
    pub schema: &'static str,
    pub verification: PromptVerification,
}

impl DirectFinalizerPromptPhenotype {
    pub fn directive(self) -> Option<&'static str> {
        match self.verification {
            PromptVerification::Evidence => None,
            PromptVerification::Minimal => Some(
                "Use a minimal final check: preserve the strongest grounded result, ensure the requested deliverable is present, and do not add new analysis or unsupported claims.",
            ),
            PromptVerification::Adversarial => Some(
                "Before committing the final answer, challenge it against the visible evidence and every user constraint. Correct contradictions, unsupported claims, omitted deliverables, and premature completion, but do not start new work, call tools, or claim evidence that is not present.",
            ),
        }
    }

    pub fn sha256(self) -> Result<String, String> {
        serde_json::to_vec(&self)
            .map(|encoded| crate::sha256_hex(&encoded))
            .map_err(|error| format!("direct finalizer phenotype serialization failed: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(super) struct PromptGenomeExecutionPhenotype {
    graph_depth: PromptGraphDepth,
    verification: PromptVerification,
    context_policy: PromptContextPolicy,
    max_parallel_branches: usize,
    tool_policy: PromptToolPolicy,
    retry_policy: PromptRetryPolicy,
    topology_strategy: PromptTopologyStrategy,
    role_strategy: PromptRoleStrategy,
    commit_strategy: PromptCommitStrategy,
    max_step_attempts: usize,
    max_model_turns_per_step: usize,
    max_tool_calls_per_step: usize,
    require_final_synthesis: bool,
    custom_directive: String,
}

#[derive(Serialize)]
struct RouteDecisionPromptPhenotype<'a> {
    schema: &'static str,
    effort: &'a str,
    workflow: PromptGenomeExecutionPhenotype,
}

impl ConductorPromptGenome {
    pub fn direct_finalizer_phenotype(&self) -> DirectFinalizerPromptPhenotype {
        DirectFinalizerPromptPhenotype {
            schema: DIRECT_FINALIZER_PHENOTYPE_SCHEMA,
            verification: self.direct_finalizer_verification,
        }
    }

    pub(super) fn normalize_behavioral_budgets(&mut self) {
        self.max_model_turns_per_step = self.effective_max_model_turns_per_step();
        self.max_tool_calls_per_step = self.effective_max_tool_calls_per_step();
    }

    pub(super) fn execution_phenotype(&self) -> PromptGenomeExecutionPhenotype {
        PromptGenomeExecutionPhenotype {
            graph_depth: self.graph_depth,
            verification: self.verification,
            context_policy: self.context_policy,
            max_parallel_branches: self.max_parallel_branches,
            tool_policy: self.tool_policy,
            retry_policy: self.retry_policy,
            topology_strategy: self.topology_strategy,
            role_strategy: self.role_strategy,
            commit_strategy: self.commit_strategy,
            max_step_attempts: self.max_step_attempts,
            max_model_turns_per_step: self.effective_max_model_turns_per_step(),
            max_tool_calls_per_step: self.effective_max_tool_calls_per_step(),
            require_final_synthesis: self.require_final_synthesis,
            custom_directive: self.custom_directive.trim().to_string(),
        }
    }

    pub fn route_decision_directive(&self, effort: &str) -> Result<Option<String>, String> {
        let effort = normalized_route_effort(effort)?;
        self.validate()?;
        let seed = Self::seed_for_effort(effort);
        if self.execution_phenotype() == seed.execution_phenotype() {
            return Ok(None);
        }
        Ok(Some(format!(
            "Learned collaboration policy for route and Task Graph construction. {}",
            self.workflow_behavior_directive()
        )))
    }

    pub fn route_decision_profile_sha256(&self, effort: &str) -> Result<String, String> {
        let effort = normalized_route_effort(effort)?;
        self.validate()?;
        let seed = Self::seed_for_effort(effort);
        let workflow = self.execution_phenotype();
        if workflow == seed.execution_phenotype() {
            return prompt_genome_sha256(&seed);
        }
        serde_json::to_vec(&RouteDecisionPromptPhenotype {
            schema: ROUTE_DECISION_PHENOTYPE_SCHEMA,
            effort,
            workflow,
        })
        .map(|encoded| crate::sha256_hex(&encoded))
        .map_err(|error| format!("route decision phenotype serialization failed: {error}"))
    }
}

fn normalized_route_effort(effort: &str) -> Result<&'static str, String> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "fast" => Ok("fast"),
        "auto" => Ok("auto"),
        "pro" => Ok("pro"),
        _ => Err("route decision prompt effort must be fast, auto, or pro".to_string()),
    }
}

#[cfg(test)]
mod direct_finalizer_tests {
    use super::*;

    #[test]
    fn legacy_genomes_keep_the_byte_stable_default_surface() {
        let genome = ConductorPromptGenome::seed_for_effort("auto");
        let encoded = serde_json::to_string(&genome).expect("seed should serialize");
        assert!(!encoded.contains("direct_finalizer_verification"));

        let decoded = serde_json::from_str::<ConductorPromptGenome>(&encoded)
            .expect("legacy-compatible seed should deserialize");
        assert_eq!(
            decoded.direct_finalizer_phenotype().verification,
            PromptVerification::Evidence
        );
        assert_eq!(decoded.direct_finalizer_phenotype().directive(), None);
    }

    #[test]
    fn direct_finalizer_is_orthogonal_to_workflow_genes() {
        let parent = ConductorPromptGenome::seed_for_effort("auto");
        let mut workflow_variant = parent.clone();
        workflow_variant.graph_depth = PromptGraphDepth::Deep;
        workflow_variant.verification = PromptVerification::Adversarial;
        workflow_variant.context_policy = PromptContextPolicy::Comprehensive;
        assert_eq!(
            parent.direct_finalizer_phenotype(),
            workflow_variant.direct_finalizer_phenotype()
        );

        let mut direct_variant = parent.clone();
        direct_variant.direct_finalizer_verification = PromptVerification::Adversarial;
        assert_ne!(
            parent.direct_finalizer_phenotype(),
            direct_variant.direct_finalizer_phenotype()
        );
        assert!(direct_variant
            .direct_finalizer_phenotype()
            .directive()
            .is_some());
    }

    #[test]
    fn route_identity_changes_only_with_workflow_behavior() {
        let seed = ConductorPromptGenome::seed_for_effort("auto");
        let seed_route = seed.route_decision_profile_sha256("auto").unwrap();
        assert_eq!(seed_route, prompt_genome_sha256(&seed).unwrap());
        assert_eq!(seed.route_decision_directive("auto").unwrap(), None);

        let mut finalizer_only = seed.clone();
        finalizer_only.id = "finalizer-only".to_string();
        finalizer_only.direct_finalizer_verification = PromptVerification::Adversarial;
        assert_eq!(
            finalizer_only
                .route_decision_profile_sha256("auto")
                .unwrap(),
            seed_route
        );
        assert_eq!(
            finalizer_only.route_decision_directive("auto").unwrap(),
            None
        );

        let mut workflow = seed;
        workflow.id = "workflow-candidate".to_string();
        workflow.graph_depth = PromptGraphDepth::Deep;
        let directive = workflow
            .route_decision_directive("auto")
            .unwrap()
            .expect("workflow mutation should affect routing");
        assert!(directive.contains("independent specialist branches"));
        assert!(!directive.contains("workflow-candidate"));
        assert_ne!(
            workflow.route_decision_profile_sha256("auto").unwrap(),
            seed_route
        );
    }
}
