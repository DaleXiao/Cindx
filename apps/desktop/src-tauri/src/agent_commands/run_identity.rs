use crate::runtime_values::unique_id;
use agent_core::{
    agent_run_id, AgentRunIdentity, AgentRunLineage, Event, Metadata,
    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_ID_METADATA_KEY,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY, SOURCE_AGENT_RUN_ID_METADATA_KEY,
};

fn replace_agent_run_identity(
    run_context: &mut Metadata,
    identity: &AgentRunIdentity,
) -> Result<(), String> {
    for key in [
        AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY,
        LOGICAL_AGENT_RUN_ID_METADATA_KEY,
        AGENT_RUN_ID_METADATA_KEY,
        SOURCE_AGENT_RUN_ID_METADATA_KEY,
    ] {
        run_context.remove(key);
    }
    identity
        .insert_into(run_context)
        .map_err(|error| format!("invalid agent run identity: {error}"))
}

pub(super) fn assign_initial_agent_run_identity(run_context: &mut Metadata) -> Result<(), String> {
    let attempt_run_id = unique_id("agent-run");
    let identity = AgentRunIdentity::new(attempt_run_id.clone(), attempt_run_id)
        .map_err(|error| format!("invalid agent run identity: {error}"))?;
    replace_agent_run_identity(run_context, &identity)
}

pub(super) fn assign_continuation_agent_run_identity(
    run_context: &mut Metadata,
    logical_run_id: &str,
    source_attempt_run_id: &str,
) -> Result<(), String> {
    let identity = AgentRunIdentity::continuation(
        logical_run_id,
        unique_id("agent-run"),
        source_attempt_run_id,
    )
    .map_err(|error| format!("invalid agent run identity: {error}"))?;
    replace_agent_run_identity(run_context, &identity)
}

pub(super) fn inherited_agent_run_identity(
    run_context: &Metadata,
) -> Result<Option<(String, String)>, String> {
    match AgentRunIdentity::from_metadata(run_context)
        .map_err(|error| format!("invalid agent run identity: {error}"))?
    {
        Some(identity) => Ok(Some((
            identity.logical_run_id().to_string(),
            identity.attempt_run_id().to_string(),
        ))),
        None => Ok(agent_run_id(run_context)
            .map(|attempt_run_id| (attempt_run_id.to_string(), attempt_run_id.to_string()))),
    }
}

pub(super) fn inherited_agent_run_identity_from_events(
    events: &[Event],
    run_context: &Metadata,
) -> Result<Option<(String, String)>, String> {
    if let Some(identity) = AgentRunIdentity::from_metadata(run_context)
        .map_err(|error| format!("invalid agent run identity: {error}"))?
    {
        return Ok(Some((
            identity.logical_run_id().to_string(),
            identity.attempt_run_id().to_string(),
        )));
    }
    let Some(attempt_run_id) = agent_run_id(run_context).map(str::to_string) else {
        return Ok(None);
    };
    let logical_run_id = AgentRunLineage::from_events(events)
        .ok()
        .and_then(|lineage| {
            lineage
                .logical_run_id_for_attempt(&attempt_run_id)
                .map(str::to_string)
        })
        .unwrap_or_else(|| attempt_run_id.clone());
    Ok(Some((logical_run_id, attempt_run_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent_read_model::agent_session_events, runtime_values::phase16_task_id};
    use agent_core::{EventId, EventKind};

    #[test]
    fn initial_attempt_uses_the_same_logical_and_physical_run_id() {
        let mut context = Metadata::new();
        assign_initial_agent_run_identity(&mut context).expect("identity should be assigned");

        let identity = AgentRunIdentity::from_metadata(&context)
            .expect("identity should decode")
            .expect("versioned identity should exist");
        assert_eq!(identity.logical_run_id(), identity.attempt_run_id());
        assert_eq!(identity.source_attempt_run_id(), None);
    }

    #[test]
    fn continuation_preserves_logical_run_and_replaces_physical_attempt() {
        let mut context = Metadata::new();
        assign_initial_agent_run_identity(&mut context).expect("identity should be assigned");
        let initial = AgentRunIdentity::from_metadata(&context)
            .expect("identity should decode")
            .expect("versioned identity should exist");
        let logical_run_id = initial.logical_run_id().to_string();
        let source_attempt_run_id = initial.attempt_run_id().to_string();

        assign_continuation_agent_run_identity(
            &mut context,
            &logical_run_id,
            &source_attempt_run_id,
        )
        .expect("continuation identity should be assigned");

        let continuation = AgentRunIdentity::from_metadata(&context)
            .expect("continuation identity should decode")
            .expect("versioned continuation identity should exist");
        assert_eq!(continuation.logical_run_id(), logical_run_id);
        assert_ne!(continuation.attempt_run_id(), source_attempt_run_id);
        assert_eq!(
            continuation.source_attempt_run_id(),
            Some(source_attempt_run_id.as_str())
        );

        let second_source_attempt_run_id = continuation.attempt_run_id().to_string();
        assign_continuation_agent_run_identity(
            &mut context,
            &logical_run_id,
            &second_source_attempt_run_id,
        )
        .expect("second continuation identity should be assigned");
        let second_continuation = AgentRunIdentity::from_metadata(&context)
            .expect("second continuation identity should decode")
            .expect("versioned second continuation identity should exist");
        assert_eq!(second_continuation.logical_run_id(), logical_run_id);
        assert_ne!(
            second_continuation.attempt_run_id(),
            second_source_attempt_run_id
        );
        assert_ne!(second_continuation.attempt_run_id(), source_attempt_run_id);
        assert_eq!(
            second_continuation.source_attempt_run_id(),
            Some(second_source_attempt_run_id.as_str())
        );
    }

    #[test]
    fn legacy_multihop_retry_without_envelope_inherits_the_root_logical_run() {
        let legacy_context = |attempt_run_id: &str, source_attempt_run_id: Option<&str>| {
            let mut metadata = [
                ("session_id".to_string(), "session-legacy".to_string()),
                ("agent_run_id".to_string(), attempt_run_id.to_string()),
            ]
            .into_iter()
            .collect::<Metadata>();
            if let Some(source_attempt_run_id) = source_attempt_run_id {
                metadata.insert(
                    SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
                    source_attempt_run_id.to_string(),
                );
            }
            metadata
        };
        let context_a = legacy_context("attempt-a", None);
        let context_b = legacy_context("attempt-b", Some("attempt-a"));
        let mut context_c = legacy_context("attempt-c", Some("attempt-b"));
        let mut events = [context_a, context_b, context_c.clone()]
            .into_iter()
            .enumerate()
            .map(|(index, metadata)| Event {
                id: EventId(format!("event-{index}")),
                task_id: phase16_task_id(),
                sequence: index as u64 + 1,
                timestamp_ms: index as u64 + 1,
                kind: EventKind::TaskStatusChanged,
                summary: if index == 0 {
                    "Agent task started".to_string()
                } else {
                    "Agent task retry started".to_string()
                },
                metadata,
            })
            .collect::<Vec<_>>();
        events.push(Event {
            id: EventId("unrelated-malformed".to_string()),
            task_id: phase16_task_id(),
            sequence: 4,
            timestamp_ms: 4,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task started".to_string(),
            metadata: [
                ("session_id".to_string(), "session-other".to_string()),
                ("agent_run_id".to_string(), "attempt-other".to_string()),
                (
                    SOURCE_AGENT_RUN_ID_METADATA_KEY.to_string(),
                    "missing-source".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        });
        let lineage_events = agent_session_events(&events, "session-legacy");

        let (logical_run_id, source_attempt_run_id) =
            inherited_agent_run_identity_from_events(&lineage_events, &context_c)
                .expect("legacy lineage should be readable")
                .expect("active legacy attempt should exist");
        assert_eq!(logical_run_id, "attempt-a");
        assert_eq!(source_attempt_run_id, "attempt-c");

        assign_continuation_agent_run_identity(
            &mut context_c,
            &logical_run_id,
            &source_attempt_run_id,
        )
        .expect("next attempt should inherit the legacy root");
        let continuation = AgentRunIdentity::from_metadata(&context_c)
            .expect("continuation identity should decode")
            .expect("continuation identity should be versioned");
        assert_eq!(continuation.logical_run_id(), "attempt-a");
        assert_eq!(continuation.source_attempt_run_id(), Some("attempt-c"));
        assert_ne!(continuation.attempt_run_id(), "attempt-a");
        assert_ne!(continuation.attempt_run_id(), "attempt-b");
        assert_ne!(continuation.attempt_run_id(), "attempt-c");
    }
}
