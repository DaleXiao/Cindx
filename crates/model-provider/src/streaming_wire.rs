use super::{parse_provider_error, ModelError, ModelToolCall};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StreamingToolCall {
    id: String,
    name: String,
    arguments_json: String,
}

impl StreamingToolCall {
    pub(super) fn merge(&mut self, delta: StreamingToolCallDelta) {
        if let Some(id) = delta.id {
            self.id.push_str(&id);
        }
        if let Some(name) = delta.name {
            self.name.push_str(&name);
        }
        if let Some(arguments) = delta.arguments_json {
            self.arguments_json.push_str(&arguments);
        }
    }

    pub(super) fn finish(self, index: usize) -> Option<ModelToolCall> {
        (!self.name.trim().is_empty()).then(|| ModelToolCall {
            id: if self.id.trim().is_empty() {
                format!("call-{index}")
            } else {
                self.id
            },
            name: self.name,
            arguments_json: if self.arguments_json.trim().is_empty() {
                "{}".to_string()
            } else {
                self.arguments_json
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamingToolCallDelta {
    pub(super) index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments_json: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StreamEvent {
    pub(super) content: Option<String>,
    pub(super) tool_calls: Vec<StreamingToolCallDelta>,
    pub(super) finish_reason: Option<String>,
}

pub(super) fn parse_stream_event(line: &str) -> Result<Option<StreamEvent>, ModelError> {
    let line = line.trim();
    if !line.starts_with("data:") {
        return Ok(None);
    }

    let payload = line.trim_start_matches("data:").trim();
    if payload == "[DONE]" {
        return Ok(None);
    }

    if let Some(message) = parse_provider_error(payload) {
        return Err(ModelError::new(message));
    }

    let value = serde_json::from_str::<serde_json::Value>(payload)
        .map_err(|error| ModelError::new(format!("invalid model stream event: {error}")))?;
    let Some(delta) = value.pointer("/choices/0/delta") else {
        return Ok(Some(StreamEvent {
            finish_reason: value
                .pointer("/choices/0/finish_reason")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            ..StreamEvent::default()
        }));
    };
    let content = delta
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let tool_calls = delta
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|call| StreamingToolCallDelta {
            index: call
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default() as usize,
            id: call
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            name: call
                .pointer("/function/name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            arguments_json: call
                .pointer("/function/arguments")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
        .collect();

    Ok(Some(StreamEvent {
        content,
        tool_calls,
        finish_reason: value
            .pointer("/choices/0/finish_reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }))
}

pub fn parse_stream_line(line: &str) -> Result<Option<String>, ModelError> {
    Ok(parse_stream_event(line)?.and_then(|event| event.content))
}
