use crate::app_state::AppState;
use crate::configuration_persistence::clone_provider_config;
use model_provider::{OpenAiCompatibleRealtimeConfig, OpenAiCompatibleRealtimeProvider};
use serde::{Deserialize, Serialize};
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

#[tauri::command]
pub(crate) async fn negotiate_voice_session(
    app: tauri::AppHandle,
    input: VoiceSessionOfferInput,
) -> Result<VoiceSessionAnswer, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let config = clone_provider_config(&state)?;
        if !config.voice_is_ready() {
            return Err("Configure a full-duplex voice model before using voice input".to_string());
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
