use super::ModelError;
use serde_json::Value;

const PCM_SAMPLE_RATE: u32 = 16_000;
pub(super) const MAX_PCM_BYTES: usize = PCM_SAMPLE_RATE as usize * 2 * 30;
pub(super) const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

pub(super) fn validate_pcm(pcm: &[u8]) -> Result<(), ModelError> {
    if pcm.is_empty() {
        return Err(ModelError::new("Voice input did not contain audio"));
    }
    if pcm.len() > MAX_PCM_BYTES {
        return Err(ModelError::new("Voice input exceeds the 30 second limit"));
    }
    if !pcm.len().is_multiple_of(2) {
        return Err(ModelError::new("Voice input PCM16 payload is invalid"));
    }
    Ok(())
}

pub(super) fn enforce_transcript_limit(bytes: usize) -> Result<(), ModelError> {
    if bytes > MAX_TRANSCRIPT_BYTES {
        Err(ModelError::new("Voice transcript exceeds 64 KB"))
    } else {
        Ok(())
    }
}

pub(super) fn server_error(value: &Value, api_key: &str) -> ModelError {
    let raw = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/code").and_then(Value::as_str))
        .or_else(|| {
            value
                .pointer("/header/error_message")
                .and_then(Value::as_str)
        })
        .or_else(|| value.pointer("/header/error_code").and_then(Value::as_str))
        .unwrap_or("DashScope realtime request failed");
    sanitized_socket_error(raw, api_key)
}

pub(super) fn sanitized_socket_error(raw: &str, api_key: &str) -> ModelError {
    let redacted = if api_key.is_empty() {
        raw.to_string()
    } else {
        raw.replace(api_key, "[REDACTED]")
    };
    let bounded: String = redacted
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(1_024)
        .collect();
    ModelError::new(bounded)
}
