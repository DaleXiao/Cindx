use super::{
    delete_prompt_profile_deployments_for_scope, prompt_profile_deployment_key,
    publish_canonical_prompt_profile_deployment, publish_prompt_profile_deployment,
    recover_prompt_profile_deployments, select_prompt_profile_for_run,
    PromptProfileAssignmentSource, PromptProfileFallback,
    PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
};
use crate::{
    event_persistence::append_event,
    prompt_evolution_hot_state::prompt_rollout_key,
    runtime_constants::PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1,
    runtime_constants::{
        PROMPT_EVOLUTION_READ_MODEL_KEY, PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
        PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION,
    },
    runtime_values::phase16_task_id,
    view_models::{
        PromptDistillationCanaryLeaseV1, PromptEvolutionReadModel, PromptGenomeRecord,
        PromptRolloutState,
    },
};
use agent_core::{EventKind, Metadata, LOGICAL_AGENT_RUN_ID_METADATA_KEY};
use agent_storage::SqliteStore;
use orchestrator::{
    prompt_genome_sha256, sha256_hex, ConductorPromptGenome, PromptEvolutionMethod,
};
use std::collections::BTreeMap;
use tempfile::tempdir;

pub(super) fn run_prompt_profile_serving_consistency_regressions() {
    lower_canonical_revision_advances_serving_generation();
    rollout_append_reloads_the_canonical_projection_before_publication();
    unrelated_canonical_events_preserve_serving_lineage();
    same_content_revision_rollback_rebinds_the_lineage();
    recovery_tombstones_a_binding_only_orphan();
    reused_revision_rejects_a_publisher_from_the_old_history();
    deleted_scope_rejects_a_stale_cross_connection_publisher();
    recovery_withdraws_absent_and_conflicted_deployments_without_aborting();
    recovery_repairs_a_same_revision_corrupt_payload();
    distilled_canary_requires_and_serves_its_exact_lease();
    eprintln!("cindx.prompt-profile-serving-consistency.v1");
}

fn rollout_append_reloads_the_canonical_projection_before_publication() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-production-publication";
    let context = Metadata::from([("project_id".to_string(), scope.to_string())]);
    crate::prompt_evolution_runtime::reconcile_prompt_evolution_for_background(
        &mut store, "auto", scope, &context,
    )
    .unwrap();

    let revision = store.event_revision(&phase16_task_id()).unwrap();
    assert!(revision.latest_sequence > 0);
    let canonical = store
        .load_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
        )
        .unwrap()
        .unwrap();
    assert_eq!(canonical.revision, revision.latest_sequence);
    let selected = select(&store, scope, "production-publication-run");
    assert_eq!(
        selected.receipt.source,
        PromptProfileAssignmentSource::Stable
    );
    assert_eq!(
        selected.receipt.source_revision,
        Some(revision.latest_sequence)
    );
}

fn unrelated_canonical_events_preserve_serving_lineage() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-lineage-stability";
    append_history(&mut store, "lineage-initial", 1);
    let profile = profile("lineage-stable-profile", "stable across unrelated events");
    let mut initial = scoped_model(
        1,
        scope,
        vec![profile.clone()],
        rollout(&profile.id, None, 0),
    );
    persist_canonical_model(&mut store, &mut initial);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &initial, "auto", scope).unwrap()
    );
    let key = prompt_profile_deployment_key(scope, "auto");
    let binding_before = store
        .load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    let deployment_before = store
        .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
        .unwrap()
        .unwrap();

    append_history(&mut store, "lineage-unrelated", 1);
    let mut after_unrelated = scoped_model(
        2,
        scope,
        vec![profile.clone()],
        rollout(&profile.id, None, 0),
    );
    persist_canonical_model(&mut store, &mut after_unrelated);
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &after_unrelated).unwrap(),
        1
    );
    assert_eq!(
        store
            .load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)
            .unwrap()
            .unwrap(),
        binding_before
    );
    assert_eq!(
        store
            .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
            .unwrap()
            .unwrap(),
        deployment_before
    );
}

fn same_content_revision_rollback_rebinds_the_lineage() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-same-content-rollback";
    let profile = profile("same-content-profile", "same content");
    append_history(&mut store, "same-content-old", 100);
    let mut old = scoped_model(
        100,
        scope,
        vec![profile.clone()],
        rollout(&profile.id, None, 0),
    );
    persist_canonical_model(&mut store, &mut old);
    assert!(publish_canonical_prompt_profile_deployment(&mut store, &old, "auto", scope,).unwrap());

    store
        .with_immediate_transaction(|transaction| {
            transaction.delete_records_by_metadata_in_transaction("history", "same-content-old")?;
            transaction.delete_read_model(
                PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
                PROMPT_EVOLUTION_READ_MODEL_KEY,
            )
        })
        .unwrap();
    append_history(&mut store, "same-content-rebound", 40);
    let mut rebound = scoped_model(
        40,
        scope,
        vec![profile.clone()],
        rollout(&profile.id, None, 0),
    );
    persist_canonical_model(&mut store, &mut rebound);
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &rebound).unwrap(),
        1
    );
    let selected = select(&store, scope, "same-content-rollback-run");
    assert_eq!(selected.receipt.source_revision, Some(40));
    assert_eq!(selected.receipt.deployment_generation, Some(2));
    assert_eq!(
        store
            .load_read_model(
                PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE,
                &prompt_profile_deployment_key(scope, "auto"),
            )
            .unwrap()
            .unwrap()
            .revision,
        2
    );
}

fn recovery_tombstones_a_binding_only_orphan() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-binding-only-orphan";
    let profile = profile("binding-only-profile", "orphaned deployment");
    append_history(&mut store, "binding-only-initial", 1);
    let mut initial = scoped_model(
        1,
        scope,
        vec![profile.clone()],
        rollout(&profile.id, None, 0),
    );
    persist_canonical_model(&mut store, &mut initial);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &initial, "auto", scope,).unwrap()
    );
    let key = prompt_profile_deployment_key(scope, "auto");
    store
        .delete_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
        .unwrap();

    append_history(&mut store, "binding-only-withdrawn", 1);
    let mut withdrawn = empty_model(2);
    persist_canonical_model(&mut store, &mut withdrawn);
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &withdrawn).unwrap(),
        0
    );
    assert_eq!(
        store
            .load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        store
            .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        select(&store, scope, "binding-only-withdrawn-run")
            .receipt
            .fallback,
        Some(PromptProfileFallback::NoDeployment)
    );

    append_history(&mut store, "binding-only-returned", 1);
    let mut returned = scoped_model(
        3,
        scope,
        vec![profile.clone()],
        rollout(&profile.id, None, 0),
    );
    persist_canonical_model(&mut store, &mut returned);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &returned, "auto", scope,).unwrap()
    );
    assert_eq!(
        select(&store, scope, "binding-only-returned-run")
            .receipt
            .deployment_generation,
        Some(3)
    );
}

fn lower_canonical_revision_advances_serving_generation() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-revision-regression";
    let removed_scope = "project-serving-revision-removed";
    for project_id in [scope, removed_scope, removed_scope] {
        append_event(
            &mut store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "serving generation fixture",
            Metadata::from([("project_id".to_string(), project_id.to_string())]),
        )
        .unwrap();
    }
    let first = profile("revision-profile-a", "first");
    let mut initial = scoped_model(3, scope, vec![first.clone()], rollout(&first.id, None, 0));
    persist_canonical_model(&mut store, &mut initial);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &initial, "auto", scope).unwrap()
    );
    store
        .with_immediate_transaction(|transaction| {
            transaction
                .delete_records_by_metadata_in_transaction("project_id", removed_scope)
                .map(|_| ())
        })
        .unwrap();

    let second = profile("revision-profile-b", "second");
    let mut current = scoped_model(1, scope, vec![second.clone()], rollout(&second.id, None, 0));
    persist_canonical_model(&mut store, &mut current);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &current, "auto", scope).unwrap()
    );
    let selected = select(&store, scope, "revision-regression-run");
    assert_eq!(selected.genome, second);
    assert_eq!(selected.receipt.source_revision, Some(1));
    assert_eq!(selected.receipt.deployment_generation, Some(2));
}

fn reused_revision_rejects_a_publisher_from_the_old_history() {
    let directory = tempdir().unwrap();
    let path = directory
        .path()
        .join("same-revision-cross-connection.sqlite3");
    let mut stale_publisher = SqliteStore::open(&path).unwrap();
    let mut authority = SqliteStore::open(&path).unwrap();
    let scope = "project-serving-reused-revision";

    append_history(&mut authority, "old", 40);
    let old_profile = profile("reused-revision-old", "old revision 40");
    let mut old_snapshot = scoped_model(
        40,
        scope,
        vec![old_profile.clone()],
        rollout(&old_profile.id, None, 0),
    );
    persist_canonical_model(&mut authority, &mut old_snapshot);
    assert!(publish_canonical_prompt_profile_deployment(
        &mut stale_publisher,
        &old_snapshot,
        "auto",
        scope,
    )
    .unwrap());

    append_history(&mut authority, "newer", 60);
    let newer_profile = profile("reused-revision-newer", "revision 100");
    let mut newer_snapshot = scoped_model(
        100,
        scope,
        vec![newer_profile.clone()],
        rollout(&newer_profile.id, None, 0),
    );
    persist_canonical_model(&mut authority, &mut newer_snapshot);
    assert_eq!(
        recover_prompt_profile_deployments(&mut authority, &newer_snapshot).unwrap(),
        1
    );

    authority
        .with_immediate_transaction(|transaction| {
            transaction.delete_records_by_metadata_in_transaction("history", "old")?;
            transaction.delete_records_by_metadata_in_transaction("history", "newer")?;
            transaction.delete_read_model(
                PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
                PROMPT_EVOLUTION_READ_MODEL_KEY,
            )
        })
        .unwrap();
    append_history(&mut authority, "rebound", 40);
    let rebound_profile = profile("reused-revision-rebound", "different revision 40");
    let mut rebound_snapshot = scoped_model(
        40,
        scope,
        vec![rebound_profile.clone()],
        rollout(&rebound_profile.id, None, 0),
    );
    persist_canonical_model(&mut authority, &mut rebound_snapshot);
    assert_eq!(
        recover_prompt_profile_deployments(&mut authority, &rebound_snapshot).unwrap(),
        1
    );

    let key = prompt_profile_deployment_key(scope, "auto");
    let binding_before_stale = stale_publisher
        .load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    let deployment_before_stale = stale_publisher
        .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    assert!(!publish_prompt_profile_deployment(
        &mut stale_publisher,
        &old_snapshot,
        "auto",
        scope,
        40,
    )
    .unwrap());
    let selected = select(&stale_publisher, scope, "reused-revision-run");
    assert_eq!(selected.genome, rebound_profile);
    assert_eq!(selected.receipt.source_revision, Some(40));
    assert_eq!(selected.receipt.deployment_generation, Some(3));
    let binding_after_stale = stale_publisher
        .load_read_model(PROMPT_PROFILE_CANONICAL_BINDING_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    let deployment_after_stale = stale_publisher
        .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    assert_eq!(binding_before_stale, binding_after_stale);
    assert_eq!(deployment_before_stale, deployment_after_stale);
    assert_eq!(binding_after_stale.revision, 3);
}

fn deleted_scope_rejects_a_stale_cross_connection_publisher() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("cross-connection.sqlite3");
    let mut publisher = SqliteStore::open(&path).unwrap();
    let mut lifecycle = SqliteStore::open(&path).unwrap();
    let scope = "project-serving-deleted-fence";
    let original = profile("deleted-profile-a", "before delete");
    let stale_snapshot = scoped_model(
        10,
        scope,
        vec![original.clone()],
        rollout(&original.id, None, 0),
    );
    assert!(publish_canonical_prompt_profile_deployment(
        &mut publisher,
        &stale_snapshot,
        "auto",
        scope
    )
    .unwrap());
    assert_eq!(
        delete_prompt_profile_deployments_for_scope(&mut lifecycle, scope).unwrap(),
        1
    );
    let stale = profile("deleted-profile-b", "stale publisher");
    let stale_snapshot = scoped_model(11, scope, vec![stale.clone()], rollout(&stale.id, None, 0));
    assert!(
        !publish_prompt_profile_deployment(&mut publisher, &stale_snapshot, "auto", scope, 11,)
            .unwrap()
    );
    assert_eq!(
        select(&publisher, scope, "deleted-fence-run")
            .receipt
            .fallback,
        Some(PromptProfileFallback::NoDeployment)
    );
}

fn recovery_withdraws_absent_and_conflicted_deployments_without_aborting() {
    let (_directory, mut store) = test_store();
    let absent_scope = "project-serving-absent";
    let conflicted_scope = "project-serving-conflicted";
    let deleted_scope = "project-serving-recovery-deleted";
    let healthy_scope = "project-serving-recovery-healthy";
    for (scope, profile_id) in [
        (absent_scope, "absent-profile"),
        (conflicted_scope, "conflicted-profile"),
        (deleted_scope, "deleted-recovery-profile"),
    ] {
        let profile = profile(profile_id, "initial");
        let initial = scoped_model(
            15,
            scope,
            vec![profile.clone()],
            rollout(&profile.id, None, 0),
        );
        assert!(
            publish_canonical_prompt_profile_deployment(&mut store, &initial, "auto", scope)
                .unwrap()
        );
    }
    assert_eq!(
        delete_prompt_profile_deployments_for_scope(&mut store, deleted_scope).unwrap(),
        1
    );

    let seed = ConductorPromptGenome::seed_for_effort("auto");
    let mut canonical = empty_model(20);
    canonical.genome_identity_fingerprints.insert(
        serde_json::to_string(&(
            conflicted_scope.to_string(),
            "auto".to_string(),
            "conflicted-profile".to_string(),
        ))
        .unwrap(),
        "conflict".to_string(),
    );
    canonical.rollouts.insert(
        prompt_rollout_key(conflicted_scope, "auto"),
        rollout("conflicted-profile", None, 0),
    );
    canonical.rollouts.insert(
        prompt_rollout_key(deleted_scope, "auto"),
        rollout(&seed.id, None, 0),
    );
    canonical.rollouts.insert(
        prompt_rollout_key(healthy_scope, "auto"),
        rollout(&seed.id, None, 0),
    );
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &canonical).unwrap(),
        1
    );

    for scope in [absent_scope, conflicted_scope, deleted_scope] {
        assert_eq!(
            select(&store, scope, "withdrawn-run").receipt.fallback,
            Some(PromptProfileFallback::NoDeployment)
        );
    }
    assert_eq!(
        select(&store, healthy_scope, "healthy-run").receipt.source,
        PromptProfileAssignmentSource::Stable
    );
}

fn recovery_repairs_a_same_revision_corrupt_payload() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-corrupt-recovery";
    let profile = profile("corrupt-recovery-profile", "repair me");
    let mut canonical = empty_model(30);
    canonical.genomes.push(record(scope, profile.clone(), None));
    canonical.rollouts.insert(
        prompt_rollout_key(scope, "auto"),
        rollout(&profile.id, None, 0),
    );
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &canonical).unwrap(),
        1
    );
    let key = prompt_profile_deployment_key(scope, "auto");
    let stored = store
        .load_read_model(PROMPT_PROFILE_DEPLOYMENT_NAMESPACE, &key)
        .unwrap()
        .unwrap();
    store
        .save_read_model(
            PROMPT_PROFILE_DEPLOYMENT_NAMESPACE,
            &key,
            stored.revision,
            "{corrupt",
        )
        .unwrap();
    assert_eq!(
        recover_prompt_profile_deployments(&mut store, &canonical).unwrap(),
        1
    );
    let selected = select(&store, scope, "corrupt-recovery-run");
    assert_eq!(selected.genome, profile);
    assert_eq!(selected.receipt.deployment_generation, Some(2));
}

fn distilled_canary_requires_and_serves_its_exact_lease() {
    let (_directory, mut store) = test_store();
    let scope = "project-serving-distillation";
    let stable = profile("distillation-parent", "stable");
    let candidate = profile("distillation-child", "candidate");
    let mut rollout = rollout(&stable.id, Some(&candidate.id), 50);
    let mut model = scoped_model(
        50,
        scope,
        vec![stable.clone(), candidate.clone()],
        rollout.clone(),
    );
    model.genomes[1].evolution_method = Some(PromptEvolutionMethod::ProToAutoDistillation);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &model, "auto", scope).is_err()
    );

    rollout.distillation_lease = Some(PromptDistillationCanaryLeaseV1 {
        schema: PROMPT_DISTILLATION_CANARY_LEASE_SCHEMA_V1.to_string(),
        candidate_profile_id: candidate.id.clone(),
        candidate_profile_sha256: prompt_genome_sha256(&candidate).unwrap(),
        stable_profile_id: stable.id.clone(),
        stable_profile_sha256: prompt_genome_sha256(&stable).unwrap(),
        cohort_sha256: "a".repeat(64),
        paired_evidence_sha256: "b".repeat(64),
    });
    model.rollouts.insert("auto".to_string(), rollout);
    assert!(
        publish_canonical_prompt_profile_deployment(&mut store, &model, "auto", scope).unwrap()
    );
    let canary_run = (0..10_000)
        .map(|index| format!("distillation-run-{index}"))
        .find(|id| {
            let digest = sha256_hex(format!("cindx.prompt-profile-run.v1\0{id}").as_bytes());
            u64::from_str_radix(&digest[..16], 16).unwrap() % 100 < 50
        })
        .unwrap();
    let selected = select(&store, scope, &canary_run);
    assert!(selected.receipt_json().unwrap().len() <= 4 * 1024);
    let lease = selected.receipt.distillation_lease.unwrap();
    assert_eq!(lease.candidate_profile_id, candidate.id);
    assert_eq!(lease.stable_profile_id, stable.id);
}

fn select(store: &SqliteStore, scope: &str, logical_run_id: &str) -> super::PromptProfileSelection {
    select_prompt_profile_for_run(store, "auto", scope, true, &context(logical_run_id))
}

fn context(logical_run_id: &str) -> Metadata {
    Metadata::from([(
        LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
        logical_run_id.to_string(),
    )])
}

fn test_store() -> (tempfile::TempDir, SqliteStore) {
    let directory = tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("serving-consistency.sqlite3")).unwrap();
    (directory, store)
}

fn append_history(store: &mut SqliteStore, history: &str, count: usize) {
    for _ in 0..count {
        append_event(
            store,
            &phase16_task_id(),
            EventKind::TaskStatusChanged,
            "canonical history fixture",
            Metadata::from([("history".to_string(), history.to_string())]),
        )
        .unwrap();
    }
}

fn persist_canonical_model(store: &mut SqliteStore, model: &mut PromptEvolutionReadModel) {
    model.schema = PROMPT_EVOLUTION_READ_MODEL_NAMESPACE.to_string();
    model.projection_version = PROMPT_EVOLUTION_READ_MODEL_PROJECTION_VERSION;
    store
        .save_read_model(
            PROMPT_EVOLUTION_READ_MODEL_NAMESPACE,
            PROMPT_EVOLUTION_READ_MODEL_KEY,
            model.revision,
            &serde_json::to_string(model).unwrap(),
        )
        .unwrap();
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

fn scoped_model(
    revision: u64,
    scope: &str,
    genomes: Vec<ConductorPromptGenome>,
    rollout: PromptRolloutState,
) -> PromptEvolutionReadModel {
    let mut model = empty_model(revision);
    model.genomes = genomes
        .into_iter()
        .map(|genome| record(scope, genome, None))
        .collect();
    model.rollouts.insert("auto".to_string(), rollout);
    model
}

fn record(
    scope: &str,
    genome: ConductorPromptGenome,
    evolution_method: Option<PromptEvolutionMethod>,
) -> PromptGenomeRecord {
    PromptGenomeRecord {
        scope: scope.to_string(),
        effort: "auto".to_string(),
        genome,
        evolution_method,
    }
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
        status: if canary_profile_id.is_some() {
            "canary".to_string()
        } else {
            "stable".to_string()
        },
        last_reason: None,
        promotion_confidence: None,
        frozen_profile: None,
    }
}
