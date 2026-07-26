use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConductorRoleHints {
    pub planner: String,
    pub executor: String,
    pub reviewer: String,
    pub synthesizer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConductorRequest {
    pub workflow_id: String,
    pub objective: String,
    pub recent_context: String,
    pub effort: String,
    pub policy: String,
    pub conductor_model: String,
    pub worker_models: Vec<String>,
    pub role_hints: ConductorRoleHints,
    pub budget: WorkflowBudget,
    pub execution_contract: ConductorExecutionContract,
    pub prior_hint: Option<String>,
    pub prompt_evolution_enabled: bool,
    pub prompt_genome: ConductorPromptGenome,
}

#[derive(Debug, Clone)]
pub struct ConductorHarness {
    request: ConductorRequest,
}

#[derive(Debug, Deserialize)]
struct ConductorWorkflowPayload {
    steps: Vec<ConductorWorkflowStepPayload>,
}

#[derive(Debug, Deserialize)]
struct ConductorWorkflowStepPayload {
    id: String,
    #[serde(default)]
    role: Option<String>,
    model: String,
    subtask: String,
    #[serde(default, alias = "access_list", alias = "accessList")]
    access: Vec<String>,
    #[serde(default)]
    output_kind: Option<WorkflowOutputKind>,
    #[serde(default)]
    tool_policy: Option<WorkflowToolPolicy>,
}

#[derive(Debug, Clone, Default)]
struct ConductorStepSemantics {
    output_kind: Option<WorkflowOutputKind>,
    tool_policy: Option<WorkflowToolPolicy>,
}

impl ConductorHarness {
    pub fn new(request: ConductorRequest) -> Self {
        Self { request }
    }

    pub fn request(&self) -> &ConductorRequest {
        &self.request
    }

    pub fn planning_prompt(&self) -> String {
        let request = &self.request;
        let worker_pool = request
            .worker_models
            .iter()
            .map(|model| format!("- {model}"))
            .collect::<Vec<_>>()
            .join("\n");
        let prior_hint = request
            .prior_hint
            .as_deref()
            .unwrap_or("(none - design from the current query)");
        let evolved_directive = if request.prompt_evolution_enabled {
            request.prompt_genome.conductor_directive()
        } else {
            "Prompt evolution is disabled. Use only the baseline harness constraints above."
                .to_string()
        };
        let branch_role_constraint = match request.prompt_genome.role_strategy {
            PromptRoleStrategy::Flexible => {
                "- Assign the smallest useful root roles; model and role reuse is allowed when it reduces waste."
            }
            PromptRoleStrategy::Specialists => {
                "- Give independent root branches non-overlapping subtasks and use distinct models when the pool permits."
            }
            PromptRoleStrategy::DiverseSpecialists => {
                "- Give independent root branches non-overlapping subtasks, distinct models when possible, and complementary domain-specific roles."
            }
        };
        format!(
            concat!(
                "You are the Conductor Agent for a Fugu-style Cindx workflow. Design a query-specific dependency graph instead of answering the user. Return only strict JSON matching this example:\n",
                "{schema_example}\n\n",
                "Harness constraints:\n",
                "- Execution contract: task_class={task_class}, expected_uplift={expected_uplift_bps}bps, confidence={confidence_bps}bps, max_parallelism={contract_parallelism}, quorum={contract_quorum}, verification_required={contract_verification}, terminal_reserve={terminal_reserve}, stop={stop_policy:?}, fallback={fallback_policy:?}.\n",
                "- Use between 1 and {max_steps} workflow steps, including the final synthesis step. Choose the smallest useful graph.\n",
                "- Use no more than {max_models} distinct worker models.\n",
                "- role is a short lowercase domain label such as mathematician, evidence_researcher, critic, or integrator; do not use it as an execution permission.\n",
                "- output_kind must be exactly analysis, evidence, verification, or synthesis. The final step must use synthesis.\n",
                "- tool_policy must be exactly none, read_only_evidence, or read_only_exploration. Never request effectful tools here.\n",
                "- Preserve listed order: access may reference only earlier step ids.\n",
                "- The task contract requires {required_contributions} independent contribution(s). This is the only minimum branch count. The evolved profile controls preferences and upper bounds; it must not force decorative agents when the task requires zero or one branch.\n",
                "- Require a verifier only when contract verification_required=true; otherwise add one only when it resolves a concrete uncertainty.\n",
                "- Keep workers isolated and expose an earlier result only through access.\n",
                "{branch_role_constraint}\n",
                "- A verification step must directly access every independent root branch it audits.\n",
                "- Every branch must reach the final synthesis step; retain dissenting or failed branches.\n",
                "- Use exact model strings from the worker pool. The Conductor model is not implicitly a worker.\n",
                "- Do not include markdown fences, commentary, tool calls, or a user-facing answer.\n\n",
                "Configured worker role hints:\nPlanner: {planner}\nExecutor: {executor}\nReviewer: {reviewer}\nSynthesizer: {synthesizer}\n\n",
                "Allowed worker pool:\n{worker_pool}\n\n",
                "Historical execution prior:\n{prior_hint}\n\n",
                "Evolved orchestration directive:\n{evolved_directive}\n\n",
                "User request:\n{objective}\n\nRecent session memory:\n{recent_context}"
            ),
            schema_example = conductor_schema_example(request),
            max_steps = request.budget.max_steps,
            max_models = request.budget.max_models,
            task_class = request.execution_contract.task_class.label(),
            expected_uplift_bps = request.execution_contract.expected_uplift_bps,
            confidence_bps = request.execution_contract.confidence_bps,
            contract_parallelism = request.execution_contract.max_parallelism,
            contract_quorum = request.execution_contract.min_successful_branches,
            contract_verification = request.execution_contract.verification_required,
            required_contributions = request.execution_contract.min_distinct_contributions,
            terminal_reserve = request.execution_contract.terminal_model_call_reserve,
            stop_policy = request.execution_contract.stop_policy,
            fallback_policy = request.execution_contract.fallback_policy,
            planner = request.role_hints.planner,
            executor = request.role_hints.executor,
            reviewer = request.role_hints.reviewer,
            synthesizer = request.role_hints.synthesizer,
            worker_pool = worker_pool,
            prior_hint = prior_hint,
            evolved_directive = evolved_directive,
            branch_role_constraint = branch_role_constraint,
            objective = request.objective,
            recent_context = if request.recent_context.trim().is_empty() {
                "(none)"
            } else {
                &request.recent_context
            },
        )
    }

    pub fn repair_prompt(&self, invalid_response: &str, validation_error: &str) -> String {
        format!(
            "Your previous WorkflowPlan was rejected by the deterministic Cindx Harness. Correct only the workflow structure and return strict JSON with no commentary.\n\nValidation error:\n{}\n\nRejected response:\n{}\n\nOriginal planning request:\n{}",
            validation_error,
            truncate_conductor_text(invalid_response, 6_000),
            self.planning_prompt()
        )
    }

    pub fn parse_plan(&self, response: &str) -> Result<WorkflowPlanIr, String> {
        if self.request.prompt_evolution_enabled {
            self.request.prompt_genome.validate()?;
        }
        let start = response
            .find('{')
            .ok_or_else(|| "conductor did not return a JSON object".to_string())?;
        let end = response
            .rfind('}')
            .filter(|end| *end >= start)
            .ok_or_else(|| "conductor returned incomplete JSON".to_string())?;
        let payload = serde_json::from_str::<ConductorWorkflowPayload>(&response[start..=end])
            .map_err(|error| format!("conductor workflow JSON is invalid: {error}"))?;
        let step_count = payload.steps.len();
        let mut semantics = Vec::with_capacity(step_count);
        let workflow = AdaptiveWorkflow {
            steps: payload
                .steps
                .into_iter()
                .enumerate()
                .map(|(index, step)| {
                    semantics.push(ConductorStepSemantics {
                        output_kind: step.output_kind.clone(),
                        tool_policy: step.tool_policy.clone(),
                    });
                    let access = step
                        .access
                        .into_iter()
                        .map(|dependency| dependency.trim().to_string())
                        .collect::<Vec<_>>();
                    let role = step.role.unwrap_or_else(|| {
                        if index + 1 == step_count {
                            "synthesizer".to_string()
                        } else if access.is_empty() {
                            "thinker".to_string()
                        } else {
                            "worker".to_string()
                        }
                    });
                    AdaptiveWorkflowStep {
                        id: step.id.trim().to_string(),
                        role: role.trim().to_ascii_lowercase(),
                        model: step.model.trim().to_string(),
                        subtask: step.subtask.trim().to_string(),
                        access,
                    }
                })
                .collect(),
        };
        self.validate_shape(&workflow)?;
        self.build_plan(&workflow, Some(&semantics))
    }

    pub fn fallback_plan(&self) -> Result<WorkflowPlanIr, String> {
        if self.request.prompt_evolution_enabled {
            self.request.prompt_genome.validate()?;
        }
        let mut worker_models = Vec::new();
        for model in &self.request.worker_models {
            let model = model.trim();
            if !model.is_empty() && !worker_models.iter().any(|selected| selected == model) {
                worker_models.push(model.to_string());
            }
        }
        worker_models.truncate(self.request.budget.max_models);
        if worker_models.is_empty() {
            return Err("deterministic conductor fallback has no worker model".to_string());
        }

        let required_branches = self.request.execution_contract.min_distinct_contributions;
        let needs_verifier = required_branches > 0
            && self.request.execution_contract.verification_required
            && self.request.prompt_genome.verification == PromptVerification::Adversarial
            && self.request.budget.max_steps >= required_branches.saturating_add(2);
        let reserved_steps = 1 + usize::from(needs_verifier);
        let branch_capacity = self
            .request
            .budget
            .max_steps
            .saturating_sub(reserved_steps)
            .min(self.request.budget.max_models)
            .min(self.request.execution_contract.max_parallelism)
            .min(
                self.request
                    .prompt_genome
                    .max_parallel_branches
                    .max(required_branches),
            );
        if branch_capacity < required_branches {
            return Err(format!(
                "deterministic conductor fallback can schedule {branch_capacity} independent branch(es), but the task requires {required_branches}"
            ));
        }
        let branch_count = required_branches;

        let hinted_models = [
            &self.request.role_hints.planner,
            &self.request.role_hints.executor,
            &self.request.role_hints.reviewer,
        ];
        let mut used_root_models = BTreeSet::new();
        let mut steps = Vec::new();
        for index in 0..branch_count {
            let hinted = hinted_models
                .get(index)
                .and_then(|hint| {
                    worker_models
                        .iter()
                        .find(|model| model.as_str() == hint.as_str())
                })
                .filter(|model| !used_root_models.contains(model.as_str()));
            let model = hinted
                .or_else(|| {
                    worker_models
                        .iter()
                        .find(|model| !used_root_models.contains(model.as_str()))
                })
                .unwrap_or_else(|| &worker_models[index % worker_models.len()])
                .clone();
            used_root_models.insert(model.clone());
            let (id, role, subtask) = match index {
                0 => (
                    "approach_a".to_string(),
                    "thinker".to_string(),
                    "derive the strongest solution and make every assumption explicit".to_string(),
                ),
                1 => (
                    "approach_b".to_string(),
                    "worker".to_string(),
                    "develop an independent solution path grounded in concrete evidence"
                        .to_string(),
                ),
                _ => (
                    format!("approach_{}", (b'a' + index as u8) as char),
                    "worker".to_string(),
                    "stress-test failure modes and propose a materially different alternative"
                        .to_string(),
                ),
            };
            steps.push(AdaptiveWorkflowStep {
                id,
                role,
                model,
                subtask,
                access: Vec::new(),
            });
        }

        let root_ids = steps.iter().map(|step| step.id.clone()).collect::<Vec<_>>();
        if needs_verifier {
            let verifier_model = worker_models
                .iter()
                .find(|model| model.as_str() == self.request.role_hints.reviewer)
                .unwrap_or(&worker_models[worker_models.len().saturating_sub(1)])
                .clone();
            steps.push(AdaptiveWorkflowStep {
                id: "verify".to_string(),
                role: "verifier".to_string(),
                model: verifier_model,
                subtask:
                    "adversarially audit every independent branch and identify unsupported claims"
                        .to_string(),
                access: root_ids.clone(),
            });
        }

        let synthesizer_model = worker_models
            .iter()
            .find(|model| model.as_str() == self.request.role_hints.synthesizer)
            .unwrap_or(&worker_models[0])
            .clone();
        let mut synthesis_access = root_ids;
        if needs_verifier {
            synthesis_access.push("verify".to_string());
        }
        steps.push(AdaptiveWorkflowStep {
            id: "synthesize".to_string(),
            role: "synthesizer".to_string(),
            model: synthesizer_model,
            subtask: "compare all authorized branches, resolve disagreements, and produce one checkable final result"
                .to_string(),
            access: synthesis_access,
        });

        let workflow = AdaptiveWorkflow { steps };
        self.validate_shape(&workflow)?;
        self.build_plan(&workflow, None)
    }

    fn build_plan(
        &self,
        workflow: &AdaptiveWorkflow,
        semantics: Option<&[ConductorStepSemantics]>,
    ) -> Result<WorkflowPlanIr, String> {
        let mut budget = self.request.budget.clone();
        if self.request.prompt_evolution_enabled {
            budget.max_model_turns_per_step = budget
                .max_model_turns_per_step
                .min(
                    self.request
                        .prompt_genome
                        .effective_max_model_turns_per_step(),
                )
                .max(1);
            budget.max_tool_calls_per_step = budget.max_tool_calls_per_step.min(
                self.request
                    .prompt_genome
                    .effective_max_tool_calls_per_step(),
            );
        }
        let mut plan = WorkflowPlanIr::from_adaptive_with_profile(
            self.request.workflow_id.clone(),
            self.request.objective.clone(),
            self.request.effort.clone(),
            self.request.policy.clone(),
            self.request.conductor_model.clone(),
            if self.request.prompt_evolution_enabled {
                self.request.prompt_genome.id.clone()
            } else {
                "legacy-baseline-v1".to_string()
            },
            workflow,
            budget,
        );
        if self.request.prompt_evolution_enabled {
            for step in &mut plan.steps {
                step.tool_policy = self.request.prompt_genome.workflow_tool_policy(&step.role);
            }
        }
        for (index, step) in plan.steps.iter_mut().enumerate() {
            let semantics = semantics.and_then(|values| values.get(index));
            if let Some(tool_policy) = semantics.and_then(|value| value.tool_policy.clone()) {
                step.tool_policy = tool_policy;
            }
            if let Some(output_kind) = semantics.and_then(|value| value.output_kind.clone()) {
                step.contract.output_kind = output_kind;
            } else if index + 1 == workflow.steps.len() {
                step.contract.output_kind = WorkflowOutputKind::Synthesis;
            }
            step.contract.input_steps = step.access.clone();
        }
        plan.validate(&self.request.worker_models)?;
        if self.request.execution_contract.verification_required {
            let root_ids = plan
                .steps
                .iter()
                .take(plan.steps.len().saturating_sub(1))
                .filter(|step| {
                    step.access.is_empty()
                        && step.contract.output_kind != WorkflowOutputKind::Verification
                })
                .map(|step| step.id.as_str())
                .collect::<BTreeSet<_>>();
            let verification_covers_roots = plan.steps.iter().any(|step| {
                step.contract.output_kind == WorkflowOutputKind::Verification
                    && root_ids
                        .iter()
                        .all(|root_id| step.access.iter().any(|access| access == root_id))
            });
            if !verification_covers_roots {
                return Err(
                    "workflow requires a verification step that directly audits every independent contribution"
                        .to_string(),
                );
            }
        }
        self.request.execution_contract.validate_plan(&plan)?;
        Ok(plan)
    }

    fn validate_shape(&self, workflow: &AdaptiveWorkflow) -> Result<(), String> {
        if workflow.steps.len() > self.request.budget.max_steps {
            return Err(format!(
                "conductor workflow exceeds the {}-step budget",
                self.request.budget.max_steps
            ));
        }
        let selected_models = workflow
            .steps
            .iter()
            .map(|step| step.model.as_str())
            .collect::<BTreeSet<_>>();
        if selected_models.len() > self.request.budget.max_models {
            return Err(format!(
                "conductor workflow exceeds the {}-model budget",
                self.request.budget.max_models
            ));
        }
        let independent_branches = workflow
            .steps
            .iter()
            .take(workflow.steps.len().saturating_sub(1))
            .filter(|step| step.access.is_empty())
            .collect::<Vec<_>>();
        let root_step_capacity = if self.request.prompt_genome.require_final_synthesis {
            self.request.budget.max_steps.saturating_sub(1)
        } else {
            self.request.budget.max_steps
        };
        let evolved_branch_limit = match self.request.prompt_genome.topology_strategy {
            PromptTopologyStrategy::Serial => 1,
            PromptTopologyStrategy::AdaptiveDag | PromptTopologyStrategy::ParallelDeliberation => {
                self.request.prompt_genome.max_parallel_branches
            }
        };
        let branch_limit = evolved_branch_limit
            .min(self.request.budget.max_models)
            .min(self.request.execution_contract.max_parallelism)
            .min(root_step_capacity);
        let required_branches = self.request.execution_contract.min_distinct_contributions;
        let branch_limit = branch_limit.max(required_branches).min(root_step_capacity);
        if independent_branches.len() < required_branches {
            return Err(
                format!(
                    "conductor workflow requires at least {required_branches} independent branch{} for the selected prompt profile",
                    if required_branches == 1 { "" } else { "es" }
                ),
            );
        }
        if independent_branches.len() > branch_limit {
            return Err(format!(
                "conductor workflow exceeds the selected prompt profile's {branch_limit}-branch limit"
            ));
        }
        if self.request.prompt_genome.role_strategy != PromptRoleStrategy::Flexible {
            let distinct_available_models = self
                .request
                .worker_models
                .iter()
                .collect::<BTreeSet<_>>()
                .len();
            let required_branch_models = required_branches.min(distinct_available_models);
            let distinct_branch_models = independent_branches
                .iter()
                .map(|step| step.model.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            if required_branch_models >= 2 && distinct_branch_models < required_branch_models {
                return Err(format!(
                    "conductor workflow requires {required_branch_models} distinct models across independent branches"
                ));
            }
            let distinct_branch_subtasks = independent_branches
                .iter()
                .map(|step| {
                    step.subtask
                        .split_whitespace()
                        .flat_map(str::chars)
                        .flat_map(char::to_lowercase)
                        .collect::<String>()
                })
                .collect::<BTreeSet<_>>()
                .len();
            if independent_branches.len() >= 2
                && distinct_branch_subtasks < independent_branches.len()
            {
                return Err("conductor independent branches repeat the same subtask".to_string());
            }
        }
        if self.request.prompt_genome.role_strategy == PromptRoleStrategy::DiverseSpecialists
            && independent_branches.len() >= 2
        {
            let branch_roles = independent_branches
                .iter()
                .map(|step| step.role.as_str())
                .collect::<BTreeSet<_>>();
            if branch_roles.len() < 2 {
                return Err(
                    "conductor diverse-specialist branches require at least two complementary role labels"
                        .to_string(),
                );
            }
        }
        if workflow
            .steps
            .last()
            .is_some_and(|step| required_branches >= 2 && step.access.len() < 2)
        {
            return Err("conductor synthesis must access at least two prior branches".to_string());
        }
        Ok(())
    }
}

fn conductor_schema_example(request: &ConductorRequest) -> String {
    let max_steps = request.budget.max_steps;
    let max_models = request.budget.max_models;
    let max_parallel_branches = request.prompt_genome.max_parallel_branches;
    let graph_depth = request.prompt_genome.graph_depth;
    let verification = request.prompt_genome.verification;
    let topology_strategy = request.prompt_genome.topology_strategy;
    let required_branches = request.execution_contract.min_distinct_contributions;
    let verification_required = request.execution_contract.verification_required;
    let role_hints = &request.role_hints;
    let branch_executor = if role_hints.executor != role_hints.planner {
        &role_hints.executor
    } else if role_hints.reviewer != role_hints.planner {
        &role_hints.reviewer
    } else {
        &role_hints.executor
    };
    let root_capacity = max_steps.saturating_sub(1).min(max_models);
    let branch_limit = match topology_strategy {
        PromptTopologyStrategy::Serial => root_capacity.min(1),
        PromptTopologyStrategy::AdaptiveDag | PromptTopologyStrategy::ParallelDeliberation => {
            root_capacity.min(max_parallel_branches)
        }
    }
    .max(required_branches.min(root_capacity));
    let branch_capacity = required_branches.min(branch_limit);
    if branch_capacity == 0 {
        return conductor_example_with_contracts(serde_json::json!({
            "steps": [{
                "id": "synthesize",
                "role": "synthesizer",
                "model": role_hints.synthesizer,
                "subtask": "produce a checkable execution brief",
                "access": [],
            }]
        }));
    }
    conductor_example_with_contracts(match branch_capacity {
        0 if graph_depth == PromptGraphDepth::Lean => serde_json::json!({
            "steps": [{
                "id": "synthesize",
                "role": "synthesizer",
                "model": role_hints.synthesizer,
                "subtask": "produce a checkable execution brief",
                "access": [],
            }]
        }),
        0 | 1 => serde_json::json!({
            "steps": [
                {
                    "id": "approach",
                    "role": "thinker",
                    "model": role_hints.planner,
                    "subtask": "analyze the request and produce the strongest approach",
                    "access": [],
                },
                {
                    "id": "synthesize",
                    "role": "synthesizer",
                    "model": role_hints.synthesizer,
                    "subtask": "turn the analysis into one checkable execution brief",
                    "access": ["approach"],
                },
            ]
        }),
        2 if verification != PromptVerification::Adversarial || max_steps < 4 => {
            serde_json::json!({
                "steps": [
                    {
                        "id": "approach_a",
                        "role": "thinker",
                        "model": role_hints.planner,
                        "subtask": "analyze assumptions and the strongest approach",
                        "access": [],
                    },
                    {
                        "id": "approach_b",
                        "role": "worker",
                        "model": branch_executor,
                        "subtask": "develop a concrete independent implementation path",
                        "access": [],
                    },
                    {
                        "id": "synthesize",
                        "role": "synthesizer",
                        "model": role_hints.synthesizer,
                        "subtask": "resolve both branches into one execution brief",
                        "access": ["approach_a", "approach_b"],
                    },
                ]
            })
        }
        _ if verification_required
            && verification == PromptVerification::Adversarial
            && max_steps >= 4 =>
        {
            serde_json::json!({
                "steps": [
                    {
                        "id": "approach_a",
                        "role": "thinker",
                        "model": role_hints.planner,
                        "subtask": "analyze assumptions and the strongest approach",
                        "access": [],
                    },
                    {
                        "id": "approach_b",
                        "role": "worker",
                        "model": branch_executor,
                        "subtask": "develop a concrete independent implementation path",
                        "access": [],
                    },
                    {
                        "id": "verify",
                        "role": "verifier",
                        "model": role_hints.reviewer,
                        "subtask": "cross-check both reports and identify unsupported claims",
                        "access": ["approach_a", "approach_b"],
                    },
                    {
                        "id": "synthesize",
                        "role": "synthesizer",
                        "model": role_hints.synthesizer,
                        "subtask": "resolve disagreements into one evidence-grounded execution brief",
                        "access": ["approach_a", "approach_b", "verify"],
                    },
                ]
            })
        }
        _ => serde_json::json!({
            "steps": [
                {
                    "id": "approach_a",
                    "role": "thinker",
                    "model": role_hints.planner,
                    "subtask": "analyze assumptions and the strongest approach",
                    "access": [],
                },
                {
                    "id": "approach_b",
                    "role": "worker",
                    "model": branch_executor,
                    "subtask": "develop a concrete independent implementation path",
                    "access": [],
                },
                {
                    "id": "synthesize",
                    "role": "synthesizer",
                    "model": role_hints.synthesizer,
                    "subtask": "resolve both branches into one evidence-grounded execution brief",
                    "access": ["approach_a", "approach_b"],
                },
            ]
        }),
    })
}

fn conductor_example_with_contracts(mut value: serde_json::Value) -> String {
    if let Some(steps) = value
        .get_mut("steps")
        .and_then(serde_json::Value::as_array_mut)
    {
        let final_index = steps.len().saturating_sub(1);
        for (index, step) in steps.iter_mut().enumerate() {
            let role = step
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let output_kind = if index == final_index {
                "synthesis"
            } else if role == "verifier" {
                "verification"
            } else if role == "worker" {
                "evidence"
            } else {
                "analysis"
            };
            let tool_policy = if matches!(output_kind, "synthesis" | "verification") {
                "none"
            } else {
                "read_only_evidence"
            };
            if let Some(object) = step.as_object_mut() {
                object.insert(
                    "output_kind".to_string(),
                    serde_json::Value::String(output_kind.to_string()),
                );
                object.insert(
                    "tool_policy".to_string(),
                    serde_json::Value::String(tool_policy.to_string()),
                );
            }
        }
    }
    value.to_string()
}

fn truncate_conductor_text(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("\n[truncated]");
    }
    output
}
