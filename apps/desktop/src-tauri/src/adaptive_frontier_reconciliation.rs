use super::*;

pub(super) enum AdaptiveWaveReconciliationOutcome {
    Continue,
    Commit(String),
}

pub(super) struct AdaptiveWaveReconciliationContext<'a, 'state> {
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
    pub(super) anchor_spec: &'a AdaptiveCollaborationSpec,
    pub(super) final_step_id: &'a str,
    pub(super) workflow_started_at_ms: u64,
    pub(super) wave: &'a AdaptiveWave,
    pub(super) completion: AdaptiveWaveCompletion,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
    pub(super) anytime_controller: &'a mut AnytimeController,
    pub(super) anchor_supervisor: &'a mut Option<ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) direct_anchor_output: &'a mut Option<String>,
    pub(super) direct_anchor_verifier: &'a mut Option<DirectAnchorVerifier>,
    pub(super) direct_anchor_verifier_attempted: &'a mut bool,
    pub(super) outputs: &'a mut BTreeMap<String, String>,
    pub(super) evidence_by_step: &'a mut BTreeMap<String, Vec<CollaborationEvidence>>,
}

pub(super) fn reconcile_adaptive_wave(
    context: AdaptiveWaveReconciliationContext<'_, '_>,
) -> Result<AdaptiveWaveReconciliationOutcome, String> {
    let AdaptiveWaveReconciliationContext {
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
        anchor_spec,
        final_step_id,
        workflow_started_at_ms,
        wave,
        completion,
        workflow_checkpoint,
        anytime_controller,
        anchor_supervisor,
        direct_anchor_output,
        direct_anchor_verifier,
        direct_anchor_verifier_attempted,
        outputs,
        evidence_by_step,
    } = context;
    let cancellation = completion.cancellation;
    let mut layer_failures = Vec::new();

    for (spec, completion) in wave.specs.iter().zip(completion.completions) {
        let settlement = settle_adaptive_step(
            AdaptiveStepRecoveryContext {
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
                cancellation: cancellation.clone(),
                spec,
                workflow_checkpoint,
                anytime_controller,
                anchor_supervisor: anchor_supervisor.as_ref(),
                direct_anchor_verifier: direct_anchor_verifier.as_ref(),
            },
            completion,
        )?;
        let AdaptiveStepSettlement::Completed(recovered) = settlement else {
            if let AdaptiveStepSettlement::Failed(error) = settlement {
                layer_failures.push(error);
            }
            continue;
        };
        let shared_evidence =
            merge_collaboration_evidence(&spec.access, evidence_by_step, &recovered.evidence);
        let step_output = collaboration_step_result(
            &spec.step_id,
            &recovered.model,
            &recovered.content,
            &shared_evidence,
        );
        let evidence_json = serde_json::to_string(&shared_evidence)
            .map_err(|error| format!("workflow evidence serialization failed: {error}"))?;
        workflow_checkpoint.complete_step(
            &spec.step_id,
            &recovered.model,
            step_output.clone(),
            evidence_json,
            current_time_millis(),
        )?;
        workflow_checkpoint.record_step_metrics(
            &spec.step_id,
            recovered.latency_ms,
            recovered.tokens,
        )?;
        workflow_checkpoint
            .anytime_outputs
            .insert(spec.step_id.clone(), step_output.clone());
        outputs.insert(spec.step_id.clone(), step_output);
        evidence_by_step.insert(spec.step_id.clone(), shared_evidence);
        append_workflow_checkpoint_event(
            state,
            task_id,
            run_context,
            collaboration_id,
            "Collaboration workflow step checkpointed",
            "completed",
            Some(&spec.step_id),
            workflow_checkpoint,
        )?;
    }

    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    advance_direct_anchor_background(
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        prompt,
        prompt_genome.verification,
        cancellation.as_ref(),
        anchor_spec,
        anchor_supervisor,
        direct_anchor_output,
        direct_anchor_verifier,
        direct_anchor_verifier_attempted,
        anytime_controller,
        workflow_checkpoint,
    )?;
    if collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
            anchor_supervisor.as_ref(),
            direct_anchor_verifier.as_ref(),
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    if direct_anchor_should_commit(anytime_controller, cancellation.as_ref()) {
        if let Some(output) = direct_anchor_output.as_ref() {
            cancel_anytime_background(anchor_supervisor.as_ref(), direct_anchor_verifier.as_ref());
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                "Anytime terminal reserve committed best-known result",
                "completed",
                None,
                workflow_checkpoint,
            )?;
            return Ok(AdaptiveWaveReconciliationOutcome::Commit(output.clone()));
        }
    }

    let successful_steps = wave
        .specs
        .iter()
        .filter(|spec| {
            workflow_checkpoint
                .steps
                .get(&spec.step_id)
                .is_some_and(|step| step.status == WorkflowStepStatus::Completed)
        })
        .count();
    if wave.independent && !layer_failures.is_empty() && successful_steps >= wave.required_successes
    {
        degrade_failed_quorum_branches(DegradedQuorumContext {
            state,
            task_id,
            run_context,
            collaboration_id,
            wave,
            workflow_checkpoint,
            outputs,
            successful_steps,
            degraded_count: layer_failures.len(),
        })?;
        layer_failures.clear();
    }

    observe_wave_results(wave, final_step_id, workflow_checkpoint, anytime_controller)?;
    persist_anytime_controller(workflow_checkpoint, anytime_controller)?;

    if let Some(error) = adaptive_layer_failure_error(&layer_failures) {
        let partial_handoff = adaptive_partial_work_handoff(prompt, outputs, &layer_failures);
        if let Some(handoff) = partial_handoff.as_ref() {
            register_partial_handoff_candidate(
                anytime_controller,
                workflow_checkpoint,
                handoff,
                outputs.len(),
                layer_failures.len(),
                evidence_by_step.values().flatten().count(),
            )?;
            append_workflow_checkpoint_event(
                state,
                task_id,
                run_context,
                collaboration_id,
                "Collaboration workflow checkpoint preserved",
                "degraded",
                None,
                workflow_checkpoint,
            )?;
        }
        if direct_anchor_output.is_none() {
            if let Some(completion) = anchor_supervisor
                .as_mut()
                .and_then(|supervisor| supervisor.recv_timeout(Duration::from_millis(500)))
            {
                let completion = parallel_completion_or_failure(completion);
                let _ = settle_direct_anchor_candidate(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    anchor_spec,
                    &completion,
                    prompt_genome.verification,
                    cancellation.as_ref(),
                    anytime_controller,
                    workflow_checkpoint,
                )?;
            }
        }
        if let Some((candidate_id, output, verdict)) =
            anytime_best_known_output(anytime_controller, workflow_checkpoint)
        {
            record_failure_commit(
                state,
                task_id,
                run_context,
                collaboration_id,
                workflow_started_at_ms,
                outputs.len(),
                layer_failures.len(),
                &candidate_id,
                &output,
                &verdict,
                &error,
                cancellation.as_ref(),
                workflow_checkpoint,
            )?;
            return Ok(AdaptiveWaveReconciliationOutcome::Commit(output));
        }
        if let Some(handoff) = partial_handoff {
            return Ok(AdaptiveWaveReconciliationOutcome::Commit(handoff));
        }
        return Err(error);
    }
    if cancellation.as_ref().is_some_and(agent_run_should_stop) {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    Ok(AdaptiveWaveReconciliationOutcome::Continue)
}

struct DegradedQuorumContext<'a, 'state> {
    state: &'a tauri::State<'state, AppState>,
    task_id: &'a TaskId,
    run_context: &'a Metadata,
    collaboration_id: &'a str,
    wave: &'a AdaptiveWave,
    workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
    outputs: &'a mut BTreeMap<String, String>,
    successful_steps: usize,
    degraded_count: usize,
}

fn degrade_failed_quorum_branches(context: DegradedQuorumContext<'_, '_>) -> Result<(), String> {
    let DegradedQuorumContext {
        state,
        task_id,
        run_context,
        collaboration_id,
        wave,
        workflow_checkpoint,
        outputs,
        successful_steps,
        degraded_count,
    } = context;
    for spec in &wave.specs {
        let Some(step) = workflow_checkpoint.steps.get(&spec.step_id) else {
            continue;
        };
        if step.status != WorkflowStepStatus::Failed {
            continue;
        }
        let error = step
            .error
            .clone()
            .unwrap_or_else(|| "worker returned no usable result".to_string());
        let degraded = adaptive_degraded_branch_output(&spec.step_id, &error);
        workflow_checkpoint.degrade_step(
            &spec.step_id,
            degraded.clone(),
            error,
            current_time_millis(),
        )?;
        workflow_checkpoint
            .anytime_outputs
            .insert(spec.step_id.clone(), degraded.clone());
        outputs.insert(spec.step_id.clone(), degraded);
        append_workflow_checkpoint_event(
            state,
            task_id,
            run_context,
            collaboration_id,
            "Collaboration workflow branch degraded",
            "degraded",
            Some(&spec.step_id),
            workflow_checkpoint,
        )?;
    }
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration quorum preserved",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "successful_branches".to_string(),
                        successful_steps.to_string(),
                    ),
                    (
                        "required_branches".to_string(),
                        wave.required_successes.to_string(),
                    ),
                    ("degraded_branches".to_string(), degraded_count.to_string()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
    Ok(())
}

fn observe_wave_results(
    wave: &AdaptiveWave,
    final_step_id: &str,
    workflow_checkpoint: &WorkflowExecutionCheckpoint,
    anytime_controller: &mut AnytimeController,
) -> Result<(), String> {
    for spec in &wave.specs {
        let Some(step_checkpoint) = workflow_checkpoint.steps.get(&spec.step_id) else {
            continue;
        };
        if anytime_controller
            .candidate(&spec.step_id)
            .is_none_or(|candidate| candidate.state != AnytimeCandidateState::Running)
        {
            continue;
        }
        match step_checkpoint.status {
            WorkflowStepStatus::Completed if spec.step_id != final_step_id => {
                anytime_controller.observe(
                    &spec.step_id,
                    AnytimeVerdict {
                        quality_bps: 6_500,
                        confidence_bps: 6_000,
                        constraint_coverage_bps: 6_500,
                        evidence_count: step_checkpoint.evidence_count,
                        safety_violations: 0,
                        deliverable: step_checkpoint
                            .output
                            .as_ref()
                            .is_some_and(|output| !output.trim().is_empty()),
                        verified: false,
                        anchor_uplift_bps: None,
                    },
                )?;
            }
            WorkflowStepStatus::Degraded if spec.step_id != final_step_id => {
                anytime_controller.observe(
                    &spec.step_id,
                    AnytimeVerdict {
                        quality_bps: 4_750,
                        confidence_bps: 4_500,
                        constraint_coverage_bps: 5_000,
                        evidence_count: step_checkpoint.evidence_count,
                        safety_violations: 0,
                        deliverable: step_checkpoint
                            .output
                            .as_ref()
                            .is_some_and(|output| !output.trim().is_empty()),
                        verified: false,
                        anchor_uplift_bps: None,
                    },
                )?;
            }
            WorkflowStepStatus::Failed => anytime_controller.fail(&spec.step_id)?,
            WorkflowStepStatus::Pending
            | WorkflowStepStatus::Running
            | WorkflowStepStatus::Completed
            | WorkflowStepStatus::Degraded => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_failure_commit(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    workflow_started_at_ms: u64,
    completed_steps: usize,
    failed_steps: usize,
    candidate_id: &str,
    output: &str,
    verdict: &AnytimeVerdict,
    error: &str,
    cancellation: Option<&Arc<AgentRunControl>>,
    workflow_checkpoint: &WorkflowExecutionCheckpoint,
) -> Result<(), String> {
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Anytime workflow failure committed best-known result",
        "degraded",
        None,
        workflow_checkpoint,
    )?;
    if let Some(control) = cancellation {
        control.record_best_known_result(
            &format!("anytime_failure:{candidate_id}"),
            output,
            if verdict.verified {
                ResultQuality::Verified
            } else {
                ResultQuality::Grounded
            },
            verdict.evidence_count,
            verdict.verified,
            false,
        );
    }
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Collaboration workflow failed",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "workflow_schema".to_string(),
                        WORKFLOW_IR_SCHEMA.to_string(),
                    ),
                    ("status".to_string(), "degraded".to_string()),
                    ("fallback_used".to_string(), "true".to_string()),
                    ("fallback_candidate".to_string(), candidate_id.to_string()),
                    ("completed_steps".to_string(), completed_steps.to_string()),
                    ("failed_steps".to_string(), failed_steps.to_string()),
                    (
                        "error".to_string(),
                        truncate_for_collaboration(error, 2_000),
                    ),
                    (
                        "latency_ms".to_string(),
                        current_time_millis()
                            .saturating_sub(workflow_started_at_ms)
                            .to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
    Ok(())
}
