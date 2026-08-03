use crate::agent_read_model::{
    agent_recovery_prompt_from_active_events, primary_agent_user_turn_event,
};
use agent_application::{AgentRecoveryIdentity, ResolvedAgentRecovery};
use agent_core::{Event, Metadata};
use orchestrator::sha256_hex;

pub(super) fn resolve_agent_recovery_identity(
    events: &[Event],
    run_context: &Metadata,
) -> Option<ResolvedAgentRecovery> {
    let session_id = run_context.get("session_id")?.clone();
    let source_run_id = events
        .iter()
        .find(|event| crate::agent_read_model::is_agent_run_start_event(event))
        .and_then(|event| event.metadata.get("agent_run_id"))
        .cloned()
        .or_else(|| run_context.get("agent_run_id").cloned())
        .unwrap_or_default();
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
            user_turn_sequence,
            prompt_fingerprint,
        },
        prompt,
    };
    resolved.validate().is_ok().then_some(resolved)
}
