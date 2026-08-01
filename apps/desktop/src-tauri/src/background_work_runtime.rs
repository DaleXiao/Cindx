use crate::app_state::AppState;
use agent_runtime::AgentRunControl;
use std::sync::{atomic::Ordering, Arc};
use std::time::{Duration, Instant};

pub(crate) fn wait_for_foreground_agent_idle(
    state: &tauri::State<'_, AppState>,
    control: &Arc<AgentRunControl>,
    idle_grace: Duration,
) -> Result<bool, String> {
    let mut idle_since = None::<Instant>;
    loop {
        if control.should_stop() || state.allow_exit.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let foreground_active = foreground_agent_active(state)?;
        if foreground_active {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= idle_grace {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

pub(crate) fn foreground_agent_active(state: &tauri::State<'_, AppState>) -> Result<bool, String> {
    state
        .agent_run_controls
        .is_empty()
        .map(|empty| !empty)
        .map_err(|error| error.to_string())
}

pub(crate) fn foreground_agent_should_preempt(state: &tauri::State<'_, AppState>) -> bool {
    state.allow_exit.load(Ordering::Relaxed) || foreground_agent_active(state).unwrap_or(true)
}
