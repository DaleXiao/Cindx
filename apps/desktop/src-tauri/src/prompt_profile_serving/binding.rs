use super::model::PromptProfileDeploymentState;
use super::{
    prompt_profile_deployment_key, scope_sha256, PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE,
    PROMPT_PROFILE_CANONICAL_BINDING_SCHEMA,
};
use agent_storage::{SqliteStore, StorageError, StoredReadModel};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PromptProfileCanonicalBinding {
    schema: String,
    scope_sha256: String,
    effort: String,
    epoch: u64,
    state: PromptProfileDeploymentState,
    source_revision: u64,
    canonical_snapshot_sha256: String,
}

pub(super) struct OrphanedBindingTombstone {
    pub(super) scope_sha256: String,
    pub(super) effort: String,
    pub(super) generation: u64,
}

pub(super) fn reconcile_active_binding_in_transaction(
    store: &mut SqliteStore,
    scope: &str,
    effort: &str,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<u64, StorageError> {
    reconcile_binding_in_transaction(
        store,
        &prompt_profile_deployment_key(scope, effort),
        &scope_sha256(scope),
        effort,
        PromptProfileDeploymentState::Active,
        source_revision,
        canonical_snapshot_sha256,
    )
}

#[cfg(test)]
pub(super) fn active_binding_matches_in_transaction(
    store: &SqliteStore,
    scope: &str,
    effort: &str,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<bool, StorageError> {
    let key = prompt_profile_deployment_key(scope, effort);
    let expected_scope_sha256 = scope_sha256(scope);
    let Some(stored) = store.load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)?
    else {
        return Ok(false);
    };
    Ok(
        serde_json::from_str::<PromptProfileCanonicalBinding>(&stored.payload)
            .ok()
            .filter(|binding| binding.validate_for(&expected_scope_sha256, effort, stored.revision))
            .is_some_and(|binding| {
                binding.state == PromptProfileDeploymentState::Active
                    && binding.source_revision == source_revision
                    && binding.canonical_snapshot_sha256 == canonical_snapshot_sha256
            }),
    )
}

pub(super) fn reconcile_withdrawn_binding_in_transaction(
    store: &mut SqliteStore,
    scope: &str,
    effort: &str,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<u64, StorageError> {
    reconcile_binding_in_transaction(
        store,
        &prompt_profile_deployment_key(scope, effort),
        &scope_sha256(scope),
        effort,
        PromptProfileDeploymentState::Withdrawn,
        source_revision,
        canonical_snapshot_sha256,
    )
}

pub(super) fn reconcile_stored_withdrawn_binding_in_transaction(
    store: &mut SqliteStore,
    key: &str,
    stored_scope_sha256: &str,
    effort: &str,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<u64, StorageError> {
    reconcile_binding_in_transaction(
        store,
        key,
        stored_scope_sha256,
        effort,
        PromptProfileDeploymentState::Withdrawn,
        source_revision,
        canonical_snapshot_sha256,
    )
}

pub(super) fn delete_binding_in_transaction(
    store: &mut SqliteStore,
    key: &str,
) -> Result<(), StorageError> {
    store.delete_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, key)
}

pub(super) fn withdraw_orphaned_binding_in_transaction(
    store: &mut SqliteStore,
    key: &str,
    stored: &StoredReadModel,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<Option<OrphanedBindingTombstone>, StorageError> {
    let Some(binding) = serde_json::from_str::<PromptProfileCanonicalBinding>(&stored.payload)
        .ok()
        .filter(|binding| binding.validate_stored(stored.revision))
    else {
        delete_binding_in_transaction(store, key)?;
        return Ok(None);
    };
    reconcile_binding_in_transaction(
        store,
        key,
        &binding.scope_sha256,
        &binding.effort,
        PromptProfileDeploymentState::Withdrawn,
        source_revision,
        canonical_snapshot_sha256,
    )?;
    let generation = store
        .load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, key)?
        .ok_or_else(|| StorageError::new("orphaned prompt profile binding disappeared"))?
        .revision;
    Ok(Some(OrphanedBindingTombstone {
        scope_sha256: binding.scope_sha256,
        effort: binding.effort,
        generation,
    }))
}

fn reconcile_binding_in_transaction(
    store: &mut SqliteStore,
    key: &str,
    expected_scope_sha256: &str,
    effort: &str,
    state: PromptProfileDeploymentState,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<u64, StorageError> {
    if !is_sha256(expected_scope_sha256)
        || !matches!(effort, "auto" | "pro")
        || !is_sha256(canonical_snapshot_sha256)
    {
        return Err(StorageError::new(
            "prompt profile canonical binding identity is invalid",
        ));
    }
    let current = store.load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, key)?;
    let parsed = current.as_ref().and_then(|stored| {
        serde_json::from_str::<PromptProfileCanonicalBinding>(&stored.payload)
            .ok()
            .filter(|binding| binding.validate_for(expected_scope_sha256, effort, stored.revision))
    });
    if parsed.as_ref().is_some_and(|binding| {
        binding.state == state
            && binding.canonical_snapshot_sha256 == canonical_snapshot_sha256
            && source_revision >= binding.source_revision
    }) {
        return Ok(parsed
            .expect("matched canonical binding was parsed")
            .source_revision);
    }
    let epoch = next_epoch(current.as_ref())?;
    let binding = PromptProfileCanonicalBinding {
        schema: PROMPT_PROFILE_CANONICAL_BINDING_SCHEMA.to_string(),
        scope_sha256: expected_scope_sha256.to_string(),
        effort: effort.to_string(),
        epoch,
        state,
        source_revision,
        canonical_snapshot_sha256: canonical_snapshot_sha256.to_string(),
    };
    let payload = serde_json::to_string(&binding).map_err(|error| {
        StorageError::new(format!(
            "prompt profile canonical binding serialization failed: {error}"
        ))
    })?;
    if !store.compare_exchange_read_model(
        PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE,
        key,
        current.as_ref(),
        epoch,
        &payload,
    )? {
        return Err(StorageError::new(
            "prompt profile canonical binding publication conflicted",
        ));
    }
    Ok(source_revision)
}

impl PromptProfileCanonicalBinding {
    fn validate_for(&self, scope_sha256: &str, effort: &str, revision: u64) -> bool {
        self.validate_stored(revision) && self.scope_sha256 == scope_sha256 && self.effort == effort
    }

    fn validate_stored(&self, revision: u64) -> bool {
        self.schema == PROMPT_PROFILE_CANONICAL_BINDING_SCHEMA
            && is_sha256(&self.scope_sha256)
            && matches!(self.effort.as_str(), "auto" | "pro")
            && self.epoch == revision
            && self.epoch > 0
            && is_sha256(&self.canonical_snapshot_sha256)
    }
}

fn next_epoch(current: Option<&StoredReadModel>) -> Result<u64, StorageError> {
    current.map_or(Ok(1), |stored| {
        stored
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::new("prompt profile canonical binding epoch overflow"))
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
