mod artifacts;
mod collaboration_learning_admission;
mod collaboration_learning_holdout;
mod collaboration_learning_policy;
mod collaboration_learning_projection;
#[cfg(feature = "collaboration-learning-offline")]
mod collaboration_learning_replay;
mod direct_judge_fitness;
mod direct_judge_outcome;
mod loss_aware_work_queue;
mod outcome_evidence;
mod outcome_projection;
mod recovery;
mod run_execution;
mod run_lifecycle;
mod run_persistence;
mod sessions;
mod strategy_decision;
mod terminal_commit;

pub use artifacts::{
    artifact_kind_from_path, artifact_manifest_message, project_agent_artifacts,
    AgentOutputArtifact,
};
pub use collaboration_learning_admission::{
    collaboration_learning_physical_run_sha256, CollaborationLearningAggregateStatusV1,
    CollaborationLearningAggregateV1, CollaborationLearningArmOrderV1, CollaborationLearningArmV1,
    CollaborationLearningCandidateV1, CollaborationLearningCensorReasonV1,
    CollaborationLearningCensorReceiptV1, CollaborationLearningComparisonBindingV1,
    CollaborationLearningComparisonHashesV1, CollaborationLearningConfigV1,
    CollaborationLearningEvidenceSetV1, CollaborationLearningFreezeReasonV1,
    CollaborationLearningOfflineAdmissionStatusV1, CollaborationLearningOfflineAdmissionV1,
    CollaborationLearningPairV1, CollaborationLearningReviewReceiptV1,
    CollaborationLearningSplitV1, CollaborationLearningTrialV1,
    COLLABORATION_LEARNING_CANDIDATE_SCHEMA, COLLABORATION_LEARNING_COMPARISON_SCHEMA,
    COLLABORATION_LEARNING_CONFIG_SCHEMA, COLLABORATION_LEARNING_OFFLINE_ADMISSION_SCHEMA,
    COLLABORATION_LEARNING_REVIEW_SCHEMA,
};
pub use collaboration_learning_holdout::{
    CollaborationLearningHoldoutCensorReceiptV1, CollaborationLearningHoldoutReservationV1,
    COLLABORATION_LEARNING_HOLDOUT_CENSOR_SCHEMA,
    COLLABORATION_LEARNING_HOLDOUT_RESERVATION_SCHEMA,
};
pub use collaboration_learning_policy::{
    CollaborationGoal2LimitsV1, CollaborationLearningError, CollaborationLearningPolicyV1,
    CollaborationPolicyAxisV1, CollaborationRepairV1, CollaborationSpecialistInvocationV1,
    CollaborationStopV1, CollaborationVerificationV1, COLLABORATION_CONTEXT_BUDGET_COMPACT_BPS,
    COLLABORATION_CONTEXT_BUDGET_EXPANDED_BPS, COLLABORATION_LEARNING_POLICY_SCHEMA,
};
pub use collaboration_learning_projection::{
    CollaborationDerivedStopReasonV1, CollaborationLearningAssignmentV1,
    CollaborationLearningContextReceiptV1, CollaborationLearningExerciseV1,
    CollaborationLearningLaneActorV1, CollaborationLearningLaneAssignmentV1,
    CollaborationLearningLaneExerciseV1, CollaborationLearningOutputKindV1,
    CollaborationLearningWorkerAttemptV1, COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA,
    COLLABORATION_LEARNING_ASSIGNMENT_SCHEMA_METADATA_KEY,
    COLLABORATION_LEARNING_ASSIGNMENT_SUMMARY,
    COLLABORATION_LEARNING_CASE_BINDING_SHA256_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_BUDGET_BPS_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_BYTES_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_COMMITTED_SUMMARY,
    COLLABORATION_LEARNING_CONTEXT_PAYLOAD_SHA256_METADATA_KEY,
    COLLABORATION_LEARNING_CONTEXT_RECEIPT_SCHEMA,
    COLLABORATION_LEARNING_CONTEXT_SCHEMA_METADATA_KEY, COLLABORATION_LEARNING_EXERCISE_SCHEMA,
    COLLABORATION_LEARNING_PLAN_REQUIRED_INDEPENDENT_VERIFIER_METADATA_KEY,
    COLLABORATION_LEARNING_POLICY_JSON_METADATA_KEY,
    COLLABORATION_LEARNING_POLICY_SHA256_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_OUTPUT_KIND_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_REPAIR_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_SPECIALIST_STEP_ID_METADATA_KEY,
    COLLABORATION_LEARNING_VERIFIER_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_VERIFIER_REPAIR_MODEL_METADATA_KEY,
    COLLABORATION_LEARNING_VERIFIER_STEP_ID_METADATA_KEY,
    COLLABORATION_LEARNING_WORKER_TURN_ORDINAL_METADATA_KEY,
};
#[cfg(feature = "collaboration-learning-offline")]
pub use collaboration_learning_replay::{
    CollaborationLearningOfflineEntryV1, CollaborationLearningOfflineGenesisV1,
    CollaborationLearningOfflineReplayV1, COLLABORATION_LEARNING_OFFLINE_ENTRY_SCHEMA,
    COLLABORATION_LEARNING_OFFLINE_GENESIS_SCHEMA,
};
pub use direct_judge_fitness::{
    summarize_direct_judge_fitness, DirectJudgeFitnessSignalV1, DirectJudgeFitnessSummaryV1,
    DIRECT_JUDGE_FITNESS_SIGNAL_SCHEMA, DIRECT_JUDGE_FITNESS_SUMMARY_SCHEMA,
    DIRECT_JUDGE_FITNESS_WINDOW,
};
pub use direct_judge_outcome::{
    disposition_family, disposition_used_repair_round, DirectJudgeCompletionFacts,
    DirectJudgeDispositionFamilyV1, DirectJudgeMutationVerificationV1, DirectJudgeOutcomeError,
    DirectJudgeOutcomeV1, DIRECT_JUDGE_DISPOSITION_INCONCLUSIVE,
    DIRECT_JUDGE_DISPOSITION_NOT_APPLICABLE, DIRECT_JUDGE_DISPOSITION_NOT_ELIGIBLE,
    DIRECT_JUDGE_DISPOSITION_PASSED, DIRECT_JUDGE_DISPOSITION_RECHECK_EXHAUSTED,
    DIRECT_JUDGE_DISPOSITION_RECHECK_INCONCLUSIVE, DIRECT_JUDGE_DISPOSITION_RECHECK_PASSED,
    DIRECT_JUDGE_DISPOSITION_REPAIR_EMPTY, DIRECT_JUDGE_DISPOSITION_REPAIR_UNAVAILABLE,
    DIRECT_JUDGE_DISPOSITION_REPAIR_UNGROUNDED, DIRECT_JUDGE_DISPOSITION_UNAVAILABLE,
    DIRECT_JUDGE_OUTCOME_SCHEMA,
};
pub use loss_aware_work_queue::{
    ClaimedWork, LossAwareWorkQueue, WorkEnqueueOutcome, WorkEnqueueResult, WorkQueueMetrics,
    WorkQueuePolicy, WorkRetryOutcome, WorkRetryResult,
};
pub use outcome_evidence::{
    AgentExternalPostconditionV1, AgentExternalVerifierV1, AgentOutcomeDispositionV1,
    AgentOutcomeEvidenceError, AgentOutcomeExposureV1, AgentOutcomeLifecycleBindingV1,
    AgentOutcomeResourcesV1, AgentOutcomeTerminalResourcesV1, AgentOutcomeTerminalStatusV1,
    AgentOutcomeUsageCompletenessV1, AgentOutcomeUsageV1, ExternallyVerifiedOutcomeV1,
    EXTERNALLY_VERIFIED_OUTCOME_SCHEMA,
};
pub use recovery::{
    AgentRecoveryIdentity, AgentRecoveryReason, AgentRecoveryState, ResolvedAgentRecovery,
};
pub use run_execution::{execute_agent_run, AgentRunEpoch, AgentRunExecutor, AgentRunPreparation};
pub use run_lifecycle::{
    is_agent_model_turn_finished, is_agent_model_turn_started, AgentRunEvent,
    AgentRunEventDecodeError, AgentRunStatus,
};
pub use run_persistence::{
    insert_run_objectives, insert_run_start_prompts, insert_runtime_message_display_prompt,
    insert_user_message_model_prompt, merge_persistable_run_context,
};
pub use sessions::{
    project_session_lifecycle, SessionLifecycleInput, SessionLifecycleProjection, SessionTitleState,
};
pub use strategy_decision::{
    insert_strategy_not_selected, strategy_receipt_is_explicitly_not_selected,
    AgentStrategyDecisionReceipt, AgentStrategyReceiptError, AgentStrategyReceiptState,
    AGENT_STRATEGY_RECEIPT_EPOCH_METADATA_KEY, AGENT_STRATEGY_RECEIPT_KEY_METADATA_KEY,
    AGENT_STRATEGY_RECEIPT_NOT_SELECTED, AGENT_STRATEGY_RECEIPT_PLAN_METADATA_KEY,
    AGENT_STRATEGY_RECEIPT_SCHEMA, AGENT_STRATEGY_RECEIPT_SCHEMA_METADATA_KEY,
    AGENT_STRATEGY_RECEIPT_SELECTED, AGENT_STRATEGY_RECEIPT_STATUS_METADATA_KEY,
};
pub use terminal_commit::{
    AgentTerminalCommitError, AgentTerminalCommitIdentity, AgentTerminalCommitState,
    AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY, AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY,
    AGENT_TERMINAL_COMMIT_SCHEMA, AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY,
};
