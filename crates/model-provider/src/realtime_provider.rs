use super::{await_http, collect_response_body, http_client, run_http, ModelError};
use crate::response_parser::parse_provider_error;
use reqwest::{header, multipart};
use serde_json::json;
use std::time::{Duration, Instant};

const MAX_OFFER_SDP_BYTES: usize = 256 * 1024;
const MAX_ANSWER_SDP_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleRealtimeConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

impl OpenAiCompatibleRealtimeConfig {
    pub fn calls_url(&self) -> String {
        let endpoint = self.base_url.trim_end_matches('/');
        if endpoint.ends_with("/realtime/calls") {
            endpoint.to_string()
        } else {
            format!("{endpoint}/realtime/calls")
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }
}

pub struct OpenAiCompatibleRealtimeProvider {
    config: OpenAiCompatibleRealtimeConfig,
}

impl OpenAiCompatibleRealtimeProvider {
    pub fn new(config: OpenAiCompatibleRealtimeConfig) -> Self {
        Self { config }
    }

    pub fn negotiate_sdp(&self, offer_sdp: &str) -> Result<String, ModelError> {
        if !self.config.is_ready() {
            return Err(ModelError::new(
                "realtime voice provider config is incomplete",
            ));
        }
        validate_offer_sdp(offer_sdp)?;

        let timeout = Duration::from_secs(self.config.timeout_seconds.clamp(5, 60));
        let session = build_realtime_session_json(&self.config.model)?;
        let form = multipart::Form::new()
            .text("sdp", offer_sdp.to_string())
            .text("session", session);
        let request = http_client()?
            .post(self.config.calls_url())
            .bearer_auth(self.config.api_key.trim())
            .header(header::ACCEPT, "application/sdp")
            .multipart(form)
            .timeout(timeout);

        run_http(async move {
            let deadline = Instant::now() + timeout;
            let mut never_cancel = || false;
            let response = await_http(
                request.send(),
                deadline,
                timeout,
                "realtime voice negotiation",
                &mut never_cancel,
            )
            .await?;
            let status = response.status();
            let body = collect_response_body(
                response,
                MAX_ANSWER_SDP_BYTES,
                deadline,
                timeout,
                "realtime voice negotiation",
                &mut never_cancel,
            )
            .await?;
            let answer = String::from_utf8(body)
                .map_err(|_| ModelError::new("realtime voice provider returned non-UTF-8 SDP"))?;
            if !status.is_success() {
                let provider_error = parse_provider_error(&answer).unwrap_or_default();
                return Err(ModelError::with_status(
                    status.as_u16(),
                    if provider_error.is_empty() {
                        format!("realtime voice negotiation failed with status {status}")
                    } else {
                        provider_error
                    },
                ));
            }
            if !answer.trim_start().starts_with("v=0") {
                return Err(ModelError::new(
                    "realtime voice provider returned an invalid SDP answer",
                ));
            }
            Ok(answer)
        })
    }
}

pub fn build_realtime_session_json(model: &str) -> Result<String, ModelError> {
    let model = model.trim();
    if model.is_empty() {
        return Err(ModelError::new("realtime voice model is empty"));
    }
    serde_json::to_string(&json!({
        "type": "realtime",
        "model": model,
        "audio": {
            "input": {
                "transcription": { "model": "gpt-4o-mini-transcribe" },
                "turn_detection": null
            }
        }
    }))
    .map_err(|error| ModelError::new(format!("failed to encode realtime session: {error}")))
}

fn validate_offer_sdp(offer_sdp: &str) -> Result<(), ModelError> {
    if offer_sdp.trim().is_empty() {
        return Err(ModelError::new("realtime voice SDP offer is empty"));
    }
    if offer_sdp.len() > MAX_OFFER_SDP_BYTES {
        return Err(ModelError::new("realtime voice SDP offer exceeds 256 KB"));
    }
    if !offer_sdp.trim_start().starts_with("v=0") {
        return Err(ModelError::new("realtime voice SDP offer is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_calls_url_accepts_provider_roots_and_explicit_endpoints() {
        let mut config = OpenAiCompatibleRealtimeConfig {
            base_url: "https://api.openai.com/v1/".to_string(),
            api_key: "key".to_string(),
            model: "gpt-realtime".to_string(),
            timeout_seconds: 30,
        };
        assert_eq!(
            config.calls_url(),
            "https://api.openai.com/v1/realtime/calls"
        );
        config.base_url = "https://example.test/realtime/calls".to_string();
        assert_eq!(config.calls_url(), "https://example.test/realtime/calls");
    }

    #[test]
    fn realtime_session_uses_the_selected_voice_model_and_manual_transcription_turns() {
        let value: serde_json::Value = serde_json::from_str(
            &build_realtime_session_json("gpt-realtime").expect("session should encode"),
        )
        .expect("session should be JSON");
        assert_eq!(value["type"], "realtime");
        assert_eq!(value["model"], "gpt-realtime");
        assert!(value.get("output_modalities").is_none());
        assert_eq!(
            value["audio"]["input"]["transcription"]["model"],
            "gpt-4o-mini-transcribe"
        );
        assert!(value["audio"]["input"]["turn_detection"].is_null());
    }

    #[test]
    fn realtime_offer_validation_rejects_empty_and_non_sdp_payloads() {
        assert!(validate_offer_sdp("").is_err());
        assert!(validate_offer_sdp("not-sdp").is_err());
        assert!(validate_offer_sdp("v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\n").is_ok());
    }
}
