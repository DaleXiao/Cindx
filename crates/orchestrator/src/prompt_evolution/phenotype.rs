use super::*;
use serde::{Deserialize, Serialize};

pub const DIRECT_FINALIZER_PHENOTYPE_SCHEMA: &str = "cindx.prompt-direct-finalizer-phenotype.v1";
pub const PROMPT_EXECUTION_INTERVENTION_SCHEMA: &str = "cindx.prompt-execution-intervention.v2";
pub const PROMPT_EXECUTION_DIAGNOSTIC_SCHEMA: &str = "cindx.prompt-execution-diagnostic.v2";
pub const PROMPT_LEARNING_READINESS_SCHEMA: &str = "cindx.prompt-learning-readiness.v2";
const ROUTE_DECISION_PHENOTYPE_SCHEMA: &str = "cindx.prompt-route-decision-phenotype.v1";
const ROUTE_DECISION_PHENOTYPE_SCHEMA_V2: &str = "cindx.prompt-route-decision-phenotype.v2";
pub const PROMPT_ROUTE_INTERVENTION_SCHEMA: &str = "cindx.prompt-route-intervention.v1";

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
pub struct PromptGenomeExecutionPhenotype {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptLearningLayer {
    RouteDecision,
    WorkflowExecution,
    DirectFinalizer,
    UntypedDirective,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptExecutionDiagnosticPlan {
    pub schema: String,
    pub execution: crate::AgentExecutionMode,
    pub execution_plan_semantic_sha256: String,
    pub route_decision_profile_sha256: String,
    pub workflow_execution_profile_sha256: Option<String>,
}

impl PromptExecutionDiagnosticPlan {
    pub fn new(
        execution: crate::AgentExecutionMode,
        execution_plan_semantic_sha256: String,
        route_decision_profile_sha256: String,
        workflow_execution_profile_sha256: Option<String>,
    ) -> Result<Self, String> {
        let diagnostic = Self {
            schema: PROMPT_EXECUTION_DIAGNOSTIC_SCHEMA.to_string(),
            execution,
            execution_plan_semantic_sha256,
            route_decision_profile_sha256,
            workflow_execution_profile_sha256,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_EXECUTION_DIAGNOSTIC_SCHEMA
            || !is_sha256(&self.execution_plan_semantic_sha256)
            || !is_sha256(&self.route_decision_profile_sha256)
        {
            return Err("prompt execution diagnostic identity is invalid".to_string());
        }
        match self.execution {
            crate::AgentExecutionMode::Direct
                if self.workflow_execution_profile_sha256.is_some() =>
            {
                Err("direct diagnostic plan must not claim a workflow profile".to_string())
            }
            crate::AgentExecutionMode::Workflow
                if !self
                    .workflow_execution_profile_sha256
                    .as_deref()
                    .is_some_and(is_sha256) =>
            {
                Err("workflow diagnostic plan is missing its profile identity".to_string())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptMutationClassification {
    pub changed_genes: Vec<String>,
    pub layers: std::collections::BTreeSet<PromptLearningLayer>,
}

impl PromptMutationClassification {
    pub fn workflow_campaign_eligible(&self) -> bool {
        !self.changed_genes.is_empty()
            && self.layers.len() == 1
            && self
                .layers
                .contains(&PromptLearningLayer::WorkflowExecution)
    }

    pub fn route_campaign_eligible(&self) -> bool {
        self.changed_genes == ["route_directive"]
            && self.layers.len() == 1
            && self.layers.contains(&PromptLearningLayer::RouteDecision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptExecutionInterventionReceipt {
    pub schema: String,
    pub layer: PromptLearningLayer,
    pub coupled_layers: std::collections::BTreeSet<PromptLearningLayer>,
    pub changed_genes: Vec<String>,
    pub diagnostic_plans: usize,
    pub active_plans: usize,
    pub mismatched_route_profile_plans: usize,
    pub mismatched_profile_plans: usize,
    pub changed_plans: usize,
    pub dormant_plans: usize,
    pub parent_execution_profile_sha256: String,
    pub candidate_execution_profile_sha256: String,
    pub parent_route_profile_sha256: String,
    pub candidate_route_profile_sha256: String,
    pub eligible: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptLearningReadinessReceipt {
    pub schema: String,
    pub layer: PromptLearningLayer,
    pub diagnostic_plans: usize,
    pub active_plans: usize,
    pub mismatched_route_profile_plans: usize,
    pub mismatched_profile_plans: usize,
    pub eligible: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptRouteInterventionReceipt {
    pub schema: String,
    pub changed_genes: Vec<String>,
    pub parent_route_profile_sha256: String,
    pub candidate_route_profile_sha256: String,
    pub parent_workflow_profile_sha256: String,
    pub candidate_workflow_profile_sha256: String,
    pub eligible: bool,
    pub reason: String,
}

#[derive(Serialize)]
struct RouteDecisionPromptPhenotype<'a> {
    schema: &'static str,
    effort: &'a str,
    workflow: PromptGenomeExecutionPhenotype,
}

#[derive(Serialize)]
struct RouteDecisionPromptPhenotypeV2<'a> {
    schema: &'static str,
    effort: &'a str,
    route_directive: &'a str,
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

    pub fn workflow_execution_profile_sha256(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_vec(&self.execution_phenotype())
            .map(|encoded| crate::sha256_hex(&encoded))
            .map_err(|error| format!("workflow execution phenotype serialization failed: {error}"))
    }

    pub fn route_decision_directive(&self, effort: &str) -> Result<Option<String>, String> {
        let effort = normalized_route_effort(effort)?;
        self.validate()?;
        if let Some(directive) = self.route_directive.as_deref() {
            let directive = directive.trim();
            return Ok((!directive.is_empty())
                .then(|| format!("Learned collaboration route policy. {directive}")));
        }
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
        if let Some(directive) = self.route_directive.as_deref() {
            return serde_json::to_vec(&RouteDecisionPromptPhenotypeV2 {
                schema: ROUTE_DECISION_PHENOTYPE_SCHEMA_V2,
                effort,
                route_directive: directive.trim(),
            })
            .map(|encoded| crate::sha256_hex(&encoded))
            .map_err(|error| format!("route decision phenotype serialization failed: {error}"));
        }
        let mut seed = Self::seed_for_effort(effort);
        seed.route_directive = None;
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

pub fn classify_prompt_mutation(
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
) -> PromptMutationClassification {
    let mut changed_genes = Vec::new();
    let mut layers = std::collections::BTreeSet::new();
    macro_rules! workflow_changed {
        ($field:ident) => {
            if candidate.$field != parent.$field {
                changed_genes.push(stringify!($field).to_string());
                layers.insert(PromptLearningLayer::WorkflowExecution);
            }
        };
    }
    workflow_changed!(graph_depth);
    workflow_changed!(verification);
    workflow_changed!(context_policy);
    workflow_changed!(max_parallel_branches);
    workflow_changed!(tool_policy);
    workflow_changed!(retry_policy);
    workflow_changed!(topology_strategy);
    workflow_changed!(role_strategy);
    workflow_changed!(commit_strategy);
    workflow_changed!(max_step_attempts);
    let mut budget_comparison = candidate.clone();
    budget_comparison.tool_policy = parent.tool_policy;
    if budget_comparison.effective_max_model_turns_per_step()
        != parent.effective_max_model_turns_per_step()
    {
        changed_genes.push("max_model_turns_per_step".to_string());
        layers.insert(PromptLearningLayer::WorkflowExecution);
    }
    if budget_comparison.effective_max_tool_calls_per_step()
        != parent.effective_max_tool_calls_per_step()
    {
        changed_genes.push("max_tool_calls_per_step".to_string());
        layers.insert(PromptLearningLayer::WorkflowExecution);
    }
    if candidate.require_final_synthesis != parent.require_final_synthesis {
        changed_genes.push("require_final_synthesis".to_string());
        layers.insert(PromptLearningLayer::WorkflowExecution);
    }
    if candidate.route_directive != parent.route_directive {
        changed_genes.push("route_directive".to_string());
        layers.insert(PromptLearningLayer::RouteDecision);
    }
    if candidate.direct_finalizer_verification != parent.direct_finalizer_verification {
        changed_genes.push("direct_finalizer_verification".to_string());
        layers.insert(PromptLearningLayer::DirectFinalizer);
    }
    if candidate.custom_directive.trim() != parent.custom_directive.trim() {
        changed_genes.push("custom_directive".to_string());
        layers.insert(PromptLearningLayer::UntypedDirective);
    }
    PromptMutationClassification {
        changed_genes,
        layers,
    }
}

pub fn assess_prompt_route_intervention(
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
    effort: &str,
) -> Result<PromptRouteInterventionReceipt, String> {
    parent.validate()?;
    candidate.validate()?;
    let classification = classify_prompt_mutation(parent, candidate);
    let parent_route_profile_sha256 = parent.route_decision_profile_sha256(effort)?;
    let candidate_route_profile_sha256 = candidate.route_decision_profile_sha256(effort)?;
    let parent_workflow_profile_sha256 = parent.workflow_execution_profile_sha256()?;
    let candidate_workflow_profile_sha256 = candidate.workflow_execution_profile_sha256()?;
    let eligible = classification.route_campaign_eligible()
        && parent_route_profile_sha256 != candidate_route_profile_sha256
        && parent_workflow_profile_sha256 == candidate_workflow_profile_sha256;
    let reason = if !classification.route_campaign_eligible() {
        "mutation_is_not_route_only"
    } else if parent_route_profile_sha256 == candidate_route_profile_sha256 {
        "mutation_does_not_change_the_route_profile"
    } else if parent_workflow_profile_sha256 != candidate_workflow_profile_sha256 {
        "route_mutation_changes_the_workflow_profile"
    } else {
        "route_profile_changes_while_workflow_execution_is_fixed"
    };
    Ok(PromptRouteInterventionReceipt {
        schema: PROMPT_ROUTE_INTERVENTION_SCHEMA.to_string(),
        changed_genes: classification.changed_genes,
        parent_route_profile_sha256,
        candidate_route_profile_sha256,
        parent_workflow_profile_sha256,
        candidate_workflow_profile_sha256,
        eligible,
        reason: reason.to_string(),
    })
}

pub fn assess_prompt_execution_intervention(
    parent: &ConductorPromptGenome,
    candidate: &ConductorPromptGenome,
    effort: &str,
    diagnostic_plans: &[PromptExecutionDiagnosticPlan],
) -> Result<PromptExecutionInterventionReceipt, String> {
    parent.validate()?;
    candidate.validate()?;
    for diagnostic in diagnostic_plans {
        diagnostic.validate()?;
    }
    let classification = classify_prompt_mutation(parent, candidate);
    let parent_execution_profile_sha256 = parent.workflow_execution_profile_sha256()?;
    let candidate_execution_profile_sha256 = candidate.workflow_execution_profile_sha256()?;
    let parent_route_profile_sha256 = parent.route_decision_profile_sha256(effort)?;
    let candidate_route_profile_sha256 = candidate.route_decision_profile_sha256(effort)?;
    let active_plans = diagnostic_plans
        .iter()
        .filter(|diagnostic| diagnostic.execution == crate::AgentExecutionMode::Workflow)
        .count();
    let mismatched_profile_plans = diagnostic_plans
        .iter()
        .filter(|diagnostic| {
            diagnostic.execution == crate::AgentExecutionMode::Workflow
                && diagnostic.workflow_execution_profile_sha256.as_deref()
                    != Some(parent_execution_profile_sha256.as_str())
        })
        .count();
    let mismatched_route_profile_plans = diagnostic_plans
        .iter()
        .filter(|diagnostic| {
            diagnostic.route_decision_profile_sha256 != parent_route_profile_sha256
        })
        .count();
    let changed_plans = if classification.workflow_campaign_eligible()
        && parent_execution_profile_sha256 != candidate_execution_profile_sha256
        && parent_route_profile_sha256 == candidate_route_profile_sha256
        && mismatched_profile_plans == 0
        && mismatched_route_profile_plans == 0
    {
        active_plans
    } else {
        0
    };
    let eligible = changed_plans > 0;
    let reason = if diagnostic_plans.is_empty() {
        "no_frozen_diagnostic_plans"
    } else if !classification.workflow_campaign_eligible() {
        "mutation_crosses_or_uses_an_untyped_learning_layer"
    } else if active_plans == 0 {
        "workflow_genes_are_dormant_for_all_frozen_plans"
    } else if mismatched_profile_plans > 0 {
        "frozen_plans_do_not_exercise_the_parent_workflow_profile"
    } else if mismatched_route_profile_plans > 0 {
        "frozen_plans_do_not_exercise_the_parent_route_profile"
    } else if parent_execution_profile_sha256 == candidate_execution_profile_sha256 {
        "mutation_does_not_change_the_executable_workflow_profile"
    } else if parent_route_profile_sha256 != candidate_route_profile_sha256 {
        "workflow_mutation_changes_the_route_decision_profile"
    } else {
        "candidate_changes_at_least_one_frozen_workflow_plan_while_route_policy_is_fixed"
    };
    Ok(PromptExecutionInterventionReceipt {
        schema: PROMPT_EXECUTION_INTERVENTION_SCHEMA.to_string(),
        layer: PromptLearningLayer::WorkflowExecution,
        coupled_layers: classification.layers,
        changed_genes: classification.changed_genes,
        diagnostic_plans: diagnostic_plans.len(),
        active_plans,
        mismatched_route_profile_plans,
        mismatched_profile_plans,
        changed_plans,
        dormant_plans: diagnostic_plans.len().saturating_sub(active_plans),
        parent_execution_profile_sha256,
        candidate_execution_profile_sha256,
        parent_route_profile_sha256,
        candidate_route_profile_sha256,
        eligible,
        reason: reason.to_string(),
    })
}

pub fn assess_prompt_learning_readiness(
    layer: PromptLearningLayer,
    expected_route_profile_sha256: Option<&str>,
    expected_workflow_profile_sha256: Option<&str>,
    diagnostic_plans: &[PromptExecutionDiagnosticPlan],
) -> Result<PromptLearningReadinessReceipt, String> {
    for diagnostic in diagnostic_plans {
        diagnostic.validate()?;
    }
    let active_plans = match layer {
        PromptLearningLayer::RouteDecision => diagnostic_plans.len(),
        PromptLearningLayer::WorkflowExecution => diagnostic_plans
            .iter()
            .filter(|diagnostic| diagnostic.execution == crate::AgentExecutionMode::Workflow)
            .count(),
        PromptLearningLayer::DirectFinalizer => diagnostic_plans.len(),
        PromptLearningLayer::UntypedDirective => 0,
    };
    let expected_route_profile_sha256 = match layer {
        PromptLearningLayer::RouteDecision | PromptLearningLayer::WorkflowExecution => {
            let expected = expected_route_profile_sha256.ok_or_else(|| {
                "route-coupled learning readiness requires the parent route profile identity"
                    .to_string()
            })?;
            if !is_sha256(expected) {
                return Err("learning parent route profile identity is invalid".to_string());
            }
            Some(expected)
        }
        _ => None,
    };
    let expected_workflow_profile_sha256 = match layer {
        PromptLearningLayer::WorkflowExecution => {
            let expected = expected_workflow_profile_sha256.ok_or_else(|| {
                "workflow learning readiness requires the parent profile identity".to_string()
            })?;
            if !is_sha256(expected) {
                return Err("workflow learning parent profile identity is invalid".to_string());
            }
            Some(expected)
        }
        _ => None,
    };
    let mismatched_profile_plans = expected_workflow_profile_sha256
        .map(|expected| {
            diagnostic_plans
                .iter()
                .filter(|diagnostic| {
                    diagnostic.execution == crate::AgentExecutionMode::Workflow
                        && diagnostic.workflow_execution_profile_sha256.as_deref() != Some(expected)
                })
                .count()
        })
        .unwrap_or(0);
    let mismatched_route_profile_plans = expected_route_profile_sha256
        .map(|expected| {
            diagnostic_plans
                .iter()
                .filter(|diagnostic| diagnostic.route_decision_profile_sha256 != expected)
                .count()
        })
        .unwrap_or(0);
    let eligible = !diagnostic_plans.is_empty()
        && active_plans > 0
        && mismatched_profile_plans == 0
        && mismatched_route_profile_plans == 0;
    let reason = if diagnostic_plans.is_empty() {
        "no_frozen_diagnostic_plans"
    } else if layer == PromptLearningLayer::UntypedDirective {
        "untyped_directive_learning_is_not_causally_admissible"
    } else if active_plans == 0 {
        "learning_layer_is_dormant_for_all_frozen_plans"
    } else if mismatched_profile_plans > 0 {
        "frozen_plans_do_not_exercise_the_expected_learning_profile"
    } else if mismatched_route_profile_plans > 0 {
        "frozen_plans_do_not_exercise_the_expected_route_profile"
    } else {
        "learning_layer_is_active_in_frozen_plans"
    };
    Ok(PromptLearningReadinessReceipt {
        schema: PROMPT_LEARNING_READINESS_SCHEMA.to_string(),
        layer,
        diagnostic_plans: diagnostic_plans.len(),
        active_plans,
        mismatched_route_profile_plans,
        mismatched_profile_plans,
        eligible,
        reason: reason.to_string(),
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
    fn explicit_route_identity_is_orthogonal_to_workflow_behavior() {
        let seed = ConductorPromptGenome::seed_for_effort("auto");
        let seed_route = seed.route_decision_profile_sha256("auto").unwrap();
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
        assert_eq!(workflow.route_decision_directive("auto").unwrap(), None);
        assert_eq!(
            workflow.route_decision_profile_sha256("auto").unwrap(),
            seed_route
        );

        workflow.route_directive = Some(
            "Use Workflow only when matched evidence predicts a verified quality gain.".to_string(),
        );
        let directive = workflow.route_decision_directive("auto").unwrap().unwrap();
        assert!(directive.contains("matched evidence"));
        assert_ne!(
            workflow.route_decision_profile_sha256("auto").unwrap(),
            seed_route
        );
    }
}

#[cfg(test)]
mod intervention_tests {
    use super::*;

    fn direct_diagnostic() -> PromptExecutionDiagnosticPlan {
        PromptExecutionDiagnosticPlan::new(
            crate::AgentExecutionMode::Direct,
            "1".repeat(64),
            "3".repeat(64),
            None,
        )
        .unwrap()
    }

    fn workflow_diagnostic(
        route_profile_sha256: &str,
        workflow_profile_sha256: &str,
    ) -> PromptExecutionDiagnosticPlan {
        PromptExecutionDiagnosticPlan::new(
            crate::AgentExecutionMode::Workflow,
            "2".repeat(64),
            route_profile_sha256.to_string(),
            Some(workflow_profile_sha256.to_string()),
        )
        .unwrap()
    }

    fn workflow_candidate(parent: &ConductorPromptGenome) -> ConductorPromptGenome {
        let mut candidate = parent.clone();
        candidate.graph_depth = match parent.graph_depth {
            PromptGraphDepth::Lean => PromptGraphDepth::Balanced,
            PromptGraphDepth::Balanced | PromptGraphDepth::Deep => PromptGraphDepth::Lean,
        };
        candidate
    }

    #[test]
    fn direct_frozen_plans_make_workflow_learning_dormant() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let parent_workflow_profile = parent.workflow_execution_profile_sha256().unwrap();
        let parent_route_profile = parent.route_decision_profile_sha256("pro").unwrap();
        let diagnostics = vec![direct_diagnostic(), direct_diagnostic()];

        let readiness = assess_prompt_learning_readiness(
            PromptLearningLayer::WorkflowExecution,
            Some(&parent_route_profile),
            Some(&parent_workflow_profile),
            &diagnostics,
        )
        .unwrap();
        let intervention = assess_prompt_execution_intervention(
            &parent,
            &workflow_candidate(&parent),
            "pro",
            &diagnostics,
        )
        .unwrap();

        assert!(!readiness.eligible);
        assert_eq!(readiness.active_plans, 0);
        assert!(!intervention.eligible);
        assert_eq!(intervention.changed_plans, 0);
        assert_eq!(
            intervention.reason,
            "workflow_genes_are_dormant_for_all_frozen_plans"
        );
    }

    #[test]
    fn matching_workflow_plan_proves_a_candidate_semantic_intervention() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let parent_workflow_profile = parent.workflow_execution_profile_sha256().unwrap();
        let parent_route_profile = parent.route_decision_profile_sha256("pro").unwrap();
        let diagnostics = vec![
            PromptExecutionDiagnosticPlan::new(
                crate::AgentExecutionMode::Direct,
                "1".repeat(64),
                parent_route_profile.clone(),
                None,
            )
            .unwrap(),
            workflow_diagnostic(&parent_route_profile, &parent_workflow_profile),
        ];

        let intervention = assess_prompt_execution_intervention(
            &parent,
            &workflow_candidate(&parent),
            "pro",
            &diagnostics,
        )
        .unwrap();

        assert!(intervention.eligible);
        assert_eq!(intervention.active_plans, 1);
        assert_eq!(intervention.changed_plans, 1);
        assert_eq!(intervention.dormant_plans, 1);
        assert_eq!(intervention.mismatched_profile_plans, 0);
        assert_eq!(intervention.mismatched_route_profile_plans, 0);
        assert_eq!(intervention.coupled_layers.len(), 1);
        assert!(intervention
            .coupled_layers
            .contains(&PromptLearningLayer::WorkflowExecution));
    }

    #[test]
    fn route_intervention_changes_only_route_identity() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let mut candidate = parent.clone();
        candidate.route_directive = Some(
            "Prefer Direct unless matched evidence shows independent verification improves correctness."
                .to_string(),
        );

        let receipt = assess_prompt_route_intervention(&parent, &candidate, "pro").unwrap();

        assert!(receipt.eligible);
        assert_eq!(receipt.changed_genes, ["route_directive"]);
        assert_ne!(
            receipt.parent_route_profile_sha256,
            receipt.candidate_route_profile_sha256
        );
        assert_eq!(
            receipt.parent_workflow_profile_sha256,
            receipt.candidate_workflow_profile_sha256
        );
    }

    #[test]
    fn foreign_workflow_profile_fails_closed_instead_of_claiming_causality() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let parent_workflow_profile = parent.workflow_execution_profile_sha256().unwrap();
        let parent_route_profile = parent.route_decision_profile_sha256("pro").unwrap();
        let diagnostics = vec![workflow_diagnostic(&parent_route_profile, &"f".repeat(64))];

        let readiness = assess_prompt_learning_readiness(
            PromptLearningLayer::WorkflowExecution,
            Some(&parent_route_profile),
            Some(&parent_workflow_profile),
            &diagnostics,
        )
        .unwrap();
        let intervention = assess_prompt_execution_intervention(
            &parent,
            &workflow_candidate(&parent),
            "pro",
            &diagnostics,
        )
        .unwrap();

        assert!(!readiness.eligible);
        assert_eq!(readiness.mismatched_profile_plans, 1);
        assert!(!intervention.eligible);
        assert_eq!(intervention.mismatched_profile_plans, 1);
    }

    #[test]
    fn untyped_directive_cannot_enter_the_workflow_learning_campaign() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let parent_workflow_profile = parent.workflow_execution_profile_sha256().unwrap();
        let parent_route_profile = parent.route_decision_profile_sha256("pro").unwrap();
        let diagnostics = vec![workflow_diagnostic(
            &parent_route_profile,
            &parent_workflow_profile,
        )];
        let mut candidate = parent.clone();
        candidate.custom_directive = "Prefer a hidden shortcut.".to_string();

        let intervention =
            assess_prompt_execution_intervention(&parent, &candidate, "pro", &diagnostics).unwrap();

        assert!(!intervention.eligible);
        assert_eq!(
            intervention.reason,
            "mutation_crosses_or_uses_an_untyped_learning_layer"
        );
    }

    #[test]
    fn foreign_route_profile_fails_closed_before_candidate_evaluation() {
        let parent = ConductorPromptGenome::seed_for_effort("pro");
        let parent_workflow_profile = parent.workflow_execution_profile_sha256().unwrap();
        let parent_route_profile = parent.route_decision_profile_sha256("pro").unwrap();
        let diagnostics = vec![workflow_diagnostic(
            &"e".repeat(64),
            &parent_workflow_profile,
        )];

        let readiness = assess_prompt_learning_readiness(
            PromptLearningLayer::WorkflowExecution,
            Some(&parent_route_profile),
            Some(&parent_workflow_profile),
            &diagnostics,
        )
        .unwrap();
        let intervention = assess_prompt_execution_intervention(
            &parent,
            &workflow_candidate(&parent),
            "pro",
            &diagnostics,
        )
        .unwrap();

        assert!(!readiness.eligible);
        assert_eq!(readiness.mismatched_route_profile_plans, 1);
        assert!(!intervention.eligible);
        assert_eq!(intervention.mismatched_route_profile_plans, 1);
    }
}
