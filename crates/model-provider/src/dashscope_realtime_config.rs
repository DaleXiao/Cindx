use super::ModelError;
use reqwest::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DashScopeAsrProtocol {
    RealtimeSession,
    InferenceTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashScopeRealtimeTranscriptionConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub connect_timeout_seconds: u64,
    pub finish_timeout_seconds: u64,
}

impl DashScopeRealtimeTranscriptionConfig {
    pub(crate) fn protocol(&self) -> DashScopeAsrProtocol {
        let model = self.model.trim().to_ascii_lowercase();
        if model.starts_with("fun-asr-") || model.starts_with("qwen-audio-3.0-asr-") {
            DashScopeAsrProtocol::InferenceTask
        } else {
            DashScopeAsrProtocol::RealtimeSession
        }
    }

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
        url.set_path(match self.protocol() {
            DashScopeAsrProtocol::RealtimeSession => "/api-ws/v1/realtime",
            DashScopeAsrProtocol::InferenceTask => "/api-ws/v1/inference",
        });
        url.set_query(None);
        url.set_fragment(None);
        if self.protocol() == DashScopeAsrProtocol::RealtimeSession {
            url.query_pairs_mut()
                .append_pair("model", self.model.trim());
        }
        Ok(url.to_string())
    }

    pub(crate) fn validate(&self) -> Result<(), ModelError> {
        if self.api_key.trim().is_empty() || self.model.trim().is_empty() {
            return Err(ModelError::new(
                "DashScope API key and speech recognition model are required",
            ));
        }
        if self.model.len() > 256 {
            return Err(ModelError::new(
                "DashScope speech recognition model is too long",
            ));
        }
        self.websocket_url().map(|_| ())
    }
}
