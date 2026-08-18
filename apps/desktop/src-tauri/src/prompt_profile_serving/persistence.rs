use super::model::{PromptProfileDeploymentRecord, PromptProfileDeploymentState};
use super::{
    prompt_profile_deployment_key, scope_sha256, valid_scope,
    PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
    PROMPT_PROFILE_SCOPE_FENCE_NAMESPACE, PROMPT_PROFILE_SCOPE_FENCE_SCHEMA,
};
use agent_storage::{SqliteStore, StorageError, StoredReadModel};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct PromptProfileScopeFence {
    schema: String,
    scope_sha256: String,
    epoch: u64,
    deleted: bool,
}

pub(crate) fn delete_prompt_profile_deployments_for_scope_in_transaction(
    store: &mut SqliteStore,
    scope: &str,
) -> Result<usize, StorageError> {
    let scope = scope.trim();
    if !valid_scope(scope) {
        return Err(StorageError::new(
            "prompt profile deployment scope is invalid",
        ));
    }
    let fence_key = scope_sha256(scope);
    let current_fence = store.load_read_model(PROMPT_PROFILE_SCOPE_FENCE_NAMESPACE, &fence_key)?;
    let epoch = next_generation(current_fence.as_ref())?;
    let fence = PromptProfileScopeFence {
        schema: PROMPT_PROFILE_SCOPE_FENCE_SCHEMA.to_string(),
        scope_sha256: fence_key.clone(),
        epoch,
        deleted: true,
    };
    let fence_payload = serde_json::to_string(&fence)
        .map_err(|error| StorageError::new(format!("scope fence serialization failed: {error}")))?;
    store.save_read_model(
        PROMPT_PROFILE_SCOPE_FENCE_NAMESPACE,
        &fence_key,
        epoch,
        &fence_payload,
    )?;

    let mut deleted = 0;
    for effort in ["auto", "pro"] {
        let key = prompt_profile_deployment_key(scope, effort);
        let current = store.load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)?;
        if current.as_ref().is_some_and(|stored| {
            serde_json::from_str::<PromptProfileDeploymentRecord>(&stored.payload)
                .map(|record| record.state == PromptProfileDeploymentState::Active)
                .unwrap_or(true)
        }) {
            deleted += 1;
        }
        store.delete_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)?;
        store.delete_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)?;
    }
    Ok(deleted)
}

fn next_generation(current: Option<&StoredReadModel>) -> Result<u64, StorageError> {
    current.map_or(Ok(1), |stored| {
        stored
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::new("prompt profile deployment generation overflow"))
    })
}
