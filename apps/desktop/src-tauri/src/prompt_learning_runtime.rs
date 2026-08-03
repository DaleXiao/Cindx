use crate::collaboration_models::PromptOfflineCase;
use crate::configuration_models::ProviderConfig;
use crate::event_security::{is_sensitive_assignment_key, redact_sensitive_text};
use crate::runtime_constants::PROMPT_MATCHED_EVALUATION_LANES;
use crate::app_state::AppState;
use agent_core::{Event, EventKind, ModelRole};
use agent_runtime::{AgentRunControl, RunBudget, RunStopReason};
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, AgentPolicy, ConductorPromptGenome, LearningAttribution,
    LearningEvidenceV1, LearningVerification, PromptDatasetCaseIdentityV1,
    PromptDatasetIdentityV1, PromptExecutionContextV1, PromptLearningCohortV1,
    PromptLearningEligibilityReceiptV1, PromptLearningPurpose,
    PromptLearningQualificationInput, PROMPT_EXECUTION_CONTEXT_SCHEMA_V1,
    PROMPT_LEARNING_REDACTION_SCHEMA_V1,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tools::ToolRegistry;

const PROMPT_EVALUATION_HARNESS_PROTOCOL: &str = "cindx.prompt-evaluation-harness.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptLearningText {
    pub(crate) text: String,
    pub(crate) redaction_verified: bool,
    pub(crate) residual_sensitive_data: bool,
}

pub(crate) fn redact_prompt_learning_text(value: &str) -> PromptLearningText {
    let text = redact_sensitive_text(value.trim());
    let idempotent = redact_sensitive_text(&text) == text;
    let residual_sensitive_data = prompt_text_contains_residual_secret(&text);
    PromptLearningText {
        text,
        redaction_verified: idempotent && !residual_sensitive_data,
        residual_sensitive_data,
    }
}

pub(crate) fn prompt_text_contains_residual_secret(value: &str) -> bool {
    if value.contains("-----BEGIN PRIVATE KEY-----")
        || value.contains("-----BEGIN RSA PRIVATE KEY-----")
        || value.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
    {
        return true;
    }
    value.lines().any(|line| {
        for (index, character) in line.char_indices() {
            if matches!(character, '=' | ':') && is_sensitive_assignment_key(&line[..index]) {
                let assigned = line[index + character.len_utf8()..].trim();
                if !assigned.is_empty() && !sensitive_assignment_is_redacted(assigned) {
                    return true;
                }
            }
        }
        let lower = line.to_ascii_lowercase();
        let mut bearer_offset = 0;
        while let Some(relative_index) = lower[bearer_offset..].find("bearer ") {
            let value_index = bearer_offset + relative_index + "bearer ".len();
            if !line[value_index..].trim_start().starts_with("[REDACTED]") {
                return true;
            }
            bearer_offset = value_index;
        }
        let prefixed = [
            ("github_pat_", 20_usize),
            ("ghp_", 16_usize),
            ("xoxb-", 16_usize),
            ("sk-", 16_usize),
            ("akia", 16_usize),
        ]
        .into_iter()
        .any(|(prefix, minimum)| {
            lower.find(prefix).is_some_and(|start| {
                let tail = &line[start..];
                let token_len = tail
                    .bytes()
                    .take_while(|byte| {
                        !byte.is_ascii_whitespace()
                            && !matches!(byte, b'\'' | b'"' | b',' | b';' | b')' | b']' | b'}')
                    })
                    .count();
                token_len >= minimum
            })
        });
        prefixed
            || line
                .split(|character: char| {
                    character.is_ascii_whitespace()
                        || matches!(
                            character,
                            '\\' | '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']'
                        )
                })
                .any(looks_like_jwt)
    })
}

fn sensitive_assignment_is_redacted(value: &str) -> bool {
    let normalized = value
        .trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '\\' | '"')
        });
    ["[REDACTED]", "Bearer [REDACTED]"]
        .into_iter()
        .any(|redacted| {
            normalized.strip_prefix(redacted).is_some_and(|remainder| {
                remainder.is_empty()
                    || remainder.starts_with(|character: char| {
                        character.is_ascii_whitespace()
                            || matches!(character, '\\' | '"' | ',' | '}' | ']')
                    })
            })
        })
}

fn looks_like_jwt(value: &str) -> bool {
    let segments = value.split('.').collect::<Vec<_>>();
    segments.len() == 3
        && segments.iter().all(|segment| {
            segment.len() >= 8
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

pub(crate) fn prompt_learning_case_is_safe(
    case: &PromptOfflineCase,
    config: &ProviderConfig,
) -> bool {
    let Ok(encoded) = serde_json::to_string(case) else {
        return false;
    };
    let api_key = config.api_key.trim();
    (api_key.len() < 8 || !encoded.contains(api_key))
        && !prompt_text_contains_residual_secret(&encoded)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_learning_receipt(
    purpose: PromptLearningPurpose,
    run_id: &str,
    project_id: &str,
    task_family: &str,
    objective: &PromptLearningText,
    run_events: &[&Event],
    terminal: &Event,
    steer_epoch: u64,
) -> PromptLearningEligibilityReceiptV1 {
    let evidence = LearningEvidenceV1::from_metadata(&terminal.metadata);
    let permission_denied = run_events.iter().any(|event| {
        event.kind == EventKind::PermissionResolved
            && event.metadata.get("decision").is_some_and(|decision| {
                !matches!(decision.as_str(), "allow_once" | "allow_for_session")
            })
    });
    let safety_violations = run_events
        .iter()
        .filter_map(|event| event.metadata.get("safety_violations"))
        .try_fold(0_u64, |maximum, value| {
            value.parse::<u64>().map(|parsed| maximum.max(parsed))
        })
        .unwrap_or(u64::MAX);
    let steer_epoch_text = steer_epoch.to_string();
    let workflow_prompt_contract_passed = run_events.iter().any(|event| {
        event_steer_epoch(event) == steer_epoch_text
            && event
                .metadata
                .get("anytime_prompt_learning_eligible")
                .map(String::as_str)
                == Some("true")
    });
    let verified_tool_postcondition = evidence.as_ref().is_some_and(|evidence| {
        evidence.attribution == LearningAttribution::Tool
            && evidence.verification == LearningVerification::Passed
    });
    PromptLearningEligibilityReceiptV1::qualify(PromptLearningQualificationInput {
        purpose,
        project_id,
        source_run_id: run_id,
        task_family,
        redacted_objective: &objective.text,
        redaction_schema: PROMPT_LEARNING_REDACTION_SCHEMA_V1,
        redaction_verified: objective.redaction_verified,
        residual_sensitive_data: objective.residual_sensitive_data,
        steer_epoch,
        prompt_contract_passed: workflow_prompt_contract_passed || verified_tool_postcondition,
        permission_denied,
        safety_violations,
        evidence: evidence.as_ref(),
    })
}

fn event_steer_epoch(event: &Event) -> String {
    event
        .metadata
        .get("steer_epoch")
        .cloned()
        .unwrap_or_else(|| "0".to_string())
}

pub(crate) fn prompt_dataset_identity(
    dataset: &[PromptOfflineCase],
    generation: u32,
) -> Result<PromptDatasetIdentityV1, String> {
    let scope = dataset
        .first()
        .map(|case| case.project_id.as_str())
        .unwrap_or("global");
    if dataset.iter().any(|case| case.project_id != scope) {
        return Err("prompt dataset crosses project scopes".to_string());
    }
    PromptDatasetIdentityV1::new(
        scope,
        generation,
        dataset
            .iter()
            .map(|case| PromptDatasetCaseIdentityV1 {
                case_id: case.id.clone(),
                objective_sha256: sha256_hex(case.objective.trim().as_bytes()),
                task_family_sha256: sha256_hex(case.task_class.trim().as_bytes()),
                split: case.split,
            })
            .collect(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_execution_context(
    config: &ProviderConfig,
    effort: &str,
    policy: &str,
    worker_models: &[String],
    agent_budget: usize,
    current_profile: &ConductorPromptGenome,
    workspace_root: &Path,
    matched_treatment_budget: RunBudget,
    control: &AgentRunControl,
) -> Result<PromptExecutionContextV1, String> {
    let (provider_sha256, model_pool_sha256, policy_sha256, budget_sha256) =
        prompt_configuration_fingerprints(
            config,
            effort,
            policy,
            worker_models,
            agent_budget,
            current_profile,
            matched_treatment_budget,
        )?;
    let tool_contract = prompt_evaluation_tool_contract_sha256(workspace_root);
    let source_revision = prompt_source_revision()?;
    let context = PromptExecutionContextV1 {
        schema: PROMPT_EXECUTION_CONTEXT_SCHEMA_V1.to_string(),
        provider_sha256,
        model_pool_sha256,
        harness_sha256: sha256_hex(PROMPT_EVALUATION_HARNESS_PROTOCOL.as_bytes()),
        system_prompt_sha256: sha256_hex(config.agent_system_prompt.as_bytes()),
        policy: AgentPolicy::parse_ingress(effort),
        policy_sha256,
        budget_sha256,
        tool_contract_sha256: tool_contract,
        source_revision_sha256: sha256_hex(source_revision.as_bytes()),
        workspace_revision_sha256: prompt_workspace_revision_sha256(workspace_root, control)?,
    };
    context.validate()?;
    Ok(context)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prompt_configuration_sha256(
    config: &ProviderConfig,
    effort: &str,
    policy: &str,
    worker_models: &[String],
    agent_budget: usize,
    current_profile: &ConductorPromptGenome,
    treatment_budget: RunBudget,
) -> Result<String, String> {
    let fingerprints = prompt_configuration_fingerprints(
        config,
        effort,
        policy,
        worker_models,
        agent_budget,
        current_profile,
        treatment_budget,
    )?;
    let system_prompt_sha256 = sha256_hex(config.agent_system_prompt.as_bytes());
    serde_json::to_vec(&(fingerprints, system_prompt_sha256))
        .map(|encoded| sha256_hex(&encoded))
        .map_err(|error| format!("prompt configuration serialization failed: {error}"))
}

pub(crate) fn prompt_evaluation_parent_budget() -> RunBudget {
    RunBudget::for_effort("pro")
}

pub(crate) fn prompt_configuration_sha256_is_valid(value: &str) -> bool {
    value.is_empty()
        || (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

pub(crate) fn prompt_evaluation_error_text(value: &str) -> String {
    redact_sensitive_text(value).chars().take(2_000).collect()
}

type PromptEvaluationConfiguration<'a> = (
    &'a String,
    &'a String,
    &'a Vec<String>,
    usize,
    &'a ConductorPromptGenome,
);

pub(crate) fn prompt_evaluation_request_configuration_sha256(
    state: &tauri::State<'_, AppState>,
    request: PromptEvaluationConfiguration<'_>,
) -> Result<String, String> {
    let config = state
        .provider_config
        .lock()
        .map_err(|error| format!("provider config lock poisoned: {error}"))?
        .clone();
    let (effort, policy, worker_models, agent_budget, profile) = request;
    prompt_configuration_sha256(
        &config,
        effort,
        policy,
        worker_models,
        agent_budget,
        profile,
        prompt_evaluation_parent_budget(),
    )
}

pub(crate) fn validate_prompt_evaluation_request_configuration(
    config: &ProviderConfig,
    expected_sha256: &str,
    request: PromptEvaluationConfiguration<'_>,
) -> Result<(), String> {
    if expected_sha256.is_empty() {
        return Ok(());
    }
    let (effort, policy, worker_models, agent_budget, profile) = request;
    let current = prompt_configuration_sha256(
        config,
        effort,
        policy,
        worker_models,
        agent_budget,
        profile,
        prompt_evaluation_parent_budget(),
    )?;
    (current == expected_sha256)
        .then_some(())
        .ok_or_else(|| {
            "prompt evaluation request execution context changed before replay".to_string()
        })
}

#[allow(clippy::too_many_arguments)]
fn prompt_configuration_fingerprints(
    config: &ProviderConfig,
    effort: &str,
    policy: &str,
    worker_models: &[String],
    agent_budget: usize,
    current_profile: &ConductorPromptGenome,
    treatment_budget: RunBudget,
) -> Result<(String, String, String, String), String> {
    let mut role_models = vec![
        ("chat", config.model.clone()),
        ("conductor", config.model_for_conductor()),
        ("planner", config.model_for_role(&ModelRole::Planner)),
        ("executor", config.model_for_role(&ModelRole::Executor)),
        ("reviewer", config.model_for_role(&ModelRole::Reviewer)),
        ("summarizer", config.model_for_role(&ModelRole::Summarizer)),
    ];
    role_models.sort();
    let mut workers = worker_models.to_vec();
    workers.sort();
    workers.dedup();
    let provider = format!(
        "{}\n{}\n{}\ncontext_window_tokens={}",
        config.provider_id.trim(),
        config.provider_resource.trim(),
        config.base_url.trim_end_matches('/'),
        config.context_window_tokens,
    );
    let policy_value = format!(
        "effort={effort}\npolicy={policy}\nagent_budget={agent_budget}\nprofile={}",
        prompt_genome_sha256(current_profile)?
    );
    Ok((
        sha256_hex(provider.as_bytes()),
        sha256_hex(
            serde_json::to_string(&(role_models, workers))
                .map_err(|error| format!("model pool serialization failed: {error}"))?
                .as_bytes(),
        ),
        sha256_hex(policy_value.as_bytes()),
        sha256_hex(
            format!(
                "parent={:?}\ncandidate={treatment_budget:?}\nallocation=candidates:parent_remaining/{};reviewers:post_candidate_remaining/2;accounting=absorbed",
                prompt_evaluation_parent_budget(),
                PROMPT_MATCHED_EVALUATION_LANES,
            )
            .as_bytes(),
        ),
    ))
}

pub(crate) fn prompt_source_revision() -> Result<&'static str, String> {
    if let Some(revision) = option_env!("CINDX_SOURCE_REVISION").filter(|revision| {
        matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Ok(revision);
    }
    #[cfg(test)]
    {
        Ok("test-source-revision")
    }
    #[cfg(not(test))]
    Err("verified Cindx source revision is unavailable for prompt replay".to_string())
}

pub(crate) fn prompt_event_sha256(event: &Event) -> Result<String, String> {
    let kind = match &event.kind {
        EventKind::TaskCreated => "task_created",
        EventKind::TaskStatusChanged => "task_status_changed",
        EventKind::MessageAdded => "message_added",
        EventKind::ModelRequestStarted => "model_request_started",
        EventKind::ModelRequestFinished => "model_request_finished",
        EventKind::ToolCallProposed => "tool_call_proposed",
        EventKind::ToolCallStarted => "tool_call_started",
        EventKind::ToolCallFinished => "tool_call_finished",
        EventKind::PermissionRequested => "permission_requested",
        EventKind::PermissionResolved => "permission_resolved",
        EventKind::RetrievalPerformed => "retrieval_performed",
        EventKind::Error => "error",
    };
    serde_json::to_vec(&(
        &event.id.0,
        &event.task_id.0,
        event.sequence,
        event.timestamp_ms,
        kind,
        &event.summary,
        &event.metadata,
    ))
    .map(|encoded| sha256_hex(&encoded))
    .map_err(|error| format!("prompt event serialization failed: {error}"))
}

pub(crate) fn prompt_learning_cohort(
    dataset: &[PromptOfflineCase],
    generation: u32,
    execution: PromptExecutionContextV1,
) -> Result<PromptLearningCohortV1, String> {
    PromptLearningCohortV1::new(prompt_dataset_identity(dataset, generation)?, execution)
}

pub(crate) fn prompt_evaluation_tool_contract_sha256(workspace_root: &Path) -> String {
    let mut specs = ToolRegistry::with_workspace_tools(workspace_root.to_path_buf()).specs();
    specs.sort_by(|left, right| left.name.cmp(&right.name));
    let canonical = specs
        .into_iter()
        .map(|spec| {
            format!(
                "{}|{}|{:?}|{:?}|{:?}|{}|{}|{:?}|{:?}",
                spec.name,
                spec.namespace,
                spec.risk,
                spec.source,
                spec.exposure,
                spec.input_schema_json,
                spec.output_schema_json.unwrap_or_default(),
                spec.effect_semantics,
                spec.execution_concurrency,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(canonical.as_bytes())
}

const PROMPT_WORKSPACE_FINGERPRINT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const PROMPT_WORKSPACE_FINGERPRINT_MAX_FILES: usize = 10_000;
const PROMPT_WORKSPACE_FINGERPRINT_MAX_ENTRIES: usize = 20_000;
const PROMPT_WORKSPACE_FINGERPRINT_MAX_DURATION: Duration = Duration::from_secs(30);
const PROMPT_WORKSPACE_FINGERPRINT_POLL_INTERVAL: Duration = Duration::from_millis(20);

struct WorkspaceFingerprintGuard<'a> {
    control: &'a AgentRunControl,
    deadline: Instant,
}

impl<'a> WorkspaceFingerprintGuard<'a> {
    fn new(control: &'a AgentRunControl) -> Self {
        Self {
            control,
            deadline: Instant::now() + PROMPT_WORKSPACE_FINGERPRINT_MAX_DURATION,
        }
    }

    fn check(&self) -> Result<(), String> {
        match self.control.stop_reason() {
            Some(RunStopReason::UserCancelled) => {
                return Err(MODEL_REQUEST_CANCELLED.to_string());
            }
            Some(reason) => {
                return Err(format!(
                    "prompt workspace fingerprint stopped: {}",
                    reason.code()
                ));
            }
            None => {}
        }
        if Instant::now() >= self.deadline {
            return Err("prompt workspace fingerprint exceeded its time budget".to_string());
        }
        Ok(())
    }
}

enum GitStreamMessage {
    Chunk(Vec<u8>),
    Failed(String),
    Finished,
}

fn stream_git_command<F>(
    workspace_root: &Path,
    args: &[&str],
    byte_limit: usize,
    guard: &WorkspaceFingerprintGuard<'_>,
    label: &str,
    mut consume: F,
) -> Result<(), String>
where
    F: FnMut(&[u8]) -> Result<(), String>,
{
    guard.check()?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{label} failed to start: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label} has no output stream"))?;
    let (sender, receiver) = mpsc::sync_channel::<GitStreamMessage>(4);
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut buffer = [0_u8; 16 * 1024];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        let _ = sender.send(GitStreamMessage::Finished);
                        return;
                    }
                    Ok(read) => {
                        if sender
                            .send(GitStreamMessage::Chunk(buffer[..read].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(GitStreamMessage::Failed(error.to_string()));
                        return;
                    }
                }
            }
        });
        let mut streamed = 0_usize;
        let mut status = None;
        let mut result = Ok(());
        while result.is_ok() {
            if let Err(error) = guard.check() {
                result = Err(error);
                break;
            }
            match receiver.recv_timeout(PROMPT_WORKSPACE_FINGERPRINT_POLL_INTERVAL) {
                Ok(GitStreamMessage::Chunk(chunk)) => {
                    streamed = streamed.saturating_add(chunk.len());
                    if streamed > byte_limit {
                        result = Err(format!("{label} output exceeded its byte bound"));
                    } else if let Err(error) = consume(&chunk) {
                        result = Err(error);
                    }
                }
                Ok(GitStreamMessage::Failed(error)) => {
                    result = Err(format!("{label} read failed: {error}"));
                }
                Ok(GitStreamMessage::Finished) => loop {
                    if let Err(error) = guard.check() {
                        result = Err(error);
                        break;
                    }
                    match child.try_wait() {
                        Ok(Some(exit_status)) => {
                            status = Some(exit_status);
                            break;
                        }
                        Ok(None) => std::thread::sleep(PROMPT_WORKSPACE_FINGERPRINT_POLL_INTERVAL),
                        Err(error) => {
                            result = Err(format!("{label} wait failed: {error}"));
                            break;
                        }
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    result = Err(format!("{label} output reader stopped unexpectedly"));
                }
            }
            if status.is_some() {
                break;
            }
        }
        if result.is_err() {
            let _ = child.kill();
        }
        if status.is_none() {
            match child.wait() {
                Ok(exit_status) => status = Some(exit_status),
                Err(error) if result.is_ok() => {
                    result = Err(format!("{label} wait failed: {error}"));
                }
                Err(_) => {}
            }
        }
        drop(receiver);
        if reader.join().is_err() && result.is_ok() {
            result = Err(format!("{label} output reader panicked"));
        }
        if result.is_ok() && status.is_none_or(|exit_status| !exit_status.success()) {
            result = Err(format!("{label} command failed"));
        }
        result
    })
}

fn prompt_workspace_revision_sha256(
    workspace_root: &Path,
    control: &AgentRunControl,
) -> Result<String, String> {
    let guard = WorkspaceFingerprintGuard::new(control);
    guard.check()?;
    let canonical = workspace_root
        .canonicalize()
        .map_err(|error| format!("prompt workspace cannot be canonicalized: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"cindx.prompt-workspace.v2\0");
    hasher.update(canonical.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    let mut content_hasher = Sha256::new();
    let mut byte_count = 0_u64;
    if hash_git_workspace(&canonical, &mut content_hasher, &mut byte_count, &guard).is_err() {
        guard.check()?;
        content_hasher = Sha256::new();
        byte_count = 0;
        let mut files = Vec::new();
        let mut entry_count = 0;
        collect_bounded_workspace_files(
            &canonical,
            &mut files,
            &mut entry_count,
            PROMPT_WORKSPACE_FINGERPRINT_MAX_ENTRIES,
            &guard,
        )?;
        for file in files {
            hash_workspace_file(
                &canonical,
                &file,
                &mut content_hasher,
                &mut byte_count,
                &guard,
            )?;
        }
    }
    hasher.update(content_hasher.finalize());
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_git_workspace(
    workspace_root: &Path,
    hasher: &mut Sha256,
    byte_count: &mut u64,
    guard: &WorkspaceFingerprintGuard<'_>,
) -> Result<(), String> {
    let top_level = git_output(
        workspace_root,
        &["rev-parse", "--show-toplevel"],
        16 * 1024,
        guard,
    )?;
    let top_level =
        String::from_utf8(top_level).map_err(|_| "Git workspace root is not UTF-8".to_string())?;
    let top_level = PathBuf::from(top_level.trim());
    hash_git_command(
        workspace_root,
        &["rev-parse", "HEAD"],
        hasher,
        byte_count,
        guard,
    )?;
    hash_git_command(
        workspace_root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "HEAD",
            "--",
            ".",
        ],
        hasher,
        byte_count,
        guard,
    )?;
    let untracked = git_output(
        workspace_root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
            "--",
            ".",
        ],
        2 * 1024 * 1024,
        guard,
    )?;
    let mut path_count = 0_usize;
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        path_count = path_count.saturating_add(1);
        if path_count > PROMPT_WORKSPACE_FINGERPRINT_MAX_FILES {
            return Err("prompt workspace has too many untracked files to fingerprint".to_string());
        }
        let path = std::str::from_utf8(path)
            .map_err(|_| "untracked workspace path is not UTF-8".to_string())?;
        hash_workspace_file(
            &top_level,
            &top_level.join(path),
            hasher,
            byte_count,
            guard,
        )?;
    }
    Ok(())
}

fn hash_git_command(
    workspace_root: &Path,
    args: &[&str],
    hasher: &mut Sha256,
    byte_count: &mut u64,
    guard: &WorkspaceFingerprintGuard<'_>,
) -> Result<(), String> {
    let remaining = PROMPT_WORKSPACE_FINGERPRINT_MAX_BYTES.saturating_sub(*byte_count);
    let limit = usize::try_from(remaining).unwrap_or(usize::MAX);
    stream_git_command(
        workspace_root,
        args,
        limit,
        guard,
        "Git workspace fingerprint",
        |chunk| {
            *byte_count = byte_count.saturating_add(chunk.len() as u64);
            hasher.update(chunk);
            Ok(())
        },
    )?;
    hasher.update(b"\0");
    Ok(())
}

fn git_output(
    workspace_root: &Path,
    args: &[&str],
    limit: usize,
    guard: &WorkspaceFingerprintGuard<'_>,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(limit.min(16 * 1024));
    if let Err(error) = stream_git_command(
        workspace_root,
        args,
        limit,
        guard,
        "Git workspace query",
        |chunk| {
            output.extend_from_slice(chunk);
            Ok(())
        },
    ) {
        if error == MODEL_REQUEST_CANCELLED || error.contains("time budget") {
            return Err(error);
        }
        return Err("Git workspace query failed or exceeded its bound".to_string());
    }
    Ok(output)
}

fn collect_bounded_workspace_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    entry_count: &mut usize,
    entry_limit: usize,
    guard: &WorkspaceFingerprintGuard<'_>,
) -> Result<(), String> {
    guard.check()?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("prompt workspace cannot be enumerated: {error}"))?
    {
        guard.check()?;
        let entry =
            entry.map_err(|error| format!("prompt workspace entry cannot be read: {error}"))?;
        *entry_count = entry_count.saturating_add(1);
        if *entry_count > entry_limit {
            return Err("prompt workspace has too many entries to fingerprint".to_string());
        }
        entries.push(entry);
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("prompt workspace file type is unavailable: {error}"))?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(
                    ".git"
                        | "node_modules"
                        | "target"
                        | "dist"
                        | "build"
                        | ".next"
                        | ".cache"
                        | "coverage"
                )
            ) {
                continue;
            }
            collect_bounded_workspace_files(&path, files, entry_count, entry_limit, guard)?;
        } else {
            files.push(path);
            if files.len() > PROMPT_WORKSPACE_FINGERPRINT_MAX_FILES {
                return Err("prompt workspace has too many files to fingerprint".to_string());
            }
        }
    }
    Ok(())
}

fn hash_workspace_file(
    root: &Path,
    path: &Path,
    hasher: &mut Sha256,
    byte_count: &mut u64,
    guard: &WorkspaceFingerprintGuard<'_>,
) -> Result<(), String> {
    guard.check()?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "prompt workspace file escaped its root".to_string())?;
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("prompt workspace metadata is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)
            .map_err(|error| format!("prompt workspace symlink is unreadable: {error}"))?;
        hasher.update(target.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        return Ok(());
    }
    if !metadata.is_file() {
        return Ok(());
    }
    let mut file = fs::File::open(path)
        .map_err(|error| format!("prompt workspace file cannot be opened: {error}"))?;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        guard.check()?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("prompt workspace file cannot be read: {error}"))?;
        if read == 0 {
            break;
        }
        *byte_count = byte_count.saturating_add(read as u64);
        if *byte_count > PROMPT_WORKSPACE_FINGERPRINT_MAX_BYTES {
            return Err("prompt workspace fingerprint exceeded its byte budget".to_string());
        }
        hasher.update(&buffer[..read]);
    }
    hasher.update(b"\0");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::PromptEvaluationSplit;

    #[test]
    fn prompt_learning_redaction_is_idempotent_and_rejects_residual_secrets() {
        let redacted = redact_prompt_learning_text(
            "authorization: Bearer secret-token\napi_key=sk-abcdefghijklmnopqrstuvwxyz",
        );
        assert!(!redacted.text.contains("secret-token"));
        assert!(!redacted.text.contains("sk-abcdefghijklmnopqrstuvwxyz"));
        assert!(redacted.redaction_verified);
        assert!(!redacted.residual_sensitive_data);
        assert!(prompt_text_contains_residual_secret(
            "Bearer still-visible-secret"
        ));
        assert!(prompt_text_contains_residual_secret(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyLTEyMyJ9.c2lnbmF0dXJlMTIz"
        ));
        assert!(prompt_text_contains_residual_secret(
            "-----BEGIN PRIVATE KEY-----"
        ));
    }

    #[test]
    fn prompt_learning_redacts_common_assignment_secrets() {
        for (key, secret) in [
            (
                "AWS_SECRET_ACCESS_KEY",
                "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            ),
            ("AWS_SESSION_TOKEN", "session-token-value"),
            ("GITHUB_TOKEN", "github-token-value"),
            ("WEBHOOK_SECRET", "webhook-secret-value"),
            ("SSH_PRIVATE_KEY", "private-key-value"),
        ] {
            let raw = format!("{key}={secret}");
            assert!(prompt_text_contains_residual_secret(&raw));
            let redacted = redact_prompt_learning_text(&raw);
            assert_eq!(redacted.text, format!("{key}=[REDACTED]"));
            assert!(redacted.redaction_verified);
            assert!(!redacted.residual_sensitive_data);
            assert!(!redacted.text.contains(secret));
        }
        assert!(!prompt_text_contains_residual_secret("max_token=4096"));
        assert_eq!(
            redact_prompt_learning_text("max_token=4096").text,
            "max_token=4096"
        );
    }

    #[test]
    fn residual_secret_scan_accepts_serialized_redaction_but_rejects_live_values() {
        assert!(!prompt_text_contains_residual_secret(
            r#"{"authorization":"Bearer [REDACTED]"}"#
        ));
        assert!(!prompt_text_contains_residual_secret(
            r#"{\"authorization\":\"[REDACTED]\"}"#
        ));
        assert!(prompt_text_contains_residual_secret(
            r#"{\"authorization\":\"Bearer [REDACTED]\",\"input\":\"eyJhbGciOiJIUzI1NiJ9.abcdefghijklmno.pqrstuvwxyz123456\"}"#
        ));
        assert!(prompt_text_contains_residual_secret(
            r#"{"authorization":"Bearer still-visible"}"#
        ));
    }

    #[test]
    fn background_prompt_evaluation_retains_the_pro_parent_budget() {
        assert_eq!(
            prompt_evaluation_parent_budget(),
            RunBudget::for_effort("pro")
        );
    }

    #[test]
    fn execution_context_changes_with_provider_model_policy_and_workspace() {
        let root = tempfile::tempdir().unwrap();
        let control = AgentRunControl::with_budget(RunBudget::for_effort("pro"));
        let profile = ConductorPromptGenome::seed_for_effort("auto");
        let mut config = ProviderConfig {
            api_key: "not-fingerprinted".to_string(),
            ..ProviderConfig::default()
        };
        let baseline = prompt_execution_context(
            &config,
            "auto",
            "auto_router",
            &[config.executor_model.clone()],
            2,
            &profile,
            root.path(),
            RunBudget::for_effort("auto"),
            &control,
        )
        .unwrap();
        config.api_key = "different-secret".to_string();
        let secret_changed = prompt_execution_context(
            &config,
            "auto",
            "auto_router",
            &[config.executor_model.clone()],
            2,
            &profile,
            root.path(),
            RunBudget::for_effort("auto"),
            &control,
        )
        .unwrap();
        assert_eq!(baseline, secret_changed);
        config.provider_id = "custom".to_string();
        assert_ne!(
            baseline,
            prompt_execution_context(
                &config,
                "auto",
                "auto_router",
                &[config.executor_model.clone()],
                2,
                &profile,
                root.path(),
                RunBudget::for_effort("auto"),
                &control,
            )
            .unwrap()
        );
        config.provider_id = ProviderConfig::default().provider_id;
        config.agent_system_prompt = "use a verified execution contract".to_string();
        assert_ne!(
            baseline,
            prompt_execution_context(
                &config,
                "auto",
                "auto_router",
                &[config.executor_model.clone()],
                2,
                &profile,
                root.path(),
                RunBudget::for_effort("auto"),
                &control,
            )
            .unwrap()
        );
        config.agent_system_prompt.clear();
        fs::write(root.path().join("objective.txt"), "first revision").unwrap();
        let workspace_changed = prompt_execution_context(
            &config,
            "auto",
            "auto_router",
            &[config.executor_model.clone()],
            2,
            &profile,
            root.path(),
            RunBudget::for_effort("auto"),
            &control,
        )
        .unwrap();
        assert_ne!(
            baseline.workspace_revision_sha256,
            workspace_changed.workspace_revision_sha256
        );
    }

    #[test]
    fn configured_or_assignment_secret_rejects_the_entire_learning_case() {
        let config = ProviderConfig {
            api_key: "provider-secret-value".to_string(),
            ..ProviderConfig::default()
        };
        let case = PromptOfflineCase {
            id: "case".to_string(),
            objective: "summarize provider-secret-value".to_string(),
            task_class: "analysis".to_string(),
            project_id: "project".to_string(),
            source_run_id: "run".to_string(),
            split: PromptEvaluationSplit::Train,
            learning_receipt: None,
            auto_teacher: None,
        };
        assert!(!prompt_learning_case_is_safe(&case, &config));
        let assignment_case = PromptOfflineCase {
            objective:
                "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            ..case
        };
        assert!(!prompt_learning_case_is_safe(
            &assignment_case,
            &ProviderConfig::default()
        ));
    }

    #[test]
    fn git_output_stops_when_the_stream_exceeds_its_bound() {
        let root = tempfile::tempdir().unwrap();
        let control = AgentRunControl::with_budget(RunBudget::for_effort("pro"));
        let guard = WorkspaceFingerprintGuard::new(&control);
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .arg(root.path())
            .status()
            .unwrap()
            .success());
        let large_value = "x".repeat(8 * 1024);
        assert!(Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["config", "test.large"])
            .arg(&large_value)
            .status()
            .unwrap()
            .success());

        assert_eq!(
            git_output(
                root.path(),
                &["config", "--get", "test.large"],
                1024,
                &guard,
            )
            .unwrap_err(),
            "Git workspace query failed or exceeded its bound"
        );
    }

    #[test]
    fn workspace_enumeration_stops_at_the_entry_bound() {
        let root = tempfile::tempdir().unwrap();
        let control = AgentRunControl::with_budget(RunBudget::for_effort("pro"));
        let guard = WorkspaceFingerprintGuard::new(&control);
        fs::write(root.path().join("a.txt"), "a").unwrap();
        fs::write(root.path().join("b.txt"), "b").unwrap();
        fs::write(root.path().join("c.txt"), "c").unwrap();
        let mut files = Vec::new();
        let mut entry_count = 0;

        assert_eq!(
            collect_bounded_workspace_files(root.path(), &mut files, &mut entry_count, 2, &guard)
                .unwrap_err(),
            "prompt workspace has too many entries to fingerprint"
        );
        assert_eq!(entry_count, 3);
        assert!(files.is_empty());
    }

    #[test]
    fn workspace_fingerprint_honors_parent_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let control = AgentRunControl::with_budget(RunBudget::for_effort("pro"));
        assert!(control.request_cancel());

        assert_eq!(
            prompt_workspace_revision_sha256(root.path(), &control).unwrap_err(),
            MODEL_REQUEST_CANCELLED
        );
    }
}
