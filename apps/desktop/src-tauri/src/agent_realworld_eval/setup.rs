use crate::sha256_hex;
use crate::{
    app_state::{AppState, ToolRegistryCache},
    attachment_upload_batches::AttachmentUploadBatches,
    configuration_models::{ProjectSessionConfig, ProviderConfig, SidecarConfig, WorkspaceConfig},
    event_persistence::{append_event, append_message_event_with_metadata},
    memory_projection_runtime::load_project_memory_ledger_inner,
    memory_runtime::refresh_project_memory_after_run,
    persistence_runtime::{app_data_root, database_path, open_app_store_at},
    project_session_persistence::project_session_metadata_for_session,
    runtime_values::{current_time_millis, phase16_task_id, unique_id},
    session_output_cache_store::SessionOutputCache,
    suspended_run_runtime::SuspendedRunStore,
};
use agent_application::SessionTitleState;
use agent_core::{EventKind, MessageRole, Metadata};
use agent_mcp::McpCatalogService;
use agent_memory::{MemoryKind, MemoryTrust};
use agent_storage::{SqliteStore, StorageError};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use tools::{ProcessManager, WebSearchConfig};

const EVALUATION_DATA_DIR_ENV: &str = "CINDX_AGENT_REALWORLD_DATA_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SetupFailureStage {
    ProjectConfiguration,
    WorkspaceIndex,
    MemorySeed,
    RecallSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SetupFailureCode {
    Configuration,
    Index,
    Persistence,
    Data,
    Transient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct SetupFailure {
    stage: SetupFailureStage,
    code: SetupFailureCode,
    retryable: bool,
}

impl SetupFailure {
    pub(super) const fn new(
        stage: SetupFailureStage,
        code: SetupFailureCode,
        retryable: bool,
    ) -> Self {
        Self {
            stage,
            code,
            retryable,
        }
    }
}

pub(super) fn validate_evaluation_data_root_paths(
    suite_root: &Path,
    evaluation_data_root: &Path,
    production_data_root: &Path,
    production_database: &Path,
) -> Result<PathBuf, String> {
    if !suite_root.is_absolute() {
        return Err("evaluation suite root must be absolute".to_string());
    }
    if !evaluation_data_root.is_absolute() {
        return Err(format!(
            "{EVALUATION_DATA_DIR_ENV} must be an absolute path"
        ));
    }
    if evaluation_data_root
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "{EVALUATION_DATA_DIR_ENV} must not contain relative path components"
        ));
    }
    let relative = evaluation_data_root.strip_prefix(suite_root).map_err(|_| {
        format!("{EVALUATION_DATA_DIR_ENV} must remain inside the evaluation suite root")
    })?;
    if relative.as_os_str().is_empty() {
        return Err(format!(
            "{EVALUATION_DATA_DIR_ENV} must be a child of the evaluation suite root"
        ));
    }
    let evaluation_database = evaluation_data_root.join("state.sqlite3");
    if evaluation_data_root == production_data_root || evaluation_database == production_database {
        return Err(
            "evaluation state database must differ from the configured user database".to_string(),
        );
    }
    Ok(evaluation_data_root.to_path_buf())
}

pub(super) fn reject_existing_evaluation_path(path: &Path, label: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!(
            "{label} must not already exist: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to inspect {label} {}: {error}",
            path.display()
        )),
    }
}

pub(super) fn create_fresh_evaluation_data_root(
    evaluation_data_root: &Path,
    evaluation_database: &Path,
) -> Result<(), String> {
    reject_existing_evaluation_path(evaluation_data_root, "evaluation data root")?;
    fs::create_dir(evaluation_data_root).map_err(|error| {
        format!(
            "failed to create fresh evaluation data root {}: {error}",
            evaluation_data_root.display()
        )
    })?;
    reject_existing_evaluation_path(evaluation_database, "evaluation state database")
}

pub(super) fn activate_evaluation_data_root(suite_root: &Path) -> Result<PathBuf, String> {
    let production_data_root = app_data_root();
    let production_database = database_path();
    let requested_data_root = PathBuf::from(
        std::env::var_os(EVALUATION_DATA_DIR_ENV)
            .ok_or_else(|| format!("{EVALUATION_DATA_DIR_ENV} is required"))?,
    );
    let evaluation_data_root = validate_evaluation_data_root_paths(
        suite_root,
        &requested_data_root,
        &production_data_root,
        &production_database,
    )?;
    let evaluation_database = evaluation_data_root.join("state.sqlite3");
    create_fresh_evaluation_data_root(&evaluation_data_root, &evaluation_database)?;
    let canonical_suite_root = fs::canonicalize(suite_root).map_err(|error| {
        format!(
            "failed to resolve evaluation suite root {}: {error}",
            suite_root.display()
        )
    })?;
    let canonical_evaluation_root = fs::canonicalize(&evaluation_data_root).map_err(|error| {
        format!(
            "failed to resolve evaluation data root {}: {error}",
            evaluation_data_root.display()
        )
    })?;
    if canonical_evaluation_root == canonical_suite_root
        || !canonical_evaluation_root.starts_with(&canonical_suite_root)
    {
        return Err(format!(
            "{EVALUATION_DATA_DIR_ENV} resolved outside the evaluation suite root"
        ));
    }
    if production_data_root
        .canonicalize()
        .is_ok_and(|root| root == canonical_evaluation_root)
    {
        return Err("evaluation data root resolves to the configured user data root".to_string());
    }

    std::env::set_var("CINDX_DATA_DIR", &evaluation_data_root);
    if database_path() != evaluation_database {
        return Err("evaluation data root activation did not bind the state database".to_string());
    }
    Ok(evaluation_database)
}

pub(super) fn build_evaluation_app(
    provider: ProviderConfig,
    sidecars: SidecarConfig,
    root: &Path,
    database_path: &Path,
) -> Result<tauri::App<tauri::Wry>, String> {
    let project_sessions = isolated_project_config(root, "bootstrap");
    let state = AppState {
        store: Mutex::new(open_app_store_at(database_path).map_err(|error| error.to_string())?),
        attachment_upload_batches: Mutex::new(AttachmentUploadBatches::default()),
        manual_tool_execution_gate: Mutex::new(()),
        provider_config: Mutex::new(provider),
        provider_config_update: Mutex::new(()),
        workspace_config: Mutex::new(WorkspaceConfig {
            root: root.to_path_buf(),
        }),
        sidecar_config: Mutex::new(sidecars),
        web_search_config: Mutex::new(WebSearchConfig::default()),
        project_session_config: Mutex::new(project_sessions),
        session_lifecycle_gate: Mutex::new(()),
        schedule_config: Mutex::new(crate::schedule::ScheduleConfig::default()),
        schedule_last_error: Mutex::new(None),
        mcp_catalog: Mutex::new(McpCatalogService::load(
            root.join(".eval-mcp.json"),
            root.join(".eval-mcp-cache.json"),
        )),
        suspended_agent_runs: SuspendedRunStore::default(),
        session_output_cache: SessionOutputCache::default(),
        agent_run_controls: agent_harness::RunRegistry::new("agent realworld control"),
        prompt_evaluation_controls: agent_harness::RunRegistry::new(
            "agent realworld prompt evaluation control",
        ),
        rag_operation_controls: Mutex::new(BTreeMap::new()),
        queue_dispatching_sessions: agent_harness::ExclusiveKeyRegistry::new(
            "agent realworld queue",
        ),
        session_title_refinement_sessions: agent_harness::ExclusiveKeyRegistry::new(
            "agent realworld title",
        ),
        workspace_knowledge_cache: Mutex::new(BTreeMap::new()),
        process_manager: Arc::new(ProcessManager::new()),
        tool_registry_cache: Mutex::new(ToolRegistryCache::default()),
        tool_registry_generation: AtomicU64::new(0),
        allow_exit: AtomicBool::new(false),
        quit_prompt_active: AtomicBool::new(false),
    };
    tauri::Builder::default()
        .manage(state)
        .build(crate::app_bootstrap::application_context())
        .map_err(|error| format!("failed to build headless evaluation app: {error}"))
}

fn isolated_project_config_with_scope(
    root: &Path,
    key: &str,
    project_scope: Option<&str>,
) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(root);
    let project_id = project_scope
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("project-realworld-{key}"));
    let session_id = format!("session-realworld-{key}");
    config.active_project_id = project_id.clone();
    config.active_session_id = session_id.clone();
    config.projects[0].id = project_id.clone();
    config.projects[0].name = format!("Realworld {key}");
    config.projects[0].root = root.display().to_string();
    config.sessions[0].id = session_id;
    config.sessions[0].project_id = project_id;
    config.sessions[0].name = format!("Realworld {key}");
    config
}

fn isolated_project_config(root: &Path, key: &str) -> ProjectSessionConfig {
    isolated_project_config_with_scope(root, key, None)
}

pub(super) fn configure_run_project(
    state: &tauri::State<'_, AppState>,
    root: &Path,
    key: &str,
    project_scope: Option<&str>,
) -> Result<(String, String), String> {
    let config = isolated_project_config_with_scope(root, key, project_scope);
    let project_id = config.active_project_id.clone();
    let session_id = config.active_session_id.clone();
    *state
        .workspace_config
        .lock()
        .map_err(|error| format!("workspace config lock poisoned: {error}"))? = WorkspaceConfig {
        root: root.to_path_buf(),
    };
    *state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session lock poisoned: {error}"))? = config;
    state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("knowledge cache lock poisoned: {error}"))?
        .clear();
    state
        .tool_registry_cache
        .lock()
        .map_err(|error| format!("tool cache lock poisoned: {error}"))?
        .clear();
    state
        .tool_registry_generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok((project_id, session_id))
}

pub(super) fn add_recall_session(
    state: &tauri::State<'_, AppState>,
    project_id: &str,
    key: &str,
) -> Result<String, String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session lock poisoned: {error}"))?;
    let mut session = config
        .sessions
        .first()
        .cloned()
        .ok_or_else(|| "evaluation project has no seed session".to_string())?;
    let session_id = format!("session-realworld-{key}-recall");
    session.id = session_id.clone();
    session.project_id = project_id.to_string();
    session.name = "Memory recall".to_string();
    session.title_state = SessionTitleState::Pending;
    session.seen_event_sequence = 0;
    session.created_at_ms = current_time_millis();
    session.updated_at_ms = session.created_at_ms;
    session.archived_at_ms = None;
    config.sessions.push(session);
    config.active_session_id = session_id.clone();
    Ok(session_id)
}

#[derive(Debug)]
pub(super) enum MemorySeedFixtureError {
    Persistence(String),
    Data(String),
}

impl MemorySeedFixtureError {
    fn setup_failure(&self) -> SetupFailure {
        let code = match self {
            Self::Persistence(_) => SetupFailureCode::Persistence,
            Self::Data(_) => SetupFailureCode::Data,
        };
        SetupFailure::new(SetupFailureStage::MemorySeed, code, false)
    }

    fn into_message(self) -> String {
        match self {
            Self::Persistence(message) | Self::Data(message) => message,
        }
    }
}

#[derive(Debug)]
pub(super) struct SeededMemoryReadback {
    pub(super) record_count: usize,
    pub(super) user_requirement: String,
    pub(super) projection_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SeededMemoryFixtureReceipt {
    pub(super) record_count: usize,
    pub(super) projection_sha256: String,
}

pub(super) fn persist_memory_seed_fixture(
    store: &mut SqliteStore,
    run_context: &Metadata,
    seed_prompt: &str,
) -> Result<(), MemorySeedFixtureError> {
    if run_context
        .get("project_id")
        .is_none_or(|project_id| project_id.trim().is_empty())
    {
        return Err(MemorySeedFixtureError::Data(
            "evaluation memory fixture has no project context".to_string(),
        ));
    }
    let task_id = phase16_task_id();
    store
        .with_immediate_transaction(|store| {
            append_message_event_with_metadata(
                store,
                &task_id,
                MessageRole::User,
                seed_prompt,
                run_context.clone(),
            )?;
            append_event(
                store,
                &task_id,
                EventKind::TaskStatusChanged,
                "Agent task completed",
                run_context.clone(),
            )?;
            refresh_project_memory_after_run(store, run_context)?.ok_or_else(|| {
                StorageError::new("evaluation memory fixture has no project context")
            })?;
            Ok(())
        })
        .map_err(|error| {
            MemorySeedFixtureError::Persistence(format!(
                "failed to persist and project evaluation memory fixture: {error}"
            ))
        })?;
    Ok(())
}

pub(super) fn read_seeded_memory_fixture(
    database_path: &Path,
    project_id: &str,
    session_id: &str,
) -> Result<SeededMemoryReadback, MemorySeedFixtureError> {
    let mut reader = SqliteStore::open_read_only(database_path).map_err(|error| {
        MemorySeedFixtureError::Persistence(format!(
            "failed to open evaluation memory read store: {error}"
        ))
    })?;
    let loaded = load_project_memory_ledger_inner(&mut reader, project_id).map_err(|error| {
        MemorySeedFixtureError::Persistence(format!(
            "failed to read evaluation memory fixture: {error}"
        ))
    })?;
    if loaded.needs_persist {
        return Err(MemorySeedFixtureError::Data(
            "evaluation memory fixture was not projected durably".to_string(),
        ));
    }
    let user_requirement = loaded
        .ledger
        .records
        .iter()
        .filter(|record| {
            record.kind == MemoryKind::Requirement
                && record.trust == MemoryTrust::UserStated
                && record.provenance.session_id == session_id
                && record
                    .source_session_ids
                    .iter()
                    .any(|source| source == session_id)
        })
        .map(|record| record.content.trim())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if user_requirement.is_empty() {
        return Err(MemorySeedFixtureError::Data(format!(
            "evaluation memory fixture is empty or invisible for session {session_id}"
        )));
    }
    let mut projection = loaded
        .ledger
        .records
        .iter()
        .map(|record| {
            (
                record.kind.label(),
                record.trust.label(),
                record.content.trim(),
                record.importance,
            )
        })
        .collect::<Vec<_>>();
    projection.sort();
    let projection = serde_json::to_vec(&projection).map_err(|error| {
        MemorySeedFixtureError::Data(format!(
            "failed to encode evaluation memory fixture receipt: {error}"
        ))
    })?;
    let mut receipt_bytes = b"cindx.agent-memory-effect-seed.v1\0".to_vec();
    receipt_bytes.extend_from_slice(&projection);
    Ok(SeededMemoryReadback {
        record_count: loaded.ledger.records.len(),
        user_requirement,
        projection_sha256: sha256_hex(&receipt_bytes),
    })
}

pub(super) fn seed_memory_fixture_for_case(
    state: &tauri::State<'_, AppState>,
    database_path: &Path,
    project_id: &str,
    session_id: &str,
    seed_prompt: &str,
) -> Result<SeededMemoryFixtureReceipt, (SetupFailure, String)> {
    let mut run_context =
        project_session_metadata_for_session(state, Some(session_id)).map_err(|error| {
            (
                SetupFailure::new(
                    SetupFailureStage::MemorySeed,
                    SetupFailureCode::Configuration,
                    false,
                ),
                error,
            )
        })?;
    run_context.insert(
        "agent_run_id".to_string(),
        unique_id("realworld-memory-seed"),
    );
    run_context.insert("steer_epoch".to_string(), "0".to_string());
    {
        let mut store = state.store.lock().map_err(|error| {
            (
                SetupFailure::new(
                    SetupFailureStage::MemorySeed,
                    SetupFailureCode::Persistence,
                    false,
                ),
                format!("evaluation store lock poisoned: {error}"),
            )
        })?;
        persist_memory_seed_fixture(&mut store, &run_context, seed_prompt).map_err(|error| {
            let failure = error.setup_failure();
            (failure, error.into_message())
        })?;
    }
    read_seeded_memory_fixture(database_path, project_id, session_id)
        .map(|readback| {
            debug_assert!(!readback.user_requirement.is_empty());
            SeededMemoryFixtureReceipt {
                record_count: readback.record_count,
                projection_sha256: readback.projection_sha256,
            }
        })
        .map_err(|error| {
            let failure = error.setup_failure();
            (failure, error.into_message())
        })
}
