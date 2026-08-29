use super::*;
use crate::agent_read_model::{
    active_agent_events_for_session, agent_events_for_session, agent_state_for_session,
};
use crate::agent_runtime_snapshot::{
    capture_persistable_agent_task_state, delete_persisted_agent_runtime_snapshot,
};
use crate::agent_run_engine::PreparedAgentExecution;
use crate::agent_terminal_commit_runtime::persist_agent_terminal_once;
use crate::suspended_run_runtime::{
    clear_suspended_agent_run, remember_suspended_agent_run, take_suspended_agent_run,
    SuspendedAgentRun,
};
use agent_application::{insert_run_objectives, merge_persistable_run_context};

/// The permission action used to surface the doom-loop confirmation.
pub(crate) const DOOM_LOOP_CONFIRMATION_ACTION: &str = "agent.doom_loop.continue";

/// Builds the confirmation request shown to the user when the loop appears
/// stuck on one repeated tool. Delivered through the ordinary permission
/// surface so the run pauses until the user decides; it never auto-continues.
pub(crate) fn doom_loop_confirmation_request(
    run_context: &Metadata,
    tool: &str,
) -> PermissionRequest {
    let mut metadata = Metadata::new();
    metadata.insert(
        "kind".to_string(),
        agent_runtime::DOOM_LOOP_CONFIRMATION_KIND.to_string(),
    );
    metadata.insert("stuck_tool".to_string(), tool.to_string());
    metadata.insert("session_reusable".to_string(), "false".to_string());
    metadata = merge_persistable_run_context(metadata, run_context);
    insert_run_objectives(&mut metadata, run_context);
    PermissionRequest {
        id: PermissionRequestId(unique_id("agent-perm")),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Read,
        action: DOOM_LOOP_CONFIRMATION_ACTION.to_string(),
        reason: format!("Agent appears stuck on {tool}; continue?"),
        scope: tool.to_string(),
        metadata,
    }
}

/// True when a permission request is a doom-loop confirmation rather than a
/// tool permission.
pub(crate) fn is_doom_loop_confirmation_request(request: &PermissionRequest) -> bool {
    request
        .metadata
        .get("kind")
        .map(String::as_str)
        == Some(agent_runtime::DOOM_LOOP_CONFIRMATION_KIND)
}

/// Persists the doom-loop pause: the confirmation request, its audit event, and
/// the blocked recovery checkpoint that keeps the run resumable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn persist_doom_loop_confirmation_pause(
    store: &mut SqliteStore,
    runtime_task_id: &TaskId,
    session_id: Option<&str>,
    run_context: &Metadata,
    request: &PermissionRequest,
    tool: &str,
    task_state: &AgentTaskStateSnapshot,
    resource_snapshot: &RunResourceSnapshot,
) -> Result<(), String> {
    store
        .save_permission_request(request.clone(), current_time_millis())
        .map_err(|error| error.to_string())?;
    let mut permission_metadata = [
        ("permission_id".to_string(), request.id.0.clone()),
        ("tool".to_string(), request.action.clone()),
        ("stuck_tool".to_string(), tool.to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    insert_run_objectives(&mut permission_metadata, run_context);
    append_event(
        store,
        runtime_task_id,
        EventKind::PermissionRequested,
        format!("Agent appears stuck on {tool}; confirmation requested"),
        metadata_with_context(permission_metadata, run_context),
    )
    .map_err(|error| error.to_string())?;
    let events = agent_events_for_session(store, &phase16_task_id(), session_id)
        .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let recovery_metadata = agent_recovery_metadata_with_task_state(
        &active_events,
        run_context,
        AgentRecoveryState::Blocked,
        AgentRecoveryReason::WaitingForPermission,
        Metadata::new(),
        Some(task_state),
        Some(resource_snapshot),
    )?;
    append_event(
        store,
        runtime_task_id,
        EventKind::TaskStatusChanged,
        "Agent task waiting for permission",
        recovery_metadata,
    )
    .map_err(|error| error.to_string())
}

/// Persists the doom-loop cancel decision: the run terminates as cancelled and
/// its persisted runtime snapshot is dropped.
pub(crate) fn persist_doom_loop_confirmation_cancel(
    store: &mut SqliteStore,
    session_id: &str,
    run_context: &Metadata,
    steer_epoch: u64,
) -> Result<AgentState, String> {
    let persisted = persist_agent_terminal_once(
        store,
        &phase16_task_id(),
        run_context,
        steer_epoch,
        |store, identity| {
            append_event(
                store,
                &phase16_task_id(),
                EventKind::TaskStatusChanged,
                "Agent task cancelled",
                metadata_with_context(
                    [("reason".to_string(), "doom_loop_confirmation_cancelled".to_string())]
                        .into_iter()
                        .chain(identity.metadata())
                        .collect(),
                    run_context,
                ),
            )?;
            delete_persisted_agent_runtime_snapshot(store, Some(session_id))
                .map_err(StorageError::new)?;
            agent_state_for_session(store, None, Some(session_id))
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(persisted.state)
}

/// Per-turn observer checkpoint: publishes the run's remaining model-call
/// budget so the finalization observer can force the closing turn in time, and
/// pauses the run for an explicit user confirmation when observers flagged a
/// doom loop. Returns `None` when the loop may continue.
#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_loop_observer_checkpoint(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    runtime: &mut agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
) -> Result<Option<AgentState>, String> {
    let progress = cancellation.progress();
    runtime.loop_observers.publish_model_call_budget(
        progress.model_call_limit.saturating_sub(progress.model_calls),
        cancellation.budget().terminal_model_call_reserve,
    );
    let Some(tool) = runtime
        .loop_observers
        .pending_doom_loop_confirmation()
        .map(str::to_string)
    else {
        return Ok(None);
    };
    pause_agent_loop_for_doom_loop_confirmation(
        state,
        runtime,
        prompt,
        run_context,
        workspace_root,
        collaboration,
        cancellation,
        &tool,
    )
    .map(Some)
}

/// Pauses the run for a doom-loop confirmation: persists the confirmation
/// request and recovery checkpoint, then suspends the run. The run stays paused
/// until the user explicitly allows one continuation or cancels.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pause_agent_loop_for_doom_loop_confirmation(
    state: &tauri::State<'_, AppState>,
    runtime: &agent_runtime::AgentLoopState,
    prompt: &str,
    run_context: &Metadata,
    workspace_root: &Path,
    collaboration: Option<&AgentCollaboration>,
    cancellation: &Arc<AgentRunControl>,
    tool: &str,
) -> Result<AgentState, String> {
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();
    let request = doom_loop_confirmation_request(run_context, tool);
    let task_state = capture_persistable_agent_task_state(runtime);
    let resource_snapshot = cancellation.resource_usage();
    let suspended = SuspendedAgentRun {
        runtime: runtime.clone(),
        prompt: prompt.to_string(),
        run_context: run_context.clone(),
        workspace_root: workspace_root.to_path_buf(),
        collaboration: collaboration.cloned(),
        run_control: cancellation.snapshot(),
        last_touched_at_ms: current_time_millis(),
    };
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        persist_doom_loop_confirmation_pause(
            &mut store,
            &runtime.task_id,
            session_id,
            run_context,
            &request,
            tool,
            &task_state,
            &resource_snapshot,
        )?;
    }
    remember_suspended_agent_run(state, suspended)?;
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())
}

/// Resolves a doom-loop confirmation decision. Returns `None` when the request
/// is not a doom-loop confirmation so the caller falls through to the ordinary
/// tool-permission path. Allow resumes the suspended run exactly once after
/// resetting the stuck streak; deny terminates the run as cancelled.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_doom_loop_confirmation_if_pending(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    session_id: &str,
    effort: AgentPolicy,
    cancellation: &Arc<AgentRunControl>,
    run_context: &Metadata,
    config: &ProviderConfig,
) -> Result<Option<AgentState>, String> {
    if !is_doom_loop_confirmation_request(request) {
        return Ok(None);
    }
    let resolution = PermissionResolution {
        request_id: request.id.clone(),
        decision: decision.clone(),
        resolved_at_ms: current_time_millis(),
        resolved_by: "local-user".to_string(),
    };
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        store
            .with_immediate_transaction(|store| {
                crate::agent_commands::persist_permission_resolution_rows(
                    store, request, &resolution, run_context,
                )
            })
            .map_err(|error| error.to_string())?;
    }

    if matches!(decision, PermissionDecision::Deny) {
        clear_suspended_agent_run(&state, session_id)?;
        cancellation.request_cancel();
        let steer_epoch = run_context_steer_epoch(run_context);
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        return persist_doom_loop_confirmation_cancel(&mut store, session_id, run_context, steer_epoch)
            .map(Some);
    }

    let Some(mut suspended) = take_suspended_agent_run(&state, session_id)? else {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        return agent_state_for_session(&store, None, Some(session_id))
            .map(Some)
            .map_err(|error| error.to_string());
    };
    // Allow continues exactly once: reset the stuck streak and the pending
    // confirmation so the run gets one fresh chance before any new pause.
    agent_runtime::clear_doom_loop_confirmation(&mut suspended.runtime);
    let workspace_root = suspended.workspace_root.clone();
    let prepared = PreparedAgentExecution {
        base_run_context: suspended.run_context.clone(),
        run_context: suspended.run_context,
        runtime: suspended.runtime,
        prompt: suspended.prompt,
        collaboration: suspended.collaboration,
    };
    continue_agent_loop(
        app,
        &state,
        config,
        &workspace_root,
        prepared,
        effort,
        cancellation,
    )
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{
        PermissionRisk, AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_IDENTITY_V1_SCHEMA,
        LOGICAL_AGENT_RUN_ID_METADATA_KEY,
    };
    use agent_runtime::{start_agent_loop, AgentRuntimeConfig};

    fn doom_run_context() -> Metadata {
        [
            ("project_id".to_string(), "project-doom".to_string()),
            ("session_id".to_string(), "session-doom".to_string()),
            ("agent_run_id".to_string(), "run-doom".to_string()),
            ("steer_epoch".to_string(), "0".to_string()),
            (
                AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
                AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
            ),
            (
                LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                "run-doom".to_string(),
            ),
            (
                "effective_prompt_objective".to_string(),
                "fetch the page".to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    fn seed_doom_run_start(store: &mut SqliteStore, run_context: &Metadata) {
        let mut start_metadata = run_context.clone();
        start_metadata.insert("prompt".to_string(), "fetch the page".to_string());
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "Agent task started",
            start_metadata,
        )
        .expect("run start should append");
    }

    #[test]
    fn confirmation_request_names_the_stuck_tool_and_allows_only_once() {
        let request = doom_loop_confirmation_request(&doom_run_context(), "web.fetch");
        assert_eq!(request.action, DOOM_LOOP_CONFIRMATION_ACTION);
        assert_eq!(request.risk, PermissionRisk::Read);
        assert_eq!(request.scope, "web.fetch");
        assert_eq!(request.reason, "Agent appears stuck on web.fetch; continue?");
        assert!(is_doom_loop_confirmation_request(&request));
        assert_eq!(
            request.metadata.get("session_reusable").map(String::as_str),
            Some("false"),
            "a stuck-loop confirmation can only be allowed once"
        );
        assert!(!agent_core::permission_can_allow_session(&request));

        let mut tool_permission = request.clone();
        tool_permission.metadata.remove("kind");
        assert!(!is_doom_loop_confirmation_request(&tool_permission));
    }

    #[test]
    fn pause_persists_a_pending_confirmation_and_a_blocked_checkpoint() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = doom_run_context();
        seed_doom_run_start(&mut store, &run_context);
        let request = doom_loop_confirmation_request(&run_context, "web.fetch");
        let runtime = start_agent_loop(
            TaskId("doom-pause".to_string()),
            "fetch the page",
            AgentRuntimeConfig::default(),
        );
        let task_state = AgentTaskStateSnapshot::capture(&runtime);
        let control = AgentRunControl::new("auto");
        let resource_snapshot = control.resource_usage();

        persist_doom_loop_confirmation_pause(
            &mut store,
            &phase16_task_id(),
            Some("session-doom"),
            &run_context,
            &request,
            "web.fetch",
            &task_state,
            &resource_snapshot,
        )
        .expect("the doom-loop pause should persist");

        let pending = pending_agent_permissions_for_run(
            &store,
            &phase16_task_id(),
            Some("session-doom"),
            Some("run-doom"),
        )
        .expect("pending confirmations should be queryable");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].action, DOOM_LOOP_CONFIRMATION_ACTION);
        assert!(is_doom_loop_confirmation_request(&pending[0]));

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-doom")
            .expect("session events should load");
        assert!(
            events
                .iter()
                .any(|event| event.summary == "Agent task waiting for permission"),
            "the pause records a user-visible blocked checkpoint"
        );
        assert!(events.iter().any(|event| event
            .summary
            .contains("Agent appears stuck on web.fetch")));
    }

    #[test]
    fn cancel_terminates_the_run_as_cancelled() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let run_context = doom_run_context();
        seed_doom_run_start(&mut store, &run_context);

        let state = persist_doom_loop_confirmation_cancel(
            &mut store,
            "session-doom",
            &run_context,
            0,
        )
        .expect("the doom-loop cancel should persist a terminal");
        assert_eq!(state.status, "cancelled");

        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-doom")
            .expect("session events should load");
        let cancelled = events
            .iter()
            .find(|event| event.summary == "Agent task cancelled")
            .expect("the cancel decision persists a terminal cancelled event");
        assert_eq!(
            cancelled.metadata.get("reason").map(String::as_str),
            Some("doom_loop_confirmation_cancelled")
        );

        // A replayed cancel returns the same terminal without re-inserting.
        let replay = persist_doom_loop_confirmation_cancel(
            &mut store,
            "session-doom",
            &run_context,
            0,
        )
        .expect("replayed cancel loads the existing terminal");
        assert_eq!(replay.status, "cancelled");
        let events = store
            .list_by_task_and_metadata(&phase16_task_id(), "session_id", "session-doom")
            .expect("session events should load");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.summary == "Agent task cancelled")
                .count(),
            1
        );
    }

    #[test]
    fn continue_once_resets_the_stuck_streak_and_flag() {
        let mut runtime = start_agent_loop(
            TaskId("doom-resume".to_string()),
            "fetch the page",
            AgentRuntimeConfig::default(),
        );
        for _ in 0..4 {
            runtime
                .repetition_advisory
                .observe("web.fetch", r#"{"url":"https://example.com"}"#);
        }
        runtime
            .loop_observers
            .request_doom_loop_confirmation("web.fetch".to_string());
        assert_eq!(runtime.repetition_advisory.streak(), 4);
        assert_eq!(
            runtime.loop_observers.pending_doom_loop_confirmation(),
            Some("web.fetch")
        );

        agent_runtime::clear_doom_loop_confirmation(&mut runtime);
        assert_eq!(runtime.repetition_advisory.streak(), 0);
        assert!(runtime.loop_observers.pending_doom_loop_confirmation().is_none());
    }
}
