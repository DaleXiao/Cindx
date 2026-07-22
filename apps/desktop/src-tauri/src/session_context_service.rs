use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SessionCompactionPlan {
    pub(super) estimated_history_tokens: u64,
    pub(super) estimated_request_tokens: u64,
    pub(super) recent_budget_tokens: u64,
    pub(super) recent_start: usize,
    pub(super) recent_tokens: u64,
    pub(super) should_compact: bool,
}

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

pub(super) fn estimate_message_tokens(message: &Message) -> u64 {
    let content_tokens = estimate_text_tokens_for_context(&message.content);
    let tool_call_tokens = message
        .metadata
        .get("raw_tool_calls_json")
        .map(|value| estimate_text_tokens_for_context(value))
        .unwrap_or(0);
    let image_tokens = message
        .metadata
        .get("image_paths")
        .map(|paths| paths.lines().filter(|path| !path.trim().is_empty()).count() as u64 * 1_024)
        .unwrap_or(0);
    content_tokens
        .saturating_add(tool_call_tokens)
        .saturating_add(image_tokens)
        .saturating_add(6)
}

pub(super) fn estimate_text_tokens_for_context(value: &str) -> u64 {
    let mut ascii = 0_u64;
    let mut non_ascii = 0_u64;
    for character in value.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or_default()
        .saturating_add(non_ascii)
        .saturating_add(u64::from(!value.is_empty()))
}

pub(super) fn estimate_context_tokens(messages: &[Message]) -> u64 {
    if messages.is_empty() {
        return 0;
    }
    512_u64.saturating_add(messages.iter().map(estimate_message_tokens).sum::<u64>())
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

    match (&event.kind, event.summary.as_str()) {
        (EventKind::ModelRequestStarted, "Agent model turn started") => event
            .metadata
            .get("context_projected_tokens")
            .and_then(|value| value.parse().ok())
            .map(|tokens| (tokens, true)),
        (EventKind::ModelRequestFinished, "Agent model turn finished") => event
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
            }),
        _ => None,
    }
}

fn context_prompt_reserve(context_window_tokens: u64) -> u64 {
    let context_window_tokens = context_window_tokens.max(1);
    (context_window_tokens / 8)
        .clamp(2_048, 16_384)
        .min(context_window_tokens / 4)
}

pub(super) fn is_user_turn_start(message: &Message) -> bool {
    matches!(message.role, MessageRole::User)
        && message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
}

pub(super) fn recent_history_start(history: &[Message], token_budget: u64) -> (usize, u64) {
    if history.is_empty() {
        return (0, 0);
    }
    let mut start = history.len();
    let mut selected = 0usize;
    let mut tokens = 0_u64;
    while start > 0 && selected < CONTEXT_RECENT_MAX_MESSAGES {
        let message_tokens = estimate_message_tokens(&history[start - 1]);
        if selected > 0 && tokens.saturating_add(message_tokens) > token_budget {
            break;
        }
        start -= 1;
        selected += 1;
        tokens = tokens.saturating_add(message_tokens);
    }
    if start > 0 && !is_user_turn_start(&history[start]) {
        if let Some(offset) = history[start..].iter().position(is_user_turn_start) {
            start += offset;
        } else if let Some(previous_turn) = history[..start].iter().rposition(is_user_turn_start) {
            start = previous_turn;
        }
    }
    if start == history.len() {
        start = history.len() - 1;
    }
    let tokens = history[start..].iter().map(estimate_message_tokens).sum();
    (start, tokens)
}

pub(super) fn session_compaction_plan(
    history: &[Message],
    context_window_tokens: u64,
) -> SessionCompactionPlan {
    let context_window_tokens = context_window_tokens.max(1);
    let estimated_history_tokens = estimate_context_tokens(history);
    let estimated_request_tokens =
        estimated_history_tokens.saturating_add(context_prompt_reserve(context_window_tokens));
    let should_compact = estimated_request_tokens
        >= context_window_tokens.saturating_mul(CONTEXT_COMPACTION_TRIGGER_PERCENT) / 100
        || history.len() > 80;
    let recent_floor = 8_000.min(context_window_tokens / 2).max(1);
    let recent_budget = (context_window_tokens.saturating_mul(CONTEXT_RECENT_TARGET_PERCENT) / 100)
        .min(CONTEXT_RECENT_MAX_TOKENS)
        .max(recent_floor);
    let (recent_start, recent_tokens) = recent_history_start(history, recent_budget);
    SessionCompactionPlan {
        estimated_history_tokens,
        estimated_request_tokens,
        recent_budget_tokens: recent_budget,
        recent_start,
        recent_tokens,
        should_compact,
    }
}

pub(super) fn context_checkpoint_is_within_reuse_window(
    checkpoint: &ValidatedContextCheckpoint,
    history: &[Message],
    plan: SessionCompactionPlan,
    context_window_tokens: u64,
) -> bool {
    if checkpoint.covered_messages > history.len() {
        return false;
    }
    let retained = &history[checkpoint.covered_messages..];
    if retained.len() > CONTEXT_RECENT_REUSE_MAX_MESSAGES {
        return false;
    }
    let retained_tokens = retained.iter().map(estimate_message_tokens).sum::<u64>();
    let reuse_budget = (context_window_tokens
        .max(1)
        .saturating_mul(CONTEXT_RECENT_REUSE_PERCENT)
        / 100)
        .min(CONTEXT_RECENT_REUSE_MAX_TOKENS)
        .max(plan.recent_budget_tokens);
    retained_tokens <= reuse_budget
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
    let (checkpoint, checkpoint_reused) = if plan.should_compact && !can_reuse_checkpoint {
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
        let Some(checkpoint) = existing_checkpoint else {
            return Ok(history);
        };
        (checkpoint, true)
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
        .saturating_add(context_prompt_reserve(context_window_tokens))
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

    Ok(compacted)
}
