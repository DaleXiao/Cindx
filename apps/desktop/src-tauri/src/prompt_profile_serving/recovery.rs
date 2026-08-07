use super::deployment::{canonical_prompt_profile_snapshot_sha256, PromptProfileDeploymentCatalog};
use super::persistence::{
    canonical_read_model_matches, reconcile_canonical_active_deployment_in_transaction,
    withdraw_binding_only_deployment_in_transaction,
    withdraw_prompt_profile_deployment_in_transaction,
    withdraw_unplanned_deployment_in_transaction,
};
use super::{
    prompt_profile_deployment_key, PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE,
    PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
};
use crate::view_models::PromptEvolutionReadModel;
use agent_storage::{SqliteStore, StorageError};
use orchestrator::sha256_hex;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn recover_prompt_profile_deployments(
    store: &mut SqliteStore,
    model: &PromptEvolutionReadModel,
) -> Result<usize, String> {
    let catalog = PromptProfileDeploymentCatalog::new(model);
    let plan = catalog
        .scope_efforts()
        .map(|(scope, effort)| {
            let key = prompt_profile_deployment_key(scope, effort);
            Ok((
                key,
                (
                    scope.clone(),
                    effort.clone(),
                    canonical_prompt_profile_snapshot_sha256(model, scope, effort)?,
                    catalog.build(scope, effort, model.revision),
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    store
        .with_immediate_transaction(|transaction| reconcile_plan(transaction, model, plan))
        .map_err(|error| error.to_string())
}

fn reconcile_plan(
    store: &mut SqliteStore,
    model: &PromptEvolutionReadModel,
    plan: BTreeMap<
        String,
        (
            String,
            String,
            String,
            Result<super::PromptProfileDeployment, String>,
        ),
    >,
) -> Result<usize, StorageError> {
    if !canonical_read_model_matches(store, model)? {
        return Err(StorageError::new(
            "prompt profile recovery source is no longer canonical",
        ));
    }
    let stored = store.list_read_models_in_namespace(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE)?;
    let stored_bindings =
        store.list_read_models_in_namespace(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE)?;
    let stored_deployment_keys = stored
        .iter()
        .map(|(key, _)| key.clone())
        .collect::<BTreeSet<_>>();
    let desired_keys = plan.keys().cloned().collect::<BTreeSet<_>>();
    let mut active = 0;
    for (_, (scope, effort, canonical_snapshot_sha256, desired)) in plan {
        match desired {
            Ok(deployment) => {
                if reconcile_canonical_active_deployment_in_transaction(
                    store,
                    &scope,
                    &effort,
                    deployment,
                    &canonical_snapshot_sha256,
                )? {
                    active += 1;
                }
            }
            Err(_) => {
                withdraw_prompt_profile_deployment_in_transaction(
                    store,
                    &scope,
                    &effort,
                    model.revision,
                    &canonical_snapshot_sha256,
                )?;
            }
        }
    }
    for (key, record) in stored {
        if !desired_keys.contains(&key) {
            let canonical_snapshot_sha256 = sha256_hex(
                format!("cindx.prompt-profile-unplanned-snapshot.v1\0withdrawn\0{key}").as_bytes(),
            );
            withdraw_unplanned_deployment_in_transaction(
                store,
                &key,
                &record,
                model.revision,
                &canonical_snapshot_sha256,
            )?;
        }
    }
    for (key, binding) in stored_bindings {
        if desired_keys.contains(&key) || stored_deployment_keys.contains(&key) {
            continue;
        }
        let canonical_snapshot_sha256 = sha256_hex(
            format!("cindx.prompt-profile-unplanned-snapshot.v1\0withdrawn\0{key}").as_bytes(),
        );
        withdraw_binding_only_deployment_in_transaction(
            store,
            &key,
            &binding,
            model.revision,
            &canonical_snapshot_sha256,
        )?;
    }
    Ok(active)
}
