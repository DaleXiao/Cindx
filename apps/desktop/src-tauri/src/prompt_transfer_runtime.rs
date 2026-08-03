use crate::app_state::AppState;
use crate::collaboration_models::{
    PromptAutoTeacherCase, PromptExecutionCandidate, PromptExecutionStep, PromptOfflineCase,
    PromptPlanCandidate, PromptWorkflowExecution,
};
use crate::configuration_models::ProviderConfig;
use crate::prompt_evaluation_feedback;
use crate::prompt_evaluation_runtime::{
    append_prompt_evaluation_status, append_prompt_transfer_observations,
};
use crate::prompt_evidence_runtime::prompt_auto_transfer_dataset_identity;
use crate::prompt_pairwise_runtime::{prompt_candidate_models, PromptTreatmentControlGroup};
use agent_core::{Metadata, TaskId};
use agent_runtime::AgentRunControl;
use model_provider::MODEL_REQUEST_CANCELLED;
use orchestrator::{
    prompt_genome_sha256, ConductorPromptGenome, PromptEvaluationAttemptEventV1,
    PromptEvaluationAttemptStatus, PromptEvaluationMode, PromptEvaluationProvenance,
    PromptEvaluationSplit, PromptEvolutionObservation, PromptLearningCohortV1,
    PromptDatasetIdentityV1,
    PromptMatchedEvaluationIdentityV1, PromptTransferProvenance, PromptTreatmentIdentityV1,
};
use std::path::Path;
use std::sync::Arc;

fn prompt_auto_teacher_candidate(teacher: &PromptAutoTeacherCase) -> PromptExecutionCandidate {
    PromptExecutionCandidate {
        plan: PromptPlanCandidate {
            genome: teacher.genome.clone(),
            plan: Some(teacher.plan.clone()),
            raw_output: String::new(),
            latency_ms: 0,
            total_tokens: 0,
        },
        execution: PromptWorkflowExecution {
            succeeded: true,
            quality_gate_met: true,
            final_output: teacher.final_output.clone(),
            steps: teacher
                .steps
                .iter()
                .map(|step| PromptExecutionStep {
                    id: step.id.clone(),
                    role: step.role.clone(),
                    model: step.model.clone(),
                    prompt: String::new(),
                    attempts: step.attempts,
                    status: step.status.clone(),
                    output: step.output.clone(),
                    tool_calls: Vec::new(),
                    errors: step.errors.clone(),
                    latency_ms: step.latency_ms,
                    total_tokens: step.total_tokens,
                    evidence_count: step.evidence_count,
                })
                .collect(),
            latency_ms: teacher.latency_ms,
            total_tokens: teacher.total_tokens,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_prompt_auto_transfer_evaluation(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    policy: &str,
    evaluation_worker_models: &[String],
    agent_budget: usize,
    workspace_root: &Path,
    teacher: &PromptAutoTeacherCase,
    transfer_dataset_identity: &PromptDatasetIdentityV1,
    selected_case: &PromptOfflineCase,
    split: PromptEvaluationSplit,
    mode: PromptEvaluationMode,
    control: &Arc<AgentRunControl>,
    evaluation_id: &str,
    objective: &str,
    task_class: &str,
    reviewer_model: &str,
    label: &str,
    candidate: &PromptExecutionCandidate,
    mut transfer_control: PromptTreatmentControlGroup<'_>,
) -> Result<(), String> {
    if control.should_stop() {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    let transfer_evaluation_id = format!("{evaluation_id}-auto-{label}");
    let transfer_budget = transfer_control.lane_budget();
    let transfer_context = crate::prompt_learning_runtime::prompt_execution_context(
        config,
        effort,
        policy,
        evaluation_worker_models,
        agent_budget,
        &candidate.plan.genome,
        workspace_root,
        transfer_budget,
        control.as_ref(),
    )?;
    let transfer_cohort =
        PromptLearningCohortV1::new(transfer_dataset_identity.clone(), transfer_context)?;
    let transfer_identity = PromptMatchedEvaluationIdentityV1::new(
        transfer_evaluation_id.clone(),
        &transfer_cohort,
        selected_case.id.clone(),
        split,
        mode,
    )?;
    let candidate_prompt_sha256 = prompt_genome_sha256(&candidate.plan.genome)?;
    let transfer_started = PromptEvaluationAttemptEventV1::started(
        transfer_identity.clone(),
        &transfer_cohort,
        [
            PromptTreatmentIdentityV1 {
                profile_id: candidate.plan.genome.id.clone(),
                prompt_sha256: candidate_prompt_sha256,
            },
            PromptTreatmentIdentityV1 {
                profile_id: teacher.profile_id.clone(),
                prompt_sha256: teacher.profile_sha256.clone(),
            },
        ],
    )?;
    let mut transfer_attempt = crate::prompt_attempt_runtime::PromptEvaluationAttemptGuard::start(
        state,
        task_id,
        run_context,
        effort,
        &transfer_cohort,
        transfer_started,
    )?;
    let candidate_failed = candidate.plan.plan.is_none()
        || !candidate.execution.succeeded
        || !candidate.execution.quality_gate_met;
    if candidate_failed {
        transfer_attempt.mark_treatment_failures([true, false]);
        transfer_attempt.finish(
            PromptEvaluationAttemptStatus::TreatmentFailure,
            "treatment_execution_failed",
        )?;
    }
    let forward_control = transfer_control.lane(0);
    let reverse_control = transfer_control.lane(1);
    let transfer_result = evaluate_prompt_auto_transfer_pair(
        config,
        reviewer_model,
        objective,
        candidate,
        teacher,
        &transfer_evaluation_id,
        task_class,
        split,
        mode,
        &transfer_identity,
        &forward_control,
        &reverse_control,
    );
    let reviewer_preempted = [forward_control.as_ref(), reverse_control.as_ref()]
        .into_iter()
        .any(|lane| lane.stop_reason() == Some(agent_runtime::RunStopReason::UserCancelled));
    if let Err(error) = transfer_control.absorb() {
        transfer_attempt.finish(
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            "reviewer_accounting_failed",
        )?;
        return Err(error);
    }
    let observations = match transfer_result {
        Ok(observations) => observations,
        Err(error) => {
            let (status, reason) = if error == MODEL_REQUEST_CANCELLED || reviewer_preempted {
                (
                    PromptEvaluationAttemptStatus::ForegroundPreempted,
                    "foreground_preempted",
                )
            } else {
                (
                    PromptEvaluationAttemptStatus::InfrastructureInvalid,
                    "reviewer_invalid",
                )
            };
            transfer_attempt.finish(status, reason)?;
            append_prompt_evaluation_status(
                state,
                task_id,
                run_context,
                "Conductor Auto transfer evaluation failed closed",
                &transfer_evaluation_id,
                effort,
                mode,
                &candidate.plan.genome.id,
                &teacher.profile_id,
                Some(&error),
            )?;
            return if error == MODEL_REQUEST_CANCELLED || reviewer_preempted {
                Err(MODEL_REQUEST_CANCELLED.to_string())
            } else {
                Ok(())
            };
        }
    };
    let final_transfer_context = crate::prompt_learning_runtime::prompt_execution_context(
        config,
        effort,
        policy,
        evaluation_worker_models,
        agent_budget,
        &candidate.plan.genome,
        workspace_root,
        transfer_budget,
        control.as_ref(),
    )?;
    if final_transfer_context != transfer_cohort.execution {
        transfer_attempt.finish(
            PromptEvaluationAttemptStatus::InfrastructureInvalid,
            "execution_context_changed",
        )?;
        return Err("Auto transfer execution context changed before persistence".to_string());
    }
    let lease = match control.execution_epoch_lease() {
        agent_runtime::RunEpochLeaseOutcome::Acquired(lease) => lease,
        agent_runtime::RunEpochLeaseOutcome::Stopped(agent_runtime::RunStopReason::UserCancelled) => {
            transfer_attempt.finish(
                PromptEvaluationAttemptStatus::ForegroundPreempted,
                "foreground_preempted",
            )?;
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        _ => {
            transfer_attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "persistence_lease_unavailable",
            )?;
            return Err("Auto transfer persistence lease is unavailable".to_string());
        }
    };
    let transfer_commit = control.commit_execution_step_with(lease, || {
        append_prompt_transfer_observations(
            state,
            task_id,
            run_context,
            effort,
            mode,
            [&observations[0], &observations[1]],
        )
    });
    match transfer_commit {
        Ok(agent_runtime::RunExecutionStepCommit::Committed(())) => {}
        Ok(agent_runtime::RunExecutionStepCommit::Stopped(
            agent_runtime::RunStopReason::UserCancelled,
        )) => {
            transfer_attempt.finish(
                PromptEvaluationAttemptStatus::ForegroundPreempted,
                "foreground_preempted",
            )?;
            return Err(MODEL_REQUEST_CANCELLED.to_string());
        }
        Ok(_) => {
            transfer_attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "observation_persistence_stopped",
            )?;
            return Err("Auto transfer stopped before observation persistence".to_string());
        }
        Err(error) => {
            transfer_attempt.finish(
                PromptEvaluationAttemptStatus::InfrastructureInvalid,
                "observation_persistence_failed",
            )?;
            return Err(error);
        }
    }
    if candidate_failed {
        transfer_attempt.finish(
            PromptEvaluationAttemptStatus::TreatmentFailure,
            "treatment_execution_failed",
        )?;
    } else {
        transfer_attempt.finish(PromptEvaluationAttemptStatus::CompletedPair, "")?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn evaluate_prompt_auto_transfer_pair(
    config: &ProviderConfig,
    reviewer_model: &str,
    objective: &str,
    candidate: &PromptExecutionCandidate,
    teacher: &PromptAutoTeacherCase,
    evaluation_id: &str,
    task_class: &str,
    split: PromptEvaluationSplit,
    mode: PromptEvaluationMode,
    matched_identity: &PromptMatchedEvaluationIdentityV1,
    forward_control: &Arc<AgentRunControl>,
    reverse_control: &Arc<AgentRunControl>,
) -> Result<[PromptEvolutionObservation; 2], String> {
    let teacher_candidate = prompt_auto_teacher_candidate(teacher);
    let participant_models = prompt_candidate_models([candidate, &teacher_candidate])
        .into_iter()
        .collect::<Vec<_>>();
    if participant_models
        .iter()
        .any(|model| model == reviewer_model)
    {
        return Err(
            "Auto transfer reviewer is not independent of the compared workflows".to_string(),
        );
    }
    let judge = prompt_evaluation_feedback::evaluate_prompt_candidate_pair_position_balanced(
        config,
        reviewer_model,
        objective,
        candidate,
        &teacher_candidate,
        evaluation_id,
        forward_control,
        reverse_control,
    )?;
    let candidate_sha256 = prompt_genome_sha256(&candidate.plan.genome)?;
    let transfer = PromptTransferProvenance::auto_to_pro(
        teacher.source_run_id.clone(),
        teacher.steer_epoch,
        teacher.profile_id.clone(),
        teacher.profile_sha256.clone(),
        teacher.output_sha256.clone(),
    );
    let candidate_observation = prompt_evaluation_feedback::prompt_pairwise_observation(
        candidate,
        &teacher_candidate,
        objective,
        evaluation_id,
        task_class,
        split,
        mode,
        judge.score_a,
        judge.score_b,
        judge.safety_violations_a,
        &judge.step_scores_a,
        judge.feedback_a,
        std::slice::from_ref(&config.api_key),
        PromptEvaluationProvenance::blind_pairwise_swap(
            vec![reviewer_model.to_string()],
            participant_models.clone(),
            matched_identity.dataset_sha256.clone(),
            candidate_sha256.clone(),
            teacher.profile_sha256.clone(),
        )
        .with_transfer(transfer.clone())
        .with_matched_evaluation(matched_identity.clone()),
    );
    let teacher_observation = prompt_evaluation_feedback::prompt_pairwise_observation(
        &teacher_candidate,
        candidate,
        objective,
        evaluation_id,
        task_class,
        split,
        mode,
        judge.score_b,
        judge.score_a,
        judge.safety_violations_b,
        &judge.step_scores_b,
        judge.feedback_b,
        std::slice::from_ref(&config.api_key),
        PromptEvaluationProvenance::blind_pairwise_swap(
            vec![reviewer_model.to_string()],
            participant_models,
            matched_identity.dataset_sha256.clone(),
            teacher.profile_sha256.clone(),
            candidate_sha256,
        )
        .with_transfer(transfer)
        .with_matched_evaluation(matched_identity.clone()),
    );
    Ok([candidate_observation, teacher_observation])
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_prompt_auto_transfer_evaluations(
    state: &tauri::State<'_, AppState>,
    config: &ProviderConfig,
    task_id: &TaskId,
    run_context: &Metadata,
    effort: &str,
    policy: &str,
    evaluation_worker_models: &[String],
    agent_budget: usize,
    workspace_root: &Path,
    dataset: &[PromptOfflineCase],
    teacher: Option<&PromptAutoTeacherCase>,
    auto_stable_profile: Option<&(ConductorPromptGenome, String)>,
    selected_case: &PromptOfflineCase,
    split: PromptEvaluationSplit,
    mode: PromptEvaluationMode,
    control: &Arc<AgentRunControl>,
    evaluation_id: &str,
    objective: &str,
    task_class: &str,
    reviewer_model: &str,
    candidates: [&PromptExecutionCandidate; 2],
) -> Result<(), String> {
    let (Some(teacher), Some((auto_profile, auto_profile_sha256))) =
        (teacher, auto_stable_profile)
    else {
        return Ok(());
    };
    let Some(transfer_dataset_identity) =
        prompt_auto_transfer_dataset_identity(dataset, &auto_profile.id, auto_profile_sha256)
    else {
        return Ok(());
    };
    let [current_control, challenger_control] = [
        PromptTreatmentControlGroup::new(control.as_ref(), 2, 4)?,
        PromptTreatmentControlGroup::new(control.as_ref(), 2, 4)?,
    ];
    let transfer_dataset_identity = &transfer_dataset_identity;
    let current_candidate = candidates[0];
    let challenger_candidate = candidates[1];
    let results = std::thread::scope(|scope| {
        let current = scope.spawn(move || {
            run_prompt_auto_transfer_evaluation(
                state,
                config,
                task_id,
                run_context,
                effort,
                policy,
                evaluation_worker_models,
                agent_budget,
                workspace_root,
                teacher,
                transfer_dataset_identity,
                selected_case,
                split,
                mode,
                control,
                evaluation_id,
                objective,
                task_class,
                reviewer_model,
                "current",
                current_candidate,
                current_control,
            )
        });
        let challenger = scope.spawn(move || {
            run_prompt_auto_transfer_evaluation(
                state,
                config,
                task_id,
                run_context,
                effort,
                policy,
                evaluation_worker_models,
                agent_budget,
                workspace_root,
                teacher,
                transfer_dataset_identity,
                selected_case,
                split,
                mode,
                control,
                evaluation_id,
                objective,
                task_class,
                reviewer_model,
                "challenger",
                challenger_candidate,
                challenger_control,
            )
        });
        [
            current.join().unwrap_or_else(|_| {
                Err("current Auto transfer evaluation panicked".to_string())
            }),
            challenger.join().unwrap_or_else(|_| {
                Err("challenger Auto transfer evaluation panicked".to_string())
            }),
        ]
    });
    if results
        .iter()
        .any(|result| result.as_ref().is_err_and(|error| error == MODEL_REQUEST_CANCELLED))
    {
        return Err(MODEL_REQUEST_CANCELLED.to_string());
    }
    results.into_iter().collect::<Result<Vec<_>, _>>()?;
    Ok(())
}
