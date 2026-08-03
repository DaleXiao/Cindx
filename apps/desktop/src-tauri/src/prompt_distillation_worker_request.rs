use crate::app_state::AppState;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::prompt_evolution_models::notify_prompt_evaluation_worker;
use crate::prompt_learning_runtime::{
    prompt_configuration_sha256_is_valid,
    prompt_evaluation_request_configuration_sha256 as request_configuration_sha256,
};
use crate::runtime_values::unique_id;
use agent_core::{EventKind, Metadata, TaskId};
use orchestrator::{ConductorPromptGenome, FrozenPromptProfileSnapshot, ProTeacherAttestationV1};
use serde::{Deserialize, Serialize};
use tauri::Manager;

pub(crate) const PROMPT_EVALUATION_REQUEST_SCHEMA: &str = "cindx.prompt-evaluation-request.v1";
pub(crate) const REQUEST_EVENT: &str = "Conductor prompt evaluation requested";
pub(crate) const REQUEST_METADATA_KEY: &str = "prompt_evaluation_request";
pub(crate) const REQUEST_ID_KEY: &str = "prompt_evaluation_request_id";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PromptEvaluationRequest {
    pub(crate) schema: String,
    pub(crate) request_id: String,
    pub(crate) task_id: String,
    pub(crate) run_context: Metadata,
    pub(crate) effort: String,
    pub(crate) policy: String,
    pub(crate) worker_models: Vec<String>,
    pub(crate) agent_budget: usize,
    pub(crate) current_profile: ConductorPromptGenome,
    #[serde(default)]
    pub(crate) pro_teacher_snapshot: Option<FrozenPromptProfileSnapshot>,
    #[serde(default)]
    pub(crate) configuration_sha256: String,
}

impl PromptEvaluationRequest {
    #[allow(clippy::too_many_arguments)]
    fn new(
        task_id: &TaskId,
        run_context: &Metadata,
        request_id: Option<String>,
        effort: String,
        policy: String,
        worker_models: Vec<String>,
        agent_budget: usize,
        current_profile: ConductorPromptGenome,
        pro_teacher_snapshot: Option<FrozenPromptProfileSnapshot>,
        configuration_sha256: String,
    ) -> Self {
        Self {
            schema: PROMPT_EVALUATION_REQUEST_SCHEMA.to_string(),
            request_id: request_id.unwrap_or_else(|| unique_id("prompt-evaluation-request")),
            task_id: task_id.0.clone(),
            run_context: persistent_prompt_evaluation_context(run_context),
            effort,
            policy,
            worker_models,
            agent_budget,
            current_profile,
            pro_teacher_snapshot,
            configuration_sha256,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema != PROMPT_EVALUATION_REQUEST_SCHEMA {
            return Err("unsupported prompt evaluation request schema".to_string());
        }
        if self.request_id.trim().is_empty()
            || self.task_id.trim().is_empty()
            || !matches!(self.effort.as_str(), "auto" | "pro")
            || self.worker_models.is_empty()
            || !(1..=3).contains(&self.agent_budget)
            || !prompt_configuration_sha256_is_valid(&self.configuration_sha256)
        {
            return Err("prompt evaluation request is incomplete".to_string());
        }
        self.current_profile.validate()?;
        if let Some(snapshot) = self.pro_teacher_snapshot.as_ref() {
            if self.effort != "auto" {
                return Err("Pro-to-Auto distillation request must target Auto".to_string());
            }
            ProTeacherAttestationV1::from_stable_snapshot(snapshot, &snapshot.genome.id)?;
        }
        Ok(())
    }

    pub(crate) fn scope_key(&self) -> (&str, &str, &str) {
        let project_id = self
            .run_context
            .get("project_id")
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("global");
        let track = if self.pro_teacher_snapshot.is_some() {
            "pro_to_auto_distillation"
        } else {
            "ordinary"
        };
        (project_id, self.effort.as_str(), track)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_prompt_pairwise_evaluation(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    run_context: &Metadata,
    request_id: Option<String>,
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
) -> Result<(), String> {
    enqueue_prompt_evaluation_request(
        app,
        task_id,
        run_context,
        request_id,
        effort,
        policy,
        worker_models,
        agent_budget,
        current_profile,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_prompt_pro_to_auto_distillation(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    run_context: &Metadata,
    request_id: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
    pro_teacher_snapshot: FrozenPromptProfileSnapshot,
) -> Result<(), String> {
    enqueue_prompt_evaluation_request(
        app,
        task_id,
        run_context,
        Some(request_id),
        "auto".to_string(),
        policy,
        worker_models,
        agent_budget,
        current_profile,
        Some(pro_teacher_snapshot),
    )
}

#[allow(clippy::too_many_arguments)]
fn enqueue_prompt_evaluation_request(
    app: &tauri::AppHandle,
    task_id: &TaskId,
    run_context: &Metadata,
    request_id: Option<String>,
    effort: String,
    policy: String,
    worker_models: Vec<String>,
    agent_budget: usize,
    current_profile: ConductorPromptGenome,
    pro_teacher_snapshot: Option<FrozenPromptProfileSnapshot>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let fingerprint = (
        &effort,
        &policy,
        &worker_models,
        agent_budget,
        &current_profile,
    );
    let configuration_sha256 = request_configuration_sha256(&state, fingerprint)?;
    let request = PromptEvaluationRequest::new(
        task_id,
        run_context,
        request_id,
        effort,
        policy,
        worker_models,
        agent_budget,
        current_profile,
        pro_teacher_snapshot,
        configuration_sha256,
    );
    request.validate()?;
    let payload = serde_json::to_string(&request)
        .map_err(|error| format!("prompt evaluation request serialization failed: {error}"))?;
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        REQUEST_EVENT,
        metadata_with_context(
            [
                (REQUEST_ID_KEY.to_string(), request.request_id.clone()),
                (REQUEST_METADATA_KEY.to_string(), payload),
                ("background_evaluation".to_string(), "true".to_string()),
                ("prompt_effort".to_string(), request.effort.clone()),
            ]
            .into_iter()
            .collect(),
            &request.run_context,
        ),
    )
    .map_err(|error| error.to_string())?;
    drop(store);
    notify_prompt_evaluation_worker();
    Ok(())
}

fn persistent_prompt_evaluation_context(run_context: &Metadata) -> Metadata {
    const SAFE_KEYS: [&str; 8] = [
        "agent_run_id",
        "agent_effort",
        "collaboration_policy",
        "project_id",
        "project_root",
        "prompt_profile",
        "session_id",
        "task_class",
    ];
    SAFE_KEYS
        .into_iter()
        .filter_map(|key| {
            run_context
                .get(key)
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect()
}
