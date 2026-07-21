use agent_runtime::{AgentRunControl, RunStopReason};
use model_provider::ModelResponse;
use std::time::{Duration, Instant};

pub(crate) fn is_transient_model_transport_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "broken pipe",
        "timed out",
        "timeout",
        "connection reset",
        "connection refused",
        "empty reply",
        "temporarily unavailable",
        "service unavailable",
        "too many requests",
        "rate limit",
        "status 429",
        "status 500",
        "status 502",
        "status 503",
        "status 504",
        "failed to start curl",
        "failed to configure curl",
        "failed to wait for curl",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

pub(crate) fn exhausted_model_transport_stop_reason(message: &str) -> Option<RunStopReason> {
    is_transient_model_transport_error(message).then_some(RunStopReason::ProviderUnavailable)
}

pub(crate) fn model_response_checkpoint_evidence(response: &ModelResponse) -> String {
    let mut evidence = response.message.content.clone();
    for call in &response.tool_calls {
        evidence.push('\n');
        evidence.push_str(&call.name);
        evidence.push(':');
        evidence.push_str(&call.arguments_json);
    }
    evidence
}

pub(crate) struct ModelStreamProgress {
    last_progress_at: Instant,
    last_snapshot_bytes: usize,
}

impl ModelStreamProgress {
    pub(crate) fn new() -> Self {
        Self {
            last_progress_at: Instant::now(),
            last_snapshot_bytes: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.last_progress_at = Instant::now();
        self.last_snapshot_bytes = 0;
    }

    pub(crate) fn observe(
        &mut self,
        control: &AgentRunControl,
        stage: &str,
        detail: &str,
        partial_output: &str,
    ) {
        let snapshot_due = partial_output
            .len()
            .saturating_sub(self.last_snapshot_bytes)
            >= 4 * 1024;
        if snapshot_due {
            control.record_partial_output(partial_output);
            self.last_snapshot_bytes = partial_output.len();
        }
        if snapshot_due || self.last_progress_at.elapsed() >= Duration::from_millis(500) {
            control.mark_progress(stage, detail);
            self.last_progress_at = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_retry_classification_is_narrow_and_stable() {
        assert!(is_transient_model_transport_error("curl: Broken pipe"));
        assert!(is_transient_model_transport_error("HTTP status 503"));
        assert!(!is_transient_model_transport_error(
            "400 invalid messages input"
        ));
    }
}
