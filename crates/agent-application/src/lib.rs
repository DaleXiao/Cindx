mod artifacts;
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
    AgentStrategyDecisionReceipt, AgentStrategyReceiptError,
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
