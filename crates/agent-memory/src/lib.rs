use agent_core::{Event, EventKind, Message, MessageRole};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointOptions {
    pub max_items_per_section: usize,
}

impl Default for CheckpointOptions {
    fn default() -> Self {
        Self {
            max_items_per_section: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCheckpoint {
    pub id: String,
    pub generated_at_ms: u64,
    pub event_count: usize,
    pub task_count: usize,
    pub latest_event_ms: u64,
    pub current_goal: Option<String>,
    pub completed_steps: Vec<String>,
    pub pending_steps: Vec<String>,
    pub decisions: Vec<String>,
    pub file_changes: Vec<String>,
    pub commands_run: Vec<String>,
    pub tool_results: Vec<String>,
    pub retrievals: Vec<String>,
    pub artifacts: Vec<String>,
    pub errors: Vec<String>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreContextPack {
    pub checkpoint: SessionCheckpoint,
    pub text: String,
}

pub fn build_session_checkpoint(events: &[Event], options: CheckpointOptions) -> SessionCheckpoint {
    let generated_at_ms = events
        .iter()
        .map(|event| event.timestamp_ms)
        .max()
        .unwrap_or(0);
    build_session_checkpoint_at(events, options, generated_at_ms)
}

pub fn build_session_checkpoint_at(
    events: &[Event],
    options: CheckpointOptions,
    generated_at_ms: u64,
) -> SessionCheckpoint {
    let limit = options.max_items_per_section.max(1);
    let mut sorted = events.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then(left.sequence.cmp(&right.sequence))
            .then(left.task_id.0.cmp(&right.task_id.0))
            .then(left.id.0.cmp(&right.id.0))
    });

    let mut task_ids = BTreeSet::new();
    let mut resolved_permissions = BTreeSet::new();
    for event in &sorted {
        task_ids.insert(event.task_id.0.clone());
        if matches!(event.kind, EventKind::PermissionResolved) {
            if let Some(permission_id) = event.metadata.get("permission_id") {
                resolved_permissions.insert(permission_id.clone());
            }
        }
    }

    let mut checkpoint = SessionCheckpoint {
        id: format!("ctx-{generated_at_ms}-{}", events.len()),
        generated_at_ms,
        event_count: events.len(),
        task_count: task_ids.len(),
        latest_event_ms: sorted
            .last()
            .map(|event| event.timestamp_ms)
            .unwrap_or(generated_at_ms),
        current_goal: None,
        completed_steps: Vec::new(),
        pending_steps: Vec::new(),
        decisions: Vec::new(),
        file_changes: Vec::new(),
        commands_run: Vec::new(),
        tool_results: Vec::new(),
        retrievals: Vec::new(),
        artifacts: Vec::new(),
        errors: Vec::new(),
        next_actions: Vec::new(),
    };

    let mut completed_seen = BTreeSet::new();
    let mut pending_seen = BTreeSet::new();
    let mut decision_seen = BTreeSet::new();
    let mut file_seen = BTreeSet::new();
    let mut command_seen = BTreeSet::new();
    let mut tool_seen = BTreeSet::new();
    let mut retrieval_seen = BTreeSet::new();
    let mut artifact_seen = BTreeSet::new();
    let mut error_seen = BTreeSet::new();

    for event in sorted.iter().rev() {
        if checkpoint.current_goal.is_none() {
            checkpoint.current_goal = goal_from_event(event);
        }

        match event.kind {
            EventKind::TaskStatusChanged | EventKind::ModelRequestFinished => {
                push_unique_limited(
                    &mut checkpoint.completed_steps,
                    &mut completed_seen,
                    event_line(event),
                    limit,
                );

                if event.metadata.contains_key("router_model") {
                    push_unique_limited(
                        &mut checkpoint.decisions,
                        &mut decision_seen,
                        router_decision_line(event),
                        limit,
                    );
                }
            }
            EventKind::ToolCallFinished => {
                push_unique_limited(
                    &mut checkpoint.completed_steps,
                    &mut completed_seen,
                    event_line(event),
                    limit,
                );
                push_unique_limited(
                    &mut checkpoint.tool_results,
                    &mut tool_seen,
                    tool_result_line(event),
                    limit,
                );

                if let Some(change) = file_change_line(event) {
                    push_unique_limited(
                        &mut checkpoint.file_changes,
                        &mut file_seen,
                        change,
                        limit,
                    );
                }

                if let Some(command) = command_line(event) {
                    push_unique_limited(
                        &mut checkpoint.commands_run,
                        &mut command_seen,
                        command,
                        limit,
                    );
                }
            }
            EventKind::PermissionRequested | EventKind::ToolCallProposed => {
                let permission_id = event.metadata.get("permission_id");
                let is_pending = permission_id
                    .map(|id| !resolved_permissions.contains(id))
                    .unwrap_or(true);
                if is_pending {
                    push_unique_limited(
                        &mut checkpoint.pending_steps,
                        &mut pending_seen,
                        event_line(event),
                        limit,
                    );
                }
            }
            EventKind::PermissionResolved => {
                push_unique_limited(
                    &mut checkpoint.decisions,
                    &mut decision_seen,
                    event_line(event),
                    limit,
                );
            }
            EventKind::RetrievalPerformed => {
                push_unique_limited(
                    &mut checkpoint.retrievals,
                    &mut retrieval_seen,
                    retrieval_line(event),
                    limit,
                );
            }
            EventKind::Error => {
                push_unique_limited(
                    &mut checkpoint.errors,
                    &mut error_seen,
                    error_line(event),
                    limit,
                );
            }
            EventKind::MessageAdded
            | EventKind::ModelRequestStarted
            | EventKind::ToolCallStarted
            | EventKind::TaskCreated => {}
        }

        for artifact in artifact_lines(event) {
            push_unique_limited(
                &mut checkpoint.artifacts,
                &mut artifact_seen,
                artifact,
                limit,
            );
        }
    }

    checkpoint.next_actions = next_actions_for(&checkpoint);
    checkpoint
}

pub fn build_restore_context_pack(checkpoint: SessionCheckpoint) -> RestoreContextPack {
    let text = checkpoint_to_markdown(&checkpoint);
    RestoreContextPack { checkpoint, text }
}

pub fn conversation_memory_to_markdown(messages: &[Message], max_items: usize) -> String {
    let limit = max_items.max(1);
    let first_user_index = messages.iter().position(|message| {
        matches!(message.role, MessageRole::User)
            && message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
            && !message.content.trim().is_empty()
    });
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(index) = first_user_index {
        let message = &messages[index];
        let value = format!(
            "Initial user request: {}",
            truncate(&sanitize_line(&message.content), 800)
        );
        seen.insert(value.clone());
        selected.push(value);
    }

    let remaining = limit.saturating_sub(selected.len());
    let mut recent = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != first_user_index)
        .filter_map(|(_, message)| conversation_memory_line(message))
        .rev()
        .filter(|value| seen.insert(value.clone()))
        .take(remaining)
        .collect::<Vec<_>>();
    recent.reverse();
    selected.extend(recent);

    if selected.is_empty() {
        return String::new();
    }

    let mut output = String::from(
        "## Conversation Memory\n- Historical extracts preserve continuity. User entries are requirements; assistant entries are prior claims and should be verified when material.\n",
    );
    for item in selected {
        output.push_str("- ");
        output.push_str(&item);
        output.push('\n');
    }
    output.push('\n');
    output
}

fn conversation_memory_line(message: &Message) -> Option<String> {
    if message.content.trim().is_empty()
        || message.metadata.get("kind").map(String::as_str) == Some("tool_observation")
    {
        return None;
    }
    match message.role {
        MessageRole::User => Some(format!(
            "User requirement: {}",
            truncate(&sanitize_line(&message.content), 800)
        )),
        MessageRole::Assistant
            if !message
                .metadata
                .get("tool_call_count")
                .and_then(|value| value.parse::<usize>().ok())
                .is_some_and(|count| count > 0) =>
        {
            Some(format!(
                "Assistant outcome: {}",
                truncate(&sanitize_line(&message.content), 600)
            ))
        }
        _ => None,
    }
}

pub fn checkpoint_to_markdown(checkpoint: &SessionCheckpoint) -> String {
    let mut output = String::new();
    output.push_str("# Cindx Context Checkpoint\n\n");
    output.push_str(&format!("- Checkpoint: {}\n", checkpoint.id));
    output.push_str(&format!("- Generated: {}\n", checkpoint.generated_at_ms));
    output.push_str(&format!("- Latest event: {}\n", checkpoint.latest_event_ms));
    output.push_str(&format!("- Events: {}\n", checkpoint.event_count));
    output.push_str(&format!("- Tasks: {}\n\n", checkpoint.task_count));

    output.push_str("## Current Goal\n");
    output.push_str("- ");
    output.push_str(
        checkpoint
            .current_goal
            .as_deref()
            .unwrap_or("No explicit goal found in the event log."),
    );
    output.push_str("\n\n");

    push_markdown_section(
        &mut output,
        "Completed Steps",
        &checkpoint.completed_steps,
        "No completed steps captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Pending Steps",
        &checkpoint.pending_steps,
        "No pending approvals or proposed actions captured.",
    );
    push_markdown_section(
        &mut output,
        "Decisions",
        &checkpoint.decisions,
        "No decisions captured yet.",
    );
    push_markdown_section(
        &mut output,
        "File Changes",
        &checkpoint.file_changes,
        "No file changes captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Commands Run",
        &checkpoint.commands_run,
        "No shell commands captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Tool Results",
        &checkpoint.tool_results,
        "No tool results captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Retrievals",
        &checkpoint.retrievals,
        "No retrievals captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Artifacts",
        &checkpoint.artifacts,
        "No artifacts captured yet.",
    );
    push_markdown_section(
        &mut output,
        "Errors",
        &checkpoint.errors,
        "No errors captured.",
    );
    push_markdown_section(
        &mut output,
        "Next Actions",
        &checkpoint.next_actions,
        "Ask the user for the next concrete goal.",
    );

    output
}

fn push_markdown_section(output: &mut String, title: &str, items: &[String], empty: &str) {
    output.push_str("## ");
    output.push_str(title);
    output.push('\n');
    if items.is_empty() {
        output.push_str("- ");
        output.push_str(empty);
        output.push_str("\n\n");
        return;
    }

    for item in items {
        output.push_str("- ");
        output.push_str(item);
        output.push('\n');
    }
    output.push('\n');
}

fn goal_from_event(event: &Event) -> Option<String> {
    if matches!(event.kind, EventKind::MessageAdded)
        && event
            .metadata
            .get("role")
            .map(|role| role == "user")
            .unwrap_or(false)
    {
        return event.metadata.get("content").map(|value| sanitize_line(value));
    }

    first_metadata_value(event, &["prompt"])
        .map(|value| sanitize_line(value))
        .filter(|value| !value.is_empty())
}

fn event_line(event: &Event) -> String {
    let detail = match event.kind {
        EventKind::MessageAdded => first_metadata_value(event, &["content"])
            .map(|value| {
                let role = event
                    .metadata
                    .get("role")
                    .map(String::as_str)
                    .unwrap_or("message");
                format!("{role}: {}", truncate(&sanitize_line(value), 220))
            })
            .unwrap_or_else(|| event.summary.clone()),
        _ => event.summary.clone(),
    };
    format!(
        "{} [{}] {}",
        event.task_id.0,
        event_kind_name(&event.kind),
        sanitize_line(&detail)
    )
}

fn router_decision_line(event: &Event) -> String {
    let model = event
        .metadata
        .get("router_model")
        .map(String::as_str)
        .unwrap_or("unknown model");
    let mode = event
        .metadata
        .get("router_retrieval_mode")
        .map(String::as_str)
        .unwrap_or("unknown retrieval mode");
    let explanation = event
        .metadata
        .get("router_explanation")
        .map(|value| truncate(&sanitize_line(value), 180))
        .unwrap_or_else(|| "no explanation".to_string());
    format!("router selected {model} with {mode}: {explanation}")
}

fn tool_result_line(event: &Event) -> String {
    let tool = first_metadata_value(event, &["tool", "tool_name"]).unwrap_or("tool");
    let status = first_metadata_value(event, &["status"]).unwrap_or("unknown");
    let output = first_metadata_value(event, &["output"])
        .map(|value| truncate(&sanitize_line(value), 220))
        .unwrap_or_else(|| event.summary.clone());
    format!("{tool} {status}: {output}")
}

fn file_change_line(event: &Event) -> Option<String> {
    let tool = first_metadata_value(event, &["tool", "tool_name"])?;
    if tool != "file.write" {
        return None;
    }

    let path = first_metadata_value(event, &["result_path", "path"]).unwrap_or("<unknown path>");
    let bytes = first_metadata_value(event, &["result_bytes", "bytes"]).unwrap_or("unknown");
    Some(format!("wrote {path} ({bytes} bytes)"))
}

fn command_line(event: &Event) -> Option<String> {
    let tool = first_metadata_value(event, &["tool", "tool_name"])?;
    if tool != "shell.run" {
        return None;
    }

    let command =
        first_metadata_value(event, &["result_command", "command"]).unwrap_or("<unknown command>");
    let cwd = first_metadata_value(event, &["result_cwd", "cwd"]).unwrap_or(".");
    let code = first_metadata_value(event, &["result_exit_code", "exit_code"]).unwrap_or("unknown");
    Some(format!("{command} (cwd {cwd}, exit {code})"))
}

fn retrieval_line(event: &Event) -> String {
    let action = first_metadata_value(event, &["action"]).unwrap_or("retrieval");
    let query = first_metadata_value(event, &["query"])
        .map(|value| format!(" query={}", truncate(&sanitize_line(value), 160)))
        .unwrap_or_default();
    let count = first_metadata_value(event, &["selected_count", "result_count", "chunks_indexed"])
        .map(|value| format!(" count={value}"))
        .unwrap_or_default();
    let mode = first_metadata_value(event, &["retrieval_mode", "embedding_backend"])
        .map(|value| format!(" mode={value}"))
        .unwrap_or_default();
    format!("{action}{query}{count}{mode}")
}

fn artifact_lines(event: &Event) -> Vec<String> {
    const KEYS: &[&str] = &[
        "index_path",
        "lancedb_export_path",
        "graph_store_path",
        "artifact_path",
        "text_path",
        "screenshot_path",
        "redaction_manifest_path",
        "result_artifact_path",
        "result_text_path",
        "result_screenshot_path",
        "result_redaction_manifest_path",
    ];

    KEYS.iter()
        .filter_map(|key| event.metadata.get(*key).map(|value| format!("{key}: {value}")))
        .collect()
}

fn error_line(event: &Event) -> String {
    first_metadata_value(event, &["error"])
        .map(|value| format!("{}: {}", event.summary, truncate(&sanitize_line(value), 220)))
        .unwrap_or_else(|| event_line(event))
}

fn next_actions_for(checkpoint: &SessionCheckpoint) -> Vec<String> {
    let mut actions = Vec::new();
    if !checkpoint.pending_steps.is_empty() {
        actions.push("Resolve pending approvals or proposed local actions.".to_string());
    }
    if !checkpoint.errors.is_empty() {
        actions.push("Inspect the latest error before continuing execution.".to_string());
    }
    if checkpoint.current_goal.is_some() {
        actions.push("Resume from this restore pack and continue the current goal.".to_string());
    } else {
        actions.push("Ask the user for the next concrete goal.".to_string());
    }
    actions
}

fn first_metadata_value<'a>(event: &'a Event, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| event.metadata.get(*key).map(String::as_str))
}

fn push_unique_limited(
    target: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    value: String,
    limit: usize,
) {
    if target.len() >= limit {
        return;
    }

    let value = sanitize_line(&value);
    if value.is_empty() || !seen.insert(value.clone()) {
        return;
    }

    target.push(value);
}

fn sanitize_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            return output;
        }
        output.push(character);
    }
    output
}

fn event_kind_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::TaskCreated => "task_created",
        EventKind::TaskStatusChanged => "task_status_changed",
        EventKind::MessageAdded => "message_added",
        EventKind::ModelRequestStarted => "model_request_started",
        EventKind::ModelRequestFinished => "model_request_finished",
        EventKind::ToolCallProposed => "tool_call_proposed",
        EventKind::ToolCallStarted => "tool_call_started",
        EventKind::ToolCallFinished => "tool_call_finished",
        EventKind::PermissionRequested => "permission_requested",
        EventKind::PermissionResolved => "permission_resolved",
        EventKind::RetrievalPerformed => "retrieval_performed",
        EventKind::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{EventId, Metadata, TaskId};

    #[test]
    fn builds_checkpoint_from_event_log() {
        let events = vec![
            event(
                1,
                EventKind::MessageAdded,
                "User message",
                [("role", "user"), ("content", "Build a context manager")],
            ),
            event(
                2,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "file.write"),
                    ("status", "succeeded"),
                    ("output", "wrote file"),
                    ("result_path", "docs/context.md"),
                    ("result_bytes", "42"),
                ],
            ),
            event(
                3,
                EventKind::RetrievalPerformed,
                "RAG search completed",
                [
                    ("action", "search"),
                    ("query", "context manager"),
                    ("selected_count", "2"),
                    ("retrieval_mode", "graph_rag"),
                ],
            ),
        ];

        let checkpoint = build_session_checkpoint_at(&events, CheckpointOptions::default(), 9);

        assert_eq!(checkpoint.current_goal.as_deref(), Some("Build a context manager"));
        assert_eq!(checkpoint.event_count, 3);
        assert_eq!(checkpoint.task_count, 1);
        assert!(checkpoint
            .file_changes
            .iter()
            .any(|item| item.contains("docs/context.md")));
        assert!(checkpoint
            .retrievals
            .iter()
            .any(|item| item.contains("graph_rag")));
    }

    #[test]
    fn excludes_resolved_permissions_from_pending_steps() {
        let events = vec![
            event(
                1,
                EventKind::PermissionRequested,
                "Permission requested",
                [("permission_id", "perm-1"), ("tool", "shell.run")],
            ),
            event(
                2,
                EventKind::PermissionResolved,
                "Permission approved",
                [("permission_id", "perm-1"), ("decision", "allow_once")],
            ),
        ];

        let checkpoint = build_session_checkpoint(&events, CheckpointOptions::default());

        assert!(checkpoint.pending_steps.is_empty());
        assert_eq!(checkpoint.decisions.len(), 1);
    }

    #[test]
    fn restore_pack_contains_operational_sections() {
        let events = vec![event(
            1,
            EventKind::RetrievalPerformed,
            "Workspace indexed",
            [
                ("action", "index"),
                ("chunks_indexed", "5"),
                ("lancedb_export_path", ".cindx/lancedb-records.jsonl"),
                ("graph_store_path", ".cindx/graph.tsv"),
            ],
        )];

        let checkpoint = build_session_checkpoint_at(&events, CheckpointOptions::default(), 7);
        let pack = build_restore_context_pack(checkpoint);

        assert!(pack.text.contains("## Retrievals"));
        assert!(pack.text.contains("lancedb_export_path"));
        assert!(pack.text.contains("graph_store_path"));
    }

    #[test]
    fn limits_sections_to_latest_items() {
        let mut events = Vec::new();
        for sequence in 1..=4 {
            events.push(event(
                sequence,
                EventKind::ToolCallFinished,
                "Tool finished",
                [
                    ("tool", "shell.run"),
                    ("status", "succeeded"),
                    ("output", "ok"),
                    ("result_command", if sequence == 4 { "pwd" } else { "echo ok" }),
                    ("result_exit_code", "0"),
                ],
            ));
        }

        let checkpoint =
            build_session_checkpoint(&events, CheckpointOptions { max_items_per_section: 2 });

        assert_eq!(checkpoint.commands_run.len(), 2);
        assert!(checkpoint.commands_run[0].contains("pwd"));
    }

    #[test]
    fn conversation_memory_preserves_requirements_and_completed_outcomes() {
        let messages = vec![
            message(MessageRole::User, "Build a local agent", []),
            message(MessageRole::Assistant, "I created the first version", []),
            message(
                MessageRole::Assistant,
                "Calling a tool",
                [("tool_call_count", "2")],
            ),
            message(
                MessageRole::User,
                "Tool observation that should stay out",
                [("kind", "tool_observation")],
            ),
            message(MessageRole::User, "Keep permissions explicit", []),
            message(MessageRole::Assistant, "Permissions are now gated", []),
        ];

        let memory = conversation_memory_to_markdown(&messages, 4);

        assert!(memory.contains("Initial user request: Build a local agent"));
        assert!(memory.contains("User requirement: Keep permissions explicit"));
        assert!(memory.contains("Assistant outcome: Permissions are now gated"));
        assert!(!memory.contains("Calling a tool"));
        assert!(!memory.contains("Tool observation that should stay out"));
    }

    fn message<const N: usize>(
        role: MessageRole,
        content: &str,
        metadata: [(&str, &str); N],
    ) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata: metadata
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    fn event<const N: usize>(
        sequence: u64,
        kind: EventKind,
        summary: &str,
        metadata: [(&str, &str); N],
    ) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: TaskId("task".to_string()),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: summary.to_string(),
            metadata: metadata
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Metadata>(),
        }
    }
}
