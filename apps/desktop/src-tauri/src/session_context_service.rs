use crate::desktop_prelude::*;
use crate::{
    app_state::AppState,
    event_persistence::append_event,
    event_projection::{collect_context_events, write_context_checkpoint},
    persistence_runtime::{
        context_checkpoint_manifest_path_for_session, context_checkpoint_path_for_session,
    },
    project_session_persistence::metadata_with_context,
    runtime_constants::{
        CONTEXT_CHECKPOINT_MANIFEST_SCHEMA, CONTEXT_COMPACTION_VERSION, CONTEXT_MEMORY_MAX_ITEMS,
        CONTEXT_RESTORE_MAX_CHARS,
    },
    runtime_values::{current_time_millis, message_role_label, phase15_task_id, phase16_task_id},
    tool_execution::message_from_event,
};

pub(super) use agent_runtime::{estimate_context_tokens, estimate_message_tokens};
#[cfg(test)]
pub(super) use agent_runtime::{
    estimate_text_tokens as estimate_text_tokens_for_context, is_user_turn_start,
};
pub(super) type SessionCompactionPlan = agent_runtime::ContextCompactionPlan;

#[derive(Debug, Clone, Copy)]
pub(super) struct ContextCheckpointCoverage<'a> {
    pub(super) history: &'a [Message],
    pub(super) covered_messages: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContextCheckpointManifest {
    schema: String,
    session_id: Option<String>,
    compaction_version: String,
    covered_messages: usize,
    history_messages: usize,
    covered_prefix_sha256: String,
    checkpoint_sha256: String,
    generated_at_ms: u64,
}

impl ContextCheckpointManifest {
    pub(super) fn new(
        session_id: Option<&str>,
        history: &[Message],
        covered_messages: usize,
        checkpoint_text: &str,
    ) -> Result<Self, String> {
        if covered_messages > history.len() {
            return Err(format!(
                "context checkpoint coverage exceeds history: {covered_messages} > {}",
                history.len()
            ));
        }
        Ok(Self {
            schema: CONTEXT_CHECKPOINT_MANIFEST_SCHEMA.to_string(),
            session_id: session_id.map(str::to_string),
            compaction_version: CONTEXT_COMPACTION_VERSION.to_string(),
            covered_messages,
            history_messages: history.len(),
            covered_prefix_sha256: context_history_prefix_sha256(&history[..covered_messages]),
            checkpoint_sha256: sha256_hex(checkpoint_text.as_bytes()),
            generated_at_ms: current_time_millis(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedContextCheckpoint {
    pub(super) text: String,
    pub(super) covered_messages: usize,
}

fn context_history_prefix_sha256(messages: &[Message]) -> String {
    let mut encoded = Vec::new();
    for message in messages {
        append_fingerprint_field(&mut encoded, message_role_label(&message.role).as_bytes());
        append_fingerprint_field(&mut encoded, message.content.as_bytes());
        encoded.extend_from_slice(&(message.metadata.len() as u64).to_le_bytes());
        for (key, value) in &message.metadata {
            append_fingerprint_field(&mut encoded, key.as_bytes());
            append_fingerprint_field(&mut encoded, value.as_bytes());
        }
    }
    sha256_hex(&encoded)
}

fn append_fingerprint_field(encoded: &mut Vec<u8>, value: &[u8]) {
    encoded.extend_from_slice(&(value.len() as u64).to_le_bytes());
    encoded.extend_from_slice(value);
}

pub(super) fn read_validated_context_checkpoint(
    workspace_root: &Path,
    session_id: Option<&str>,
    history: &[Message],
) -> Option<ValidatedContextCheckpoint> {
    let checkpoint_path = context_checkpoint_path_for_session(workspace_root, session_id);
    let manifest_path = context_checkpoint_manifest_path_for_session(workspace_root, session_id);
    let text = fs::read_to_string(&checkpoint_path).ok()?;
    let manifest_text = fs::read_to_string(&manifest_path).ok()?;
    let manifest = serde_json::from_str::<ContextCheckpointManifest>(&manifest_text).ok()?;
    if text.trim().is_empty()
        || manifest.schema != CONTEXT_CHECKPOINT_MANIFEST_SCHEMA
        || manifest.compaction_version != CONTEXT_COMPACTION_VERSION
        || manifest.session_id.as_deref() != session_id
        || manifest.covered_messages > history.len()
        || manifest.history_messages < manifest.covered_messages
        || manifest.history_messages > history.len()
        || manifest.checkpoint_sha256 != sha256_hex(text.as_bytes())
        || manifest.covered_prefix_sha256
            != context_history_prefix_sha256(&history[..manifest.covered_messages])
    {
        return None;
    }

    Some(ValidatedContextCheckpoint {
        text,
        covered_messages: manifest.covered_messages,
    })
}

pub(super) fn history_with_context_checkpoint(
    history: &[Message],
    checkpoint: ValidatedContextCheckpoint,
    checkpoint_path: &Path,
) -> Option<Vec<Message>> {
    if checkpoint.text.trim().is_empty() || checkpoint.covered_messages > history.len() {
        return None;
    }
    let tail = &history[checkpoint.covered_messages..];
    let mut compacted = Vec::with_capacity(tail.len() + 1);
    compacted.push(Message {
        role: MessageRole::System,
        content: format!(
            "Recovered memory for this project and session. Preserve historical user requirements, but treat prior assistant and tool statements as memory that may need verification. Prioritize the recent verbatim messages that follow.\n\n{}",
            truncate_for_collaboration(&checkpoint.text, CONTEXT_RESTORE_MAX_CHARS)
        ),
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), "context_restore_pack".to_string()),
            (
                "context_source_schema".to_string(),
                agent_runtime::CONTEXT_SOURCE_SCHEMA.to_string(),
            ),
            (
                "compaction_version".to_string(),
                CONTEXT_COMPACTION_VERSION.to_string(),
            ),
            (
                "covered_messages".to_string(),
                checkpoint.covered_messages.to_string(),
            ),
            (
                "context_checkpoint_path".to_string(),
                checkpoint_path.display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    });
    compacted.extend(tail.iter().cloned());
    Some(compacted)
}

pub(super) fn effective_context_usage_from_event(event: &Event) -> Option<(u64, bool)> {
    if event
        .metadata
        .get("context_usage_reset")
        .map(String::as_str)
        == Some("true")
    {
        let tokens = event.metadata.get("context_tokens_used")?.parse().ok()?;
        let estimated = event
            .metadata
            .get("context_usage_estimated")
            .map(String::as_str)
            != Some("false");
        return Some((tokens, estimated));
    }

    if agent_application::is_agent_model_turn_started(event) {
        return event
            .metadata
            .get("context_projected_tokens")
            .and_then(|value| value.parse().ok())
            .map(|tokens| (tokens, true));
    }
    if agent_application::is_agent_model_turn_finished(event) {
        return event
            .metadata
            .get("prompt_tokens")
            .and_then(|value| value.parse().ok())
            .map(|tokens| (tokens, false))
            .or_else(|| {
                event
                    .metadata
                    .get("context_projected_tokens")
                    .and_then(|value| value.parse().ok())
                    .map(|tokens| (tokens, true))
            });
    }
    None
}

#[cfg(test)]
pub(super) fn recent_history_start(history: &[Message], token_budget: u64) -> (usize, u64) {
    agent_runtime::ContextEngine::default().recent_history_start(history, token_budget)
}

pub(super) fn session_compaction_plan(
    history: &[Message],
    context_window_tokens: u64,
) -> SessionCompactionPlan {
    agent_runtime::ContextEngine::default().compaction_plan(history, context_window_tokens)
}

pub(super) fn context_checkpoint_is_within_reuse_window(
    checkpoint: &ValidatedContextCheckpoint,
    history: &[Message],
    plan: SessionCompactionPlan,
    context_window_tokens: u64,
) -> bool {
    agent_runtime::ContextEngine::default().checkpoint_is_reusable(
        checkpoint.covered_messages,
        history,
        plan,
        context_window_tokens,
    )
}

pub(super) fn context_events_for_covered_history_prefix(
    events: &[Event],
    covered_messages: usize,
) -> Vec<Event> {
    if covered_messages == 0 {
        return Vec::new();
    }

    let task_id = phase16_task_id();
    let mut visible_messages = 0usize;
    let cutoff = events.iter().position(|event| {
        if event.task_id != task_id || message_from_event(event).is_none() {
            return false;
        }
        visible_messages += 1;
        visible_messages == covered_messages
    });
    let Some(cutoff) = cutoff else {
        return Vec::new();
    };

    events[..=cutoff]
        .iter()
        .filter(|event| event.task_id == task_id)
        .cloned()
        .collect()
}

pub(super) fn prepare_session_history_context(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    run_context: &Metadata,
    history: Vec<Message>,
    context_window_tokens: u64,
    telemetry_control: Option<&AgentRunControl>,
) -> Result<Vec<Message>, String> {
    if history.is_empty() {
        return Ok(history);
    }
    let checkpoint_path = context_checkpoint_path_for_session(
        workspace_root,
        run_context.get("session_id").map(String::as_str),
    );
    let plan = session_compaction_plan(&history, context_window_tokens);
    let session_id = run_context.get("session_id").map(String::as_str);
    let existing_checkpoint =
        read_validated_context_checkpoint(workspace_root, session_id, &history);
    let can_reuse_checkpoint = existing_checkpoint.as_ref().is_some_and(|checkpoint| {
        context_checkpoint_is_within_reuse_window(checkpoint, &history, plan, context_window_tokens)
    });
    let (checkpoint, checkpoint_reused) = if can_reuse_checkpoint {
        (
            existing_checkpoint.expect("validated reusable checkpoint should exist"),
            true,
        )
    } else if plan.should_compact {
        if plan.recent_start == 0 {
            return Ok(history);
        }
        let older_messages = &history[..plan.recent_start];
        let events = {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            collect_context_events(&store, run_context).map_err(|error| error.to_string())?
        };
        let covered_events = context_events_for_covered_history_prefix(&events, plan.recent_start);
        let checkpoint = build_session_checkpoint_at(
            &covered_events,
            CheckpointOptions::default(),
            current_time_millis(),
        );
        let mut pack = build_restore_context_pack(checkpoint);
        pack.text.push_str(&conversation_memory_to_markdown(
            older_messages,
            CONTEXT_MEMORY_MAX_ITEMS,
        ));
        let path = write_context_checkpoint(
            workspace_root,
            session_id,
            &pack.text,
            Some(ContextCheckpointCoverage {
                history: &history,
                covered_messages: plan.recent_start,
            }),
        )?;
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        append_event(
            &mut store,
            &phase15_task_id(),
            EventKind::TaskStatusChanged,
            "Context checkpoint automatically compacted",
            metadata_with_context(
                [
                    ("checkpoint_id".to_string(), pack.checkpoint.id.clone()),
                    (
                        "context_checkpoint_path".to_string(),
                        path.display().to_string(),
                    ),
                    (
                        "compaction_version".to_string(),
                        CONTEXT_COMPACTION_VERSION.to_string(),
                    ),
                    ("original_messages".to_string(), history.len().to_string()),
                    (
                        "retained_messages".to_string(),
                        history.len().saturating_sub(plan.recent_start).to_string(),
                    ),
                    (
                        "covered_messages".to_string(),
                        plan.recent_start.to_string(),
                    ),
                    (
                        "covered_events".to_string(),
                        covered_events.len().to_string(),
                    ),
                    (
                        "original_tokens".to_string(),
                        plan.estimated_history_tokens.to_string(),
                    ),
                    (
                        "estimated_request_tokens".to_string(),
                        plan.estimated_request_tokens.to_string(),
                    ),
                    (
                        "context_window_tokens".to_string(),
                        context_window_tokens.to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
                run_context,
            ),
        )
        .map_err(|error| error.to_string())?;
        (
            ValidatedContextCheckpoint {
                text: pack.text,
                covered_messages: plan.recent_start,
            },
            false,
        )
    } else {
        return Ok(history);
    };
    let covered_messages = checkpoint.covered_messages;
    let retained_messages = history.len().saturating_sub(covered_messages);
    let retained_tokens = if plan.should_compact && covered_messages == plan.recent_start {
        plan.recent_tokens
    } else {
        history[covered_messages..]
            .iter()
            .map(estimate_message_tokens)
            .sum()
    };
    let Some(compacted) = history_with_context_checkpoint(&history, checkpoint, &checkpoint_path)
    else {
        return Ok(history);
    };

    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        &phase15_task_id(),
        EventKind::TaskStatusChanged,
        "Session context restored for agent run",
        metadata_with_context(
            [
                ("original_messages".to_string(), history.len().to_string()),
                (
                    "retained_messages".to_string(),
                    retained_messages.to_string(),
                ),
                ("covered_messages".to_string(), covered_messages.to_string()),
                (
                    "original_tokens".to_string(),
                    plan.estimated_history_tokens.to_string(),
                ),
                ("retained_tokens".to_string(), retained_tokens.to_string()),
                (
                    "compaction_version".to_string(),
                    CONTEXT_COMPACTION_VERSION.to_string(),
                ),
                (
                    "checkpoint_reused".to_string(),
                    checkpoint_reused.to_string(),
                ),
                (
                    "context_checkpoint_path".to_string(),
                    checkpoint_path.display().to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    let context_tokens_used = estimate_context_tokens(&compacted)
        .saturating_add(agent_runtime::context_prompt_reserve(context_window_tokens))
        .min(context_window_tokens.max(1));
    append_event(
        &mut store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent context compacted",
        metadata_with_context(
            [
                ("internal".to_string(), "true".to_string()),
                ("context_usage_reset".to_string(), "true".to_string()),
                (
                    "context_tokens_used".to_string(),
                    context_tokens_used.to_string(),
                ),
                (
                    "context_window_tokens".to_string(),
                    context_window_tokens.to_string(),
                ),
                ("context_usage_estimated".to_string(), "true".to_string()),
                ("covered_messages".to_string(), covered_messages.to_string()),
                (
                    "retained_messages".to_string(),
                    retained_messages.to_string(),
                ),
                (
                    "checkpoint_reused".to_string(),
                    checkpoint_reused.to_string(),
                ),
                (
                    "compaction_version".to_string(),
                    CONTEXT_COMPACTION_VERSION.to_string(),
                ),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())?;

    if let Some(control) = telemetry_control {
        control.record_context_compaction();
    }
    Ok(compacted)
}
