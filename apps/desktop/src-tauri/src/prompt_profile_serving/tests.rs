use crate::prompt_evolution_hot_state::prompt_rollout_key;
use crate::prompt_profile_serving::{
    delete_prompt_profile_deployments_for_scope, frozen_prompt_profile_selection,
    prompt_profile_deployment_key, publish_canonical_prompt_profile_deployment,
    publish_prompt_profile_deployment, recover_prompt_profile_deployments,
    restore_prompt_profile_selection, seed_prompt_profile_selection, select_prompt_profile_for_run,
    PromptProfileAssignmentSource, PromptProfileFallback, PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
};
use crate::view_models::{PromptEvolutionReadModel, PromptGenomeRecord, PromptRolloutState};
use agent_core::{Metadata, AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_storage::SqliteStore;
use orchestrator::{sha256_hex, ConductorPromptGenome, FrozenPromptProfileSnapshot};
use std::collections::BTreeMap;
use tempfile::tempdir;

#[test]
fn prompt_profile_serving_contract_gate() {
    stable_canary_and_retry_assignments_are_bounded();
    scope_isolation_tamper_and_operation_counts_fail_closed();
    exact_scope_cleanup_removes_auto_and_pro_deployments();
    seed_and_frozen_helpers_do_not_require_storage();
    eprintln!("cindx.prompt-profile-serving-contract.v1");
}

#[test]
fn prompt_profile_recovery_contract_gate() {
    publisher_cas_rejects_stale_sources_and_recovers_scopes();
    super::consistency_tests::run_prompt_profile_serving_consistency_regressions();
    eprintln!("cindx.prompt-profile-recovery-contract.v1");
}

#[test]
fn prompt_profile_selection_scaling_gate() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-scaling";
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let initial_model = model(
        17,
        scope,
        "auto",
        Vec::new(),
        rollout(&seed.id, None, 0, "stable"),
    );
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &initial_model, "auto", scope)
            .unwrap()
    );
    let context = run_context("logical-scaling", "attempt-scaling");
    let baseline = select_prompt_profile_for_run(&store, "auto", scope, true, &context);

    for index in 0..512 {
        store
            .save_read_model(
                PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
                &format!("unrelated-{index}"),
                1,
                "{}",
            )
            .unwrap();
    }
    let loaded = select_prompt_profile_for_run(&store, "auto", scope, true, &context);
    assert_eq!(loaded.genome, baseline.genome);
    assert_eq!(loaded.receipt.operations, baseline.receipt.operations);
    assert_eq!(loaded.receipt.operations.read_model_loads, 1);
    assert_eq!(loaded.receipt.operations.observation_rows_scanned, 0);
    assert_eq!(loaded.receipt.operations.history_rows_scanned, 0);
    assert_eq!(loaded.receipt.operations.writes, 0);
    assert!(loaded.receipt_json().unwrap().len() <= 4 * 1024);
    eprintln!("cindx.prompt-profile-selection-scaling.v1");
}

fn stable_canary_and_retry_assignments_are_bounded() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-a";
    let stable = profile("learned-stable-a", "stable directive");
    let canary = profile("learned-canary-a", "canary directive");
    let initial_model = model(
        7,
        scope,
        "auto",
        vec![stable.clone(), canary.clone()],
        rollout(&stable.id, Some(&canary.id), 50, "canary"),
    );
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &initial_model, "auto", scope)
            .unwrap()
    );

    let canary_id = bucket_identity(true);
    let stable_id = bucket_identity(false);
    let canary_run = run_context(&canary_id, "attempt-canary-a");
    let stable_run = run_context(&stable_id, "attempt-stable-a");
    let canary_selection = select_prompt_profile_for_run(&store, "auto", scope, true, &canary_run);
    let stable_selection = select_prompt_profile_for_run(&store, "auto", scope, true, &stable_run);

    assert_eq!(
        canary_selection.receipt.source,
        PromptProfileAssignmentSource::Canary
    );
    assert_eq!(canary_selection.genome, canary);
    assert_eq!(
        stable_selection.receipt.source,
        PromptProfileAssignmentSource::Stable
    );
    assert_eq!(stable_selection.genome, stable);

    let retry = run_context(&canary_id, "attempt-canary-b");
    let retried = select_prompt_profile_for_run(&store, "auto", scope, true, &retry);
    assert_eq!(retried.genome, canary_selection.genome);
    assert_eq!(retried.receipt.source, canary_selection.receipt.source);
    assert_eq!(
        retried.receipt.profile_sha256,
        canary_selection.receipt.profile_sha256
    );

    let receipt = retried.receipt_json().unwrap();
    assert!(receipt.len() <= 4 * 1024);
    assert!(!receipt.contains(scope));
    assert!(!receipt.contains(&canary_id));
    assert!(!receipt.contains("attempt-canary-b"));
    assert_eq!(retried.receipt.operations.read_model_loads, 1);
    assert_eq!(retried.receipt.operations.observation_rows_scanned, 0);
    assert_eq!(retried.receipt.operations.history_rows_scanned, 0);
    assert_eq!(retried.receipt.operations.writes, 0);
    assert_eq!(retried.receipt.stable_profile_sha256.len(), 64);
    assert_eq!(
        retried
            .receipt
            .canary_profile_sha256
            .as_deref()
            .map(str::len),
        Some(64)
    );
    assert_eq!(retried.source_label(), "canary_50");
    assert_eq!(retried.rollout_status(), Some("canary"));
    assert_eq!(retried.receipt_json_and_sha256().unwrap().1.len(), 64);

    let mut durable_retry = run_context(&canary_id, "attempt-canary-c");
    let (receipt_json, receipt_sha256) = canary_selection.receipt_json_and_sha256().unwrap();
    durable_retry.insert(
        "prompt_profile".to_string(),
        canary_selection.genome.id.clone(),
    );
    durable_retry.insert(
        "prompt_genome".to_string(),
        serde_json::to_string(&canary_selection.genome).unwrap(),
    );
    durable_retry.insert(
        "prompt_profile_source".to_string(),
        canary_selection.source_label(),
    );
    durable_retry.insert(
        "prompt_profile_assignment_receipt".to_string(),
        receipt_json,
    );
    durable_retry.insert(
        "prompt_profile_assignment_sha256".to_string(),
        receipt_sha256,
    );
    durable_retry.insert(
        "prompt_rollout_status".to_string(),
        canary_selection.rollout_status().unwrap().to_string(),
    );
    let rotated = profile("learned-canary-b", "rotated directive");
    let changed = model(
        8,
        scope,
        "auto",
        vec![stable, rotated.clone()],
        rollout("learned-stable-a", Some(&rotated.id), 10, "canary"),
    );
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &changed, "auto", scope).unwrap()
    );
    let restored = restore_prompt_profile_selection("auto", scope, &durable_retry)
        .unwrap()
        .unwrap();
    assert_eq!(restored, canary_selection);
}

fn scope_isolation_tamper_and_operation_counts_fail_closed() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-isolated";
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let model = model(
        11,
        scope,
        "auto",
        Vec::new(),
        rollout(&seed.id, None, 0, "stable"),
    );
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &model, "auto", scope).unwrap()
    );

    let context = run_context("logical-isolation", "attempt-isolation");
    let isolated =
        select_prompt_profile_for_run(&store, "auto", "project-serving-other", true, &context);
    assert_eq!(
        isolated.receipt.fallback,
        Some(PromptProfileFallback::NoDeployment)
    );
    assert_eq!(isolated.receipt.operations.read_model_loads, 1);

    let key = prompt_profile_deployment_key(scope, "auto");
    let stored = store
        .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    let mut payload = serde_json::from_str::<serde_json::Value>(&stored.payload).unwrap();
    payload["deployment"]["stable"]["sha256"] = serde_json::Value::String("0".repeat(64));
    store
        .save_read_model(
            PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
            &key,
            stored.revision,
            &serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();
    let tampered = select_prompt_profile_for_run(&store, "auto", scope, true, &context);
    assert_eq!(
        tampered.receipt.fallback,
        Some(PromptProfileFallback::Invalid)
    );
    assert_eq!(tampered.receipt.operations.read_model_loads, 1);
    assert_eq!(tampered.receipt.operations.profile_validations, 1);
}

fn publisher_cas_rejects_stale_sources_and_recovers_scopes() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-cas";
    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let current = model(
        20,
        scope,
        "auto",
        Vec::new(),
        rollout(&seed.id, None, 0, "stable"),
    );
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &current, "auto", scope).unwrap()
    );
    assert!(publish_prompt_profile_deployment(&mut store, &current, "auto", scope, 20).unwrap());
    assert!(publish_prompt_profile_deployment(&mut store, &current, "auto", scope, 21).is_err());
    assert!(publish_prompt_profile_deployment(&mut store, &current, "auto", scope, 19).is_err());

    let stale = model(
        19,
        scope,
        "auto",
        Vec::new(),
        rollout(&seed.id, None, 0, "stable"),
    );
    assert!(!publish_prompt_profile_deployment(&mut store, &stale, "auto", scope, 19).unwrap());
    let stored = store
        .load_read_model(
            PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
            &prompt_profile_deployment_key(scope, "auto"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(stored.revision, 1);

    let recovery_scope = "project-serving-recovery";
    let mut global = empty_model(30);
    global.rollouts.insert(
        prompt_rollout_key(recovery_scope, "auto"),
        rollout(&seed.id, None, 0, "stable"),
    );
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &global).unwrap(),
        1
    );
    let recovered = select_prompt_profile_for_run(
        &store,
        "auto",
        recovery_scope,
        true,
        &run_context("logical-recovery", "attempt-recovery"),
    );
    assert_eq!(
        recovered.receipt.source,
        PromptProfileAssignmentSource::Stable
    );
}

fn seed_and_frozen_helpers_do_not_require_storage() {
    let context = run_context("logical-seed", "attempt-seed");
    let fast = seed_prompt_profile_selection("fast", PromptProfileFallback::Fast, &context);
    assert_eq!(fast.genome, ConductorPromptGenome::seed_for_effort("fast"));
    assert_eq!(fast.receipt.operations.read_model_loads, 0);
    assert_eq!(fast.receipt.operations.writes, 0);
    assert_eq!(fast.receipt.fallback, Some(PromptProfileFallback::Fast));

    let mut frozen_genome = profile("frozen-serving-profile", "bounded frozen directive");
    frozen_genome.generation = 1;
    frozen_genome.parents = vec!["seed-pro-v1".to_string()];
    let snapshot = FrozenPromptProfileSnapshot::new_gepa(
        "pro",
        frozen_genome.clone(),
        "seed-pro-v1",
        "a".repeat(64),
        "b".repeat(64),
    )
    .unwrap();
    let frozen = frozen_prompt_profile_selection(&snapshot, &context).unwrap();
    assert_eq!(frozen.genome, frozen_genome);
    assert_eq!(frozen.receipt.source, PromptProfileAssignmentSource::Frozen);
    assert_eq!(frozen.receipt.operations.read_model_loads, 0);
    assert!(frozen.receipt_json().unwrap().len() <= 4 * 1024);
}

fn exact_scope_cleanup_removes_auto_and_pro_deployments() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-cleanup";
    for effort in ["auto", "pro"] {
        let seed = ConductorPromptGenome::seed_for_effort(effort);
        let model = model(
            40,
            scope,
            effort,
            Vec::new(),
            rollout(&seed.id, None, 0, "stable"),
        );
        assert!(
            publish_canonical_prompt_profile_deployment(&mut store, &model, effort, scope).unwrap()
        );
    }
    assert_eq!(
        delete_prompt_profile_deployments_for_scope(&mut store, scope).unwrap(),
        2
    );
    for effort in ["auto", "pro"] {
        let fallback = select_prompt_profile_for_run(
            &store,
            effort,
            scope,
            true,
            &run_context("logical-cleanup", "attempt-cleanup"),
        );
        assert_eq!(
            fallback.receipt.fallback,
            Some(PromptProfileFallback::NoDeployment)
        );
    }
    assert_eq!(
        delete_prompt_profile_deployments_for_scope(&mut store, scope).unwrap(),
        0
    );
}

fn test_store() -> (tempfile::TempDir, SqliteStore) {
    let directory = tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("serving.sqlite3")).unwrap();
    (directory, store)
}

fn empty_model(revision: u64) -> PromptEvolutionReadModel {
    PromptEvolutionReadModel {
        schema: "cindx.prompt-evolution-read-model.v1".to_string(),
        projection_version: 1,
        revision,
        event_count: revision,
        genomes: Vec::new(),
        genome_identity_fingerprints: BTreeMap::new(),
        observations: Vec::new(),
        failure_curricula: Vec::new(),
        attempts: BTreeMap::new(),
        cohorts: BTreeMap::new(),
        cohort_sequences: BTreeMap::new(),
        rollouts: BTreeMap::new(),
        datasets: BTreeMap::new(),
    }
}

fn model(
    revision: u64,
    scope: &str,
    effort: &str,
    genomes: Vec<ConductorPromptGenome>,
    rollout: PromptRolloutState,
) -> PromptEvolutionReadModel {
    let mut model = empty_model(revision);
    model.genomes = genomes
        .into_iter()
        .map(|genome| PromptGenomeRecord {
            scope: scope.to_string(),
            effort: effort.to_string(),
            genome,
            evolution_method: None,
        })
        .collect();
    model.rollouts.insert(effort.to_string(), rollout);
    model
}

fn profile(id: &str, directive: &str) -> ConductorPromptGenome {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.id = id.to_string();
    genome.custom_directive = directive.to_string();
    genome
}

fn rollout(
    stable_profile_id: &str,
    canary_profile_id: Option<&str>,
    canary_percent: u8,
    status: &str,
) -> PromptRolloutState {
    PromptRolloutState {
        stable_profile_id: stable_profile_id.to_string(),
        canary_profile_id: canary_profile_id.map(str::to_string),
        canary_percent,
        evidence_checkpoint: 0,
        live_checkpoint: 0,
        stable_live_checkpoint: 0,
        quarantined_profile_ids: Vec::new(),
        distillation_lease: None,
        rollback_count: 0,
        status: status.to_string(),
        last_reason: None,
        promotion_confidence: None,
        frozen_profile: None,
    }
}

fn run_context(logical_run_id: &str, attempt_run_id: &str) -> Metadata {
    Metadata::from([
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            logical_run_id.to_string(),
        ),
        (
            AGENT_RUN_ID_METADATA_KEY.to_string(),
            attempt_run_id.to_string(),
        ),
    ])
}

fn bucket_identity(canary: bool) -> String {
    (0..10_000)
        .map(|index| format!("logical-bucket-{index}"))
        .find(|identity| {
            let digest = sha256_hex(format!("cindx.prompt-profile-run.v1\0{identity}").as_bytes());
            let bucket = u64::from_str_radix(&digest[..16], 16).unwrap() % 100;
            (bucket < 50) == canary
        })
        .unwrap()
}
