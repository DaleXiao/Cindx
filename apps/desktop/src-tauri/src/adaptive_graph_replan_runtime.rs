use super::*;

pub(super) struct AdaptiveGraphReplanContext<'a, 'state> {
    pub(super) state: &'a tauri::State<'state, AppState>,
    pub(super) config: &'a ProviderConfig,
    pub(super) task_id: &'a TaskId,
    pub(super) run_context: &'a Metadata,
    pub(super) collaboration_id: &'a str,
    pub(super) models: &'a [String],
    pub(super) wave: &'a AdaptiveWave,
    pub(super) layer_failures: &'a [String],
    pub(super) checkpoint: &'a mut WorkflowExecutionCheckpoint,
    pub(super) cancellation: Option<&'a Arc<AgentRunControl>>,
}

pub(super) fn attempt_adaptive_graph_replan(
    context: AdaptiveGraphReplanContext<'_, '_>,
) -> Result<Option<orchestrator::WorkflowPlanRevisionRecord>, String> {
    let AdaptiveGraphReplanContext {
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        models,
        wave,
        layer_failures,
        checkpoint,
        cancellation,
    } = context;
    if layer_failures.is_empty()
        || checkpoint.plan_revisions.len() >= orchestrator::MAX_WORKFLOW_PLAN_REVISIONS
    {
        return Ok(None);
    }
    if collaboration_steer_pending(cancellation) {
        return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
    }
    let Some(target_step_id) = wave.specs.iter().find_map(|spec| {
        checkpoint.steps.get(&spec.step_id).and_then(|step| {
            (step.status == WorkflowStepStatus::Failed).then(|| spec.step_id.clone())
        })
    }) else {
        return Ok(None);
    };
    let failure = checkpoint
        .steps
        .get(&target_step_id)
        .and_then(|step| step.error.clone())
        .unwrap_or_else(|| layer_failures.join(" | "));
    let preserved_outputs = checkpoint
        .completed_outputs()
        .into_iter()
        .map(|(step_id, output)| {
            format!("[{step_id}] {}", truncate_for_collaboration(&output, 4_000))
        })
        .collect::<Vec<_>>();
    let harness =
        orchestrator::WorkflowRevisionHarness::new(orchestrator::WorkflowRevisionRequest {
            plan: checkpoint.plan.clone(),
            failed_step_id: target_step_id.clone(),
            failure: truncate_for_collaboration(&failure, 3_000),
            preserved_outputs,
            allowed_models: models.to_vec(),
        });
    let conductor_model = config.model_for_conductor();
    let planning_prompt = harness.planning_prompt()?;
    let first_response = match run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        &format!("graph_replan_{}", checkpoint.plan_revisions.len() + 1),
        ModelRole::Planner,
        &conductor_model,
        planning_prompt,
        AgentModelAttribution::service(
            AgentService::Conductor,
            AgentStage::Plan,
            AgentModelProfile::Reasoning,
        ),
    ) {
        Ok(response) => response,
        Err(error)
            if error == COLLABORATION_STEER_INTERRUPTED
                || collaboration_steer_pending(cancellation) =>
        {
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        Err(_) => return Ok(None),
    };
    if let Ok(record) = validate_and_apply_revision(
        checkpoint,
        &harness,
        &first_response,
        models,
        current_time_millis(),
    ) {
        return Ok(Some(record));
    }
    let validation_error = validate_revision_without_mutation(
        checkpoint,
        &harness,
        &first_response,
        models,
        current_time_millis(),
    )
    .unwrap_err();
    let repair_prompt = harness.repair_prompt(&first_response, &validation_error)?;
    let repaired_response = match run_collaboration_stage(
        state,
        config,
        task_id,
        run_context,
        collaboration_id,
        &format!(
            "graph_replan_{}_repair",
            checkpoint.plan_revisions.len() + 1
        ),
        ModelRole::Planner,
        &conductor_model,
        repair_prompt,
        AgentModelAttribution::service(
            AgentService::Conductor,
            AgentStage::Plan,
            AgentModelProfile::Reasoning,
        ),
    ) {
        Ok(response) => response,
        Err(error)
            if error == COLLABORATION_STEER_INTERRUPTED
                || collaboration_steer_pending(cancellation) =>
        {
            return Err(COLLABORATION_STEER_INTERRUPTED.to_string());
        }
        Err(_) => return Ok(None),
    };
    validate_and_apply_revision(
        checkpoint,
        &harness,
        &repaired_response,
        models,
        current_time_millis(),
    )
    .map(Some)
    .or(Ok(None))
}

fn validate_revision_without_mutation(
    checkpoint: &WorkflowExecutionCheckpoint,
    harness: &orchestrator::WorkflowRevisionHarness,
    response: &str,
    models: &[String],
    now_ms: u64,
) -> Result<orchestrator::WorkflowPlanRevisionRecord, String> {
    let revision = harness.parse_revision(response)?;
    let mut candidate = checkpoint.clone();
    candidate.apply_plan_revision(revision, models, now_ms)
}

fn validate_and_apply_revision(
    checkpoint: &mut WorkflowExecutionCheckpoint,
    harness: &orchestrator::WorkflowRevisionHarness,
    response: &str,
    models: &[String],
    now_ms: u64,
) -> Result<orchestrator::WorkflowPlanRevisionRecord, String> {
    let revision = harness.parse_revision(response)?;
    let mut candidate = checkpoint.clone();
    let record = candidate.apply_plan_revision(revision, models, now_ms)?;
    *checkpoint = candidate;
    Ok(record)
}
