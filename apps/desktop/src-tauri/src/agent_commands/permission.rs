use crate::*;
use agent_core::{AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};

mod commands;
mod observations;
mod recovery;
mod resolution;
#[cfg(test)]
mod tests;

pub(crate) use commands::resolve_agent_permission;
pub(crate) use recovery::recovery_task_state_with_persisted_permission_denials;

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
