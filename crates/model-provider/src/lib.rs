use agent_core::Metadata;
#[cfg(test)]
use agent_core::{Message, ModelRole, ToolSpec};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::{header, redirect, Client, RequestBuilder, Response, StatusCode};
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};

mod dashscope_asr_task_provider;
mod dashscope_realtime_config;
mod dashscope_realtime_guard;
mod dashscope_realtime_provider;
mod image_provider;
mod json_wire;
mod prepared_non_streaming;
mod prepared_payload;
mod prepared_request;
mod provider_receipt;
mod provider_validation;
mod realtime_provider;
mod redirect_policy;
mod request_builder;
mod request_tool_calls;
mod request_vision;
mod response_parser;
mod stream_delta_aggregator;
mod streaming_finish;
mod streaming_response;
mod streaming_wire;
mod usage;

use redirect_policy::{api_key_safe_redirect_policy, provider_redirect_policy};
#[cfg(test)]
use request_builder::{
    build_chat_request_json_with_tools_and_output_limit,
    build_chat_request_json_with_tools_output_limit_and_vision,
    build_chat_request_json_with_tools_output_limit_vision_and_images,
};
use request_vision::ImageDataUrlCache;
#[cfg(test)]
use streaming_finish::{finish_streaming_response, StreamingResponseParts};
#[cfg(test)]
use streaming_response::consume_streaming_body;
use streaming_response::consume_streaming_response;

pub use agent_core::{
    classify_provider_failure, tool_function_name, ModelCallMode, ModelError, ModelRequest,
    ModelResponse, ModelResponseAssessment, ModelResponseDisposition, ModelResponseTermination,
    ModelToolCall, ProviderFailureClass,
};
pub use dashscope_realtime_provider::{
    DashScopeRealtimeTranscriptionConfig, DashScopeRealtimeTranscriptionProvider,
};
pub use image_provider::{
    build_image_generation_request_json, OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider,
};
pub use prepared_non_streaming::PreparedNonStreamingModelRequest;
pub use prepared_payload::PreparedStreamingModelRequest;
pub use provider_validation::parse_model_list_response;
pub use realtime_provider::{
    build_realtime_session_json, OpenAiCompatibleRealtimeConfig, OpenAiCompatibleRealtimeProvider,
};
pub use request_builder::{
    build_chat_request_json, build_chat_request_json_with_tools, build_embedding_request_json,
    generation_temperature_from_metadata, model_disables_thinking_by_default,
    parse_embedding_response, GENERATION_TEMPERATURE_KEY,
};
pub use request_vision::model_supports_vision_content;
pub use response_parser::{
    parse_chat_response, parse_model_response, parse_provider_error, parse_tool_calls,
    tool_arguments_to_key_value_input,
};
pub use streaming_wire::parse_stream_line;
pub use usage::{
    estimate_completion_tokens, estimate_request_tokens, estimate_text_tokens,
    normalize_model_usage, UsageSource,
};

pub const MODEL_REQUEST_CANCELLED: &str = "model request cancelled";
const STREAMING_HARD_TIMEOUT_MULTIPLIER: u64 = 4;
const HTTP_POLL_INTERVAL: Duration = Duration::from_millis(40);
const MAX_MODEL_RESPONSE_BYTES: usize = agent_core::HTTP_MODEL_RESPONSE_MAX_BYTES;
const DSML_TOOL_CALLS_OPEN: &str = "<｜DSML｜tool_calls>";
const DSML_TOOL_CALLS_CLOSE: &str = "</｜DSML｜tool_calls>";
const DSML_INVOKE_OPEN: &str = "<｜DSML｜invoke";
const DSML_INVOKE_CLOSE: &str = "</｜DSML｜invoke>";
const DSML_PARAMETER_OPEN: &str = "<｜DSML｜parameter";
const DSML_PARAMETER_CLOSE: &str = "</｜DSML｜parameter>";
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static AZURE_API_KEY_HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static HTTP_RUNTIME: OnceLock<Runtime> = OnceLock::new();

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

pub trait ModelProvider {
    fn name(&self) -> &str;

    fn capabilities(&self) -> ProviderCapabilities;

    fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;
}

pub trait StreamingModelProvider: Send + Sync {
    fn prepare_streaming_request(
        &self,
        request: &ModelRequest,
    ) -> Result<PreparedStreamingModelRequest, ModelError> {
        Ok(PreparedStreamingModelRequest::deferred(request.clone()))
    }

    fn complete_prepared_streaming_cancellable(
        &self,
        request: &PreparedStreamingModelRequest,
        on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        let request = request
            .deferred_request()
            .ok_or_else(|| ModelError::new("provider received an incompatible prepared request"))?;
        self.complete_streaming_cancellable(request.clone(), on_delta, should_cancel)
    }

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

    fn supports_vision_content(&self) -> bool {
        model_supports_vision_content(&self.model)
    }
}

pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    image_cache: ImageDataUrlCache,
}

fn streaming_hard_timeout_seconds(idle_timeout_seconds: u64) -> u64 {
    idle_timeout_seconds
        .max(1)
        .saturating_mul(STREAMING_HARD_TIMEOUT_MULTIPLIER)
}

fn initialize_http_client(
    cell: &'static OnceLock<Client>,
    redirect_policy: Option<redirect::Policy>,
) -> Result<&'static Client, ModelError> {
    if let Some(client) = cell.get() {
        return Ok(client);
    }
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(Duration::from_secs(30));
    if let Some(redirect_policy) = redirect_policy {
        builder = builder.redirect(redirect_policy);
    }
    let client = builder
        .build()
        .map_err(|error| ModelError::new(format!("failed to initialize HTTP client: {error}")))?;
    let _ = cell.set(client);
    cell.get()
        .ok_or_else(|| ModelError::new("HTTP client did not initialize"))
}

fn http_client() -> Result<&'static Client, ModelError> {
    // The bound is stated explicitly from the shared HTTP policy owner instead of
    // relying on the client library's implicit default.
    initialize_http_client(&HTTP_CLIENT, Some(provider_redirect_policy()))
}

fn azure_api_key_http_client() -> Result<&'static Client, ModelError> {
    initialize_http_client(
        &AZURE_API_KEY_HTTP_CLIENT,
        Some(api_key_safe_redirect_policy()),
    )
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
    request_body: Option<Bytes>,
    hard_timeout: Duration,
    streaming: bool,
) -> Result<RequestBuilder, ModelError> {
    let azure_api_key_request = !api_key.trim().is_empty() && is_azure_openai_url(url);
    let client = if azure_api_key_request {
        azure_api_key_http_client()?
    } else {
        http_client()?
    };
    let mut request = if let Some(request_body) = request_body {
        client
            .post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(request_body)
    } else {
        client.get(url)
    };
    if !api_key.trim().is_empty() {
        request = apply_api_key_auth(request, url, api_key);
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

fn apply_api_key_auth(request: RequestBuilder, url: &str, api_key: &str) -> RequestBuilder {
    if is_azure_openai_url(url) {
        request.header("api-key", api_key)
    } else {
        request.bearer_auth(api_key)
    }
}

pub(crate) fn is_azure_openai_url(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.');
    [".openai.azure.com", ".services.ai.azure.com"]
        .into_iter()
        .any(|suffix| {
            host.strip_suffix(suffix)
                .is_some_and(|subdomain| !subdomain.is_empty())
        })
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
    execute_http_bytes_cancellable(
        url,
        api_key,
        request_body.map(|body| Bytes::copy_from_slice(body.as_bytes())),
        timeout_seconds,
        max_response_bytes,
        should_cancel,
    )
}

fn execute_http_bytes_cancellable(
    url: &str,
    api_key: &str,
    request_body: Option<Bytes>,
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
        Self {
            config,
            image_cache: ImageDataUrlCache::new(),
        }
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
        on_delta: impl FnMut(&str),
        should_cancel: impl FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_streaming_cancellable_with_activity(request, on_delta, || {}, should_cancel)
    }

    pub fn complete_streaming_cancellable_with_activity(
        &self,
        request: ModelRequest,
        mut on_delta: impl FnMut(&str),
        mut on_activity: impl FnMut(),
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        let prepared = self.prepare_streaming_model_request(&request)?;
        drop(request);
        self.complete_prepared_streaming_model_request_with_activity(
            &prepared,
            &mut on_delta,
            &mut on_activity,
            &mut should_cancel,
        )
    }

    pub fn complete_once(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let prepared = self.prepare_non_streaming_request(&request)?;
        self.complete_prepared_non_streaming_request(prepared)
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

impl StreamingModelProvider for OpenAiCompatibleProvider {
    fn prepare_streaming_request(
        &self,
        request: &ModelRequest,
    ) -> Result<PreparedStreamingModelRequest, ModelError> {
        self.prepare_streaming_model_request(request)
    }

    fn complete_prepared_streaming_cancellable(
        &self,
        request: &PreparedStreamingModelRequest,
        on_delta: &mut dyn FnMut(&str),
        should_cancel: &mut dyn FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_prepared_streaming_model_request(request, on_delta, should_cancel)
    }

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
            supports_vision: self.config.supports_vision_content(),
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
    use crate::image_provider::{
        build_dashscope_image_generation_request_json, decode_generated_image,
        generated_image_mime_type, image_endpoint_probe_succeeded, parse_image_generation_payloads,
        ImagePayload,
    };
    use crate::streaming_wire::{parse_stream_event, StreamingToolCall};
    use agent_core::{MessageRole, ToolRisk};
    use base64::Engine;
    use futures_util::{stream, Stream};
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    fn serve_credential_probe(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("probe listener should bind");
        let address = listener.local_addr().expect("probe address");
        let status = status.to_string();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("probe request should connect");
            let mut request = vec![0_u8; 8192];
            let size = stream
                .read(&mut request)
                .expect("probe request should read");
            let request = String::from_utf8_lossy(&request[..size]).to_string();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("probe response should write");
            request
        });
        (format!("http://{address}/v1"), handle)
    }

    fn serve_credential_probe_sequence(
        responses: Vec<(&str, &str)>,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("probe listener should bind");
        let address = listener.local_addr().expect("probe address");
        let responses = responses
            .into_iter()
            .map(|(status, body)| (status.to_string(), body.to_string()))
            .collect::<Vec<_>>();
        let handle = thread::spawn(move || {
            responses
                .into_iter()
                .map(|(status, body)| {
                    let (mut stream, _) = listener.accept().expect("probe request should connect");
                    let mut request = vec![0_u8; 8192];
                    let size = stream
                        .read(&mut request)
                        .expect("probe request should read");
                    let request = String::from_utf8_lossy(&request[..size]).to_string();
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .expect("probe response should write");
                    request
                })
                .collect()
        });
        (format!("http://{address}/v1"), handle)
    }

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
            &mut || {},
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
                    "data: {\"id\":\"resp-stream-1\",\"model\":\"served-model\",\"system_fingerprint\":\"fp-stream\",\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n\n",
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
        assert_eq!(response.metadata["provider"], "openai-compatible");
        assert_eq!(response.metadata["provider_protocol"], "openai-compatible");
        assert_eq!(response.metadata["provider_response_id"], "resp-stream-1");
        assert_eq!(response.metadata["provider_response_model"], "served-model");
        assert_eq!(
            response.metadata["provider_system_fingerprint"],
            "fp-stream"
        );
        assert_eq!(response.metadata["provider_receipt_status"], "observed");
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("done", &[], None)
        );
        assert_eq!(
            response.metadata.get("prompt_tokens").map(String::as_str),
            Some("21")
        );
        assert_eq!(
            response
                .metadata
                .get("completion_tokens")
                .map(String::as_str),
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
        let request_len = request_text.len();
        let request = http_request(
            "https://example.test/v1/chat/completions",
            "test-secret",
            Some(Bytes::from(request_text)),
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
            request_len
        );
        assert!(std::ptr::eq(
            http_client().expect("shared client should exist"),
            http_client().expect("shared client should be reused")
        ));
    }

    #[test]
    fn azure_openai_requests_use_api_key_header() {
        for url in [
            "https://cindx.openai.azure.com/openai/v1/chat/completions",
            "https://cindx.services.ai.azure.com/openai/v1/models",
        ] {
            let request = http_request(url, "test-secret", None, Duration::from_secs(10), false)
                .expect("request should build")
                .build()
                .expect("request should be valid");

            assert_eq!(
                request
                    .headers()
                    .get("api-key")
                    .expect("Azure API key header should exist"),
                "test-secret"
            );
            assert!(request.headers().get(header::AUTHORIZATION).is_none());
        }
    }

    #[test]
    fn azure_openai_auth_requires_an_exact_host_with_a_resource_subdomain() {
        for url in [
            "https://openai.azure.com/openai/v1",
            "https://services.ai.azure.com/openai/v1",
            "https://cindx.openai.azure.com.example.test/openai/v1",
            "https://example.test/openai/v1",
        ] {
            let request = http_request(url, "test-secret", None, Duration::from_secs(10), false)
                .expect("request should build")
                .build()
                .expect("request should be valid");

            assert!(request.headers().get("api-key").is_none());
            assert_eq!(
                request
                    .headers()
                    .get(header::AUTHORIZATION)
                    .expect("bearer authorization should exist"),
                "Bearer test-secret"
            );
        }
    }

    #[test]
    fn azure_deployment_names_do_not_invent_multimodal_capabilities() {
        let azure = OpenAiCompatibleConfig {
            base_url: "https://cindx.openai.azure.com/openai/v1".to_string(),
            api_key: "key".to_string(),
            model: "production-deployment".to_string(),
            embedding_model: "embedding-deployment".to_string(),
            timeout_seconds: 30,
        };
        let custom = OpenAiCompatibleConfig {
            base_url: "https://example.test/v1".to_string(),
            ..azure.clone()
        };
        let known_multimodal = OpenAiCompatibleConfig {
            model: "gpt-4.1".to_string(),
            ..azure.clone()
        };

        assert!(!azure.supports_vision_content());
        assert!(!custom.supports_vision_content());
        assert!(known_multimodal.supports_vision_content());
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
    fn request_json_disables_thinking_for_thinking_default_models() {
        let body = build_chat_request_json(
            "qwen3.8-max",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            false,
        )
        .expect("body should encode");

        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert_eq!(value["enable_thinking"], false);
    }

    #[test]
    fn request_json_leaves_thinking_absent_for_other_models() {
        let body = build_chat_request_json(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            false,
        )
        .expect("body should encode");

        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert!(value.get("enable_thinking").is_none());
    }

    #[test]
    fn reasoning_effort_maps_to_thinking_budget_for_thinking_default_models() {
        let build = |reasoning_effort: Option<&str>| {
            let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
                "qwen3.8-max",
                &[Message {
                    role: MessageRole::User,
                    content: "hello".to_string(),
                    metadata: Metadata::new(),
                }],
                false,
                &[],
                None,
                false,
                &mut |_| None,
                None,
                reasoning_effort,
                None,
            )
            .expect("body should encode");
            serde_json::from_str::<serde_json::Value>(&body).expect("valid request JSON")
        };

        // Fast keeps thinking off.
        assert_eq!(build(Some("fast"))["enable_thinking"], false);
        // Default/High/Xhigh enable thinking with a growing budget.
        assert_eq!(build(Some("default"))["enable_thinking"], true);
        assert_eq!(build(Some("default"))["thinking_budget"], 4096);
        assert_eq!(build(Some("high"))["thinking_budget"], 8192);
        assert_eq!(build(Some("xhigh"))["thinking_budget"], 16384);
        // Absent reasoning effort keeps thinking off (unchanged default).
        assert_eq!(build(None)["enable_thinking"], false);

        // Non-thinking-default models are untouched by reasoning effort.
        let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            false,
            &[],
            None,
            false,
            &mut |_| None,
            None,
            Some("high"),
            None,
        )
        .expect("body should encode");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert!(value.get("enable_thinking").is_none());
        assert!(value.get("thinking_budget").is_none());
    }

    #[test]
    fn thinking_default_predicate_covers_configured_dashscope_families() {
        for model in [
            "qwen3.8-max",
            "qwq-plus",
            "glm-5.2-fast-preview",
            "kimi-k2",
            "deepseek-v4-flash-0731",
        ] {
            assert!(
                model_disables_thinking_by_default(model),
                "{model} should disable thinking by default"
            );
        }
        for model in ["model-a", "gpt-5", "claude-4-sonnet"] {
            assert!(
                !model_disables_thinking_by_default(model),
                "{model} should keep thinking untouched"
            );
        }
    }

    #[test]
    fn request_json_includes_metadata_generation_temperature() {
        let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            false,
            &[],
            None,
            false,
            &mut |_| None,
            Some(0.0),
            None,
            None,
        )
        .expect("body should encode");

        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert_eq!(value["temperature"], 0.0);
    }

    #[test]
    fn request_json_omits_temperature_without_an_explicit_override() {
        let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            false,
            &[],
            None,
            false,
            &mut |_| None,
            None,
            None,
            None,
        )
        .expect("body should encode");

        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert!(value.get("temperature").is_none());
    }

    #[test]
    fn request_json_clamps_generation_temperature_into_provider_bounds() {
        let body = build_chat_request_json_with_tools_output_limit_vision_and_images(
            "model-a",
            &[Message {
                role: MessageRole::User,
                content: "hello".to_string(),
                metadata: Metadata::new(),
            }],
            false,
            &[],
            None,
            false,
            &mut |_| None,
            Some(9.0),
            None,
            None,
        )
        .expect("body should encode");

        let value: serde_json::Value = serde_json::from_str(&body).expect("valid request JSON");
        assert_eq!(value["temperature"], 2.0);
    }

    #[test]
    fn generation_temperature_metadata_parses_valid_values_only() {
        let present: Metadata = [(GENERATION_TEMPERATURE_KEY.to_string(), "0.3".to_string())]
            .into_iter()
            .collect();
        assert_eq!(generation_temperature_from_metadata(&present), Some(0.3));

        let invalid: Metadata = [(GENERATION_TEMPERATURE_KEY.to_string(), "warm".to_string())]
            .into_iter()
            .collect();
        assert_eq!(generation_temperature_from_metadata(&invalid), None);
        assert_eq!(generation_temperature_from_metadata(&Metadata::new()), None);
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
        assert_eq!(
            response.metadata["provider_receipt_status"],
            "provider_id_missing"
        );
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("abc", &[], None)
        );
    }

    #[test]
    fn streaming_identity_conflict_preserves_output_and_first_observation() {
        let mut visible = String::new();
        let response = consume_test_stream(
            vec![
                (
                    Duration::ZERO,
                    "data: {\"id\":\"response-first\",\"model\":\"served-a\",\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n",
                ),
                (
                    Duration::ZERO,
                    "data: {\"id\":\"response-second\",\"model\":\"served-b\",\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n",
                ),
                (Duration::ZERO, "data: [DONE]\n"),
            ],
            Duration::from_secs(1),
            &mut |delta| visible.push_str(delta),
            &mut || false,
        )
        .expect("identity conflict must not discard a valid stream");

        assert_eq!(visible, "ab");
        assert_eq!(response.message.content, "ab");
        assert_eq!(response.metadata["provider_response_id"], "response-first");
        assert_eq!(response.metadata["provider_response_model"], "served-a");
        assert_eq!(
            response.metadata["provider_receipt_status"],
            "identity_conflict"
        );
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("ab", &[], None)
        );
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
            StreamingResponseParts {
                fallback_response: r#"{"id":"fallback-response-1","model":"fallback-model","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]}}]}"#.to_string(),
                fallback_truncated: false,
                answer: String::new(),
                streamed_tool_calls: BTreeMap::new(),
                finish_reason: None,
                usage: Metadata::new(),
            },
            "test-model",
            "http://example.test/v1",
        )
        .expect("fallback response should parse");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "file_read");
        assert!(response.raw_tool_calls_json.is_some());
        assert_eq!(
            response.metadata["provider_response_id"],
            "fallback-response-1"
        );
        assert_eq!(response.metadata["provider_receipt_status"], "observed");
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("", &response.tool_calls, None,)
        );
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
            StreamingResponseParts {
                fallback_response: String::new(),
                fallback_truncated: false,
                answer: dsml.to_string(),
                streamed_tool_calls: BTreeMap::new(),
                finish_reason: None,
                usage: Metadata::new(),
            },
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
            StreamingResponseParts {
                fallback_response: String::new(),
                fallback_truncated: false,
                answer: "<｜DSML｜tool_calls><｜DSML｜invoke name=\"shell_run\">".to_string(),
                streamed_tool_calls: BTreeMap::new(),
                finish_reason: None,
                usage: Metadata::new(),
            },
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
        let text = r#"{"id":"resp-once-1","model":"served-model","system_fingerprint":"fp-once","choices":[{"message":{"role":"assistant","content":"done"}}],"usage":{"prompt_tokens":21,"completion_tokens":4,"total_tokens":25}}"#;
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
        assert_eq!(response.metadata["provider_response_id"], "resp-once-1");
        assert_eq!(response.metadata["provider_response_model"], "served-model");
        assert_eq!(response.metadata["provider_system_fingerprint"], "fp-once");
        assert_eq!(response.metadata["provider_receipt_status"], "observed");
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("done", &[], None)
        );
    }

    #[test]
    fn non_streaming_response_marks_missing_provider_id_without_losing_content() {
        let response = parse_model_response(
            r#"{"model":"served-model","choices":[{"message":{"role":"assistant","content":"done"}}]}"#,
        )
        .expect("response without an id should remain usable");

        assert_eq!(response.message.content, "done");
        assert_eq!(response.metadata["provider_response_model"], "served-model");
        assert_eq!(
            response.metadata["provider_receipt_status"],
            "provider_id_missing"
        );
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("done", &[], None)
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

        assert!(model_supports_vision_content("qwen3.7-plus"));
        assert!(!model_supports_vision_content("qwen3.7-max"));
    }

    #[test]
    fn completed_requests_expose_only_the_canonical_request_digest() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: "receipt prompt".to_string(),
            metadata: Metadata::new(),
        }];
        let expected_body = build_chat_request_json_with_tools_and_output_limit(
            "receipt-model",
            &messages,
            false,
            &[],
            None,
        )
        .expect("canonical request should encode");
        let expected_digest =
            crate::provider_receipt::request_payload_sha256(expected_body.as_bytes());
        let (base_url, request) = serve_credential_probe(
            "200 OK",
            r#"{"id":"receipt-response","choices":[{"message":{"role":"assistant","content":"done"}}]}"#,
        );
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "private-api-key".to_string(),
            model: "receipt-model".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 5,
        });

        let response = provider
            .complete_once(ModelRequest {
                role: ModelRole::Executor,
                messages,
                tools: Vec::new(),
                mode: ModelCallMode::NonStreaming,
                metadata: Metadata::new(),
            })
            .expect("request should complete");
        request.join().expect("probe server should finish");

        assert_eq!(response.metadata["request_payload_sha256"], expected_digest);
        assert!(!response.metadata["request_payload_sha256"].contains("receipt prompt"));
        assert!(!response.metadata["request_payload_sha256"].contains("private-api-key"));
    }

    #[test]
    fn prepared_streaming_request_attaches_its_cached_request_digest() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: "stream receipt prompt".to_string(),
            metadata: Metadata::new(),
        }];
        let expected_body = build_chat_request_json_with_tools_and_output_limit(
            "receipt-model",
            &messages,
            true,
            &[],
            None,
        )
        .expect("canonical streaming request should encode");
        let expected_digest =
            crate::provider_receipt::request_payload_sha256(expected_body.as_bytes());
        let (base_url, request) = serve_credential_probe(
            "200 OK",
            r#"{"id":"stream-fallback-response","choices":[{"message":{"role":"assistant","content":"done"}}]}"#,
        );
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "private-api-key".to_string(),
            model: "receipt-model".to_string(),
            embedding_model: String::new(),
            timeout_seconds: 5,
        });
        let prepared = provider
            .prepare_streaming_request(&ModelRequest {
                role: ModelRole::Executor,
                messages,
                tools: Vec::new(),
                mode: ModelCallMode::Streaming,
                metadata: Metadata::new(),
            })
            .expect("streaming request should prepare");

        let response = provider
            .complete_prepared_streaming_cancellable(&prepared, &mut |_| {}, &mut || false)
            .expect("prepared streaming request should complete");
        request.join().expect("probe server should finish");

        assert_eq!(response.metadata["request_payload_sha256"], expected_digest);
        assert_eq!(response.metadata["provider_receipt_status"], "observed");
        assert_eq!(
            response.metadata["response_semantic_sha256"],
            crate::provider_receipt::response_semantic_sha256("done", &[], None)
        );
    }

    #[test]
    fn credential_validation_returns_authenticated_model_catalog() {
        let (base_url, request) = serve_credential_probe(
            "200 OK",
            r#"{"data":[{"id":"gpt-4.1"},{"id":"gpt-image-2"}]}"#,
        );
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "verified-key".to_string(),
            model: "gpt-4.1".to_string(),
            embedding_model: "text-embedding-3-large".to_string(),
            timeout_seconds: 5,
        });

        assert_eq!(
            provider.validate_credentials().expect("key should verify"),
            vec!["gpt-4.1".to_string(), "gpt-image-2".to_string()]
        );
        let request = request.join().expect("probe server should finish");
        assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        assert!(request.contains("authorization: Bearer verified-key"));
    }

    #[test]
    fn credential_validation_rejects_unauthorized_keys() {
        let (base_url, request) = serve_credential_probe(
            "401 Unauthorized",
            r#"{"error":{"message":"invalid API key"}}"#,
        );
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "bad-key".to_string(),
            model: "gpt-4.1".to_string(),
            embedding_model: "text-embedding-3-large".to_string(),
            timeout_seconds: 5,
        });

        let error = provider
            .validate_credentials()
            .expect_err("unauthorized key must fail");
        assert_eq!(error.status_code, Some(401));
        assert!(error.message.contains("invalid API key"));
        request.join().expect("probe server should finish");
    }

    #[test]
    fn credential_validation_rejects_unparseable_success_and_rate_limits() {
        for (status, body) in [
            ("200 OK", r#"{"ok":true}"#),
            (
                "429 Too Many Requests",
                r#"{"error":{"message":"retry later"}}"#,
            ),
        ] {
            let (base_url, request) = serve_credential_probe(status, body);
            let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url,
                api_key: "uncertain-key".to_string(),
                model: "gpt-4.1".to_string(),
                embedding_model: "text-embedding-3-large".to_string(),
                timeout_seconds: 5,
            });

            provider
                .validate_credentials()
                .expect_err("an ambiguous response must not mark credentials verified");
            request.join().expect("probe server should finish");
        }
    }

    #[test]
    fn credential_validation_fallback_requires_a_real_chat_completion() {
        let (base_url, requests) = serve_credential_probe_sequence(vec![
            (
                "404 Not Found",
                r#"{"error":{"message":"no model catalog"}}"#,
            ),
            (
                "200 OK",
                r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}]}"#,
            ),
        ]);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "verified-key".to_string(),
            model: "private-chat".to_string(),
            embedding_model: "private-embedding".to_string(),
            timeout_seconds: 5,
        });

        assert!(provider
            .validate_credentials()
            .expect("chat completion should verify")
            .is_empty());
        let requests = requests.join().expect("probe server should finish");
        assert!(requests[0].starts_with("GET /v1/models HTTP/1.1"));
        assert!(requests[1].starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(requests[1].contains("\"model\":\"private-chat\""));
    }

    #[test]
    fn chat_verification_retries_modern_completion_limit_parameter() {
        let (base_url, requests) = serve_credential_probe_sequence(vec![
            (
                "400 Bad Request",
                r#"{"error":{"message":"max_tokens is unsupported; use max_completion_tokens"}}"#,
            ),
            (
                "200 OK",
                r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}]}"#,
            ),
        ]);
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url,
            api_key: "verified-key".to_string(),
            model: "reasoning-deployment".to_string(),
            embedding_model: "embedding-deployment".to_string(),
            timeout_seconds: 5,
        });

        provider
            .validate_chat_access()
            .expect("the modern completion limit retry should verify");
        let requests = requests.join().expect("probe server should finish");
        assert!(requests[0].contains("\"max_tokens\":1"));
        assert!(!requests[0].contains("\"max_completion_tokens\""));
        assert!(requests[1].contains("\"max_completion_tokens\":1"));
        assert!(!requests[1].contains("\"max_tokens\""));
    }
}
