use agent_core::{Message, MessageRole, Metadata, ModelRole, ToolSpec};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCallMode {
    NonStreaming,
    Streaming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRequest {
    pub role: ModelRole,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub mode: ModelCallMode,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResponse {
    pub message: Message,
    pub raw_tool_calls_json: Option<String>,
    pub tool_calls: Vec<ModelToolCall>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRequest {
    pub input: Vec<String>,
    pub dimensions: Option<usize>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingVector {
    pub index: usize,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingResponse {
    pub model: String,
    pub vectors: Vec<EmbeddingVector>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_embeddings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    pub message: String,
}

impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ModelError {}

pub trait ModelProvider {
    fn name(&self) -> &str;

    fn capabilities(&self) -> ProviderCapabilities;

    fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub embedding_model: String,
    pub timeout_seconds: u64,
}

impl OpenAiCompatibleConfig {
    pub fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }

    pub fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    pub fn embeddings_url(&self) -> String {
        format!("{}/embeddings", self.base_url.trim_end_matches('/'))
    }

    pub fn is_ready(&self) -> bool {
        !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.base_url.trim().is_empty()
    }

    pub fn embeddings_ready(&self) -> bool {
        !self.api_key.trim().is_empty()
            && !self.embedding_model.trim().is_empty()
            && !self.base_url.trim().is_empty()
    }
}

pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        Self { config }
    }

    pub fn list_models(&self) -> Result<Vec<String>, ModelError> {
        if self.config.base_url.trim().is_empty() || self.config.api_key.trim().is_empty() {
            return Err(ModelError::new("provider base URL and API key are required"));
        }

        let output = Command::new("/usr/bin/curl")
            .arg("-sS")
            .arg("--fail-with-body")
            .arg("--max-time")
            .arg(self.config.timeout_seconds.to_string())
            .arg("-H")
            .arg(format!("Authorization: Bearer {}", self.config.api_key))
            .arg(self.config.models_url())
            .output()
            .map_err(|error| ModelError::new(format!("failed to start curl: {error}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let provider_error = parse_provider_error(&stdout).unwrap_or(stderr);
            return Err(ModelError::new(if provider_error.is_empty() {
                format!("model list request failed with status {}", output.status)
            } else {
                provider_error
            }));
        }

        parse_model_list_response(&stdout)
    }

    pub fn complete_streaming(
        &self,
        request: ModelRequest,
        mut on_delta: impl FnMut(&str),
    ) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

        let request_body =
            build_chat_request_json_with_tools(&self.config.model, &request.messages, true, &request.tools)?;
        let mut child = Command::new("/usr/bin/curl")
            .arg("-sS")
            .arg("--no-buffer")
            .arg("--fail-with-body")
            .arg("--max-time")
            .arg(self.config.timeout_seconds.to_string())
            .arg("-X")
            .arg("POST")
            .arg(self.config.chat_completions_url())
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-H")
            .arg(format!("Authorization: Bearer {}", self.config.api_key))
            .arg("-d")
            .arg(request_body)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ModelError::new(format!("failed to start curl: {error}")))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ModelError::new("curl stdout was not available"))?;
        let mut reader = BufReader::new(stdout);
        let mut raw_response = String::new();
        let mut answer = String::new();
        let mut line = String::new();

        loop {
            line.clear();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|error| ModelError::new(format!("failed to read model stream: {error}")))?;
            if bytes == 0 {
                break;
            }
            raw_response.push_str(&line);

            if let Some(delta) = parse_stream_line(&line)? {
                answer.push_str(&delta);
                on_delta(&delta);
            }
        }

        let status = child
            .wait()
            .map_err(|error| ModelError::new(format!("failed to wait for curl: {error}")))?;
        let mut stderr = String::new();
        if let Some(mut stream) = child.stderr.take() {
            stream
                .read_to_string(&mut stderr)
                .map_err(|error| ModelError::new(format!("failed to read curl stderr: {error}")))?;
        }

        if !status.success() {
            let provider_error = parse_provider_error(&raw_response)
                .unwrap_or_else(|| stderr.trim().to_string())
                .trim()
                .to_string();
            return Err(ModelError::new(if provider_error.is_empty() {
                format!("model request failed with status {status}")
            } else {
                provider_error
            }));
        }

        if answer.is_empty() {
            answer = parse_chat_response(&raw_response)?;
        }

        let mut metadata = Metadata::new();
        metadata.insert("provider".to_string(), self.name().to_string());
        metadata.insert("model".to_string(), self.config.model.clone());
        metadata.insert("base_url".to_string(), self.config.base_url.clone());
        metadata.insert("streamed".to_string(), "true".to_string());

        Ok(ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content: answer,
                metadata: metadata.clone(),
            },
            raw_tool_calls_json: None,
            tool_calls: Vec::new(),
            metadata,
        })
    }

    pub fn complete_once(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

        let request_body = build_chat_request_json_with_tools(
            &self.config.model,
            &request.messages,
            false,
            &request.tools,
        )?;
        let output = Command::new("/usr/bin/curl")
            .arg("-sS")
            .arg("--fail-with-body")
            .arg("--max-time")
            .arg(self.config.timeout_seconds.to_string())
            .arg("-X")
            .arg("POST")
            .arg(self.config.chat_completions_url())
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-H")
            .arg(format!("Authorization: Bearer {}", self.config.api_key))
            .arg("-d")
            .arg(request_body)
            .output()
            .map_err(|error| ModelError::new(format!("failed to start curl: {error}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let provider_error = parse_provider_error(&stdout).unwrap_or(stderr);
            return Err(ModelError::new(if provider_error.is_empty() {
                format!("model request failed with status {}", output.status)
            } else {
                provider_error
            }));
        }

        parse_model_response(&stdout)
    }

    pub fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ModelError> {
        if !self.config.embeddings_ready() {
            return Err(ModelError::new("embedding provider config is incomplete"));
        }
        if request.input.is_empty() {
            return Err(ModelError::new("embedding request input is empty"));
        }

        let request_body = build_embedding_request_json(
            &self.config.embedding_model,
            &request.input,
            request.dimensions,
        )?;
        let output = Command::new("/usr/bin/curl")
            .arg("-sS")
            .arg("--fail-with-body")
            .arg("--max-time")
            .arg(self.config.timeout_seconds.to_string())
            .arg("-X")
            .arg("POST")
            .arg(self.config.embeddings_url())
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-H")
            .arg(format!("Authorization: Bearer {}", self.config.api_key))
            .arg("-d")
            .arg(request_body)
            .output()
            .map_err(|error| ModelError::new(format!("failed to start curl: {error}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let provider_error = parse_provider_error(&stdout).unwrap_or(stderr);
            return Err(ModelError::new(if provider_error.is_empty() {
                format!("embedding request failed with status {}", output.status)
            } else {
                provider_error
            }));
        }

        let mut response = parse_embedding_response(&stdout)?;
        response
            .metadata
            .insert("provider".to_string(), self.name().to_string());
        response
            .metadata
            .insert("base_url".to_string(), self.config.base_url.clone());
        if let Some(dimensions) = request.dimensions {
            response
                .metadata
                .insert("dimensions".to_string(), dimensions.to_string());
        }

        Ok(response)
    }
}

pub fn parse_model_list_response(text: &str) -> Result<Vec<String>, ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }

    let data = extract_json_array_after(text, "\"data\"")
        .ok_or_else(|| ModelError::new("model list response did not include data"))?;
    let mut models = split_top_level_objects(&data)
        .into_iter()
        .filter_map(|object| extract_json_string_field(&object, "id"))
        .filter(|model| !model.trim().is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();

    if models.is_empty() {
        return Err(ModelError::new("provider returned an empty model list"));
    }
    Ok(models)
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_embeddings: true,
        }
    }

    fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        match request.mode {
            ModelCallMode::Streaming => self.complete_streaming(request, |_| {}),
            ModelCallMode::NonStreaming => self.complete_once(request),
        }
    }
}

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
    let messages_json = messages
        .iter()
        .map(|message| match message.role {
            MessageRole::Assistant => {
                if let Some(tool_calls_json) = assistant_tool_calls_json(message) {
                    format!(
                        "{{\"role\":\"assistant\",\"content\":\"{}\",\"tool_calls\":{}}}",
                        json_escape(&message.content),
                        tool_calls_json
                    )
                } else {
                    format!(
                        "{{\"role\":\"assistant\",\"content\":\"{}\"}}",
                        json_escape(&message.content)
                    )
                }
            }
            MessageRole::Tool => {
                let tool_call_id = message
                    .metadata
                    .get("tool_call_id")
                    .map(String::as_str)
                    .unwrap_or("tool-call");
                format!(
                    "{{\"role\":\"tool\",\"tool_call_id\":\"{}\",\"content\":\"{}\"}}",
                    json_escape(tool_call_id),
                    json_escape(&message.content)
                )
            }
            _ => format!(
                "{{\"role\":\"{}\",\"content\":\"{}\"}}",
                json_escape(message_role_to_str(&message.role)),
                json_escape(&message.content)
            ),
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

    Ok(format!(
        "{{\"model\":\"{}\",\"stream\":{},\"messages\":[{}]{}}}",
        json_escape(model),
        if stream { "true" } else { "false" },
        messages_json.join(","),
        tools_json
    ))
}

pub fn parse_stream_line(line: &str) -> Result<Option<String>, ModelError> {
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

    Ok(extract_json_string_field_after(payload, "\"delta\"", "content"))
}

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

    let content = extract_json_string_field_after(text, "\"message\"", "content")
        .unwrap_or_default();
    let tool_calls = parse_tool_calls(text)?;
    let raw_tool_calls_json = extract_json_array_after(text, "\"tool_calls\"");
    let mut metadata = Metadata::new();
    metadata.insert("tool_calls".to_string(), tool_calls.len().to_string());
    for field in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        if let Some(value) = extract_json_number_field(text, field) {
            metadata.insert(field.to_string(), value);
        }
    }

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

pub fn parse_tool_calls(text: &str) -> Result<Vec<ModelToolCall>, ModelError> {
    let Some(array) = extract_json_array_after(text, "\"tool_calls\"") else {
        return Ok(Vec::new());
    };

    let mut calls = Vec::new();
    for (index, object) in split_top_level_objects(&array).into_iter().enumerate() {
        let id = extract_json_string_field(&object, "id")
            .unwrap_or_else(|| format!("call-{index}"));
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
    let key = raw_key
        .trim()
        .trim_start_matches("<arg_key>")
        .trim();
    let value = raw_value
        .trim()
        .trim_end_matches("</arg_value>")
        .trim();
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
    let description = format!(
        "{} Original tool name: {}.",
        tool.description,
        tool.name
    );
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

fn assistant_tool_calls_json(message: &Message) -> Option<&str> {
    let raw = message
        .metadata
        .get("raw_tool_calls_json")
        .map(String::as_str)?
        .trim();
    if raw.starts_with('[') && raw.ends_with(']') {
        Some(raw)
    } else {
        None
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            other if other.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

fn extract_json_string_field_after(text: &str, marker: &str, field: &str) -> Option<String> {
    let start = if marker.is_empty() {
        0
    } else {
        text.find(marker)? + marker.len()
    };
    extract_json_string_field(&text[start..], field)
}

fn extract_json_string_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let mut offset = 0;

    while let Some(position) = text[offset..].find(&pattern) {
        let mut index = offset + position + pattern.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        if text.as_bytes().get(index).copied() != Some(b'"') {
            offset = index.saturating_add(1);
            continue;
        }

        return parse_json_string_at(text, index).ok();
    }

    None
}

fn extract_json_number_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let mut offset = 0;

    while let Some(position) = text[offset..].find(&pattern) {
        let mut index = offset + position + pattern.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        let start = index;
        while matches!(
            text.as_bytes().get(index).copied(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            index += 1;
        }
        if start != index {
            return Some(text[start..index].to_string());
        }
        offset = index.saturating_add(1);
    }

    None
}

fn extract_json_bool_field(text: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{field}\"");
    let mut offset = 0;

    while let Some(position) = text[offset..].find(&pattern) {
        let mut index = offset + position + pattern.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        if text[index..].starts_with("true") {
            return Some("true".to_string());
        }
        if text[index..].starts_with("false") {
            return Some("false".to_string());
        }
        offset = index.saturating_add(1);
    }

    None
}

fn extract_json_array_after(text: &str, marker: &str) -> Option<String> {
    let mut offset = 0;
    while let Some(position) = text[offset..].find(marker) {
        let mut index = offset + position + marker.len();
        index = skip_whitespace(text, index);
        if text.as_bytes().get(index).copied() != Some(b':') {
            offset = index.saturating_add(1);
            continue;
        }
        index = skip_whitespace(text, index + 1);
        if text.as_bytes().get(index).copied() != Some(b'[') {
            offset = index.saturating_add(1);
            continue;
        }

        return extract_balanced_array(text, index);
    }

    None
}

fn extract_balanced_array(text: &str, open_index: usize) -> Option<String> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text[open_index..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = open_index + offset + character.len_utf8();
                    return Some(text[open_index..end].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

fn split_top_level_objects(array: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;

    for (index, character) in array.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    if let Some(start) = start.take() {
                        objects.push(array[start..index + character.len_utf8()].to_string());
                    }
                }
            }
            _ => {}
        }
    }

    objects
}

fn parse_number_array(array: &str) -> Result<Vec<f32>, ModelError> {
    let trimmed = array.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| ModelError::new("number array was malformed"))?;
    let mut values = Vec::new();
    for part in inner.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        values.push(
            part.parse::<f32>()
                .map_err(|error| ModelError::new(format!("invalid embedding number: {error}")))?,
        );
    }

    Ok(values)
}

fn skip_whitespace(text: &str, mut index: usize) -> usize {
    while matches!(
        text.as_bytes().get(index).copied(),
        Some(b' ' | b'\n' | b'\r' | b'\t')
    ) {
        index += 1;
    }
    index
}

fn parse_json_string_at(text: &str, quote_index: usize) -> Result<String, ModelError> {
    if text.as_bytes().get(quote_index).copied() != Some(b'"') {
        return Err(ModelError::new("json string did not start with a quote"));
    }

    let mut output = String::new();
    let mut index = quote_index + 1;

    while index < text.len() {
        let character = text[index..]
            .chars()
            .next()
            .ok_or_else(|| ModelError::new("invalid json string"))?;
        if character == '"' {
            return Ok(output);
        }
        if character != '\\' {
            output.push(character);
            index += character.len_utf8();
            continue;
        }

        index += 1;
        let escape = text[index..]
            .chars()
            .next()
            .ok_or_else(|| ModelError::new("unterminated json escape"))?;
        match escape {
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            '/' => output.push('/'),
            'b' => output.push('\u{08}'),
            'f' => output.push('\u{0c}'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            'u' => {
                let start = index + 1;
                let end = start + 4;
                let hex = text
                    .get(start..end)
                    .ok_or_else(|| ModelError::new("invalid unicode escape"))?;
                let value = u32::from_str_radix(hex, 16)
                    .map_err(|error| ModelError::new(format!("invalid unicode escape: {error}")))?;
                let character = char::from_u32(value)
                    .ok_or_else(|| ModelError::new("unicode escape is not a valid scalar"))?;
                output.push(character);
                index = end;
                continue;
            }
            other => {
                return Err(ModelError::new(format!("unsupported json escape: {other}")));
            }
        }
        index += escape.len_utf8();
    }

    Err(ModelError::new("unterminated json string"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MessageRole, ToolRisk};

    #[test]
    fn chat_url_trims_base_url_slashes() {
        let config = OpenAiCompatibleConfig {
            base_url: "https://example.test/v1/".to_string(),
            api_key: "key".to_string(),
            model: "model-a".to_string(),
            embedding_model: "embed-a".to_string(),
            timeout_seconds: 10,
        };

        assert_eq!(
            config.chat_completions_url(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(config.models_url(), "https://example.test/v1/models");
        assert_eq!(config.embeddings_url(), "https://example.test/v1/embeddings");
    }

    #[test]
    fn parses_sorted_unique_model_list() {
        let models = parse_model_list_response(
            r#"{"object":"list","data":[{"id":"model-b"},{"id":"model-a"},{"id":"model-b"}]}"#,
        )
        .expect("model list should parse");

        assert_eq!(models, vec!["model-a", "model-b"]);
    }

    #[test]
    fn request_json_uses_chat_messages() {
        let body = build_chat_request_json(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            true,
        )
        .expect("body should encode");

        assert!(body.contains("\"model\":\"model-a\""));
        assert!(body.contains("\"stream\":true"));
        assert!(body.contains("\"role\":\"user\""));
        assert!(body.contains("\"content\":\"hello\""));
    }

    #[test]
    fn request_json_includes_openai_tool_definitions() {
        let body = build_chat_request_json_with_tools(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "read README".to_string(),
                metadata: Metadata::new(),
            }],
            false,
            &[ToolSpec::builtin(
                "file.read",
                "file",
                "Read a file.",
                ToolRisk::ReadOnly,
                r#"{"type":"object","properties":{"path":{"type":"string","description":"workspace-relative path"}},"required":["path"],"additionalProperties":false}"#,
            )],
        )
        .expect("body should encode");

        assert!(body.contains("\"tools\""));
        assert!(body.contains("\"name\":\"file_read\""));
        assert!(body.contains("Original tool name: file.read"));
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert_eq!(
            value["tools"][0]["function"]["parameters"]["properties"]["path"]["type"],
            "string"
        );
        assert_eq!(
            value["tools"][0]["function"]["parameters"]["required"][0],
            "path"
        );
        assert!(body.contains("\"tool_choice\":\"auto\""));
    }

    #[test]
    fn request_json_preserves_assistant_tool_calls_and_tool_messages() {
        let mut assistant_metadata = Metadata::new();
        assistant_metadata.insert(
            "raw_tool_calls_json".to_string(),
            r#"[{"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]"#.to_string(),
        );
        let mut tool_metadata = Metadata::new();
        tool_metadata.insert("tool_call_id".to_string(), "call_1".to_string());

        let body = build_chat_request_json_with_tools(
            "model-a",
            &[
                Message {
                    role: MessageRole::Assistant,
                    content: String::new(),
                    metadata: assistant_metadata,
                },
                Message {
                    role: MessageRole::Tool,
                    content: "tool=file.read\nstatus=succeeded\noutput=hello".to_string(),
                    metadata: tool_metadata,
                },
            ],
            false,
            &[],
        )
        .expect("body should encode");

        assert!(body.contains("\"role\":\"assistant\""));
        assert!(body.contains("\"tool_calls\":[{\"id\":\"call_1\""));
        assert!(body.contains("\"role\":\"tool\""));
        assert!(body.contains("\"tool_call_id\":\"call_1\""));
    }

    #[test]
    fn parses_streaming_delta_lines() {
        let delta = parse_stream_line(
            r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#,
        )
        .expect("line should parse");

        assert_eq!(delta.as_deref(), Some("hi"));
        assert_eq!(parse_stream_line("data: [DONE]").expect("done"), None);
    }

    #[test]
    fn parses_non_streaming_chat_response() {
        let text = r#"{"choices":[{"message":{"role":"assistant","content":"done"}}],"usage":{"prompt_tokens":21,"completion_tokens":4,"total_tokens":25}}"#;
        let answer = parse_chat_response(text)
        .expect("response should parse");

        assert_eq!(answer, "done");
        let response = parse_model_response(text).expect("model response should parse");
        assert_eq!(response.metadata.get("prompt_tokens").map(String::as_str), Some("21"));
        assert_eq!(response.metadata.get("total_tokens").map(String::as_str), Some("25"));
    }

    #[test]
    fn parses_non_streaming_tool_calls() {
        let response = parse_model_response(
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]}}]}"#,
        )
        .expect("response should parse");

        assert_eq!(response.message.content, "");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_1");
        assert_eq!(response.tool_calls[0].name, "file_read");
        assert_eq!(
            response.tool_calls[0].arguments_json,
            r#"{"input":"path=README.md"}"#
        );
        assert!(response.raw_tool_calls_json.is_some());
    }

    #[test]
    fn converts_tool_arguments_to_key_value_input() {
        assert_eq!(
            tool_arguments_to_key_value_input(r#"{"input":"path=README.md"}"#),
            "path=README.md"
        );
        assert_eq!(
            tool_arguments_to_key_value_input(r#"{"path":"README.md","limit":3,"destructive":false}"#),
            "path=README.md\ndestructive=false\nlimit=3"
        );
        assert_eq!(
            tool_arguments_to_key_value_input(
                r#"{"input":"command</arg_key><arg_value>ls scripts/ && cat Cargo.toml"}"#
            ),
            "command=ls scripts/ && cat Cargo.toml"
        );
    }

    #[test]
    fn parses_provider_errors() {
        let error = parse_provider_error(r#"{"error":{"message":"bad key"}}"#);

        assert_eq!(error.as_deref(), Some("bad key"));
    }

    #[test]
    fn embedding_request_json_includes_dimensions_when_set() {
        let body = build_embedding_request_json(
            "text-embedding-model",
            &["alpha".to_string(), "beta".to_string()],
            Some(256),
        )
        .expect("embedding body should encode");

        assert!(body.contains("\"model\":\"text-embedding-model\""));
        assert!(body.contains("\"input\":[\"alpha\",\"beta\"]"));
        assert!(body.contains("\"dimensions\":256"));
    }

    #[test]
    fn parses_embedding_response_vectors_by_index() {
        let response = parse_embedding_response(
            r#"{"object":"list","model":"embed-a","data":[{"object":"embedding","index":1,"embedding":[0.3,0.4]},{"object":"embedding","index":0,"embedding":[0.1,0.2]}]}"#,
        )
        .expect("embedding response should parse");

        assert_eq!(response.model, "embed-a");
        assert_eq!(response.vectors.len(), 2);
        assert_eq!(response.vectors[0].index, 0);
        assert_eq!(response.vectors[0].embedding, vec![0.1, 0.2]);
        assert_eq!(response.vectors[1].index, 1);
        assert_eq!(response.metadata.get("vectors").map(String::as_str), Some("2"));
    }

    #[test]
    fn provider_capabilities_include_embeddings() {
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            api_key: "key".to_string(),
            model: "model-a".to_string(),
            embedding_model: "embed-a".to_string(),
            timeout_seconds: 10,
        });

        assert!(provider.capabilities().supports_embeddings);
        assert!(provider.capabilities().supports_tools);
    }
}
