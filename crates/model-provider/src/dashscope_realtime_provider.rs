use super::dashscope_realtime_guard::{
    enforce_transcript_limit, sanitized_socket_error, server_error, validate_pcm,
};
#[cfg(test)]
use super::dashscope_realtime_guard::{MAX_PCM_BYTES, MAX_TRANSCRIPT_BYTES};
use super::{run_http, ModelError};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use reqwest::Url;
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header, HeaderValue, Request},
        Message,
    },
};

const AUDIO_CHUNK_BYTES: usize = 32 * 1024;
const DASHSCOPE_TRANSCRIPTION_MODEL: &str = "qwen3-asr-flash-realtime";
const SOCKET_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashScopeRealtimeTranscriptionConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub connect_timeout_seconds: u64,
    pub finish_timeout_seconds: u64,
}

impl DashScopeRealtimeTranscriptionConfig {
    pub fn websocket_url(&self) -> Result<String, ModelError> {
        let mut url = Url::parse(self.base_url.trim())
            .map_err(|_| ModelError::new("DashScope realtime base URL is invalid"))?;
        let scheme = match url.scheme() {
            "https" | "wss" => "wss",
            "http" | "ws" => "ws",
            _ => {
                return Err(ModelError::new(
                    "DashScope realtime base URL must use HTTP or HTTPS",
                ))
            }
        };
        url.set_scheme(scheme)
            .map_err(|_| ModelError::new("DashScope realtime WebSocket scheme is invalid"))?;
        url.set_path("/api-ws/v1/realtime");
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("model", self.model.trim());
        Ok(url.to_string())
    }

    fn validate(&self) -> Result<(), ModelError> {
        if self.api_key.trim().is_empty() || self.model.trim().is_empty() {
            return Err(ModelError::new(
                "DashScope realtime API key and voice model are required",
            ));
        }
        if self.model.len() > 256 {
            return Err(ModelError::new(
                "DashScope realtime voice model is too long",
            ));
        }
        self.websocket_url().map(|_| ())
    }
}

pub struct DashScopeRealtimeTranscriptionProvider {
    config: DashScopeRealtimeTranscriptionConfig,
}

impl DashScopeRealtimeTranscriptionProvider {
    pub fn new(config: DashScopeRealtimeTranscriptionConfig) -> Self {
        Self { config }
    }

    pub fn transcribe_pcm16(&self, pcm: &[u8]) -> Result<String, ModelError> {
        self.config.validate()?;
        validate_pcm(pcm)?;
        let config = self.config.clone();
        let pcm = pcm.to_vec();
        run_http(async move { transcribe(config, pcm).await })
    }
}

async fn transcribe(
    config: DashScopeRealtimeTranscriptionConfig,
    pcm: Vec<u8>,
) -> Result<String, ModelError> {
    let request = websocket_request(&config)?;
    let connect_timeout = Duration::from_secs(config.connect_timeout_seconds.clamp(3, 15));
    let (mut socket, _) = timeout(connect_timeout, connect_async(request))
        .await
        .map_err(|_| ModelError::new("DashScope realtime connection timed out"))?
        .map_err(|error| sanitized_socket_error(&error.to_string(), &config.api_key))?;

    let session_timeout = Duration::from_secs(config.finish_timeout_seconds.clamp(5, 60));
    let session_result = bounded_wait(session_timeout, async {
        let mut event_sequence = 0_u64;
        wait_until_session_ready(
            &mut socket,
            connect_timeout,
            &config.api_key,
            &mut event_sequence,
        )
        .await?;
        for chunk in pcm.chunks(AUDIO_CHUNK_BYTES) {
            let audio = base64::engine::general_purpose::STANDARD.encode(chunk);
            send_event(
                &mut socket,
                &mut event_sequence,
                json!({ "type": "input_audio_buffer.append", "audio": audio }),
            )
            .await?;
        }
        send_event(
            &mut socket,
            &mut event_sequence,
            json!({ "type": "input_audio_buffer.commit" }),
        )
        .await?;

        collect_transcript(&mut socket, session_timeout, &config.api_key).await
    })
    .await;
    let _ = bounded_wait(SOCKET_CLOSE_TIMEOUT, socket.close(None)).await;
    session_result.map_err(|_| ModelError::new("DashScope realtime transcription timed out"))?
}

async fn bounded_wait<T>(
    wait: Duration,
    future: impl Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    timeout(wait, future).await
}

fn websocket_request(
    config: &DashScopeRealtimeTranscriptionConfig,
) -> Result<Request<()>, ModelError> {
    let mut request = config
        .websocket_url()?
        .into_client_request()
        .map_err(|error| ModelError::new(format!("failed to build DashScope request: {error}")))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", config.api_key.trim()))
        .map_err(|_| ModelError::new("DashScope API key contains invalid header characters"))?;
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, authorization);
    request.headers_mut().insert(
        header::USER_AGENT,
        HeaderValue::from_static("Cindx/voice-input"),
    );
    Ok(request)
}

async fn wait_until_session_ready<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    wait: Duration,
    api_key: &str,
    event_sequence: &mut u64,
) -> Result<(), ModelError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let future = async {
        while let Some(message) = socket.next().await {
            let value = text_message_value(message, api_key)?;
            match value.get("type").and_then(Value::as_str) {
                Some("session.created") => {
                    send_event(socket, event_sequence, session_update_event()).await?;
                }
                Some("session.updated") => return Ok(()),
                Some("error" | "conversation.item.input_audio_transcription.failed") => {
                    return Err(server_error(&value, api_key));
                }
                _ => {}
            }
        }
        Err(ModelError::new(
            "DashScope realtime connection closed before the session was ready",
        ))
    };
    timeout(wait, future)
        .await
        .map_err(|_| ModelError::new("DashScope realtime session setup timed out"))?
}

async fn collect_transcript<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    wait: Duration,
    api_key: &str,
) -> Result<String, ModelError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let future = async {
        while let Some(message) = socket.next().await {
            let value = text_message_value(message, api_key)?;
            match value.get("type").and_then(Value::as_str) {
                Some("conversation.item.input_audio_transcription.completed") => {
                    let transcript = value
                        .get("transcript")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim();
                    enforce_transcript_limit(transcript.len())?;
                    return Ok(transcript.to_string());
                }
                Some("error" | "conversation.item.input_audio_transcription.failed") => {
                    return Err(server_error(&value, api_key));
                }
                _ => {}
            }
        }
        Err(ModelError::new(
            "DashScope realtime connection closed before transcription completed",
        ))
    };
    timeout(wait, future)
        .await
        .map_err(|_| ModelError::new("DashScope realtime transcription timed out"))?
}

async fn send_event<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    sequence: &mut u64,
    mut value: Value,
) -> Result<(), ModelError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    *sequence += 1;
    value["event_id"] = Value::String(format!("cindx-voice-{sequence}"));
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|error| ModelError::new(format!("DashScope realtime send failed: {error}")))
}

fn text_message_value(
    message: Result<Message, tokio_tungstenite::tungstenite::Error>,
    api_key: &str,
) -> Result<Value, ModelError> {
    let message = message.map_err(|error| sanitized_socket_error(&error.to_string(), api_key))?;
    let Message::Text(text) = message else {
        return Ok(Value::Null);
    };
    serde_json::from_str(&text)
        .map_err(|_| ModelError::new("DashScope realtime returned invalid JSON"))
}

fn session_update_event() -> Value {
    json!({
        "type": "session.update",
        "session": {
            "input_audio_format": "pcm",
            "input_audio_transcription": {
                "model": DASHSCOPE_TRANSCRIPTION_MODEL
            },
            "turn_detection": null
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_url_preserves_region_host_and_replaces_compatible_path() {
        let config = DashScopeRealtimeTranscriptionConfig {
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            api_key: "secret".to_string(),
            model: "qwen3.5-omni-flash-realtime".to_string(),
            connect_timeout_seconds: 8,
            finish_timeout_seconds: 45,
        };
        assert_eq!(
            config.websocket_url().expect("URL should resolve"),
            "wss://dashscope.aliyuncs.com/api-ws/v1/realtime?model=qwen3.5-omni-flash-realtime"
        );
    }

    #[test]
    fn session_update_requests_pcm16_16khz_without_server_vad() {
        let value = session_update_event();
        assert_eq!(value["session"]["input_audio_format"], "pcm");
        assert!(value["session"].get("sample_rate").is_none());
        assert!(value["session"]["turn_detection"].is_null());
        assert_eq!(
            value["session"]["input_audio_transcription"]["model"],
            DASHSCOPE_TRANSCRIPTION_MODEL
        );
        assert!(!value.to_string().contains("session.finish"));
    }

    #[test]
    fn websocket_request_keeps_standard_upgrade_headers_with_bearer_auth() {
        let config = DashScopeRealtimeTranscriptionConfig {
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            api_key: "secret".to_string(),
            model: "qwen3.5-omni-flash-realtime".to_string(),
            connect_timeout_seconds: 8,
            finish_timeout_seconds: 45,
        };
        let request = websocket_request(&config).expect("handshake request should build");
        for name in [
            header::UPGRADE,
            header::CONNECTION,
            header::SEC_WEBSOCKET_KEY,
            header::SEC_WEBSOCKET_VERSION,
        ] {
            assert!(request.headers().contains_key(name));
        }
        assert_eq!(request.headers()[header::AUTHORIZATION], "Bearer secret");
    }

    #[test]
    fn audio_and_transcript_limits_are_bounded() {
        assert!(validate_pcm(&[]).is_err());
        assert!(validate_pcm(&vec![0; MAX_PCM_BYTES]).is_ok());
        assert!(validate_pcm(&vec![0; MAX_PCM_BYTES + 2]).is_err());
        assert!(validate_pcm(&[0]).is_err());
        assert!(enforce_transcript_limit(MAX_TRANSCRIPT_BYTES).is_ok());
        assert!(enforce_transcript_limit(MAX_TRANSCRIPT_BYTES + 1).is_err());
    }

    #[test]
    fn provider_errors_never_echo_the_api_key() {
        let error = sanitized_socket_error("request secret-token rejected", "secret-token");
        assert!(!error.message.contains("secret-token"));
        assert!(error.message.contains("[REDACTED]"));
    }

    #[test]
    fn session_deadline_releases_a_stalled_operation() {
        let result = run_http(async {
            bounded_wait(Duration::from_millis(10), std::future::pending::<()>())
                .await
                .map_err(|_| ModelError::new("deadline reached"))
        });
        assert_eq!(
            result.expect_err("pending work must time out").message,
            "deadline reached"
        );
    }
}
