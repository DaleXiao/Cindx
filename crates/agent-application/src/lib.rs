mod artifacts;
mod loss_aware_work_queue;
mod recovery;
mod run_execution;
mod run_lifecycle;
mod run_persistence;
mod sessions;
mod terminal_commit;

pub use artifacts::{
    artifact_kind_from_path, artifact_manifest_message, project_agent_artifacts,
    AgentOutputArtifact,
};
pub use loss_aware_work_queue::{
    ClaimedWork, LossAwareWorkQueue, WorkEnqueueOutcome, WorkEnqueueResult, WorkQueueMetrics,
    WorkQueuePolicy, WorkRetryOutcome, WorkRetryResult,
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
pub use terminal_commit::{
    AgentTerminalCommitError, AgentTerminalCommitIdentity, AgentTerminalCommitState,
    AGENT_TERMINAL_COMMIT_EPOCH_METADATA_KEY, AGENT_TERMINAL_COMMIT_KEY_METADATA_KEY,
    AGENT_TERMINAL_COMMIT_SCHEMA, AGENT_TERMINAL_COMMIT_SCHEMA_METADATA_KEY,
};
