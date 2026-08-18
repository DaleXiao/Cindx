use crate::prompt_profile_serving::{
    delete_prompt_profile_deployments_for_scope_in_transaction, frozen_prompt_profile_selection,
    prompt_profile_deployment_key, seed_prompt_profile_selection, select_prompt_profile_for_run,
    PromptProfileAssignmentSource, PromptProfileFallback, PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
};
use agent_core::{Metadata, AGENT_RUN_ID_METADATA_KEY, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_storage::SqliteStore;
use orchestrator::{prompt_genome_sha256, ConductorPromptGenome, FrozenPromptProfileSnapshot};
use tempfile::tempdir;

#[test]
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

#[test]
fn scope_cleanup_deletes_auto_and_pro_deployments() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-cleanup";
    for effort in ["auto", "pro"] {
        store
            .save_read_model(
                PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
                &prompt_profile_deployment_key(scope, effort),
                1,
                &serde_json::to_string(&deployment_record(scope, effort)).unwrap(),
            )
            .unwrap();
    }
    assert_eq!(
        delete_prompt_profile_deployments_for_scope_in_transaction(&mut store, scope).unwrap(),
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
        delete_prompt_profile_deployments_for_scope_in_transaction(&mut store, scope).unwrap(),
        0
    );
}

fn deployment_record(scope: &str, effort: &str) -> serde_json::Value {
    let genome = ConductorPromptGenome::seed_for_effort(effort);
    let sha256 = prompt_genome_sha256(&genome).unwrap();
    serde_json::json!({
        "schema": super::PROMPT_PROFILE_RECORD_SCHEMA,
        "scope_sha256": super::scope_sha256(scope),
        "effort": effort,
        "generation": 1,
        "state": "active",
        "deployment": {
            "schema": super::PROMPT_PROFILE_DEPLOYMENT_SCHEMA,
            "scope_sha256": super::scope_sha256(scope),
            "effort": effort,
            "stable": {
                "genome": genome,
                "sha256": sha256,
            },
            "rollout_status": "stable",
            "canary_percent": 0,
            "source_revision": 1,
        },
    })
}

fn test_store() -> (tempfile::TempDir, SqliteStore) {
    let directory = tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("serving.sqlite3")).unwrap();
    (directory, store)
}

fn profile(id: &str, directive: &str) -> ConductorPromptGenome {
    let mut genome = ConductorPromptGenome::seed_for_effort("auto");
    genome.id = id.to_string();
    genome.custom_directive = directive.to_string();
    genome
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
