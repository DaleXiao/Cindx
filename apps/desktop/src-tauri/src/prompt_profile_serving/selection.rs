use super::model::{
    new_receipt, validate_profile, PromptProfileAssignmentReceipt, PromptProfileDeploymentRecord,
    PromptProfileDeploymentState, PromptProfileSelection,
};
use super::{
    normalize_effort, prompt_profile_deployment_key, scope_sha256, PromptProfileAssignmentSource,
    PromptProfileDeployment, PromptProfileFallback, PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
};
use agent_core::{Metadata, AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_storage::SqliteStore;
#[cfg(any(test, feature = "realworld-eval"))]
use orchestrator::FrozenPromptProfileSnapshot;
use orchestrator::{prompt_genome_sha256, sha256_hex, ConductorPromptGenome};

pub(crate) fn seed_prompt_profile_selection(
    effort: &str,
    reason: PromptProfileFallback,
    run_context: &Metadata,
) -> PromptProfileSelection {
    let effort = normalize_effort(effort);
    let genome = ConductorPromptGenome::seed_for_effort(effort);
    let digest = prompt_genome_sha256(&genome).expect("built-in prompt seeds must validate");
    let mut receipt = new_receipt(
        effort,
        PromptProfileAssignmentSource::SeedFallback,
        Some(reason),
        genome.id.clone(),
        digest,
    );
    receipt.run_identity_sha256 = run_identity_sha256(run_context);
    PromptProfileSelection { genome, receipt }
}

#[cfg(any(test, feature = "realworld-eval"))]
pub(crate) fn frozen_prompt_profile_selection(
    snapshot: &FrozenPromptProfileSnapshot,
    run_context: &Metadata,
) -> Result<PromptProfileSelection, String> {
    snapshot.validate()?;
    validate_profile(&snapshot.genome)?;
    if !matches!(snapshot.effort.as_str(), "auto" | "pro") {
        return Err("frozen prompt profile effort must be auto or pro".to_string());
    }
    let mut receipt = new_receipt(
        &snapshot.effort,
        PromptProfileAssignmentSource::Frozen,
        None,
        snapshot.genome.id.clone(),
        prompt_genome_sha256(&snapshot.genome)?,
    );
    receipt.run_identity_sha256 = run_identity_sha256(run_context);
    receipt.rollout_status = Some("frozen".to_string());
    let selected = PromptProfileSelection {
        genome: snapshot.genome.clone(),
        receipt,
    };
    selected.receipt_json()?;
    Ok(selected)
}

pub(crate) fn select_prompt_profile_for_run(
    store: &SqliteStore,
    effort: &str,
    scope: &str,
    evolution_enabled: bool,
    run_context: &Metadata,
) -> PromptProfileSelection {
    let effort = normalize_effort(effort);
    if effort == "fast" {
        return seed_prompt_profile_selection(effort, PromptProfileFallback::Fast, run_context);
    }
    if !matches!(effort, "auto" | "pro") || !super::valid_scope(scope) {
        return seed_prompt_profile_selection(effort, PromptProfileFallback::Invalid, run_context);
    }
    if !evolution_enabled {
        return seed_prompt_profile_selection(
            effort,
            PromptProfileFallback::EvolutionDisabled,
            run_context,
        );
    }
    select_deployed(store, effort, scope.trim(), run_context)
}

pub(crate) fn restore_prompt_profile_selection(
    effort: &str,
    scope: &str,
    run_context: &Metadata,
) -> Result<Option<PromptProfileSelection>, String> {
    let encoded_receipt = run_context.get("prompt_profile_assignment_receipt");
    let encoded_genome = run_context.get("prompt_genome");
    let assignment_sha256 = run_context.get("prompt_profile_assignment_sha256");
    if encoded_receipt.is_none() && encoded_genome.is_none() && assignment_sha256.is_none() {
        return Ok(None);
    }
    let encoded_receipt = encoded_receipt
        .ok_or_else(|| "prompt profile assignment is missing its receipt".to_string())?;
    let encoded_genome = encoded_genome
        .ok_or_else(|| "prompt profile assignment is missing its genome".to_string())?;
    let assignment_sha256 = assignment_sha256
        .ok_or_else(|| "prompt profile assignment is missing its digest".to_string())?;
    if sha256_hex(encoded_receipt.as_bytes()) != *assignment_sha256 {
        return Err("prompt profile assignment receipt digest mismatch".to_string());
    }
    let receipt = serde_json::from_str::<PromptProfileAssignmentReceipt>(encoded_receipt)
        .map_err(|error| format!("prompt profile assignment receipt is invalid: {error}"))?;
    let genome = serde_json::from_str::<ConductorPromptGenome>(encoded_genome)
        .map_err(|error| format!("prompt profile assignment genome is invalid: {error}"))?;
    validate_profile(&genome)?;
    let effort = normalize_effort(effort);
    if receipt.schema != super::PROMPT_PROFILE_ASSIGNMENT_SCHEMA
        || receipt.effort != effort
        || receipt.profile_id != genome.id
        || receipt.profile_sha256 != prompt_genome_sha256(&genome)?
        || receipt.run_identity_sha256 != run_identity_sha256(run_context)
        || run_context.get("prompt_profile").map(String::as_str) != Some(genome.id.as_str())
    {
        return Err("prompt profile assignment identity is invalid".to_string());
    }
    let expected_scope_sha256 = scope_sha256(scope);
    if receipt
        .scope_sha256
        .as_deref()
        .is_some_and(|digest| digest != expected_scope_sha256.as_str())
        || matches!(
            receipt.source,
            PromptProfileAssignmentSource::Stable | PromptProfileAssignmentSource::Canary
        ) && receipt.scope_sha256.as_deref() != Some(expected_scope_sha256.as_str())
    {
        return Err("prompt profile assignment scope is invalid".to_string());
    }
    let lease_is_valid = receipt.distillation_lease.as_ref().is_none_or(|lease| {
        let selected_matches = match receipt.source {
            PromptProfileAssignmentSource::Stable => {
                receipt.profile_id == lease.stable_profile_id
                    && receipt.profile_sha256 == lease.stable_profile_sha256
            }
            PromptProfileAssignmentSource::Canary => {
                receipt.profile_id == lease.candidate_profile_id
                    && receipt.profile_sha256 == lease.candidate_profile_sha256
            }
            PromptProfileAssignmentSource::Frozen | PromptProfileAssignmentSource::SeedFallback => {
                false
            }
        };
        lease.validate().is_ok()
            && selected_matches
            && lease.stable_profile_sha256 == receipt.stable_profile_sha256
            && receipt.canary_profile_sha256.as_deref()
                == Some(lease.candidate_profile_sha256.as_str())
    });
    let source_is_valid = match receipt.source {
        PromptProfileAssignmentSource::Stable => {
            receipt.fallback.is_none()
                && receipt.profile_sha256 == receipt.stable_profile_sha256
                && receipt.source_revision.is_some()
                && receipt.deployment_generation.is_some()
        }
        PromptProfileAssignmentSource::Canary => {
            receipt.fallback.is_none()
                && receipt.canary_profile_sha256.as_deref() == Some(receipt.profile_sha256.as_str())
                && receipt.source_revision.is_some()
                && receipt.deployment_generation.is_some()
                && receipt.canary_percent > 0
        }
        PromptProfileAssignmentSource::Frozen => {
            receipt.fallback.is_none()
                && receipt.profile_sha256 == receipt.stable_profile_sha256
                && receipt.rollout_status.as_deref() == Some("frozen")
                && receipt.deployment_generation.is_none()
                && receipt.distillation_lease.is_none()
        }
        PromptProfileAssignmentSource::SeedFallback => {
            receipt.fallback.is_some()
                && receipt.profile_sha256 == receipt.stable_profile_sha256
                && receipt.canary_profile_sha256.is_none()
                && receipt.canary_percent == 0
                && receipt.deployment_generation.is_none()
                && receipt.distillation_lease.is_none()
        }
    };
    if !source_is_valid
        || !lease_is_valid
        || receipt.deployment_generation == Some(0)
        || run_context.get("prompt_rollout_status").map(String::as_str)
            != receipt.rollout_status.as_deref()
    {
        return Err("prompt profile assignment source is invalid".to_string());
    }
    let selection = PromptProfileSelection { genome, receipt };
    let (_, verified_sha256) = selection.receipt_json_and_sha256()?;
    if verified_sha256 != *assignment_sha256 {
        return Err("prompt profile assignment is not canonical".to_string());
    }
    let recorded_source = run_context
        .get("prompt_profile_source")
        .map(String::as_str)
        .ok_or_else(|| "prompt profile assignment is missing its source".to_string())?;
    if recorded_source != selection.source_label()
        && !(selection.receipt.source == PromptProfileAssignmentSource::Frozen
            && recorded_source == "evaluation_frozen_profile")
    {
        return Err("prompt profile assignment source label is invalid".to_string());
    }
    Ok(Some(selection))
}

fn select_deployed(
    store: &SqliteStore,
    effort: &str,
    scope: &str,
    run_context: &Metadata,
) -> PromptProfileSelection {
    let key = prompt_profile_deployment_key(scope, effort);
    let stored = match store.load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key) {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            return fallback_after_read(
                effort,
                scope,
                PromptProfileFallback::NoDeployment,
                run_context,
            )
        }
        Err(_) => {
            return fallback_after_read(effort, scope, PromptProfileFallback::Storage, run_context)
        }
    };
    let record = match serde_json::from_str::<PromptProfileDeploymentRecord>(&stored.payload) {
        Ok(record)
            if record.generation == stored.revision
                && record.validate_for(scope, effort).is_ok() =>
        {
            record
        }
        _ => {
            return fallback_after_read(effort, scope, PromptProfileFallback::Invalid, run_context)
        }
    };
    if record.state != PromptProfileDeploymentState::Active {
        return fallback_after_read(
            effort,
            scope,
            PromptProfileFallback::NoDeployment,
            run_context,
        );
    }
    assignment_from_deployment(
        record.deployment.expect("validated active deployment"),
        record.generation,
        effort,
        scope,
        run_context,
    )
}

fn assignment_from_deployment(
    deployment: PromptProfileDeployment,
    deployment_generation: u64,
    effort: &str,
    scope: &str,
    run_context: &Metadata,
) -> PromptProfileSelection {
    let run_digest = run_identity_sha256(run_context);
    let use_canary = deployment.canary.is_some()
        && deployment.canary_percent > 0
        && run_digest.as_deref().is_some_and(|digest| {
            u64::from_str_radix(&digest[..16], 16).unwrap_or_default() % 100
                < u64::from(deployment.canary_percent)
        });
    let profile_validations = 1 + u8::from(deployment.canary.is_some());
    let stable_sha256 = deployment.stable.sha256.clone();
    let canary_sha256 = deployment
        .canary
        .as_ref()
        .map(|profile| profile.sha256.clone());
    let distillation_lease = deployment.distillation_lease.clone();
    let (profile, source) = if use_canary {
        (
            deployment.canary.expect("validated canary deployment"),
            PromptProfileAssignmentSource::Canary,
        )
    } else {
        (deployment.stable, PromptProfileAssignmentSource::Stable)
    };
    let mut receipt = new_receipt(
        effort,
        source,
        None,
        profile.genome.id.clone(),
        profile.sha256,
    );
    receipt.stable_profile_sha256 = stable_sha256;
    receipt.canary_profile_sha256 = canary_sha256;
    receipt.scope_sha256 = Some(scope_sha256(scope));
    receipt.run_identity_sha256 = run_digest;
    receipt.rollout_status = Some(deployment.rollout_status);
    receipt.canary_percent = deployment.canary_percent;
    receipt.source_revision = Some(deployment.source_revision);
    receipt.deployment_generation = Some(deployment_generation);
    receipt.distillation_lease = distillation_lease;
    receipt.operations.read_model_loads = 1;
    receipt.operations.profile_validations = profile_validations;
    PromptProfileSelection {
        genome: profile.genome,
        receipt,
    }
}

fn fallback_after_read(
    effort: &str,
    scope: &str,
    reason: PromptProfileFallback,
    run_context: &Metadata,
) -> PromptProfileSelection {
    let mut fallback = seed_prompt_profile_selection(effort, reason, run_context);
    fallback.receipt.scope_sha256 = Some(scope_sha256(scope));
    fallback.receipt.operations.read_model_loads = 1;
    fallback
}

fn run_identity_sha256(metadata: &Metadata) -> Option<String> {
    [LOGICAL_AGENT_RUN_ID_METADATA_KEY, AGENT_RUN_ID_METADATA_KEY]
        .into_iter()
        .find_map(|key| {
            metadata
                .get(key)
                .map(String::as_str)
                .filter(|id| !id.trim().is_empty())
        })
        .map(|id| sha256_hex(format!("cindx.prompt-profile-run.v1\0{id}").as_bytes()))
}

/// Selects the prompt profile for a run: restores a durable assignment for the
/// run identity when present, honours the evaluation frozen profile under
/// `realworld-eval`, and otherwise serves the seed profile for the effort tier
/// (deployment lookups stay best-effort and fail closed to the seed).
pub(crate) fn selected_strategy_profile(
    state: &tauri::State<'_, crate::app_state::AppState>,
    config: &crate::configuration_models::ProviderConfig,
    effort: orchestrator::AgentPolicy,
    run_context: &mut Metadata,
) -> Result<(ConductorPromptGenome, String), String> {
    let scope = run_context
        .get("project_id")
        .map(String::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .unwrap_or("global")
        .to_string();
    if let Some(selection) = restore_prompt_profile_selection(effort.label(), &scope, run_context)?
    {
        let source = run_context.get("prompt_profile_source").cloned();
        return install_prompt_profile_selection(run_context, selection, source.as_deref());
    }
    #[cfg(feature = "realworld-eval")]
    if let Some(snapshot) = evaluation_frozen_strategy_profile(effort)? {
        let selection = frozen_prompt_profile_selection(&snapshot, run_context)?;
        return install_prompt_profile_selection(
            run_context,
            selection,
            Some("evaluation_frozen_profile"),
        );
    }
    if !should_evaluate_strategy_profile(effort, config.prompt_evolution_enabled) {
        let reason = if matches!(effort, orchestrator::AgentPolicy::Fast) {
            PromptProfileFallback::Fast
        } else {
            PromptProfileFallback::EvolutionDisabled
        };
        let selection = seed_prompt_profile_selection(effort.label(), reason, run_context);
        return install_prompt_profile_selection(run_context, selection, None);
    }
    let selection = match state.store.lock() {
        Ok(store) => select_prompt_profile_for_run(
            &store,
            effort.label(),
            &scope,
            config.prompt_evolution_enabled,
            run_context,
        ),
        Err(_) => seed_prompt_profile_selection(
            effort.label(),
            PromptProfileFallback::Storage,
            run_context,
        ),
    };
    install_prompt_profile_selection(run_context, selection, None)
}

fn install_prompt_profile_selection(
    run_context: &mut Metadata,
    selection: PromptProfileSelection,
    source_override: Option<&str>,
) -> Result<(ConductorPromptGenome, String), String> {
    let (receipt, receipt_sha256) = selection.receipt_json_and_sha256()?;
    let prompt_profile = selection.genome.id.clone();
    let prompt_genome = serde_json::to_string(&selection.genome)
        .map_err(|error| format!("prompt profile serialization failed: {error}"))?;
    let source = source_override
        .map(str::to_string)
        .unwrap_or_else(|| selection.source_label());
    run_context.insert("prompt_profile".to_string(), prompt_profile);
    run_context.insert("prompt_genome".to_string(), prompt_genome);
    run_context.insert("prompt_profile_source".to_string(), source.clone());
    run_context.insert("prompt_profile_assignment_receipt".to_string(), receipt);
    run_context.insert(
        "prompt_profile_assignment_sha256".to_string(),
        receipt_sha256,
    );
    if let Some(status) = selection.rollout_status() {
        run_context.insert("prompt_rollout_status".to_string(), status.to_string());
    } else {
        run_context.remove("prompt_rollout_status");
    }
    Ok((selection.genome, source))
}

pub(crate) fn should_evaluate_strategy_profile(
    effort: orchestrator::AgentPolicy,
    prompt_evolution_enabled: bool,
) -> bool {
    prompt_evolution_enabled
        && effort.prompt_evolution() != orchestrator::PromptEvolutionStrategy::Disabled
}

#[cfg(feature = "realworld-eval")]
fn evaluation_frozen_strategy_profile(
    effort: orchestrator::AgentPolicy,
) -> Result<Option<FrozenPromptProfileSnapshot>, String> {
    let Some(path) = std::env::var_os("CINDX_AGENT_REALWORLD_PROFILE_PATH") else {
        return Ok(None);
    };
    if !matches!(
        effort,
        orchestrator::AgentPolicy::Auto | orchestrator::AgentPolicy::Pro
    ) {
        return Err("evaluation frozen profiles are valid only for Auto or Pro".to_string());
    }
    let encoded = std::fs::read(&path).map_err(|error| {
        format!(
            "failed to read evaluation frozen profile {}: {error}",
            std::path::Path::new(&path).display()
        )
    })?;
    let snapshot = FrozenPromptProfileSnapshot::from_json_slice(&encoded)?;
    if snapshot.effort != effort.label() {
        return Err(format!(
            "evaluation frozen profile effort {} does not match {}",
            snapshot.effort,
            effort.label()
        ));
    }
    if let Ok(expected) = std::env::var("CINDX_AGENT_REALWORLD_PROFILE_ARTIFACT_SHA256") {
        if expected != snapshot.artifact_sha256()? {
            return Err(
                "evaluation frozen profile artifact digest changed after preflight".to_string(),
            );
        }
    }
    Ok(Some(snapshot))
}
