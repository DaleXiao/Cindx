use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use agent_core::{
    Message, MessageRole, Metadata, ModelCallMode, ModelRequest, ModelRole,
    GENERATION_TEMPERATURE_KEY,
};
use agent_runtime::{
    compaction_summary_instruction, serialize_transcript_for_compaction, AgentRunControl,
};
use model_provider::StreamingModelProvider;

use crate::agent_query_commands::agent_run_should_stop;

const MODEL_SUMMARY_MIN_MESSAGES: usize = 16;
const SUMMARY_TRANSCRIPT_MESSAGE_CAP_CHARS: usize = 600;
const SUMMARY_RESULT_CAP_CHARS: usize = 2400;
const SUMMARY_CACHE_MAX_ENTRIES: usize = 24;

fn summary_cache() -> &'static Mutex<HashMap<u64, String>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn transcript_fingerprint(messages: &[Message]) -> u64 {
    let mut hasher = DefaultHasher::new();
    messages.len().hash(&mut hasher);
    let mut total_chars = 0usize;
    for message in messages {
        total_chars = total_chars.saturating_add(message.content.chars().count());
    }
    total_chars.hash(&mut hasher);
    if let Some(first_user) = messages.iter().find(|message| message.role == MessageRole::User) {
        first_user.content.trim().hash(&mut hasher);
    }
    if let Some(last) = messages.last() {
        last.content.trim().hash(&mut hasher);
    }
    hasher.finish()
}

/// Builds the one-shot summarization request for a long transcript, or None when the
/// transcript is short enough that the deterministic extractive summary is preferred.
pub(crate) fn build_summary_request(messages: &[Message]) -> Option<ModelRequest> {
    if messages.len() < MODEL_SUMMARY_MIN_MESSAGES {
        return None;
    }
    let indices: Vec<usize> = (0..messages.len()).collect();
    let transcript = serialize_transcript_for_compaction(
        messages,
        &indices,
        SUMMARY_TRANSCRIPT_MESSAGE_CAP_CHARS,
    );
    if transcript.trim().is_empty() {
        return None;
    }
    let mut metadata = Metadata::new();
    metadata.insert(GENERATION_TEMPERATURE_KEY.to_string(), "0.2".to_string());
    Some(ModelRequest {
        role: ModelRole::Summarizer,
        messages: vec![
            Message {
                role: MessageRole::System,
                content: compaction_summary_instruction().to_string(),
                metadata: Metadata::new(),
            },
            Message {
                role: MessageRole::User,
                content: transcript,
                metadata: Metadata::new(),
            },
        ],
        tools: Vec::new(),
        mode: ModelCallMode::Streaming,
        metadata,
    })
}

/// Model-generated rolling summary of a long transcript, cached by transcript
/// fingerprint. Returns None when the transcript is short, the run is cancelled, or
/// the provider call fails — callers fall back to the extractive summary.
pub(crate) fn model_rolling_summary(
    provider: &dyn StreamingModelProvider,
    messages: &[Message],
    cancellation: &Arc<AgentRunControl>,
) -> Option<String> {
    let fingerprint = transcript_fingerprint(messages);
    if let Ok(cache) = summary_cache().lock() {
        if let Some(hit) = cache.get(&fingerprint) {
            return Some(hit.clone());
        }
    }
    let request = build_summary_request(messages)?;
    let mut should_cancel = || agent_run_should_stop(cancellation);
    let mut ignore_delta = |_delta: &str| {};
    let response = provider
        .complete_streaming_cancellable(request, &mut ignore_delta, &mut should_cancel)
        .ok()?;
    let summary: String = response
        .message
        .content
        .trim()
        .chars()
        .take(SUMMARY_RESULT_CAP_CHARS)
        .collect();
    if summary.is_empty() {
        return None;
    }
    if let Ok(mut cache) = summary_cache().lock() {
        if cache.len() >= SUMMARY_CACHE_MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(fingerprint, summary.clone());
    }
    Some(summary)
}

/// Fraction of the context window beyond which a transcript is worth a
/// model-generated summary. Below this the cheap extractive summary is used, so
/// routine runs never pay a blocking summarization call at startup.
const MODEL_SUMMARY_CONTEXT_FRACTION_PERCENT: u64 = 40;

/// Rolling summary for a run. Only transcripts already pressing on the context
/// window pay the model-generated summary (cached by fingerprint); everything
/// else uses the deterministic extractive summary, so the common path is cheap.
pub(crate) fn rolling_summary_for_run(
    provider: &dyn StreamingModelProvider,
    messages: &[Message],
    cancellation: &Arc<AgentRunControl>,
    context_window_tokens: u64,
) -> Option<String> {
    if context_pressure_warrants_model_summary(messages, context_window_tokens) {
        if let Some(summary) = model_rolling_summary(provider, messages, cancellation) {
            return Some(summary);
        }
    }
    agent_runtime::extractive_rolling_summary(messages, 1200)
}

fn context_pressure_warrants_model_summary(messages: &[Message], context_window_tokens: u64) -> bool {
    if context_window_tokens == 0 {
        return false;
    }
    let used = agent_runtime::estimate_context_tokens(messages);
    used.saturating_mul(100) >= context_window_tokens * MODEL_SUMMARY_CONTEXT_FRACTION_PERCENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata: Metadata::new(),
        }
    }

    fn long_transcript(count: usize) -> Vec<Message> {
        let mut messages = Vec::new();
        for index in 0..count {
            let role = if index % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            messages.push(message(role, &format!("turn {index} content")));
        }
        messages
    }

    #[test]
    fn short_transcript_prefers_extractive_summary() {
        assert!(build_summary_request(&long_transcript(10)).is_none());
    }

    #[test]
    fn long_transcript_builds_a_summarizer_request() {
        let request = build_summary_request(&long_transcript(20)).expect("request");
        assert!(matches!(request.role, ModelRole::Summarizer));
        assert!(request.tools.is_empty());
        assert_eq!(request.messages.len(), 2);
        assert!(matches!(request.messages[0].role, MessageRole::System));
        assert!(request.messages[0].content.contains("## Goal"));
        assert!(request.messages[1].content.contains("[user] turn 0 content"));
        assert_eq!(
            request.metadata.get(GENERATION_TEMPERATURE_KEY).map(String::as_str),
            Some("0.2")
        );
    }

    #[test]
    fn model_summary_only_when_context_pressure_is_high() {
        let small = long_transcript(20);
        assert!(!context_pressure_warrants_model_summary(&small, 1_000_000));
        assert!(!context_pressure_warrants_model_summary(&small, 0));

        let heavy: Vec<Message> = (0..40)
            .map(|index| message(MessageRole::User, &format!("bulk {index} {}", "x".repeat(400))))
            .collect();
        assert!(context_pressure_warrants_model_summary(&heavy, 100));
    }

    #[test]
    fn fingerprint_is_stable_and_changes_with_content() {
        let base = long_transcript(20);
        let same = long_transcript(20);
        assert_eq!(transcript_fingerprint(&base), transcript_fingerprint(&same));
        let mut changed = long_transcript(20);
        changed.push(message(MessageRole::Assistant, "an extra turn"));
        assert_ne!(transcript_fingerprint(&base), transcript_fingerprint(&changed));
    }
}
