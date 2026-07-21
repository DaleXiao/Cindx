use super::*;

pub(crate) fn stable_workflow_resume_key(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("workflow-resume-{hash:016x}")
}

pub(crate) fn latest_external_user_turn_event(events: &[Event]) -> Option<&Event> {
    events.iter().rev().find(|event| {
        event.kind == EventKind::MessageAdded
            && event.metadata.get("role").map(String::as_str) == Some("user")
            && event
                .metadata
                .get("continuation_replay")
                .map(String::as_str)
                != Some("true")
            && event.metadata.get("internal").map(String::as_str) != Some("true")
    })
}

pub(crate) fn workflow_resume_key_from_events(
    events: &[Event],
    session_id: Option<&str>,
    prompt: &str,
    effort: &str,
    policy: &str,
) -> String {
    let user_sequence = latest_external_user_turn_event(events)
        .map(|event| event.sequence)
        .unwrap_or_default();
    stable_workflow_resume_key(&format!(
        "{}\n{}\n{}\n{}\n{}",
        session_id.unwrap_or_default(),
        user_sequence,
        effort,
        policy,
        prompt
    ))
}

pub(crate) fn resumable_workflow_checkpoint_from_events(
    events: &[Event],
    resume_key: &str,
    prompt: &str,
    allowed_models: &[String],
) -> Option<WorkflowExecutionCheckpoint> {
    let event = events.iter().rev().find(|event| {
        event
            .metadata
            .get("workflow_resume_key")
            .map(String::as_str)
            == Some(resume_key)
            && event.metadata.contains_key("workflow_checkpoint")
    })?;
    let encoded = event.metadata.get("workflow_checkpoint")?;
    let checkpoint = WorkflowExecutionCheckpoint::from_json(encoded, allowed_models).ok()?;
    (!checkpoint.is_complete() && checkpoint.plan.objective == prompt).then_some(checkpoint)
}

pub(crate) fn load_workflow_checkpoint_for_run(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    prompt: &str,
    effort: &str,
    policy: &str,
    allowed_models: &[String],
) -> Result<(String, Option<WorkflowExecutionCheckpoint>), String> {
    let session_id = run_context.get("session_id").map(String::as_str);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let events = match session_id {
        Some(session_id) => store
            .list_by_task_and_metadata_or_unscoped(task_id, "session_id", session_id)
            .map_err(|error| error.to_string())?,
        None => store
            .list_by_task(task_id)
            .map_err(|error| error.to_string())?,
    };
    let resume_key =
        workflow_resume_key_from_events(events.as_slice(), session_id, prompt, effort, policy);
    let checkpoint = resumable_workflow_checkpoint_from_events(
        events.as_slice(),
        &resume_key,
        prompt,
        allowed_models,
    );
    Ok((resume_key, checkpoint))
}

pub(crate) fn append_workflow_checkpoint_event(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    collaboration_id: &str,
    summary: &str,
    status: &str,
    step_id: Option<&str>,
    checkpoint: &WorkflowExecutionCheckpoint,
) -> Result<(), String> {
    let encoded = checkpoint.to_json()?;
    let mut metadata = [
        ("collaboration_id".to_string(), collaboration_id.to_string()),
        (
            "workflow_schema".to_string(),
            WORKFLOW_IR_SCHEMA.to_string(),
        ),
        (
            "workflow_checkpoint_schema".to_string(),
            WORKFLOW_CHECKPOINT_SCHEMA.to_string(),
        ),
        (
            "workflow_resume_key".to_string(),
            checkpoint.resume_key.clone(),
        ),
        ("workflow_checkpoint".to_string(), encoded),
        ("checkpoint_status".to_string(), status.to_string()),
        (
            "completed_steps".to_string(),
            checkpoint.completed_step_count().to_string(),
        ),
        (
            "workflow_steps".to_string(),
            checkpoint.plan.steps.len().to_string(),
        ),
        (
            "workflow_continuations".to_string(),
            checkpoint.continuations.to_string(),
        ),
        (
            "additional_model_turns_per_step".to_string(),
            checkpoint.additional_model_turns_per_step.to_string(),
        ),
    ]
    .into_iter()
    .collect::<Metadata>();
    if let Some(step_id) = step_id {
        metadata.insert("step_id".to_string(), step_id.to_string());
        if let Some(step) = checkpoint.steps.get(step_id) {
            metadata.insert(
                "step_status".to_string(),
                format!("{:?}", step.status).to_lowercase(),
            );
            metadata.insert("step_attempts".to_string(), step.attempts.to_string());
            metadata.insert("step_model".to_string(), step.model.clone());
        }
    }
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn checkpoint_evidence_by_step(
    checkpoint: &WorkflowExecutionCheckpoint,
) -> BTreeMap<String, Vec<CollaborationEvidence>> {
    checkpoint
        .steps
        .iter()
        .filter(|(_, step)| step.status == WorkflowStepStatus::Completed)
        .filter_map(|(step_id, step)| {
            serde_json::from_str::<Vec<CollaborationEvidence>>(&step.evidence_json)
                .ok()
                .map(|evidence| (step_id.clone(), evidence))
        })
        .collect()
}

pub(crate) fn ensure_adaptive_step_attempt_started(
    checkpoint: &mut WorkflowExecutionCheckpoint,
    step_id: &str,
    model: &str,
    attempt_limit: usize,
    now_ms: u64,
) -> Result<bool, String> {
    let is_running = checkpoint
        .steps
        .get(step_id)
        .is_some_and(|step| step.status == WorkflowStepStatus::Running);
    if is_running {
        return Ok(false);
    }
    checkpoint.begin_step_with_attempt_limit(step_id, model, attempt_limit, now_ms)?;
    Ok(true)
}

pub(crate) fn adaptive_layer_failure_error(failures: &[String]) -> Option<String> {
    if failures.is_empty() {
        None
    } else {
        Some(format!(
            "{WORKFLOW_RESUMABLE_ERROR_PREFIX} {}",
            failures.join(" | ")
        ))
    }
}
