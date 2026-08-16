use super::{receipts::TreatmentExposureReceipt, RawRun, RealworldCase};
use orchestrator::{
    prompt_genome_sha256, AgentExecutionMode, AgentPolicy, ConductorPromptGenome,
    PromptExecutionDiagnosticPlan,
};
use serde::Serialize;
use std::collections::BTreeSet;

pub(super) const CAMPAIGN_VERSION: u32 = 7;
pub(super) const CAMPAIGN_SCHEMA: &str = "cindx.workflow-gepa-campaign.v7";
pub(super) const CAMPAIGN_SUITE_ID: &str = "cindx-workflow-gepa-v7";
pub(super) const CAMPAIGN_PROJECT_ID: &str = "project-workflow-gepa-v7";
pub(super) const TEST_REPLICATES: u32 = 2;
const VALIDATION_RESOURCE_RATIO_CEILING: f64 = 1.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CampaignSplit {
    Train,
    Validation,
    Test,
}

impl CampaignSplit {
    pub(super) fn parse(case: &RealworldCase) -> Result<Self, String> {
        match case.campaign_split.as_deref() {
            Some("train") => Ok(Self::Train),
            Some("validation") => Ok(Self::Validation),
            Some("test") => Ok(Self::Test),
            other => Err(format!(
                "Workflow GEPA case {} has invalid campaign split {:?}",
                case.id, other
            )),
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Train => "train",
            Self::Validation => "validation",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ProductRunReceipt {
    pub(super) case_id: String,
    pub(super) category: String,
    pub(super) split: CampaignSplit,
    pub(super) replicate: u32,
    pub(super) treatment: String,
    pub(super) execution_mode: String,
    #[serde(skip)]
    pub(super) execution_constraint: Option<String>,
    pub(super) completed: bool,
    pub(super) terminal_status: String,
    pub(super) behavior_checks_passed: usize,
    pub(super) behavior_checks_total: usize,
    pub(super) behavior_score: f64,
    pub(super) quality_passed: bool,
    pub(super) safety_violations: usize,
    pub(super) latency_ms: u64,
    pub(super) model_calls: usize,
    pub(super) total_tokens: u64,
    pub(super) output_sha256: String,
    pub(super) conductor_candidate_sha256: Option<String>,
    pub(super) workflow_proposal_sha256: Option<String>,
    pub(super) profile_id: Option<String>,
    pub(super) profile_sha256: Option<String>,
    pub(super) route_profile_sha256: Option<String>,
    pub(super) execution_plan_sha256: Option<String>,
    pub(super) execution_plan_semantic_sha256: Option<String>,
    pub(super) execution_plan_authority: Option<String>,
    pub(super) workflow_execution_profile_sha256: Option<String>,
    pub(super) route_profile_semantics_exercised: bool,
    pub(super) workflow_profile_exercised: bool,
    #[serde(skip)]
    pub(super) workflow_verifier_steps: usize,
    #[serde(skip)]
    pub(super) treatment_exposure: Option<TreatmentExposureReceipt>,
}

impl ProductRunReceipt {
    pub(super) fn from_run(
        run: &RawRun,
        split: CampaignSplit,
        replicate: u32,
    ) -> Result<Self, String> {
        validate_run_evidence(run)?;
        let behavior = run
            .verification
            .postcondition_receipts
            .iter()
            .filter(|receipt| receipt.kind != "immutable_fixture")
            .collect::<Vec<_>>();
        if behavior.is_empty() {
            return Err(format!(
                "real product task {} has no external behavior postcondition",
                run.case_id
            ));
        }
        let behavior_checks_passed = behavior.iter().filter(|receipt| receipt.passed).count();
        let behavior_checks_total = behavior.len();
        let behavior_score = externally_verified_behavior_score(
            run.completed,
            run.verification.safety_violations,
            behavior_checks_passed,
            behavior_checks_total,
        );
        let strategy = run.strategy_receipt.as_ref();
        Ok(Self {
            case_id: run.case_id.clone(),
            category: run.category.clone(),
            split,
            replicate,
            treatment: run.treatment.label().to_string(),
            execution_mode: strategy
                .map(|receipt| receipt.execution_mode.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            execution_constraint: strategy.map(|receipt| receipt.execution_constraint.clone()),
            completed: run.completed,
            terminal_status: run.terminal_status.clone(),
            behavior_checks_passed,
            behavior_checks_total,
            behavior_score,
            quality_passed: run.verification.quality_passed,
            safety_violations: run.verification.safety_violations,
            latency_ms: run.metrics.latency_ms,
            model_calls: run.metrics.model_calls,
            total_tokens: run.metrics.total_tokens,
            output_sha256: run.output_sha256.clone(),
            conductor_candidate_sha256: strategy
                .and_then(|receipt| receipt.conductor_candidate_sha256.clone()),
            workflow_proposal_sha256: strategy
                .and_then(|receipt| receipt.workflow_proposal_sha256.clone()),
            profile_id: strategy.map(|receipt| receipt.profile_id.clone()),
            profile_sha256: strategy.map(|receipt| receipt.profile_sha256.clone()),
            route_profile_sha256: strategy.map(|receipt| receipt.route_profile_sha256.clone()),
            execution_plan_sha256: strategy
                .and_then(|receipt| receipt.execution_plan_sha256.clone()),
            execution_plan_semantic_sha256: strategy
                .and_then(|receipt| receipt.execution_plan_semantic_sha256.clone()),
            execution_plan_authority: strategy
                .and_then(|receipt| receipt.execution_plan_authority.clone()),
            workflow_execution_profile_sha256: strategy
                .and_then(|receipt| receipt.workflow_execution_profile_sha256.clone()),
            route_profile_semantics_exercised: strategy
                .is_some_and(|receipt| receipt.route_profile_semantics_exercised),
            workflow_profile_exercised: strategy
                .is_some_and(|receipt| receipt.workflow_profile_exercised),
            workflow_verifier_steps: strategy.map_or(0, |receipt| receipt.workflow_verifier_steps),
            treatment_exposure: strategy.and_then(|receipt| receipt.treatment_exposure.clone()),
        })
    }

    pub(super) fn execution_diagnostic(&self) -> Result<PromptExecutionDiagnosticPlan, String> {
        let execution = match self.execution_mode.as_str() {
            "direct" => AgentExecutionMode::Direct,
            "workflow" => AgentExecutionMode::Workflow,
            other => {
                return Err(format!(
                    "product run {} has unsupported execution mode {other}",
                    self.case_id
                ))
            }
        };
        PromptExecutionDiagnosticPlan::new(
            execution,
            self.execution_plan_semantic_sha256.clone().ok_or_else(|| {
                format!(
                    "product run {} is missing its execution-plan semantic identity",
                    self.case_id
                )
            })?,
            self.route_profile_sha256.clone().ok_or_else(|| {
                format!(
                    "product run {} is missing its route-profile identity",
                    self.case_id
                )
            })?,
            self.workflow_execution_profile_sha256.clone(),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ProductPairReceipt {
    pub(super) evaluation_id: String,
    pub(super) case_id: String,
    pub(super) category: String,
    pub(super) split: CampaignSplit,
    pub(super) replicate: u32,
    pub(super) execution_order: String,
    pub(super) workspace_prestate_sha256: String,
    pub(super) seed: ProductRunReceipt,
    pub(super) candidate: ProductRunReceipt,
    pub(super) behavior_delta: f64,
    pub(super) outcome: String,
}

impl ProductPairReceipt {
    pub(super) fn new(
        evaluation_id: String,
        execution_order: String,
        workspace_prestate_sha256: String,
        seed: ProductRunReceipt,
        candidate: ProductRunReceipt,
    ) -> Result<Self, String> {
        if seed.case_id != candidate.case_id
            || seed.category != candidate.category
            || seed.split != candidate.split
            || seed.replicate != candidate.replicate
        {
            return Err("matched product pair identity is inconsistent".to_string());
        }
        let behavior_delta = candidate.behavior_score - seed.behavior_score;
        let outcome = if behavior_delta > f64::EPSILON
            || behavior_delta.abs() <= f64::EPSILON && candidate.completed && !seed.completed
        {
            "candidate_win"
        } else if behavior_delta < -f64::EPSILON
            || behavior_delta.abs() <= f64::EPSILON && seed.completed && !candidate.completed
        {
            "candidate_loss"
        } else {
            "tie"
        };
        Ok(Self {
            evaluation_id,
            case_id: seed.case_id.clone(),
            category: seed.category.clone(),
            split: seed.split,
            replicate: seed.replicate,
            execution_order,
            workspace_prestate_sha256,
            seed,
            candidate,
            behavior_delta,
            outcome: outcome.to_string(),
        })
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct PairAggregateReceipt {
    pub(super) pairs: usize,
    pub(super) unique_cases: usize,
    pub(super) task_classes: usize,
    pub(super) candidate_wins: usize,
    pub(super) candidate_win_cases: usize,
    pub(super) candidate_losses: usize,
    pub(super) ties: usize,
    pub(super) seed_behavior_score: f64,
    pub(super) candidate_behavior_score: f64,
    pub(super) behavior_delta: f64,
    pub(super) seed_completed: usize,
    pub(super) candidate_completed: usize,
    pub(super) seed_quality_passed: usize,
    pub(super) candidate_quality_passed: usize,
    pub(super) candidate_safety_violations: usize,
    pub(super) latency_ratio: f64,
    pub(super) token_ratio: f64,
    pub(super) causal_profile_runs: usize,
    pub(super) route_semantics_runs: usize,
    pub(super) workflow_profile_runs: usize,
    pub(super) seed_direct_runs: usize,
    pub(super) seed_workflow_runs: usize,
    pub(super) candidate_direct_runs: usize,
    pub(super) candidate_workflow_runs: usize,
    pub(super) route_changed_pairs: usize,
}

pub(super) fn aggregate_pairs(
    pairs: &[ProductPairReceipt],
    candidate: &ConductorPromptGenome,
) -> Result<PairAggregateReceipt, String> {
    if pairs.is_empty() {
        return Ok(PairAggregateReceipt::default());
    }
    let candidate_sha256 = prompt_genome_sha256(candidate)?;
    let candidate_route_sha256 =
        candidate.route_decision_profile_sha256(AgentPolicy::Pro.label())?;
    let denominator = pairs.len() as f64;
    let seed_behavior_score = pairs
        .iter()
        .map(|pair| pair.seed.behavior_score)
        .sum::<f64>()
        / denominator;
    let candidate_behavior_score = pairs
        .iter()
        .map(|pair| pair.candidate.behavior_score)
        .sum::<f64>()
        / denominator;
    let seed_latency = pairs
        .iter()
        .map(|pair| pair.seed.latency_ms as f64)
        .sum::<f64>();
    let candidate_latency = pairs
        .iter()
        .map(|pair| pair.candidate.latency_ms as f64)
        .sum::<f64>();
    let seed_tokens = pairs
        .iter()
        .map(|pair| pair.seed.total_tokens as f64)
        .sum::<f64>();
    let candidate_tokens = pairs
        .iter()
        .map(|pair| pair.candidate.total_tokens as f64)
        .sum::<f64>();
    Ok(PairAggregateReceipt {
        pairs: pairs.len(),
        unique_cases: pairs
            .iter()
            .map(|pair| pair.case_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        task_classes: pairs
            .iter()
            .map(|pair| pair.category.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        candidate_wins: pairs
            .iter()
            .filter(|pair| pair.outcome == "candidate_win")
            .count(),
        candidate_win_cases: pairs
            .iter()
            .filter(|pair| pair.outcome == "candidate_win")
            .map(|pair| pair.case_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        candidate_losses: pairs
            .iter()
            .filter(|pair| pair.outcome == "candidate_loss")
            .count(),
        ties: pairs.iter().filter(|pair| pair.outcome == "tie").count(),
        seed_behavior_score,
        candidate_behavior_score,
        behavior_delta: candidate_behavior_score - seed_behavior_score,
        seed_completed: pairs.iter().filter(|pair| pair.seed.completed).count(),
        candidate_completed: pairs.iter().filter(|pair| pair.candidate.completed).count(),
        seed_quality_passed: pairs.iter().filter(|pair| pair.seed.quality_passed).count(),
        candidate_quality_passed: pairs
            .iter()
            .filter(|pair| pair.candidate.quality_passed)
            .count(),
        candidate_safety_violations: pairs
            .iter()
            .map(|pair| pair.candidate.safety_violations)
            .sum(),
        latency_ratio: ratio(candidate_latency, seed_latency),
        token_ratio: ratio(candidate_tokens, seed_tokens),
        causal_profile_runs: pairs
            .iter()
            .filter(|pair| {
                pair.candidate.profile_id.as_deref() == Some(candidate.id.as_str())
                    && pair.candidate.profile_sha256.as_deref() == Some(candidate_sha256.as_str())
                    && pair.candidate.route_profile_sha256.as_deref()
                        == Some(candidate_route_sha256.as_str())
            })
            .count(),
        route_semantics_runs: pairs
            .iter()
            .filter(|pair| pair.candidate.route_profile_semantics_exercised)
            .count(),
        workflow_profile_runs: pairs
            .iter()
            .filter(|pair| pair.candidate.workflow_profile_exercised)
            .count(),
        seed_direct_runs: pairs
            .iter()
            .filter(|pair| pair.seed.execution_mode == "direct")
            .count(),
        seed_workflow_runs: pairs
            .iter()
            .filter(|pair| pair.seed.execution_mode == "workflow")
            .count(),
        candidate_direct_runs: pairs
            .iter()
            .filter(|pair| pair.candidate.execution_mode == "direct")
            .count(),
        candidate_workflow_runs: pairs
            .iter()
            .filter(|pair| pair.candidate.execution_mode == "workflow")
            .count(),
        route_changed_pairs: pairs
            .iter()
            .filter(|pair| pair.seed.execution_mode != pair.candidate.execution_mode)
            .count(),
    })
}

pub(super) fn validation_gate(aggregate: &PairAggregateReceipt) -> Result<(), String> {
    let quality_non_regression =
        aggregate.candidate_behavior_score + f64::EPSILON >= aggregate.seed_behavior_score;
    let resource_bounded = aggregate.latency_ratio <= VALIDATION_RESOURCE_RATIO_CEILING
        && aggregate.token_ratio <= VALIDATION_RESOURCE_RATIO_CEILING;
    let efficiency_uplift = (aggregate.latency_ratio <= 0.90 && aggregate.token_ratio <= 1.05)
        || (aggregate.token_ratio <= 0.90 && aggregate.latency_ratio <= 1.05);
    let passed = aggregate.pairs >= 2
        && aggregate.unique_cases >= 2
        && aggregate.task_classes >= 2
        && quality_non_regression
        && aggregate.candidate_losses == 0
        && aggregate.candidate_completed >= aggregate.seed_completed
        && aggregate.candidate_quality_passed == aggregate.pairs
        && aggregate.candidate_safety_violations == 0
        && aggregate.causal_profile_runs == aggregate.pairs
        && aggregate.route_semantics_runs == aggregate.pairs
        && resource_bounded
        && (aggregate.candidate_wins > 0 || efficiency_uplift);
    if passed {
        Ok(())
    } else {
        Err(format!(
            "candidate failed the validation-only admission gate: {}",
            serde_json::to_string(aggregate)
                .unwrap_or_else(|_| "aggregate unavailable".to_string())
        ))
    }
}

pub(super) fn test_gate(aggregate: &PairAggregateReceipt) -> Result<(), String> {
    let passed = aggregate.pairs >= 8
        && aggregate.unique_cases >= 4
        && aggregate.task_classes >= 2
        && aggregate.candidate_wins >= 2
        && aggregate.candidate_win_cases >= 2
        && aggregate.candidate_losses == 0
        && aggregate.behavior_delta > f64::EPSILON
        && aggregate.candidate_completed >= aggregate.seed_completed
        && aggregate.candidate_quality_passed == aggregate.pairs
        && aggregate.candidate_safety_violations == 0
        && aggregate.latency_ratio <= 1.05
        && aggregate.token_ratio <= 1.05
        && aggregate.causal_profile_runs == aggregate.pairs
        && aggregate.route_semantics_runs == aggregate.pairs;
    if passed {
        Ok(())
    } else {
        Err(format!(
            "candidate failed the untouched product-test gate: {}",
            serde_json::to_string(aggregate)
                .unwrap_or_else(|_| "aggregate unavailable".to_string())
        ))
    }
}

pub(super) fn validate_grounded_control(run: &RawRun) -> Result<(), String> {
    validate_run_evidence(run)?;
    let direct = run
        .strategy_receipt
        .as_ref()
        .is_some_and(|receipt| receipt.execution_mode == "direct");
    if run.completed
        && run.verification.quality_passed
        && run.verification.external_effect_passed == Some(true)
        && run.verification.safety_violations == 0
        && direct
    {
        Ok(())
    } else {
        Err(format!(
            "Grounded Direct control {} failed its completion/evidence gate",
            run.case_id
        ))
    }
}

fn validate_run_evidence(run: &RawRun) -> Result<(), String> {
    let has_execution_plan = run.strategy_receipt.as_ref().is_some_and(|receipt| {
        receipt.execution_plan_sha256.is_some()
            && receipt.execution_plan_semantic_sha256.is_some()
            && receipt.execution_plan_authority.is_some()
            && match receipt.execution_mode.as_str() {
                "direct" => receipt.workflow_execution_profile_sha256.is_none(),
                "workflow" => receipt.workflow_execution_profile_sha256.is_some(),
                _ => false,
            }
    });
    if run.setup_failure.is_some()
        || run.evidence_error.is_some()
        || run.verification.total_checks == 0
        || run.strategy_receipt.is_none()
        || !has_execution_plan
        || run.model_receipts.is_empty()
    {
        return Err(format!(
            "real product task {} produced invalid evaluation evidence: setup={:?}, evidence={:?}, checks={}, strategy={}, execution_plan={}, models={}",
            run.case_id,
            run.setup_failure,
            run.evidence_error,
            run.verification.total_checks,
            run.strategy_receipt.is_some(),
            has_execution_plan,
            run.model_receipts.len(),
        ));
    }
    if let Some(strategy) = run.strategy_receipt.as_ref().filter(|receipt| {
        matches!(
            receipt.execution_constraint.as_str(),
            "matched_direct" | "matched_workflow"
        )
    }) {
        let exposure = strategy.treatment_exposure.as_ref().ok_or_else(|| {
            format!(
                "real product task {} is missing matched-route treatment exposure",
                run.case_id
            )
        })?;
        if exposure.logical_model_calls != run.metrics.model_calls {
            return Err(format!(
                "real product task {} changed the matched-route model-call counting boundary",
                run.case_id
            ));
        }
    }
    Ok(())
}

fn ratio(candidate: f64, seed: f64) -> f64 {
    if seed <= f64::EPSILON {
        if candidate <= f64::EPSILON {
            1.0
        } else {
            f64::INFINITY
        }
    } else {
        candidate / seed
    }
}

fn externally_verified_behavior_score(
    completed: bool,
    safety_violations: usize,
    checks_passed: usize,
    checks_total: usize,
) -> f64 {
    if !completed || safety_violations > 0 || checks_total == 0 {
        0.0
    } else {
        checks_passed.min(checks_total) as f64 / checks_total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        case_id: &str,
        category: &str,
        score: f64,
        latency_ms: u64,
        profile: &ConductorPromptGenome,
        workflow: bool,
    ) -> ProductRunReceipt {
        ProductRunReceipt {
            case_id: case_id.to_string(),
            category: category.to_string(),
            split: CampaignSplit::Test,
            replicate: 1,
            treatment: "pro".to_string(),
            execution_mode: if workflow {
                "workflow".to_string()
            } else {
                "direct".to_string()
            },
            execution_constraint: None,
            completed: true,
            terminal_status: "completed".to_string(),
            behavior_checks_passed: usize::from(score > 0.0),
            behavior_checks_total: 1,
            behavior_score: score,
            quality_passed: score == 1.0,
            safety_violations: 0,
            latency_ms,
            model_calls: 1,
            total_tokens: 100,
            output_sha256: "a".repeat(64),
            conductor_candidate_sha256: Some("0".repeat(64)),
            workflow_proposal_sha256: Some("1".repeat(64)),
            profile_id: Some(profile.id.clone()),
            profile_sha256: Some(prompt_genome_sha256(profile).unwrap()),
            route_profile_sha256: Some(
                profile
                    .route_decision_profile_sha256(AgentPolicy::Pro.label())
                    .unwrap(),
            ),
            execution_plan_sha256: None,
            execution_plan_semantic_sha256: None,
            execution_plan_authority: None,
            workflow_execution_profile_sha256: None,
            route_profile_semantics_exercised: true,
            workflow_profile_exercised: workflow,
            workflow_verifier_steps: usize::from(workflow),
            treatment_exposure: None,
        }
    }

    #[test]
    fn behavior_score_preserves_partial_external_quality_without_rewarding_unsafe_runs() {
        assert_eq!(externally_verified_behavior_score(true, 0, 3, 4), 0.75);
        assert_eq!(externally_verified_behavior_score(false, 0, 3, 4), 0.0);
        assert_eq!(externally_verified_behavior_score(true, 1, 4, 4), 0.0);
        assert_eq!(externally_verified_behavior_score(true, 0, 1, 0), 0.0);
    }

    #[test]
    fn execution_diagnostic_binds_route_and_workflow_profiles() {
        let profile = ConductorPromptGenome::seed_for_effort("pro");
        let mut receipt = run("diagnostic", "coding", 1.0, 100, &profile, true);
        receipt.execution_plan_semantic_sha256 = Some("b".repeat(64));
        receipt.workflow_execution_profile_sha256 =
            Some(profile.workflow_execution_profile_sha256().unwrap());

        let diagnostic = receipt.execution_diagnostic().expect("diagnostic plan");
        assert_eq!(
            diagnostic.route_decision_profile_sha256,
            profile
                .route_decision_profile_sha256(AgentPolicy::Pro.label())
                .unwrap()
        );
        assert_eq!(
            diagnostic.workflow_execution_profile_sha256,
            Some(profile.workflow_execution_profile_sha256().unwrap())
        );

        receipt.route_profile_sha256 = None;
        assert!(receipt.execution_diagnostic().is_err());
    }

    #[test]
    fn test_gate_requires_quality_wins_on_multiple_unseen_cases() {
        let candidate = ConductorPromptGenome::seed_for_effort("pro")
            .mutations()
            .into_iter()
            .find(|profile| !profile.custom_directive.is_empty())
            .unwrap_or_else(|| {
                let mut profile = ConductorPromptGenome::seed_for_effort("pro");
                profile.id = "learned-test".to_string();
                profile.generation = 1;
                profile.parents = vec!["seed-pro-v1".to_string()];
                profile.custom_directive = "Verify the public contract.".to_string();
                profile
            });
        let mut pairs = Vec::new();
        for replicate in 1..=2 {
            for (index, (case_id, category)) in [
                ("a", "coding"),
                ("b", "coding"),
                ("c", "research"),
                ("d", "research"),
            ]
            .into_iter()
            .enumerate()
            {
                let mut seed = run(case_id, category, 1.0, 100, &candidate, index == 0);
                seed.profile_id = Some("seed-pro-v1".to_string());
                let candidate_score = 1.0;
                let candidate_run = run(
                    case_id,
                    category,
                    candidate_score,
                    100,
                    &candidate,
                    index == 0,
                );
                pairs.push(
                    ProductPairReceipt::new(
                        format!("{case_id}-{replicate}"),
                        "seed_then_candidate".to_string(),
                        "f".repeat(64),
                        seed,
                        candidate_run,
                    )
                    .unwrap(),
                );
            }
        }
        let aggregate = aggregate_pairs(&pairs, &candidate).unwrap();
        assert!(test_gate(&aggregate).is_err());

        for pair in pairs
            .iter_mut()
            .filter(|pair| pair.replicate == 1 && matches!(pair.case_id.as_str(), "a" | "b"))
        {
            pair.seed.behavior_score = 0.0;
            pair.behavior_delta = 1.0;
            pair.outcome = "candidate_win".to_string();
        }
        let aggregate = aggregate_pairs(&pairs, &candidate).unwrap();
        test_gate(&aggregate).unwrap();

        pairs[0].candidate.quality_passed = false;
        let aggregate = aggregate_pairs(&pairs, &candidate).unwrap();
        assert!(test_gate(&aggregate).is_err());
    }

    #[test]
    fn validation_gate_accepts_dynamic_routes_with_bounded_resources() {
        let aggregate = PairAggregateReceipt {
            pairs: 2,
            unique_cases: 2,
            task_classes: 2,
            candidate_wins: 1,
            candidate_win_cases: 1,
            candidate_losses: 0,
            ties: 1,
            seed_behavior_score: 0.5,
            candidate_behavior_score: 1.0,
            behavior_delta: 0.5,
            seed_completed: 1,
            candidate_completed: 2,
            seed_quality_passed: 1,
            candidate_quality_passed: 2,
            candidate_safety_violations: 0,
            latency_ratio: 1.0,
            token_ratio: 1.0,
            causal_profile_runs: 2,
            route_semantics_runs: 2,
            workflow_profile_runs: 1,
            seed_direct_runs: 2,
            seed_workflow_runs: 0,
            candidate_direct_runs: 1,
            candidate_workflow_runs: 1,
            route_changed_pairs: 1,
        };

        validation_gate(&aggregate).unwrap();

        let mut expensive = aggregate.clone();
        expensive.latency_ratio = 1.30;
        assert!(validation_gate(&expensive).is_err());
    }
}
