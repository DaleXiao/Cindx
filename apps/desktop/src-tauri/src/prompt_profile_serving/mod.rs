mod binding;
#[cfg(test)]
mod consistency_tests;
mod continuity;
mod deployment;
mod model;
mod persistence;
mod recovery;
mod selection;
#[cfg(test)]
mod tests;

pub(crate) use continuity::{
    copy_prompt_profile_assignment, prompt_profile_assignment_from_events,
};
#[cfg(test)]
pub(crate) use model::PromptProfileDistillationLease;
pub(crate) use model::{
    PromptProfileAssignmentSource, PromptProfileDeployment, PromptProfileDeploymentLineage,
    PromptProfileFallback, PromptProfileSelection,
};
pub(crate) use persistence::{
    active_prompt_profile_deployment_lineage,
    delete_prompt_profile_deployments_for_scope_in_transaction,
    publish_canonical_prompt_profile_deployment, validate_canonical_prompt_profile_deployment,
};
#[cfg(test)]
pub(crate) use persistence::{
    delete_prompt_profile_deployments_for_scope, publish_prompt_profile_deployment,
};
pub(crate) use recovery::recover_prompt_profile_deployments;
#[cfg(any(test, feature = "realworld-eval"))]
pub(crate) use selection::frozen_prompt_profile_selection;
pub(crate) use selection::{
    restore_prompt_profile_selection, seed_prompt_profile_selection, select_prompt_profile_for_run,
};

use orchestrator::sha256_hex;

pub(crate) const PROMPT_PROFILE_DEPLOYMENT_NAMESPACE: &str = "cindx.prompt-profile-deployment.v1";
pub(super) const PROMPT_PROFILE_DEPLOYMENT_SCHEMA: &str = "cindx.prompt-profile-deployment.v1";
pub(super) const PROMPT_PROFILE_RECORD_SCHEMA: &str = "cindx.prompt-profile-record.v1";
pub(super) const PROMPT_PROFILE_SCOPE_FENCE_NAMESPACE: &str = "cindx.prompt-profile-scope-fence.v1";
pub(super) const PROMPT_PROFILE_SCOPE_FENCE_SCHEMA: &str = "cindx.prompt-profile-scope-fence.v1";
pub(super) const PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE: &str =
    "cindx.prompt-profile-canonical-binding.v1";
pub(super) const PROMPT_PROFILE_CANONICAL_BINDING_SCHEMA: &str =
    "cindx.prompt-profile-canonical-binding.v1";
pub(super) const PROMPT_PROFILE_ASSIGNMENT_SCHEMA: &str = "cindx.prompt-profile-assignment.v1";
pub(super) const MAX_ASSIGNMENT_RECEIPT_BYTES: usize = 4 * 1024;
pub(super) const MAX_PROFILE_ID_BYTES: usize = 128;
const MAX_SCOPE_BYTES: usize = 512;

pub(crate) fn prompt_profile_deployment_key(scope: &str, effort: &str) -> String {
    sha256_hex(format!("cindx.prompt-profile-deployment-key.v1\0{scope}\0{effort}").as_bytes())
}

pub(super) fn scope_sha256(scope: &str) -> String {
    sha256_hex(format!("cindx.prompt-profile-scope.v1\0{}", scope.trim()).as_bytes())
}

pub(super) fn normalize_effort(effort: &str) -> &str {
    match effort.trim() {
        "fast" => "fast",
        "pro" => "pro",
        "auto" => "auto",
        _ => "invalid",
    }
}

pub(super) fn valid_scope(scope: &str) -> bool {
    let scope = scope.trim();
    !scope.is_empty() && scope.len() <= MAX_SCOPE_BYTES
}
