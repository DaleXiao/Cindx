use super::*;

pub(crate) fn clone_provider_config(
    state: &tauri::State<'_, AppState>,
) -> Result<ProviderConfig, String> {
    state
        .provider_config
        .lock()
        .map(|config| config.clone())
        .map_err(|error| format!("provider config lock poisoned: {error}"))
}

pub(crate) fn apply_provider_config_input(config: &mut ProviderConfig, input: ProviderConfigInput) {
    config.base_url = normalized_config_value(&input.base_url);
    config.model = normalized_config_value(&input.model);
    config.conductor_model = normalized_config_value(&input.conductor_model);
    config.planner_model = normalized_config_value(&input.planner_model);
    config.executor_model = normalized_config_value(&input.executor_model);
    config.reviewer_model = normalized_config_value(&input.reviewer_model);
    config.summarizer_model = normalized_config_value(&input.summarizer_model);
    config.embedding_model = normalized_config_value(&input.embedding_model);
    config.image_model = normalized_config_value(&input.image_model);
    config.image_endpoint = normalized_config_value(&input.image_endpoint);
    config.collaboration_policy = match input.collaboration_policy.as_str() {
        "single" | "plan_execute_review" | "best_of_n" | "auto_router" => {
            input.collaboration_policy
        }
        _ => "auto_router".to_string(),
    };
    config.prompt_evolution_enabled = input.prompt_evolution_enabled;
    config.context_window_tokens = input.context_window_tokens.max(4_096);
    config.agent_system_prompt = normalized_agent_instructions(&input.agent_system_prompt);
    let api_key = normalized_config_value(&input.api_key);
    if !api_key.is_empty() {
        config.api_key = api_key;
    }

    if config.planner_model.is_empty() {
        config.planner_model = config.model.clone();
    }
    if config.conductor_model.is_empty() {
        config.conductor_model = config.planner_model.clone();
    }
    if config.executor_model.is_empty() {
        config.executor_model = config.model.clone();
    }
    if config.reviewer_model.is_empty() {
        config.reviewer_model = config.model.clone();
    }
    if config.summarizer_model.is_empty() {
        config.summarizer_model = config.model.clone();
    }
    config.embedding_model =
        embedding_model_for_provider(&config.base_url, &config.embedding_model);
}

pub(crate) fn load_provider_config() -> ProviderConfig {
    let Ok(text) = fs::read_to_string(provider_config_path()) else {
        return ProviderConfig::default();
    };
    provider_config_from_text(&text)
}

pub(crate) fn provider_config_from_text(text: &str) -> ProviderConfig {
    let mut config = ProviderConfig::default();
    let mut conductor_model_loaded = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "base_url" => config.base_url = value.to_string(),
            "api_key" => config.api_key = value.to_string(),
            "model" => config.model = value.to_string(),
            "conductor_model" => {
                config.conductor_model = value.to_string();
                conductor_model_loaded = true;
            }
            "planner_model" => config.planner_model = value.to_string(),
            "executor_model" => config.executor_model = value.to_string(),
            "reviewer_model" => config.reviewer_model = value.to_string(),
            "summarizer_model" => config.summarizer_model = value.to_string(),
            "embedding_model" => config.embedding_model = value.to_string(),
            "image_model" => config.image_model = value.to_string(),
            "image_endpoint" => config.image_endpoint = value.to_string(),
            "collaboration_policy" => config.collaboration_policy = value.to_string(),
            "prompt_evolution_enabled" => config.prompt_evolution_enabled = config_bool(value),
            "context_window_tokens" => {
                config.context_window_tokens = value.parse().unwrap_or(128_000)
            }
            "agent_system_prompt_hex" => {
                if let Some(prompt) = config_hex_decode(value) {
                    config.agent_system_prompt = normalized_agent_instructions(&prompt);
                }
            }
            _ => {}
        }
    }

    if !conductor_model_loaded || config.conductor_model.trim().is_empty() {
        config.conductor_model = config.model_for_role(&ModelRole::Planner);
    }
    config.embedding_model =
        embedding_model_for_provider(&config.base_url, &config.embedding_model);
    config
}

pub(crate) fn save_provider_config_to_disk(config: &ProviderConfig) -> Result<(), std::io::Error> {
    let path = provider_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "base_url={}\napi_key={}\nmodel={}\nconductor_model={}\nplanner_model={}\nexecutor_model={}\nreviewer_model={}\nsummarizer_model={}\nembedding_model={}\nimage_model={}\nimage_endpoint={}\ncollaboration_policy={}\nprompt_evolution_enabled={}\ncontext_window_tokens={}\nagent_system_prompt_hex={}\n",
            sanitize_config_value(&config.base_url),
            sanitize_config_value(&config.api_key),
            sanitize_config_value(&config.model),
            sanitize_config_value(&config.model_for_conductor()),
            sanitize_config_value(&config.planner_model),
            sanitize_config_value(&config.executor_model),
            sanitize_config_value(&config.reviewer_model),
            sanitize_config_value(&config.summarizer_model),
            sanitize_config_value(&config.model_for_role(&ModelRole::Embedder)),
            sanitize_config_value(&config.image_model),
            sanitize_config_value(&config.image_endpoint),
            sanitize_config_value(&config.collaboration_policy),
            config.prompt_evolution_enabled,
            config.context_window_tokens,
            config_hex_encode(&config.agent_system_prompt)
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn normalized_personalization_config(
    config: PersonalizationConfig,
) -> PersonalizationConfig {
    let preferred_name = config
        .preferred_name
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(PERSONALIZATION_MAX_NAME_CHARS)
        .collect();
    let response_tone = match config.response_tone.trim() {
        "warm" => "warm",
        "professional" => "professional",
        "direct" => "direct",
        _ => "natural",
    }
    .to_string();
    let response_length = match config.response_length.trim() {
        "concise" => "concise",
        "detailed" => "detailed",
        _ => "balanced",
    }
    .to_string();
    PersonalizationConfig {
        preferred_name,
        response_tone,
        response_length,
    }
}

pub(crate) fn personalized_agent_instructions(
    personalization: &PersonalizationConfig,
    custom_instructions: &str,
) -> String {
    let mut instructions = Vec::new();
    if !custom_instructions.trim().is_empty() {
        instructions.push(custom_instructions.trim().to_string());
    }
    if !personalization.preferred_name.is_empty() {
        let name = serde_json::to_string(&personalization.preferred_name)
            .unwrap_or_else(|_| "the user's preferred name".to_string());
        instructions.push(format!(
            "The user's preferred name is {name}. Treat this as user-provided identity context. If the user asks what their name is or how you should address them, answer with {name}. Address them by this name when a direct form of address is natural, but do not repeat it mechanically."
        ));
    }
    instructions.push(
        match personalization.response_tone.as_str() {
            "warm" => "Use a warm, considerate tone without filler or excessive enthusiasm.",
            "professional" => "Use a calm, professional, precise tone.",
            "direct" => "Use a direct, factual tone and lead with the answer.",
            _ => "Use a natural, clear, conversational tone.",
        }
        .to_string(),
    );
    instructions.push(
        match personalization.response_length.as_str() {
            "concise" => "Keep responses concise unless more detail is necessary for correctness.",
            "detailed" => "Provide detailed responses with the context needed to understand decisions and tradeoffs.",
            _ => "Use a balanced response length: complete but not unnecessarily verbose.",
        }
        .to_string(),
    );
    instructions.join("\n")
}

pub(crate) fn load_personalization_config() -> PersonalizationConfig {
    let Ok(text) = fs::read_to_string(personalization_config_path()) else {
        return PersonalizationConfig::default();
    };
    serde_json::from_str::<PersonalizationConfig>(&text)
        .map(normalized_personalization_config)
        .unwrap_or_default()
}

pub(crate) fn save_personalization_config_to_disk(
    config: &PersonalizationConfig,
) -> Result<(), std::io::Error> {
    let path = personalization_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    let payload = serde_json::to_vec_pretty(config)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    file.write_all(&payload)?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn load_workspace_config() -> WorkspaceConfig {
    let mut config = WorkspaceConfig::default();
    let Ok(text) = fs::read_to_string(workspace_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "root" {
            if let Ok(root) = validate_workspace_root(value) {
                config.root = root;
            }
        }
    }

    config
}

pub(crate) fn save_workspace_config_to_disk(
    config: &WorkspaceConfig,
) -> Result<(), std::io::Error> {
    let path = workspace_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(format!("root={}\n", config.root.display()).as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn load_project_session_config(fallback_root: &Path) -> ProjectSessionConfig {
    let mut config = ProjectSessionConfig::default_for_root(fallback_root);
    let Ok(text) = fs::read_to_string(project_session_config_path()) else {
        return config;
    };

    let mut active_project_id = String::new();
    let mut active_session_id = String::new();
    let mut projects = Vec::new();
    let mut sessions = Vec::new();

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("active_project_id=") {
            active_project_id = value.to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("active_session_id=") {
            active_session_id = value.to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("project\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() >= 6 {
                projects.push(ProjectRecord {
                    id: fields[0].to_string(),
                    name: fields[1].to_string(),
                    root: fields[2].to_string(),
                    detail: fields[3].to_string(),
                    created_at_ms: fields[4].parse().unwrap_or_default(),
                    updated_at_ms: fields[5].parse().unwrap_or_default(),
                });
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("session\t") {
            let fields = value.split('\t').collect::<Vec<_>>();
            if fields.len() >= 7 {
                let name = fields[2].to_string();
                sessions.push(SessionRecord {
                    id: fields[0].to_string(),
                    project_id: fields[1].to_string(),
                    title_state: SessionTitleState::parse(
                        fields.get(9).copied(),
                        is_automatic_session_name(&name),
                    ),
                    name,
                    detail: fields[3].to_string(),
                    effort: fields
                        .get(8)
                        .map(|value| AgentEffort::parse(value).label().to_string())
                        .unwrap_or_else(default_agent_effort),
                    seen_event_sequence: fields
                        .get(10)
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or_default(),
                    created_at_ms: fields[4].parse().unwrap_or_default(),
                    updated_at_ms: fields[5].parse().unwrap_or_default(),
                    archived_at_ms: fields
                        .get(7)
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|value| *value > 0),
                });
                if fields[6] == "active" {
                    active_session_id = fields[0].to_string();
                }
            }
        }
    }

    config.projects = projects;
    config.sessions = sessions;
    config.active_project_id = active_project_id;
    config.active_session_id = active_session_id;
    if config.projects.is_empty() {
        config.sessions.clear();
        config.active_project_id.clear();
        config.active_session_id.clear();
        return config;
    }
    config.ensure_consistent(fallback_root);

    config
}

pub(crate) fn save_project_session_config_to_disk(
    config: &ProjectSessionConfig,
) -> Result<(), std::io::Error> {
    let path = project_session_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut text = format!(
        "active_project_id={}\nactive_session_id={}\n",
        sanitize_config_value(&config.active_project_id),
        sanitize_config_value(&config.active_session_id)
    );
    for project in &config.projects {
        text.push_str(&format!(
            "project\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_record_field(&project.id),
            sanitize_record_field(&project.name),
            sanitize_record_field(&project.root),
            sanitize_record_field(&project.detail),
            project.created_at_ms,
            project.updated_at_ms
        ));
    }
    for session in &config.sessions {
        let active_marker = if session.id == config.active_session_id {
            "active"
        } else {
            ""
        };
        text.push_str(&format!(
            "session\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sanitize_record_field(&session.id),
            sanitize_record_field(&session.project_id),
            sanitize_record_field(&session.name),
            sanitize_record_field(&session.detail),
            session.created_at_ms,
            session.updated_at_ms,
            active_marker,
            session.archived_at_ms.unwrap_or_default(),
            sanitize_record_field(&session.effort),
            session.title_state.label(),
            session.seen_event_sequence
        ));
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(text.as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn project_session_state(
    config: &ProjectSessionConfig,
    last_error: Option<String>,
) -> ProjectSessionState {
    ProjectSessionState {
        projects: config
            .projects
            .iter()
            .map(|project| ProjectView {
                id: project.id.clone(),
                name: project.name.clone(),
                root: project.root.clone(),
                detail: project.detail.clone(),
                status: if project.id == config.active_project_id {
                    "Selected".to_string()
                } else {
                    "Ready".to_string()
                },
                active: project.id == config.active_project_id,
                created_at_ms: project.created_at_ms,
                updated_at_ms: project.updated_at_ms,
            })
            .collect(),
        sessions: config
            .sessions
            .iter()
            .filter(|session| !is_schedule_execution_session(session))
            .map(|session| SessionView {
                id: session.id.clone(),
                project_id: session.project_id.clone(),
                name: session.name.clone(),
                title_state: session.title_state.label().to_string(),
                detail: session.detail.clone(),
                effort: session.effort.clone(),
                status: if session.id == config.active_session_id {
                    "Active".to_string()
                } else if session.archived_at_ms.is_some() {
                    "Archived".to_string()
                } else {
                    "Ready".to_string()
                },
                activity: "idle".to_string(),
                attention_reason: None,
                unseen_result: false,
                latest_sequence: session.seen_event_sequence,
                active: session.id == config.active_session_id,
                archived: session.archived_at_ms.is_some(),
                archived_at_ms: session.archived_at_ms,
                created_at_ms: session.created_at_ms,
                updated_at_ms: session.updated_at_ms,
            })
            .collect(),
        active_project_id: config.active_project_id.clone(),
        active_session_id: config.active_session_id.clone(),
        last_error,
    }
}

pub(crate) fn apply_session_lifecycle_projections(
    state: &mut ProjectSessionState,
    config: &ProjectSessionConfig,
    store: &SqliteStore,
) -> Result<(), StorageError> {
    for session in &mut state.sessions {
        if session.archived {
            continue;
        }
        let Some(record) = config
            .sessions
            .iter()
            .find(|record| record.id == session.id)
        else {
            continue;
        };
        let model = load_agent_session_read_model_snapshot(store, &session.id)?;
        let projection = project_session_lifecycle(SessionLifecycleInput {
            status: &model.state.status,
            can_continue: model.state.can_continue,
            latest_sequence: model.state.latest_sequence,
            seen_event_sequence: record.seen_event_sequence,
        });
        session.status = projection.status_label.to_string();
        session.activity = projection.activity.to_string();
        session.attention_reason = projection.attention_reason.map(str::to_string);
        session.unseen_result = projection.unseen_result;
        session.latest_sequence = projection.latest_sequence;
    }
    Ok(())
}

pub(crate) fn project_session_state_from_store(
    config: &ProjectSessionConfig,
    store: &SqliteStore,
    last_error: Option<String>,
) -> Result<ProjectSessionState, StorageError> {
    let mut state = project_session_state(config, last_error);
    apply_session_lifecycle_projections(&mut state, config, store)?;
    Ok(state)
}

pub(crate) fn remove_project_from_config(
    config: &mut ProjectSessionConfig,
    project_id: &str,
) -> Option<(ProjectRecord, Vec<String>)> {
    let project_index = config
        .projects
        .iter()
        .position(|project| project.id == project_id)?;
    let project = config.projects[project_index].clone();
    let deleted_session_ids = config
        .sessions
        .iter()
        .filter(|session| session.project_id == project.id)
        .map(|session| session.id.clone())
        .collect::<Vec<_>>();
    let deleting_active_project = config.active_project_id == project.id;

    config.projects.remove(project_index);
    config
        .sessions
        .retain(|session| session.project_id != project.id);

    if config.projects.is_empty() {
        config.active_project_id.clear();
        config.active_session_id.clear();
        return Some((project, deleted_session_ids));
    }
    if deleting_active_project {
        config.active_project_id = config.projects
            [project_index.min(config.projects.len().saturating_sub(1))]
        .id
        .clone();
    }
    let active_session_is_valid = config.sessions.iter().any(|session| {
        session.id == config.active_session_id
            && session.project_id == config.active_project_id
            && session.archived_at_ms.is_none()
            && !is_schedule_execution_session(session)
    });
    if !active_session_is_valid {
        let active_project_id = config.active_project_id.clone();
        config.active_session_id = ensure_open_session_for_project(config, &active_project_id);
    }

    Some((project, deleted_session_ids))
}

pub(crate) fn ensure_open_session_for_project(
    config: &mut ProjectSessionConfig,
    project_id: &str,
) -> String {
    if let Some(session) = config.sessions.iter().find(|session| {
        session.project_id == project_id
            && session.archived_at_ms.is_none()
            && !is_schedule_execution_session(session)
    }) {
        return session.id.clone();
    }

    let project_name = config
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .map(|project| project.name.clone())
        .unwrap_or_else(|| "Runtime".to_string());
    let name = format!("{project_name} Session");
    let id = new_session_id();
    let now = current_time_millis();
    config.sessions.push(SessionRecord {
        id: id.clone(),
        project_id: project_id.to_string(),
        name,
        title_state: SessionTitleState::Pending,
        detail: "timeline + chat".to_string(),
        effort: default_agent_effort(),
        seen_event_sequence: 0,
        created_at_ms: now,
        updated_at_ms: now,
        archived_at_ms: None,
    });
    id
}

pub(crate) fn unique_fork_name(config: &ProjectSessionConfig, source: &SessionRecord) -> String {
    let base = format!("{} Fork", source.name);
    if !config
        .sessions
        .iter()
        .any(|session| session.project_id == source.project_id && session.name == base)
    {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{} Fork {suffix}", source.name);
        if !config
            .sessions
            .iter()
            .any(|session| session.project_id == source.project_id && session.name == candidate)
        {
            return candidate;
        }
        suffix += 1;
    }
}

pub(crate) fn project_session_state_with_error(
    state: &tauri::State<'_, AppState>,
    message: impl Into<String>,
) -> Result<ProjectSessionState, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    Ok(project_session_state(&config, Some(message.into())))
}

pub(crate) fn sync_active_project_root(
    state: &tauri::State<'_, AppState>,
    root: &Path,
) -> Result<(), String> {
    let mut config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let active_project_id = config.active_project_id.clone();
    if let Some(project) = config
        .projects
        .iter_mut()
        .find(|project| project.id == active_project_id)
    {
        project.root = root.display().to_string();
        project.updated_at_ms = current_time_millis();
        save_project_session_config_to_disk(&config).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn project_session_metadata_for_session(
    state: &tauri::State<'_, AppState>,
    session_id: Option<&str>,
) -> Result<Metadata, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?
        .clone();
    let Some(session_id) = session_id else {
        return Ok(project_session_metadata(&config));
    };
    let session = config
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
        .ok_or_else(|| format!("session not found: {session_id}"))?;
    let project = config
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| format!("project not found for session: {session_id}"))?;

    Ok([
        ("project_id".to_string(), project.id.clone()),
        ("project_name".to_string(), project.name.clone()),
        ("project_root".to_string(), project.root.clone()),
        ("session_id".to_string(), session.id.clone()),
        ("session_name".to_string(), session.name.clone()),
    ]
    .into_iter()
    .collect())
}

pub(crate) fn project_session_metadata(config: &ProjectSessionConfig) -> Metadata {
    let mut metadata = Metadata::new();
    if let Some(project) = config.active_project() {
        metadata.insert("project_id".to_string(), project.id.clone());
        metadata.insert("project_name".to_string(), project.name.clone());
        metadata.insert("project_root".to_string(), project.root.clone());
    }
    if let Some(session) = config.active_session() {
        metadata.insert("session_id".to_string(), session.id.clone());
        metadata.insert("session_name".to_string(), session.name.clone());
    }
    metadata
}

pub(crate) fn metadata_with_context(mut metadata: Metadata, context: &Metadata) -> Metadata {
    for (key, value) in context {
        metadata.entry(key.clone()).or_insert_with(|| value.clone());
    }
    metadata
}

pub(crate) fn agent_run_context_from_events(events: &[Event]) -> AgentRunContext {
    let start = events
        .iter()
        .find(|event| is_agent_run_start_event(event))
        .or_else(|| {
            events
                .iter()
                .find(|event| event.metadata.contains_key("session_id"))
        });
    AgentRunContext {
        project_id: start.and_then(|event| event.metadata.get("project_id").cloned()),
        project_name: start.and_then(|event| event.metadata.get("project_name").cloned()),
        session_id: start.and_then(|event| event.metadata.get("session_id").cloned()),
        session_name: start.and_then(|event| event.metadata.get("session_name").cloned()),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AgentRunContext {
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) session_name: Option<String>,
}

pub(crate) fn load_sidecar_config() -> SidecarConfig {
    let mut config = SidecarConfig::default();
    let Ok(text) = fs::read_to_string(sidecar_config_path()) else {
        return config;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "browser_path" => config.browser_path = value.to_string(),
            "computer_path" => config.computer_path = value.to_string(),
            "auto_configure" => config.auto_configure = config_bool(value),
            _ => {}
        }
    }

    if config.browser_path.trim().is_empty() {
        config.browser_path = default_browser_sidecar_path().display().to_string();
    }
    if config.computer_path.trim().is_empty() {
        config.computer_path = default_computer_sidecar_path().display().to_string();
    }

    config
}

pub(crate) fn save_sidecar_config_to_disk(config: &SidecarConfig) -> Result<(), std::io::Error> {
    let path = sidecar_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "browser_path={}\ncomputer_path={}\nauto_configure={}\n",
            sanitize_config_value(&config.browser_path),
            sanitize_config_value(&config.computer_path),
            config.auto_configure
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}

pub(crate) fn load_web_search_config() -> WebSearchConfig {
    let mut config = WebSearchConfig::default();
    let Ok(text) = fs::read_to_string(web_search_config_path()) else {
        return config;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "endpoint" => config.endpoint = value.to_string(),
            "api_key" => config.api_key = value.to_string(),
            _ => {}
        }
    }
    config
}

pub(crate) fn save_web_search_config_to_disk(
    config: &WebSearchConfig,
) -> Result<(), std::io::Error> {
    let path = web_search_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path)?;
    file.write_all(
        format!(
            "endpoint={}\napi_key={}\n",
            sanitize_config_value(&config.endpoint),
            sanitize_config_value(&config.api_key)
        )
        .as_bytes(),
    )?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn apply_sidecar_env(config: &SidecarConfig) {
    if config.auto_configure {
        std::env::set_var("CINDX_BROWSER_SIDECAR", &config.browser_path);
        std::env::set_var("CINDX_COMPUTER_SIDECAR", &config.computer_path);
        if let Some(node_path) = find_node_executable() {
            std::env::set_var("CINDX_NODE", node_path);
        }
    } else {
        std::env::remove_var("CINDX_BROWSER_SIDECAR");
        std::env::remove_var("CINDX_COMPUTER_SIDECAR");
        std::env::remove_var("CINDX_NODE");
    }
}

pub(crate) fn sidecar_state(config: &SidecarConfig, last_error: Option<String>) -> SidecarState {
    SidecarState {
        browser: sidecar_endpoint_state("CINDX_BROWSER_SIDECAR", &config.browser_path),
        computer: sidecar_endpoint_state("CINDX_COMPUTER_SIDECAR", &config.computer_path),
        auto_configure: config.auto_configure,
        last_error,
    }
}

pub(crate) fn sidecar_endpoint_state(env_key: &str, path: &str) -> SidecarEndpointState {
    let path_buf = PathBuf::from(path);
    let exists = path_buf.exists();
    let executable = exists && path_buf.is_file();
    let health = if executable {
        let mut command = sidecar_command(&path_buf);
        command
            .arg("--health")
            .output()
            .map(|output| {
                (
                    output.status.success(),
                    if output.status.success() {
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    } else {
                        String::from_utf8_lossy(&output.stderr).trim().to_string()
                    },
                )
            })
            .unwrap_or_else(|error| (false, error.to_string()))
    } else {
        (false, "sidecar path is missing".to_string())
    };

    SidecarEndpointState {
        path: path.to_string(),
        exists,
        executable,
        healthy: health.0,
        health_output: health.1,
        env_key: env_key.to_string(),
    }
}

pub(crate) fn sidecar_command(path: &Path) -> std::process::Command {
    if path.extension().and_then(|extension| extension.to_str()) == Some("js") {
        let mut command = std::process::Command::new(
            find_node_executable().unwrap_or_else(|| PathBuf::from("node")),
        );
        command.arg(path);
        command
    } else {
        std::process::Command::new(path)
    }
}

pub(crate) fn find_node_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CINDX_NODE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    [
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
        PathBuf::from("/usr/bin/node"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

pub(crate) fn default_browser_sidecar_path() -> PathBuf {
    bundled_or_development_resource("browser-sidecar.js")
}

pub(crate) fn default_computer_sidecar_path() -> PathBuf {
    bundled_or_development_resource("computer-sidecar.js")
}

pub(crate) fn development_repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

pub(crate) fn bundled_or_development_resource(file_name: &str) -> PathBuf {
    let packaged = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .and_then(|macos| macos.parent().map(Path::to_path_buf))
        .map(|contents| contents.join("Resources").join("sidecars").join(file_name));
    if let Some(path) = packaged.as_ref().filter(|path| path.is_file()) {
        return path.clone();
    }

    let development = development_repo_root()
        .join("scripts")
        .join("sidecars")
        .join(file_name);
    if development.is_file() {
        return development;
    }

    packaged.unwrap_or(development)
}

pub(crate) fn config_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}

pub(crate) fn validate_workspace_root(path: &str) -> Result<PathBuf, String> {
    let path = normalized_config_value(path);
    if path.is_empty() {
        return Err("workspace path is empty".to_string());
    }
    let candidate = PathBuf::from(path);
    let root = if candidate.is_absolute() {
        candidate
    } else {
        workspace_root().join(candidate)
    };
    let canonical =
        fs::canonicalize(&root).map_err(|error| format!("failed to resolve workspace: {error}"))?;
    if !canonical.is_dir() {
        return Err("workspace path must be a directory".to_string());
    }

    Ok(canonical)
}

pub(crate) fn active_workspace_root(state: &tauri::State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .workspace_config
        .lock()
        .map(|config| config.root.clone())
        .map_err(|error| format!("workspace config lock poisoned: {error}"))
}

pub(crate) fn project_root_for_session(
    state: &tauri::State<'_, AppState>,
    session_id: &str,
) -> Result<PathBuf, String> {
    let config = state
        .project_session_config
        .lock()
        .map_err(|error| format!("project session config lock poisoned: {error}"))?;
    let session = config
        .sessions
        .iter()
        .find(|session| session.id == session_id && session.archived_at_ms.is_none())
        .ok_or_else(|| "session not found".to_string())?;
    let project = config
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| "project not found for session".to_string())?;
    validate_workspace_root(&project.root)
}

pub(crate) fn safe_attachment_name(value: &str) -> String {
    let file_name = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment");
    let sanitized = file_name
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| match character {
            '/' | '\\' | ':' => '_',
            other => other,
        })
        .take(120)
        .collect::<String>();
    if sanitized.trim().is_empty() {
        "attachment".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn normalized_attachment_mime(value: &str, path: &Path) -> String {
    let value = normalized_config_value(value).to_ascii_lowercase();
    if value.starts_with("image/") || value.starts_with("text/") {
        return value;
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "avif" => "image/avif",
        "gif" => "image/gif",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub(crate) fn attachment_storage_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("attachments")
}

pub(crate) fn validated_attachment_path(
    workspace_root: &Path,
    value: &str,
) -> Result<PathBuf, String> {
    let attachment_root = attachment_storage_root(workspace_root);
    let canonical_root = fs::canonicalize(&attachment_root)
        .map_err(|error| format!("failed to resolve attachment directory: {error}"))?;
    let canonical_path = fs::canonicalize(value)
        .map_err(|error| format!("failed to resolve attachment: {error}"))?;
    if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
        return Err("attachment must be a staged file for this project".to_string());
    }
    Ok(canonical_path)
}

pub(crate) fn tool_registry_for_state(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<ToolRegistry, String> {
    let generation = state.tool_registry_generation.load(Ordering::Acquire);
    if let Some(registry) = state
        .tool_registry_cache
        .lock()
        .map_err(|error| format!("tool registry cache lock poisoned: {error}"))?
        .get(workspace_root, generation)
    {
        return Ok(registry);
    }

    let registry = build_tool_registry_for_state(state, workspace_root)?;
    if state.tool_registry_generation.load(Ordering::Acquire) == generation {
        state
            .tool_registry_cache
            .lock()
            .map_err(|error| format!("tool registry cache lock poisoned: {error}"))?
            .insert(workspace_root.to_path_buf(), generation, registry.clone());
    }
    Ok(registry)
}

pub(crate) fn build_tool_registry_for_state(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<ToolRegistry, String> {
    let web_search_config = state
        .web_search_config
        .lock()
        .map_err(|error| format!("web search config lock poisoned: {error}"))?
        .clone();
    let provider_config = state
        .provider_config
        .lock()
        .map_err(|error| format!("provider config lock poisoned: {error}"))?
        .clone();
    let image_endpoint = if provider_config.image_endpoint.trim().is_empty() {
        provider_config.base_url.clone()
    } else {
        provider_config.image_endpoint.clone()
    };
    let image_generation_config =
        (!provider_config.image_model.trim().is_empty()).then_some(ImageGenerationConfig {
            base_url: image_endpoint,
            api_key: provider_config.api_key,
            model: provider_config.image_model,
            timeout_seconds: 300,
        });
    let mut registry = ToolRegistry::with_workspace_tools_and_services(
        workspace_root.to_path_buf(),
        web_search_config,
        image_generation_config,
    );
    let catalog = state
        .mcp_catalog
        .lock()
        .map_err(|error| format!("MCP catalog lock poisoned: {error}"))?;
    for tool in catalog.cached_tools() {
        if let Err(error) = registry.try_register(tool) {
            eprintln!("skipping invalid MCP tool: {error}");
        }
    }
    drop(catalog);
    for tool in skill_catalog_for_root(workspace_root).tools() {
        if let Err(error) = registry.try_register(tool) {
            eprintln!("skipping invalid skill tool: {error}");
        }
    }
    registry.install_meta_tools();
    Ok(registry)
}

pub(crate) fn invalidate_tool_registry_cache(state: &AppState) -> Result<(), String> {
    state
        .tool_registry_generation
        .fetch_add(1, Ordering::AcqRel);
    state
        .tool_registry_cache
        .lock()
        .map_err(|error| format!("tool registry cache lock poisoned: {error}"))?
        .clear();
    Ok(())
}

pub(crate) fn skill_catalog_for_root(workspace_root: &Path) -> SkillCatalog {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.to_path_buf());
    SkillCatalog::load(
        home.join(".cindx/skills"),
        workspace_root,
        home.join(".cindx/skill-preferences.json"),
    )
}

pub(crate) fn open_app_store() -> Result<SqliteStore, StorageError> {
    let database_path = database_path();
    if let Some(parent) = database_path.parent() {
        fs::create_dir_all(parent).map_err(|error| StorageError::new(error.to_string()))?;
        secure_directory(parent).map_err(|error| StorageError::new(error.to_string()))?;
    }

    let store = SqliteStore::open(&database_path)?;
    #[cfg(unix)]
    fs::set_permissions(&database_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| StorageError::new(error.to_string()))?;

    Ok(store)
}

pub(crate) fn open_app_read_store() -> Result<SqliteStore, String> {
    SqliteStore::open_read_only(database_path()).map_err(|error| error.to_string())
}

pub(crate) fn database_path() -> PathBuf {
    app_data_root().join("state.sqlite3")
}

pub(crate) fn provider_config_path() -> PathBuf {
    app_data_root().join("provider.conf")
}

pub(crate) fn personalization_config_path() -> PathBuf {
    app_data_root().join("personalization.json")
}

pub(crate) fn workspace_config_path() -> PathBuf {
    app_data_root().join("workspace.conf")
}

pub(crate) fn project_session_config_path() -> PathBuf {
    app_data_root().join("projects.conf")
}

pub(crate) fn schedule_config_path() -> PathBuf {
    app_data_root().join("schedules.json")
}

pub(crate) fn sidecar_config_path() -> PathBuf {
    app_data_root().join("sidecars.conf")
}

pub(crate) fn web_search_config_path() -> PathBuf {
    app_data_root().join("web-search.conf")
}

pub(crate) fn mcp_config_path() -> PathBuf {
    app_data_root().join("mcp-servers.json")
}

pub(crate) fn mcp_catalog_cache_path() -> PathBuf {
    app_data_root().join("mcp-catalog.json")
}

pub(crate) fn rag_index_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("rag-index.tsv")
}

pub(crate) fn lancedb_export_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("lancedb-records.jsonl")
}

pub(crate) fn lancedb_database_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("lancedb")
}

pub(crate) fn memory_lancedb_root_for(workspace_root: &Path, project_id: &str) -> PathBuf {
    let digest = sha256_hex(project_id.as_bytes());
    workspace_root
        .join(".cindx")
        .join("memory-lancedb")
        .join(&digest[..24])
}

pub(crate) fn memory_lancedb_database_path_for(workspace_root: &Path, project_id: &str) -> PathBuf {
    memory_lancedb_root_for(workspace_root, project_id).join("index")
}

pub(crate) fn memory_lancedb_manifest_path_for(workspace_root: &Path, project_id: &str) -> PathBuf {
    memory_lancedb_root_for(workspace_root, project_id).join("manifest.json")
}

pub(crate) fn graph_store_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("graph.tsv")
}

pub(crate) fn graph_state_for(
    workspace_root: &Path,
    focus_paths: &[String],
) -> Result<GraphStateView, String> {
    let store = FileGraphStore::open(graph_store_path_for(workspace_root))
        .map_err(|error| error.to_string())?;
    let all_nodes = store.nodes();
    let all_edges = store.edges();
    let total_nodes = all_nodes.len();
    let total_edges = all_edges.len();
    let focus_paths = focus_paths
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut selected_ids = all_nodes
        .iter()
        .filter(|node| {
            if focus_paths.is_empty() {
                node.kind.label() == "file"
            } else {
                focus_paths.contains(node.label.as_str())
                    || focus_paths.contains(node.provenance.source_path.as_str())
            }
        })
        .take(if focus_paths.is_empty() { 18 } else { 40 })
        .map(|node| node.id.clone())
        .collect::<BTreeSet<_>>();
    if selected_ids.is_empty() {
        selected_ids.extend(all_nodes.iter().take(24).map(|node| node.id.clone()));
    }
    for _ in 0..2 {
        let neighbors = all_edges
            .iter()
            .filter(|edge| selected_ids.contains(&edge.from) || selected_ids.contains(&edge.to))
            .flat_map(|edge| [edge.from.clone(), edge.to.clone()])
            .collect::<Vec<_>>();
        for id in neighbors {
            if selected_ids.len() >= 80 {
                break;
            }
            selected_ids.insert(id);
        }
    }
    let mut nodes = all_nodes
        .into_iter()
        .filter(|node| selected_ids.contains(&node.id))
        .map(|node| GraphNodeView {
            focused: focus_paths.contains(node.label.as_str())
                || focus_paths.contains(node.provenance.source_path.as_str()),
            id: node.id,
            kind: node.kind.label().to_string(),
            label: node.label,
            source_path: node.provenance.source_path,
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        right
            .focused
            .cmp(&left.focused)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.label.cmp(&right.label))
    });
    let visible_ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut edges = all_edges
        .into_iter()
        .filter(|edge| {
            visible_ids.contains(edge.from.as_str()) && visible_ids.contains(edge.to.as_str())
        })
        .map(|edge| GraphEdgeView {
            id: edge.id,
            from: edge.from,
            to: edge.to,
            kind: edge.kind.label().to_string(),
        })
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    edges.truncate(140);
    Ok(GraphStateView {
        total_nodes,
        total_edges,
        nodes,
        edges,
    })
}

pub(crate) fn empty_graph_state() -> GraphStateView {
    GraphStateView {
        total_nodes: 0,
        total_edges: 0,
        nodes: Vec::new(),
        edges: Vec::new(),
    }
}

pub(crate) fn context_checkpoint_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("context-checkpoint.md")
}

pub(crate) fn context_checkpoint_path_for_session(
    workspace_root: &Path,
    session_id: Option<&str>,
) -> PathBuf {
    let segment = session_id
        .filter(|value| !value.trim().is_empty())
        .map(safe_path_segment)
        .unwrap_or_else(|| "project".to_string());
    workspace_root
        .join(".cindx")
        .join("context")
        .join(format!("{segment}.md"))
}

pub(crate) fn context_checkpoint_manifest_path_for_session(
    workspace_root: &Path,
    session_id: Option<&str>,
) -> PathBuf {
    context_checkpoint_path_for_session(workspace_root, session_id).with_extension("coverage.json")
}

pub(crate) fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .take(160)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn agent_trace_export_path_for(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".cindx").join("agent-trace.jsonl")
}

pub(crate) fn open_rag_adapter_for(workspace_root: &Path) -> Result<FileRagAdapter, String> {
    FileRagAdapter::open(rag_index_path_for(workspace_root)).map_err(|error| error.to_string())
}

pub(crate) fn workspace_knowledge_cache_key(workspace_root: &Path) -> String {
    fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .display()
        .to_string()
}

pub(crate) fn cached_rag_adapter_for(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<(FileRagAdapter, bool), String> {
    let key = workspace_knowledge_cache_key(workspace_root);
    if let Some(entry) = state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?
        .get(&key)
        .filter(|entry| entry.validated_at.elapsed() <= WORKSPACE_KNOWLEDGE_CACHE_TTL)
        .cloned()
    {
        return Ok((entry.adapter, true));
    }
    open_rag_adapter_for(workspace_root).map(|adapter| (adapter, false))
}

pub(crate) fn cache_rag_adapter(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
    adapter: &FileRagAdapter,
) -> Result<(), String> {
    let key = workspace_knowledge_cache_key(workspace_root);
    let graph_path = graph_store_path_for(workspace_root);
    let graph_store = graph_path
        .exists()
        .then(|| FileGraphStore::open(&graph_path).map_err(|error| error.to_string()))
        .transpose()?;
    let mut cache = state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?;
    cache.retain(|_, entry| entry.validated_at.elapsed() <= WORKSPACE_KNOWLEDGE_CACHE_TTL);
    if !cache.contains_key(&key) && cache.len() >= WORKSPACE_KNOWLEDGE_CACHE_MAX_ENTRIES {
        let oldest = cache
            .iter()
            .max_by_key(|(_, entry)| entry.validated_at.elapsed())
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            cache.remove(&oldest);
        }
    }
    cache.insert(
        key,
        WorkspaceKnowledgeCacheEntry {
            adapter: adapter.clone(),
            graph_store,
            validated_at: Instant::now(),
        },
    );
    Ok(())
}

pub(crate) fn cached_graph_store_for(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<Option<FileGraphStore>, String> {
    let key = workspace_knowledge_cache_key(workspace_root);
    if let Some(graph_store) = state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?
        .get(&key)
        .filter(|entry| entry.validated_at.elapsed() <= WORKSPACE_KNOWLEDGE_CACHE_TTL)
        .and_then(|entry| entry.graph_store.clone())
    {
        return Ok(Some(graph_store));
    }
    let graph_path = graph_store_path_for(workspace_root);
    if !graph_path.exists() {
        return Ok(None);
    }
    FileGraphStore::open(graph_path)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(crate) fn invalidate_workspace_knowledge_cache(
    state: &tauri::State<'_, AppState>,
    workspace_root: &Path,
) -> Result<(), String> {
    state
        .workspace_knowledge_cache
        .lock()
        .map_err(|error| format!("workspace knowledge cache lock poisoned: {error}"))?
        .remove(&workspace_knowledge_cache_key(workspace_root));
    Ok(())
}

pub(crate) fn index_graph_chunks(
    workspace_root: &Path,
    chunks: &[RagChunk],
) -> Result<(usize, usize), String> {
    index_graph_chunks_cancellable(workspace_root, chunks, || false)
}

pub(crate) fn index_graph_chunks_cancellable(
    workspace_root: &Path,
    chunks: &[RagChunk],
    mut should_cancel: impl FnMut() -> bool,
) -> Result<(usize, usize), String> {
    let graph_path = graph_store_path_for(workspace_root);
    let file_name = graph_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("graph.tsv");
    let temporary_path =
        graph_path.with_file_name(format!(".{file_name}.{}.tmp", unique_id("graph-index")));
    let result = (|| {
        let mut graph_store =
            FileGraphStore::open(&temporary_path).map_err(|error| error.to_string())?;
        let mut cancelled = false;
        {
            let extractions = chunks.iter().map_while(|chunk| {
                if should_cancel() {
                    cancelled = true;
                    None
                } else {
                    Some(extract_graph_from_chunk(chunk))
                }
            });
            graph_store
                .upsert_all(extractions)
                .map_err(|error| error.to_string())?;
        }
        if cancelled || should_cancel() {
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        let counts = (graph_store.nodes().len(), graph_store.edges().len());
        drop(graph_store);
        fs::rename(&temporary_path, &graph_path)
            .map_err(|error| format!("failed to replace graph store: {error}"))?;
        Ok(counts)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(crate) fn workspace_root() -> PathBuf {
    if let Some(root) = runtime_path_from_env("CINDX_DEFAULT_WORKSPACE") {
        if root.is_dir() {
            return root;
        }
    }
    if let Some(home) = user_home_directory() {
        if home.is_dir() {
            return home;
        }
    }
    std::env::current_dir()
        .ok()
        .filter(|path| path.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

pub(crate) fn app_data_root() -> PathBuf {
    app_data_root_for(
        runtime_path_from_env("CINDX_DATA_DIR"),
        user_home_directory(),
    )
}

pub(crate) fn app_data_root_for(override_root: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(root) = override_root {
        return root;
    }
    if let Some(home) = home {
        #[cfg(target_os = "macos")]
        return home
            .join("Library")
            .join("Application Support")
            .join("Cindx");
        #[cfg(not(target_os = "macos"))]
        return home.join(".cindx");
    }
    std::env::temp_dir().join("Cindx")
}

pub(crate) fn runtime_path_from_env(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir()
            .ok()
            .map(|current| current.join(path))
    }
}

pub(crate) fn user_home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn migrate_legacy_app_data() -> Result<(), std::io::Error> {
    if runtime_path_from_env("CINDX_DATA_DIR").is_some() {
        return Ok(());
    }
    let source = development_repo_root().join(".cindx");
    let destination = app_data_root();
    if source == destination || !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(&destination)?;
    secure_directory(&destination)?;
    for file_name in [
        "state.sqlite3",
        "state.sqlite3-shm",
        "state.sqlite3-wal",
        "provider.conf",
        "workspace.conf",
        "projects.conf",
        "schedules.json",
        "sidecars.conf",
        "mcp-servers.json",
        "mcp-catalog.json",
    ] {
        let source_path = source.join(file_name);
        let destination_path = destination.join(file_name);
        if source_path.is_file() && !destination_path.exists() {
            fs::copy(&source_path, &destination_path)?;
            secure_private_file(&destination_path)?;
        }
    }
    Ok(())
}

pub(crate) fn secure_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(crate) fn secure_private_file(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub(crate) fn append_startup_log(message: &str) {
    let root = app_data_root();
    if fs::create_dir_all(&root).is_err() {
        return;
    }
    let _ = secure_directory(&root);
    let path = root.join("startup.log");
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let Ok(mut file) = options.open(&path) else {
        return;
    };
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let _ = writeln!(file, "{timestamp_ms} {message}");
    let _ = secure_private_file(&path);
}

pub(crate) fn install_startup_panic_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        append_startup_log(&format!("panic: {info}"));
        previous(info);
    }));
}

pub(crate) fn phase3_task_id() -> TaskId {
    TaskId(PHASE3_TASK_ID.to_string())
}

pub(crate) fn phase4_task_id() -> TaskId {
    TaskId(PHASE4_TASK_ID.to_string())
}

pub(crate) fn phase5_task_id() -> TaskId {
    TaskId(PHASE5_TASK_ID.to_string())
}

pub(crate) fn phase6_task_id() -> TaskId {
    TaskId(PHASE6_TASK_ID.to_string())
}

pub(crate) fn phase7_task_id() -> TaskId {
    TaskId(PHASE7_TASK_ID.to_string())
}

pub(crate) fn phase8_task_id() -> TaskId {
    TaskId(PHASE8_TASK_ID.to_string())
}

pub(crate) fn phase15_task_id() -> TaskId {
    TaskId(PHASE15_TASK_ID.to_string())
}

pub(crate) fn phase16_task_id() -> TaskId {
    TaskId(PHASE16_TASK_ID.to_string())
}

pub(crate) fn context_task_ids() -> Vec<TaskId> {
    vec![
        phase3_task_id(),
        phase4_task_id(),
        phase5_task_id(),
        phase6_task_id(),
        phase7_task_id(),
        phase8_task_id(),
        phase15_task_id(),
        phase16_task_id(),
    ]
}

pub(crate) fn unique_id(prefix: &str) -> String {
    let counter = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", current_time_millis())
}

pub(crate) fn new_session_id() -> String {
    format!("sess_{}", uuid::Uuid::now_v7())
}

pub(crate) fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

pub(crate) fn parse_permission_decision(value: &str) -> Result<PermissionDecision, StorageError> {
    match value {
        "allow_once" => Ok(PermissionDecision::AllowOnce),
        "allow_for_session" => Ok(PermissionDecision::AllowForSession),
        "deny" => Ok(PermissionDecision::Deny),
        other => Err(StorageError::new(format!(
            "unknown permission decision: {other}"
        ))),
    }
}

pub(crate) fn event_kind_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::TaskStatusChanged => "Status",
        EventKind::ToolCallProposed => "Tool proposed",
        EventKind::PermissionRequested => "Permission requested",
        EventKind::PermissionResolved => "Permission resolved",
        EventKind::ToolCallStarted => "Tool started",
        EventKind::ToolCallFinished => "Tool finished",
        EventKind::ModelRequestStarted => "Model started",
        EventKind::ModelRequestFinished => "Model finished",
        EventKind::RetrievalPerformed => "Retrieval",
        EventKind::MessageAdded => "Message",
        EventKind::Error => "Error",
        _ => "Runtime event",
    }
}

pub(crate) fn timeline_event_label(event: &Event) -> String {
    if matches!(
        event.kind,
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished
    ) && event.metadata.contains_key("collaboration_id")
    {
        if let Some(stage) = event.metadata.get("stage") {
            return collaboration_stage_display_label(stage);
        }
    }
    event_kind_label(&event.kind).to_string()
}

pub(crate) fn collaboration_stage_display_label(stage: &str) -> String {
    match stage {
        "coordinator" => "Conductor".to_string(),
        "conductor_plan" => "Conductor".to_string(),
        "conductor_repair" => "Conductor repair".to_string(),
        "planner" => "Planner".to_string(),
        "arbiter" => "Arbiter".to_string(),
        "executor" => "Executor".to_string(),
        "reviewer" => "Reviewer".to_string(),
        "synthesizer" => "Synthesis".to_string(),
        _ => {
            if let Some(index) = stage.strip_prefix("candidate_") {
                format!("Candidate {index}")
            } else if let Some(index) = stage.strip_prefix("worker_") {
                format!("Worker {index}")
            } else {
                "Model collaboration".to_string()
            }
        }
    }
}

pub(crate) fn event_kind_ui_kind(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ToolCallProposed | EventKind::ToolCallStarted | EventKind::ToolCallFinished => {
            "tool"
        }
        EventKind::PermissionRequested | EventKind::PermissionResolved => "permission",
        EventKind::ModelRequestStarted | EventKind::ModelRequestFinished => "model",
        EventKind::RetrievalPerformed => "tool",
        _ => "message",
    }
}

pub(crate) fn event_state(kind: &EventKind, permission_is_pending: bool) -> &'static str {
    match kind {
        EventKind::PermissionRequested if permission_is_pending => "pending",
        EventKind::ToolCallProposed if permission_is_pending => "pending",
        EventKind::Error => "pending",
        _ => "done",
    }
}

pub(crate) fn message_role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "reviewer",
    }
}

pub(crate) fn message_role_from_label(value: &str) -> Option<MessageRole> {
    match value {
        "system" => Some(MessageRole::System),
        "user" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "tool" => Some(MessageRole::Tool),
        "reviewer" => Some(MessageRole::Reviewer),
        _ => None,
    }
}

pub(crate) fn tool_risk_label(risk: &ToolRisk) -> &'static str {
    match risk {
        ToolRisk::ReadOnly => "read_only",
        ToolRisk::WritesWorkspace => "writes_workspace",
        ToolRisk::ExecutesProcess => "executes_process",
        ToolRisk::UsesNetwork => "uses_network",
        ToolRisk::SensitiveContext => "sensitive_context",
        ToolRisk::Destructive => "destructive",
    }
}

pub(crate) fn tool_outcome_label(status: &ToolOutcomeStatus) -> &'static str {
    match status {
        ToolOutcomeStatus::Succeeded => "succeeded",
        ToolOutcomeStatus::Failed => "failed",
        ToolOutcomeStatus::Cancelled => "cancelled",
        ToolOutcomeStatus::Denied => "denied",
    }
}

pub(crate) fn normalized_config_value(value: &str) -> String {
    sanitize_config_value(value.trim())
}

pub(crate) fn normalized_agent_instructions(value: &str) -> String {
    let prompt = value.trim();
    if prompt.is_empty() || prompt == LEGACY_AGENT_SYSTEM_PROMPT {
        String::new()
    } else {
        prompt.chars().take(32_000).collect()
    }
}

pub(crate) fn normalized_current_time_context(value: &str) -> String {
    let context = sanitize_config_value(value.trim())
        .chars()
        .take(256)
        .collect::<String>();
    if context.is_empty() {
        format!("Unix time {} ms (UTC)", current_time_millis())
    } else {
        context
    }
}

pub(crate) fn add_image_generation_run_context(
    run_context: &mut Metadata,
    config: &ProviderConfig,
    prompt: &str,
) {
    if !prompt_requests_image_generation(prompt) || config.image_model.trim().is_empty() {
        return;
    }
    run_context.insert("image_generation_required".to_string(), "true".to_string());
    run_context.insert(
        "configured_image_model".to_string(),
        config.image_model.trim().to_string(),
    );
    run_context.insert(
        "configured_image_endpoint".to_string(),
        if config.image_endpoint.trim().is_empty() {
            config.base_url.trim().to_string()
        } else {
            config.image_endpoint.trim().to_string()
        },
    );
}

pub(crate) fn required_image_generation_satisfied(
    runtime: &agent_runtime::AgentLoopState,
    run_context: &Metadata,
) -> bool {
    if run_context
        .get("image_generation_required")
        .map(String::as_str)
        != Some("true")
    {
        return true;
    }
    runtime.messages.iter().any(|message| {
        matches!(message.role, MessageRole::Tool)
            && message.content.contains("tool=image.generate")
            && message.content.contains("status=succeeded")
    })
}

pub(crate) fn agent_runtime_context_for_run(run_context: &Metadata) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(current_time) = run_context.get("current_time") {
        sections.push(format!(
            "Current date and time: {current_time}\nTreat this time as authoritative for this turn."
        ));
    }
    if run_context
        .get("image_generation_required")
        .map(String::as_str)
        == Some("true")
    {
        let model = run_context
            .get("configured_image_model")
            .map(String::as_str)
            .unwrap_or("the configured image model");
        let endpoint = run_context
            .get("configured_image_endpoint")
            .map(String::as_str)
            .unwrap_or("the configured image endpoint");
        sections.push(format!(
            "Image generation policy (authoritative): this request requires raster image generation. You MUST use `image.generate`, which is locked to the user's Settings model `{model}` at `{endpoint}`. Never substitute a model, provider, shell command, browser workflow, direct HTTP request, SVG, emoji, CSS drawing, or text-only approximation for the requested generated image. The visual prompt is your responsibility; model and provider selection are not."
        ));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

pub(crate) fn collaboration_system_prompt_for_run(
    user_instructions: &str,
    run_context: &Metadata,
) -> String {
    let runtime_context = agent_runtime_context_for_run(run_context);
    compose_agent_system_prompt(Some(user_instructions), runtime_context.as_deref())
}

pub(crate) fn config_hex_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn config_hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn sanitize_config_value(value: &str) -> String {
    value.replace(['\n', '\r'], "")
}

pub(crate) fn sanitize_record_field(value: &str) -> String {
    sanitize_config_value(value).replace('\t', " ")
}

pub(crate) fn unique_config_id(prefix: &str, label: &str, existing: &[String]) -> String {
    let slug = slug_label(label);
    let base = format!("{prefix}-{slug}");
    if !existing.iter().any(|id| id == &base) {
        return base;
    }
    for index in 2..1000 {
        let candidate = format!("{base}-{index}");
        if !existing.iter().any(|id| id == &candidate) {
            return candidate;
        }
    }

    format!("{base}-{}", current_time_millis())
}

pub(crate) fn slug_label(label: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "item".to_string()
    } else {
        slug
    }
}

pub(crate) fn truncate_for_timeline(value: &str) -> String {
    const LIMIT: usize = 160;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }

    let mut truncated = value.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...");
    truncated
}
