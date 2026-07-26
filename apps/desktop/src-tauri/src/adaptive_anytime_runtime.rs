use super::*;

pub(crate) const PARTIAL_HANDOFF_CANDIDATE_ID: &str = "__partial_handoff";
pub(super) const DIRECT_ANCHOR_JOB_ID: usize = 0;
const DIRECT_ANCHOR_VERIFIER_JOB_ID: usize = 0;

pub(super) struct DirectAnchorVerifier {
    spec: AdaptiveCollaborationSpec,
    supervisor: ParallelJobSupervisor<CollaborationCompletion>,
}

pub(super) fn direct_anchor_spec(
    role_hints: &ConductorRoleHints,
    prompt: &str,
    shared_memory: &str,
) -> AdaptiveCollaborationSpec {
    let bounded_memory = if shared_memory.trim().is_empty() {
        "(none)".to_string()
    } else {
        truncate_for_collaboration(shared_memory, 8_000)
    };
    AdaptiveCollaborationSpec {
        step_index: 0,
        step_id: DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
        role: "direct_anchor".to_string(),
        stage: "direct_anchor".to_string(),
        model: role_hints.executor.clone(),
        subtask: "Produce an independent, immediately usable direct answer.".to_string(),
        prompt: direct_anchor_response_prompt(prompt, &bounded_memory),
        request_id: unique_id("collaboration-anchor"),
        access: Vec::new(),
        tool_policy: WorkflowToolPolicy::None,
        max_attempts: 1,
        max_model_turns: 1,
        max_tool_calls: 0,
        max_output_tokens: 2_048,
    }
}

pub(super) fn direct_anchor_verifier_spec(
    config: &ProviderConfig,
    user_prompt: &str,
    anchor_output: &str,
) -> AdaptiveCollaborationSpec {
    AdaptiveCollaborationSpec {
        step_index: 0,
        step_id: format!("{DIRECT_ANCHOR_CANDIDATE_ID}:verification"),
        role: "reviewer".to_string(),
        stage: "direct_anchor_verifier".to_string(),
        model: config.model_for_role(&ModelRole::Reviewer),
        subtask: "Independently verify the direct execution anchor.".to_string(),
        prompt: format!(
            "You are Cindx's independent direct-answer verifier. Judge whether the candidate is a correct, useful, safe user-facing answer to the exact request. Reject it when it drops constraints, substitutes a different objective, exposes internal orchestration, invents completed effects, makes unsupported claims, or omits decisive qualifications. For a task requiring tools, accept only a truthful partial result that clearly identifies what remains unverified. Return exactly one JSON object and no prose: {{\"pass\":true,\"score\":0.0,\"issues\":[\"...\"],\"safety_violations\":0}}. score must be between 0 and 1.\n\nUser request:\n{}\n\nDirect candidate answer:\n{}",
            truncate_for_collaboration(user_prompt, 12_000),
            truncate_for_collaboration(anchor_output, 14_000),
        ),
        request_id: unique_id("collaboration-anchor-verifier"),
        access: Vec::new(),
        tool_policy: WorkflowToolPolicy::None,
        max_attempts: 1,
        max_model_turns: 1,
        max_tool_calls: 0,
        max_output_tokens: 1_024,
    }
}

pub(super) fn direct_anchor_metadata(spec: &AdaptiveCollaborationSpec) -> Metadata {
    let mut metadata = adaptive_stage_metadata(spec);
    metadata.insert(
        "anytime_candidate_kind".to_string(),
        "direct_anchor".to_string(),
    );
    metadata
}

pub(super) fn direct_anchor_verifier_metadata(spec: &AdaptiveCollaborationSpec) -> Metadata {
    let mut metadata = adaptive_stage_metadata(spec);
    metadata.insert(
        "anytime_candidate_kind".to_string(),
        "verification".to_string(),
    );
    metadata.insert(
        "anytime_target_candidate".to_string(),
        DIRECT_ANCHOR_CANDIDATE_ID.to_string(),
    );
    metadata
}

pub(super) fn direct_anchor_verdict(
    _verification: PromptVerification,
    content: Option<&str>,
) -> AnytimeVerdict {
    let deliverable = content.is_some_and(|value| !value.trim().is_empty());
    direct_anchor_response_verdict(deliverable, false)
}

pub(super) fn initialize_anytime_controller(
    contract: &ConductorExecutionContract,
    _workflow: &orchestrator::AdaptiveWorkflow,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Result<AnytimeController, String> {
    AgentEngineSession::restore(contract, checkpoint.clone()).map(|session| session.into_parts().1)
}

pub(super) fn persist_anytime_controller(
    checkpoint: &mut WorkflowExecutionCheckpoint,
    controller: &AnytimeController,
) -> Result<(), String> {
    orchestrator::persist_anytime_controller(checkpoint, controller)
}

pub(super) fn collaboration_steer_pending(cancellation: Option<&Arc<AgentRunControl>>) -> bool {
    cancellation.is_some_and(|control| control.has_pending_steer())
}

pub(super) fn cancel_anytime_background(
    anchor: Option<&ParallelJobSupervisor<CollaborationCompletion>>,
    verifier: Option<&DirectAnchorVerifier>,
) {
    if let Some(anchor) = anchor {
        anchor.cancel_all();
    }
    if let Some(verifier) = verifier {
        verifier.supervisor.cancel_all();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pause_anytime_for_steer(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    checkpoint: &mut WorkflowExecutionCheckpoint,
    controller: &AnytimeController,
    anchor: Option<&ParallelJobSupervisor<CollaborationCompletion>>,
    verifier: Option<&DirectAnchorVerifier>,
) -> Result<(), String> {
    cancel_anytime_background(anchor, verifier);
    persist_anytime_controller(checkpoint, controller)?;
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Collaboration workflow paused for user steering",
        "steered",
        None,
        checkpoint,
    )
}

pub(crate) fn anytime_best_known_output(
    controller: &AnytimeController,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Option<(String, String, AnytimeVerdict)> {
    let best = controller.best()?;
    let output = checkpoint
        .anytime_outputs
        .get(&best.candidate_id)
        .filter(|output| !output.trim().is_empty())?;
    Some((
        best.candidate_id.clone(),
        output.clone(),
        best.verdict.clone(),
    ))
}

pub(crate) fn register_partial_handoff_candidate(
    controller: &mut AnytimeController,
    checkpoint: &mut WorkflowExecutionCheckpoint,
    handoff: &str,
    completed_steps: usize,
    failed_steps: usize,
    evidence_count: usize,
) -> Result<(), String> {
    if handoff.trim().is_empty() {
        return Ok(());
    }
    if controller.candidate(PARTIAL_HANDOFF_CANDIDATE_ID).is_none() {
        controller.register(AnytimeCandidate {
            id: PARTIAL_HANDOFF_CANDIDATE_ID.to_string(),
            kind: AnytimeCandidateKind::Synthesis,
            contribution_signature: None,
            commit_eligible: true,
            dependencies: Vec::new(),
            expected_uplift_bps: 6_000,
            uncertainty_bps: 3_000,
            evidence_gap_bps: 3_000,
            latency_risk_bps: 0,
            failure_risk_bps: 1_000,
            state: AnytimeCandidateState::Pending,
        })?;
    }
    controller_mark_running_if_pending(controller, PARTIAL_HANDOFF_CANDIDATE_ID)?;
    if controller
        .candidate(PARTIAL_HANDOFF_CANDIDATE_ID)
        .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Running)
    {
        let total_steps = completed_steps.saturating_add(failed_steps).max(1);
        let completion_bps = u16::try_from(
            completed_steps
                .saturating_mul(10_000)
                .saturating_div(total_steps),
        )
        .unwrap_or(10_000)
        .min(10_000);
        let quality_bps = 4_500u16.saturating_add(
            u16::try_from(u32::from(completion_bps).saturating_mul(2_500) / 10_000)
                .unwrap_or(2_500),
        );
        let coverage_bps = 4_500u16.saturating_add(
            u16::try_from(u32::from(completion_bps).saturating_mul(3_000) / 10_000)
                .unwrap_or(3_000),
        );
        controller.observe(
            PARTIAL_HANDOFF_CANDIDATE_ID,
            AnytimeVerdict {
                quality_bps,
                confidence_bps: 4_500u16
                    .saturating_add(u16::try_from(evidence_count.min(10) * 200).unwrap_or(2_000)),
                constraint_coverage_bps: coverage_bps,
                evidence_count,
                safety_violations: 0,
                deliverable: true,
                verified: false,
                anchor_uplift_bps: None,
            },
        )?;
    }
    checkpoint.anytime_outputs.insert(
        PARTIAL_HANDOFF_CANDIDATE_ID.to_string(),
        handoff.to_string(),
    );
    persist_anytime_controller(checkpoint, controller)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_direct_anchor_completion(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    spec: &AdaptiveCollaborationSpec,
    completion: &CollaborationCompletion,
    _verification: PromptVerification,
    cancellation: Option<&Arc<AgentRunControl>>,
) -> Result<Option<String>, String> {
    let role = adaptive_model_role(&spec.role);
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        collaboration_id,
        &spec.stage,
        &role,
        &spec.model,
        &spec.request_id,
        completion,
        &direct_anchor_metadata(spec),
    )?;
    let content = completion
        .content
        .as_ref()
        .filter(|content| !content.trim().is_empty())
        .cloned();
    if let (Some(control), Some(content)) = (cancellation, content.as_deref()) {
        control.record_best_known_result(
            "direct_anchor",
            content,
            ResultQuality::Substantive,
            0,
            false,
            false,
        );
    }
    Ok(content)
}

pub(super) fn parallel_completion_or_failure(
    completion: ParallelJobCompletion<CollaborationCompletion>,
) -> CollaborationCompletion {
    completion.result.unwrap_or_else(|error| {
        CollaborationCompletion::failed(format!("direct anchor failed: {error}"))
    })
}

pub(super) fn controller_mark_running_if_pending(
    controller: &mut AnytimeController,
    candidate_id: &str,
) -> Result<(), String> {
    if controller
        .candidate(candidate_id)
        .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Pending)
    {
        controller.mark_running(candidate_id)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn settle_direct_anchor_candidate(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    spec: &AdaptiveCollaborationSpec,
    completion: &CollaborationCompletion,
    verification: PromptVerification,
    cancellation: Option<&Arc<AgentRunControl>>,
    controller: &mut AnytimeController,
    checkpoint: &mut WorkflowExecutionCheckpoint,
) -> Result<Option<String>, String> {
    let content = record_direct_anchor_completion(
        state,
        task_id,
        run_context,
        collaboration_id,
        spec,
        completion,
        verification,
        cancellation,
    )?;
    if controller
        .candidate(DIRECT_ANCHOR_CANDIDATE_ID)
        .is_some_and(|candidate| !candidate.state.is_terminal())
    {
        controller_mark_running_if_pending(controller, DIRECT_ANCHOR_CANDIDATE_ID)?;
        if let Some(output) = content.as_ref() {
            controller.observe(
                DIRECT_ANCHOR_CANDIDATE_ID,
                direct_anchor_verdict(verification, Some(output)),
            )?;
        } else {
            controller.fail(DIRECT_ANCHOR_CANDIDATE_ID)?;
        }
    }
    if let Some(output) = content.as_ref() {
        checkpoint
            .anytime_outputs
            .insert(DIRECT_ANCHOR_CANDIDATE_ID.to_string(), output.clone());
    }
    persist_anytime_controller(checkpoint, controller)?;
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Anytime direct anchor checkpointed",
        if content.is_some() {
            "usable"
        } else {
            "failed"
        },
        None,
        checkpoint,
    )?;
    Ok(content)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start_direct_anchor_verifier(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    collaboration_id: &str,
    user_prompt: &str,
    anchor_output: &str,
    cancellation: Option<Arc<AgentRunControl>>,
) -> Result<DirectAnchorVerifier, String> {
    let spec = direct_anchor_verifier_spec(config, user_prompt, anchor_output);
    let role = adaptive_model_role(&spec.role);
    record_collaboration_stage_started(
        state,
        task_id,
        run_context,
        collaboration_id,
        &spec.stage,
        &role,
        &spec.model,
        &spec.request_id,
        &direct_anchor_verifier_metadata(&spec),
    )?;

    let mut supervisor = model_job_supervisor::<CollaborationCompletion>();
    let app = app.clone();
    let config = config.clone();
    let task_id = task_id.clone();
    let workspace_root = workspace_root.to_path_buf();
    let run_context = run_context.clone();
    let collaboration_id = collaboration_id.to_string();
    let stage = spec.stage.clone();
    let model = spec.model.clone();
    let prompt = spec.prompt.clone();
    let max_model_turns = spec.max_model_turns;
    let max_tool_calls = spec.max_tool_calls;
    let max_output_tokens = spec.max_output_tokens;
    supervisor
        .submit(
            DIRECT_ANCHOR_VERIFIER_JOB_ID,
            "direct-anchor-verifier",
            Box::new(move |branch_cancellation| {
                complete_collaboration_worker_with_tools(
                    app,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    stage,
                    ModelRole::Reviewer,
                    model,
                    prompt,
                    false,
                    max_model_turns,
                    max_tool_calls,
                    max_output_tokens,
                    cancellation,
                    Some(branch_cancellation),
                )
            }),
        )
        .map_err(|error| format!("direct anchor verifier could not start: {error}"))?;
    Ok(DirectAnchorVerifier { spec, supervisor })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record_direct_anchor_verifier_unavailable(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    error: &str,
) {
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Direct anchor verifier unavailable",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    (
                        "verification_error".to_string(),
                        truncate_for_collaboration(error, 2_000),
                    ),
                    ("fail_soft".to_string(), "true".to_string()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn settle_direct_anchor_verifier(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    verifier: &DirectAnchorVerifier,
    completion: &CollaborationCompletion,
    anchor_output: &str,
    cancellation: Option<&Arc<AgentRunControl>>,
    controller: &mut AnytimeController,
    checkpoint: &mut WorkflowExecutionCheckpoint,
) -> Result<(), String> {
    let role = adaptive_model_role(&verifier.spec.role);
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        collaboration_id,
        &verifier.spec.stage,
        &role,
        &verifier.spec.model,
        &verifier.spec.request_id,
        completion,
        &direct_anchor_verifier_metadata(&verifier.spec),
    )?;

    let raw_gate = match completion
        .content
        .as_deref()
        .filter(|content| !content.trim().is_empty())
    {
        Some(content) => content,
        None => {
            record_direct_anchor_verifier_unavailable(
                state,
                task_id,
                run_context,
                collaboration_id,
                completion
                    .error
                    .as_deref()
                    .unwrap_or("reviewer returned no verdict"),
            );
            return Ok(());
        }
    };
    let gate = match parse_collaboration_quality(raw_gate) {
        Ok(gate) => gate,
        Err(error) => {
            record_direct_anchor_verifier_unavailable(
                state,
                task_id,
                run_context,
                collaboration_id,
                &error,
            );
            return Ok(());
        }
    };
    let score_bps = if gate.score.is_finite() {
        (gate.score.clamp(0.0, 1.0) * 10_000.0).round() as u16
    } else {
        0
    };
    let passed = adaptive_quality_gate_passes(&gate);
    let verdict = AnytimeVerdict {
        quality_bps: score_bps,
        confidence_bps: 8_000,
        constraint_coverage_bps: score_bps,
        evidence_count: 1,
        safety_violations: gate.safety_violations,
        deliverable: !anchor_output.trim().is_empty(),
        verified: passed,
        anchor_uplift_bps: None,
    };
    if controller
        .candidate(DIRECT_ANCHOR_CANDIDATE_ID)
        .is_some_and(|candidate| {
            matches!(
                candidate.state,
                AnytimeCandidateState::Usable | AnytimeCandidateState::Verified
            )
        })
    {
        controller.revise(DIRECT_ANCHOR_CANDIDATE_ID, verdict)?;
    }
    if let Some(control) = cancellation {
        control.record_best_known_result(
            "direct_anchor_verifier",
            anchor_output,
            adaptive_quality_result_quality(&gate),
            1,
            passed,
            false,
        );
    }
    persist_anytime_controller(checkpoint, controller)?;
    append_workflow_checkpoint_event(
        state,
        task_id,
        run_context,
        collaboration_id,
        "Anytime direct anchor independently reviewed",
        if passed { "verified" } else { "rejected" },
        None,
        checkpoint,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn advance_direct_anchor_background(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    collaboration_id: &str,
    user_prompt: &str,
    verification: PromptVerification,
    cancellation: Option<&Arc<AgentRunControl>>,
    anchor_spec: &AdaptiveCollaborationSpec,
    anchor_supervisor: &mut Option<ParallelJobSupervisor<CollaborationCompletion>>,
    anchor_output: &mut Option<String>,
    verifier: &mut Option<DirectAnchorVerifier>,
    verifier_attempted: &mut bool,
    controller: &mut AnytimeController,
    checkpoint: &mut WorkflowExecutionCheckpoint,
) -> Result<(), String> {
    if anchor_output.is_none() {
        if let Some(completion) = anchor_supervisor
            .as_mut()
            .and_then(ParallelJobSupervisor::try_recv)
        {
            let completion = parallel_completion_or_failure(completion);
            *anchor_output = settle_direct_anchor_candidate(
                state,
                task_id,
                run_context,
                collaboration_id,
                anchor_spec,
                &completion,
                verification,
                cancellation,
                controller,
                checkpoint,
            )?;
        }
    }

    if verification != PromptVerification::Minimal
        && !*verifier_attempted
        && anchor_output.is_some()
        && controller
            .candidate(DIRECT_ANCHOR_CANDIDATE_ID)
            .is_some_and(|candidate| candidate.state == AnytimeCandidateState::Usable)
    {
        *verifier_attempted = true;
        let output = anchor_output.as_deref().unwrap_or_default();
        match start_direct_anchor_verifier(
            app,
            state,
            config,
            task_id,
            workspace_root,
            run_context,
            collaboration_id,
            user_prompt,
            output,
            cancellation.cloned(),
        ) {
            Ok(started) => *verifier = Some(started),
            Err(error) => record_direct_anchor_verifier_unavailable(
                state,
                task_id,
                run_context,
                collaboration_id,
                &error,
            ),
        }
    }

    let completion = verifier
        .as_mut()
        .and_then(|verifier| verifier.supervisor.try_recv());
    if let Some(completion) = completion {
        if let Some(verifier) = verifier.take() {
            let completion = parallel_completion_or_failure(completion);
            if let Some(output) = anchor_output.as_deref() {
                settle_direct_anchor_verifier(
                    state,
                    task_id,
                    run_context,
                    collaboration_id,
                    &verifier,
                    &completion,
                    output,
                    cancellation,
                    controller,
                    checkpoint,
                )?;
            }
        }
    }
    Ok(())
}

pub(super) fn direct_anchor_should_commit(
    controller: &AnytimeController,
    cancellation: Option<&Arc<AgentRunControl>>,
) -> bool {
    let (remaining_ms, terminal_reserve_ms) = cancellation.map_or((u64::MAX, 0), |control| {
        let progress = control.progress();
        let budget = control.budget();
        (
            u64::try_from(progress.remaining.as_millis()).unwrap_or(u64::MAX),
            u64::try_from(budget.terminal_time_reserve.as_millis()).unwrap_or(u64::MAX),
        )
    });
    matches!(
        controller.decision(remaining_ms, terminal_reserve_ms),
        AnytimeDecision::Commit { ref candidate_id }
            if candidate_id == DIRECT_ANCHOR_CANDIDATE_ID
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn await_direct_anchor_fallback(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    spec: &AdaptiveCollaborationSpec,
    verification: PromptVerification,
    cancellation: Option<&Arc<AgentRunControl>>,
    supervisor: &mut Option<ParallelJobSupervisor<CollaborationCompletion>>,
    current_output: &mut Option<String>,
    timeout: Duration,
) -> Result<Option<String>, String> {
    if current_output.is_some() {
        return Ok(current_output.clone());
    }
    let Some(supervisor) = supervisor.as_mut() else {
        return Ok(None);
    };
    if collaboration_steer_pending(cancellation) {
        supervisor.cancel_all();
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let completion = if timeout.is_zero() {
        supervisor.try_recv()
    } else {
        let started = std::time::Instant::now();
        loop {
            if collaboration_steer_pending(cancellation) {
                supervisor.cancel_all();
                return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break None;
            }
            if let Some(completion) =
                supervisor.recv_timeout(remaining.min(Duration::from_millis(20)))
            {
                break Some(completion);
            }
        }
    };
    let Some(completion) = completion else {
        return Ok(None);
    };
    let completion = parallel_completion_or_failure(completion);
    *current_output = record_direct_anchor_completion(
        state,
        task_id,
        run_context,
        collaboration_id,
        spec,
        &completion,
        verification,
        cancellation,
    )?;
    Ok(current_output.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_policy_does_not_masquerade_as_completed_verification() {
        let verdict = direct_anchor_verdict(PromptVerification::Minimal, Some("usable answer"));

        assert!(verdict.deliverable);
        assert!(!verdict.verified);
    }
}
