use super::{
    wilson_lower_bound, LearningDisposition, LearningEvidenceV1, LearningTermination,
    LearningUsageCompleteness, TaskClass, LEARNED_ROUTER_MIN_EXAMPLES,
    LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE,
};
use crate::{
    minimum_team_uplift_bps, WorkflowPlanIr, WorkflowToolPolicy, AUTO_COLLABORATION_MIN_UPLIFT_BPS,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowExecutionTelemetry {
    pub task_class: TaskClass,
    #[serde(default)]
    pub pre_decision_context_fingerprint: String,
    #[serde(default)]
    pub route_action_id: String,
    pub routing_signature: String,
    pub plan: WorkflowPlanIr,
    pub succeeded: bool,
    pub quality_score: Option<f32>,
    #[serde(default)]
    pub learning_evidence: LearningEvidenceV1,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub successful_tools_by_step: BTreeMap<String, Vec<String>>,
    pub fallback_used: bool,
    #[serde(default)]
    pub paired_team_score_bps: Option<u16>,
    #[serde(default)]
    pub paired_anchor_score_bps: Option<u16>,
    #[serde(default)]
    pub paired_uplift_bps: Option<i16>,
    #[serde(default)]
    pub selected_anchor: bool,
    #[serde(default)]
    pub anchor_latency_ms: Option<u64>,
}

impl WorkflowExecutionTelemetry {
    fn has_valid_matched_comparison(&self) -> bool {
        let (Some(team), Some(anchor), Some(uplift)) = (
            self.paired_team_score_bps,
            self.paired_anchor_score_bps,
            self.paired_uplift_bps,
        ) else {
            return false;
        };
        let scores_are_valid = team <= 10_000
            && anchor <= 10_000
            && i32::from(team) - i32::from(anchor) == i32::from(uplift);
        let provenance_is_complete = self.learning_evidence.contract_is_valid()
            && self.learning_evidence.termination == LearningTermination::Completed
            && self.learning_evidence.usage_completeness != LearningUsageCompleteness::Missing
            && self.learning_evidence.steer_epoch.is_some()
            && self.learning_evidence.budget_fingerprint.is_some();
        self.succeeded && scores_are_valid && provenance_is_complete
    }
}

pub(super) const ADAPTIVE_WORKFLOW_PRIOR_MIN_QUALITY: f32 = 0.72;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowTopologyStep {
    pub role: String,
    pub model: String,
    pub access: Vec<usize>,
    pub tool_policy: WorkflowToolPolicy,
    pub preferred_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowTopologyPrior {
    pub task_class: TaskClass,
    pub routing_signature: String,
    pub effort: String,
    pub max_models: usize,
    pub profile_id: String,
    pub steps: Vec<WorkflowTopologyStep>,
    pub examples: usize,
    pub success_rate: f32,
    pub success_confidence: f64,
    pub average_quality: Option<f32>,
    pub average_latency_ms: u64,
    pub average_total_tokens: u64,
    pub average_tool_calls: f32,
    score: i64,
}

impl WorkflowTopologyPrior {
    pub fn prompt_hint(&self) -> String {
        let quality = self
            .average_quality
            .map(|score| format!("{score:.2}"))
            .unwrap_or_else(|| "unrated".to_string());
        let steps = self
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let access = if step.access.is_empty() {
                    "none".to_string()
                } else {
                    step.access
                        .iter()
                        .map(|dependency| (dependency + 1).to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                };
                let preferred_tools = if step.preferred_tools.is_empty() {
                    "none".to_string()
                } else {
                    step.preferred_tools.join(",")
                };
                format!(
                    "{}. role={} model={} access={} tools={} observed_successful_tools={}",
                    index + 1,
                    step.role,
                    step.model,
                    access,
                    step.tool_policy.label(),
                    preferred_tools,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Pareto prompt/topology prior profile={} scope={} from {} comparable executions (success={:.0}%, confidence={:.2}, quality={}, avg_latency_ms={}, avg_tokens={}, avg_tools={:.1}). Treat this only as a prior: keep it when it fits the current query, otherwise design a better graph.\n{}",
            self.profile_id,
            if self.routing_signature.is_empty() {
                "task_class"
            } else {
                self.routing_signature.as_str()
            },
            self.examples,
            self.success_rate * 100.0,
            self.success_confidence,
            quality,
            self.average_latency_ms,
            self.average_total_tokens,
            self.average_tool_calls,
            steps
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorkflowSearchTeacher {
    priors: Vec<WorkflowTopologyPrior>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchedCollaborationEvidence {
    pub task_class: TaskClass,
    pub effort: String,
    pub pre_decision_context_fingerprint: String,
    pub route_action_id: String,
    pub routing_signature: String,
    pub examples: usize,
    pub team_wins: usize,
    pub team_win_rate: f32,
    pub team_win_confidence: f64,
    pub below_admission_floor: usize,
    pub below_admission_floor_confidence: f64,
    pub anchor_selections: usize,
    pub average_uplift_bps: i16,
    pub average_team_latency_ms: u64,
    pub average_anchor_latency_ms: Option<u64>,
}

impl MatchedCollaborationEvidence {
    pub fn evidence_ready(&self) -> bool {
        self.examples >= LEARNED_ROUTER_MIN_EXAMPLES
    }

    pub fn strong_evidence_against_collaboration(&self, required_uplift_bps: u16) -> bool {
        self.evidence_ready()
            && i32::from(self.average_uplift_bps) < i32::from(required_uplift_bps)
            && self.below_admission_floor_confidence >= 0.5
    }

    pub fn prompt_hint(&self) -> String {
        format!(
            "matched_direct_team class={} effort={} context={} action={} legacy_signature={} samples={} team_wins={} team_win_rate={:.0}% team_win_lower_confidence={:.2} below_admission_floor={} below_admission_floor_lower_confidence={:.2} average_uplift_bps={} anchor_selected={} average_team_latency_ms={} average_anchor_latency_ms={} support={}",
            self.task_class.label(),
            self.effort,
            if self.pre_decision_context_fingerprint.is_empty() {
                "legacy"
            } else {
                self.pre_decision_context_fingerprint.as_str()
            },
            if self.route_action_id.is_empty() {
                "legacy"
            } else {
                self.route_action_id.as_str()
            },
            self.routing_signature,
            self.examples,
            self.team_wins,
            self.team_win_rate * 100.0,
            self.team_win_confidence,
            self.below_admission_floor,
            self.below_admission_floor_confidence,
            self.average_uplift_bps,
            self.anchor_selections,
            self.average_team_latency_ms,
            self.average_anchor_latency_ms
                .map(|latency| latency.to_string())
                .unwrap_or_else(|| "unmeasured".to_string()),
            if self.evidence_ready() {
                "ready"
            } else {
                "insufficient"
            },
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchedCollaborationEvidenceTeacher {
    evidence: Vec<MatchedCollaborationEvidence>,
    exact_index: BTreeMap<(TaskClass, String, String, String), usize>,
    legacy_index: BTreeMap<(TaskClass, String, String), usize>,
}

impl MatchedCollaborationEvidenceTeacher {
    pub fn train(telemetry: &[WorkflowExecutionTelemetry]) -> Self {
        let mut grouped = BTreeMap::<
            (TaskClass, String, String, String, String),
            MatchedEvidenceAccumulator,
        >::new();
        for entry in telemetry
            .iter()
            .filter(|entry| entry.has_valid_matched_comparison())
        {
            grouped
                .entry((
                    entry.task_class.clone(),
                    entry.plan.effort.clone(),
                    entry.pre_decision_context_fingerprint.clone(),
                    entry.route_action_id.clone(),
                    entry.routing_signature.clone(),
                ))
                .or_default()
                .record(entry);
        }
        let mut evidence = grouped
            .into_iter()
            .map(
                |(
                    (task_class, effort, context_fingerprint, route_action_id, routing_signature),
                    accumulator,
                )| {
                    accumulator.finish(
                        task_class,
                        effort,
                        context_fingerprint,
                        route_action_id,
                        routing_signature,
                    )
                },
            )
            .collect::<Vec<_>>();
        evidence.sort_by(|left, right| {
            right
                .evidence_ready()
                .cmp(&left.evidence_ready())
                .then_with(|| right.examples.cmp(&left.examples))
                .then_with(|| left.task_class.cmp(&right.task_class))
                .then_with(|| left.effort.cmp(&right.effort))
                .then_with(|| {
                    left.pre_decision_context_fingerprint
                        .cmp(&right.pre_decision_context_fingerprint)
                })
                .then_with(|| left.route_action_id.cmp(&right.route_action_id))
                .then_with(|| left.routing_signature.cmp(&right.routing_signature))
        });
        Self::from_calibrated_evidence(evidence)
    }

    pub(crate) fn from_calibrated_evidence(evidence: Vec<MatchedCollaborationEvidence>) -> Self {
        let mut exact_index = BTreeMap::new();
        let mut legacy_index = BTreeMap::new();
        for (index, entry) in evidence.iter().enumerate() {
            let effort = entry.effort.trim().to_ascii_lowercase();
            if !entry.pre_decision_context_fingerprint.is_empty()
                && !entry.route_action_id.is_empty()
            {
                exact_index.insert(
                    (
                        entry.task_class.clone(),
                        effort,
                        entry.pre_decision_context_fingerprint.clone(),
                        entry.route_action_id.clone(),
                    ),
                    index,
                );
            } else if entry.pre_decision_context_fingerprint.is_empty()
                && entry.route_action_id.is_empty()
                && !entry.routing_signature.is_empty()
            {
                legacy_index.insert(
                    (
                        entry.task_class.clone(),
                        effort,
                        entry.routing_signature.clone(),
                    ),
                    index,
                );
            }
        }
        Self {
            evidence,
            exact_index,
            legacy_index,
        }
    }

    pub fn calibrated_evidence(&self) -> &[MatchedCollaborationEvidence] {
        &self.evidence
    }

    pub fn match_context_action(
        &self,
        task_class: &TaskClass,
        effort: &str,
        context_fingerprint: &str,
        route_action_id: &str,
        legacy_signature: &str,
    ) -> (Option<&MatchedCollaborationEvidence>, usize) {
        let effort = effort.trim().to_ascii_lowercase();
        let mut lookups = 0usize;
        if !context_fingerprint.is_empty() && !route_action_id.is_empty() {
            lookups = lookups.saturating_add(1);
            if let Some(index) = self.exact_index.get(&(
                task_class.clone(),
                effort.clone(),
                context_fingerprint.to_string(),
                route_action_id.to_string(),
            )) {
                return (self.evidence.get(*index), lookups);
            }
        }
        if !legacy_signature.is_empty() {
            lookups = lookups.saturating_add(1);
            if let Some(index) =
                self.legacy_index
                    .get(&(task_class.clone(), effort, legacy_signature.to_string()))
            {
                return (self.evidence.get(*index), lookups);
            }
        }
        (None, lookups)
    }
}

#[derive(Debug, Clone, Default)]
struct MatchedEvidenceAccumulator {
    examples: usize,
    team_wins: usize,
    below_admission_floor: usize,
    anchor_selections: usize,
    uplift_bps: i64,
    team_latency_ms: u64,
    anchor_latency_ms: u64,
    anchor_latency_examples: usize,
}

impl MatchedEvidenceAccumulator {
    fn record(&mut self, telemetry: &WorkflowExecutionTelemetry) {
        let Some(uplift) = telemetry.paired_uplift_bps else {
            return;
        };
        self.examples += 1;
        self.team_wins += usize::from(uplift > 0 && !telemetry.selected_anchor);
        let admission_floor = if telemetry.plan.effort.eq_ignore_ascii_case("auto") {
            AUTO_COLLABORATION_MIN_UPLIFT_BPS
        } else {
            minimum_team_uplift_bps(&telemetry.plan.effort)
        };
        self.below_admission_floor += usize::from(i32::from(uplift) < i32::from(admission_floor));
        self.anchor_selections += usize::from(telemetry.selected_anchor);
        self.uplift_bps = self.uplift_bps.saturating_add(i64::from(uplift));
        self.team_latency_ms = self.team_latency_ms.saturating_add(telemetry.latency_ms);
        if let Some(latency) = telemetry.anchor_latency_ms {
            self.anchor_latency_ms = self.anchor_latency_ms.saturating_add(latency);
            self.anchor_latency_examples += 1;
        }
    }

    fn finish(
        self,
        task_class: TaskClass,
        effort: String,
        pre_decision_context_fingerprint: String,
        route_action_id: String,
        routing_signature: String,
    ) -> MatchedCollaborationEvidence {
        let examples = self.examples.max(1);
        MatchedCollaborationEvidence {
            task_class,
            effort,
            pre_decision_context_fingerprint,
            route_action_id,
            routing_signature,
            examples: self.examples,
            team_wins: self.team_wins,
            team_win_rate: self.team_wins as f32 / examples as f32,
            team_win_confidence: wilson_lower_bound(self.team_wins, self.examples),
            below_admission_floor: self.below_admission_floor,
            below_admission_floor_confidence: wilson_lower_bound(
                self.below_admission_floor,
                self.examples,
            ),
            anchor_selections: self.anchor_selections,
            average_uplift_bps: (self.uplift_bps / examples as i64)
                .clamp(i64::from(i16::MIN), i64::from(i16::MAX))
                as i16,
            average_team_latency_ms: self.team_latency_ms / examples as u64,
            average_anchor_latency_ms: (self.anchor_latency_examples > 0)
                .then(|| self.anchor_latency_ms / self.anchor_latency_examples as u64),
        }
    }
}

impl WorkflowSearchTeacher {
    pub fn train(telemetry: &[WorkflowExecutionTelemetry]) -> Self {
        let mut grouped = BTreeMap::<
            (TaskClass, String, usize, String),
            BTreeMap<String, WorkflowPriorAccumulator>,
        >::new();
        for entry in telemetry
            .iter()
            .filter(|entry| !entry.fallback_used && entry.learning_evidence.is_learnable())
        {
            let scopes = if entry.routing_signature.trim().is_empty() {
                vec![String::new()]
            } else {
                vec![String::new(), entry.routing_signature.clone()]
            };
            for routing_signature in scopes {
                let key = (
                    entry.task_class.clone(),
                    entry.plan.effort.clone(),
                    entry.plan.budget.max_models,
                    routing_signature,
                );
                grouped
                    .entry(key)
                    .or_default()
                    .entry(workflow_topology_signature(&entry.plan))
                    .or_insert_with(|| WorkflowPriorAccumulator::new(&entry.plan))
                    .record(entry);
            }
        }

        let priors = grouped
            .into_iter()
            .flat_map(
                |((task_class, effort, max_models, routing_signature), candidates)| {
                    candidates.into_values().map(move |candidate| {
                        candidate.finish(
                            task_class.clone(),
                            effort.clone(),
                            max_models,
                            routing_signature.clone(),
                        )
                    })
                },
            )
            .collect();
        Self { priors }
    }

    pub fn pareto_front(
        &self,
        task_class: &TaskClass,
        effort: &str,
        allowed_models: &[String],
        max_models: usize,
    ) -> Vec<&WorkflowTopologyPrior> {
        self.pareto_front_for_signature(task_class, effort, allowed_models, max_models, None)
    }

    pub fn pareto_front_for_signature(
        &self,
        task_class: &TaskClass,
        effort: &str,
        allowed_models: &[String],
        max_models: usize,
        routing_signature: Option<&str>,
    ) -> Vec<&WorkflowTopologyPrior> {
        let allowed = allowed_models
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let eligible = self
            .priors
            .iter()
            .filter(|prior| {
                &prior.task_class == task_class
                    && prior.effort == effort
                    && prior.max_models <= max_models
                    && match routing_signature {
                        Some(signature) => prior.routing_signature == signature,
                        None => prior.routing_signature.is_empty(),
                    }
                    && prior.examples >= LEARNED_ROUTER_MIN_EXAMPLES
                    && prior.success_confidence >= LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE
                    && prior.average_quality.unwrap_or(prior.success_rate)
                        >= ADAPTIVE_WORKFLOW_PRIOR_MIN_QUALITY
                    && prior
                        .steps
                        .iter()
                        .all(|step| allowed.contains(step.model.as_str()))
            })
            .collect::<Vec<_>>();
        eligible
            .iter()
            .enumerate()
            .filter(|(index, prior)| {
                !eligible.iter().enumerate().any(|(other_index, other)| {
                    index != &other_index && workflow_prior_dominates(other, prior)
                })
            })
            .map(|(_, prior)| *prior)
            .collect()
    }

    pub fn best_prior(
        &self,
        task_class: &TaskClass,
        effort: &str,
        allowed_models: &[String],
        max_models: usize,
    ) -> Option<&WorkflowTopologyPrior> {
        self.pareto_front(task_class, effort, allowed_models, max_models)
            .into_iter()
            .max_by(|left, right| compare_workflow_priors(left, right, effort))
    }

    pub fn best_prior_for_signature(
        &self,
        task_class: &TaskClass,
        effort: &str,
        allowed_models: &[String],
        max_models: usize,
        routing_signature: &str,
    ) -> Option<&WorkflowTopologyPrior> {
        self.pareto_front_for_signature(
            task_class,
            effort,
            allowed_models,
            max_models,
            Some(routing_signature),
        )
        .into_iter()
        .max_by(|left, right| compare_workflow_priors(left, right, effort))
        .or_else(|| self.best_prior(task_class, effort, allowed_models, max_models))
    }
}

fn workflow_prior_dominates(left: &WorkflowTopologyPrior, right: &WorkflowTopologyPrior) -> bool {
    let left_quality = left.average_quality.unwrap_or(left.success_rate);
    let right_quality = right.average_quality.unwrap_or(right.success_rate);
    let no_worse = left.success_rate >= right.success_rate
        && left.success_confidence >= right.success_confidence
        && left_quality >= right_quality
        && left.average_latency_ms <= right.average_latency_ms
        && left.task_class == right.task_class;
    let strictly_better = left.success_rate > right.success_rate
        || left.success_confidence > right.success_confidence
        || left_quality > right_quality
        || left.average_latency_ms < right.average_latency_ms;
    no_worse && strictly_better
}

fn compare_workflow_priors(
    left: &WorkflowTopologyPrior,
    right: &WorkflowTopologyPrior,
    effort: &str,
) -> std::cmp::Ordering {
    left.success_confidence
        .total_cmp(&right.success_confidence)
        .then_with(|| match effort {
            "fast" => right
                .average_latency_ms
                .cmp(&left.average_latency_ms)
                .then_with(|| left.success_rate.total_cmp(&right.success_rate)),
            "pro" => left
                .average_quality
                .unwrap_or(left.success_rate)
                .total_cmp(&right.average_quality.unwrap_or(right.success_rate))
                .then_with(|| left.success_rate.total_cmp(&right.success_rate))
                .then_with(|| right.average_latency_ms.cmp(&left.average_latency_ms)),
            _ => left.score.cmp(&right.score),
        })
}

#[derive(Debug, Clone)]
struct WorkflowPriorAccumulator {
    plan: WorkflowPlanIr,
    examples: usize,
    successes: usize,
    quality_total: f32,
    quality_examples: usize,
    latency_ms: u64,
    total_tokens: u64,
    token_examples: usize,
    tool_calls: u64,
    successful_tools_by_step: BTreeMap<String, BTreeMap<String, usize>>,
}

impl WorkflowPriorAccumulator {
    fn new(plan: &WorkflowPlanIr) -> Self {
        Self {
            plan: plan.clone(),
            examples: 0,
            successes: 0,
            quality_total: 0.0,
            quality_examples: 0,
            latency_ms: 0,
            total_tokens: 0,
            token_examples: 0,
            tool_calls: 0,
            successful_tools_by_step: BTreeMap::new(),
        }
    }

    fn record(&mut self, telemetry: &WorkflowExecutionTelemetry) {
        let evidence = &telemetry.learning_evidence;
        self.examples += 1;
        self.successes += usize::from(evidence.disposition == LearningDisposition::Positive);
        if let Some(score) = evidence.quality_score() {
            self.quality_total += score.clamp(0.0, 1.0);
            self.quality_examples += 1;
        }
        self.latency_ms = self.latency_ms.saturating_add(telemetry.latency_ms);
        if evidence.usage_completeness == LearningUsageCompleteness::Complete {
            self.total_tokens = self.total_tokens.saturating_add(telemetry.total_tokens);
            self.token_examples += 1;
        }
        self.tool_calls = self.tool_calls.saturating_add(telemetry.tool_calls);
        if evidence.disposition == LearningDisposition::Positive
            && evidence.quality_score().unwrap_or_default() >= ADAPTIVE_WORKFLOW_PRIOR_MIN_QUALITY
        {
            for (step_id, tools) in &telemetry.successful_tools_by_step {
                let counts = self
                    .successful_tools_by_step
                    .entry(step_id.clone())
                    .or_default();
                for tool in tools {
                    if !tool.trim().is_empty() && tool.len() <= 128 {
                        *counts.entry(tool.clone()).or_default() += 1;
                    }
                }
            }
        }
    }

    fn finish(
        self,
        task_class: TaskClass,
        effort: String,
        max_models: usize,
        routing_signature: String,
    ) -> WorkflowTopologyPrior {
        let divisor = self.examples.max(1) as u64;
        let success_rate = self.successes as f32 / self.examples.max(1) as f32;
        let success_confidence = wilson_lower_bound(self.successes, self.examples);
        let average_quality =
            (self.quality_examples > 0).then(|| self.quality_total / self.quality_examples as f32);
        let average_latency_ms = self.latency_ms / divisor;
        let average_total_tokens = self.total_tokens / self.token_examples.max(1) as u64;
        let average_tool_calls = self.tool_calls as f32 / self.examples.max(1) as f32;
        let quality = average_quality.unwrap_or(success_rate);
        let score = (success_rate * 10_000.0) as i64 + (quality * 5_000.0) as i64
            - average_latency_ms as i64 / 100;
        let mut steps = normalized_topology_steps(&self.plan);
        for (index, step) in self.plan.steps.iter().enumerate() {
            let Some(counts) = self.successful_tools_by_step.get(&step.id) else {
                continue;
            };
            let mut tools = counts
                .iter()
                .filter(|(_, count)| **count >= 2 && **count * 2 >= self.examples)
                .map(|(tool, count)| (tool.clone(), *count))
                .collect::<Vec<_>>();
            tools.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            if let Some(prior_step) = steps.get_mut(index) {
                prior_step.preferred_tools =
                    tools.into_iter().take(3).map(|(tool, _)| tool).collect();
            }
        }
        WorkflowTopologyPrior {
            task_class,
            routing_signature,
            effort,
            max_models,
            profile_id: self.plan.prompt_profile.clone(),
            steps,
            examples: self.examples,
            success_rate,
            success_confidence,
            average_quality,
            average_latency_ms,
            average_total_tokens,
            average_tool_calls,
            score,
        }
    }
}

fn normalized_topology_steps(plan: &WorkflowPlanIr) -> Vec<WorkflowTopologyStep> {
    let indexes = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    plan.steps
        .iter()
        .map(|step| WorkflowTopologyStep {
            role: step.role.clone(),
            model: step.model.clone(),
            access: step
                .access
                .iter()
                .filter_map(|dependency| indexes.get(dependency.as_str()).copied())
                .collect(),
            tool_policy: step.tool_policy,
            preferred_tools: Vec::new(),
        })
        .collect()
}

fn workflow_topology_signature(plan: &WorkflowPlanIr) -> String {
    let topology = normalized_topology_steps(plan)
        .into_iter()
        .map(|step| {
            format!(
                "{}:{}:[{}]:{}",
                step.role,
                step.model,
                step.access
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                step.tool_policy.label()
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("profile={}|{topology}", plan.prompt_profile)
}
