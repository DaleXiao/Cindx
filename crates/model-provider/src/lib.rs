use agent_core::{Message, MessageRole, Metadata, ModelRole, ToolSpec};
use base64::Engine;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::{self, RecvTimeoutError},
};
use std::thread;
use std::time::{Duration, Instant};

pub const MODEL_REQUEST_CANCELLED: &str = "model request cancelled";
const STREAMING_HARD_TIMEOUT_MULTIPLIER: u64 = 4;
const MAX_IMAGE_RESPONSE_BYTES: usize = 48 * 1024 * 1024;
const MAX_GENERATED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
static CURL_REQUEST_BODY_ID: AtomicU64 = AtomicU64::new(1);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleImageConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageGenerationProtocol {
    OpenAiImages,
    DashScopeMultimodal,
}

impl OpenAiCompatibleImageConfig {
    pub fn images_url(&self) -> String {
        let endpoint = self.base_url.trim_end_matches('/');
        if endpoint.ends_with("/images/generations")
            || endpoint.ends_with("/api/v1/services/aigc/multimodal-generation/generation")
        {
            endpoint.to_string()
        } else {
            format!("{endpoint}/images/generations")
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.base_url.trim().is_empty()
    }

    fn protocol(&self) -> ImageGenerationProtocol {
        if self
            .images_url()
            .contains("/api/v1/services/aigc/multimodal-generation/generation")
        {
            ImageGenerationProtocol::DashScopeMultimodal
        } else {
            ImageGenerationProtocol::OpenAiImages
        }
    }
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

pub struct OpenAiCompatibleImageProvider {
    config: OpenAiCompatibleImageConfig,
}

fn curl_config_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn curl_request_config(url: &str, api_key: &str, request_body_path: Option<&Path>) -> String {
    let mut config = format!("url = \"{}\"\n", curl_config_escape(url));
    if !api_key.trim().is_empty() {
        let authorization = format!("Authorization: Bearer {api_key}");
        config.push_str(&format!(
            "header = \"{}\"\n",
            curl_config_escape(&authorization)
        ));
    }
    if let Some(request_body_path) = request_body_path {
        config.push_str("request = \"POST\"\n");
        config.push_str("header = \"Content-Type: application/json\"\n");
        config.push_str(&format!(
            "data-binary = \"@{}\"\n",
            curl_config_escape(&request_body_path.to_string_lossy())
        ));
    }
    config
}

struct SensitiveRequestBody {
    path: PathBuf,
}

impl SensitiveRequestBody {
    fn write(contents: &str) -> Result<Self, ModelError> {
        let temp_dir = std::env::temp_dir();
        for _ in 0..32 {
            let id = CURL_REQUEST_BODY_ID.fetch_add(1, Ordering::Relaxed);
            let path = temp_dir.join(format!(
                "cindx-model-request-{}-{id}.json",
                std::process::id()
            ));
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(contents.as_bytes()).map_err(|error| {
                        let _ = fs::remove_file(&path);
                        ModelError::new(format!("failed to stage model request: {error}"))
                    })?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(ModelError::new(format!(
                        "failed to stage model request: {error}"
                    )))
                }
            }
        }
        Err(ModelError::new(
            "failed to allocate a private model request file",
        ))
    }
}

impl Drop for SensitiveRequestBody {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct CurlProcess {
    child: Child,
    _request_body: Option<SensitiveRequestBody>,
}

fn curl_command(timeout_seconds: u64, no_buffer: bool) -> Command {
    let mut command = Command::new("/usr/bin/curl");
    command
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg(timeout_seconds.to_string());
    if no_buffer {
        command.arg("--no-buffer");
    }
    command.arg("--config").arg("-");
    command
}

fn streaming_hard_timeout_seconds(idle_timeout_seconds: u64) -> u64 {
    idle_timeout_seconds
        .max(1)
        .saturating_mul(STREAMING_HARD_TIMEOUT_MULTIPLIER)
}

fn spawn_curl(
    url: &str,
    api_key: &str,
    request_body: Option<&str>,
    timeout_seconds: u64,
    no_buffer: bool,
) -> Result<CurlProcess, ModelError> {
    let request_body = request_body
        .map(SensitiveRequestBody::write)
        .transpose()?;
    let config = curl_request_config(
        url,
        api_key,
        request_body.as_ref().map(|body| body.path.as_path()),
    );
    let mut child = curl_command(timeout_seconds, no_buffer)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ModelError::new(format!("failed to start curl: {error}")))?;

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| ModelError::new("curl stdin was not available"))
        .and_then(|mut stdin| {
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| ModelError::new(format!("failed to configure curl: {error}")))
        });
    if let Err(error) = write_result {
        let output = child.wait_with_output().map_err(|wait_error| {
            ModelError::new(format!("{error}; failed to read curl failure: {wait_error}"))
        })?;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ModelError::new(if stderr.is_empty() {
            error.message
        } else {
            format!("curl rejected the request before upload: {stderr}")
        }));
    }

    Ok(CurlProcess {
        child,
        _request_body: request_body,
    })
}

fn execute_curl(
    url: &str,
    api_key: &str,
    request_body: Option<&str>,
    timeout_seconds: u64,
) -> Result<Output, ModelError> {
    execute_curl_cancellable(
        url,
        api_key,
        request_body,
        timeout_seconds,
        &mut || false,
    )
}

fn execute_curl_cancellable(
    url: &str,
    api_key: &str,
    request_body: Option<&str>,
    timeout_seconds: u64,
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<Output, ModelError> {
    let process = spawn_curl(url, api_key, request_body, timeout_seconds, false)?;
    consume_buffered_child(process, should_cancel)
}

fn consume_buffered_child(
    mut process: CurlProcess,
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<Output, ModelError> {
    let mut stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| ModelError::new("child stdout was not available"))?;
    let mut stderr = process
        .child
        .stderr
        .take()
        .ok_or_else(|| ModelError::new("child stderr was not available"))?;
    let stdout_handle = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|error| format!("failed to read child stdout: {error}"))
    });
    let stderr_handle = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(|error| format!("failed to read child stderr: {error}"))
    });

    let status = loop {
        if should_cancel() {
            let _ = process.child.kill();
            let _ = process.child.wait();
            return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
        }
        match process.child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(error) => {
                let _ = process.child.kill();
                let _ = process.child.wait();
                return Err(ModelError::new(format!("failed to wait for child: {error}")));
            }
        }
    };
    let stdout = stdout_handle
        .join()
        .map_err(|_| ModelError::new("child stdout reader panicked"))?
        .map_err(ModelError::new)?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| ModelError::new("child stderr reader panicked"))?
        .map_err(ModelError::new)?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn consume_streaming_child(
    mut process: CurlProcess,
    model: &str,
    base_url: &str,
    idle_timeout: Duration,
    on_delta: &mut impl FnMut(&str),
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<ModelResponse, ModelError> {
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| ModelError::new("curl stdout was not available"))?;
    let (line_sender, line_receiver) = mpsc::channel();
    let reader_handle = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if line_sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = line_sender.send(Err(format!("failed to read model stream: {error}")));
                    break;
                }
            }
        }
    });
    let mut raw_response = String::new();
    let mut answer = String::new();
    let mut streamed_tool_calls = BTreeMap::<usize, StreamingToolCall>::new();
    let idle_timeout = if idle_timeout.is_zero() {
        Duration::from_secs(1)
    } else {
        idle_timeout
    };
    let mut last_activity = Instant::now();

    loop {
        if should_cancel() {
            let _ = process.child.kill();
            let _ = process.child.wait();
            return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
        }
        match line_receiver.recv_timeout(Duration::from_millis(40)) {
            Ok(Ok(line)) => {
                last_activity = Instant::now();
                raw_response.push_str(&line);
                let event = match parse_stream_event(&line) {
                    Ok(event) => event,
                    Err(error) => {
                        let _ = process.child.kill();
                        let _ = process.child.wait();
                        return Err(error);
                    }
                };
                if let Some(event) = event {
                    if let Some(delta) = event.content {
                        answer.push_str(&delta);
                        on_delta(&delta);
                    }
                    for delta in event.tool_calls {
                        streamed_tool_calls
                            .entry(delta.index)
                            .or_default()
                            .merge(delta);
                    }
                }
            }
            Ok(Err(error)) => {
                let _ = process.child.kill();
                let _ = process.child.wait();
                let _ = reader_handle.join();
                return Err(ModelError::new(error));
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_activity.elapsed() >= idle_timeout {
                    let _ = process.child.kill();
                    let _ = process.child.wait();
                    let _ = reader_handle.join();
                    return Err(ModelError::new(format!(
                        "model stream timed out after {} seconds without receiving data",
                        idle_timeout.as_secs()
                    )));
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = process
        .child
        .wait()
        .map_err(|error| ModelError::new(format!("failed to wait for curl: {error}")))?;
    let _ = reader_handle.join();
    let mut stderr = String::new();
    if let Some(mut stream) = process.child.stderr.take() {
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

    let mut metadata = Metadata::new();
    metadata.insert("provider".to_string(), "openai-compatible".to_string());
    metadata.insert("model".to_string(), model.to_string());
    metadata.insert("base_url".to_string(), base_url.to_string());
    metadata.insert("streamed".to_string(), "true".to_string());
    let mut tool_calls = streamed_tool_calls
        .into_iter()
        .filter_map(|(index, call)| call.finish(index))
        .collect::<Vec<_>>();
    let mut raw_tool_calls_json = None;
    if answer.is_empty() && tool_calls.is_empty() {
        let fallback = parse_model_response(&raw_response)?;
        answer = fallback.message.content;
        tool_calls = fallback.tool_calls;
        raw_tool_calls_json = fallback.raw_tool_calls_json;
        for key in ["prompt_tokens", "completion_tokens", "total_tokens"] {
            if let Some(value) = fallback.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
    }
    metadata.insert("tool_calls".to_string(), tool_calls.len().to_string());
    if raw_tool_calls_json.is_none() && !tool_calls.is_empty() {
        raw_tool_calls_json = Some(
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
            .unwrap_or_else(|_| "[]".to_string()),
        );
    }

    Ok(ModelResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: answer,
            metadata: metadata.clone(),
        },
        raw_tool_calls_json,
        tool_calls,
        metadata,
    })
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        Self { config }
    }

    pub fn list_models(&self) -> Result<Vec<String>, ModelError> {
        if self.config.base_url.trim().is_empty() || self.config.api_key.trim().is_empty() {
            return Err(ModelError::new("provider base URL and API key are required"));
        }

        let output = execute_curl(
            &self.config.models_url(),
            &self.config.api_key,
            None,
            self.config.timeout_seconds,
        )?;

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
        let process = spawn_curl(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(&request_body),
            streaming_hard_timeout_seconds(self.config.timeout_seconds),
            true,
        )?;
        consume_streaming_child(
            process,
            &self.config.model,
            &self.config.base_url,
            Duration::from_secs(self.config.timeout_seconds.max(1)),
            &mut on_delta,
            &mut should_cancel,
        )
    }

    pub fn complete_once(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("provider config is incomplete"));
        }

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
        let output = execute_curl(
            &self.config.chat_completions_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
        )?;

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
        let output = execute_curl_cancellable(
            &self.config.embeddings_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
            &mut should_cancel,
        )?;

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

enum ImagePayload {
    Base64(String),
    Url(String),
}

struct ParsedImagePayload {
    payload: ImagePayload,
    revised_prompt: Option<String>,
}

impl OpenAiCompatibleImageProvider {
    pub fn new(config: OpenAiCompatibleImageConfig) -> Self {
        Self { config }
    }

    pub fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ModelError> {
        self.generate_cancellable(request, || false)
    }

    pub fn generate_cancellable(
        &self,
        request: ImageGenerationRequest,
        mut should_cancel: impl FnMut() -> bool,
    ) -> Result<ImageGenerationResponse, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new("image generation provider config is incomplete"));
        }
        if request.prompt.trim().is_empty() {
            return Err(ModelError::new("image generation prompt is empty"));
        }

        let protocol = self.config.protocol();
        let request_body = match protocol {
            ImageGenerationProtocol::OpenAiImages => build_image_generation_request_json(
                &self.config.model,
                &request.prompt,
                request.size.as_deref(),
            )?,
            ImageGenerationProtocol::DashScopeMultimodal => {
                build_dashscope_image_generation_request_json(
                    &self.config.model,
                    &request.prompt,
                    request.size.as_deref(),
                )?
            }
        };
        let output = execute_curl_cancellable(
            &self.config.images_url(),
            &self.config.api_key,
            Some(&request_body),
            self.config.timeout_seconds,
            &mut should_cancel,
        )?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let provider_error = parse_provider_error(&stdout).unwrap_or(stderr);
            return Err(ModelError::new(if provider_error.is_empty() {
                format!("image generation request failed with status {}", output.status)
            } else {
                provider_error
            }));
        }
        if output.stdout.len() > MAX_IMAGE_RESPONSE_BYTES {
            return Err(ModelError::new("image generation response exceeded 48 MB"));
        }

        let (response_model, payloads) =
            parse_image_generation_payloads(&stdout, &self.config.model)?;
        let mut images = Vec::with_capacity(payloads.len());
        for parsed in payloads {
            if should_cancel() {
                return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
            }
            let bytes = match parsed.payload {
                ImagePayload::Base64(value) => decode_generated_image(&value)?,
                ImagePayload::Url(url) => {
                    if !url.starts_with("https://") && !url.starts_with("http://") {
                        return Err(ModelError::new(
                            "image generation response included an unsupported URL",
                        ));
                    }
                    let output = execute_curl_cancellable(
                        &url,
                        "",
                        None,
                        self.config.timeout_seconds,
                        &mut should_cancel,
                    )?;
                    if !output.status.success() {
                        return Err(ModelError::new(format!(
                            "generated image download failed with status {}",
                            output.status
                        )));
                    }
                    output.stdout
                }
            };
            if bytes.is_empty() {
                return Err(ModelError::new("image generation returned an empty image"));
            }
            if bytes.len() > MAX_GENERATED_IMAGE_BYTES {
                return Err(ModelError::new("generated image exceeded 32 MB"));
            }
            let mime_type = generated_image_mime_type(&bytes)
                .ok_or_else(|| ModelError::new("image generation returned unsupported data"))?
                .to_string();
            images.push(GeneratedImage {
                bytes,
                mime_type,
                revised_prompt: parsed.revised_prompt,
            });
        }

        let mut metadata = request.metadata;
        metadata.insert("model".to_string(), response_model.clone());
        metadata.insert("images".to_string(), images.len().to_string());
        metadata.insert(
            "protocol".to_string(),
            match protocol {
                ImageGenerationProtocol::OpenAiImages => "openai-images",
                ImageGenerationProtocol::DashScopeMultimodal => "dashscope-multimodal",
            }
            .to_string(),
        );
        Ok(ImageGenerationResponse {
            model: response_model,
            images,
            metadata,
        })
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

pub fn build_image_generation_request_json(
    model: &str,
    prompt: &str,
    size: Option<&str>,
) -> Result<String, ModelError> {
    if model.trim().is_empty() {
        return Err(ModelError::new("image generation model is empty"));
    }
    if prompt.trim().is_empty() {
        return Err(ModelError::new("image generation prompt is empty"));
    }

    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": 1
    });
    if let Some(size) = size.filter(|value| !value.trim().is_empty()) {
        body["size"] = serde_json::Value::String(size.to_string());
    }
    serde_json::to_string(&body)
        .map_err(|error| ModelError::new(format!("failed to encode image request: {error}")))
}

fn build_dashscope_image_generation_request_json(
    model: &str,
    prompt: &str,
    size: Option<&str>,
) -> Result<String, ModelError> {
    if model.trim().is_empty() {
        return Err(ModelError::new("image generation model is empty"));
    }
    if prompt.trim().is_empty() {
        return Err(ModelError::new("image generation prompt is empty"));
    }

    let mut parameters = serde_json::json!({
        "n": 1,
        "watermark": false
    });
    if let Some(size) = size.filter(|value| !value.trim().is_empty()) {
        parameters["size"] = serde_json::Value::String(size.replace('x', "*"));
    }
    let body = serde_json::json!({
        "model": model,
        "input": {
            "messages": [{
                "role": "user",
                "content": [{ "text": prompt }]
            }]
        },
        "parameters": parameters
    });
    serde_json::to_string(&body)
        .map_err(|error| ModelError::new(format!("failed to encode image request: {error}")))
}

fn parse_image_generation_payloads(
    text: &str,
    fallback_model: &str,
) -> Result<(String, Vec<ParsedImagePayload>), ModelError> {
    if let Some(message) = parse_provider_error(text) {
        return Err(ModelError::new(message));
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| ModelError::new(format!("invalid image generation response: {error}")))?;
    if let Some(message) = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .filter(|_| value.get("code").is_some())
    {
        return Err(ModelError::new(message));
    }
    let model = value
        .get("model")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback_model)
        .to_string();
    let mut payloads = Vec::new();
    if let Some(data) = value.get("data").and_then(serde_json::Value::as_array) {
        for item in data {
            let revised_prompt = item
                .get("revised_prompt")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let payload = if let Some(value) = item
                .get("b64_json")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                ImagePayload::Base64(value.to_string())
            } else if let Some(value) = item
                .get("url")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                ImagePayload::Url(value.to_string())
            } else {
                return Err(ModelError::new(
                    "image generation item did not include image data",
                ));
            };
            payloads.push(ParsedImagePayload {
                payload,
                revised_prompt,
            });
        }
    } else if let Some(choices) = value
        .pointer("/output/choices")
        .and_then(serde_json::Value::as_array)
    {
        for content in choices.iter().filter_map(|choice| {
            choice
                .pointer("/message/content")
                .and_then(serde_json::Value::as_array)
        }) {
            for item in content {
                if let Some(url) = item
                    .get("image")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    payloads.push(ParsedImagePayload {
                        payload: ImagePayload::Url(url.to_string()),
                        revised_prompt: None,
                    });
                }
            }
        }
    } else {
        return Err(ModelError::new(
            "image generation response did not include image data",
        ));
    }
    if payloads.is_empty() {
        return Err(ModelError::new("image generation returned no images"));
    }
    Ok((model, payloads))
}

fn decode_generated_image(value: &str) -> Result<Vec<u8>, ModelError> {
    let encoded = value
        .strip_prefix("data:")
        .and_then(|data| data.split_once(',').map(|(_, encoded)| encoded))
        .unwrap_or(value);
    let compact = encoded
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .map_err(|error| ModelError::new(format!("invalid generated image data: {error}")))
}

fn generated_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis")
    {
        Some("image/avif")
    } else {
        None
    }
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

fn build_chat_request_json_with_tools_and_output_limit(
    model: &str,
    messages: &[Message],
    stream: bool,
    tools: &[ToolSpec],
    max_output_tokens: Option<u64>,
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
                "{{\"role\":\"{}\",\"content\":{}}}",
                json_escape(message_role_to_str(&message.role)),
                message_content_json(model, message)
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

fn message_content_json(model: &str, message: &Message) -> String {
    let Some(paths) = message.metadata.get("image_paths") else {
        return format!("\"{}\"", json_escape(&message.content));
    };
    if !model_supports_vision_content(model) {
        return format!(
            "\"{}\"",
            json_escape(&format!(
                "{}\n\n[Image attachment omitted because the selected model does not support vision.]",
                message.content
            ))
        );
    }
    let images = paths
        .lines()
        .filter_map(image_data_url)
        .collect::<Vec<_>>();
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

fn model_supports_vision_content(model: &str) -> bool {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StreamingToolCall {
    id: String,
    name: String,
    arguments_json: String,
}

impl StreamingToolCall {
    fn merge(&mut self, delta: StreamingToolCallDelta) {
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

    fn finish(self, index: usize) -> Option<ModelToolCall> {
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
struct StreamingToolCallDelta {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments_json: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StreamEvent {
    content: Option<String>,
    tool_calls: Vec<StreamingToolCallDelta>,
}

fn parse_stream_event(line: &str) -> Result<Option<StreamEvent>, ModelError> {
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
        return Ok(Some(StreamEvent::default()));
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
    }))
}

pub fn parse_stream_line(line: &str) -> Result<Option<String>, ModelError> {
    Ok(parse_stream_event(line)?.and_then(|event| event.content))
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
    use std::time::Instant;

    fn test_curl_process(child: Child) -> CurlProcess {
        CurlProcess {
            child,
            _request_body: None,
        }
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
        assert_eq!(config.embeddings_url(), "https://example.test/v1/embeddings");
    }

    #[test]
    fn image_url_accepts_a_base_url_or_explicit_endpoint() {
        let base = OpenAiCompatibleImageConfig {
            base_url: "https://example.test/v1/".to_string(),
            api_key: "key".to_string(),
            model: "image-a".to_string(),
            timeout_seconds: 10,
        };
        let dashscope = OpenAiCompatibleImageConfig {
            base_url: "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation".to_string(),
            ..base.clone()
        };

        assert_eq!(
            base.images_url(),
            "https://example.test/v1/images/generations"
        );
        assert_eq!(
            dashscope.images_url(),
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
        );
        assert_eq!(
            dashscope.protocol(),
            ImageGenerationProtocol::DashScopeMultimodal
        );
    }

    #[test]
    fn curl_keeps_credentials_off_arguments_and_large_bodies_off_config_stdin() {
        let request_text = format!("{{\"prompt\":\"{}\"}}", "x".repeat(2_000_000));
        let request_body = SensitiveRequestBody::write(&request_text)
            .expect("request body should be staged privately");
        let request_path = request_body.path.clone();
        let config = curl_request_config(
            "https://example.test/v1/chat/completions",
            "test-secret",
            Some(&request_path),
        );
        let command = curl_command(10, true);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(config.contains("Authorization: Bearer test-secret"));
        assert!(!curl_request_config("https://example.test/image.png", "", None)
            .contains("Authorization"));
        assert!(config.contains("data-binary"));
        assert!(config.len() < 2_048);
        assert!(!config.contains(&"x".repeat(1_000)));
        assert_eq!(
            fs::metadata(&request_path)
                .expect("request file should exist")
                .len(),
            request_text.len() as u64
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&request_path)
                    .expect("request file should exist")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(arguments.iter().any(|argument| argument == "--config"));
        assert!(arguments.iter().any(|argument| argument == "-"));
        assert!(!arguments.iter().any(|argument| argument.contains("test-secret")));
        assert!(!arguments.iter().any(|argument| argument.contains("prompt")));
        assert_eq!(curl_config_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
        drop(request_body);
        assert!(!request_path.exists());
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
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"printf 'data: {"choices":[{"delta":{"content":"started"}}]}\n\n'; sleep 2"#)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test stream process should start");
        let started = Instant::now();
        let mut output = String::new();
        let result = consume_streaming_child(
            test_curl_process(child),
            "test-model",
            "http://example.test/v1",
            Duration::from_secs(10),
            &mut |delta| output.push_str(delta),
            &mut || started.elapsed() >= Duration::from_millis(100),
        );

        assert_eq!(result.expect_err("stream should cancel").message, MODEL_REQUEST_CANCELLED);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(output, "started");
    }

    #[test]
    fn stops_a_stream_after_the_idle_timeout() {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test stream process should start");
        let started = Instant::now();
        let result = consume_streaming_child(
            test_curl_process(child),
            "test-model",
            "http://example.test/v1",
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
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(concat!(
                "printf 'data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\\n'; ",
                "sleep 0.1; ",
                "printf 'data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\\n'; ",
                "sleep 0.1; ",
                "printf 'data: {\"choices\":[{\"delta\":{\"content\":\"c\"}}]}\\n'; ",
                "sleep 0.1"
            ))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test stream process should start");
        let mut output = String::new();
        let response = consume_streaming_child(
            test_curl_process(child),
            "test-model",
            "http://example.test/v1",
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
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 2; printf done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test request process should start");
        let started = Instant::now();
        let result = consume_buffered_child(test_curl_process(child), &mut || {
            started.elapsed() >= Duration::from_millis(100)
        });

        assert_eq!(
            result.expect_err("request should cancel").message,
            MODEL_REQUEST_CANCELLED
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn streaming_reader_accepts_non_streaming_tool_call_fallback() {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(
                r#"printf '%s\n' '{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"file_read","arguments":"{\"input\":\"path=README.md\"}"}}]}}]}'"#,
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test response process should start");
        let response = consume_streaming_child(
            test_curl_process(child),
            "test-model",
            "http://example.test/v1",
            Duration::from_secs(10),
            &mut |_| {},
            &mut || false,
        )
        .expect("fallback response should parse");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "file_read");
        assert!(response.raw_tool_calls_json.is_some());
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
        assert_eq!(response.metadata.get("vectors").map(String::as_str), Some("2"));
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
