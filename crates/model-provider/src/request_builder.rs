use super::{EmbeddingResponse, EmbeddingVector, ModelError};
use crate::json_wire::{
    extract_json_array_after, extract_json_number_field, extract_json_string_field, json_escape,
    parse_number_array, split_top_level_objects,
};
use crate::response_parser::{parse_provider_error, tool_function_name};
use agent_core::{Message, MessageRole, Metadata, ToolSpec};
use base64::Engine;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub fn build_embedding_request_json(
    model: &str,
    input: &[String],
    dimensions: Option<usize>,
) -> Result<String, ModelError> {
    if model.trim().is_empty() {
        return Err(ModelError::new("embedding model is empty"));
    }
    if input.is_empty() {
        return Err(ModelError::new("embedding input is empty"));
    }

    let inputs = input
        .iter()
        .map(|value| format!("\"{}\"", json_escape(value)))
        .collect::<Vec<_>>();
    let dimensions = dimensions
        .map(|value| format!(",\"dimensions\":{value}"))
        .unwrap_or_default();

    Ok(format!(
        "{{\"model\":\"{}\",\"input\":[{}]{}}}",
        json_escape(model),
        inputs.join(","),
        dimensions
    ))
}

pub fn parse_embedding_response(text: &str) -> Result<EmbeddingResponse, ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }

    let model = extract_json_string_field(text, "model")
        .ok_or_else(|| ModelError::new("embedding response did not include model"))?;
    let data = extract_json_array_after(text, "\"data\"")
        .ok_or_else(|| ModelError::new("embedding response did not include data"))?;
    let mut vectors = Vec::new();

    for object in split_top_level_objects(&data) {
        let index = extract_json_number_field(&object, "index")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(vectors.len());
        let embedding_text = extract_json_array_after(&object, "\"embedding\"")
            .ok_or_else(|| ModelError::new("embedding item did not include vector"))?;
        let embedding = parse_number_array(&embedding_text)?;
        vectors.push(EmbeddingVector { index, embedding });
    }
    vectors.sort_by_key(|vector| vector.index);

    let mut metadata = Metadata::new();
    metadata.insert("model".to_string(), model.clone());
    metadata.insert("vectors".to_string(), vectors.len().to_string());

    Ok(EmbeddingResponse {
        model,
        vectors,
        metadata,
    })
}

pub fn build_chat_request_json(
    model: &str,
    messages: &[Message],
    stream: bool,
) -> Result<String, ModelError> {
    build_chat_request_json_with_tools(model, messages, stream, &[])
}

pub fn build_chat_request_json_with_tools(
    model: &str,
    messages: &[Message],
    stream: bool,
    tools: &[ToolSpec],
) -> Result<String, ModelError> {
    build_chat_request_json_with_tools_and_output_limit(model, messages, stream, tools, None)
}

pub(super) fn build_chat_request_json_with_tools_and_output_limit(
    model: &str,
    messages: &[Message],
    stream: bool,
    tools: &[ToolSpec],
    max_output_tokens: Option<u64>,
) -> Result<String, ModelError> {
    build_chat_request_json_with_tools_output_limit_and_vision(
        model,
        messages,
        stream,
        tools,
        max_output_tokens,
        model_supports_vision_content(model),
    )
}

pub(super) fn build_chat_request_json_with_tools_output_limit_and_vision(
    model: &str,
    messages: &[Message],
    stream: bool,
    tools: &[ToolSpec],
    max_output_tokens: Option<u64>,
    supports_vision: bool,
) -> Result<String, ModelError> {
    let mut declared_tool_calls = BTreeSet::new();
    let messages_json = messages
        .iter()
        .filter_map(|message| match message.role {
            MessageRole::Assistant => {
                if let Some(tool_calls_json) = assistant_tool_calls_json(message) {
                    declared_tool_calls.extend(tool_call_ids_from_json(&tool_calls_json));
                    Some(format!(
                        "{{\"role\":\"assistant\",\"content\":\"{}\",\"tool_calls\":{}}}",
                        json_escape(&message.content),
                        tool_calls_json
                    ))
                } else {
                    Some(format!(
                        "{{\"role\":\"assistant\",\"content\":\"{}\"}}",
                        json_escape(&message.content)
                    ))
                }
            }
            MessageRole::Tool => {
                let tool_call_id = message
                    .metadata
                    .get("tool_call_id")
                    .map(String::as_str)
                    .unwrap_or("tool-call");
                if !declared_tool_calls.remove(tool_call_id) {
                    return None;
                }
                Some(format!(
                    "{{\"role\":\"tool\",\"tool_call_id\":\"{}\",\"content\":\"{}\"}}",
                    json_escape(tool_call_id),
                    json_escape(&message.content)
                ))
            }
            _ => Some(format!(
                "{{\"role\":\"{}\",\"content\":{}}}",
                json_escape(message_role_to_str(&message.role)),
                message_content_json(supports_vision, message)
            )),
        })
        .collect::<Vec<_>>();
    let tools_json = if tools.is_empty() {
        String::new()
    } else {
        format!(
            ",\"tools\":[{}],\"tool_choice\":\"auto\"",
            tools
                .iter()
                .map(tool_spec_json)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let output_limit_json = max_output_tokens
        .filter(|value| *value > 0)
        .map(|value| format!(",\"max_tokens\":{value}"))
        .unwrap_or_default();

    Ok(format!(
        "{{\"model\":\"{}\",\"stream\":{},\"messages\":[{}]{}{}}}",
        json_escape(model),
        if stream { "true" } else { "false" },
        messages_json.join(","),
        output_limit_json,
        tools_json
    ))
}

fn message_content_json(supports_vision: bool, message: &Message) -> String {
    let Some(paths) = message.metadata.get("image_paths") else {
        return format!("\"{}\"", json_escape(&message.content));
    };
    if !supports_vision {
        return format!(
            "\"{}\"",
            json_escape(&format!(
                "{}\n\n[Image attachment omitted because the selected model does not support vision.]",
                message.content
            ))
        );
    }
    let images = paths.lines().filter_map(image_data_url).collect::<Vec<_>>();
    if images.is_empty() {
        return format!("\"{}\"", json_escape(&message.content));
    }
    let mut parts = vec![format!(
        "{{\"type\":\"text\",\"text\":\"{}\"}}",
        json_escape(&message.content)
    )];
    parts.extend(images.into_iter().map(|data_url| {
        format!(
            "{{\"type\":\"image_url\",\"image_url\":{{\"url\":\"{}\"}}}}",
            json_escape(&data_url)
        )
    }));
    format!("[{}]", parts.join(","))
}

pub(super) fn model_supports_vision_content(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase().replace('_', "-");
    model.contains("vision")
        || model.contains("-vl")
        || model.contains("omni")
        || model.contains("pixtral")
        || model.contains("llava")
        || model.contains("glm-4v")
        || model.starts_with("gpt-4o")
        || model.starts_with("gpt-4.1")
        || model.starts_with("gpt-5")
        || model.starts_with("gemini")
        || model.starts_with("claude-3")
        || model.starts_with("claude-4")
}

fn image_data_url(value: &str) -> Option<String> {
    let path = Path::new(value.trim());
    if !path.is_absolute()
        || !path
            .components()
            .any(|component| component.as_os_str() == ".cindx")
    {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 24 * 1024 * 1024 {
        return None;
    }
    let mime_type = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = fs::read(path).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime_type};base64,{encoded}"))
}

fn message_role_to_str(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "assistant",
    }
}

fn tool_spec_json(tool: &ToolSpec) -> String {
    let function_name = tool_function_name(&tool.name);
    let description = format!("{} Original tool name: {}.", tool.description, tool.name);
    let parameters = serde_json::from_str::<serde_json::Value>(&tool.input_schema_json)
        .ok()
        .filter(|schema| schema.get("type").and_then(serde_json::Value::as_str) == Some("object"))
        .unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })
        });
    format!(
        "{{\"type\":\"function\",\"function\":{{\"name\":\"{}\",\"description\":\"{}\",\"parameters\":{}}}}}",
        json_escape(&function_name),
        json_escape(&description),
        parameters
    )
}

fn assistant_tool_calls_json(message: &Message) -> Option<String> {
    let raw = message
        .metadata
        .get("raw_tool_calls_json")
        .map(String::as_str)?
        .trim();
    let parsed = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let calls = parsed.as_array()?;
    (!calls.is_empty() && calls.iter().all(valid_tool_call_value))
        .then(|| serde_json::to_string(&parsed).ok())
        .flatten()
}

fn valid_tool_call_value(value: &serde_json::Value) -> bool {
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
        && value
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| !name.trim().is_empty())
        && value
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|arguments| serde_json::from_str::<serde_json::Value>(arguments).is_ok())
}

fn tool_call_ids_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|call| {
            call.get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}
