use crate::event_projection::write_private_file_atomically;
use crate::persistence_runtime::app_data_root;
use agent_core::Metadata;
use agent_runtime::AgentRunControl;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const RUN_TELEMETRY_JOURNAL_CAPACITY: usize = 256;
const RUN_TELEMETRY_JOURNAL_FILE: &str = "run-telemetry.journal.jsonl";

pub(crate) fn run_telemetry_journal_path() -> PathBuf {
    run_telemetry_journal_path_for(&app_data_root())
}

pub(crate) fn run_telemetry_journal_path_for(data_root: &Path) -> PathBuf {
    data_root.join(RUN_TELEMETRY_JOURNAL_FILE)
}

/// Terminal facts available at the commit point: the run context supplies
/// identity and effort labels, the run control supplies the counters.
pub(crate) struct RunTelemetryTerminalFacts<'a> {
    pub(crate) run_context: &'a Metadata,
    pub(crate) control: &'a AgentRunControl,
    pub(crate) terminal_path: agent_application::RunTelemetryTerminalPathV1,
    pub(crate) stop_reason: &'a str,
}

/// Appends the run's telemetry receipt to the private capped journal.
/// Best-effort shadow measurement: recording failures never disturb delivery.
pub(crate) fn record_run_telemetry_terminal(facts: RunTelemetryTerminalFacts<'_>) {
    if let Err(error) = record_run_telemetry_terminal_to(&run_telemetry_journal_path(), &facts) {
        eprintln!("run telemetry journal unavailable: {error}");
    }
}

pub(crate) fn record_run_telemetry_terminal_to(
    journal_path: &Path,
    facts: &RunTelemetryTerminalFacts<'_>,
) -> Result<agent_application::RunTelemetryReceiptV1, String> {
    let receipt = project_run_telemetry_receipt(facts)?;
    append_run_telemetry_receipt(journal_path, &receipt)?;
    Ok(receipt)
}

pub(crate) fn project_run_telemetry_receipt(
    facts: &RunTelemetryTerminalFacts<'_>,
) -> Result<agent_application::RunTelemetryReceiptV1, String> {
    let progress = facts.control.progress();
    let resources = facts.control.resource_usage();
    let usage = &resources.segment;
    let observation = agent_application::RunTelemetryObservationV1 {
        agent_run_id: facts
            .run_context
            .get("agent_run_id")
            .cloned()
            .unwrap_or_default(),
        session_id: facts
            .run_context
            .get("session_id")
            .cloned()
            .unwrap_or_default(),
        effort: facts
            .run_context
            .get("agent_effort")
            .cloned()
            .unwrap_or_default(),
        terminal_path: facts.terminal_path,
        stop_reason: facts.stop_reason.to_string(),
        wall_ms: progress.elapsed.as_millis() as u64,
        model_calls: progress.model_calls as u64,
        model_wait_ms: progress.telemetry.model_wait_ms,
        tool_calls: progress.tool_calls as u64,
        tool_execution_ms: progress.telemetry.tool_execution_ms,
        agent_turns: progress.agent_turns as u64,
        context_compactions: progress.telemetry.context_compactions,
        rolling_summaries: progress.telemetry.rolling_summaries,
        retrieval_ms: progress.telemetry.retrieval_ms,
        retrieval_channels: progress.telemetry.retrieval_channels,
        retrieval_channel_hits: progress.telemetry.retrieval_channel_hits,
        prompt_tokens: usage.provider_prompt_tokens,
        completion_tokens: usage.provider_completion_tokens,
        usage_provider_attempts: usage.usage_sources.provider,
        usage_provider_partial_attempts: usage.usage_sources.provider_partial,
        usage_estimated_attempts: usage.usage_sources.estimated,
        usage_unknown_attempts: usage.usage_sources.unknown,
    };
    agent_application::RunTelemetryReceiptV1::from_observation(&observation)
        .map_err(|error| error.to_string())
}

fn append_run_telemetry_receipt(
    path: &Path,
    receipt: &agent_application::RunTelemetryReceiptV1,
) -> Result<(), String> {
    let mut lines = read_run_telemetry_journal_lines(path);
    let encoded = receipt.to_json().map_err(|error| error.to_string())?;
    lines.push(encoded);
    if lines.len() > RUN_TELEMETRY_JOURNAL_CAPACITY {
        let excess = lines.len() - RUN_TELEMETRY_JOURNAL_CAPACITY;
        lines.drain(..excess);
    }
    let mut payload = lines.join("\n");
    payload.push('\n');
    write_private_file_atomically(path, payload.as_bytes(), "run telemetry journal")
}

fn read_run_telemetry_journal_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|content| {
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads the telemetry journal back. Test-only: production records the
/// journal but never reads it (inert measurement plumbing).
#[cfg(test)]
pub(crate) fn load_run_telemetry_receipts(
    path: &Path,
) -> Result<Vec<agent_application::RunTelemetryReceiptV1>, String> {
    let mut receipts = Vec::new();
    for line in read_run_telemetry_journal_lines(path) {
        let receipt = agent_application::RunTelemetryReceiptV1::from_json(&line)
            .map_err(|error| error.to_string())?;
        receipts.push(receipt);
    }
    Ok(receipts)
}
