use crate::app_state::AppState;

pub(crate) fn with_prompt_evolution_store<T>(
    state: &tauri::State<'_, AppState>,
    operation: impl FnOnce(&mut agent_storage::SqliteStore) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(test)]
    {
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        operation(&mut store)
    }
    #[cfg(not(test))]
    {
        let _ = state;
        let mut store = crate::persistence_runtime::open_app_store_at(
            &crate::persistence_runtime::database_path(),
        )
        .map_err(|error| error.to_string())?;
        operation(&mut store)
    }
}
