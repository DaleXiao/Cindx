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

use agent_core::Metadata;

use crate::ModelError;

/// Response-metadata fact: this response was served with the thinking
/// parameters suppressed (the endpoint rejected them on a body that actually
/// carried them), so an effort tier's thinking budget was NOT delivered on
/// this call. The durable model-turn event whitelists copy this key, and
/// evaluation receipts classify a nonzero count as treatment-undelivered.
pub const THINKING_SUPPRESSED_METADATA_KEY: &str = "thinking_params_suppressed";

/// Response-metadata fact: this streaming response was served without the
/// `stream_options` usage extension (the endpoint rejected it). Non-streaming
/// bodies never carry the extension and are never stamped.
pub const USAGE_EXTENSION_SUPPRESSED_METADATA_KEY: &str = "usage_extension_suppressed";

/// Stamps the suppression facts a response was served under; absent flags leave the metadata untouched.
pub(crate) fn attach_parameter_suppression_facts(
    metadata: &mut Metadata,
    thinking_suppressed: bool,
    usage_extension_suppressed: bool,
) {
    if thinking_suppressed {
        let key = THINKING_SUPPRESSED_METADATA_KEY;
        metadata.insert(key.to_string(), "true".to_string());
    }
    if usage_extension_suppressed {
        let key = USAGE_EXTENSION_SUPPRESSED_METADATA_KEY;
        metadata.insert(key.to_string(), "true".to_string());
    }
}

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

/// The exact `stream_options` segment the request builder emits. One owner
/// for the shape so the stripper can never drift from the producer.
pub(crate) const STREAM_OPTIONS_SEGMENT: &str = ",\"stream_options\":{\"include_usage\":true}";

/// True only for an HTTP 400 whose message names `stream_options` — an
/// endpoint that does not implement the OpenAI usage-chunk extension.
pub(crate) fn is_stream_options_rejection(error: &ModelError) -> bool {
    error.status_code == Some(400)
        && error
            .message
            .to_ascii_lowercase()
            .contains("stream_options")
}

/// Removes the exact `stream_options` segment, or `None` when absent.
pub(crate) fn strip_stream_options(body: &str) -> Option<String> {
    let start = body.find(STREAM_OPTIONS_SEGMENT)?;
    let mut stripped = String::with_capacity(body.len());
    stripped.push_str(&body[..start]);
    stripped.push_str(&body[start + STREAM_OPTIONS_SEGMENT.len()..]);
    Some(stripped)
}

/// The shared chained parameter-rejection fallback for both dispatch stages.
/// Dispatches the current body; on an exact 400 naming the thinking params or
/// the `stream_options` extension, strips that family (each at most once) and
/// re-dispatches. A rejection is recorded (for prepare-time suppression and
/// the response-metadata facts) only when the body actually carried that
/// family's strippable segment, so a 400 merely naming a parameter this
/// request never sent cannot mark the provider treatment-undelivered. The
/// loop bound plus strip-returns-None-when-absent guarantee termination
/// after at most two extra requests, in any rejection order — including one
/// 400 naming both families.
pub(crate) fn dispatch_with_parameter_fallbacks<Response>(
    body: String,
    mut dispatch: impl FnMut(&str) -> Result<Response, ModelError>,
    note_thinking: impl Fn(&ModelError) -> bool,
    note_stream_options: impl Fn(&ModelError) -> bool,
) -> Result<Response, ModelError> {
    let mut body = body;
    let mut thinking_stripped = false;
    let mut stream_options_stripped = false;
    for _pass in 0..2 {
        match dispatch(&body) {
            Err(error) if !thinking_stripped && is_thinking_parameter_rejection(&error) => {
                match strip_thinking_params(&body) {
                    Some(stripped) => {
                        note_thinking(&error);
                        body = stripped;
                        thinking_stripped = true;
                        continue;
                    }
                    None => return Err(error),
                }
            }
            Err(error) if !stream_options_stripped && is_stream_options_rejection(&error) => {
                match strip_stream_options(&body) {
                    Some(stripped) => {
                        note_stream_options(&error);
                        body = stripped;
                        stream_options_stripped = true;
                        continue;
                    }
                    None => return Err(error),
                }
            }
            other => return other,
        }
    }
    dispatch(&body)
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
    fn stream_options_rejection_strips_only_its_own_segment() {
        let body = format!(
            r#"{{"model":"m","stream":true,"messages":[]{},"temperature":0.2}}"#,
            STREAM_OPTIONS_SEGMENT
        );
        let stripped = strip_stream_options(&body).expect("segment strips");
        assert_eq!(
            stripped,
            r#"{"model":"m","stream":true,"messages":[],"temperature":0.2}"#
        );
        assert!(strip_stream_options(&stripped).is_none());

        let rejection =
            ModelError::with_status(400, "InvalidParameter: stream_options is not supported");
        assert!(is_stream_options_rejection(&rejection));
        let other =
            ModelError::with_status(400, "InvalidParameter: thinking_budget is not supported");
        assert!(!is_stream_options_rejection(&other));
        let server = ModelError::with_status(500, "stream_options exploded");
        assert!(!is_stream_options_rejection(&server));
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
