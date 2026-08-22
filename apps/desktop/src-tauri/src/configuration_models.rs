use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) require_approval: bool,
    pub(crate) timeout_ms: u64,
    pub(crate) transport_type: String,
    pub(crate) command: Option<String>,
    pub(crate) args: Vec<String>,
    pub(crate) url: Option<String>,
    pub(crate) secret_keys: Vec<String>,
    pub(crate) tool_count: usize,
    pub(crate) refreshed_at_ms: Option<u64>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpStateView {
    pub(crate) servers: Vec<McpServerView>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServersInput {
    pub(crate) servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerInput {
    pub(crate) server: McpServerConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerPolicyInput {
    pub(crate) server_id: String,
    pub(crate) enabled: bool,
    pub(crate) require_approval: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillStateView {
    pub(crate) skills: Vec<SkillRecord>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillPreferenceInput {
    pub(crate) skill_id: String,
    pub(crate) enabled: bool,
    pub(crate) trusted: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillPackageInstallInput {
    pub(crate) data_base64: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillUrlInstallInput {
    pub(crate) url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderConfig {
    pub(crate) provider_id: String,
    pub(crate) provider_resource: String,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) model: String,
    pub(crate) conductor_model: String,
    pub(crate) planner_model: String,
    pub(crate) executor_model: String,
    pub(crate) reviewer_model: String,
    pub(crate) summarizer_model: String,
    pub(crate) fast_model: String,
    pub(crate) auto_model: String,
    pub(crate) pro_model: String,
    pub(crate) embedding_model: String,
    pub(crate) image_model: String,
    pub(crate) image_endpoint: String,
    pub(crate) voice_model: String,
    pub(crate) auth_verified_at_ms: Option<u64>,
    pub(crate) collaboration_policy: String,
    pub(crate) context_window_tokens: u64,
    pub(crate) agent_system_prompt: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct PersonalizationConfig {
    pub(crate) preferred_name: String,
    pub(crate) response_tone: String,
    pub(crate) response_length: String,
}

impl Default for PersonalizationConfig {
    fn default() -> Self {
        Self {
            preferred_name: String::new(),
            response_tone: "natural".to_string(),
            response_length: "balanced".to_string(),
        }
    }
}

pub(crate) fn default_agent_effort() -> String {
    AgentPolicy::Auto.label().to_string()
}

pub(crate) fn persisted_agent_policy(value: Option<&str>) -> Result<AgentPolicy, String> {
    match value {
        None => Ok(AgentPolicy::Auto),
        Some(value) => AgentPolicy::parse_persisted(value)
            .ok_or_else(|| format!("persisted agent policy is invalid: {value}")),
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        let defaults = provider_model_defaults(PROVIDER_OPENAI)
            .expect("OpenAI defaults must exist in the embedded provider catalog");
        let profile = resolve_provider_profile(PROVIDER_OPENAI, "", "", "");
        Self {
            provider_id: PROVIDER_OPENAI.to_string(),
            provider_resource: String::new(),
            base_url: profile.base_url,
            api_key: String::new(),
            model: defaults.chat.clone(),
            conductor_model: defaults.conductor.clone(),
            planner_model: defaults.planner.clone(),
            executor_model: defaults.executor.clone(),
            reviewer_model: defaults.reviewer.clone(),
            summarizer_model: defaults.summarizer.clone(),
            fast_model: String::new(),
            auto_model: String::new(),
            pro_model: String::new(),
            embedding_model: defaults.embedding.clone(),
            image_model: defaults.image.clone(),
            image_endpoint: profile.image_endpoint,
            voice_model: defaults.voice.clone(),
            auth_verified_at_ms: None,
            collaboration_policy: "auto_router".to_string(),
            context_window_tokens: defaults.context_window_tokens,
            agent_system_prompt: String::new(),
        }
    }
}

impl ProviderConfig {
    pub(crate) fn provider_profile(&self) -> ProviderProfile {
        resolve_provider_profile(
            &self.provider_id,
            &self.provider_resource,
            &self.base_url,
            &self.image_endpoint,
        )
    }

    pub(crate) fn is_ready(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.executor_model.trim().is_empty()
    }

    pub(crate) fn voice_is_ready(&self) -> bool {
        self.voice_transport() != ProviderVoiceTransport::Unavailable
            && !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.voice_model.trim().is_empty()
    }

    pub(crate) fn voice_transport(&self) -> ProviderVoiceTransport {
        provider_voice_transport(&self.provider_id, &self.base_url)
    }

    #[cfg(test)]
    pub(crate) fn supports_webrtc_voice(&self) -> bool {
        provider_supports_webrtc_voice(&self.provider_id, &self.base_url)
    }

    pub(crate) fn effort_default_model(&self, effort_label: &str) -> String {
        let pinned = match effort_label {
            "fast" => self.fast_model.trim(),
            "auto" => self.auto_model.trim(),
            "pro" => self.pro_model.trim(),
            _ => return String::new(),
        };
        if !pinned.is_empty() {
            return pinned.to_string();
        }
        provider_effort_default_model(&self.provider_id, effort_label).to_string()
    }

    pub(crate) fn model_for_role(&self, role: &ModelRole) -> String {
        if *role == ModelRole::Embedder {
            return embedding_model_for_provider(&self.base_url, &self.embedding_model);
        }
        let model = match role {
            ModelRole::Planner => &self.planner_model,
            ModelRole::Executor => &self.executor_model,
            ModelRole::Reviewer => &self.reviewer_model,
            ModelRole::Summarizer => &self.summarizer_model,
            ModelRole::Embedder => unreachable!("embedder handled above"),
        };

        if model.trim().is_empty() {
            self.model.clone()
        } else {
            model.clone()
        }
    }

    pub(crate) fn model_for_conductor(&self) -> String {
        if self.conductor_model.trim().is_empty() {
            self.model_for_role(&ModelRole::Planner)
        } else {
            self.conductor_model.clone()
        }
    }

    pub(crate) fn model_for_agent_policy(&self, policy: &OrchestrationPolicy) -> String {
        if *policy == OrchestrationPolicy::Single && !self.model.trim().is_empty() {
            self.model.clone()
        } else {
            self.model_for_role(&ModelRole::Executor)
        }
    }
}

pub(crate) fn embedding_model_for_provider(base_url: &str, configured_model: &str) -> String {
    let configured_model = configured_model.trim();
    if configured_model.is_empty() {
        return String::new();
    }
    let is_alibaba_cn =
        resolve_provider_profile("", "", base_url, "").provider_id == PROVIDER_ALIBABA_CN;
    if is_alibaba_cn && configured_model.eq_ignore_ascii_case(OPENAI_DEFAULT_EMBEDDING_MODEL) {
        DASHSCOPE_DEFAULT_EMBEDDING_MODEL.to_string()
    } else {
        configured_model.to_string()
    }
}

pub(crate) fn agent_model_for_run(config: &ProviderConfig, run_context: &Metadata) -> String {
    if let Some(model) = run_context
        .get("agent_model")
        .filter(|model| !model.trim().is_empty())
    {
        return model.clone();
    }
    let policy = run_context
        .get("collaboration_policy")
        .and_then(|label| parse_policy(label));
    policy
        .as_ref()
        .map(|policy| config.model_for_agent_policy(policy))
        .unwrap_or_else(|| config.model_for_role(&ModelRole::Executor))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceConfig {
    pub(crate) root: PathBuf,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: workspace_root(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidecarConfig {
    pub(crate) browser_path: String,
    pub(crate) computer_path: String,
    pub(crate) auto_configure: bool,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            browser_path: default_browser_sidecar_path().display().to_string(),
            computer_path: default_computer_sidecar_path().display().to_string(),
            auto_configure: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSessionConfig {
    pub(crate) active_project_id: String,
    pub(crate) active_session_id: String,
    pub(crate) projects: Vec<ProjectRecord>,
    pub(crate) sessions: Vec<SessionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectRecord {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) root: String,
    pub(crate) detail: String,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRecord {
    pub(crate) id: String,
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) title_state: SessionTitleState,
    pub(crate) detail: String,
    pub(crate) effort: String,
    pub(crate) seen_event_sequence: u64,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) archived_at_ms: Option<u64>,
}

pub(crate) fn is_schedule_execution_session(session: &SessionRecord) -> bool {
    session.detail == SCHEDULE_EXECUTION_SESSION_DETAIL
}

impl ProjectSessionConfig {
    pub(crate) fn default_for_root(root: &Path) -> Self {
        let now = current_time_millis();
        let project_id = "project-cindx".to_string();
        let session_id = new_session_id();
        Self {
            active_project_id: project_id.clone(),
            active_session_id: session_id.clone(),
            projects: vec![ProjectRecord {
                id: project_id.clone(),
                name: "Cindx".to_string(),
                root: root.display().to_string(),
                detail: "current workspace".to_string(),
                created_at_ms: now,
                updated_at_ms: now,
            }],
            sessions: vec![SessionRecord {
                id: session_id,
                project_id,
                name: "Runtime Session".to_string(),
                title_state: SessionTitleState::Pending,
                detail: "timeline + chat".to_string(),
                effort: default_agent_effort(),
                seen_event_sequence: 0,
                created_at_ms: now,
                updated_at_ms: now,
                archived_at_ms: None,
            }],
        }
    }

    pub(crate) fn active_project(&self) -> Option<&ProjectRecord> {
        self.projects
            .iter()
            .find(|project| project.id == self.active_project_id)
    }

    pub(crate) fn active_session(&self) -> Option<&SessionRecord> {
        self.sessions
            .iter()
            .find(|session| session.id == self.active_session_id)
    }

    pub(crate) fn ensure_consistent(&mut self, fallback_root: &Path) {
        if self.projects.is_empty() {
            *self = Self::default_for_root(fallback_root);
            return;
        }
        if !self
            .projects
            .iter()
            .any(|project| project.id == self.active_project_id)
        {
            self.active_project_id = self.projects[0].id.clone();
        }
        if !self.sessions.iter().any(|session| {
            session.project_id == self.active_project_id
                && session.archived_at_ms.is_none()
                && !is_schedule_execution_session(session)
        }) {
            let now = current_time_millis();
            let session_id = new_session_id();
            self.sessions.push(SessionRecord {
                id: session_id,
                project_id: self.active_project_id.clone(),
                name: "Runtime Session".to_string(),
                title_state: SessionTitleState::Pending,
                detail: "timeline + chat".to_string(),
                effort: default_agent_effort(),
                seen_event_sequence: 0,
                created_at_ms: now,
                updated_at_ms: now,
                archived_at_ms: None,
            });
        }
        if !self.sessions.iter().any(|session| {
            session.id == self.active_session_id
                && session.archived_at_ms.is_none()
                && !is_schedule_execution_session(session)
        }) {
            self.active_session_id = self
                .sessions
                .iter()
                .find(|session| {
                    session.project_id == self.active_project_id
                        && session.archived_at_ms.is_none()
                        && !is_schedule_execution_session(session)
                })
                .or_else(|| {
                    self.sessions.iter().find(|session| {
                        session.archived_at_ms.is_none() && !is_schedule_execution_session(session)
                    })
                })
                .map(|session| session.id.clone())
                .unwrap_or_default();
        }
    }
}
