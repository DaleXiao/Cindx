use super::*;

pub(super) enum AdaptiveStepSettlement {
    Completed(AdaptiveRecoveredStep),
    Failed(String),
    Paused,
}

pub(super) struct AdaptiveRecoveredStep {
    pub(super) model: String,
    pub(super) content: String,
    pub(super) evidence: Vec<CollaborationEvidence>,
    pub(super) latency_ms: u64,
    pub(super) tokens: u64,
}

pub(super) struct AdaptiveStepRecoveryContext<'a, 'state> {
    pub(super) app: &'a tauri::AppHandle,
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) workspace_root: &'a Path,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) models: &'a [String],
    pub(super) prompt_genome: &'a ConductorPromptGenome,
    pub(super) cancellation: Option<Arc<AgentRunControl>>,
    pub(super) spec: &'a AdaptiveCollaborationSpec,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
    pub(super) anytime_controller: &'a AnytimeController,
    pub(super) anchor_supervisor: Option<&'a ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) direct_anchor_verifier: Option<&'a DirectAnchorVerifier>,
}

pub(super) fn settle_adaptive_step(
    context: AdaptiveStepRecoveryContext<'_, '_>,
    completion: CollaborationCompletion,
) -> Result<AdaptiveStepSettlement, String> {
    let AdaptiveStepRecoveryContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        prompt,
        models,
        prompt_genome,
        cancellation,
        spec,
        workflow_checkpoint,
        anytime_controller,
        anchor_supervisor,
        direct_anchor_verifier,
    } = context;

    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
            anchor_supervisor,
            direct_anchor_verifier,
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let metadata = adaptive_stage_metadata(spec);
    let role = adaptive_model_role(&spec.role, &spec.output_kind);
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        collaboration_id,
        &spec.stage,
        &role,
        &spec.model,
        &spec.request_id,
        &completion,
        &metadata,
    )?;
    if completion
        .content
        .as_ref()
        .is_none_or(|content| content.trim().is_empty())
        && cancellation.as_ref().is_some_and(agent_run_should_stop)
    {
        workflow_checkpoint.fail_step(
            &spec.step_id,
            MODEL_REQUEST_CANCELLED,
            current_time_millis(),
        )?;
        append_workflow_checkpoint_event(
            state,
            task_id,
            run_context,
            collaboration_id,
            "Collaboration workflow step paused",
            "paused",
            Some(&spec.step_id),
            workflow_checkpoint,
        )?;
        return Ok(AdaptiveStepSettlement::Paused);
    }

    let mut effective_completion = completion;
    let mut completed_model = spec.model.clone();
    let mut accumulated_evidence = Vec::new();
    let mut accumulated_latency_ms = 0u64;
    let mut accumulated_tokens = 0u64;
    loop {
        accumulated_latency_ms =
            accumulated_latency_ms.saturating_add(effective_completion.latency_ms);
        accumulated_tokens = accumulated_tokens.saturating_add(
            effective_completion
                .usage
                .get("total_tokens")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default(),
        );
        accumulated_evidence.extend(effective_completion.evidence.iter().cloned());
        if effective_completion
            .content
            .as_ref()
            .is_some_and(|content| !content.trim().is_empty())
        {
            break;
        }

        if let (Some(control), Some(partial)) = (
            cancellation.as_ref(),
            effective_completion.best_available_content(),
        ) {
            control.record_best_known_result_at(
                run_context_steer_epoch(run_context),
                &spec.stage,
                partial,
                ResultQuality::Draft,
                accumulated_evidence.len(),
                false,
                false,
            );
        }

        let failure = effective_completion.failure_or_empty_output();
        let attempts = workflow_checkpoint
            .steps
            .get(&spec.step_id)
            .map(|step| step.attempts)
            .unwrap_or_default();
        if attempts >= spec.max_attempts {
            let error = format!(
                "step {} exhausted {} attempts; last failure: {}",
                spec.step_id,
                spec.max_attempts,
                effective_completion
                    .error
                    .as_deref()
                    .unwrap_or("worker returned empty content")
            );
            fail_adaptive_step(
                state,
                task_id,
                run_context,
                collaboration_id,
                workflow_checkpoint,
                &spec.step_id,
                &error,
            )?;
            return Ok(AdaptiveStepSettlement::Failed(format!(
                "step {} failed after recovery: {error}",
                spec.step_id
            )));
        }

        let recovery_attempt = attempts.saturating_add(1);
        let replacement_model = match adaptive_recovery_model(
            &spec.step_id,
            &completed_model,
            recovery_attempt,
            models,
            prompt_genome.retry_policy,
            &failure,
        ) {
            Ok(model) => model,
            Err(error) => {
                fail_adaptive_step(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    workflow_checkpoint,
                    &spec.step_id,
                    &error,
                )?;
                return Ok(AdaptiveStepSettlement::Failed(format!(
                    "step {} failed after recovery: {error}",
                    spec.step_id
                )));
            }
        };
        workflow_checkpoint.begin_step_with_attempt_limit(
            &spec.step_id,
            &replacement_model,
            spec.max_attempts,
            current_time_millis(),
        )?;
        append_workflow_checkpoint_event(
            state,
            task_id,
            run_context,
            collaboration_id,
            "Collaboration workflow recovery started",
            "running",
            Some(&spec.step_id),
            workflow_checkpoint,
        )?;
        match recover_adaptive_worker(AdaptiveWorkerRecoveryContext {
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            collaboration_id,
            user_prompt: prompt,
            spec,
            failed: &effective_completion,
            failed_model: &completed_model,
            replacement_model: &replacement_model,
            recovery_attempt,
            retry_policy: prompt_genome.retry_policy,
            cancellation: cancellation.clone(),
        }) {
            Ok(recovered) => {
                effective_completion = recovered;
                completed_model = replacement_model;
            }
            Err(error) => {
                if error == COLLABORATION_STEER_INTERRUPTED
                    || collaboration_steer_pending(cancellation.as_ref())
                {
                    pause_anytime_for_steer(
                        state,
                        task_id,
                        run_context,
                        collaboration_id,
                        workflow_checkpoint,
                        anytime_controller,
                        anchor_supervisor,
                        direct_anchor_verifier,
                    )?;
                    return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
                }
                fail_adaptive_step(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    workflow_checkpoint,
                    &spec.step_id,
                    &error,
                )?;
                return Ok(AdaptiveStepSettlement::Failed(format!(
                    "step {} failed after recovery: {error}",
                    spec.step_id
                )));
            }
        }
    }

    let content = effective_completion
        .content
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| format!("adaptive step {} completed without content", spec.step_id))?;
    Ok(AdaptiveStepSettlement::Completed(AdaptiveRecoveredStep {
        model: completed_model,
        content,
        evidence: accumulated_evidence,
        latency_ms: accumulated_latency_ms,
        tokens: accumulated_tokens,
    }))
}

fn fail_adaptive_step(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    checkpoint: &mut WorkflowExecutionCheckpoint,
    step_id: &str,
    error: &str,
) -> Result<(), String> {
    checkpoint.fail_step(step_id, error, current_time_millis())?;
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Collaboration workflow step failed",
        "failed",
        Some(step_id),
        checkpoint,
    )
}
