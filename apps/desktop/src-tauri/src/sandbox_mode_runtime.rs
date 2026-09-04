//! Session-scoped shell confinement mode.
//!
//! The sandbox mode of a session is stored as durable session events
//! (`sandbox_event = mode_changed`), exactly like other replayable session
//! state: the effective mode is a fold over the session's event log, restart
//! recovers it by replaying persisted events, and sessions are isolated
//! because the fold only honors events tagged with the session's own id. The
//! default is [`SandboxMode::FullAccess`] (no confinement), which keeps the
//! historical unconfined behavior.

use agent_core::{Event, Metadata, SandboxMode, SANDBOX_MODE_METADATA_KEY};
use agent_storage::SqliteStore;
use tauri::Manager;

pub(crate) const SANDBOX_MODE_SCHEMA: &str = "cindx.sandbox-mode.v1";
pub(crate) const SANDBOX_MODE_EVENT_KEY: &str = "sandbox_event";
pub(crate) const SANDBOX_MODE_EVENT_MARKER: &str = "mode_changed";
pub(crate) const SANDBOX_MODE_CHANGED_SUMMARY: &str = "Session sandbox mode changed";

/// Metadata carried by a sandbox mode change event. Run identity and session
/// context ride along through `metadata_with_context` at the call site.
pub(crate) fn sandbox_mode_changed_event_metadata(mode: SandboxMode) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert(
        SANDBOX_MODE_EVENT_KEY.to_string(),
        SANDBOX_MODE_EVENT_MARKER.to_string(),
    );
    metadata.insert(
        "sandbox_mode_schema".to_string(),
        SANDBOX_MODE_SCHEMA.to_string(),
    );
    metadata.insert(
        SANDBOX_MODE_METADATA_KEY.to_string(),
        mode.label().to_string(),
    );
    metadata
}

/// Effective sandbox mode = fold over the session's events: the latest valid
/// mode change wins, events of other sessions and malformed values are
/// ignored, and no change at all yields the default (FullAccess).
pub(crate) fn effective_sandbox_mode_from_events(
    events: &[Event],
    session_id: &str,
) -> SandboxMode {
    events
        .iter()
        .rev()
        .filter(|event| event.metadata.get("session_id").map(String::as_str) == Some(session_id))
        .filter_map(|event| {
            if event
                .metadata
                .get(SANDBOX_MODE_EVENT_KEY)
                .map(String::as_str)
                != Some(SANDBOX_MODE_EVENT_MARKER)
            {
                return None;
            }
            event
                .metadata
                .get(SANDBOX_MODE_METADATA_KEY)
                .map(String::as_str)
                .and_then(SandboxMode::parse)
        })
        .next()
        .unwrap_or_default()
}

/// Replay the durable session events and return the session's effective
/// sandbox mode. This is the restart-recovery path: nothing besides the event
/// log feeds the answer.
pub(crate) fn effective_sandbox_mode_for_session(
    store: &SqliteStore,
    session_id: &str,
) -> Result<SandboxMode, String> {
    let events = crate::agent_read_model::agent_events_for_session(
        store,
        &crate::runtime_values::phase16_task_id(),
        Some(session_id),
    )
    .map_err(|error| error.to_string())?;
    Ok(effective_sandbox_mode_from_events(&events, session_id))
}

/// The effective sandbox mode to apply before a tool executes, resolved from
/// the live event log so a mid-run mode change takes effect immediately. A
/// context without a session stays unconfined.
pub(crate) fn effective_sandbox_mode_before_tool_execution(
    state: &tauri::State<'_, crate::app_state::AppState>,
    run_context: &Metadata,
) -> Result<SandboxMode, String> {
    let Some(session_id) = run_context.get("session_id").map(String::as_str) else {
        return Ok(SandboxMode::FullAccess);
    };
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    effective_sandbox_mode_for_session(&store, session_id)
}

/// Append the mode-change event for a session (the durable record the fold
/// replays).
pub(crate) fn append_sandbox_mode_event(
    store: &mut SqliteStore,
    run_context: &Metadata,
    mode: SandboxMode,
) -> Result<(), String> {
    crate::event_persistence::append_event(
        store,
        &crate::runtime_values::phase16_task_id(),
        agent_core::EventKind::TaskStatusChanged,
        SANDBOX_MODE_CHANGED_SUMMARY,
        crate::project_session_persistence::metadata_with_context(
            sandbox_mode_changed_event_metadata(mode),
            run_context,
        ),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn set_session_sandbox_mode(
    state: tauri::State<'_, crate::app_state::AppState>,
    session_id: String,
    mode: String,
) -> Result<String, String> {
    let mode = SandboxMode::parse(mode.trim()).ok_or_else(|| {
        format!("unknown sandbox mode: {mode} (expected read-only, workspace-write, or full)")
    })?;
    let run_context = crate::project_session_persistence::project_session_metadata_for_session(
        &state,
        Some(&session_id),
    )?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_sandbox_mode_event(&mut store, &run_context, mode)?;
    Ok(mode.label().to_string())
}

/// Scans the session's events to fold out its effective sandbox mode.
///
/// Runs off the invoke thread: a sync command body would block the UI for the
/// whole read (P2-05).
#[tauri::command]
pub(crate) async fn get_session_sandbox_mode(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<crate::app_state::AppState>();
        get_session_sandbox_mode_blocking(state, session_id)
    })
    .await
    .map_err(|error| format!("sandbox mode failed to join: {error}"))?
}

fn get_session_sandbox_mode_blocking(
    state: tauri::State<'_, crate::app_state::AppState>,
    session_id: String,
) -> Result<String, String> {
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let mode = effective_sandbox_mode_for_session(&store, &session_id)?;
    Ok(mode.label().to_string())
}

#[cfg(test)]
#[path = "sandbox_mode_runtime_tests.rs"]
mod tests;
