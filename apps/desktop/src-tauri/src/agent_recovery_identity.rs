use crate::agent_read_model::{
    agent_recovery_prompt_from_active_events, primary_agent_user_turn_event,
};
use crate::{app_state::AgentRecoveryEnvelope, runtime_constants::AGENT_RECOVERY_SCHEMA};
use agent_application::{AgentRecoveryIdentity, ResolvedAgentRecovery};
use agent_core::{
    agent_run_id, AgentRunIdentity, AgentRunLineage, Event, Metadata,
    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_IDENTITY_V1_SCHEMA,
    AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use orchestrator::sha256_hex;

pub(super) fn resolve_agent_recovery_identity(
    events: &[Event],
    run_context: &Metadata,
) -> Option<ResolvedAgentRecovery> {
    let session_id = run_context.get("session_id")?.clone();
    let run_start = events
        .iter()
        .find(|event| crate::agent_read_model::is_agent_run_start_event(event));
    let source_run_id = run_start
        .and_then(|event| agent_run_id(&event.metadata))
        .map(str::to_string)
        .or_else(|| agent_run_id(run_context).map(str::to_string))
        .unwrap_or_default();
    let logical_run_id = match run_start
        .map(|event| AgentRunIdentity::from_metadata(&event.metadata))
        .transpose()
        .ok()?
        .flatten()
    {
        Some(identity) => identity.logical_run_id().to_string(),
        None => match AgentRunIdentity::from_metadata(run_context).ok()? {
            Some(identity) => identity.logical_run_id().to_string(),
            None => source_run_id.clone(),
        },
    };
    let user_turn_sequence = primary_agent_user_turn_event(events)
        .map(|event| event.sequence)
        .unwrap_or_default();
    let prompt = agent_recovery_prompt_from_active_events(events)?;
    let prompt_fingerprint = sha256_hex(prompt.as_bytes());
    let project_id = run_context.get("project_id").cloned();
    let project_scope = project_id.as_deref().unwrap_or_default();
    let resume_key = format!(
        "agent-resume-{}",
        &sha256_hex(
            format!(
                "{project_scope}\n{session_id}\n{source_run_id}\n{user_turn_sequence}\n{prompt_fingerprint}"
            )
            .as_bytes()
        )[..24]
    );
    let resolved = ResolvedAgentRecovery {
        identity: AgentRecoveryIdentity {
            project_id,
            session_id,
            resume_key,
            source_run_id,
            logical_run_id: Some(logical_run_id),
            user_turn_sequence,
            prompt_fingerprint,
        },
        prompt,
    };
    resolved.validate().is_ok().then_some(resolved)
}

fn legacy_logical_run_id(events: &[Event], source_run_id: &str) -> Option<String> {
    AgentRunLineage::from_events(events)
        .ok()?
        .logical_run_id_for_attempt(source_run_id)
        .map(str::to_string)
}

pub(super) fn recovery_envelope_matches_active_turn(
    envelope: &AgentRecoveryEnvelope,
    events: &[Event],
    run_context: &Metadata,
) -> bool {
    let Some(active) = resolve_agent_recovery_identity(events, run_context) else {
        return false;
    };
    envelope.schema == AGENT_RECOVERY_SCHEMA && envelope.identity.matches(&active.identity)
}

pub(super) fn enrich_legacy_recovery_envelope(
    events: &[Event],
    envelope: &mut AgentRecoveryEnvelope,
) {
    if envelope.identity.logical_run_id.is_none() {
        envelope.identity.logical_run_id =
            legacy_logical_run_id(events, &envelope.identity.source_run_id);
    }
}

pub(super) fn enrich_legacy_run_context(events: &[Event], run_context: &mut Metadata) {
    if run_context.contains_key(LOGICAL_AGENT_RUN_ID_METADATA_KEY) {
        return;
    }
    let Some(attempt_run_id) = run_context.get(AGENT_RUN_ID_METADATA_KEY).cloned() else {
        return;
    };
    let logical_run_id =
        legacy_logical_run_id(events, &attempt_run_id).unwrap_or_else(|| attempt_run_id.clone());
    run_context.insert(
        AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
        AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
    );
    run_context.insert(
        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
        logical_run_id,
    );
}
