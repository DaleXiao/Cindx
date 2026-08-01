use crate::app_state::AppState;
use crate::configuration_persistence::clone_provider_config;
use crate::event_security::redact_sensitive_text;
use crate::persistence_runtime::append_startup_log;
use crate::provider_profiles::ProviderVoiceTransport;
use base64::Engine;
use model_provider::{
    DashScopeRealtimeTranscriptionConfig, DashScopeRealtimeTranscriptionProvider,
    OpenAiCompatibleRealtimeConfig, OpenAiCompatibleRealtimeProvider,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tauri::Manager;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceSessionOfferInput {
    offer_sdp: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceSessionAnswer {
    answer_sdp: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceTranscriptionInput {
    pcm_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceTranscription {
    transcript: String,
}

#[tauri::command]
pub(crate) async fn negotiate_voice_session(
    app: tauri::AppHandle,
    input: VoiceSessionOfferInput,
) -> Result<VoiceSessionAnswer, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let config = clone_provider_config(&state)?;
        if config.voice_transport() != ProviderVoiceTransport::OpenAiWebRtc {
            return Err(format!(
                "Full-duplex voice is not available for provider '{}' because it requires a different realtime transport adapter",
                config.provider_id
            ));
        }
        if !config.voice_is_ready() {
            return Err("Configure a speech recognition model before using voice input".to_string());
        }
        let provider = OpenAiCompatibleRealtimeProvider::new(OpenAiCompatibleRealtimeConfig {
            base_url: config.base_url,
            api_key: config.api_key,
            model: config.voice_model,
            timeout_seconds: 30,
        });
        provider
            .negotiate_sdp(&input.offer_sdp)
            .map(|answer_sdp| VoiceSessionAnswer { answer_sdp })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("voice negotiation task failed to join: {error}"))?
}

#[tauri::command]
pub(crate) async fn transcribe_voice_audio(
    app: tauri::AppHandle,
    input: VoiceTranscriptionInput,
) -> Result<VoiceTranscription, String> {
    const MAX_PCM_BASE64_BYTES: usize = 1_280_004;
    if input.pcm_base64.len() > MAX_PCM_BASE64_BYTES {
        return Err("Voice input exceeds the 30 second limit".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let config = clone_provider_config(&state)?;
        if config.voice_transport() != ProviderVoiceTransport::DashScopeWebSocket {
            return Err(
                "The configured voice provider does not use a DashScope ASR adapter".to_string(),
            );
        }
        if !config.voice_is_ready() {
            return Err(
                "Configure and verify a speech recognition model before using voice input".to_string(),
            );
        }
        let pcm = base64::engine::general_purpose::STANDARD
            .decode(input.pcm_base64.as_bytes())
            .map_err(|_| "Voice input PCM payload is invalid".to_string())?;
        let pcm_bytes = pcm.len();
        let started = Instant::now();
        let provider = DashScopeRealtimeTranscriptionProvider::new(DashScopeRealtimeTranscriptionConfig {
            base_url: config.base_url,
            api_key: config.api_key,
            model: config.voice_model,
            connect_timeout_seconds: 8,
            finish_timeout_seconds: 45,
        });
        match provider.transcribe_pcm16(&pcm) {
            Ok(transcript) => Ok(VoiceTranscription { transcript }),
            Err(error) => {
                let error = redact_sensitive_text(&error.to_string())
                    .lines()
                    .collect::<Vec<_>>()
                    .join(" ");
                append_startup_log(&format!(
                    "voice transcription failed stage=dashscope_transcribe pcm_bytes={pcm_bytes} elapsed_ms={} error={error}",
                    started.elapsed().as_millis()
                ));
                Err(error)
            }
        }
    })
    .await
    .map_err(|error| format!("voice transcription task failed to join: {error}"))?
}
