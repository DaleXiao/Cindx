use crate::provider_receipt::{attach_response_semantic_sha256, finalize_provider_receipt_status};
use crate::response_parser::{normalize_dsml_tool_calls, serialize_tool_calls};
use crate::streaming_wire::StreamingToolCall;
use crate::{parse_model_response, ModelError, ModelResponse};
use agent_core::{Message, MessageRole, Metadata};
use std::collections::BTreeMap;

pub(crate) struct StreamingResponseParts {
    pub(crate) fallback_response: String,
    pub(crate) fallback_truncated: bool,
    pub(crate) answer: String,
    pub(crate) streamed_tool_calls: BTreeMap<usize, StreamingToolCall>,
    pub(crate) finish_reason: Option<String>,
    /// The recognized stream ended at EOF with no finish_reason and no `[DONE]`
    /// sentinel, so completion was never confirmed by the provider.
    pub(crate) eof_without_finish: bool,
    pub(crate) usage: Metadata,
}

pub(crate) fn finish_streaming_response(
    parts: StreamingResponseParts,
    model: &str,
    base_url: &str,
) -> Result<ModelResponse, ModelError> {
    let StreamingResponseParts {
        fallback_response,
        fallback_truncated,
        mut answer,
        streamed_tool_calls,
        mut finish_reason,
        eof_without_finish,
        usage,
    } = parts;
    let mut metadata = Metadata::new();
    metadata.insert("provider".to_string(), "openai-compatible".to_string());
    metadata.insert(
        "provider_protocol".to_string(),
        "openai-compatible".to_string(),
    );
    metadata.insert("model".to_string(), model.to_string());
    metadata.insert("base_url".to_string(), base_url.to_string());
    metadata.insert("streamed".to_string(), "true".to_string());
    metadata.extend(usage);
    let mut tool_calls = streamed_tool_calls
        .into_iter()
        .filter_map(|(index, call)| call.finish(index))
        .collect::<Vec<_>>();
    let mut raw_tool_calls_json = None;
    if answer.is_empty() && tool_calls.is_empty() {
        if fallback_truncated {
            return Err(ModelError::new(
                "model returned an unrecognized non-streaming response larger than 1 MB",
            ));
        }
        let fallback = parse_model_response(&fallback_response)?;
        answer = fallback.message.content;
        tool_calls = fallback.tool_calls;
        raw_tool_calls_json = fallback.raw_tool_calls_json;
        if finish_reason.is_none() {
            finish_reason = fallback.metadata.get("finish_reason").cloned();
        }
        for key in [
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "tool_protocol",
            "provider_response_id",
            "provider_response_model",
            "provider_system_fingerprint",
        ] {
            if let Some(value) = fallback.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
    }
    if normalize_dsml_tool_calls(&mut answer, &mut tool_calls)? {
        metadata.insert("tool_protocol".to_string(), "dsml".to_string());
    }
    metadata.insert("tool_calls".to_string(), tool_calls.len().to_string());
    if let Some(finish_reason) = finish_reason.filter(|value| !value.trim().is_empty()) {
        metadata.insert("finish_reason".to_string(), finish_reason);
    }
    if eof_without_finish && !metadata.contains_key("finish_reason") {
        metadata.insert("stream_truncated_eof".to_string(), "true".to_string());
    }
    if raw_tool_calls_json.is_none() && !tool_calls.is_empty() {
        raw_tool_calls_json = Some(serialize_tool_calls(&tool_calls));
    }
    finalize_provider_receipt_status(&mut metadata);
    let finish_reason = metadata.get("finish_reason").cloned();
    attach_response_semantic_sha256(
        &mut metadata,
        &answer,
        &tool_calls,
        finish_reason.as_deref(),
    );

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
