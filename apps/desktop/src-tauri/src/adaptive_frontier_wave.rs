use super::*;
use orchestrator::AdaptiveWorkflow;

pub(super) struct AdaptiveWave {
    pub(super) layer_index: usize,
    pub(super) specs: Vec<AdaptiveCollaborationSpec>,
    pub(super) independent: bool,
    pub(super) required_successes: usize,
}

pub(super) struct AdaptiveWaveCompletion {
    pub(super) cancellation: Option<Arc<AgentRunControl>>,
    pub(super) completions: Vec<CollaborationCompletion>,
}

pub(super) struct AdaptiveWavePlanningContext<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) prompt: &'a str,
    pub(super) shared_memory: &'a str,
    pub(super) workflow: &'a AdaptiveWorkflow,
    pub(super) workflow_plan: &'a WorkflowPlanIr,
    pub(super) models: &'a [String],
    pub(super) execution_contract: &'a ConductorExecutionContract,
    pub(super) layer: Vec<usize>,
    pub(super) layer_index: usize,
    pub(super) layer_count: usize,
    pub(super) max_step_attempts: usize,
    pub(super) max_model_turns_per_step: usize,
    pub(super) outputs: &'a BTreeMap<String, String>,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
    pub(super) anytime_controller: &'a mut AnytimeController,
}

pub(super) fn prepare_adaptive_wave(
    context: AdaptiveWavePlanningContext<'_, '_>,
) -> Result<AdaptiveWave, String> {
    let AdaptiveWavePlanningContext {
        state,
        task_id,
        run_context,
        collaboration_id,
        prompt,
        shared_memory,
        workflow,
        workflow_plan,
        models,
        execution_contract,
        layer,
        layer_index,
        layer_count,
        max_step_attempts,
        max_model_turns_per_step,
        outputs,
        workflow_checkpoint,
        anytime_controller,
    } = context;

    if let Some(control) =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?
    {
        control.mark_progress_at(
            run_context_steer_epoch(run_context),
            "collaboration",
            &format!(
                "Frontier wave {} · {}/{} steps restored",
                layer_index + 1,
                workflow_checkpoint.completed_step_count(),
                workflow_plan.steps.len()
            ),
        );
    }
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            task_id,
            EventKind::TaskStatusChanged,
            format!("Anytime frontier wave {} started", layer_index + 1),
            metadata_with_context(
                [
                    ("collaboration_id".to_string(), collaboration_id.to_string()),
                    ("layer".to_string(), (layer_index + 1).to_string()),
                    (
                        "planned_dependency_depth".to_string(),
                        layer_count.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
    }

    let mut specs = layer
        .into_iter()
        .map(|step_index| {
            let step = &workflow.steps[step_index];
            let worker_prompt = adaptive_worker_prompt_for_plan(
                workflow_plan,
                step_index,
                prompt,
                shared_memory,
                outputs,
            )
            .ok_or_else(|| format!("adaptive worker prompt is missing for {}", step.id))?;
            Ok(AdaptiveCollaborationSpec {
                step_index,
                step_id: step.id.clone(),
                role: step.role.clone(),
                stage: format!("worker_{}", step_index + 1),
                model: workflow_checkpoint
                    .steps
                    .get(&step.id)
                    .map(|checkpoint| checkpoint.model.clone())
                    .unwrap_or_else(|| step.model.clone()),
                subtask: step.subtask.clone(),
                prompt: worker_prompt,
                request_id: unique_id("collaboration-model"),
                access: step.access.clone(),
                tool_policy: workflow_plan.steps[step_index].tool_policy,
                output_kind: workflow_plan.steps[step_index].contract.output_kind.clone(),
                max_attempts: max_step_attempts,
                max_model_turns: max_model_turns_per_step,
                max_tool_calls: workflow_plan.steps[step_index]
                    .tool_policy
                    .tool_call_budget_within(workflow_plan.budget.max_tool_calls_per_step),
                max_output_tokens: workflow_plan.budget.max_output_tokens_per_step as u64,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    for spec in &mut specs {
        if let Some(error) =
            adaptive_worker_model_distinctness_error(spec, &spec.model, workflow_checkpoint)
        {
            let Some(replacement_model) = models.iter().find(|model| {
                adaptive_worker_model_distinctness_error(spec, model, workflow_checkpoint).is_none()
            }) else {
                return Err(error);
            };
            spec.model = replacement_model.clone();
        }
    }

    let claimed_steps = workflow_checkpoint.claim_steps(
        &specs
            .iter()
            .map(|spec| spec.step_id.clone())
            .collect::<Vec<_>>(),
        max_step_attempts,
        current_time_millis(),
    )?;
    let resumed_steps = claimed_steps
        .into_iter()
        .map(|claim| (claim.step_id, claim.resumed))
        .collect::<BTreeMap<_, _>>();
    for spec in &specs {
        controller_mark_running_if_pending(anytime_controller, &spec.step_id)?;
        persist_anytime_controller(workflow_checkpoint, anytime_controller)?;
        let resumed = resumed_steps.get(&spec.step_id).copied().unwrap_or(false);
        append_workflow_checkpoint_event(
            state,
            task_id,
            run_context,
            collaboration_id,
            if resumed {
                "Collaboration workflow step resumed"
            } else {
                "Collaboration workflow step started"
            },
            "running",
            Some(&spec.step_id),
            workflow_checkpoint,
        )?;
        let metadata = adaptive_stage_metadata(spec);
        let role = adaptive_model_role(&spec.role, &spec.output_kind);
        record_collaboration_stage_started(
            state,
            task_id,
            run_context,
            collaboration_id,
            &spec.stage,
            &role,
            &spec.model,
            &spec.request_id,
            adaptive_step_attribution(&spec.output_kind, &role),
            &metadata,
        )?;
    }

    let independent = specs.iter().all(|spec| spec.access.is_empty());
    let required_successes =
        if independent && execution_contract.stop_policy != ConductorStopPolicy::Exhaustive {
            execution_contract.required_successes_for_layer(specs.len())
        } else {
            specs.len().max(1)
        };
    Ok(AdaptiveWave {
        layer_index,
        specs,
        independent,
        required_successes,
    })
}

pub(super) struct AdaptiveWaveExecutionContext<'a, 'state> {
    pub(super) app: &'a tauri::AppHandle,
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) workspace_root: &'a Path,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) execution_contract: &'a ConductorExecutionContract,
    pub(super) wave: &'a AdaptiveWave,
    pub(super) workflow_checkpoint: &'a mut WorkflowExecutionCheckpoint,
    pub(super) anytime_controller: &'a mut AnytimeController,
}

pub(super) fn execute_adaptive_wave(
    context: AdaptiveWaveExecutionContext<'_, '_>,
) -> Result<AdaptiveWaveCompletion, String> {
    let AdaptiveWaveExecutionContext {
        app,
        state,
        config,
        task_id,
        workspace_root,
        run_context,
        collaboration_id,
        execution_contract,
        wave,
        workflow_checkpoint,
        anytime_controller,
    } = context;
    let cancellation =
        active_agent_run_control(state, run_context.get("session_id").map(String::as_str))?;
    let jobs = wave
        .specs
        .iter()
        .map(|spec| {
            let app = app.clone();
            let config = config.clone();
            let task_id = task_id.clone();
            let workspace_root = workspace_root.to_path_buf();
            let run_context = run_context.clone();
            let collaboration_id = collaboration_id.to_string();
            let stage = spec.stage.clone();
            let model = spec.model.clone();
            let role = adaptive_model_role(&spec.role, &spec.output_kind);
            let prompt = spec.prompt.clone();
            let worker_access =
                CollaborationWorkerAccess::new(spec.step_id.clone(), spec.tool_policy);
            let max_model_turns = spec.max_model_turns;
            let max_tool_calls = spec.max_tool_calls;
            let max_output_tokens = spec.max_output_tokens;
            let cancellation = cancellation.clone();
            Box::new(move |branch_cancellation| {
                complete_collaboration_worker_with_tools(
                    app,
                    config,
                    task_id,
                    workspace_root,
                    run_context,
                    collaboration_id,
                    stage,
                    role,
                    model,
                    prompt,
                    worker_access,
                    max_model_turns,
                    max_tool_calls,
                    max_output_tokens,
                    cancellation,
                    Some(branch_cancellation),
                )
            }) as CancellableParallelJob<CollaborationCompletion>
        })
        .collect::<Vec<_>>();
    let mut steer_interrupt_requested = false;
    let layer_execution = run_model_jobs_until_quorum_interruptible(
        "adaptive-worker",
        jobs,
        InterruptibleQuorumPolicy::new(
            wave.required_successes,
            Duration::from_millis(execution_contract.quorum_grace_ms()),
            Duration::from_millis(20),
        ),
        |completion| {
            completion
                .content
                .as_ref()
                .is_some_and(|content| !content.trim().is_empty())
        },
        || {
            if collaboration_steer_pending(cancellation.as_ref()) {
                steer_interrupt_requested = true;
                return true;
            }
            false
        },
    );
    if steer_interrupt_requested || collaboration_steer_pending(cancellation.as_ref()) {
        pause_anytime_for_steer(
            state,
            task_id,
            run_context,
            collaboration_id,
            workflow_checkpoint,
            anytime_controller,
        )?;
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }

    let layer_execution = layer_execution.execution;
    if layer_execution.cancelled_stragglers > 0 {
        if let Ok(mut store) = state.store.lock() {
            let _ = append_event(
                &mut store,
                task_id,
                EventKind::TaskStatusChanged,
                "Collaboration stragglers cancelled",
                metadata_with_context(
                    [
                        ("collaboration_id".to_string(), collaboration_id.to_string()),
                        ("layer".to_string(), (wave.layer_index + 1).to_string()),
                        (
                            "cancelled_stragglers".to_string(),
                            layer_execution.cancelled_stragglers.to_string(),
                        ),
                        (
                            "successful_branches".to_string(),
                            layer_execution.successful.to_string(),
                        ),
                        (
                            "required_branches".to_string(),
                            wave.required_successes.to_string(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                    run_context,
                ),
            );
        }
    }
    let completions = layer_execution
        .results
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|error| {
                CollaborationCompletion::failed(format!(
                    "adaptive collaboration worker failed: {error}"
                ))
            })
        })
        .collect();
    Ok(AdaptiveWaveCompletion {
        cancellation,
        completions,
    })
}
