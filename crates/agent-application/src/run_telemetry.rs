use serde::{Deserialize, Serialize};
use std::fmt;

pub const RUN_TELEMETRY_SCHEMA: &str = "cindx.agent.run-telemetry.v1";

const MAX_RUN_TELEMETRY_JSON_BYTES: usize = 16 * 1024;
const MAX_RUN_TELEMETRY_LABEL_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTelemetryError(String);

impl RunTelemetryError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for RunTelemetryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RunTelemetryError {}

/// Delivery route a terminating run took. `None` marks a run that ended
/// without a delivery (failure or cancellation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTelemetryTerminalPathV1 {
    Direct,
    TerminalFinalizer,
    Fallback,
    None,
}

/// Bounded counters observed by the desktop run control at a terminal commit.
/// Token fields are settled strictly from provider-reported usage; the usage
/// source counters disclose how many physical attempts carried each
/// provenance class so estimated or unknown usage can never masquerade as
/// measured provider traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTelemetryObservationV1 {
    pub agent_run_id: String,
    pub session_id: String,
    pub effort: String,
    pub terminal_path: RunTelemetryTerminalPathV1,
    pub stop_reason: String,
    pub wall_ms: u64,
    pub model_calls: u64,
    pub model_wait_ms: u64,
    pub tool_calls: u64,
    pub tool_execution_ms: u64,
    pub agent_turns: u64,
    pub context_compactions: u64,
    pub rolling_summaries: u64,
    pub retrieval_ms: u64,
    pub retrieval_channels: u64,
    pub retrieval_channel_hits: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub usage_provider_attempts: u64,
    pub usage_provider_partial_attempts: u64,
    pub usage_estimated_attempts: u64,
    pub usage_unknown_attempts: u64,
}

/// Shadow-only performance telemetry receipt for one physical agent run,
/// appended to a private capped journal at the terminal commit. It has no
/// routing, prompt, memory, permission, or serving consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTelemetryReceiptV1 {
    pub schema: String,
    pub agent_run_id: String,
    pub session_id: String,
    pub effort: String,
    pub terminal_path: RunTelemetryTerminalPathV1,
    pub stop_reason: String,
    pub wall_ms: u64,
    pub model_calls: u64,
    pub model_wait_ms: u64,
    pub tool_calls: u64,
    pub tool_execution_ms: u64,
    pub agent_turns: u64,
    pub context_compactions: u64,
    pub rolling_summaries: u64,
    pub retrieval_ms: u64,
    pub retrieval_channels: u64,
    pub retrieval_channel_hits: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub usage_provider_attempts: u64,
    pub usage_provider_partial_attempts: u64,
    pub usage_estimated_attempts: u64,
    pub usage_unknown_attempts: u64,
}

impl RunTelemetryReceiptV1 {
    pub fn from_observation(
        observation: &RunTelemetryObservationV1,
    ) -> Result<Self, RunTelemetryError> {
        let receipt = Self {
            schema: RUN_TELEMETRY_SCHEMA.to_string(),
            agent_run_id: observation.agent_run_id.clone(),
            session_id: observation.session_id.clone(),
            effort: observation.effort.clone(),
            terminal_path: observation.terminal_path,
            stop_reason: observation.stop_reason.clone(),
            wall_ms: observation.wall_ms,
            model_calls: observation.model_calls,
            model_wait_ms: observation.model_wait_ms,
            tool_calls: observation.tool_calls,
            tool_execution_ms: observation.tool_execution_ms,
            agent_turns: observation.agent_turns,
            context_compactions: observation.context_compactions,
            rolling_summaries: observation.rolling_summaries,
            retrieval_ms: observation.retrieval_ms,
            retrieval_channels: observation.retrieval_channels,
            retrieval_channel_hits: observation.retrieval_channel_hits,
            prompt_tokens: observation.prompt_tokens,
            completion_tokens: observation.completion_tokens,
            usage_provider_attempts: observation.usage_provider_attempts,
            usage_provider_partial_attempts: observation.usage_provider_partial_attempts,
            usage_estimated_attempts: observation.usage_estimated_attempts,
            usage_unknown_attempts: observation.usage_unknown_attempts,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn from_json(encoded: &str) -> Result<Self, RunTelemetryError> {
        if encoded.len() > MAX_RUN_TELEMETRY_JSON_BYTES {
            return Err(RunTelemetryError::new(
                "run telemetry receipt JSON exceeds its size bound",
            ));
        }
        let receipt = serde_json::from_str::<Self>(encoded).map_err(|error| {
            RunTelemetryError::new(format!("run telemetry receipt JSON is invalid: {error}"))
        })?;
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn to_json(&self) -> Result<String, RunTelemetryError> {
        self.validate()?;
        let encoded = serde_json::to_string(self).map_err(|error| {
            RunTelemetryError::new(format!(
                "run telemetry receipt JSON encoding failed: {error}"
            ))
        })?;
        if encoded.len() > MAX_RUN_TELEMETRY_JSON_BYTES {
            return Err(RunTelemetryError::new(
                "run telemetry receipt JSON exceeds its size bound",
            ));
        }
        Ok(encoded)
    }

    pub fn validate(&self) -> Result<(), RunTelemetryError> {
        if self.schema != RUN_TELEMETRY_SCHEMA {
            return Err(RunTelemetryError::new(
                "run telemetry receipt schema is unsupported",
            ));
        }
        validate_label(&self.agent_run_id, "agent run id")?;
        validate_label(&self.session_id, "session id")?;
        validate_label(&self.effort, "effort")?;
        validate_label(&self.stop_reason, "stop reason")?;
        let provider_tokens = self.prompt_tokens.saturating_add(self.completion_tokens);
        if provider_tokens > 0 && self.usage_provider_attempts == 0 {
            return Err(RunTelemetryError::new(
                "run telemetry provider-observed tokens require a provider usage attempt",
            ));
        }
        Ok(())
    }
}

fn validate_label(value: &str, label: &str) -> Result<(), RunTelemetryError> {
    if value.trim().is_empty() {
        return Err(RunTelemetryError::new(format!(
            "run telemetry receipt {label} is missing"
        )));
    }
    if value.len() > MAX_RUN_TELEMETRY_LABEL_BYTES {
        return Err(RunTelemetryError::new(format!(
            "run telemetry receipt {label} exceeds its size bound"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> RunTelemetryObservationV1 {
        RunTelemetryObservationV1 {
            agent_run_id: "run-1".to_string(),
            session_id: "session-1".to_string(),
            effort: "default".to_string(),
            terminal_path: RunTelemetryTerminalPathV1::Direct,
            stop_reason: "completed".to_string(),
            wall_ms: 4_200,
            model_calls: 3,
            model_wait_ms: 3_100,
            tool_calls: 2,
            tool_execution_ms: 480,
            agent_turns: 3,
            context_compactions: 1,
            rolling_summaries: 2,
            retrieval_ms: 130,
            retrieval_channels: 3,
            retrieval_channel_hits: 9,
            prompt_tokens: 12_000,
            completion_tokens: 800,
            usage_provider_attempts: 3,
            usage_provider_partial_attempts: 0,
            usage_estimated_attempts: 0,
            usage_unknown_attempts: 0,
        }
    }

    #[test]
    fn run_telemetry_contract_schema_round_trip() {
        println!("{RUN_TELEMETRY_SCHEMA}");
        let receipt = RunTelemetryReceiptV1::from_observation(&observation()).expect("projects");
        assert_eq!(receipt.schema, RUN_TELEMETRY_SCHEMA);
        let encoded = receipt.to_json().expect("encodes");
        let decoded = RunTelemetryReceiptV1::from_json(&encoded).expect("decodes");
        assert_eq!(decoded, receipt);
    }

    #[test]
    fn run_telemetry_contract_terminal_paths_have_stable_wire_labels() {
        for (path, label) in [
            (RunTelemetryTerminalPathV1::Direct, "direct"),
            (
                RunTelemetryTerminalPathV1::TerminalFinalizer,
                "terminal_finalizer",
            ),
            (RunTelemetryTerminalPathV1::Fallback, "fallback"),
            (RunTelemetryTerminalPathV1::None, "none"),
        ] {
            let encoded = serde_json::to_value(path).expect("path encodes");
            assert_eq!(encoded, serde_json::Value::String(label.to_string()));
        }
    }

    #[test]
    fn run_telemetry_contract_fails_closed_on_missing_identity() {
        for (field, value) in [
            ("agent_run_id", ""),
            ("session_id", ""),
            ("effort", ""),
            ("stop_reason", "  "),
        ] {
            let mut drifted = observation();
            match field {
                "agent_run_id" => drifted.agent_run_id = value.to_string(),
                "session_id" => drifted.session_id = value.to_string(),
                "effort" => drifted.effort = value.to_string(),
                _ => drifted.stop_reason = value.to_string(),
            }
            assert!(
                RunTelemetryReceiptV1::from_observation(&drifted).is_err(),
                "empty {field} must fail closed"
            );
        }
    }

    #[test]
    fn run_telemetry_contract_rejects_schema_drift_and_unknown_fields() {
        let receipt = RunTelemetryReceiptV1::from_observation(&observation()).expect("projects");
        let encoded = receipt.to_json().expect("encodes");

        let mut drifted: serde_json::Value = serde_json::from_str(&encoded).expect("value");
        drifted["schema"] = serde_json::json!("cindx.agent.run-telemetry.v0");
        assert!(
            RunTelemetryReceiptV1::from_json(&serde_json::to_string(&drifted).unwrap()).is_err()
        );

        let mut extended: serde_json::Value = serde_json::from_str(&encoded).expect("value");
        extended["routing_hint"] = serde_json::json!("fast");
        assert!(
            RunTelemetryReceiptV1::from_json(&serde_json::to_string(&extended).unwrap()).is_err()
        );
    }

    #[test]
    fn run_telemetry_contract_provider_tokens_require_provider_attempts() {
        let mut drifted = observation();
        drifted.usage_provider_attempts = 0;
        assert!(RunTelemetryReceiptV1::from_observation(&drifted).is_err());

        let mut estimated_only = observation();
        estimated_only.prompt_tokens = 0;
        estimated_only.completion_tokens = 0;
        estimated_only.usage_provider_attempts = 0;
        estimated_only.usage_estimated_attempts = 3;
        let receipt = RunTelemetryReceiptV1::from_observation(&estimated_only)
            .expect("estimated usage projects");
        assert_eq!(receipt.prompt_tokens, 0);
        assert_eq!(receipt.usage_estimated_attempts, 3);
    }
}
