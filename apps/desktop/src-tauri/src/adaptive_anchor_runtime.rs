use super::*;

pub(super) enum AdaptiveAnchorOutcome {
    Continue(AdaptiveAnchorState),
    Commit(String),
}

pub(super) struct AdaptiveAnchorState {
    pub(super) output: Option<String>,
    pub(super) supervisor: Option<ParallelJobSupervisor<CollaborationCompletion>>,
    pub(super) attempted: bool,
}

pub(super) struct AdaptiveAnchorContext<'a, 'state> {
    pub(super) app: &'a tauri::AppHandle,
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) workspace_root: &'a Path,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) effort: &'a str,
    pub(super) resumed_from_checkpoint: bool,
    pub(super) execution_contract: &'a ConductorExecutionContract,
    pub(super) prompt_genome: &'a ConductorPromptGenome,
    pub(super) anchor_spec: &'a AdaptiveCollaborationSpec,
    pub(super) checkpoint: Option<&'a WorkflowExecutionCheckpoint>,
    pub(super) cancellation: Option<Arc<AgentRunControl>>,
}

pub(super) fn start_adaptive_anchor(
    context: AdaptiveAnchorContext<'_, '_>,
) -> Result<AdaptiveAnchorOutcome, String> {
    let AdaptiveAnchorContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        effort,
        resumed_from_checkpoint,
        execution_contract,
        prompt_genome,
        anchor_spec,
        checkpoint,
        cancellation,
    } = context;
    let mut output = checkpoint.and_then(|checkpoint| {
        checkpoint
            .anytime_outputs
            .get(DIRECT_ANCHOR_CANDIDATE_ID)
            .cloned()
    });
    let mut supervisor = None;
    let mut attempted = false;
    if output.is_some() {
        return Ok(AdaptiveAnchorOutcome::Continue(AdaptiveAnchorState {
            output,
            supervisor,
            attempted,
        }));
    }

    let anchor_role = adaptive_model_role(&anchor_spec.role);
    record_collaboration_stage_started(
        state,
        task_id,
        run_context,
        collaboration_id,
        &anchor_spec.stage,
        &anchor_role,
        &anchor_spec.model,
        &anchor_spec.request_id,
        &direct_anchor_metadata(anchor_spec),
    )?;
    if effort == "fast" && !resumed_from_checkpoint {
        attempted = true;
        let completion = complete_collaboration_worker_with_tools(
            app.clone(),
            config.clone(),
            task_id.clone(),
            workspace_root.to_path_buf(),
            run_context.clone(),
            collaboration_id.to_string(),
            anchor_spec.stage.clone(),
            anchor_role,
            anchor_spec.model.clone(),
            anchor_spec.prompt.clone(),
            false,
            anchor_spec.max_model_turns,
            anchor_spec.max_tool_calls,
            anchor_spec.max_output_tokens,
            cancellation.clone(),
            None,
        );
        if collaboration_steer_pending(cancellation.as_ref()) {
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        output = record_direct_anchor_completion(
            state,
            task_id,
            run_context,
            collaboration_id,
            anchor_spec,
            &completion,
            prompt_genome.verification,
            cancellation.as_ref(),
        )?;
        if let Some(output) = output.as_ref() {
            let mut controller =
                AnytimeController::new(AnytimeControllerConfig::from_contract(execution_contract));
            controller.register(AnytimeCandidate::direct_anchor(DIRECT_ANCHOR_CANDIDATE_ID))?;
            controller.mark_running(DIRECT_ANCHOR_CANDIDATE_ID)?;
            controller.observe(
                DIRECT_ANCHOR_CANDIDATE_ID,
                direct_anchor_verdict(prompt_genome.verification, Some(output)),
            )?;
            if matches!(
                controller.decision(u64::MAX, 0),
                AnytimeDecision::Commit { .. }
            ) {
                record_direct_anchor_commit(state, task_id, run_context, collaboration_id, effort);
                return Ok(AdaptiveAnchorOutcome::Commit(output.clone()));
            }
        }
    } else {
        supervisor = Some(spawn_direct_anchor(
            app,
            config,
            task_id,
            workspace_root,
            run_context,
            collaboration_id,
            anchor_spec,
            cancellation,
        )?);
    }
    Ok(AdaptiveAnchorOutcome::Continue(AdaptiveAnchorState {
        output,
        supervisor,
        attempted,
    }))
}

#[allow(clippy::too_many_arguments)]
fn spawn_direct_anchor(
    app: &tauri::AppHandle,
    config: &ProviderConfig,
    task_id: &TaskId,
    workspace_root: &Path,
    run_context: &Metadata,
    collaboration_id: &str,
    anchor_spec: &AdaptiveCollaborationSpec,
    cancellation: Option<Arc<AgentRunControl>>,
) -> Result<ParallelJobSupervisor<CollaborationCompletion>, String> {
    let mut supervisor = model_job_supervisor::<CollaborationCompletion>();
    let app = app.clone();
    let config = config.clone();
    let task_id = task_id.clone();
    let workspace_root = workspace_root.to_path_buf();
    let run_context = run_context.clone();
    let collaboration_id = collaboration_id.to_string();
    let stage = anchor_spec.stage.clone();
    let model = anchor_spec.model.clone();
    let prompt = anchor_spec.prompt.clone();
    let max_model_turns = anchor_spec.max_model_turns;
    let max_tool_calls = anchor_spec.max_tool_calls;
    let max_output_tokens = anchor_spec.max_output_tokens;
    supervisor
        .submit(
            DIRECT_ANCHOR_JOB_ID,
            "direct-anchor",
            Box::new(move |branch_cancellation| {
                complete_collaboration_worker_with_tools(
                    app,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    stage,
                    ModelRole::Executor,
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
        .map_err(|error| format!("direct anchor could not start: {error}"))?;
    Ok(supervisor)
}

fn record_direct_anchor_commit(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    effort: &str,
) {
    if let Ok(mut store) = state.store.lock() {
        let _ = append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            "Anytime direct anchor committed",
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("effort".to_string(), effort.to_string()),
                    ("conductor_skipped".to_string(), "true".to_string()),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        );
    }
}
