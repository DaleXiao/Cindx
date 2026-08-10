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

pub(crate) struct LoadedWorkflowCheckpoint {
    pub(crate) checkpoint: WorkflowExecutionCheckpoint,
    pub(crate) resumable: bool,
}

pub(crate) fn resumable_workflow_checkpoint_from_events(
    events: &[Event],
    resume_key: &str,
    prompt: &str,
    allowed_models: &[String],
) -> Option<LoadedWorkflowCheckpoint> {
    let event = events.iter().rev().find(|event| {
        event
            .metadata
            .get("workflow_resume_key")
            .map(String::as_str)
            == Some(resume_key)
            && event.metadata.contains_key("workflow_checkpoint")
    })?;
    let encoded = event.metadata.get("workflow_checkpoint")?;
    let (checkpoint, models_available) =
        match WorkflowExecutionCheckpoint::from_json(encoded, allowed_models) {
            Ok(checkpoint) => (checkpoint, true),
            Err(_) => (
                WorkflowExecutionCheckpoint::from_json_for_untrusted_handoff(encoded).ok()?,
                false,
            ),
        };
    if checkpoint.is_complete() || checkpoint.plan.objective != prompt {
        return None;
    }
    let required_verification = checkpoint
        .plan
        .steps
        .iter()
        .any(|step| step.contract.output_kind == WorkflowOutputKind::Verification);
    let owner_execution_shape = checkpoint
        .plan
        .validate_owner_execution_graph(required_verification)
        .is_ok();
    let pristine_owner_handoff = checkpoint
        .plan
        .steps
        .last()
        .and_then(|step| checkpoint.steps.get(&step.id))
        .is_some_and(|step| step.attempts == 0);
    Some(LoadedWorkflowCheckpoint {
        checkpoint,
        resumable: models_available && owner_execution_shape && pristine_owner_handoff,
    })
}

pub(crate) fn load_workflow_checkpoint_for_run(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    prompt: &str,
    effort: &str,
    policy: &str,
    allowed_models: &[String],
) -> Result<(String, Option<LoadedWorkflowCheckpoint>), String> {
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

#[allow(clippy::too_many_arguments)]
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
        (
            "workflow_plan_revisions".to_string(),
            checkpoint.plan_revisions.len().to_string(),
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
        .filter(|(_, step)| {
            matches!(
                step.status,
                WorkflowStepStatus::Completed | WorkflowStepStatus::Degraded
            )
        })
        .filter_map(|(step_id, step)| {
            serde_json::from_str::<Vec<CollaborationEvidence>>(&step.evidence_json)
                .ok()
                .map(|evidence| (step_id.clone(), evidence))
        })
        .collect()
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

pub(crate) fn adaptive_partial_work_handoff(
    prompt: &str,
    outputs: &BTreeMap<String, String>,
    failures: &[String],
) -> Option<String> {
    if outputs.is_empty() || failures.is_empty() {
        return None;
    }
    let completed = outputs
        .iter()
        .map(|(step_id, output)| {
            format!(
                "Completed step {step_id}:\n{}",
                truncate_for_collaboration(output, 10_000)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(format!(
        "INTERNAL PARTIAL WORKFLOW HANDOFF: The adaptive workflow completed useful independent work before one or more branches exhausted recovery. Continue from the preserved work below instead of restarting the task. Recheck dependencies, resolve the listed failures, verify claims and artifacts, and produce the final user-facing result yourself. Do not expose this internal note to the user.\n\nUser objective:\n{}\n\nFailed branches:\n- {}\n\nPreserved completed work:\n{}",
        truncate_for_collaboration(prompt, 4_000),
        truncate_for_collaboration(&failures.join("\n- "), 4_000),
        truncate_for_collaboration(&completed, 30_000),
    ))
}

pub(crate) fn adaptive_degraded_branch_output(step_id: &str, error: &str) -> String {
    format!(
        "INTERNAL DEGRADED BRANCH {step_id}: This branch exhausted recovery and produced no trustworthy result. Do not treat it as evidence or a completed answer. Continue with the other independent branches, explicitly account for the missing perspective during verification, and repair any coverage gap before final delivery. Failure: {}",
        truncate_for_collaboration(error, 2_000),
    )
}
