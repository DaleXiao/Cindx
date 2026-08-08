use crate::control::{AgentRunControl, RunStopReason};
use agent_core::{ModelError, ModelResponse, ModelResponseDisposition};
use std::time::{Duration, Instant};

pub fn exhausted_model_transport_error_stop_reason(error: &ModelError) -> Option<RunStopReason> {
    error
        .is_retryable()
        .then_some(RunStopReason::ProviderUnavailable)
}

pub fn model_response_checkpoint_evidence(response: &ModelResponse) -> Option<String> {
    if !matches!(
        response.assessment().disposition,
        ModelResponseDisposition::Usable | ModelResponseDisposition::ToolCalls
    ) {
        return None;
    }
    let mut evidence = response.message.content.clone();
    for call in &response.tool_calls {
        evidence.push('\n');
        evidence.push_str(&call.name);
        evidence.push(':');
        evidence.push_str(&call.arguments_json);
    }
    (!evidence.trim().is_empty()).then_some(evidence)
}

pub fn model_transport_retry_delay(attempt: usize) -> Duration {
    let exponent = attempt.saturating_sub(1).min(3) as u32;
    Duration::from_millis(500_u64.saturating_mul(2_u64.pow(exponent)))
}

pub struct ModelStreamProgress {
    last_progress_at: Instant,
    last_snapshot_bytes: usize,
}

impl Default for ModelStreamProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelStreamProgress {
    pub fn new() -> Self {
        Self {
            last_progress_at: Instant::now(),
            last_snapshot_bytes: 0,
        }
    }

    pub fn reset(&mut self) {
        self.last_progress_at = Instant::now();
        self.last_snapshot_bytes = 0;
    }

    pub fn observe(
        &mut self,
        control: &AgentRunControl,
        objective_epoch: u64,
        stage: &str,
        detail: &str,
        partial_output: &str,
    ) {
        let snapshot_due = partial_output
            .len()
            .saturating_sub(self.last_snapshot_bytes)
            >= 4 * 1024;
        if snapshot_due {
            control.record_partial_output_at(objective_epoch, partial_output);
            self.last_snapshot_bytes = partial_output.len();
        }
        if snapshot_due || self.last_progress_at.elapsed() >= Duration::from_millis(500) {
            control.mark_progress_at(objective_epoch, stage, detail);
            self.last_progress_at = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_retry_classification_is_narrow_and_stable() {
        assert!(ModelError::new("curl: Broken pipe").is_retryable());
        assert!(ModelError::new("HTTP status 503").is_retryable());
        assert!(!ModelError::new("400 invalid messages input").is_retryable());
    }

    #[test]
    fn transport_retry_delay_is_bounded_exponential_backoff() {
        assert_eq!(model_transport_retry_delay(1), Duration::from_millis(500));
        assert_eq!(model_transport_retry_delay(2), Duration::from_secs(1));
        assert_eq!(model_transport_retry_delay(3), Duration::from_secs(2));
        assert_eq!(model_transport_retry_delay(99), Duration::from_secs(4));
    }

    #[test]
    fn empty_model_responses_do_not_create_progress_evidence() {
        let response = ModelResponse {
            message: agent_core::Message {
                role: agent_core::MessageRole::Assistant,
                content: "  ".to_string(),
                metadata: Default::default(),
            },
            tool_calls: Vec::new(),
            metadata: Default::default(),
            raw_tool_calls_json: None,
        };

        assert_eq!(model_response_checkpoint_evidence(&response), None);
    }
}
