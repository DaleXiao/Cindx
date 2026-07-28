use crate::{
    desktop_prelude::{Event, EventKind, Message, MessageRole},
    tool_execution::runtime_message_from_event,
};
use std::collections::BTreeSet;

pub(crate) fn recovery_safe_transcript(events: &[Event]) -> Vec<Message> {
    let resolved_tool_calls = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::MessageAdded
                && event.metadata.get("role").map(String::as_str) == Some("tool")
        })
        .filter_map(|event| event.metadata.get("tool_call_id").cloned())
        .collect::<BTreeSet<_>>();
    let mut synthetic = BTreeSet::new();
    let mut messages = Vec::new();
    for message in events.iter().filter_map(runtime_message_from_event) {
        let unresolved = if message.role == MessageRole::Assistant {
            message
                .metadata
                .get("tool_call_ids")
                .map(|ids| {
                    ids.split(',')
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .filter(|id| !resolved_tool_calls.contains(*id))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        messages.push(message);
        for tool_call_id in unresolved {
            if !synthetic.insert(tool_call_id.clone()) {
                continue;
            }
            messages.push(Message {
                role: MessageRole::Tool,
                content: "The prior tool call was interrupted before a durable result was recorded. Treat its outcome as unknown. Inspect current state before retrying, and request permission again for any write or destructive action.".to_string(),
                metadata: [
                    ("tool_call_id".to_string(), tool_call_id),
                    ("status".to_string(), "interrupted".to_string()),
                    ("kind".to_string(), "recovery_observation".to_string()),
                ]
                .into_iter()
                .collect(),
            });
        }
    }
    messages
}
