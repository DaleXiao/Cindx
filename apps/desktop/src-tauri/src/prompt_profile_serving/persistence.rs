#[cfg(test)]
use super::binding::active_binding_matches_in_transaction;
use super::binding::{
    delete_binding_in_transaction, reconcile_active_binding_in_transaction,
    reconcile_stored_withdrawn_binding_in_transaction, reconcile_withdrawn_binding_in_transaction,
    withdraw_orphaned_binding_in_transaction,
};
use super::deployment::{canonical_prompt_profile_snapshot_sha256, PromptProfileDeploymentCatalog};
use super::model::{
    PromptProfileDeploymentLineage, PromptProfileDeploymentRecord, PromptProfileDeploymentState,
};
use super::{
    prompt_profile_deployment_key, scope_sha256, valid_scope, PromptProfileDeployment,
    PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, PROMPT_PROFILE_SCOPE_FENCE_NAMESPACE,
    PROMPT_PROFILE_SCOPE_FENCE_SCHEMA,
};
use crate::{
    runtime_constants::{
        PROMPT_EVOLUTION_READ_MODEL_KEY, PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
        PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
    },
    runtime_values::phase16_task_id,
    view_models::PromptEvolutionReadModel,
};
use agent_storage::{SqliteStore, StorageError, StoredReadModel};
use orchestrator::sha256_hex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct PromptProfileScopeFence {
    schema: String,
    scope_sha256: String,
    epoch: u64,
    deleted: bool,
}

#[cfg(test)]
pub(crate) fn delete_prompt_profile_deployments_for_scope(
    store: &mut SqliteStore,
    scope: &str,
) -> Result<usize, StorageError> {
    store.with_immediate_transaction(|transaction| {
        delete_prompt_profile_deployments_for_scope_in_transaction(transaction, scope)
    })
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
        delete_binding_in_transaction(store, &key)?;
    }
    Ok(deleted)
}

pub(crate) fn active_prompt_profile_deployment_lineage(
    store: &SqliteStore,
    scope: &str,
    effort: &str,
) -> Result<Option<PromptProfileDeploymentLineage>, StorageError> {
    let scope = scope.trim();
    if !valid_scope(scope) || !matches!(effort, "auto" | "pro") {
        return Ok(None);
    }
    let key = prompt_profile_deployment_key(scope, effort);
    let Some(stored) = store.load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)? else {
        return Ok(None);
    };
    Ok(
        serde_json::from_str::<PromptProfileDeploymentRecord>(&stored.payload)
            .ok()
            .filter(|record| {
                record.generation == stored.revision && record.validate_for(scope, effort).is_ok()
            })
            .and_then(|record| record.active_lineage()),
    )
}

#[cfg(test)]
pub(crate) fn publish_prompt_profile_deployment(
    store: &mut SqliteStore,
    model: &PromptEvolutionReadModel,
    effort: &str,
    scope: &str,
    source_revision: u64,
) -> Result<bool, String> {
    let scope = scope.trim();
    let catalog = PromptProfileDeploymentCatalog::new(model);
    let deployment = catalog.build(scope, effort, source_revision)?;
    let canonical_snapshot_sha256 = canonical_prompt_profile_snapshot_sha256(model, scope, effort)?;
    store
        .with_immediate_transaction(|transaction| {
            if !publication_matches_current_canonical_snapshot(
                transaction,
                scope,
                effort,
                &canonical_snapshot_sha256,
                &deployment,
            )? {
                return Ok(false);
            }
            if !active_binding_matches_in_transaction(
                transaction,
                scope,
                effort,
                deployment.source_revision,
                &canonical_snapshot_sha256,
            )? {
                return Ok(false);
            }
            reconcile_active_deployment_record_in_transaction(
                transaction,
                scope,
                effort,
                deployment,
            )
        })
        .map_err(|error| error.to_string())
}

pub(crate) fn publish_canonical_prompt_profile_deployment(
    store: &mut SqliteStore,
    model: &PromptEvolutionReadModel,
    effort: &str,
    scope: &str,
) -> Result<bool, String> {
    let scope = scope.trim();
    let catalog = PromptProfileDeploymentCatalog::new(model);
    let deployment = catalog.build(scope, effort, model.revision)?;
    let canonical_snapshot_sha256 = canonical_prompt_profile_snapshot_sha256(model, scope, effort)?;
    store
        .with_immediate_transaction(|transaction| {
            if !publication_matches_current_canonical_snapshot(
                transaction,
                scope,
                effort,
                &canonical_snapshot_sha256,
                &deployment,
            )? {
                return Ok(false);
            }
            reconcile_canonical_active_deployment_in_transaction(
                transaction,
                scope,
                effort,
                deployment,
                &canonical_snapshot_sha256,
            )
        })
        .map_err(|error| error.to_string())
}

pub(super) fn reconcile_canonical_active_deployment_in_transaction(
    store: &mut SqliteStore,
    scope: &str,
    effort: &str,
    mut deployment: PromptProfileDeployment,
    canonical_snapshot_sha256: &str,
) -> Result<bool, StorageError> {
    if scope_is_deleted(store, scope)? {
        return Ok(false);
    }
    deployment.source_revision = reconcile_active_binding_in_transaction(
        store,
        scope,
        effort,
        deployment.source_revision,
        canonical_snapshot_sha256,
    )?;
    reconcile_active_deployment_record_in_transaction(store, scope, effort, deployment)
}

fn reconcile_active_deployment_record_in_transaction(
    store: &mut SqliteStore,
    scope: &str,
    effort: &str,
    deployment: PromptProfileDeployment,
) -> Result<bool, StorageError> {
    if scope_is_deleted(store, scope)? {
        return Ok(false);
    }
    let key = prompt_profile_deployment_key(scope, effort);
    let current = store.load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)?;
    let parsed = current.as_ref().and_then(|stored| {
        serde_json::from_str::<PromptProfileDeploymentRecord>(&stored.payload)
            .ok()
            .filter(|record| {
                record.generation == stored.revision && record.validate_for(scope, effort).is_ok()
            })
    });
    if parsed
        .as_ref()
        .and_then(|record| record.deployment.as_ref())
        .is_some_and(|current| current == &deployment)
    {
        return Ok(true);
    }
    let generation = next_generation(current.as_ref())?;
    let record = PromptProfileDeploymentRecord::active(scope, effort, generation, deployment);
    if !compare_exchange_record(store, &key, current.as_ref(), &record)? {
        return Err(StorageError::new(
            "prompt profile deployment publication conflicted",
        ));
    }
    Ok(true)
}

pub(super) fn withdraw_prompt_profile_deployment_in_transaction(
    store: &mut SqliteStore,
    scope: &str,
    effort: &str,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<(), StorageError> {
    if scope_is_deleted(store, scope)? {
        return Ok(());
    }
    reconcile_withdrawn_binding_in_transaction(
        store,
        scope,
        effort,
        source_revision,
        canonical_snapshot_sha256,
    )?;
    let key = prompt_profile_deployment_key(scope, effort);
    let current = store.load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)?;
    let Some(stored) = current.as_ref() else {
        return Ok(());
    };
    if serde_json::from_str::<PromptProfileDeploymentRecord>(&stored.payload)
        .ok()
        .filter(|record| {
            record.generation == stored.revision && record.validate_for(scope, effort).is_ok()
        })
        .is_some_and(|record| record.state != PromptProfileDeploymentState::Active)
    {
        return Ok(());
    }
    let generation = next_generation(Some(stored))?;
    let record = PromptProfileDeploymentRecord::tombstone(
        scope,
        effort,
        generation,
        PromptProfileDeploymentState::Withdrawn,
    );
    if !compare_exchange_record(store, &key, Some(stored), &record)? {
        return Err(StorageError::new(
            "prompt profile withdrawal publication conflicted",
        ));
    }
    Ok(())
}

pub(super) fn withdraw_unplanned_deployment_in_transaction(
    store: &mut SqliteStore,
    key: &str,
    stored: &StoredReadModel,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<(), StorageError> {
    let Some(record) = serde_json::from_str::<PromptProfileDeploymentRecord>(&stored.payload)
        .ok()
        .filter(|record| record.has_valid_stored_identity(stored.revision))
    else {
        delete_binding_in_transaction(store, key)?;
        return store.delete_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, key);
    };
    reconcile_stored_withdrawn_binding_in_transaction(
        store,
        key,
        record.stored_scope_sha256(),
        record.stored_effort(),
        source_revision,
        canonical_snapshot_sha256,
    )?;
    if record.state != PromptProfileDeploymentState::Active {
        return Ok(());
    }
    let generation = next_generation(Some(stored))?;
    let withdrawn = record.withdrawn_from_stored(generation);
    if !compare_exchange_record(store, key, Some(stored), &withdrawn)? {
        return Err(StorageError::new(
            "unplanned prompt profile withdrawal conflicted",
        ));
    }
    Ok(())
}

pub(super) fn withdraw_binding_only_deployment_in_transaction(
    store: &mut SqliteStore,
    key: &str,
    stored_binding: &StoredReadModel,
    source_revision: u64,
    canonical_snapshot_sha256: &str,
) -> Result<(), StorageError> {
    let Some(tombstone) = withdraw_orphaned_binding_in_transaction(
        store,
        key,
        stored_binding,
        source_revision,
        canonical_snapshot_sha256,
    )?
    else {
        return Ok(());
    };
    let record = PromptProfileDeploymentRecord::tombstone_from_stored_identity(
        tombstone.scope_sha256,
        tombstone.effort,
        tombstone.generation,
    );
    if !compare_exchange_record(store, key, None, &record)? {
        return Err(StorageError::new(
            "orphaned prompt profile tombstone publication conflicted",
        ));
    }
    Ok(())
}

pub(super) fn canonical_read_model_matches(
    store: &SqliteStore,
    expected: &PromptEvolutionReadModel,
) -> Result<bool, StorageError> {
    let revision = store.event_revision(&phase16_task_id())?;
    if cfg!(test) && revision.latest_sequence == 0 && revision.event_count == 0 {
        return Ok(true);
    }
    let Some(current) = load_current_canonical_model(store, revision)? else {
        return Ok(false);
    };
    Ok(prompt_evolution_model_sha256(&current)? == prompt_evolution_model_sha256(expected)?)
}

fn publication_matches_current_canonical_snapshot(
    store: &SqliteStore,
    scope: &str,
    effort: &str,
    expected_snapshot_sha256: &str,
    desired: &PromptProfileDeployment,
) -> Result<bool, StorageError> {
    let revision = store.event_revision(&phase16_task_id())?;
    if cfg!(test) && revision.latest_sequence == 0 && revision.event_count == 0 {
        return Ok(true);
    }
    let Some(current) = load_current_canonical_model(store, revision)? else {
        return Ok(false);
    };
    let current_snapshot_sha256 =
        match canonical_prompt_profile_snapshot_sha256(&current, scope, effort) {
            Ok(identity) => identity,
            Err(_) => return Ok(false),
        };
    if current_snapshot_sha256 != expected_snapshot_sha256 {
        return Ok(false);
    }
    let current_deployment = match PromptProfileDeploymentCatalog::new(&current).build(
        scope,
        effort,
        current.revision,
    ) {
        Ok(deployment) => deployment,
        Err(_) => return Ok(false),
    };
    Ok(&current_deployment == desired)
}

fn load_current_canonical_model(
    store: &SqliteStore,
    revision: agent_storage::EventRevision,
) -> Result<Option<PromptEvolutionReadModel>, StorageError> {
    let Some(stored) = store.load_read_model(
        PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
        PROMPT_EVOLUTION_READ_MODEL_KEY,
    )?
    else {
        return Ok(None);
    };
    Ok(
        serde_json::from_str::<PromptEvolutionReadModel>(&stored.payload)
            .ok()
            .filter(|model| {
                model.schema == PROMPT_EVOLUTION_READ_MODEL_NAMESPACE
                    && model.projection_version == PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION
                    && model.revision == stored.revision
                    && model.revision == revision.latest_sequence
                    && model.event_count == revision.event_count
            }),
    )
}

pub(super) fn prompt_evolution_model_sha256(
    model: &PromptEvolutionReadModel,
) -> Result<String, StorageError> {
    let payload = serde_json::to_vec(model).map_err(|error| {
        StorageError::new(format!(
            "prompt evolution canonical snapshot serialization failed: {error}"
        ))
    })?;
    Ok(sha256_hex(&payload))
}

fn scope_is_deleted(store: &SqliteStore, scope: &str) -> Result<bool, StorageError> {
    let key = scope_sha256(scope);
    let Some(stored) = store.load_read_model(PROMPT_PROFILE_SCOPE_FENCE_NAMESPACE, &key)? else {
        return Ok(false);
    };
    Ok(
        serde_json::from_str::<PromptProfileScopeFence>(&stored.payload).map_or(true, |fence| {
            fence.schema != PROMPT_PROFILE_SCOPE_FENCE_SCHEMA
                || fence.scope_sha256 != key
                || fence.epoch != stored.revision
                || fence.deleted
        }),
    )
}

fn next_generation(current: Option<&StoredReadModel>) -> Result<u64, StorageError> {
    current.map_or(Ok(1), |stored| {
        stored
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::new("prompt profile deployment generation overflow"))
    })
}

fn compare_exchange_record(
    store: &mut SqliteStore,
    key: &str,
    current: Option<&StoredReadModel>,
    record: &PromptProfileDeploymentRecord,
) -> Result<bool, StorageError> {
    let payload = serde_json::to_string(record).map_err(|error| {
        StorageError::new(format!(
            "prompt profile deployment serialization failed: {error}"
        ))
    })?;
    store.compare_exchange_read_model(
        PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
        key,
        current,
        record.generation,
        &payload,
    )
}
