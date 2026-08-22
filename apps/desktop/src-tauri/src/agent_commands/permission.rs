use crate::agent_run_engine::PreparedAgentExecution;
use crate::agent_preparation_runtime::effective_prompt_objective_for_messages;
use crate::suspended_run_runtime::{
    append_observations_to_suspended_run, remember_suspended_agent_run,
    suspended_agent_run_control_snapshot, suspended_agent_run_policy, take_suspended_agent_run,
};
use crate::*;
use agent_core::{
    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_IDENTITY_V1_SCHEMA,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};

const PERMISSION_TOOL_OBSERVATION_SCHEMA: &str = "cindx.permission-tool-observation.v1";
const PERMISSION_TOOL_OBSERVATION_PROVENANCE: &str = "runtime_permission_resolution";

const PERMISSION_RUN_CONTEXT_KEYS: &[&str] = &[
    AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY,
    "agent_run_id",
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
    "agent_effort",
    "agent_model",
    "requested_policy",
    "collaboration_policy",
    "current_time",
    "effective_prompt_objective",
    "steer_epoch",
    "prompt_contract_epoch",
    "task_class",
    "tool_requirement",
    "vision_required",
    "collaboration_profile",
    "conductor_contract",
    "verification_required",
    "queue_id",
    "recovery_resume_key",
    "recovery_attempts",
    "image_generation_required",
    "configured_image_model",
    "configured_image_endpoint",
];

fn restore_permission_run_context(run_context: &mut Metadata, request_metadata: &Metadata) {
    for key in PERMISSION_RUN_CONTEXT_KEYS {
        if let Some(value) = request_metadata.get(*key) {
            run_context.insert((*key).to_string(), value.clone());
        }
    }
}

fn permission_prompt_contract_epoch(run_context: &Metadata) -> u64 {
    run_context
        .get("prompt_contract_epoch")
        .or_else(|| run_context.get("steer_epoch"))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn permission_tool_observation_metadata(
    request_id: &PermissionRequestId,
    tool_call_id: &str,
    tool_name: &str,
    status: &ToolOutcomeStatus,
    tool_input: &str,
    risk: Option<&ToolRisk>,
    effect_spec: Option<&ToolSpec>,
    postcondition_evidence: Option<&agent_core::ToolPostconditionEvidence>,
    run_context: &Metadata,
) -> Metadata {
    let input_fingerprint = agent_runtime::tool_input_fingerprint(tool_name, tool_input);
    let mut metadata: Metadata = [
        ("kind".to_string(), "tool_observation".to_string()),
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), tool_name.to_string()),
        ("status".to_string(), tool_outcome_label(status).to_string()),
        ("permission_id".to_string(), request_id.0.clone()),
        (
            "permission_observation_schema".to_string(),
            PERMISSION_TOOL_OBSERVATION_SCHEMA.to_string(),
        ),
        (
            "permission_observation_provenance".to_string(),
            PERMISSION_TOOL_OBSERVATION_PROVENANCE.to_string(),
        ),
        (
            "tool_input_fingerprint".to_string(),
            input_fingerprint.clone(),
        ),
        (
            "prompt_contract_epoch".to_string(),
            permission_prompt_contract_epoch(run_context).to_string(),
        ),
    ]
    .into_iter()
    .collect();
    if matches!(status, ToolOutcomeStatus::Denied) {
        metadata.extend([
            (
                "action_denial_schema".to_string(),
                agent_runtime::ACTION_DENIAL_SCHEMA.to_string(),
            ),
            (
                "action_denial_kind".to_string(),
                "user_permission".to_string(),
            ),
            (
                "action_denial_code".to_string(),
                "user_permission_denied".to_string(),
            ),
            (
                "action_denial_recovery".to_string(),
                "finalize_blocked".to_string(),
            ),
        ]);
    }
    let completion_intent = agent_runtime::prompt_completion_intent(run_context);
    if let Some(witness) = agent_runtime::evidence_target_witness(
        tool_input,
        &completion_intent.target_anchors,
        tool_name,
        &input_fingerprint,
        permission_prompt_contract_epoch(run_context),
    ) {
        metadata.insert("evidence_target_witness".to_string(), witness);
    }
    if matches!(status, ToolOutcomeStatus::Succeeded) {
        if let Some(witness) = agent_runtime::postcondition_lineage_scope(run_context)
            .and_then(|lineage_scope| {
                agent_runtime::PersistedToolEffectWitness::capture_with_postcondition_evidence(
                    tool_name,
                    tool_input,
                    risk,
                    &lineage_scope,
                    effect_spec,
                    postcondition_evidence,
                )
            })
            .and_then(|witness| witness.encode())
        {
            metadata.insert(
                agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY.to_string(),
                witness,
            );
        }
    }
    metadata_with_context(metadata, run_context)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistedPermissionObservation {
    message_index: usize,
    permission_id: PermissionRequestId,
    call_id: agent_core::ToolCallId,
    tool_name: String,
    input_fingerprint: String,
    target_witness: Option<String>,
    effect_witness: Option<agent_runtime::PersistedToolEffectWitness>,
    status: ToolOutcomeStatus,
    observation: String,
}

fn persisted_permission_observation_payload_matches(
    left: &PersistedPermissionObservation,
    right: &PersistedPermissionObservation,
) -> bool {
    left.permission_id == right.permission_id
        && left.call_id == right.call_id
        && left.tool_name == right.tool_name
        && left.input_fingerprint == right.input_fingerprint
        && left.target_witness == right.target_witness
        && left.effect_witness == right.effect_witness
        && left.status == right.status
        && left.observation == right.observation
}

fn copy_replayed_contract_evidence_metadata(
    runtime: &agent_runtime::AgentLoopState,
    transcript: &mut [Message],
    message_index: usize,
) {
    let Some(source) = runtime.messages.last() else {
        return;
    };
    let Some(target) = transcript.get_mut(message_index) else {
        return;
    };
    for key in [
        agent_runtime::CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY,
        "contract_evidence_sequence",
    ] {
        if let Some(value) = source.metadata.get(key) {
            target.metadata.insert(key.to_string(), value.clone());
        }
    }
}

fn persisted_tool_outcome_status(value: &str) -> Option<ToolOutcomeStatus> {
    match value {
        "succeeded" => Some(ToolOutcomeStatus::Succeeded),
        "failed" => Some(ToolOutcomeStatus::Failed),
        "cancelled" => Some(ToolOutcomeStatus::Cancelled),
        "denied" => Some(ToolOutcomeStatus::Denied),
        _ => None,
    }
}

fn persisted_permission_observations(
    transcript: &[Message],
    run_context: &Metadata,
) -> Vec<PersistedPermissionObservation> {
    let Some(session_id) = run_context.get("session_id") else {
        return Vec::new();
    };
    let Some(agent_run_id) = run_context.get("agent_run_id") else {
        return Vec::new();
    };
    let prompt_contract_epoch = permission_prompt_contract_epoch(run_context).to_string();

    let candidates = transcript
        .iter()
        .enumerate()
        .filter_map(|(message_index, message)| {
            if message.role != MessageRole::Tool
                || message.metadata.get("kind").map(String::as_str) != Some("tool_observation")
                || message
                    .metadata
                    .get("permission_observation_schema")
                    .map(String::as_str)
                    != Some(PERMISSION_TOOL_OBSERVATION_SCHEMA)
                || message
                    .metadata
                    .get("permission_observation_provenance")
                    .map(String::as_str)
                    != Some(PERMISSION_TOOL_OBSERVATION_PROVENANCE)
                || message.metadata.get("session_id") != Some(session_id)
                || message.metadata.get("agent_run_id") != Some(agent_run_id)
                || message
                    .metadata
                    .get("prompt_contract_epoch")
                    .map(String::as_str)
                    != Some(prompt_contract_epoch.as_str())
                || message
                    .metadata
                    .get("permission_id")
                    .is_none_or(|value| value.trim().is_empty())
            {
                return None;
            }
            let permission_id = message
                .metadata
                .get("permission_id")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let call_id = message
                .metadata
                .get("tool_call_id")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let tool_name = message
                .metadata
                .get("tool")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let input_fingerprint = message
                .metadata
                .get("tool_input_fingerprint")
                .filter(|value| !value.trim().is_empty())?
                .clone();
            let status = persisted_tool_outcome_status(message.metadata.get("status")?)?;
            let effect_witness = match message
                .metadata
                .get(agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY)
            {
                Some(encoded) => Some(agent_runtime::PersistedToolEffectWitness::decode(encoded)?),
                None => None,
            };
            Some(PersistedPermissionObservation {
                message_index,
                permission_id: PermissionRequestId(permission_id),
                call_id: agent_core::ToolCallId(call_id),
                tool_name,
                input_fingerprint,
                target_witness: message.metadata.get("evidence_target_witness").cloned(),
                effect_witness,
                status,
                observation: message.content.clone(),
            })
        })
        .collect::<Vec<_>>();
    let mut first_by_id = BTreeMap::<String, usize>::new();
    let mut conflicting_ids = BTreeSet::new();
    for (index, observation) in candidates.iter().enumerate() {
        match first_by_id.get(&observation.permission_id.0).copied() {
            Some(first_index)
                if !persisted_permission_observation_payload_matches(
                    &candidates[first_index],
                    observation,
                ) =>
            {
                conflicting_ids.insert(observation.permission_id.0.clone());
            }
            Some(_) => {}
            None => {
                first_by_id.insert(observation.permission_id.0.clone(), index);
            }
        }
    }
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, observation)| {
            (!conflicting_ids.contains(&observation.permission_id.0)
                && first_by_id.get(&observation.permission_id.0) == Some(&index))
            .then_some(observation)
        })
        .collect()
}

fn current_permission_observation_boundary(
    observations: &[PersistedPermissionObservation],
    current_permission_ids: &BTreeSet<String>,
    transcript_len: usize,
) -> usize {
    observations
        .iter()
        .find(|observation| current_permission_ids.contains(&observation.permission_id.0))
        .map(|observation| observation.message_index)
        .unwrap_or(transcript_len)
}

fn permission_checkpoint_message_boundary(
    events: &[Event],
    recovery: &AgentRecoveryEnvelope,
    snapshot: &AgentTaskStateSnapshot,
) -> Option<usize> {
    let mut message_count = 0usize;
    for event in events {
        if event.kind == EventKind::MessageAdded {
            let role = event
                .metadata
                .get("role")
                .and_then(|role| message_role_from_label(role));
            let has_content = if role == Some(MessageRole::User) {
                event.metadata.contains_key("model_content")
                    || event.metadata.contains_key("content")
            } else {
                role.is_some() && event.metadata.contains_key("content")
            };
            if has_content {
                message_count = message_count.saturating_add(1);
            }
        }
        if event
            .metadata
            .get("recovery_resume_key")
            .map(String::as_str)
            != Some(recovery.identity.resume_key.as_str())
            || event.metadata.get("recovery_state").map(String::as_str) != Some("blocked")
        {
            continue;
        }
        let matches_snapshot = event
            .metadata
            .get("recovery_envelope")
            .and_then(|encoded| serde_json::from_str::<AgentRecoveryEnvelope>(encoded).ok())
            .is_some_and(|checkpoint| checkpoint.task_state.as_ref() == Some(snapshot));
        if matches_snapshot {
            return Some(message_count);
        }
    }
    None
}

fn restore_permission_snapshot_from_boundary(
    snapshot: &AgentTaskStateSnapshot,
    recovery_prompt: &str,
    effective_objective: &str,
    transcript: &[Message],
    boundary: usize,
) -> Option<(agent_runtime::AgentLoopState, usize)> {
    let boundary = boundary.min(transcript.len());
    snapshot
        .restore_with_effective_objective(
            recovery_prompt.to_string(),
            transcript[..boundary].to_vec(),
            effective_objective,
        )
        .ok()
        .map(|runtime| (runtime, boundary))
}

pub(crate) fn recovery_task_state_with_persisted_permission_denials(
    active_events: &[Event],
    run_context: &Metadata,
    recovery: &AgentRecoveryEnvelope,
    persisted_task_state: Option<&AgentTaskStateSnapshot>,
) -> Option<AgentTaskStateSnapshot> {
    let snapshot = recovery.task_state.as_ref()?;
    let mut transcript = agent_runtime_transcript_from_active_events(active_events);
    let observations = persisted_permission_observations(&transcript, run_context);
    let replay_boundary =
        permission_checkpoint_message_boundary(active_events, recovery, snapshot)?;
    let denied_observations = observations
        .iter()
        .filter(|observation| {
            observation.message_index >= replay_boundary
                && matches!(observation.status, ToolOutcomeStatus::Denied)
        })
        .collect::<Vec<_>>();
    if denied_observations.is_empty() {
        return None;
    }
    if let Some(persisted_task_state) = persisted_task_state.filter(|task_state| {
        denied_observations.iter().all(|observation| {
            task_state.task_contract.has_user_permission_finalization(
                &observation.tool_name,
                &observation.input_fingerprint,
            )
        })
    }) {
        return Some(persisted_task_state.clone());
    }
    let recovery_prompt = agent_recovery_prompt_from_active_events(active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let effective_objective = initial_agent_objective_from_events(active_events)
        .map(|initial| effective_prompt_objective_for_messages(&initial, &transcript))
        .unwrap_or_else(|| {
            crate::runtime_values::effective_agent_objective(run_context, &recovery_prompt)
                .to_string()
        });
    let (mut runtime, _) = restore_permission_snapshot_from_boundary(
        snapshot,
        &recovery_prompt,
        &effective_objective,
        &transcript,
        replay_boundary,
    )?;
    let tools = Vec::<ToolSpec>::new();
    for observation in denied_observations {
        AgentKernel::new(&mut runtime, &tools).apply_persisted_tool_observation_with_denial(
            observation.call_id.clone(),
            &observation.tool_name,
            &observation.input_fingerprint,
            observation.target_witness.as_deref(),
            None,
            &observation.status,
            None,
            &observation.observation,
            Some(&agent_runtime::AgentActionDenialFeedback::user_permission()),
        );
        copy_replayed_contract_evidence_metadata(
            &runtime,
            &mut transcript,
            observation.message_index,
        );
    }
    runtime.messages = transcript;
    Some(crate::agent_runtime_snapshot::capture_persistable_agent_task_state(&runtime))
}

#[tauri::command]
pub(crate) async fn resolve_agent_permission(
    app: tauri::AppHandle,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        resolve_agent_permission_blocking(&app, state, request_id, decision, session_id)
    })
    .await
    .map_err(|error| format!("agent permission resume failed to join: {error}"))?
}

pub(crate) fn resolve_agent_permission_blocking(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
) -> Result<AgentState, String> {
    let snapshot = suspended_agent_run_control_snapshot(&state, &session_id)?;
    let recovery_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let (request_effort, durable_recovery, applied_steer_epoch) = {
        let store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let persisted_effort = store
            .get_permission_request(&PermissionRequestId(request_id.clone()))
            .map_err(|error| error.to_string())?
            .and_then(|request| request.metadata.get("agent_effort").cloned());
        let effort = persisted_effort
            .as_deref()
            .map(|value| persisted_agent_policy(Some(value)))
            .transpose()?;
        let recovery = if snapshot.is_some() {
            None
        } else {
            peek_agent_recovery_envelope(&store, &recovery_context, &[AgentRecoveryState::Blocked])?
        };
        let events = agent_events_for_session(&store, &phase16_task_id(), Some(&session_id))
            .map_err(|error| error.to_string())?;
        let active_events = active_agent_events_for_session(&events, Some(&session_id));
        (
            effort,
            recovery,
            latest_applied_agent_steer_epoch(&active_events),
        )
    };
    let suspended_effort = suspended_agent_run_policy(&state, &session_id)?;
    let effort = match (request_effort, suspended_effort) {
        (Some(requested), Some(suspended)) if requested != suspended => {
            return Err("suspended agent policy does not match the permission request".to_string())
        }
        (Some(requested), _) => requested,
        (None, Some(suspended)) => suspended,
        (None, None) => AgentPolicy::Default,
    };
    let run_control_lease = if let Some(snapshot) = snapshot {
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), Some(snapshot))?
    } else if let Some(resources) = durable_recovery.and_then(|recovery| recovery.resource_snapshot)
    {
        begin_agent_run_control_from_persisted_resources(
            &state,
            &session_id,
            effort.label(),
            applied_steer_epoch,
            resources,
            false,
        )?
    } else {
        begin_agent_run_control_for_effort(&state, &session_id, effort.label(), None)?
    };
    let cancellation = run_control_lease.control();
    let outcome = resolve_agent_permission_blocking_inner(
        app,
        state.clone(),
        request_id,
        decision,
        session_id.clone(),
        effort,
        &cancellation,
    );
    let Err(original) = outcome else {
        return outcome;
    };
    let recovery_result = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))
        .and_then(|mut store| {
            pause_permission_recovery_after_handoff_error(&mut store, &recovery_context)
        });
    if let Err(recovery_error) = recovery_result {
        return Err(format!(
            "{original}; failed to preserve permission recovery after handoff error: {recovery_error}"
        ));
    }
    Err(original)
}

pub(crate) fn resolve_agent_permission_blocking_inner(
    app: &tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    request_id: String,
    decision: String,
    session_id: String,
    effort: AgentPolicy,
    cancellation: &Arc<AgentRunControl>,
) -> Result<AgentState, String> {
    let mut run_context = project_session_metadata_for_session(&state, Some(&session_id))?;
    let root = run_context
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or(active_workspace_root(&state)?);
    let config = clone_provider_config(&state)?;
    if !config.is_ready() {
        return agent_state_with_error_in_context(
            &state,
            &run_context,
            "Provider config is incomplete",
        );
    }
    let decision = parse_permission_decision(&decision).map_err(|error| error.to_string())?;
    let request_id = PermissionRequestId(request_id);
    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let request = store
        .get_permission_request(&request_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "permission request not found".to_string())?;
    if matches!(&decision, PermissionDecision::AllowForSession)
        && !permission_can_allow_session(&request)
    {
        return agent_state_for_session(
            &store,
            Some("this permission can only be allowed once".to_string()),
            Some(&session_id),
        )
        .map_err(|error| error.to_string());
    }
    restore_permission_run_context(&mut run_context, &request.metadata);
    let session_id_owned = run_context.get("session_id").cloned();
    let session_id = session_id_owned.as_deref();

    if request.task_id != phase16_task_id() {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the agent loop".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    let current =
        agent_state_for_session(&store, None, session_id).map_err(|error| error.to_string())?;
    if !current
        .pending_approvals
        .iter()
        .any(|approval| approval.request_id == request_id.0)
    {
        return agent_state_for_session(
            &store,
            Some("permission does not belong to the active session".to_string()),
            session_id,
        )
        .map_err(|error| error.to_string());
    }
    drop(store);

    let mut resolved_permission_ids = BTreeSet::from([request.id.0.clone()]);
    let mut resolved_observations = vec![resolve_agent_permission_request(
        &state,
        &request,
        &decision,
        "local-user",
        &root,
        &run_context,
        cancellation,
    )?];

    if matches!(&decision, PermissionDecision::AllowForSession) {
        let pending = {
            let store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            pending_agent_permissions_for_run(
                &store,
                &phase16_task_id(),
                session_id,
                run_context.get("agent_run_id").map(String::as_str),
            )
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|pending| permission_capability_matches(&request, pending))
            .collect::<Vec<_>>()
        };
        for pending_request in pending {
            resolved_permission_ids.insert(pending_request.id.0.clone());
            resolved_observations.push(resolve_agent_permission_request(
                &state,
                &pending_request,
                &PermissionDecision::AllowForSession,
                "session-grant",
                &root,
                &run_context,
                cancellation,
            )?);
        }
    }

    let hot_resume_ready = match append_observations_to_suspended_run(
        &state,
        session_id.unwrap_or_default(),
        &resolved_observations,
    ) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "hot permission recovery unavailable; rebuilding from the durable checkpoint: {error}"
            );
            false
        }
    };

    let store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    let pending = pending_agent_permissions_for_run(
        &store,
        &phase16_task_id(),
        session_id,
        run_context.get("agent_run_id").map(String::as_str),
    )
    .map_err(|error| error.to_string())?;
    if !pending.is_empty() {
        return agent_state_for_session(&store, None, session_id)
            .map_err(|error| error.to_string());
    }

    let recovery =
        peek_agent_recovery_envelope(&store, &run_context, &[AgentRecoveryState::Blocked])?;
    if let Some(recovery) = recovery.as_ref() {
        run_context = permission_recovery_run_context(
            &run_context,
            recovery,
            recovery.attempts.saturating_add(1),
        );
    }
    let events = match session_id {
        Some(session_id) => store.list_by_task_and_metadata_or_unscoped(
            &phase16_task_id(),
            "session_id",
            session_id,
        ),
        None => store.list_by_task(&phase16_task_id()),
    }
    .map_err(|error| error.to_string())?;
    let active_events = active_agent_events_for_session(&events, session_id);
    let prompt = latest_agent_prompt_from_active_events(&active_events)
        .unwrap_or_else(|| "Continue the agent task.".to_string());
    let recovery_prompt =
        agent_recovery_prompt_from_active_events(&active_events).unwrap_or_else(|| prompt.clone());
    let mut transcript = agent_runtime_transcript_from_active_events(&active_events);
    let persisted_permission_observations =
        persisted_permission_observations(&transcript, &run_context);
    let permission_observation_boundary = current_permission_observation_boundary(
        &persisted_permission_observations,
        &resolved_permission_ids,
        transcript.len(),
    );
    drop(store);

    let hot_suspended = if hot_resume_ready {
        match session_id {
            Some(session_id) => match take_suspended_agent_run(&state, session_id) {
                Ok(suspended) => suspended,
                Err(error) => {
                    eprintln!(
                        "hot permission handoff unavailable; rebuilding from the durable checkpoint: {error}"
                    );
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };
    if let Some(mut suspended) = hot_suspended {
        let claimed_context = match commit_permission_recovery_claim(
            &state,
            &request,
            &request_id,
            &decision,
            &run_context,
        ) {
            Ok(context) => context,
            Err(error) => {
                remember_suspended_agent_run(&state, suspended).map_err(|restore_error| {
                    format!(
                        "{error}; failed to restore the unclaimed suspended run: {restore_error}"
                    )
                })?;
                return Err(error);
            }
        };
        for (key, value) in &claimed_context {
            suspended.run_context.insert(key.clone(), value.clone());
        }
        let workspace_root = suspended.workspace_root.clone();
        let prepared = PreparedAgentExecution {
            base_run_context: suspended.run_context.clone(),
            run_context: suspended.run_context,
            runtime: suspended.runtime,
            prompt: suspended.prompt,
            collaboration: suspended.collaboration,
        };
        return continue_agent_loop(
            app,
            &state,
            &config,
            &workspace_root,
            prepared,
            effort,
            cancellation,
        );
    }

    let recovered_effective_objective = initial_agent_objective_from_events(&active_events)
        .map(|initial| effective_prompt_objective_for_messages(&initial, &transcript))
        .unwrap_or_else(|| {
            crate::runtime_values::effective_agent_objective(&run_context, &recovery_prompt)
                .to_string()
        });
    run_context.insert(
        "effective_prompt_objective".to_string(),
        recovered_effective_objective,
    );
    let recovered_prepared_task_state =
        crate::prepared_task_state_metadata::prepared_task_state_from_legacy_metadata(
            &run_context,
            &recovery_prompt,
            agent_runtime::prompt_completion_intent(&run_context),
        );
    crate::prepared_task_state_metadata::project_prepared_task_state_to_legacy_metadata(
        &recovered_prepared_task_state,
        &mut run_context,
    );
    let restored = recovery.as_ref().and_then(|recovery| {
        let snapshot = recovery.task_state.as_ref()?;
        let checkpoint_boundary =
            permission_checkpoint_message_boundary(&active_events, recovery, snapshot)
                .unwrap_or(permission_observation_boundary);
        restore_permission_snapshot_from_boundary(
            snapshot,
            &recovery_prompt,
            recovered_prepared_task_state.effective_objective(),
            &transcript,
            checkpoint_boundary,
        )
    });
    let (mut runtime, replay_boundary) = restored.unwrap_or_else(|| {
        (
            resume_agent_loop_from_messages(
                phase16_task_id(),
                recovery_prompt.clone(),
                transcript[..permission_observation_boundary].to_vec(),
                cancellation.runtime_config(),
            ),
            0,
        )
    });
    let registry = tool_registry_for_state(&state, &root)?;
    let catalog = registry.specs();
    let (tools, completion_intent) = crate::agent_loop_runtime::planned_agent_tools(
        &registry,
        &run_context,
        crate::runtime_values::effective_agent_objective(&run_context, &prompt),
        config.context_window_tokens,
    );
    apply_run_task_contract_with_completion_intent(
        &mut runtime,
        &run_context,
        &tools,
        &catalog,
        None,
        &completion_intent,
    )?;
    let replayed_permission_observations = persisted_permission_observations
        .iter()
        .filter(|observation| observation.message_index >= replay_boundary)
        .collect::<Vec<_>>();
    let mut recovered_goal_deltas = Vec::new();
    for resolved in &replayed_permission_observations {
        let risk = registry
            .get(&resolved.tool_name)
            .map(|tool| tool.spec().risk);
        let denial = matches!(resolved.status, ToolOutcomeStatus::Denied)
            .then(agent_runtime::AgentActionDenialFeedback::user_permission);
        let goal_delta = AgentKernel::new(&mut runtime, &tools)
            .apply_persisted_tool_observation_with_denial(
                resolved.call_id.clone(),
                &resolved.tool_name,
                &resolved.input_fingerprint,
                resolved.target_witness.as_deref(),
                resolved.effect_witness.as_ref(),
                &resolved.status,
                risk.as_ref(),
                &resolved.observation,
                denial.as_ref(),
            );
        recovered_goal_deltas.extend(goal_delta);
        copy_replayed_contract_evidence_metadata(&runtime, &mut transcript, resolved.message_index);
    }
    runtime.messages = transcript;
    let objective_epoch = run_context_steer_epoch(&run_context);
    persist_cold_permission_recovery(
        &recovered_goal_deltas,
        !replayed_permission_observations.is_empty(),
        || {
            let mut store = state
                .store
                .lock()
                .map_err(|error| format!("store lock poisoned: {error}"))?;
            crate::agent_runtime_snapshot::persist_agent_runtime_snapshot(
                &mut store,
                &runtime,
                &run_context,
            )
        },
    )?;
    run_context =
        commit_permission_recovery_claim(&state, &request, &request_id, &decision, &run_context)?;
    for delta in &recovered_goal_deltas {
        cancellation.record_goal_delta_at(objective_epoch, delta);
    }
    let prepared = PreparedAgentExecution {
        base_run_context: run_context.clone(),
        run_context,
        runtime,
        prompt,
        collaboration: None,
    };
    continue_agent_loop(app, &state, &config, &root, prepared, effort, cancellation)
}

fn permission_recovery_run_context(
    run_context: &Metadata,
    recovery: &AgentRecoveryEnvelope,
    attempts: u32,
) -> Metadata {
    let mut recovered = run_context.clone();
    recovered.insert(
        AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
        AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
    );
    recovered.insert(
        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
        recovery.identity.logical_run_id().to_string(),
    );
    recovered.insert(
        "recovery_resume_key".to_string(),
        recovery.identity.resume_key.clone(),
    );
    recovered.insert(
        "source_agent_run_id".to_string(),
        recovery.identity.source_run_id.clone(),
    );
    recovered.insert("recovery_attempts".to_string(), attempts.to_string());
    recovered.insert("continuation".to_string(), "true".to_string());
    if let Some(queue_id) = recovery.queue_id.as_ref() {
        recovered.insert("queue_id".to_string(), queue_id.clone());
    }
    recovered
}

fn commit_permission_recovery_claim(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    request_id: &PermissionRequestId,
    decision: &PermissionDecision,
    run_context: &Metadata,
) -> Result<Metadata, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    store
        .with_immediate_transaction(|store| {
            let claimed = claim_agent_recovery_envelope_in_transaction(
                store,
                run_context,
                &[AgentRecoveryState::Blocked],
                AgentRecoveryReason::PermissionResolved,
            )
            .map_err(StorageError::new)?;
            if run_context.contains_key("recovery_resume_key") && claimed.is_none() {
                return Err(StorageError::new(
                    "agent recovery checkpoint disappeared before permission resume",
                ));
            }
            let claimed_context = claimed
                .as_ref()
                .map(|recovery| {
                    permission_recovery_run_context(run_context, recovery, recovery.attempts)
                })
                .unwrap_or_else(|| run_context.clone());
            append_event(
                store,
                &request.task_id,
                EventKind::TaskStatusChanged,
                "Agent task resumed after permission",
                metadata_with_context(
                    [
                        ("permission_id".to_string(), request_id.0.clone()),
                        (
                            "decision".to_string(),
                            permission_decision_label(decision).to_string(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                    &claimed_context,
                ),
            )?;
            Ok(claimed_context)
        })
        .map_err(|error| error.to_string())
}

fn persist_cold_permission_recovery(
    recovered_goal_deltas: &[agent_runtime::AgentGoalDelta],
    recovered_state_changed: bool,
    commit_recovered_runtime: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if recovered_goal_deltas.is_empty() && !recovered_state_changed {
        return Ok(());
    }
    // Cold recovery has no in-memory suspended snapshot to fall back to. Make
    // the rebuilt contract state durable before the caller claims the recovery
    // and credits any Goal Delta to the live control.
    commit_recovered_runtime()?;
    Ok(())
}

fn persist_permission_resolution_rows(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    resolution: &PermissionResolution,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    store.resolve_permission_in_transaction(resolution)?;
    append_event(
        store,
        &request.task_id,
        EventKind::PermissionResolved,
        format!(
            "Permission {}",
            permission_decision_past_tense(&resolution.decision)
        ),
        metadata_with_context(
            [
                ("permission_id".to_string(), resolution.request_id.0.clone()),
                (
                    "decision".to_string(),
                    permission_decision_label(&resolution.decision).to_string(),
                ),
                ("tool".to_string(), request.action.clone()),
                ("resolved_by".to_string(), resolution.resolved_by.clone()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_denied_permission_resolution_rows(
    store: &mut SqliteStore,
    request: &PermissionRequest,
    resolution: &PermissionResolution,
    tool_call_id: &str,
    tool_name: &str,
    observation: &str,
    message_metadata: &Metadata,
    run_context: &Metadata,
) -> Result<(), StorageError> {
    persist_permission_resolution_rows(store, request, resolution, run_context)?;
    append_event(
        store,
        &request.task_id,
        EventKind::ToolCallFinished,
        "Agent tool denied",
        metadata_with_context(
            [
                ("tool_call_id".to_string(), tool_call_id.to_string()),
                ("tool".to_string(), tool_name.to_string()),
                ("status".to_string(), "denied".to_string()),
                (
                    "failure_code".to_string(),
                    "user_permission_denied".to_string(),
                ),
                ("output".to_string(), observation.to_string()),
            ]
            .into_iter()
            .collect(),
            run_context,
        ),
    )?;
    append_message_event_with_metadata(
        store,
        &request.task_id,
        MessageRole::Tool,
        observation,
        message_metadata.clone(),
    )
}

pub(crate) fn resolve_agent_permission_request(
    state: &tauri::State<'_, AppState>,
    request: &PermissionRequest,
    decision: &PermissionDecision,
    resolved_by: &str,
    root: &Path,
    run_context: &Metadata,
    cancellation: &Arc<AgentRunControl>,
) -> Result<ResolvedToolObservation, String> {
    let request_id = request.id.clone();
    let tool_call_id = request
        .metadata
        .get("tool_call_id")
        .cloned()
        .unwrap_or_else(|| unique_id("agent-tool"));
    let tool_name = request
        .metadata
        .get("tool_name")
        .cloned()
        .unwrap_or_else(|| request.action.clone());
    let tool_input = request
        .metadata
        .get("tool_input")
        .cloned()
        .unwrap_or_default();
    let resolution = PermissionResolution {
        request_id: request_id.clone(),
        decision: decision.clone(),
        resolved_at_ms: current_time_millis(),
        resolved_by: resolved_by.to_string(),
    };
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;

    if matches!(decision, PermissionDecision::Deny) {
        let status = ToolOutcomeStatus::Denied;
        let observation =
            observation_from_tool_result(&tool_name, "denied", "The user denied this tool call.");
        let message_metadata = permission_tool_observation_metadata(
            &request_id,
            &tool_call_id,
            &tool_name,
            &status,
            &tool_input,
            None,
            None,
            None,
            run_context,
        );
        store
            .with_immediate_transaction(|store| {
                persist_denied_permission_resolution_rows(
                    store,
                    request,
                    &resolution,
                    &tool_call_id,
                    &tool_name,
                    &observation,
                    &message_metadata,
                    run_context,
                )
            })
            .map_err(|error| error.to_string())?;
        return Ok(ResolvedToolObservation {
            call_id: agent_core::ToolCallId(tool_call_id),
            tool_name,
            input_json: tool_input,
            risk: None,
            effect_spec: None,
            postcondition_evidence: None,
            status,
            observation,
            image_paths: Vec::new(),
            message_metadata,
        });
    }

    store
        .with_immediate_transaction(|store| {
            persist_permission_resolution_rows(store, request, &resolution, run_context)
        })
        .map_err(|error| error.to_string())?;

    let (
        observation,
        status,
        image_paths,
        message_metadata,
        risk,
        effect_spec,
        postcondition_evidence,
    ) = if matches!(
        decision,
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession
    ) {
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId(tool_call_id.clone()),
            task_id: request.task_id.clone(),
            tool_name: tool_name.clone(),
            input_json: tool_input.clone(),
            proposed_by_model: "agent-loop".to_string(),
            metadata: Metadata::new(),
        };
        drop(store);
        let registry = tool_registry_for_state(state, root)?;
        let registered_tool = registry.get(&tool_name);
        let risk = registered_tool.map(|tool| tool.spec().risk);
        let effect_spec = registered_tool.map(|tool| tool.effect_spec(&invocation));
        let verification_invocation = invocation.clone();
        let (observation, status, image_paths, postcondition_evidence) =
            match execute_agent_tool_invocation_for_objective_epoch(
                state,
                &registry,
                invocation,
                root,
                run_context,
                cancellation,
                run_context_steer_epoch(run_context),
            )? {
                AgentToolInvocationOutcome::Completed(result) => {
                    let postcondition_evidence = registered_tool.and_then(|tool| {
                        tool.postcondition_evidence(&verification_invocation, &result)
                    });
                    (
                        observation_from_agent_tool_result(&tool_name, &result),
                        result.status.clone(),
                        tool_result_image_paths(&result),
                        postcondition_evidence,
                    )
                }
                AgentToolInvocationOutcome::RestartAfterSteer => (
                    observation_from_tool_result(
                        &tool_name,
                        "cancelled",
                        "Permission-approved tool call was superseded by user steering before its result could enter the agent transcript.",
                    ),
                    ToolOutcomeStatus::Cancelled,
                    Vec::new(),
                    None,
                ),
            };
        let mut store = state
            .store
            .lock()
            .map_err(|error| format!("store lock poisoned: {error}"))?;
        let message_metadata = permission_tool_observation_metadata(
            &request_id,
            &tool_call_id,
            &tool_name,
            &status,
            &tool_input,
            risk.as_ref(),
            effect_spec.as_ref(),
            postcondition_evidence.as_ref(),
            run_context,
        );
        append_message_event_with_metadata(
            &mut store,
            &request.task_id,
            MessageRole::Tool,
            &observation,
            message_metadata.clone(),
        )
        .map_err(|error| error.to_string())?;
        if !image_paths.is_empty() {
            append_visual_reference_event(
                &mut store,
                &request.task_id,
                &tool_name,
                &image_paths,
                run_context,
            )?;
        }
        (
            observation,
            status,
            image_paths,
            message_metadata,
            risk,
            effect_spec,
            postcondition_evidence,
        )
    } else {
        unreachable!("deny decisions return after their atomic persistence transaction")
    };
    Ok(ResolvedToolObservation {
        call_id: agent_core::ToolCallId(tool_call_id),
        tool_name,
        input_json: tool_input,
        risk,
        effect_spec,
        postcondition_evidence,
        status,
        observation,
        image_paths,
        message_metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::PromptEvidenceScope;

    #[test]
    fn permission_cold_recovery_preserves_verification_contract() {
        let mut run_context = Metadata::new();
        let conductor_contract = crate::agent_effort_planner::effort_execution_contract("auto")
            .to_json()
            .expect("contract serializes");
        let request_metadata = [
            ("conductor_contract".to_string(), conductor_contract.clone()),
            ("tool_input".to_string(), "sensitive".to_string()),
        ]
        .into_iter()
        .collect();

        restore_permission_run_context(&mut run_context, &request_metadata);

        assert_eq!(
            run_context.get("conductor_contract").map(String::as_str),
            Some(conductor_contract.as_str())
        );
        assert!(!run_context.contains_key("tool_input"));
        assert_eq!(
            workspace_verification_policy_for_run_context(&run_context)
                .expect("restored contract decodes"),
            WorkspaceVerificationPolicy::RequiredAfterMutation
        );
    }

    #[test]
    fn permission_cold_recovery_preserves_browser_execution_intent() {
        let objective = "Open the incident dashboard in the browser and create a JSON report";
        let mut run_context = Metadata::new();
        let request_metadata = [
            (
                "effective_prompt_objective".to_string(),
                objective.to_string(),
            ),
            ("task_class".to_string(), "browser".to_string()),
            ("tool_requirement".to_string(), "effects".to_string()),
            ("vision_required".to_string(), "false".to_string()),
        ]
        .into_iter()
        .collect();

        restore_permission_run_context(&mut run_context, &request_metadata);

        let root = std::env::temp_dir().join("cindx-permission-browser-tool-plan");
        let mut registry = ToolRegistry::with_workspace_tools(root);
        registry.install_meta_tools();
        let (tools, completion_intent) = crate::agent_loop_runtime::planned_agent_tools(
            &registry,
            &run_context,
            objective,
            128_000,
        );
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            run_context.get("tool_requirement").map(String::as_str),
            Some("effects")
        );
        assert!(completion_intent
            .evidence_scopes
            .contains(&PromptEvidenceScope::Browser));
        assert!(names.contains("browser.open"));
        assert!(names.contains("browser.extract_text"));
        assert!(names.contains("file.write"));
        assert!(!names.iter().any(|name| name.starts_with("computer.")));
    }

    fn permission_run_context(steer_epoch: u64, prompt_contract_epoch: u64) -> Metadata {
        [
            ("session_id".to_string(), "session-a".to_string()),
            (
                AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
                AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
            ),
            ("agent_run_id".to_string(), "run-a".to_string()),
            (
                LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
                "logical-run-a".to_string(),
            ),
            ("steer_epoch".to_string(), steer_epoch.to_string()),
            (
                "prompt_contract_epoch".to_string(),
                prompt_contract_epoch.to_string(),
            ),
            (
                "effective_prompt_objective".to_string(),
                "核实当前屏幕上的保存按钮".to_string(),
            ),
        ]
        .into_iter()
        .collect()
    }

    fn message(role: MessageRole, content: &str, metadata: Metadata) -> Message {
        Message {
            role,
            content: content.to_string(),
            metadata,
        }
    }

    fn permission_message(
        call_id: &str,
        tool_name: &str,
        status: ToolOutcomeStatus,
        content: &str,
        run_context: &Metadata,
    ) -> Message {
        permission_message_with_id(
            &format!("permission-{call_id}"),
            call_id,
            tool_name,
            status,
            content,
            run_context,
        )
    }

    fn permission_message_with_id(
        permission_id: &str,
        call_id: &str,
        tool_name: &str,
        status: ToolOutcomeStatus,
        content: &str,
        run_context: &Metadata,
    ) -> Message {
        message(
            MessageRole::Tool,
            content,
            permission_tool_observation_metadata(
                &PermissionRequestId(permission_id.to_string()),
                call_id,
                tool_name,
                &status,
                r#"{"secret":"super-secret"}"#,
                None,
                None,
                None,
                run_context,
            ),
        )
    }

    fn permission_recovery_event(sequence: u64, kind: EventKind, metadata: Metadata) -> Event {
        Event {
            id: EventId(format!("event-{sequence}")),
            task_id: phase16_task_id(),
            sequence,
            timestamp_ms: sequence,
            kind,
            summary: String::new(),
            metadata,
        }
    }

    fn permission_recovery_envelope(
        resume_key: &str,
        task_state: AgentTaskStateSnapshot,
    ) -> AgentRecoveryEnvelope {
        AgentRecoveryEnvelope {
            schema: AGENT_RECOVERY_SCHEMA.to_string(),
            identity: AgentRecoveryIdentity {
                project_id: Some("project-a".to_string()),
                session_id: "session-a".to_string(),
                resume_key: resume_key.to_string(),
                source_run_id: "run-a".to_string(),
                logical_run_id: Some("logical-run-a".to_string()),
                user_turn_sequence: 1,
                prompt_fingerprint: task_state.user_prompt_fingerprint.clone(),
            },
            effort: "auto".to_string(),
            policy: "auto_router".to_string(),
            queue_id: None,
            state: AgentRecoveryState::Blocked,
            reason: AgentRecoveryReason::WaitingForPermission,
            attempts: 0,
            model_calls: 1,
            tool_calls: 0,
            material_checkpoints: 0,
            observations: 0,
            budget_extensions: 0,
            task_state: Some(task_state),
            resource_snapshot: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn permission_recovery_keeps_logical_identity_on_the_physical_attempt() {
        let runtime = start_agent_loop(
            TaskId("permission-identity".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let recovery =
            permission_recovery_envelope("identity", AgentTaskStateSnapshot::capture(&runtime));
        let recovered =
            permission_recovery_run_context(&permission_run_context(0, 0), &recovery, 1);

        assert_eq!(
            recovered.get("agent_run_id").map(String::as_str),
            Some("run-a")
        );
        assert_eq!(
            recovered
                .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
                .map(String::as_str),
            Some("logical-run-a")
        );
        assert_eq!(
            recovered.get("source_agent_run_id").map(String::as_str),
            Some("run-a")
        );
    }

    #[test]
    fn permission_checkpoint_boundary_uses_the_matching_blocked_event() {
        let message_event = |sequence, role: &str, content: &str| {
            permission_recovery_event(
                sequence,
                EventKind::MessageAdded,
                [
                    ("role".to_string(), role.to_string()),
                    ("content".to_string(), content.to_string()),
                ]
                .into_iter()
                .collect(),
            )
        };
        let mut runtime = start_agent_loop(
            TaskId("permission-boundary".to_string()),
            "inspect",
            AgentRuntimeConfig::default(),
        );
        let older_snapshot = AgentTaskStateSnapshot::capture(&runtime);
        runtime
            .messages
            .push(message(MessageRole::Assistant, "working", Metadata::new()));
        let active_snapshot = AgentTaskStateSnapshot::capture(&runtime);
        let active_recovery = permission_recovery_envelope("active", active_snapshot.clone());
        let missing_recovery = permission_recovery_envelope("missing", active_snapshot.clone());
        let blocked_event = |sequence, envelope: AgentRecoveryEnvelope| {
            permission_recovery_event(
                sequence,
                EventKind::TaskStatusChanged,
                [
                    (
                        "recovery_resume_key".to_string(),
                        envelope.identity.resume_key.clone(),
                    ),
                    ("recovery_state".to_string(), "blocked".to_string()),
                    (
                        "recovery_envelope".to_string(),
                        serde_json::to_string(&envelope).expect("envelope serializes"),
                    ),
                ]
                .into_iter()
                .collect(),
            )
        };
        let events = vec![
            message_event(1, "user", "inspect"),
            blocked_event(2, permission_recovery_envelope("older", older_snapshot)),
            message_event(3, "assistant", "working"),
            blocked_event(4, active_recovery.clone()),
            message_event(5, "tool", "resolved after the checkpoint"),
            blocked_event(6, active_recovery.clone()),
        ];

        assert_eq!(
            permission_checkpoint_message_boundary(&events, &active_recovery, &active_snapshot),
            Some(2)
        );
        assert_eq!(
            permission_checkpoint_message_boundary(&events, &missing_recovery, &active_snapshot),
            None
        );
    }

    #[test]
    fn startup_recovery_replays_a_committed_permission_denial_into_task_state() {
        let run_context = permission_run_context(4, 0);
        let prompt = "核实当前屏幕上的保存按钮";
        let mut runtime = start_agent_loop(
            TaskId("permission-startup-replay".to_string()),
            prompt,
            AgentRuntimeConfig::default(),
        );
        runtime.task_contract.require_tool_success("shell.run");
        let snapshot = AgentTaskStateSnapshot::capture(&runtime);
        let recovery = permission_recovery_envelope("startup-replay", snapshot);
        let mut user_metadata = metadata_with_context(
            [
                ("role".to_string(), "user".to_string()),
                ("content".to_string(), prompt.to_string()),
            ]
            .into_iter()
            .collect(),
            &run_context,
        );
        user_metadata.insert("project_id".to_string(), "project-a".to_string());
        let mut blocked_metadata = metadata_with_context(
            [
                (
                    "recovery_resume_key".to_string(),
                    recovery.identity.resume_key.clone(),
                ),
                ("recovery_state".to_string(), "blocked".to_string()),
                (
                    "recovery_envelope".to_string(),
                    serde_json::to_string(&recovery).expect("envelope serializes"),
                ),
            ]
            .into_iter()
            .collect(),
            &run_context,
        );
        blocked_metadata.insert("project_id".to_string(), "project-a".to_string());
        let denied = permission_message(
            "call-denied",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "The user denied this tool call.",
            &run_context,
        );
        let mut denied_metadata = denied.metadata;
        denied_metadata.insert("role".to_string(), "tool".to_string());
        denied_metadata.insert("content".to_string(), denied.content);
        let events = vec![
            permission_recovery_event(1, EventKind::MessageAdded, user_metadata),
            permission_recovery_event(2, EventKind::TaskStatusChanged, blocked_metadata),
            permission_recovery_event(3, EventKind::MessageAdded, denied_metadata),
        ];

        let recovered = recovery_task_state_with_persisted_permission_denials(
            &events,
            &run_context,
            &recovery,
            None,
        )
        .expect("startup recovery should rebuild the typed denial");
        let ledger = recovered.task_contract.outcome_ledger_shadow(4);
        let blocked = ledger
            .obligations
            .iter()
            .find(|obligation| {
                obligation
                    .blocker
                    .as_ref()
                    .is_some_and(|blocker| blocker.code == "user_permission_denied")
            })
            .expect("the denied required tool must remain blocked");
        assert_eq!(
            blocked.satisfaction,
            agent_runtime::OutcomeSatisfaction::Blocked
        );
        let encoded = serde_json::to_string(&recovered).expect("snapshot serializes");
        assert!(!encoded.contains("super-secret"));
        assert!(!encoded.contains("The user denied"));

        let transcript = agent_runtime_transcript_from_active_events(&events);
        let mut more_complete = recovered
            .restore_with_effective_objective(prompt, transcript.clone(), prompt)
            .expect("recovered denial state should restore");
        more_complete
            .task_contract
            .require_tool_success("file.read");
        let read_fingerprint =
            agent_runtime::tool_input_fingerprint("file.read", r#"{"path":"status.md"}"#);
        AgentKernel::new(
            &mut more_complete,
            &[ToolSpec::builtin(
                "file.read",
                "test",
                "Read status",
                ToolRisk::ReadOnly,
                r#"{"type":"object"}"#,
            )],
        )
        .apply_persisted_tool_observation(
            agent_core::ToolCallId("read-after-denial".to_string()),
            "file.read",
            &read_fingerprint,
            None,
            None,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "status evidence",
        );
        let preferred = AgentTaskStateSnapshot::capture(&more_complete);
        let selected = recovery_task_state_with_persisted_permission_denials(
            &events,
            &run_context,
            &recovery,
            Some(&preferred),
        )
        .expect("a newer snapshot containing the denial should be retained");
        assert_eq!(selected, preferred);
        assert!(selected
            .task_contract
            .outcome_ledger_shadow(4)
            .obligations
            .iter()
            .any(|obligation| {
                obligation.kind == agent_runtime::OutcomeObligationKind::RequiredTool
                    && obligation.satisfaction == agent_runtime::OutcomeSatisfaction::Satisfied
            }));

        let denied_fingerprint = transcript
            .last()
            .and_then(|message| message.metadata.get("tool_input_fingerprint"))
            .cloned()
            .expect("the canonical denial should carry an input fingerprint");
        let (mut wrong_semantics, _) = restore_permission_snapshot_from_boundary(
            recovery
                .task_state
                .as_ref()
                .expect("blocked recovery should carry task state"),
            prompt,
            prompt,
            &transcript,
            1,
        )
        .expect("the blocked baseline should restore");
        AgentKernel::new(&mut wrong_semantics, &[]).apply_persisted_tool_observation_with_denial(
            agent_core::ToolCallId("wrong-capability-denial".to_string()),
            "shell.run",
            &denied_fingerprint,
            None,
            None,
            &ToolOutcomeStatus::Denied,
            None,
            "capability unavailable",
            Some(
                &agent_runtime::AgentActionDenialFeedback::capability_unavailable(
                    "tool_capability_unavailable",
                ),
            ),
        );
        let wrong_semantics = AgentTaskStateSnapshot::capture(&wrong_semantics);
        assert!(!wrong_semantics
            .task_contract
            .has_user_permission_finalization("shell.run", &denied_fingerprint));
        let corrected = recovery_task_state_with_persisted_permission_denials(
            &events,
            &run_context,
            &recovery,
            Some(&wrong_semantics),
        )
        .expect("a semantically different denial must not suppress canonical replay");
        assert_ne!(corrected, wrong_semantics);
        assert!(corrected
            .task_contract
            .outcome_ledger_shadow(4)
            .obligations
            .iter()
            .any(|obligation| {
                obligation.blocker.as_ref().is_some_and(|blocker| {
                    blocker.kind == agent_runtime::AgentActionDenialKind::UserPermission
                        && blocker.code == "user_permission_denied"
                })
            }));
    }

    #[test]
    fn permission_observation_marker_is_runtime_only_and_lineage_bound() {
        let run_context = permission_run_context(4, 0);
        let valid = permission_message(
            "call-valid",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "captured",
            &run_context,
        );
        assert!(valid
            .metadata
            .values()
            .all(|value| !value.contains("super-secret")));

        let mut legacy = valid.clone();
        legacy.metadata.remove("permission_observation_schema");
        let mut forged = valid.clone();
        forged.metadata.insert(
            "permission_observation_provenance".to_string(),
            "model_claim".to_string(),
        );
        let mut wrong_run = valid.clone();
        wrong_run
            .metadata
            .insert("agent_run_id".to_string(), "run-b".to_string());
        let mut wrong_contract_epoch = valid.clone();
        wrong_contract_epoch
            .metadata
            .insert("prompt_contract_epoch".to_string(), "4".to_string());
        let mut invalid_status = valid.clone();
        invalid_status
            .metadata
            .insert("status".to_string(), "unknown".to_string());

        let observations = persisted_permission_observations(
            &[
                legacy,
                forged,
                wrong_run,
                wrong_contract_epoch,
                invalid_status,
                valid,
            ],
            &run_context,
        );

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].call_id.0, "call-valid");
        assert_eq!(observations[0].permission_id.0, "permission-call-valid");
        assert_eq!(observations[0].message_index, 5);
    }

    #[test]
    fn permission_boundary_uses_unique_permission_id_when_call_ids_repeat() {
        let run_context = permission_run_context(4, 0);
        let transcript = vec![
            permission_message_with_id(
                "permission-old",
                "reused-call",
                "computer.screenshot",
                ToolOutcomeStatus::Succeeded,
                "old capture",
                &run_context,
            ),
            permission_message_with_id(
                "permission-current",
                "reused-call",
                "computer.screenshot",
                ToolOutcomeStatus::Succeeded,
                "current capture",
                &run_context,
            ),
        ];
        let observations = persisted_permission_observations(&transcript, &run_context);

        assert_eq!(
            current_permission_observation_boundary(
                &observations,
                &["permission-current".to_string()].into_iter().collect(),
                transcript.len(),
            ),
            1
        );
    }

    #[test]
    fn persisted_permission_observations_replay_each_permission_only_once() {
        let run_context = permission_run_context(4, 0);
        let observation = permission_message_with_id(
            "permission-a",
            "call-a",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "denied",
            &run_context,
        );

        let observations = persisted_permission_observations(
            &[observation.clone(), observation.clone()],
            &run_context,
        );

        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].permission_id.0, "permission-a");

        let mut conflicting = observation.clone();
        conflicting
            .metadata
            .insert("tool_call_id".to_string(), "call-b".to_string());
        assert!(
            persisted_permission_observations(&[observation, conflicting], &run_context).is_empty()
        );
    }

    #[test]
    fn cold_recovery_replays_all_permission_observations_without_duplicates() {
        let mut run_context = permission_run_context(4, 0);
        run_context.insert(
            "effective_prompt_objective".to_string(),
            "Initial request:\n核实当前屏幕上的保存按钮\n\nAccepted steering 1:\n继续核实当前屏幕上的保存按钮"
                .to_string(),
        );
        let tools = vec![ToolSpec::builtin(
            "computer.screenshot",
            "computer",
            "Capture the current screen",
            ToolRisk::SensitiveContext,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)];
        let mut original = start_agent_loop(
            TaskId("permission-recovery".to_string()),
            "核实当前屏幕上的保存按钮",
            AgentRuntimeConfig::default(),
        );
        original.messages.push(message(
            MessageRole::Assistant,
            "",
            [("tool_call_ids".to_string(), "call-a,call-b".to_string())]
                .into_iter()
                .collect(),
        ));
        apply_run_task_contract(&mut original, &run_context, &tools, None)
            .expect("contract applies before permission pause");
        AgentKernel::new(&mut original, &tools).require_tool_success("shell.run");
        let ledger_before_pause = original.task_contract.outcome_ledger_shadow(4);
        let snapshot = AgentTaskStateSnapshot::capture(&original);
        let original_message_count = original.messages.len();

        let mut transcript = original.messages.clone();
        transcript.push(message(
            MessageRole::User,
            "noop control message after the blocked checkpoint",
            [("steer".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
        ));
        transcript.push(permission_message(
            "call-a",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "captured",
            &run_context,
        ));
        transcript.push(message(
            MessageRole::User,
            "Visual reference captured by computer.screenshot.",
            [("kind".to_string(), "visual_reference".to_string())]
                .into_iter()
                .collect(),
        ));
        transcript.push(permission_message(
            "call-b",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "denied",
            &run_context,
        ));
        transcript.push(permission_message(
            "call-c",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "denied again",
            &run_context,
        ));

        let observations = persisted_permission_observations(&transcript, &run_context);
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.call_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["call-a", "call-b", "call-c"]
        );
        let (mut restored, boundary) = restore_permission_snapshot_from_boundary(
            &snapshot,
            &original.user_prompt,
            original.prepared_task_state().effective_objective(),
            &transcript,
            original_message_count,
        )
        .expect("blocked checkpoint should restore from its recorded message boundary");
        assert_eq!(boundary, original_message_count);
        assert_eq!(restored.prepared_task_state().steer_epoch(), 4);
        assert_eq!(restored.prepared_task_state().contract_epoch(), 0);
        assert_ne!(
            restored.prepared_task_state().effective_objective(),
            restored.user_prompt
        );
        assert_eq!(
            restored.task_contract.outcome_ledger_shadow(4),
            ledger_before_pause,
            "permission pause and cold recovery must preserve the shadow contract"
        );

        apply_run_task_contract(&mut restored, &run_context, &tools, None)
            .expect("contract reapplies after recovery");
        for observation in observations
            .iter()
            .filter(|observation| observation.message_index >= boundary)
        {
            let risk = (observation.tool_name == "computer.screenshot")
                .then_some(ToolRisk::SensitiveContext);
            let denial = matches!(observation.status, ToolOutcomeStatus::Denied)
                .then(agent_runtime::AgentActionDenialFeedback::user_permission);
            AgentKernel::new(&mut restored, &tools).apply_persisted_tool_observation_with_denial(
                observation.call_id.clone(),
                &observation.tool_name,
                &observation.input_fingerprint,
                observation.target_witness.as_deref(),
                observation.effect_witness.as_ref(),
                &observation.status,
                risk.as_ref(),
                &observation.observation,
                denial.as_ref(),
            );
            copy_replayed_contract_evidence_metadata(
                &restored,
                &mut transcript,
                observation.message_index,
            );
        }
        assert_eq!(
            AgentKernel::new(&mut restored, &tools).repeated_tool_failure_count(
                &AgentToolRequest {
                    call_id: agent_core::ToolCallId("probe-same".to_string()),
                    tool_name: "shell.run".to_string(),
                    input: r#"{"secret":"super-secret"}"#.to_string(),
                }
            ),
            MAX_IDENTICAL_TOOL_FAILURES
        );
        assert_eq!(
            AgentKernel::new(&mut restored, &tools).repeated_tool_failure_count(
                &AgentToolRequest {
                    call_id: agent_core::ToolCallId("probe-changed".to_string()),
                    tool_name: "shell.run".to_string(),
                    input: r#"{"secret":"different"}"#.to_string(),
                }
            ),
            0
        );
        restored.messages = transcript.clone();

        assert!(restored.messages.iter().any(|message| {
            message.metadata.get("tool_call_id").map(String::as_str) == Some("call-a")
                && message
                    .metadata
                    .contains_key(agent_runtime::CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY)
        }));

        assert_eq!(
            AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
            Ok(None)
        );
        assert_eq!(restored.messages, transcript);
        assert_eq!(
            restored
                .messages
                .iter()
                .filter(|message| message.role == MessageRole::Tool)
                .count(),
            3
        );
        let recovered_ledger = restored.task_contract.outcome_ledger_shadow(4);
        let screenshot_evidence = recovered_ledger
            .evidence
            .iter()
            .filter(|evidence| evidence.source == "computer.screenshot")
            .map(|evidence| evidence.kind)
            .collect::<Vec<_>>();
        assert_eq!(screenshot_evidence.len(), 2);
        assert!(screenshot_evidence.contains(&agent_runtime::ContractEvidenceKind::Grounding));
        assert!(
            screenshot_evidence.contains(&agent_runtime::ContractEvidenceKind::OtherTool),
            "a recovered observation without a bound interaction must fail closed: {screenshot_evidence:?}"
        );
        let shell_denial_evidence = recovered_ledger
            .evidence
            .iter()
            .filter(|evidence| {
                evidence.source == "shell.run"
                    && evidence.kind == agent_runtime::ContractEvidenceKind::Denial
            })
            .collect::<Vec<_>>();
        assert_eq!(shell_denial_evidence.len(), 1);
        let shell_obligation = recovered_ledger
            .obligations
            .iter()
            .find(|obligation| {
                obligation
                    .blocker
                    .as_ref()
                    .is_some_and(|blocker| blocker.code == "user_permission_denied")
            })
            .expect("cold recovery must preserve the typed permission blocker");
        assert_eq!(
            shell_obligation.satisfaction,
            agent_runtime::OutcomeSatisfaction::Blocked
        );
        assert_eq!(
            shell_obligation.evidence_sequence,
            Some(shell_denial_evidence[0].sequence)
        );
        let encoded_contract =
            serde_json::to_string(&restored.task_contract).expect("contract serializes");
        assert!(encoded_contract.contains("user_permission_denied"));
        assert!(!encoded_contract.contains("super-secret"));
        assert!(!encoded_contract.contains("denied again"));
    }

    #[test]
    fn later_permission_pause_restores_after_prior_permission_evidence() {
        let run_context = permission_run_context(4, 0);
        let tools = vec![ToolSpec::builtin(
            "computer.screenshot",
            "computer",
            "Capture the current screen",
            ToolRisk::SensitiveContext,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)];
        let mut prior = start_agent_loop(
            TaskId("sequential-permission-recovery".to_string()),
            "核实当前屏幕上的保存按钮",
            AgentRuntimeConfig::default(),
        );
        apply_run_task_contract(&mut prior, &run_context, &tools, None)
            .expect("initial contract applies");
        AgentKernel::new(&mut prior, &tools).apply_tool_observation(
            &AgentToolRequest {
                call_id: agent_core::ToolCallId("call-a".to_string()),
                tool_name: "computer.screenshot".to_string(),
                input: r#"{"display":0}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::SensitiveContext),
            "captured first screen",
        );
        prior.messages.pop();
        prior.messages.push(permission_message(
            "call-a",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "captured first screen",
            &run_context,
        ));
        let snapshot = AgentTaskStateSnapshot::capture(&prior);
        let snapshot_boundary = prior.messages.len();

        let mut transcript = prior.messages.clone();
        transcript.push(message(
            MessageRole::Assistant,
            "continuing after the first approval",
            Metadata::new(),
        ));
        transcript.push(permission_message(
            "call-b",
            "shell.run",
            ToolOutcomeStatus::Denied,
            "denied",
            &run_context,
        ));
        let observations = persisted_permission_observations(&transcript, &run_context);
        let boundary = current_permission_observation_boundary(
            &observations,
            &["permission-call-b".to_string()].into_iter().collect(),
            transcript.len(),
        );
        assert_eq!(boundary, transcript.len() - 1);

        let (mut restored, restored_boundary) = restore_permission_snapshot_from_boundary(
            &snapshot,
            &prior.user_prompt,
            prior.prepared_task_state().effective_objective(),
            &transcript,
            snapshot_boundary,
        )
        .expect("latest blocked snapshot should match its recorded message boundary");
        assert_eq!(restored_boundary, snapshot_boundary);
        assert_eq!(
            AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
            Ok(None)
        );
    }

    #[test]
    fn cold_permission_recovery_reconstructs_verified_workspace_postcondition() {
        let mut run_context = permission_run_context(0, 0);
        let conductor_contract = crate::agent_effort_planner::effort_execution_contract("auto")
            .to_json()
            .expect("contract serializes");
        run_context.insert("conductor_contract".to_string(), conductor_contract);
        run_context.insert(
            "effective_prompt_objective".to_string(),
            "write the requested file and verify it".to_string(),
        );
        let tools = vec![
            ToolSpec::builtin(
                "file.write",
                "file",
                "Write a file",
                ToolRisk::WritesWorkspace,
                r#"{"type":"object"}"#,
            )
            .with_effect_semantics(agent_core::ToolEffectSemantics::Verifiable {
                verifier: "workspace_file_content_v1".to_string(),
            }),
            ToolSpec::builtin(
                "file.read",
                "file",
                "Read a file",
                ToolRisk::ReadOnly,
                r#"{"type":"object"}"#,
            )
            .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)
            .with_postcondition_verifier(
                agent_core::PostconditionVerifierKind::WorkspaceExactReadbackV1,
            ),
        ];
        let mut original = start_agent_loop(
            TaskId("cold-workspace-postcondition".to_string()),
            "write the requested file and verify it",
            AgentRuntimeConfig::default(),
        );
        apply_run_task_contract(&mut original, &run_context, &tools, None)
            .expect("verification contract applies before the permission pause");
        let snapshot = AgentTaskStateSnapshot::capture(&original);
        let boundary = original.messages.len();

        let target = "private/super-secret-goal.md";
        let write_input = format!(r#"{{"path":"{target}","content":"never-persist-this-secret"}}"#);
        let read_input = format!(r#"{{"path":"{target}"}}"#);
        let permission_observation =
            |permission_id: &str, call_id: &str, tool_name: &str, input: &str, risk: &ToolRisk| {
                let effect_spec = tools.iter().find(|spec| spec.name == tool_name);
                let postcondition_evidence =
                    (tool_name == "file.read").then(|| agent_core::ToolPostconditionEvidence {
                        kind: agent_core::PostconditionVerifierKind::WorkspaceExactReadbackV1,
                        target_input_json: input.to_string(),
                    });
                message(
                    MessageRole::Tool,
                    "tool completed with bounded evidence",
                    permission_tool_observation_metadata(
                        &PermissionRequestId(permission_id.to_string()),
                        call_id,
                        tool_name,
                        &ToolOutcomeStatus::Succeeded,
                        input,
                        Some(risk),
                        effect_spec,
                        postcondition_evidence.as_ref(),
                        &run_context,
                    ),
                )
            };
        let mut transcript = original.messages.clone();
        transcript.push(permission_observation(
            "permission-write",
            "write",
            "file.write",
            &write_input,
            &ToolRisk::WritesWorkspace,
        ));
        transcript.push(permission_observation(
            "permission-read",
            "read",
            "file.read",
            &read_input,
            &ToolRisk::ReadOnly,
        ));

        for persisted in &transcript[boundary..] {
            let witness = persisted
                .metadata
                .get(agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY)
                .expect("successful effect should persist a recovery witness");
            assert!(witness.len() <= agent_runtime::MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES);
            for secret in [
                target,
                "private",
                "super-secret",
                "never-persist-this-secret",
            ] {
                assert!(!witness.contains(secret));
                assert!(persisted
                    .metadata
                    .values()
                    .all(|value| !value.contains(secret)));
            }
        }

        let observations = persisted_permission_observations(&transcript, &run_context);
        assert_eq!(observations.len(), 2);
        assert!(observations
            .iter()
            .all(|observation| observation.effect_witness.is_some()));
        let (mut restored, replay_boundary) = restore_permission_snapshot_from_boundary(
            &snapshot,
            &original.user_prompt,
            original.prepared_task_state().effective_objective(),
            &transcript,
            boundary,
        )
        .expect("cold recovery should restore the blocked snapshot");
        assert_eq!(replay_boundary, boundary);
        apply_run_task_contract(&mut restored, &run_context, &tools, None)
            .expect("verification contract reapplies after restart");
        let mut deltas = Vec::new();
        for observation in &observations {
            let risk = tools
                .iter()
                .find(|tool| tool.name == observation.tool_name)
                .map(|tool| &tool.risk);
            deltas.extend(
                AgentKernel::new(&mut restored, &tools).apply_persisted_tool_observation(
                    observation.call_id.clone(),
                    &observation.tool_name,
                    &observation.input_fingerprint,
                    observation.target_witness.as_deref(),
                    observation.effect_witness.as_ref(),
                    &observation.status,
                    risk,
                    &observation.observation,
                ),
            );
        }
        assert!(deltas.iter().any(|delta| {
            delta
                .kinds()
                .contains(&agent_runtime::AgentGoalDeltaKind::WorkspaceVerified)
        }));
        assert_eq!(
            AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
            Ok(None),
            "the cold-replayed write/read pair must close the postcondition"
        );
        assert!(restored
            .task_contract
            .outcome_ledger_shadow(0)
            .postconditions
            .iter()
            .any(|postcondition| {
                postcondition.status == agent_runtime::OutcomePostconditionStatus::Verified
            }));

        let mut legacy_transcript = transcript;
        for message in &mut legacy_transcript[boundary..] {
            message
                .metadata
                .remove(agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY);
        }
        let legacy_observations =
            persisted_permission_observations(&legacy_transcript, &run_context);
        assert!(legacy_observations
            .iter()
            .all(|observation| observation.effect_witness.is_none()));
        let (mut legacy, _) = restore_permission_snapshot_from_boundary(
            &snapshot,
            &original.user_prompt,
            original.prepared_task_state().effective_objective(),
            &legacy_transcript,
            boundary,
        )
        .expect("old snapshots without a witness remain readable");
        apply_run_task_contract(&mut legacy, &run_context, &tools, None)
            .expect("old snapshot contract reapplies");
        for observation in &legacy_observations {
            let risk = tools
                .iter()
                .find(|tool| tool.name == observation.tool_name)
                .map(|tool| &tool.risk);
            AgentKernel::new(&mut legacy, &tools).apply_persisted_tool_observation(
                observation.call_id.clone(),
                &observation.tool_name,
                &observation.input_fingerprint,
                observation.target_witness.as_deref(),
                None,
                &observation.status,
                risk,
                &observation.observation,
            );
        }
        assert!(AgentKernel::new(&mut legacy, &tools)
            .completion_gate_for_task()
            .expect("legacy completion gate evaluates")
            .is_some());
    }

    fn permission_goal_delta() -> agent_runtime::AgentGoalDelta {
        let tools = vec![ToolSpec::builtin(
            "file.read",
            "test",
            "Read a file",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )];
        let mut runtime = start_agent_loop(
            TaskId("cold-permission-credit".to_string()),
            "read the required file",
            AgentRuntimeConfig::default(),
        );
        runtime.task_contract.require_tool_success("file.read");
        AgentKernel::new(&mut runtime, &tools)
            .apply_tool_observation(
                &AgentToolRequest {
                    call_id: agent_core::ToolCallId("read".to_string()),
                    tool_name: "file.read".to_string(),
                    input: r#"{"path":"goal.md"}"#.to_string(),
                },
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "goal evidence",
            )
            .expect("the required tool should produce a Goal Delta")
    }

    #[test]
    fn denied_permission_bundle_commits_and_rolls_back_as_one_unit() {
        let mut store = SqliteStore::in_memory().expect("store should open");
        let task_id = TaskId("permission-bundle".to_string());
        let request_id = PermissionRequestId("permission-bundle-deny".to_string());
        let request = PermissionRequest {
            id: request_id.clone(),
            task_id: task_id.clone(),
            risk: PermissionRisk::Execute,
            action: "shell.run".to_string(),
            reason: "Run a protected command".to_string(),
            scope: "/tmp/project".to_string(),
            metadata: Metadata::new(),
        };
        store
            .save_permission_request(request.clone(), 100)
            .expect("permission should persist");
        let resolution = PermissionResolution {
            request_id: request_id.clone(),
            decision: PermissionDecision::Deny,
            resolved_at_ms: 200,
            resolved_by: "local-user".to_string(),
        };
        let run_context = [
            ("session_id".to_string(), "session-bundle".to_string()),
            ("agent_run_id".to_string(), "run-bundle".to_string()),
            ("prompt_contract_epoch".to_string(), "0".to_string()),
        ]
        .into_iter()
        .collect::<Metadata>();
        let observation = "Tool shell.run denied: The user denied this tool call.";
        let message_metadata = permission_tool_observation_metadata(
            &request_id,
            "call-bundle",
            "shell.run",
            &ToolOutcomeStatus::Denied,
            r#"{"command":"touch protected"}"#,
            None,
            None,
            None,
            &run_context,
        );

        let injected = store.with_immediate_transaction(|store| {
            persist_denied_permission_resolution_rows(
                store,
                &request,
                &resolution,
                "call-bundle",
                "shell.run",
                observation,
                &message_metadata,
                &run_context,
            )?;
            Err::<(), _>(StorageError::new("injected post-bundle failure"))
        });
        assert!(injected.is_err());
        assert!(
            store.list_permission_audits().expect("audits should load")[0]
                .resolution
                .is_none()
        );
        assert!(store
            .list_by_task(&task_id)
            .expect("rolled-back events should load")
            .is_empty());

        store
            .with_immediate_transaction(|store| {
                persist_denied_permission_resolution_rows(
                    store,
                    &request,
                    &resolution,
                    "call-bundle",
                    "shell.run",
                    observation,
                    &message_metadata,
                    &run_context,
                )
            })
            .expect("denial bundle should commit");
        let audit = store
            .list_permission_audits()
            .expect("audits should load")
            .remove(0);
        assert_eq!(
            audit.resolution.map(|resolution| resolution.decision),
            Some(PermissionDecision::Deny)
        );
        let events = store
            .list_by_task(&task_id)
            .expect("bundle events should load");
        assert_eq!(events.len(), 3);
        assert!(events.iter().any(|event| {
            event.kind == EventKind::PermissionResolved
                && event.metadata.get("permission_id") == Some(&request_id.0)
        }));
        assert!(events.iter().any(|event| {
            event.kind == EventKind::ToolCallFinished
                && event.metadata.get("failure_code").map(String::as_str)
                    == Some("user_permission_denied")
        }));
        assert!(events.iter().any(|event| {
            event.kind == EventKind::MessageAdded
                && event
                    .metadata
                    .get("action_denial_schema")
                    .map(String::as_str)
                    == Some(agent_runtime::ACTION_DENIAL_SCHEMA)
        }));
    }

    #[test]
    fn cold_permission_recovery_persists_before_the_caller_credits_goal_delta() {
        let empty_commit_called = std::cell::Cell::new(false);
        persist_cold_permission_recovery(&[], false, || {
            empty_commit_called.set(true);
            Ok(())
        })
        .expect("an empty recovery should be a no-op");
        assert!(
            !empty_commit_called.get(),
            "a recovery without Goal Deltas should not add a snapshot write"
        );

        let delta = permission_goal_delta();
        let failed_control = AgentRunControl::new("auto");
        let failure = persist_cold_permission_recovery(std::slice::from_ref(&delta), true, || {
            Err("injected runtime snapshot failure".to_string())
        });
        assert!(failure.is_err());
        assert!(
            failed_control.record_goal_delta_at(0, &delta),
            "a failed runtime commit must not consume or credit the Goal Delta"
        );

        let committed_control = AgentRunControl::new("auto");
        persist_cold_permission_recovery(std::slice::from_ref(&delta), true, || Ok(()))
            .expect("the recovered runtime should persist");
        assert!(
            committed_control.record_goal_delta_at(0, &delta),
            "runtime persistence must not credit the live control before the recovery claim"
        );

        let denial_only_commit_called = std::cell::Cell::new(false);
        persist_cold_permission_recovery(&[], true, || {
            denial_only_commit_called.set(true);
            Ok(())
        })
        .expect("a denial-only recovery should persist its rebuilt contract");
        assert!(denial_only_commit_called.get());
    }
}
