use super::{
    wilson_lower_bound, TaskClass, LEARNED_ROUTER_MIN_EXAMPLES,
    LEARNED_ROUTER_MIN_SUCCESS_CONFIDENCE,
};
use crate::{WorkflowPlanIr, WorkflowToolPolicy};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowExecutionTelemetry {
    pub task_class: TaskClass,
    pub routing_signature: String,
    pub plan: WorkflowPlanIr,
    pub succeeded: bool,
    pub quality_score: Option<f32>,
    pub latency_ms: u64,
    pub total_tokens: u64,
    pub tool_calls: u64,
    pub successful_tools_by_step: BTreeMap<String, Vec<String>>,
    pub fallback_used: bool,
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

impl WorkflowSearchTeacher {
    pub fn train(telemetry: &[WorkflowExecutionTelemetry]) -> Self {
        let mut grouped = BTreeMap::<
            (TaskClass, String, usize, String),
            BTreeMap<String, WorkflowPriorAccumulator>,
        >::new();
        for entry in telemetry.iter().filter(|entry| !entry.fallback_used) {
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
            tool_calls: 0,
            successful_tools_by_step: BTreeMap::new(),
        }
    }

    fn record(&mut self, telemetry: &WorkflowExecutionTelemetry) {
        self.examples += 1;
        self.successes += usize::from(telemetry.succeeded);
        if let Some(score) = telemetry.quality_score {
            self.quality_total += score.clamp(0.0, 1.0);
            self.quality_examples += 1;
        }
        self.latency_ms = self.latency_ms.saturating_add(telemetry.latency_ms);
        self.total_tokens = self.total_tokens.saturating_add(telemetry.total_tokens);
        self.tool_calls = self.tool_calls.saturating_add(telemetry.tool_calls);
        if telemetry.succeeded
            && telemetry.quality_score.unwrap_or_default().clamp(0.0, 1.0)
                >= ADAPTIVE_WORKFLOW_PRIOR_MIN_QUALITY
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
        let average_total_tokens = self.total_tokens / divisor;
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
            tool_policy: step.tool_policy.clone(),
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
