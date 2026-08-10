use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_background_prompt_mutation_stage(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    mutation_id: &str,
    stage: &str,
    prompt: String,
    control: &Arc<AgentRunControl>,
) -> Result<String, String> {
    run_background_prompt_mutation_stage_with_liveness(
        state,
        config,
        task_id,
        run_context,
        mutation_id,
        stage,
        prompt,
        control,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_background_prompt_mutation_stage_with_liveness(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    mutation_id: &str,
    stage: &str,
    prompt: String,
    control: &Arc<AgentRunControl>,
    response_start_timeout: Option<std::time::Duration>,
) -> Result<String, String> {
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let role = ModelRole::Planner;
    let model = config.model_for_conductor();
    let request_id = unique_id("prompt-mutation-model");
    record_collaboration_stage_started(
        state,
        task_id,
        run_context,
        mutation_id,
        stage,
        &role,
        &model,
        &request_id,
        AgentModelAttribution::service(
            AgentService::LearningUtility,
            AgentStage::Plan,
            AgentModelProfile::Reasoning,
        ),
        &Metadata::new(),
    )?;
    let completion = complete_collaboration_model_for_stage_with_recovery_control(
        config.clone(),
        stage.to_string(),
        role.clone(),
        model.clone(),
        collaboration_system_prompt_for_run(&config.agent_system_prompt, run_context),
        prompt,
        Some(control.clone()),
        prompt_mutation_call_limits(response_start_timeout, run_context_steer_epoch(run_context)),
        None,
        |_| {},
    );
    record_collaboration_stage_finished(
        state,
        task_id,
        run_context,
        mutation_id,
        stage,
        &role,
        &model,
        &request_id,
        &completion,
        AgentModelAttribution::service(
            AgentService::LearningUtility,
            AgentStage::Plan,
            AgentModelProfile::Reasoning,
        ),
        &Metadata::new(),
    )?;
    completion.content.ok_or_else(|| {
        completion
            .error
            .unwrap_or_else(|| "prompt mutation model returned no content".to_string())
    })
}

fn prompt_mutation_call_limits(
    response_start_timeout: Option<std::time::Duration>,
    objective_epoch: u64,
) -> CollaborationCallLimits {
    CollaborationCallLimits {
        no_progress_timeout: response_start_timeout,
        provider_activity_is_progress: response_start_timeout.is_some(),
        objective_epoch: Some(objective_epoch),
        ..CollaborationCallLimits::default()
    }
}

pub(crate) fn append_prompt_mutation_status(
    state: &tauri::State<'_, AppState>,
    task_id: &TaskId,
    run_context: &Metadata,
    summary: &str,
    metadata: Metadata,
) -> Result<(), String> {
    let mut store = state
        .store
        .lock()
        .map_err(|error| format!("store lock poisoned: {error}"))?;
    append_event(
        &mut store,
        task_id,
        EventKind::TaskStatusChanged,
        summary,
        metadata_with_context(metadata, run_context),
    )
    .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_background_prompt_mutation(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    parent: &ConductorPromptGenome,
    trajectories: &[AgentEvaluationReflectionPacket],
    control: &Arc<AgentRunControl>,
) -> Result<bool, String> {
    if trajectories.is_empty() || control.should_stop() {
        return Ok(false);
    }
    let mutation_run_id = unique_id("prompt-mutation");
    let reflection_trajectory_count = trajectories.len();
    let reflection_evidence_sha256 = sha256_hex(
        &serde_json::to_vec(trajectories)
            .map_err(|error| format!("reflection evidence serialization failed: {error}"))?,
    );
    let mutation_prompt = parent.reflective_mutation_prompt(trajectories)?;
    let response = match run_background_prompt_mutation_stage(
        state,
        config,
        task_id,
        run_context,
        &mutation_run_id,
        "prompt_evolution_mutation",
        mutation_prompt,
        control,
    ) {
        Ok(response) => response,
        Err(error) => {
            if error == MODEL_REQUEST_CANCELLED {
                return Ok(false);
            }
            append_prompt_mutation_status(
                state,
                task_id,
                run_context,
                "Conductor prompt mutation failed",
                [
                    ("background_evaluation".to_string(), "true".to_string()),
                    ("collaboration_id".to_string(), mutation_run_id),
                    ("prompt_effort".to_string(), effort.to_string()),
                    ("parent_profile".to_string(), parent.id.clone()),
                    (
                        "mutation_strategy".to_string(),
                        "gepa_reflection".to_string(),
                    ),
                    (
                        "reflection_trajectory_count".to_string(),
                        reflection_trajectory_count.to_string(),
                    ),
                    (
                        "reflection_selector_schema".to_string(),
                        PROMPT_REFLECTION_SELECTOR_SCHEMA_V1.to_string(),
                    ),
                    (
                        "reflection_evidence_sha256".to_string(),
                        reflection_evidence_sha256.clone(),
                    ),
                    (
                        "error".to_string(),
                        truncate_for_collaboration(&error, 1_000),
                    ),
                ]
                .into_iter()
                .collect(),
            )?;
            return Ok(false);
        }
    };
    let mutation_id = format!(
        "learned-{}-g{}-{}",
        effort,
        parent.generation.saturating_add(1),
        unique_id("profile")
    );
    let mut mutation_repaired = false;
    let mutation = match parent.learned_reflective_mutation_from_response(
        &response,
        mutation_id.clone(),
        trajectories,
    ) {
        Ok(mutation) => Ok(mutation),
        Err(initial_error) => {
            let repair_prompt = parent.mutation_repair_prompt(&response, &initial_error);
            match run_background_prompt_mutation_stage(
                state,
                config,
                task_id,
                run_context,
                &mutation_run_id,
                "prompt_evolution_mutation_repair",
                repair_prompt,
                control,
            ) {
                Ok(repaired_response) => {
                    mutation_repaired = true;
                    parent
                        .learned_reflective_mutation_from_response(
                            &repaired_response,
                            mutation_id,
                            trajectories,
                        )
                        .map_err(|repair_error| {
                            format!(
                                "initial mutation: {initial_error}; repaired mutation: {repair_error}"
                            )
                        })
                }
                Err(repair_error) => Err(format!(
                    "initial mutation: {initial_error}; repair request: {repair_error}"
                )),
            }
        }
    };
    let generated = match mutation {
        Ok(mutation) => {
            append_prompt_mutation_status(
                state,
                task_id,
                run_context,
                "Conductor prompt mutation generated",
                [
                    ("background_evaluation".to_string(), "true".to_string()),
                    ("collaboration_id".to_string(), mutation_run_id),
                    ("prompt_effort".to_string(), effort.to_string()),
                    ("parent_profile".to_string(), parent.id.clone()),
                    (
                        "mutation_strategy".to_string(),
                        "gepa_reflection".to_string(),
                    ),
                    ("reflection_split".to_string(), "feedback".to_string()),
                    (
                        "reflection_trajectory_count".to_string(),
                        reflection_trajectory_count.to_string(),
                    ),
                    (
                        "reflection_selector_schema".to_string(),
                        PROMPT_REFLECTION_SELECTOR_SCHEMA_V1.to_string(),
                    ),
                    (
                        "reflection_evidence_sha256".to_string(),
                        reflection_evidence_sha256.clone(),
                    ),
                    (
                        "mutation_repaired".to_string(),
                        mutation_repaired.to_string(),
                    ),
                    ("prompt_profile".to_string(), mutation.id.clone()),
                    (
                        "prompt_generation".to_string(),
                        mutation.generation.to_string(),
                    ),
                    (
                        "prompt_genome".to_string(),
                        serde_json::to_string(&mutation).unwrap_or_else(|_| "{}".to_string()),
                    ),
                    (
                        "promotion_status".to_string(),
                        "evaluation_required".to_string(),
                    ),
                ]
                .into_iter()
                .collect(),
            )?;
            true
        }
        Err(error) => {
            if error.contains(MODEL_REQUEST_CANCELLED) {
                return Ok(false);
            }
            append_prompt_mutation_status(
                state,
                task_id,
                run_context,
                "Conductor prompt mutation rejected",
                [
                    ("background_evaluation".to_string(), "true".to_string()),
                    ("collaboration_id".to_string(), mutation_run_id),
                    ("prompt_effort".to_string(), effort.to_string()),
                    ("parent_profile".to_string(), parent.id.clone()),
                    (
                        "mutation_strategy".to_string(),
                        "gepa_reflection".to_string(),
                    ),
                    ("reflection_split".to_string(), "feedback".to_string()),
                    (
                        "reflection_trajectory_count".to_string(),
                        reflection_trajectory_count.to_string(),
                    ),
                    (
                        "reflection_selector_schema".to_string(),
                        PROMPT_REFLECTION_SELECTOR_SCHEMA_V1.to_string(),
                    ),
                    (
                        "reflection_evidence_sha256".to_string(),
                        reflection_evidence_sha256,
                    ),
                    (
                        "mutation_repaired".to_string(),
                        mutation_repaired.to_string(),
                    ),
                    (
                        "validation_error".to_string(),
                        truncate_for_collaboration(&error, 1_000),
                    ),
                ]
                .into_iter()
                .collect(),
            )?;
            false
        }
    };
    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_protocol_activity_only_extends_explicit_liveness_calls() {
        let production = prompt_mutation_call_limits(None, 7);
        let bounded_candidate =
            prompt_mutation_call_limits(Some(std::time::Duration::from_secs(300)), 7);

        assert!(!production.provider_activity_is_progress);
        assert_eq!(production.no_progress_timeout, None);
        assert!(bounded_candidate.provider_activity_is_progress);
        assert_eq!(
            bounded_candidate.no_progress_timeout,
            Some(std::time::Duration::from_secs(300))
        );
        assert_eq!(bounded_candidate.objective_epoch, Some(7));
    }
}
