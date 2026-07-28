use agent_core::{Message, Metadata, ModelRole, ToolSpec};
use futures_util::StreamExt;
use reqwest::{header, Client, RequestBuilder, Response, StatusCode};
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

mod error;
mod image_provider;
mod json_wire;
mod request_builder;
mod response_parser;
mod streaming_response;
mod streaming_wire;
mod usage;

use json_wire::{
    extract_json_array_after, extract_json_string_field, split_top_level_objects,
};
use request_builder::{
    build_chat_request_json_with_tools_and_output_limit, model_supports_vision_content,
};
use streaming_response::{
    consume_streaming_body, consume_streaming_response, finish_streaming_response,
};

pub use error::{classify_provider_failure, ProviderFailureClass};
pub use image_provider::{
    build_image_generation_request_json, OpenAiCompatibleImageConfig,
    OpenAiCompatibleImageProvider,
};
pub use request_builder::{
    build_chat_request_json, build_chat_request_json_with_tools, build_embedding_request_json,
    parse_embedding_response,
};
pub use response_parser::{
    parse_chat_response, parse_model_response, parse_provider_error, parse_tool_calls,
    tool_arguments_to_key_value_input, tool_function_name,
};
pub use streaming_wire::parse_stream_line;
pub use usage::{
    estimate_completion_tokens, estimate_request_tokens, estimate_text_tokens,
    normalize_model_usage, UsageSource,
};

pub const MODEL_REQUEST_CANCELLED: &str = "model request cancelled";
const STREAMING_HARD_TIMEOUT_MULTIPLIER: u64 = 4;
const HTTP_POLL_INTERVAL: Duration = Duration::from_millis(40);
const MAX_MODEL_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const DSML_TOOL_CALLS_OPEN: &str = "<｜DSML｜tool_calls>";
const DSML_TOOL_CALLS_CLOSE: &str = "</｜DSML｜tool_calls>";
const DSML_INVOKE_OPEN: &str = "<｜DSML｜invoke";
const DSML_INVOKE_CLOSE: &str = "</｜DSML｜invoke>";
const DSML_PARAMETER_OPEN: &str = "<｜DSML｜parameter";
const DSML_PARAMETER_CLOSE: &str = "</｜DSML｜parameter>";
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static HTTP_RUNTIME: OnceLock<Runtime> = OnceLock::new();

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResponseTermination {
    Complete,
    ToolCalls,
    OutputLimit,
    ContentFiltered,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResponseDisposition {
    Usable,
    ToolCalls,
    Empty,
    IncompleteOutput,
    Filtered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelResponseAssessment {
    pub termination: ModelResponseTermination,
    pub disposition: ModelResponseDisposition,
}

impl ModelResponse {
    pub fn assessment(&self) -> ModelResponseAssessment {
        let finish_reason = self
            .metadata
            .get("finish_reason")
            .map(|value| value.trim().to_ascii_lowercase());
        let termination = if !self.tool_calls.is_empty()
            || matches!(
                finish_reason.as_deref(),
                Some("tool_calls" | "function_call")
            ) {
            ModelResponseTermination::ToolCalls
        } else {
            match finish_reason.as_deref() {
                Some("stop" | "end_turn" | "completed") => ModelResponseTermination::Complete,
                Some("length" | "max_tokens" | "max_output_tokens") => {
                    ModelResponseTermination::OutputLimit
                }
                Some("content_filter" | "safety" | "blocked") => {
                    ModelResponseTermination::ContentFiltered
                }
                _ => ModelResponseTermination::Unknown,
            }
        };
        let disposition = match termination {
            ModelResponseTermination::ToolCalls => ModelResponseDisposition::ToolCalls,
            ModelResponseTermination::OutputLimit => ModelResponseDisposition::IncompleteOutput,
            ModelResponseTermination::ContentFiltered => ModelResponseDisposition::Filtered,
            ModelResponseTermination::Complete | ModelResponseTermination::Unknown => {
                if self.message.content.trim().is_empty() {
                    ModelResponseDisposition::Empty
                } else {
                    ModelResponseDisposition::Usable
                }
            }
        };

        ModelResponseAssessment {
            termination,
            disposition,
        }
    }
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
pub struct ImageGenerationRequest {
    pub prompt: String,
    pub size: Option<String>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub revised_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationResponse {
    pub model: String,
    pub images: Vec<GeneratedImage>,
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
    pub class: ProviderFailureClass,
    pub status_code: Option<u16>,
    pub retryable: bool,
}

impl ModelError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let class = classify_provider_failure(&message, None);
        Self {
            message,
            class,
            status_code: None,
            retryable: class.is_retryable(),
        }
    }

    pub fn with_status(status_code: u16, message: impl Into<String>) -> Self {
        let message = message.into();
        let class = classify_provider_failure(&message, Some(status_code));
        Self {
            message,
            class,
            status_code: Some(status_code),
            retryable: class.is_retryable(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub fn is_cancelled(&self) -> bool {
        self.class == ProviderFailureClass::Cancelled
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

pub trait StreamingModelProvider: Send + Sync {
    fn complete_streaming_cancellable(
        &self,
        request: ModelRequest,
        on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError>;
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

fn streaming_hard_timeout_seconds(idle_timeout_seconds: u64) -> u64 {
    idle_timeout_seconds
        .max(1)
        .saturating_mul(STREAMING_HARD_TIMEOUT_MULTIPLIER)
}

fn http_client() -> Result<&'static Client, ModelError> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client);
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .map_err(|error| ModelError::new(format!("failed to initialize HTTP client: {error}")))?;
    let _ = HTTP_CLIENT.set(client);
    HTTP_CLIENT
        .get()
        .ok_or_else(|| ModelError::new("HTTP client did not initialize"))
}

fn http_runtime() -> Result<&'static Runtime, ModelError> {
    if let Some(runtime) = HTTP_RUNTIME.get() {
        return Ok(runtime);
    }
    let runtime = RuntimeBuilder::new_multi_thread()
        .worker_threads(2)
        .thread_name("cindx-model-http")
        .enable_all()
        .build()
        .map_err(|error| ModelError::new(format!("failed to initialize HTTP runtime: {error}")))?;
    let _ = HTTP_RUNTIME.set(runtime);
    HTTP_RUNTIME
        .get()
        .ok_or_else(|| ModelError::new("HTTP runtime did not initialize"))
}

fn run_http<T>(future: impl Future<Output = Result<T, ModelError>>) -> Result<T, ModelError> {
    http_runtime()?.block_on(future)
}

fn http_request(
    url: &str,
    api_key: &str,
    request_body: Option<&str>,
    hard_timeout: Duration,
    streaming: bool,
) -> Result<RequestBuilder, ModelError> {
    let client = http_client()?;
    let mut request = if let Some(request_body) = request_body {
        client
            .post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(request_body.to_string())
    } else {
        client.get(url)
    };
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key);
    }
    request = request.header(
        header::ACCEPT,
        if streaming {
            "text/event-stream"
        } else {
            "application/json"
        },
    );
    Ok(request.timeout(hard_timeout))
}

async fn await_http<T, E>(
    future: impl Future<Output = Result<T, E>>,
    deadline: Instant,
    timeout: Duration,
    action: &str,
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<T, ModelError>
where
    E: std::fmt::Display,
{
    let mut future = Box::pin(future);
    loop {
        if should_cancel() {
            return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
        }
        if Instant::now() >= deadline {
            return Err(ModelError::new(format!(
                "{action} timed out after {} seconds",
                timeout.as_secs().max(1)
            )));
        }
        match tokio::time::timeout(HTTP_POLL_INTERVAL, future.as_mut()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error)) => return Err(ModelError::new(format!("{action} failed: {error}"))),
            Err(_) => continue,
        }
    }
}

async fn collect_response_body(
    response: Response,
    max_bytes: usize,
    deadline: Instant,
    timeout: Duration,
    action: &str,
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<Vec<u8>, ModelError> {
    let mut stream = Box::pin(response.bytes_stream());
    let mut bytes = Vec::new();
    loop {
        if should_cancel() {
            return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
        }
        if Instant::now() >= deadline {
            return Err(ModelError::new(format!(
                "{action} timed out after {} seconds",
                timeout.as_secs().max(1)
            )));
        }
        match tokio::time::timeout(HTTP_POLL_INTERVAL, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                if bytes.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(ModelError::new(format!(
                        "{action} exceeded {} MB",
                        max_bytes / (1024 * 1024)
                    )));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Some(Err(error))) => {
                return Err(ModelError::new(format!(
                    "{action} failed while reading response: {error}"
                )))
            }
            Ok(None) => return Ok(bytes),
            Err(_) => continue,
        }
    }
}

struct HttpOutput {
    status: StatusCode,
    body: Vec<u8>,
}

fn execute_http(
    url: &str,
    api_key: &str,
    request_body: Option<&str>,
    timeout_seconds: u64,
    max_response_bytes: usize,
) -> Result<HttpOutput, ModelError> {
    execute_http_cancellable(
        url,
        api_key,
        request_body,
        timeout_seconds,
        max_response_bytes,
        &mut || false,
    )
}

fn execute_http_cancellable(
    url: &str,
    api_key: &str,
    request_body: Option<&str>,
    timeout_seconds: u64,
    max_response_bytes: usize,
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<HttpOutput, ModelError> {
    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let deadline = Instant::now() + timeout;
    let request = http_request(url, api_key, request_body, timeout, false)?;
    run_http(async {
        let response = await_http(
            request.send(),
            deadline,
            timeout,
            "model request",
            should_cancel,
        )
        .await?;
        let status = response.status();
        let body = collect_response_body(
            response,
            max_response_bytes,
            deadline,
            timeout,
            "model response",
            should_cancel,
        )
        .await?;
        Ok(HttpOutput { status, body })
    })
}

#[derive(Default)]
struct DsmlStreamDeltaFilter {
    pending: String,
    inside_protocol: bool,
}

impl DsmlStreamDeltaFilter {
    fn push(&mut self, delta: &str, on_delta: &mut impl FnMut(&str)) {
        self.pending.push_str(delta);
        loop {
            if self.inside_protocol {
                if let Some(end) = self.pending.find(DSML_TOOL_CALLS_CLOSE) {
                    self.pending.drain(..end + DSML_TOOL_CALLS_CLOSE.len());
                    self.inside_protocol = false;
                    continue;
                }
                let retained = marker_prefix_suffix_len(&self.pending, DSML_TOOL_CALLS_CLOSE);
                let discarded = self.pending.len() - retained;
                self.pending.drain(..discarded);
                return;
            }

            if let Some(start) = self.pending.find(DSML_TOOL_CALLS_OPEN) {
                if start > 0 {
                    on_delta(&self.pending[..start]);
                }
                self.pending.drain(..start + DSML_TOOL_CALLS_OPEN.len());
                self.inside_protocol = true;
                continue;
            }

            let retained = marker_prefix_suffix_len(&self.pending, DSML_TOOL_CALLS_OPEN);
            let visible_len = self.pending.len() - retained;
            if visible_len > 0 {
                on_delta(&self.pending[..visible_len]);
                self.pending.drain(..visible_len);
            }
            return;
        }
    }

    fn finish(&mut self, on_delta: &mut impl FnMut(&str)) {
        if !self.inside_protocol && !self.pending.is_empty() {
            on_delta(&self.pending);
        }
        self.pending.clear();
    }
}

fn marker_prefix_suffix_len(value: &str, marker: &str) -> usize {
    let mut longest = 0;
    for (index, _) in marker.char_indices().skip(1) {
        if value.ends_with(&marker[..index]) {
            longest = index;
        }
    }
    longest
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        Self { config }
    }

    pub fn list_models(&self) -> Result<Vec<String>, ModelError> {
        if self.config.base_url.trim().is_empty() || self.config.api_key.trim().is_empty() {
            return Err(ModelError::new(
                "provider base URL and API key are required",
            ));
        }

        let output = execute_http(
            &self.config.models_url(),
            &self.config.api_key,
            None,
            self.config.timeout_seconds,
            MAX_MODEL_RESPONSE_BYTES,
        )?;

        let stdout = String::from_utf8_lossy(&output.body).to_string();
        if !output.status.is_success() {
            let provider_error = parse_provider_error(&stdout).unwrap_or_default();
            return Err(ModelError::with_status(
                output.status.as_u16(),
                if provider_error.is_empty() {
                    format!("model list request failed with status {}", output.status)
                } else {
                    provider_error
                },
            ));
        }

        parse_model_list_response(&stdout)
    }

    pub fn complete_streaming(
        &self,
        request: ModelRequest,
        on_delta: impl FnMut(&str),
    ) -> Result<ModelResponse, ModelError> {
        self.complete_streaming_cancellable(request, on_delta, || false)
    }

    pub fn complete_streaming_cancellable(
        &self,
        request: ModelRequest,
        mut on_delta: impl FnMut(&str),
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

        let estimated_prompt_tokens = estimate_request_tokens(&request.messages, &request.tools);
        let max_output_tokens = request
            .metadata
            .get("max_output_tokens")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        let request_body = build_chat_request_json_with_tools_and_output_limit(
            &self.config.model,
            &request.messages,
            true,
            &request.tools,
            max_output_tokens,
        )?;
        let idle_timeout = Duration::from_secs(self.config.timeout_seconds.max(1));
        let hard_timeout =
            Duration::from_secs(streaming_hard_timeout_seconds(self.config.timeout_seconds));
        let deadline = Instant::now() + hard_timeout;
        let http_request = http_request(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(&request_body),
            hard_timeout,
            true,
        )?;
        let mut response = run_http(async {
            let response = await_http(
                http_request.send(),
                deadline,
                hard_timeout,
                "model stream request",
                &mut should_cancel,
            )
            .await?;
            consume_streaming_response(
                response,
                &self.config.model,
                &self.config.base_url,
                idle_timeout,
                hard_timeout,
                deadline,
                &mut on_delta,
                &mut should_cancel,
            )
            .await
        })?;
        normalize_model_usage(&mut response, estimated_prompt_tokens);
        Ok(response)
    }

    pub fn complete_once(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

        let estimated_prompt_tokens = estimate_request_tokens(&request.messages, &request.tools);
        let max_output_tokens = request
            .metadata
            .get("max_output_tokens")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0);
        let request_body = build_chat_request_json_with_tools_and_output_limit(
            &self.config.model,
            &request.messages,
            false,
            &request.tools,
            max_output_tokens,
        )?;
        let output = execute_http(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
            MAX_MODEL_RESPONSE_BYTES,
        )?;

        let stdout = String::from_utf8_lossy(&output.body).to_string();
        if !output.status.is_success() {
            let provider_error = parse_provider_error(&stdout).unwrap_or_default();
            return Err(ModelError::with_status(
                output.status.as_u16(),
                if provider_error.is_empty() {
                    format!("model request failed with status {}", output.status)
                } else {
                    provider_error
                },
            ));
        }

        let mut response = parse_model_response(&stdout)?;
        normalize_model_usage(&mut response, estimated_prompt_tokens);
        Ok(response)
    }

    pub fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ModelError> {
        self.embed_cancellable(request, || false)
    }

    pub fn embed_cancellable(
        &self,
        request: EmbeddingRequest,
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<EmbeddingResponse, ModelError> {
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
        let output = execute_http_cancellable(
            &self.config.embeddings_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
            MAX_MODEL_RESPONSE_BYTES,
            &mut should_cancel,
        )?;

        let stdout = String::from_utf8_lossy(&output.body).to_string();
        if !output.status.is_success() {
            let provider_error = parse_provider_error(&stdout).unwrap_or_default();
            return Err(ModelError::with_status(
                output.status.as_u16(),
                if provider_error.is_empty() {
                    format!("embedding request failed with status {}", output.status)
                } else {
                    provider_error
                },
            ));
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

impl StreamingModelProvider for OpenAiCompatibleProvider {
    fn complete_streaming_cancellable(
        &self,
        request: ModelRequest,
        on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        OpenAiCompatibleProvider::complete_streaming_cancellable(
            self,
            request,
            |delta| on_delta(delta),
            should_cancel,
        )
    }
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: model_supports_vision_content(&self.config.model),
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MessageRole, ToolRisk};
    use base64::Engine;
    use crate::image_provider::{
        build_dashscope_image_generation_request_json, decode_generated_image,
        generated_image_mime_type, image_endpoint_probe_succeeded,
        parse_image_generation_payloads, ImagePayload,
    };
    use crate::streaming_wire::{parse_stream_event, StreamingToolCall};
    use std::fs;
    use futures_util::{stream, Stream};
    use std::collections::BTreeMap;
    use std::time::Instant;

    struct ScriptedStreamingProvider;

    impl StreamingModelProvider for ScriptedStreamingProvider {
        fn complete_streaming_cancellable(
            &self,
            request: ModelRequest,
            on_delta: &mut dyn FnMut(&str),
            should_cancel: &mut dyn FnMut() -> bool,
        ) -> Result<ModelResponse, ModelError> {
            if should_cancel() {
                return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
            }
            on_delta("scripted ");
            on_delta("answer");
            Ok(ModelResponse {
                message: Message {
                    role: MessageRole::Assistant,
                    content: "scripted answer".to_string(),
                    metadata: Metadata::new(),
                },
                raw_tool_calls_json: None,
                tool_calls: Vec::new(),
                metadata: request.metadata,
            })
        }
    }

    #[test]
    fn streaming_provider_contract_is_object_safe_and_preserves_callbacks() {
        let provider: &dyn StreamingModelProvider = &ScriptedStreamingProvider;
        let mut output = String::new();
        let mut on_delta = |delta: &str| output.push_str(delta);
        let mut should_cancel = || false;
        let response = provider
            .complete_streaming_cancellable(
                ModelRequest {
                    role: ModelRole::Executor,
                    messages: Vec::new(),
                    tools: Vec::new(),
                    mode: ModelCallMode::Streaming,
                    metadata: [("request_id".to_string(), "scripted".to_string())]
                        .into_iter()
                        .collect(),
                },
                &mut on_delta,
                &mut should_cancel,
            )
            .unwrap();

        assert_eq!(output, "scripted answer");
        assert_eq!(response.message.content, "scripted answer");
        assert_eq!(response.metadata["request_id"], "scripted");
    }

    fn delayed_stream(
        chunks: Vec<(Duration, &'static str)>,
    ) -> impl Stream<Item = Result<Vec<u8>, ModelError>> {
        stream::unfold(chunks.into_iter(), |mut chunks| async move {
            let (delay, chunk) = chunks.next()?;
            tokio::time::sleep(delay).await;
            Some((Ok(chunk.as_bytes().to_vec()), chunks))
        })
    }

    fn consume_test_stream(
        chunks: Vec<(Duration, &'static str)>,
        idle_timeout: Duration,
        on_delta: &mut impl FnMut(&str),
        should_cancel: &mut impl FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        let hard_timeout = Duration::from_secs(10);
        let deadline = Instant::now() + hard_timeout;
        run_http(consume_streaming_body(
            delayed_stream(chunks),
            "test-model",
            "http://example.test/v1",
            idle_timeout,
            hard_timeout,
            deadline,
            on_delta,
            should_cancel,
        ))
    }

    #[test]
    fn streaming_reader_preserves_incremental_usage_without_buffering_raw_body() {
        let mut visible = String::new();
        let response = consume_test_stream(
            vec![
                (
                    Duration::ZERO,
                    "data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n",
                ),
                (
                    Duration::ZERO,
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":4,\"total_tokens\":25}}\n\n",
                ),
                (Duration::ZERO, "data: [DONE]\n\n"),
            ],
            Duration::from_secs(1),
            &mut |delta| visible.push_str(delta),
            &mut || false,
        )
        .expect("stream should preserve usage metadata");

        assert_eq!(visible, "done");
        assert_eq!(response.message.content, "done");
        assert_eq!(
            response.metadata.get("prompt_tokens").map(String::as_str),
            Some("21")
        );
        assert_eq!(
            response.metadata.get("completion_tokens").map(String::as_str),
            Some("4")
        );
        assert_eq!(
            response.metadata.get("total_tokens").map(String::as_str),
            Some("25")
        );
    }

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
        assert_eq!(
            config.embeddings_url(),
            "https://example.test/v1/embeddings"
        );
    }

    #[test]
    fn image_endpoint_probe_accepts_validation_and_auth_responses() {
        for status in [200, 400, 401, 403, 422, 429] {
            assert!(image_endpoint_probe_succeeded(
                StatusCode::from_u16(status).expect("status should be valid")
            ));
        }
        for status in [404, 405, 500] {
            assert!(!image_endpoint_probe_succeeded(
                StatusCode::from_u16(status).expect("status should be valid")
            ));
        }
    }

    #[test]
    fn pooled_http_request_keeps_credentials_in_headers_and_bodies_in_memory() {
        let request_text = format!("{{\"prompt\":\"{}\"}}", "x".repeat(2_000_000));
        let request = http_request(
            "https://example.test/v1/chat/completions",
            "test-secret",
            Some(&request_text),
            Duration::from_secs(10),
            false,
        )
        .expect("request should build")
        .build()
        .expect("request should be valid");

        assert_eq!(
            request.url().as_str(),
            "https://example.test/v1/chat/completions"
        );
        assert_eq!(
            request
                .headers()
                .get(header::AUTHORIZATION)
                .expect("authorization header should exist"),
            "Bearer test-secret"
        );
        assert_eq!(
            request
                .body()
                .and_then(|body| body.as_bytes())
                .expect("request body should remain in memory")
                .len(),
            request_text.len()
        );
        assert!(std::ptr::eq(
            http_client().expect("shared client should exist"),
            http_client().expect("shared client should be reused")
        ));
    }

    #[test]
    fn streaming_uses_idle_timeout_with_a_larger_hard_ceiling() {
        assert_eq!(streaming_hard_timeout_seconds(180), 720);
        assert_eq!(streaming_hard_timeout_seconds(0), 4);
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
    fn request_json_includes_configured_output_limit() {
        let body = build_chat_request_json_with_tools_and_output_limit(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            true,
            &[],
            Some(4096),
        )
        .expect("body should encode");

        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert_eq!(value["max_tokens"], 4096);
    }

    #[test]
    fn request_json_embeds_trusted_image_paths_as_vision_content() {
        let root = std::env::temp_dir().join(format!(
            "cindx-provider-vision-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let path = root.join(".cindx/vision.png");
        fs::create_dir_all(path.parent().expect("fixture should have a parent"))
            .expect("fixture directory should write");
        fs::write(&path, [0x89, b'P', b'N', b'G']).expect("image fixture should write");
        let body = build_chat_request_json(
            "qwen-vl-max",
            &[Message {
                role: MessageRole::User,
                content: "Inspect this screenshot".to_string(),
                metadata: [("image_paths".to_string(), path.display().to_string())]
                    .into_iter()
                    .collect(),
            }],
            false,
        )
        .expect("body should encode");
        let _ = fs::remove_dir_all(root);

        assert!(body.contains("\"type\":\"image_url\""));
        assert!(body.contains("data:image/png;base64,"));
        assert!(body.contains("Inspect this screenshot"));
    }

    #[test]
    fn request_json_omits_image_parts_for_text_only_models() {
        let body = build_chat_request_json(
            "qwen3.7-max",
            &[Message {
                role: MessageRole::User,
                content: "Inspect this screenshot".to_string(),
                metadata: [(
                    "image_paths".to_string(),
                    "/missing/.cindx/vision.png".to_string(),
                )]
                .into_iter()
                .collect(),
            }],
            false,
        )
        .expect("body should encode");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");

        assert!(value["messages"][0]["content"].is_string());
        assert!(!body.contains("\"type\":\"image_url\""));
        assert!(body.contains("selected model does not support vision"));
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
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");

        assert!(body.contains("\"role\":\"assistant\""));
        assert_eq!(value["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert!(body.contains("\"role\":\"tool\""));
        assert!(body.contains("\"tool_call_id\":\"call_1\""));
    }

    #[test]
    fn request_json_drops_corrupt_tool_calls_and_orphan_tool_messages() {
        let mut assistant_metadata = Metadata::new();
        assistant_metadata.insert(
            "raw_tool_calls_json".to_string(),
            r#"[{"id":"call_broken","type":"function","function":{"name":"shell_run","arguments":"{\"command\":\"api_key=[REDACTED]"}}]"#.to_string(),
        );
        let mut tool_metadata = Metadata::new();
        tool_metadata.insert("tool_call_id".to_string(), "call_broken".to_string());

        let body = build_chat_request_json_with_tools(
            "model-a",
            &[
                Message {
                    role: MessageRole::Assistant,
                    content: "Inspecting configuration.".to_string(),
                    metadata: assistant_metadata,
                },
                Message {
                    role: MessageRole::Tool,
                    content: "tool=shell.run\nstatus=succeeded".to_string(),
                    metadata: tool_metadata,
                },
                Message {
                    role: MessageRole::User,
                    content: "continue".to_string(),
                    metadata: Metadata::new(),
                },
            ],
            false,
            &[],
        )
        .expect("body should encode");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        let messages = value["messages"]
            .as_array()
            .expect("messages should be an array");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert!(messages[0].get("tool_calls").is_none());
        assert_eq!(messages[1]["role"], "user");
        assert!(!body.contains("call_broken"));
    }

    #[test]
    fn parses_streaming_delta_lines() {
        let delta =
            parse_stream_line(r#"data: {"choices":[{"delta":{"content":"hi"},"index":0}]}"#)
                .expect("line should parse");

        assert_eq!(delta.as_deref(), Some("hi"));
        assert_eq!(parse_stream_line("data: [DONE]").expect("done"), None);
    }

    #[test]
    fn reconstructs_streaming_tool_call_deltas() {
        let first = parse_stream_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path="}}]}}]}"#,
        )
        .expect("first event should parse")
        .expect("first event should exist");
        let second = parse_stream_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"README.md\"}"}}]}}]}"#,
        )
        .expect("second event should parse")
        .expect("second event should exist");
        let mut call = StreamingToolCall::default();
        for delta in first.tool_calls.into_iter().chain(second.tool_calls) {
            call.merge(delta);
        }
        let call = call.finish(0).expect("tool call should be complete");

        assert_eq!(call.id, "call_1");
        assert_eq!(call.name, "file_read");
        assert_eq!(call.arguments_json, r#"{"input":"path=README.md"}"#);
    }

    #[test]
    fn cancels_an_open_model_stream_without_waiting_for_timeout() {
        let chunks = vec![
            (
                Duration::ZERO,
                "data: {\"choices\":[{\"delta\":{\"content\":\"started\"}}]}\n\n",
            ),
            (Duration::from_secs(2), ""),
        ];
        let started = Instant::now();
        let mut output = String::new();
        let result = consume_test_stream(
            chunks,
            Duration::from_secs(10),
            &mut |delta| output.push_str(delta),
            &mut || started.elapsed() >= Duration::from_millis(100),
        );

        assert_eq!(
            result.expect_err("stream should cancel").message,
            MODEL_REQUEST_CANCELLED
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(output, "started");
    }

    #[test]
    fn stops_a_stream_after_the_idle_timeout() {
        let started = Instant::now();
        let result = consume_test_stream(
            vec![(Duration::from_secs(2), "")],
            Duration::from_millis(100),
            &mut |_| {},
            &mut || false,
        );

        assert!(result
            .expect_err("stream should time out")
            .message
            .contains("without receiving data"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn stream_activity_refreshes_the_idle_timeout() {
        let chunks = vec![
            (
                Duration::ZERO,
                "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n",
            ),
            (
                Duration::from_millis(100),
                "data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n",
            ),
            (
                Duration::from_millis(100),
                "data: {\"choices\":[{\"delta\":{\"content\":\"c\"}}]}\n",
            ),
        ];
        let mut output = String::new();
        let response = consume_test_stream(
            chunks,
            Duration::from_millis(250),
            &mut |delta| output.push_str(delta),
            &mut || false,
        )
        .expect("active stream should complete");

        assert_eq!(output, "abc");
        assert_eq!(response.message.content, "abc");
    }

    #[test]
    fn cancels_a_buffered_model_request_without_waiting_for_timeout() {
        let started = Instant::now();
        let timeout = Duration::from_secs(10);
        let result = run_http(async {
            await_http(
                std::future::pending::<Result<(), std::io::Error>>(),
                Instant::now() + timeout,
                timeout,
                "test request",
                &mut || started.elapsed() >= Duration::from_millis(100),
            )
            .await
        });

        assert_eq!(
            result.expect_err("request should cancel").message,
            MODEL_REQUEST_CANCELLED
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn streaming_reader_accepts_non_streaming_tool_call_fallback() {
        let response = finish_streaming_response(
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]}}]}"#.to_string(),
            false,
            String::new(),
            BTreeMap::new(),
            None,
            Metadata::new(),
            "test-model",
            "http://example.test/v1",
        )
        .expect("fallback response should parse");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "file_read");
        assert!(response.raw_tool_calls_json.is_some());
    }

    #[test]
    fn streaming_reader_normalizes_dsml_tool_calls() {
        let dsml = concat!(
            "<｜DSML｜tool_calls> ",
            "<｜DSML｜invoke name=\"shell_run\"> ",
            "<｜DSML｜parameter name=\"command\" string=\"true\">ls -la racing_game.html</｜DSML｜parameter> ",
            "</｜DSML｜invoke> ",
            "<｜DSML｜invoke name=\"shell_run\"> ",
            "<｜DSML｜parameter name=\"command\" string=\"true\">find ~ -name build_game.py</｜DSML｜parameter> ",
            "</｜DSML｜invoke> ",
            "</｜DSML｜tool_calls>"
        );
        let response = finish_streaming_response(
            String::new(),
            false,
            dsml.to_string(),
            BTreeMap::new(),
            None,
            Metadata::new(),
            "test-model",
            "http://example.test/v1",
        )
        .expect("DSML response should normalize");

        assert_eq!(response.message.content, "");
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].name, "shell_run");
        assert_eq!(response.tool_calls[1].name, "shell_run");
        let first_arguments: serde_json::Value =
            serde_json::from_str(&response.tool_calls[0].arguments_json)
                .expect("arguments should be JSON");
        let second_arguments: serde_json::Value =
            serde_json::from_str(&response.tool_calls[1].arguments_json)
                .expect("arguments should be JSON");
        assert_eq!(first_arguments["command"], "ls -la racing_game.html");
        assert_eq!(second_arguments["command"], "find ~ -name build_game.py");
        assert_eq!(
            response.metadata.get("tool_protocol").map(String::as_str),
            Some("dsml")
        );
        assert!(response.raw_tool_calls_json.is_some());
    }

    #[test]
    fn streaming_delta_filter_hides_split_dsml_protocol() {
        let mut filter = DsmlStreamDeltaFilter::default();
        let mut visible = String::new();
        for delta in [
            "Before <｜DS",
            "ML｜tool_calls><｜DSML｜invoke name=\"shell_run\">",
            "<｜DSML｜parameter name=\"command\">pwd</｜DSML｜parameter>",
            "</｜DSML｜invoke></｜DSML｜tool_calls> after",
        ] {
            filter.push(delta, &mut |value| visible.push_str(value));
        }
        filter.finish(&mut |value| visible.push_str(value));

        assert_eq!(visible, "Before  after");
    }

    #[test]
    fn streaming_delta_filter_drops_unclosed_dsml_protocol() {
        let mut filter = DsmlStreamDeltaFilter::default();
        let mut visible = String::new();
        filter.push(
            "Visible<｜DSML｜tool_calls><｜DSML｜invoke name=\"shell_run\">secret",
            &mut |value| visible.push_str(value),
        );
        filter.finish(&mut |value| visible.push_str(value));

        assert_eq!(visible, "Visible");
    }

    #[test]
    fn non_streaming_reader_preserves_visible_text_around_dsml_calls() {
        let content = concat!(
            "I will inspect the workspace.\n",
            "<｜DSML｜tool_calls>",
            "<｜DSML｜invoke name=\"file_read\">",
            "<｜DSML｜parameter name=\"path\" string=\"true\">README.md</｜DSML｜parameter>",
            "</｜DSML｜invoke>",
            "</｜DSML｜tool_calls>"
        );
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": content
                }
            }]
        })
        .to_string();
        let response = parse_model_response(&body).expect("DSML response should parse");

        assert_eq!(response.message.content, "I will inspect the workspace.");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "file_read");
    }

    #[test]
    fn incomplete_dsml_tool_protocol_is_rejected() {
        let result = finish_streaming_response(
            String::new(),
            false,
            "<｜DSML｜tool_calls><｜DSML｜invoke name=\"shell_run\">".to_string(),
            BTreeMap::new(),
            None,
            Metadata::new(),
            "test-model",
            "http://example.test/v1",
        );

        assert_eq!(
            result.expect_err("incomplete DSML must fail").message,
            "model returned incomplete DSML tool protocol"
        );
    }

    #[test]
    fn parses_non_streaming_chat_response() {
        let text = r#"{"choices":[{"message":{"role":"assistant","content":"done"}}],"usage":{"prompt_tokens":21,"completion_tokens":4,"total_tokens":25}}"#;
        let answer = parse_chat_response(text).expect("response should parse");

        assert_eq!(answer, "done");
        let response = parse_model_response(text).expect("model response should parse");
        assert_eq!(
            response.metadata.get("prompt_tokens").map(String::as_str),
            Some("21")
        );
        assert_eq!(
            response.metadata.get("total_tokens").map(String::as_str),
            Some("25")
        );
    }

    #[test]
    fn parses_non_streaming_tool_calls() {
        let response = parse_model_response(
            r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]},"finish_reason":"tool_calls"}]}"#,
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
        assert_eq!(
            response.assessment(),
            ModelResponseAssessment {
                termination: ModelResponseTermination::ToolCalls,
                disposition: ModelResponseDisposition::ToolCalls,
            }
        );
    }

    #[test]
    fn model_response_assessment_rejects_incomplete_filtered_and_empty_outputs() {
        let incomplete = parse_model_response(
            r#"{"choices":[{"message":{"role":"assistant","content":"partial"},"finish_reason":"length"}]}"#,
        )
        .expect("length response should parse");
        assert_eq!(
            incomplete.assessment().disposition,
            ModelResponseDisposition::IncompleteOutput
        );

        let filtered = parse_model_response(
            r#"{"choices":[{"message":{"role":"assistant","content":""},"finish_reason":"content_filter"}]}"#,
        )
        .expect("filtered response should parse");
        assert_eq!(
            filtered.assessment().disposition,
            ModelResponseDisposition::Filtered
        );

        let empty = parse_model_response(
            r#"{"choices":[{"message":{"role":"assistant","content":"  "},"finish_reason":"stop"}]}"#,
        )
        .expect("empty response should parse");
        assert_eq!(
            empty.assessment().disposition,
            ModelResponseDisposition::Empty
        );

        let complete = parse_model_response(
            r#"{"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#,
        )
        .expect("complete response should parse");
        assert_eq!(
            complete.assessment(),
            ModelResponseAssessment {
                termination: ModelResponseTermination::Complete,
                disposition: ModelResponseDisposition::Usable,
            }
        );
    }

    #[test]
    fn streaming_events_preserve_finish_reason() {
        let event =
            parse_stream_event(r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#)
                .expect("stream event should parse")
                .expect("stream event should exist");

        assert_eq!(event.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn converts_tool_arguments_to_key_value_input() {
        assert_eq!(
            tool_arguments_to_key_value_input(r#"{"input":"path=README.md"}"#),
            "path=README.md"
        );
        assert_eq!(
            tool_arguments_to_key_value_input(
                r#"{"path":"README.md","limit":3,"destructive":false}"#
            ),
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
    fn image_generation_request_uses_the_selected_model_and_size() {
        let body = build_image_generation_request_json(
            "image-model-a",
            "A blue circle on white",
            Some("1024x1024"),
        )
        .expect("image body should encode");
        let value: serde_json::Value =
            serde_json::from_str(&body).expect("image body should be json");

        assert_eq!(value["model"], "image-model-a");
        assert_eq!(value["prompt"], "A blue circle on white");
        assert_eq!(value["size"], "1024x1024");
        assert_eq!(value["n"], 1);
    }

    #[test]
    fn dashscope_image_request_uses_multimodal_messages() {
        let body = build_dashscope_image_generation_request_json(
            "wan2.7-image-pro",
            "A blue circle on white",
            Some("1024x1024"),
        )
        .expect("DashScope image body should encode");
        let value: serde_json::Value =
            serde_json::from_str(&body).expect("DashScope image body should be json");

        assert_eq!(value["model"], "wan2.7-image-pro");
        assert_eq!(
            value["input"]["messages"][0]["content"][0]["text"],
            "A blue circle on white"
        );
        assert_eq!(value["parameters"]["size"], "1024*1024");
        assert_eq!(value["parameters"]["n"], 1);
        assert_eq!(value["parameters"]["watermark"], false);
    }

    #[test]
    fn parses_base64_and_url_image_generation_payloads() {
        let png = b"\x89PNG\r\n\x1a\nfixture";
        let encoded = base64::engine::general_purpose::STANDARD.encode(png);
        let response = format!(
            r#"{{"model":"image-model-a","data":[{{"b64_json":"{encoded}","revised_prompt":"refined"}},{{"url":"https://example.test/image.png"}}]}}"#
        );
        let (model, payloads) = parse_image_generation_payloads(&response, "fallback")
            .expect("image payloads should parse");

        assert_eq!(model, "image-model-a");
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[0].revised_prompt.as_deref(), Some("refined"));
        let ImagePayload::Base64(value) = &payloads[0].payload else {
            panic!("first payload should be base64");
        };
        let bytes = decode_generated_image(value).expect("base64 should decode");
        assert_eq!(bytes, png);
        assert_eq!(generated_image_mime_type(&bytes), Some("image/png"));
        assert!(matches!(payloads[1].payload, ImagePayload::Url(_)));
    }

    #[test]
    fn parses_dashscope_multimodal_image_payloads() {
        let response = r#"{
            "output": {
                "choices": [{
                    "message": {
                        "content": [{
                            "image": "https://example.test/generated.png",
                            "type": "image"
                        }]
                    }
                }]
            }
        }"#;
        let (model, payloads) = parse_image_generation_payloads(response, "wan2.7-image-pro")
            .expect("DashScope image payload should parse");

        assert_eq!(model, "wan2.7-image-pro");
        assert_eq!(payloads.len(), 1);
        assert!(matches!(payloads[0].payload, ImagePayload::Url(_)));
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
        assert_eq!(
            response.metadata.get("vectors").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn provider_capabilities_follow_the_selected_model() {
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            api_key: "key".to_string(),
            model: "model-a".to_string(),
            embedding_model: "embed-a".to_string(),
            timeout_seconds: 10,
        });

        assert!(provider.capabilities().supports_embeddings);
        assert!(provider.capabilities().supports_tools);
        assert!(!provider.capabilities().supports_vision);

        let vision_provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            api_key: "key".to_string(),
            model: "qwen-vl-max".to_string(),
            embedding_model: "embed-a".to_string(),
            timeout_seconds: 10,
        });
        assert!(vision_provider.capabilities().supports_vision);
    }
}
