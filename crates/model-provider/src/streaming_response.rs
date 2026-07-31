use super::{
    collect_response_body, parse_model_response, parse_provider_error, ModelError, ModelResponse,
    HTTP_POLL_INTERVAL, MAX_MODEL_RESPONSE_BYTES, MODEL_REQUEST_CANCELLED,
};
use crate::response_parser::{normalize_dsml_tool_calls, serialize_tool_calls};
use crate::stream_delta_aggregator::FilteredStreamDeltaEmitter;
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

struct StreamingLineDecoder {
    pending: Vec<u8>,
    #[cfg(test)]
    scanned_bytes: usize,
}

impl StreamingLineDecoder {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            #[cfg(test)]
            scanned_bytes: 0,
        }
    }

    fn append_fragment(&mut self, fragment: &[u8]) -> Result<(), ModelError> {
        if fragment.len() > MAX_STREAM_EVENT_BYTES.saturating_sub(self.pending.len()) {
            return Err(ModelError::new(
                "model stream event exceeded the 8 MB line limit",
            ));
        }
        self.pending.extend_from_slice(fragment);
        Ok(())
    }

    fn push_chunk(
        &mut self,
        chunk: &[u8],
        mut apply_line: impl FnMut(&str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        let mut offset = 0;
        let mut parsed_event = false;
        while offset < chunk.len() {
            let remaining = &chunk[offset..];
            let Some(newline) = remaining.iter().position(|byte| *byte == b'\n') else {
                #[cfg(test)]
                {
                    self.scanned_bytes = self.scanned_bytes.saturating_add(remaining.len());
                }
                self.append_fragment(remaining)?;
                break;
            };
            #[cfg(test)]
            {
                self.scanned_bytes = self.scanned_bytes.saturating_add(newline + 1);
            }
            self.append_fragment(&remaining[..newline])?;
            let line = String::from_utf8_lossy(&self.pending);
            parsed_event |= apply_line(&line)?;
            self.pending.clear();
            offset = offset.saturating_add(newline + 1);
        }
        Ok(parsed_event)
    }

    fn finish(
        &mut self,
        mut apply_line: impl FnMut(&str) -> Result<bool, ModelError>,
    ) -> Result<bool, ModelError> {
        if self.pending.is_empty() {
            return Ok(false);
        }
        let line = String::from_utf8_lossy(&self.pending);
        let parsed_event = apply_line(&line)?;
        self.pending.clear();
        Ok(parsed_event)
    }
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
    let mut line_decoder = StreamingLineDecoder::new();
    let mut answer = String::new();
    let mut streamed_tool_calls = BTreeMap::<usize, StreamingToolCall>::new();
    let mut finish_reason = None;
    let mut usage = Metadata::new();
    let mut last_activity = Instant::now();
    let mut delta_stream = FilteredStreamDeltaEmitter::new(on_delta);

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
        let poll_interval = delta_stream.before_poll(Instant::now(), HTTP_POLL_INTERVAL);
        match tokio::time::timeout(poll_interval, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                last_activity = Instant::now();
                let chunk = chunk.as_ref();
                received_bytes = received_bytes.saturating_add(chunk.len());
                if received_bytes > MAX_MODEL_RESPONSE_BYTES {
                    return Err(ModelError::new("model stream exceeded 64 MB"));
                }
                if !stream_protocol_seen {
                    let remaining =
                        MAX_STREAMING_FALLBACK_BYTES.saturating_sub(fallback_response.len());
                    fallback_response.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                    fallback_truncated |= chunk.len() > remaining;
                }
                let parsed_event = line_decoder.push_chunk(chunk, |line| {
                    apply_stream_line(
                        line,
                        &mut answer,
                        &mut streamed_tool_calls,
                        &mut finish_reason,
                        &mut usage,
                        &mut |delta| delta_stream.push(delta, Instant::now()),
                    )
                })?;
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

    stream_protocol_seen |= line_decoder.finish(|line| {
        apply_stream_line(
            line,
            &mut answer,
            &mut streamed_tool_calls,
            &mut finish_reason,
            &mut usage,
            &mut |delta| delta_stream.push(delta, Instant::now()),
        )
    })?;
    delta_stream.finish(Instant::now());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn consume_immediate_test_stream(
        chunks: Vec<Result<&'static str, ModelError>>,
        on_delta: &mut impl FnMut(&str),
        should_cancel: &mut impl FnMut() -> bool,
    ) -> Result<ModelResponse, ModelError> {
        let hard_timeout = Duration::from_secs(1);
        crate::run_http(consume_streaming_body(
            futures_util::stream::iter(
                chunks
                    .into_iter()
                    .map(|chunk| chunk.map(|value| value.as_bytes().to_vec())),
            ),
            "test-model",
            "http://example.test/v1",
            Duration::from_secs(1),
            hard_timeout,
            Instant::now() + hard_timeout,
            on_delta,
            should_cancel,
        ))
    }

    #[test]
    fn streaming_body_batches_callbacks_and_flushes_before_success() {
        let mut emitted = Vec::new();
        let response = consume_immediate_test_stream(
            vec![
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n"),
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n"),
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"c\"}}]}\n"),
            ],
            &mut |delta| emitted.push(delta.to_string()),
            &mut || false,
        )
        .expect("stream should complete");

        assert_eq!(response.message.content, "abc");
        assert_eq!(emitted, ["a", "bc"]);
    }

    #[test]
    fn streaming_body_flushes_safe_text_before_read_error() {
        let mut emitted = Vec::new();
        let error = consume_immediate_test_stream(
            vec![
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n"),
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n"),
                Err(ModelError::new("wire failed")),
            ],
            &mut |delta| emitted.push(delta.to_string()),
            &mut || false,
        )
        .expect_err("stream should surface the read error");

        assert!(error.message.contains("wire failed"));
        assert_eq!(emitted, ["a", "b"]);
    }

    #[test]
    fn streaming_body_flushes_safe_text_before_cancellation() {
        let mut emitted = Vec::new();
        let mut cancellation_checks = 0;
        let error = consume_immediate_test_stream(
            vec![
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n"),
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n"),
            ],
            &mut |delta| emitted.push(delta.to_string()),
            &mut || {
                cancellation_checks += 1;
                cancellation_checks >= 3
            },
        )
        .expect_err("stream should cancel before polling again");

        assert_eq!(error.message, MODEL_REQUEST_CANCELLED);
        assert_eq!(emitted, ["a", "b"]);
    }

    #[test]
    fn line_decoder_scans_an_eight_mib_event_once_across_one_kib_chunks() {
        let mut decoder = StreamingLineDecoder::new();
        let chunk = [b'x'; 1024];
        let mut completed_lines = 0;

        for _ in 0..(MAX_STREAM_EVENT_BYTES / chunk.len()) {
            decoder
                .push_chunk(&chunk, |_| {
                    completed_lines += 1;
                    Ok(false)
                })
                .expect("an event at the configured limit should remain valid");
        }
        decoder
            .push_chunk(b"\n", |_| {
                completed_lines += 1;
                Ok(false)
            })
            .expect("the line terminator should complete the bounded event");

        assert_eq!(decoder.scanned_bytes, MAX_STREAM_EVENT_BYTES + 1);
        assert_eq!(completed_lines, 1);
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn line_decoder_rejects_an_oversized_event_before_appending_it() {
        let mut decoder = StreamingLineDecoder::new();
        let chunk = [b'x'; 1024];
        for _ in 0..(MAX_STREAM_EVENT_BYTES / chunk.len()) {
            decoder
                .push_chunk(&chunk, |_| Ok(false))
                .expect("an event exactly at the limit should remain valid");
        }

        let error = decoder
            .push_chunk(b"x\n", |_| panic!("an oversized line must not be parsed"))
            .expect_err("the event should fail before the extra byte is appended");

        assert_eq!(
            error.message,
            "model stream event exceeded the 8 MB line limit"
        );
        assert_eq!(decoder.pending.len(), MAX_STREAM_EVENT_BYTES);
    }

    #[test]
    fn line_decoder_preserves_crlf_and_eof_stream_semantics_across_byte_chunks() {
        let wire = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\r\n",
            "\r\n",
            "data: {\"choices\":[],\"usage\":{\"total_tokens\":7}}\r\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"file_read\",\"arguments\":\"{\\\"input\\\":\\\"path=\"}}]}}]}\r\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"README.md\\\"}\"}}]}}]}"
        );
        let mut decoder = StreamingLineDecoder::new();
        let mut answer = String::new();
        let mut visible = String::new();
        let mut tool_calls = BTreeMap::new();
        let mut finish_reason = None;
        let mut usage = Metadata::new();
        let mut parsed_event = false;

        for byte in wire.as_bytes().chunks(1) {
            parsed_event |= decoder
                .push_chunk(byte, |line| {
                    apply_stream_line(
                        line,
                        &mut answer,
                        &mut tool_calls,
                        &mut finish_reason,
                        &mut usage,
                        &mut |delta| visible.push_str(delta),
                    )
                })
                .expect("split stream line should parse");
        }
        parsed_event |= decoder
            .finish(|line| {
                apply_stream_line(
                    line,
                    &mut answer,
                    &mut tool_calls,
                    &mut finish_reason,
                    &mut usage,
                    &mut |delta| visible.push_str(delta),
                )
            })
            .expect("unterminated final data line should preserve EOF behavior");

        let call = tool_calls
            .remove(&0)
            .and_then(|call| call.finish(0))
            .expect("split tool call should be complete");

        assert!(parsed_event);
        assert_eq!(decoder.scanned_bytes, wire.len());
        assert_eq!(answer, "hello");
        assert_eq!(visible, "hello");
        assert_eq!(usage.get("total_tokens").map(String::as_str), Some("7"));
        assert_eq!(call.id, "call_1");
        assert_eq!(call.name, "file_read");
        assert_eq!(call.arguments_json, r#"{"input":"path=README.md"}"#);
        assert!(finish_reason.is_none());
    }
}
