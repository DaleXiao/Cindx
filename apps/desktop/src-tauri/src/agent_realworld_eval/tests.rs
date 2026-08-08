use super::setup::{
    create_fresh_evaluation_data_root, persist_memory_seed_fixture, read_seeded_memory_fixture,
    reject_existing_evaluation_path, validate_evaluation_data_root_paths, SetupFailure,
    SetupFailureCode, SetupFailureStage,
};
use super::verification::verify_case;
use super::{
    directory_size, failed_run, selected_replicates, ExecutionCell, FailedRunDetails,
    PermissionPolicy, RealworldCase, RealworldSuite, Treatment, VerificationContract,
};
use crate::persistence_runtime::open_app_store_at;
use agent_core::Metadata;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[cfg(unix)]
#[test]
fn workspace_size_does_not_follow_external_symlinks() {
    use std::os::unix::fs::symlink;

    let external = tempfile::tempdir().expect("external tempdir");
    fs::write(external.path().join("large.bin"), vec![0_u8; 64 * 1024]).expect("external fixture");
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    fs::write(workspace.path().join("local.txt"), b"local").expect("local fixture");
    symlink(external.path(), workspace.path().join("shared-runtime")).expect("runtime symlink");

    assert_eq!(directory_size(workspace.path()), 5);
}

#[test]
fn replicate_selection_is_bounded() {
    std::env::set_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX", "2");
    assert_eq!(selected_replicates(3).expect("selection"), vec![2]);
    std::env::set_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX", "4");
    assert!(selected_replicates(3).is_err());
    std::env::remove_var("CINDX_AGENT_REALWORLD_REPLICATE_INDEX");
}

#[test]
fn frozen_denied_mutation_case_rejects_empty_product_output() {
    let suite_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("benchmarks/agent/realworld-v4.json");
    let suite: RealworldSuite = serde_json::from_slice(
        &fs::read(&suite_path).expect("frozen real-world suite should be readable"),
    )
    .expect("frozen real-world suite should decode");
    let case = suite
        .cases
        .iter()
        .find(|case| case.id == "permission-denied-mutation")
        .expect("frozen denied-mutation case should exist");
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    for fixture in &case.files {
        let path = workspace.path().join(&fixture.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent should exist");
        }
        fs::write(path, &fixture.content).expect("fixture should be written");
    }

    let result = verify_case(case, Treatment::Auto, workspace.path(), "", &[], None, 1);

    assert!(!result.answer_passed, "empty output must not report denial");
    assert!(
        result
            .external_effect_passed
            .expect("product effect result"),
        "the unchanged protected file and observed denial remain safe"
    );
    assert!(!result.quality_passed);
}

#[test]
fn file_backed_memory_seed_is_visible_to_an_independent_eval_reader() {
    let suite = tempfile::tempdir().expect("suite tempdir");
    let production_data_root = tempfile::tempdir().expect("production data tempdir");
    let production_database = production_data_root.path().join("state.sqlite3");
    let evaluation_data_root = suite.path().join(".cindx-eval-data");
    let validated_data_root = validate_evaluation_data_root_paths(
        suite.path(),
        &evaluation_data_root,
        production_data_root.path(),
        &production_database,
    )
    .expect("evaluation data root should validate");
    let evaluation_database = validated_data_root.join("state.sqlite3");
    assert_eq!(
        evaluation_database,
        evaluation_data_root.join("state.sqlite3")
    );
    assert_ne!(evaluation_database, production_database);
    create_fresh_evaluation_data_root(&evaluation_data_root, &evaluation_database)
        .expect("evaluation data root should be created fresh");

    let project_id = "project-realworld-memory-seed";
    let session_id = "session-realworld-memory-seed";
    let run_context: Metadata = [
        ("project_id".to_string(), project_id.to_string()),
        ("session_id".to_string(), session_id.to_string()),
        ("agent_run_id".to_string(), "run-memory-seed".to_string()),
        ("steer_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect();
    let mut writer = open_app_store_at(&evaluation_database).expect("file-backed writer");
    persist_memory_seed_fixture(
        &mut writer,
        &run_context,
        "Long-term project requirement: demo materials must label the retired launch ring as Red Juniper and list Niko Vale as the demo contact.",
    )
    .expect("memory fixture should persist");
    drop(writer);

    let readback = read_seeded_memory_fixture(&evaluation_database, project_id, session_id)
        .expect("independent read-only connection should see the fixture");
    assert!(readback.record_count > 0);
    assert!(readback.user_requirement.contains("Red Juniper"));
    assert!(readback.user_requirement.contains("Niko Vale"));
    assert_eq!(readback.projection_sha256.len(), 64);
    assert!(!production_database.exists());
}

#[cfg(unix)]
#[test]
fn evaluation_paths_reject_preexisting_root_and_dangling_database_symlink() {
    use std::os::unix::fs::symlink;

    let suite = tempfile::tempdir().expect("suite tempdir");
    let occupied_root = suite.path().join("occupied-eval-data");
    fs::create_dir(&occupied_root).expect("occupied root fixture");
    let occupied_database = occupied_root.join("state.sqlite3");
    let root_error = create_fresh_evaluation_data_root(&occupied_root, &occupied_database)
        .expect_err("preexisting evaluation root must be rejected");
    assert!(root_error.contains("evaluation data root must not already exist"));

    let symlink_root = suite.path().join("symlink-eval-data");
    fs::create_dir(&symlink_root).expect("symlink root fixture");
    let dangling_database = symlink_root.join("state.sqlite3");
    symlink(
        suite.path().join("missing-production-state.sqlite3"),
        &dangling_database,
    )
    .expect("dangling database symlink fixture");
    let database_error =
        reject_existing_evaluation_path(&dangling_database, "evaluation state database")
            .expect_err("dangling database symlink must be rejected");
    assert!(database_error.contains("evaluation state database must not already exist"));
}

#[test]
fn setup_failure_is_typed_and_preserves_spent_setup_latency() {
    let case = RealworldCase {
        id: "rag-memory".to_string(),
        category: "rag_memory".to_string(),
        objective: "recall the seeded requirement".to_string(),
        campaign_split: None,
        seed_memory_prompt: None,
        index_workspace: false,
        files: Vec::new(),
        permission_policy: PermissionPolicy::AllowOnce,
        verification: VerificationContract::default(),
        memory_effect: None,
    };
    let run = failed_run(
        &case,
        Treatment::Auto,
        1,
        &ExecutionCell {
            execution_index: 1,
            treatment_position: 1,
            plan_sha256: "a".repeat(64),
        },
        FailedRunDetails {
            input_sha256: "a".repeat(64),
            error: "temporary store lock".to_string(),
            started: Instant::now(),
            setup_failure: SetupFailure::new(
                SetupFailureStage::MemorySeed,
                SetupFailureCode::Transient,
                true,
            ),
            setup_latency_ms: 37,
        },
    );
    let json = serde_json::to_value(&run).expect("raw run should serialize");

    assert_eq!(run.metrics.setup_latency_ms, 37);
    assert_eq!(
        json["setup_failure"],
        serde_json::json!({
            "stage": "memory_seed",
            "code": "transient",
            "retryable": true
        })
    );
}
