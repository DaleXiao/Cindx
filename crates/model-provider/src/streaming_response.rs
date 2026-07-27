use super::{
    collect_response_body, parse_model_response, parse_provider_error, DsmlStreamDeltaFilter,
    ModelError, ModelResponse, HTTP_POLL_INTERVAL, MAX_MODEL_RESPONSE_BYTES,
    MODEL_REQUEST_CANCELLED,
};
use crate::response_parser::{normalize_dsml_tool_calls, serialize_tool_calls};
use crate::streaming_wire::{parse_stream_event, StreamingToolCall};
use agent_core::{Message, MessageRole, Metadata};
use futures_util::{Stream, StreamExt};
use reqwest::{header, Response};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const MAX_STREAMING_FALLBACK_BYTES: usize = 1024 * 1024;
const MAX_STREAM_EVENT_BYTES: usize = 8 * 1024 * 1024;

fn apply_stream_line(
    line: &str,
    answer: &mut String,
    streamed_tool_calls: &mut BTreeMap<usize, StreamingToolCall>,
    finish_reason: &mut Option<String>,
    usage: &mut Metadata,
    on_delta: &mut impl FnMut(&str),
) -> Result<bool, ModelError> {
    if let Some(event) = parse_stream_event(line)? {
        if event.finish_reason.is_some() {
            *finish_reason = event.finish_reason;
        }
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
        usage.extend(event.usage);
        return Ok(true);
    }
    Ok(false)
}

fn apply_complete_stream_lines(
    pending: &mut Vec<u8>,
    answer: &mut String,
    streamed_tool_calls: &mut BTreeMap<usize, StreamingToolCall>,
    finish_reason: &mut Option<String>,
    usage: &mut Metadata,
    on_delta: &mut impl FnMut(&str),
) -> Result<bool, ModelError> {
    let mut consumed = 0;
    let mut parsed_event = false;
    for index in 0..pending.len() {
        if pending[index] != b'\n' {
            continue;
        }
        let line = String::from_utf8_lossy(&pending[consumed..=index]);
        parsed_event |= apply_stream_line(
            &line,
            answer,
            streamed_tool_calls,
            finish_reason,
            usage,
            on_delta,
        )?;
        consumed = index + 1;
    }
    if consumed > 0 {
        pending.drain(..consumed);
    }
    Ok(parsed_event)
}

pub(super) fn finish_streaming_response(
    fallback_response: String,
    fallback_truncated: bool,
    mut answer: String,
    streamed_tool_calls: BTreeMap<usize, StreamingToolCall>,
    mut finish_reason: Option<String>,
    usage: Metadata,
    model: &str,
    base_url: &str,
) -> Result<ModelResponse, ModelError> {
    let mut metadata = Metadata::new();
    metadata.insert("provider".to_string(), "openai-compatible".to_string());
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
    if raw_tool_calls_json.is_none() && !tool_calls.is_empty() {
        raw_tool_calls_json = Some(serialize_tool_calls(&tool_calls));
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn consume_streaming_body<S, B, E>(
    stream: S,
    model: &str,
    base_url: &str,
    idle_timeout: Duration,
    hard_timeout: Duration,
    deadline: Instant,
    on_delta: &mut impl FnMut(&str),
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<ModelResponse, ModelError>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let idle_timeout = if idle_timeout.is_zero() {
        Duration::from_secs(1)
    } else {
        idle_timeout
    };
    let mut stream = Box::pin(stream);
    let mut fallback_response = Vec::new();
    let mut fallback_truncated = false;
    let mut stream_protocol_seen = false;
    let mut received_bytes = 0usize;
    let mut pending = Vec::new();
    let mut answer = String::new();
    let mut streamed_tool_calls = BTreeMap::<usize, StreamingToolCall>::new();
    let mut finish_reason = None;
    let mut usage = Metadata::new();
    let mut last_activity = Instant::now();
    let mut dsml_filter = DsmlStreamDeltaFilter::default();
    let mut filtered_on_delta = |delta: &str| dsml_filter.push(delta, on_delta);

    loop {
        if should_cancel() {
            return Err(ModelError::new(MODEL_REQUEST_CANCELLED));
        }
        if Instant::now() >= deadline {
            return Err(ModelError::new(format!(
                "model stream timed out after {} seconds",
                hard_timeout.as_secs().max(1)
            )));
        }
        match tokio::time::timeout(HTTP_POLL_INTERVAL, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                last_activity = Instant::now();
                let chunk = chunk.as_ref();
                received_bytes = received_bytes.saturating_add(chunk.len());
                if received_bytes > MAX_MODEL_RESPONSE_BYTES {
                    return Err(ModelError::new("model stream exceeded 64 MB"));
                }
                if !stream_protocol_seen {
                    let remaining = MAX_STREAMING_FALLBACK_BYTES
                        .saturating_sub(fallback_response.len());
                    fallback_response.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                    fallback_truncated |= chunk.len() > remaining;
                }
                pending.extend_from_slice(chunk);
                let parsed_event = apply_complete_stream_lines(
                    &mut pending,
                    &mut answer,
                    &mut streamed_tool_calls,
                    &mut finish_reason,
                    &mut usage,
                    &mut filtered_on_delta,
                )?;
                if pending.len() > MAX_STREAM_EVENT_BYTES {
                    return Err(ModelError::new(
                        "model stream event exceeded the 8 MB line limit",
                    ));
                }
                if parsed_event {
                    stream_protocol_seen = true;
                    fallback_response.clear();
                    fallback_truncated = false;
                }
            }
            Ok(Some(Err(error))) => {
                return Err(ModelError::new(format!(
                    "model stream failed while reading response: {error}"
                )))
            }
            Ok(None) => break,
            Err(_) => {
                if last_activity.elapsed() >= idle_timeout {
                    return Err(ModelError::new(format!(
                        "model stream timed out after {} seconds without receiving data",
                        idle_timeout.as_secs().max(1)
                    )));
                }
            }
        }
    }

    if !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending).into_owned();
        stream_protocol_seen |= apply_stream_line(
            &line,
            &mut answer,
            &mut streamed_tool_calls,
            &mut finish_reason,
            &mut usage,
            &mut filtered_on_delta,
        )?;
    }
    dsml_filter.finish(on_delta);
    finish_streaming_response(
        if stream_protocol_seen {
            String::new()
        } else {
            String::from_utf8_lossy(&fallback_response).into_owned()
        },
        fallback_truncated,
        answer,
        streamed_tool_calls,
        finish_reason,
        usage,
        model,
        base_url,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn consume_streaming_response(
    response: Response,
    model: &str,
    base_url: &str,
    idle_timeout: Duration,
    hard_timeout: Duration,
    deadline: Instant,
    on_delta: &mut impl FnMut(&str),
    should_cancel: &mut impl FnMut() -> bool,
) -> Result<ModelResponse, ModelError> {
    let status = response.status();
    if !status.is_success() {
        let body = collect_response_body(
            response,
            MAX_MODEL_RESPONSE_BYTES,
            deadline,
            hard_timeout,
            "model response",
            should_cancel,
        )
        .await?;
        let text = String::from_utf8_lossy(&body).into_owned();
        let provider_error = parse_provider_error(&text).unwrap_or_default();
        return Err(ModelError::with_status(
            status.as_u16(),
            if provider_error.trim().is_empty() {
                format!("model request failed with status {status}")
            } else {
                provider_error
            },
        ));
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("application/json") || content_type.contains("+json") {
        let body = collect_response_body(
            response,
            MAX_MODEL_RESPONSE_BYTES,
            deadline,
            hard_timeout,
            "model response",
            should_cancel,
        )
        .await?;
        return finish_streaming_response(
            String::from_utf8_lossy(&body).into_owned(),
            false,
            String::new(),
            BTreeMap::new(),
            None,
            Metadata::new(),
            model,
            base_url,
        );
    }

    consume_streaming_body(
        response.bytes_stream(),
        model,
        base_url,
        idle_timeout,
        hard_timeout,
        deadline,
        on_delta,
        should_cancel,
    )
    .await
}
