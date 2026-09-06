//! Provider-side thinking-parameter rejection fallback.
//!
//! Some OpenAI-compatible endpoints reject the thinking parameters that the
//! family-name predicate attaches — the consumed Phase 4 run measured this in
//! vivo: dashscope-hosted `kimi-k3` answers every High/Xhigh call with
//! `400 InvalidParameter: Parameter thinking_budget is not supported`, while
//! its `kimi-k2` sibling accepts them. Model support is a provider×model fact
//! that no name predicate can know, so instead of guessing harder the provider
//! detects exactly that rejection, strips the thinking params from the
//! (possibly pre-prepared) request body, retries once, and suppresses the
//! params at prepare time for the rest of the provider instance's life.

use bytes::Bytes;

use crate::prepared_payload::PreparedStreamingModelRequest;
use crate::provider_receipt::request_payload_sha256;
use crate::ModelError;

/// True only for the precise provider rejection this fallback addresses: an
/// HTTP 400 whose message names a thinking parameter. Deliberately narrow —
/// every other error must surface unchanged.
pub(crate) fn is_thinking_parameter_rejection(error: &ModelError) -> bool {
    if error.status_code != Some(400) {
        return false;
    }
    let message = error.message.to_ascii_lowercase();
    message.contains("thinking_budget") || message.contains("enable_thinking")
}

/// Removes the thinking params — in the exact shapes `thinking_json_for`
/// emits — from an encoded request body. Returns `None` when the body carries
/// no thinking params, so a rejection of a body without them never retries.
pub(crate) fn strip_thinking_params(body: &str) -> Option<String> {
    let key = ",\"enable_thinking\":";
    let start = body.find(key)?;
    let rest = &body[start + key.len()..];
    let end = if rest.starts_with("true") {
        // Shape: ,"enable_thinking":true,"thinking_budget":<digits>
        let budget_key = ",\"thinking_budget\":";
        let budget_at = rest.find(budget_key)?;
        let digits_at = budget_at + budget_key.len();
        let digits = rest[digits_at..]
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        digits_at + digits
    } else if rest.starts_with("false") {
        "false".len()
    } else {
        return None;
    };
    let mut stripped = String::with_capacity(body.len());
    stripped.push_str(&body[..start]);
    stripped.push_str(&rest[end..]);
    Some(stripped)
}

/// The streaming prepared request rebuilt without thinking params (fresh
/// payload digest, same prompt estimate), or `None` when there is nothing to
/// strip.
pub(crate) fn streaming_prepared_without_thinking_params(
    request: &PreparedStreamingModelRequest,
) -> Option<PreparedStreamingModelRequest> {
    let (body, estimated_prompt_tokens, _sha) = request.encoded_parts()?;
    let text = std::str::from_utf8(body).ok()?;
    let stripped = strip_thinking_params(text)?;
    Some(PreparedStreamingModelRequest::encoded(
        stripped,
        estimated_prompt_tokens,
    ))
}

/// The non-streaming encoded body rebuilt without thinking params (fresh
/// payload digest), or `None` when there is nothing to strip.
pub(crate) fn non_streaming_body_without_thinking_params(body: &Bytes) -> Option<(Bytes, String)> {
    let text = std::str::from_utf8(body).ok()?;
    let stripped = strip_thinking_params(text)?;
    let digest = request_payload_sha256(stripped.as_bytes());
    Some((Bytes::from(stripped), digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_both_emitted_thinking_shapes_and_only_those() {
        let with_budget = r#"{"model":"kimi-k3","stream":false,"messages":[],"enable_thinking":true,"thinking_budget":8192,"temperature":0.2}"#;
        let stripped = strip_thinking_params(with_budget).expect("budget shape strips");
        assert_eq!(
            stripped,
            r#"{"model":"kimi-k3","stream":false,"messages":[],"temperature":0.2}"#
        );

        let disabled = r#"{"model":"kimi-k3","messages":[],"enable_thinking":false,"tools":[]}"#;
        let stripped = strip_thinking_params(disabled).expect("disabled shape strips");
        assert_eq!(stripped, r#"{"model":"kimi-k3","messages":[],"tools":[]}"#);

        // A body without thinking params offers nothing to strip, so the
        // fallback never retries it.
        assert!(strip_thinking_params(r#"{"model":"a","messages":[]}"#).is_none());
        // A malformed shape is refused rather than guessed at.
        assert!(strip_thinking_params(r#"{"enable_thinking":true}"#).is_none());
    }

    #[test]
    fn only_a_400_naming_a_thinking_parameter_triggers_the_fallback() {
        let rejection = ModelError::with_status(
            400,
            "InternalError.Algo.InvalidParameter: Parameter thinking_budget is not supported.",
        );
        assert!(is_thinking_parameter_rejection(&rejection));

        let enable_rejection =
            ModelError::with_status(400, "InvalidParameter: enable_thinking is not supported");
        assert!(is_thinking_parameter_rejection(&enable_rejection));

        // Neither a different status nor a different 400 qualifies.
        let server_error = ModelError::with_status(500, "thinking_budget exploded");
        assert!(!is_thinking_parameter_rejection(&server_error));
        let other_400 = ModelError::with_status(400, "InvalidParameter: top_p is not supported");
        assert!(!is_thinking_parameter_rejection(&other_400));
        let unstatused = ModelError::new("thinking_budget is not supported");
        assert!(!is_thinking_parameter_rejection(&unstatused));
    }
}
