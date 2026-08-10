use super::*;

pub(crate) const ADAPTIVE_MODEL_DISTINCTNESS_ERROR_PREFIX: &str =
    "adaptive model distinctness blocked";

pub(crate) fn adaptive_recovery_model(
    step_id: &str,
    failed_model: &str,
    recovery_attempt: usize,
    models: &[String],
    retry_policy: PromptRetryPolicy,
    failure: &AgentFailure,
) -> Result<String, String> {
    let alternate_available = models.iter().any(|model| model != failed_model);
    let recovery_action = failure.recovery_action(alternate_available);
    match retry_policy {
        PromptRetryPolicy::FailFast => Err(format!(
            "step {step_id} failed under the fail-fast retry policy: {}",
            failure.message
        )),
        PromptRetryPolicy::SameModel if failure.should_retry() => Ok(failed_model.to_string()),
        PromptRetryPolicy::AlternateModel => {
            if recovery_action != AgentRecoveryAction::RetryAlternate {
                return Err(format!(
                    "step {step_id} cannot recover failure {} with an alternate model: {}",
                    failure.code, failure.message
                ));
            }
            let candidates = models
                .iter()
                .filter(|model| model.as_str() != failed_model)
                .collect::<Vec<_>>();
            candidates
                .get(recovery_attempt.saturating_sub(2) % candidates.len().max(1))
                .map(|model| (*model).clone())
                .ok_or_else(|| format!("no alternate model is available for failed step {step_id}"))
        }
        PromptRetryPolicy::SameModel => Err(format!(
            "step {step_id} cannot retry failure {} on the same model: {}",
            failure.code, failure.message
        )),
    }
}

fn adaptive_forbidden_models(
    spec: &AdaptiveCollaborationSpec,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> BTreeSet<String> {
    if spec.output_kind == WorkflowOutputKind::Verification {
        return checkpoint
            .plan
            .steps
            .iter()
            .filter(|step| {
                matches!(
                    step.contract.output_kind,
                    WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence
                )
            })
            .filter_map(|step| checkpoint.steps.get(&step.id))
            .filter(|step| {
                matches!(
                    step.status,
                    WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
                )
            })
            .map(|step| step.model.clone())
            .collect();
    }

    if matches!(
        spec.output_kind,
        WorkflowOutputKind::Analysis | WorkflowOutputKind::Evidence
    ) {
        return checkpoint
            .plan
            .steps
            .iter()
            .filter(|step| step.contract.output_kind == WorkflowOutputKind::Verification)
            .map(|step| step.model.clone())
            .collect();
    }

    BTreeSet::new()
}

pub(crate) fn adaptive_distinct_recovery_models(
    spec: &AdaptiveCollaborationSpec,
    checkpoint: &WorkflowExecutionCheckpoint,
    models: &[String],
) -> Vec<String> {
    let forbidden = adaptive_forbidden_models(spec, checkpoint);
    models
        .iter()
        .filter(|model| !forbidden.contains(model.as_str()))
        .cloned()
        .collect()
}

pub(crate) fn adaptive_worker_model_distinctness_error(
    spec: &AdaptiveCollaborationSpec,
    model: &str,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Option<String> {
    let forbidden = adaptive_forbidden_models(spec, checkpoint);
    forbidden.contains(model).then(|| {
        format!(
            "{ADAPTIVE_MODEL_DISTINCTNESS_ERROR_PREFIX}: step {} cannot use model {model} because the production Specialist and Independent Verifier must remain model-distinct",
            spec.step_id
        )
    })
}

pub(crate) struct AdaptiveWorkerRecoveryContext<'a, 'state> {
    pub(crate) app: &'a tauri::AppHandle,
    pub(crate) state: &'a tauri::State<'state, AppState>,
    pub(crate) config: &'a ProviderConfig,
    pub(crate) task_id: &'a TaskId,
    pub(crate) workspace_root: &'a Path,
    pub(crate) run_context: &'a Metadata,
    pub(crate) collaboration_id: &'a str,
    pub(crate) spec: &'a AdaptiveCollaborationSpec,
    pub(crate) failed: &'a CollaborationCompletion,
    pub(crate) failed_model: &'a str,
    pub(crate) replacement_model: &'a str,
    pub(crate) recovery_attempt: usize,
    pub(crate) retry_policy: PromptRetryPolicy,
    pub(crate) cancellation: Option<Arc<AgentRunControl>>,
}

pub(crate) fn recover_adaptive_worker(
    context: AdaptiveWorkerRecoveryContext<'_, '_>,
) -> Result<CollaborationCompletion, String> {
    let AdaptiveWorkerRecoveryContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        spec,
        failed,
        failed_model,
        replacement_model,
        recovery_attempt,
        retry_policy,
        cancellation,
    } = context;
    let failure = failed
        .error
        .as_deref()
        .unwrap_or("worker returned empty content");
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration worker retry planned",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("failed_step_id".to_string(), spec.step_id.clone()),
                    ("failed_model".to_string(), failed_model.to_string()),
                    (
                        "replacement_model".to_string(),
                        replacement_model.to_string(),
                    ),
                    (
                        "retry_policy".to_string(),
                        match retry_policy {
                            PromptRetryPolicy::FailFast => "fail_fast",
                            PromptRetryPolicy::SameModel => "same_model",
                            PromptRetryPolicy::AlternateModel => "alternate_model",
                        }
                        .to_string(),
                    ),
                    (
                        "failure".to_string(),
                        truncate_for_collaboration(failure, 1_000),
                    ),
                    (
                        "retry_attempt".to_string(),
                        recovery_attempt.saturating_sub(1).to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }

    let stage_suffix = if recovery_attempt <= 2 {
        String::new()
    } else {
        format!("_attempt_{recovery_attempt}")
    };
    if collaboration_steer_pending(cancellation.as_ref()) {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    if cancellation.as_ref().is_some_and(agent_run_should_stop) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let recovery_stage = format!("recovery_{}{}", spec.step_index + 1, stage_suffix);
    let recovery_request_id = unique_id("collaboration-recovery");
    let mut recovery_metadata = adaptive_stage_metadata(spec);
    recovery_metadata.insert("recovery".to_string(), "true".to_string());
    recovery_metadata.insert("failed_model".to_string(), failed_model.to_string());
    recovery_metadata.insert("recovery_attempt".to_string(), recovery_attempt.to_string());
    record_collaboration_stage_started(
        state,
        task_id,
        run_context,
        collaboration_id,
        &recovery_stage,
        &adaptive_model_role(&spec.role, &spec.output_kind),
        replacement_model,
        &recovery_request_id,
        adaptive_step_attribution(
            &spec.output_kind,
            &adaptive_model_role(&spec.role, &spec.output_kind),
        ),
        &recovery_metadata,
    )?;
    let recovered = complete_collaboration_worker_with_tools(
        app.clone(),
        config.clone(),
        task_id.clone(),
        workspace_root.to_path_buf(),
        run_context.clone(),
        collaboration_id.to_string(),
        recovery_stage.clone(),
        adaptive_model_role(&spec.role, &spec.output_kind),
        replacement_model.to_string(),
        spec.prompt.clone(),
        CollaborationWorkerAccess::new(spec.step_id.clone(), spec.tool_policy),
        spec.max_model_turns,
        spec.max_tool_calls,
        spec.max_output_tokens,
        cancellation,
        None,
    );
    if recovered.error.as_deref() == Some(COLLABORATION_STEER_INTERRUPTED) {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        collaboration_id,
        &recovery_stage,
        &adaptive_model_role(&spec.role, &spec.output_kind),
        replacement_model,
        &recovery_request_id,
        &recovered,
        adaptive_step_attribution(
            &spec.output_kind,
            &adaptive_model_role(&spec.role, &spec.output_kind),
        ),
        &recovery_metadata,
    )?;
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::{WorkflowPlanStep, WorkflowStepContract};

    fn plan_step(
        id: &str,
        model: &str,
        access: Vec<String>,
        output_kind: WorkflowOutputKind,
    ) -> WorkflowPlanStep {
        WorkflowPlanStep {
            id: id.to_string(),
            role: id.to_string(),
            model: model.to_string(),
            subtask: id.to_string(),
            access: access.clone(),
            tool_policy: WorkflowToolPolicy::None,
            contract: WorkflowStepContract {
                input_steps: access,
                output_kind,
                ..WorkflowStepContract::default()
            },
        }
    }

    fn runtime_spec(
        id: &str,
        model: &str,
        output_kind: WorkflowOutputKind,
    ) -> AdaptiveCollaborationSpec {
        AdaptiveCollaborationSpec {
            step_index: 0,
            step_id: id.to_string(),
            role: id.to_string(),
            stage: id.to_string(),
            model: model.to_string(),
            subtask: id.to_string(),
            prompt: id.to_string(),
            request_id: id.to_string(),
            access: Vec::new(),
            tool_policy: WorkflowToolPolicy::None,
            output_kind,
            max_attempts: 2,
            max_model_turns: 1,
            max_tool_calls: 0,
            max_output_tokens: 1_024,
        }
    }

    #[test]
    fn production_retries_keep_specialist_and_verifier_models_distinct() {
        let plan = WorkflowPlanIr {
            schema: WORKFLOW_IR_SCHEMA.to_string(),
            workflow_id: "model-distinct-runtime".to_string(),
            objective: "test".to_string(),
            effort: "pro".to_string(),
            policy: "adaptive".to_string(),
            coordinator_model: "planner".to_string(),
            prompt_profile: "baseline".to_string(),
            steps: vec![
                plan_step(
                    "specialist",
                    "planned-specialist",
                    Vec::new(),
                    WorkflowOutputKind::Evidence,
                ),
                plan_step(
                    "verifier",
                    "planned-verifier",
                    vec!["specialist".to_string()],
                    WorkflowOutputKind::Verification,
                ),
                plan_step(
                    "owner-handoff",
                    "planner",
                    vec!["verifier".to_string()],
                    WorkflowOutputKind::Synthesis,
                ),
            ],
            budget: WorkflowBudget {
                max_steps: 3,
                max_models: 3,
                max_model_turns_per_step: 2,
                max_tool_calls_per_step: 0,
                max_output_tokens_per_step: 1_024,
            },
        };
        let mut checkpoint = WorkflowExecutionCheckpoint::new("model-distinct-runtime", plan, 1);
        let completed_specialist = checkpoint.steps.get_mut("specialist").unwrap();
        completed_specialist.status = WorkflowStepStatus::Completed;
        completed_specialist.model = "actual-specialist".to_string();

        let specialist = runtime_spec(
            "specialist",
            "planned-specialist",
            WorkflowOutputKind::Evidence,
        );
        let verifier = runtime_spec(
            "verifier",
            "planned-verifier",
            WorkflowOutputKind::Verification,
        );
        let models = vec![
            "actual-specialist".to_string(),
            "planned-specialist".to_string(),
            "planned-verifier".to_string(),
        ];

        assert_eq!(
            adaptive_distinct_recovery_models(&specialist, &checkpoint, &models),
            vec![
                "actual-specialist".to_string(),
                "planned-specialist".to_string()
            ]
        );
        assert!(adaptive_worker_model_distinctness_error(
            &specialist,
            "planned-verifier",
            &checkpoint
        )
        .is_some());
        assert_eq!(
            adaptive_distinct_recovery_models(&verifier, &checkpoint, &models),
            vec![
                "planned-specialist".to_string(),
                "planned-verifier".to_string()
            ]
        );
        assert!(adaptive_worker_model_distinctness_error(
            &verifier,
            "actual-specialist",
            &checkpoint
        )
        .is_some());
    }
}
