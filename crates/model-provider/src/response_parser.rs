use agent_core::{Message, MessageRole, Metadata};

use crate::json_wire::{
    extract_json_array_after, extract_json_bool_field, extract_json_number_field,
    extract_json_string_field, extract_json_string_field_after, split_top_level_objects,
};
use crate::provider_receipt::{
    attach_response_semantic_sha256, finalize_provider_receipt_status, provider_identity_metadata,
};
use crate::{
    ModelError, ModelResponse, ModelToolCall, DSML_INVOKE_CLOSE, DSML_INVOKE_OPEN,
    DSML_PARAMETER_CLOSE, DSML_PARAMETER_OPEN, DSML_TOOL_CALLS_CLOSE, DSML_TOOL_CALLS_OPEN,
};

pub fn parse_chat_response(text: &str) -> Result<String, ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }

    extract_json_string_field_after(text, "\"message\"", "content")
        .ok_or_else(|| ModelError::new("model response did not include assistant content"))
}

pub fn parse_model_response(text: &str) -> Result<ModelResponse, ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }

    let mut content =
        extract_json_string_field_after(text, "\"message\"", "content").unwrap_or_default();
    let mut tool_calls = parse_tool_calls(text)?;
    let mut raw_tool_calls_json = extract_json_array_after(text, "\"tool_calls\"");
    let mut metadata = Metadata::new();
    if normalize_dsml_tool_calls(&mut content, &mut tool_calls)? {
        metadata.insert("tool_protocol".to_string(), "dsml".to_string());
    }
    metadata.insert("tool_calls".to_string(), tool_calls.len().to_string());
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(finish_reason) = value
            .pointer("/choices/0/finish_reason")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            metadata.insert("finish_reason".to_string(), finish_reason.to_string());
        }
        metadata.extend(provider_identity_metadata(&value));
    }
    if raw_tool_calls_json.is_none() && !tool_calls.is_empty() {
        raw_tool_calls_json = Some(serialize_tool_calls(&tool_calls));
    }
    for field in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        if let Some(value) = extract_json_number_field(text, field) {
            metadata.insert(field.to_string(), value);
        }
    }
    finalize_provider_receipt_status(&mut metadata);
    let finish_reason = metadata.get("finish_reason").cloned();
    attach_response_semantic_sha256(
        &mut metadata,
        &content,
        &tool_calls,
        finish_reason.as_deref(),
    );

    Ok(ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content,
            metadata: Metadata::new(),
        },
        raw_tool_calls_json,
        tool_calls,
        metadata,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DsmlToolCallBlock {
    visible_content: String,
    tool_calls: Vec<ModelToolCall>,
}

pub(crate) fn normalize_dsml_tool_calls(
    content: &mut String,
    tool_calls: &mut Vec<ModelToolCall>,
) -> Result<bool, ModelError> {
    let Some(parsed) = parse_dsml_tool_call_blocks(content)? else {
        return Ok(false);
    };
    *content = parsed.visible_content;
    if tool_calls.is_empty() {
        *tool_calls = parsed.tool_calls;
    }
    Ok(true)
}

fn parse_dsml_tool_call_blocks(content: &str) -> Result<Option<DsmlToolCallBlock>, ModelError> {
    if !content.contains(DSML_TOOL_CALLS_OPEN) {
        return Ok(None);
    }

    let mut visible_content = String::with_capacity(content.len());
    let mut tool_calls = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = content[cursor..].find(DSML_TOOL_CALLS_OPEN) {
        let block_start = cursor + relative_start;
        visible_content.push_str(&content[cursor..block_start]);
        let body_start = block_start + DSML_TOOL_CALLS_OPEN.len();
        let relative_end = content[body_start..]
            .find(DSML_TOOL_CALLS_CLOSE)
            .ok_or_else(|| ModelError::new("model returned incomplete DSML tool protocol"))?;
        let body_end = body_start + relative_end;
        parse_dsml_invocations(&content[body_start..body_end], &mut tool_calls)?;
        cursor = body_end + DSML_TOOL_CALLS_CLOSE.len();
    }
    visible_content.push_str(&content[cursor..]);
    if tool_calls.is_empty() {
        return Err(ModelError::new(
            "model returned a DSML tool block without invocations",
        ));
    }

    Ok(Some(DsmlToolCallBlock {
        visible_content: visible_content.trim().to_string(),
        tool_calls,
    }))
}

fn parse_dsml_invocations(
    body: &str,
    tool_calls: &mut Vec<ModelToolCall>,
) -> Result<(), ModelError> {
    let mut cursor = 0;
    let mut invocation_count = 0;
    while let Some(relative_start) = body[cursor..].find(DSML_INVOKE_OPEN) {
        let invoke_start = cursor + relative_start;
        if !body[cursor..invoke_start].trim().is_empty() {
            return Err(ModelError::new(
                "model returned unexpected text inside DSML tool protocol",
            ));
        }
        let header_end = body[invoke_start..]
            .find('>')
            .map(|offset| invoke_start + offset)
            .ok_or_else(|| ModelError::new("model returned incomplete DSML invoke tag"))?;
        let header = &body[invoke_start..=header_end];
        let name = dsml_attribute(header, "name")
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| ModelError::new("DSML invoke did not include a tool name"))?;
        let parameters_start = header_end + 1;
        let relative_end = body[parameters_start..]
            .find(DSML_INVOKE_CLOSE)
            .ok_or_else(|| ModelError::new("model returned incomplete DSML invoke"))?;
        let invoke_end = parameters_start + relative_end;
        let arguments = parse_dsml_parameters(&body[parameters_start..invoke_end])?;
        let call_index = tool_calls.len();
        tool_calls.push(ModelToolCall {
            id: format!("call-dsml-{call_index}"),
            name: name.to_string(),
            arguments_json: serde_json::to_string(&arguments)
                .map_err(|error| ModelError::new(format!("invalid DSML arguments: {error}")))?,
        });
        invocation_count += 1;
        cursor = invoke_end + DSML_INVOKE_CLOSE.len();
    }
    if invocation_count == 0 {
        return Err(ModelError::new(
            "model returned a DSML tool block without invocations",
        ));
    }
    if !body[cursor..].trim().is_empty() {
        return Err(ModelError::new(
            "model returned trailing text inside DSML tool protocol",
        ));
    }
    Ok(())
}

fn parse_dsml_parameters(
    body: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ModelError> {
    let mut parameters = serde_json::Map::new();
    let mut cursor = 0;
    while let Some(relative_start) = body[cursor..].find(DSML_PARAMETER_OPEN) {
        let parameter_start = cursor + relative_start;
        if !body[cursor..parameter_start].trim().is_empty() {
            return Err(ModelError::new(
                "model returned unexpected text inside a DSML invoke",
            ));
        }
        let header_end = body[parameter_start..]
            .find('>')
            .map(|offset| parameter_start + offset)
            .ok_or_else(|| ModelError::new("model returned incomplete DSML parameter tag"))?;
        let header = &body[parameter_start..=header_end];
        let name = dsml_attribute(header, "name")
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| ModelError::new("DSML parameter did not include a name"))?;
        if parameters.contains_key(name) {
            return Err(ModelError::new(format!(
                "DSML invoke repeated parameter {name}"
            )));
        }
        let value_start = header_end + 1;
        let relative_end = body[value_start..]
            .find(DSML_PARAMETER_CLOSE)
            .ok_or_else(|| ModelError::new("model returned incomplete DSML parameter"))?;
        let value_end = value_start + relative_end;
        let raw_value = body[value_start..value_end].trim();
        let force_string = dsml_attribute(header, "string") == Some("true");
        let value = if force_string {
            serde_json::Value::String(raw_value.to_string())
        } else {
            serde_json::from_str(raw_value)
                .unwrap_or_else(|_| serde_json::Value::String(raw_value.to_string()))
        };
        parameters.insert(name.to_string(), value);
        cursor = value_end + DSML_PARAMETER_CLOSE.len();
    }
    if !body[cursor..].trim().is_empty() {
        return Err(ModelError::new(
            "model returned trailing text inside a DSML invoke",
        ));
    }
    Ok(parameters)
}

fn dsml_attribute<'a>(tag: &'a str, attribute: &str) -> Option<&'a str> {
    let needle = format!("{attribute}=");
    let mut cursor = 0;
    while let Some(relative_start) = tag[cursor..].find(&needle) {
        let start = cursor + relative_start;
        let boundary_ok = start == 0
            || tag[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let value_start = start + needle.len();
        let quote = tag[value_start..].chars().next()?;
        if boundary_ok && matches!(quote, '\'' | '"') {
            let quoted_value_start = value_start + quote.len_utf8();
            let value_end = tag[quoted_value_start..].find(quote)? + quoted_value_start;
            return Some(&tag[quoted_value_start..value_end]);
        }
        cursor = value_start;
    }
    None
}

pub(crate) fn serialize_tool_calls(tool_calls: &[ModelToolCall]) -> String {
    serde_json::to_string(
        &tool_calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments_json,
                    }
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string())
}

pub fn parse_tool_calls(text: &str) -> Result<Vec<ModelToolCall>, ModelError> {
    let Some(array) = extract_json_array_after(text, "\"tool_calls\"") else {
        return Ok(Vec::new());
    };

    let mut calls = Vec::new();
    for (index, object) in split_top_level_objects(&array).into_iter().enumerate() {
        let id =
            extract_json_string_field(&object, "id").unwrap_or_else(|| format!("call-{index}"));
        let name = extract_json_string_field_after(&object, "\"function\"", "name")
            .ok_or_else(|| ModelError::new("tool call did not include function name"))?;
        let arguments_json = extract_json_string_field_after(&object, "\"function\"", "arguments")
            .unwrap_or_else(|| "{}".to_string());
        calls.push(ModelToolCall {
            id,
            name,
            arguments_json,
        });
    }

    Ok(calls)
}

pub fn tool_function_name(tool_name: &str) -> String {
    let mut name = String::with_capacity(tool_name.len());
    for character in tool_name.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            name.push(character);
        } else {
            name.push('_');
        }
    }
    if name.is_empty() {
        "local_tool".to_string()
    } else {
        name
    }
}

pub fn tool_arguments_to_key_value_input(arguments_json: &str) -> String {
    let trimmed = arguments_json.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if !trimmed.starts_with('{') {
        return normalize_tool_input(trimmed);
    }
    if let Some(input) = extract_json_string_field(trimmed, "input") {
        return normalize_tool_input(&input);
    }

    const FIELDS: &[&str] = &[
        "path",
        "content",
        "command",
        "cwd",
        "query",
        "url",
        "selector",
        "text",
        "x",
        "y",
        "delta_x",
        "delta_y",
        "key",
        "destructive",
        "output_dir",
        "redaction",
        "region",
        "max_results",
        "limit",
    ];
    let mut rows = Vec::new();
    for field in FIELDS {
        if let Some(value) = extract_json_string_field(trimmed, field)
            .or_else(|| extract_json_number_field(trimmed, field))
            .or_else(|| extract_json_bool_field(trimmed, field))
        {
            rows.push(format!("{field}={value}"));
        }
    }
    rows.join("\n")
}

fn normalize_tool_input(input: &str) -> String {
    const ARGUMENT_SEPARATOR: &str = "</arg_key><arg_value>";
    let trimmed = input.trim();
    let Some((raw_key, raw_value)) = trimmed.split_once(ARGUMENT_SEPARATOR) else {
        return trimmed.to_string();
    };
    let key = raw_key.trim().trim_start_matches("<arg_key>").trim();
    let value = raw_value.trim().trim_end_matches("</arg_value>").trim();
    if key.is_empty() {
        trimmed.to_string()
    } else {
        format!("{key}={value}")
    }
}

pub fn parse_provider_error(text: &str) -> Option<String> {
    if !text.contains("\"error\"") {
        return None;
    }

    extract_json_string_field_after(text, "\"error\"", "message")
}
