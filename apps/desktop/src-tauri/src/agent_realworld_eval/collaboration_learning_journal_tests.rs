use super::*;
use agent_application::{
    CollaborationLearningArmOrderV1, CollaborationLearningComparisonHashesV1,
    CollaborationLearningSplitV1,
};

// Canonical output of the portable admission fixture; SHA-256
// 600ff58ba4b396dbd958faffddf55a655a931f1fcab009138c79d34301890292.

const OFFLINE_GENESIS_JSON: &str = r###"{"schema":"cindx.agent-collaboration-learning-offline-genesis.v1","candidate":{"policy":{"schema":"cindx.agent-collaboration-learning-policy.v1","parent_policy_sha256":"2736e9ebe0382d2f2207eaa0553de6bcb94971eb78c846f25b4aec820d2ee7ef","limits":{"owner_final_delivery":true,"owner_exclusive_side_effects":true,"max_specialists":1,"max_distinct_verifiers":1,"serial_execution":true,"workers_read_only":true},"specialist_invocation":"one_read_only_specialist","context_budget_bps":7500,"verification":"plan_required_only","repair":"fail_fast","stop":"derived_from_required_lanes_and_repair","policy_sha256":"be59bb7ebc6517a9215d014ddac51267babdb0298084c2d18c86d887b8cc6cc3"},"changed_axis":"context_budget_bps","holdout_manifest_sha256":"a9050cc4e98d1a716fda6f5bea4ba3ae4e133b5cc0851189becb3efdc121c9b9","config_sha256":"27297b930e5b101cfd35f9da7afb1f07ce6d3b1545cc9e39c52feeaa2de5e5b4","proposer_identity_sha256":"9999999999999999999999999999999999999999999999999999999999999999","ordinal":1,"candidate_sha256":"2da30eee115f2e7375e85df9fb4cc54b68da742cc66a28378f792ac377b78732"},"baseline":{"binding":{"hashes":{"source_commit_sha256":"1111111111111111111111111111111111111111111111111111111111111111","suite_sha256":"2222222222222222222222222222222222222222222222222222222222222222","case_sha256":"3333333333333333333333333333333333333333333333333333333333333333","prestate_sha256":"4444444444444444444444444444444444444444444444444444444444444444","provider_sha256":"5555555555555555555555555555555555555555555555555555555555555555","model_pool_sha256":"6666666666666666666666666666666666666666666666666666666666666666","route_profile_sha256":"7777777777777777777777777777777777777777777777777777777777777777","prompt_profile_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","conductor_candidate_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","workflow_proposal_sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","shared_conductor_anchor_sha256":"8888888888888888888888888888888888888888888888888888888888888888","direct_execution_plan_semantic_sha256":"0000000000000000000000000000000000000000000000000000000000000000","workflow_execution_plan_semantic_sha256":"9999999999999999999999999999999999999999999999999999999999999999","budget_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cohort_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"split":"train","replicate":1,"arm_order":"direct_first","binding_sha256":"64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330"},"direct":{"binding":{"hashes":{"source_commit_sha256":"1111111111111111111111111111111111111111111111111111111111111111","suite_sha256":"2222222222222222222222222222222222222222222222222222222222222222","case_sha256":"3333333333333333333333333333333333333333333333333333333333333333","prestate_sha256":"4444444444444444444444444444444444444444444444444444444444444444","provider_sha256":"5555555555555555555555555555555555555555555555555555555555555555","model_pool_sha256":"6666666666666666666666666666666666666666666666666666666666666666","route_profile_sha256":"7777777777777777777777777777777777777777777777777777777777777777","prompt_profile_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","conductor_candidate_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","workflow_proposal_sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","shared_conductor_anchor_sha256":"8888888888888888888888888888888888888888888888888888888888888888","direct_execution_plan_semantic_sha256":"0000000000000000000000000000000000000000000000000000000000000000","workflow_execution_plan_semantic_sha256":"9999999999999999999999999999999999999999999999999999999999999999","budget_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cohort_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"split":"train","replicate":1,"arm_order":"direct_first","binding_sha256":"64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330"},"arm":"direct","outcome":{"schema":"cindx.agent.externally-verified-outcome.v1","lifecycle":{"agent_run_id":"fixture-Direct-64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330","steer_epoch":0,"strategy_receipt_key":"e7cc08b1664202f6bbdcb0db05e223585a0dc78f370c81ad67b7fa753b670c8d","strategy_plan_sha256":"0000000000000000000000000000000000000000000000000000000000000000","execution_plan_semantic_sha256":"0000000000000000000000000000000000000000000000000000000000000000","terminal_commit_key":"27b72617037515b326fa347ae8885366547f844fdee0f4b95e68a7f308ee477f","terminal_status":"completed","decision_sequence":1,"terminal_sequence":10},"exposure":{"logical_model_calls":4,"worker_model_calls":0,"successful_owner_model_calls":4,"successful_specialist_model_calls":0,"successful_independent_verifier_model_calls":0,"successful_conductor_model_calls":0,"successful_workflow_specialist_model_calls":0,"successful_workflow_verifier_model_calls":0,"worker_models":[],"successful_workflow_specialist_models":[],"successful_workflow_verifier_models":[],"direct_anchor_competition_calls":0,"non_owner_permission_gated_calls":0,"workflow_planned":false,"workflow_completed":false},"verifier":{"kind":"external","protocol_sha256":"3333333333333333333333333333333333333333333333333333333333333333","subject_sha256":"3333333333333333333333333333333333333333333333333333333333333333","safety_violations":0},"postconditions":[{"kind":"behavior","subject_sha256":"3333333333333333333333333333333333333333333333333333333333333333","expected_sha256":"4444444444444444444444444444444444444444444444444444444444444444","observed_sha256":"5555555555555555555555555555555555555555555555555555555555555555","artifact_sha256":null,"bytes":null,"passed":false,"preservation":false}],"resources":{"budget_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","model_receipts_sha256":"6666666666666666666666666666666666666666666666666666666666666666","tool_receipts_sha256":"7777777777777777777777777777777777777777777777777777777777777777","elapsed_ms":10,"logical_model_calls":4,"tool_calls":0,"terminal":{"segment":{"physical_model_attempts":4,"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"reserved_tokens":0,"provider_usage_attempts":4,"partial_usage_attempts":0,"estimated_usage_attempts":0,"unknown_usage_attempts":0},"lineage":{"physical_model_attempts":4,"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"reserved_tokens":0,"provider_usage_attempts":4,"partial_usage_attempts":0,"estimated_usage_attempts":0,"unknown_usage_attempts":0}}},"receipt_sha256":"3a3e5d992a4da690dc27458dba7a0ea9b2c67384ad8b317a8df9302b4f5549ba"},"exercise":{"schema":"cindx.agent-collaboration-learning-exercise.v1","outcome_receipt_sha256":"3a3e5d992a4da690dc27458dba7a0ea9b2c67384ad8b317a8df9302b4f5549ba","assignment":{"schema":"cindx.agent-collaboration-learning-assignment.v1","agent_run_id":"fixture-Direct-64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330","steer_epoch":0,"execution_plan_semantic_sha256":"0000000000000000000000000000000000000000000000000000000000000000","policy_json":"{\"schema\":\"cindx.agent-collaboration-learning-policy.v1\",\"parent_policy_sha256\":null,\"limits\":{\"owner_final_delivery\":true,\"owner_exclusive_side_effects\":true,\"max_specialists\":1,\"max_distinct_verifiers\":1,\"serial_execution\":true,\"workers_read_only\":true},\"specialist_invocation\":\"direct_owner_only\",\"context_budget_bps\":0,\"verification\":\"plan_required_only\",\"repair\":\"fail_fast\",\"stop\":\"derived_from_required_lanes_and_repair\",\"policy_sha256\":\"db022e9afad97a5f4452e8a0a9daa406b9d39ef3f6faa8184d64044161c859af\"}","policy_sha256":"db022e9afad97a5f4452e8a0a9daa406b9d39ef3f6faa8184d64044161c859af","case_binding_sha256":"3333333333333333333333333333333333333333333333333333333333333333","assignment_sequence":2,"plan_required_independent_verifier":false,"collaboration_id":null,"specialist":null,"verifier":null},"lanes":[],"stop_reason":"direct_owner_terminal","receipt_sha256":"0b1d802e48d09845fc2bfef170aa896efc3ab507a9421e2a3866d14ddc7a903c"}},"workflow":{"binding":{"hashes":{"source_commit_sha256":"1111111111111111111111111111111111111111111111111111111111111111","suite_sha256":"2222222222222222222222222222222222222222222222222222222222222222","case_sha256":"3333333333333333333333333333333333333333333333333333333333333333","prestate_sha256":"4444444444444444444444444444444444444444444444444444444444444444","provider_sha256":"5555555555555555555555555555555555555555555555555555555555555555","model_pool_sha256":"6666666666666666666666666666666666666666666666666666666666666666","route_profile_sha256":"7777777777777777777777777777777777777777777777777777777777777777","prompt_profile_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","conductor_candidate_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","workflow_proposal_sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","shared_conductor_anchor_sha256":"8888888888888888888888888888888888888888888888888888888888888888","direct_execution_plan_semantic_sha256":"0000000000000000000000000000000000000000000000000000000000000000","workflow_execution_plan_semantic_sha256":"9999999999999999999999999999999999999999999999999999999999999999","budget_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cohort_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"split":"train","replicate":1,"arm_order":"direct_first","binding_sha256":"64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330"},"arm":"workflow","outcome":{"schema":"cindx.agent.externally-verified-outcome.v1","lifecycle":{"agent_run_id":"fixture-Workflow-64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330","steer_epoch":0,"strategy_receipt_key":"98023d7b9bf3627eb31625a3f290aa41e3273a9734ae1ef979d3bb52a5909a50","strategy_plan_sha256":"9999999999999999999999999999999999999999999999999999999999999999","execution_plan_semantic_sha256":"9999999999999999999999999999999999999999999999999999999999999999","terminal_commit_key":"0cd8435622a64b9ce6cb1e4920f10796375a2f37ad2bf62ee75f117e4b7ca580","terminal_status":"completed","decision_sequence":1,"terminal_sequence":10},"exposure":{"logical_model_calls":5,"worker_model_calls":1,"successful_owner_model_calls":4,"successful_specialist_model_calls":1,"successful_independent_verifier_model_calls":0,"successful_conductor_model_calls":0,"successful_workflow_specialist_model_calls":1,"successful_workflow_verifier_model_calls":0,"worker_models":["specialist-model"],"successful_workflow_specialist_models":["specialist-model"],"successful_workflow_verifier_models":[],"direct_anchor_competition_calls":0,"non_owner_permission_gated_calls":0,"workflow_planned":true,"workflow_completed":true},"verifier":{"kind":"external","protocol_sha256":"3333333333333333333333333333333333333333333333333333333333333333","subject_sha256":"3333333333333333333333333333333333333333333333333333333333333333","safety_violations":0},"postconditions":[{"kind":"behavior","subject_sha256":"3333333333333333333333333333333333333333333333333333333333333333","expected_sha256":"4444444444444444444444444444444444444444444444444444444444444444","observed_sha256":"5555555555555555555555555555555555555555555555555555555555555555","artifact_sha256":null,"bytes":null,"passed":true,"preservation":false}],"resources":{"budget_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","model_receipts_sha256":"6666666666666666666666666666666666666666666666666666666666666666","tool_receipts_sha256":"7777777777777777777777777777777777777777777777777777777777777777","elapsed_ms":10,"logical_model_calls":5,"tool_calls":0,"terminal":{"segment":{"physical_model_attempts":5,"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"reserved_tokens":0,"provider_usage_attempts":5,"partial_usage_attempts":0,"estimated_usage_attempts":0,"unknown_usage_attempts":0},"lineage":{"physical_model_attempts":5,"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"reserved_tokens":0,"provider_usage_attempts":5,"partial_usage_attempts":0,"estimated_usage_attempts":0,"unknown_usage_attempts":0}}},"receipt_sha256":"cf635cdb3ed55eb13de6c25a4602a66509de4dca9da856498e9167f111f6a0f8"},"exercise":{"schema":"cindx.agent-collaboration-learning-exercise.v1","outcome_receipt_sha256":"cf635cdb3ed55eb13de6c25a4602a66509de4dca9da856498e9167f111f6a0f8","assignment":{"schema":"cindx.agent-collaboration-learning-assignment.v1","agent_run_id":"fixture-Workflow-64749bbf624843749f439e07de89bac17b04a0d253a0d4544831f0f7dc986330","steer_epoch":0,"execution_plan_semantic_sha256":"9999999999999999999999999999999999999999999999999999999999999999","policy_json":"{\"schema\":\"cindx.agent-collaboration-learning-policy.v1\",\"parent_policy_sha256\":null,\"limits\":{\"owner_final_delivery\":true,\"owner_exclusive_side_effects\":true,\"max_specialists\":1,\"max_distinct_verifiers\":1,\"serial_execution\":true,\"workers_read_only\":true},\"specialist_invocation\":\"one_read_only_specialist\",\"context_budget_bps\":5000,\"verification\":\"plan_required_only\",\"repair\":\"fail_fast\",\"stop\":\"derived_from_required_lanes_and_repair\",\"policy_sha256\":\"2736e9ebe0382d2f2207eaa0553de6bcb94971eb78c846f25b4aec820d2ee7ef\"}","policy_sha256":"2736e9ebe0382d2f2207eaa0553de6bcb94971eb78c846f25b4aec820d2ee7ef","case_binding_sha256":"3333333333333333333333333333333333333333333333333333333333333333","assignment_sequence":2,"plan_required_independent_verifier":false,"collaboration_id":"fixture-collaboration","specialist":{"actor":"specialist","workflow_step_id":"specialist","output_kind":"analysis","initial_model":"specialist-model","repair_model":null},"verifier":null},"lanes":[{"actor":"specialist","workflow_step_id":"specialist","output_kind":"analysis","attempts":[{"request_id":"specialist-request","model":"specialist-model","started_sequence":3,"finished_sequence":4,"recovery_attempt":null,"context":{"budget_bps":5000,"payload_sha256":"8888888888888888888888888888888888888888888888888888888888888888","bytes":64},"provider_response_observed":true,"succeeded":true}],"final_success":true}],"stop_reason":"required_lanes_completed","receipt_sha256":"ce263e00819c1550d2f50f7a0eca1da63a9a54f17ed83a7013d6bce0de660549"}},"pair_sha256":"1bfd72ba0252467f8687c5f82b9c777a96f57eb8f38c7c39d9b6dd5601a3ee7b"},"config":{"min_train_pairs":1,"min_holdout_pairs":1,"minimum_uplift_bps":1,"max_resource_regression_bps":2500,"max_position_imbalance":1,"candidate_budget":4,"config_sha256":"27297b930e5b101cfd35f9da7afb1f07ce6d3b1545cc9e39c52feeaa2de5e5b4"},"frozen_holdout":[{"hashes":{"source_commit_sha256":"1111111111111111111111111111111111111111111111111111111111111111","suite_sha256":"2222222222222222222222222222222222222222222222222222222222222222","case_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","prestate_sha256":"4444444444444444444444444444444444444444444444444444444444444444","provider_sha256":"5555555555555555555555555555555555555555555555555555555555555555","model_pool_sha256":"6666666666666666666666666666666666666666666666666666666666666666","route_profile_sha256":"7777777777777777777777777777777777777777777777777777777777777777","prompt_profile_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","conductor_candidate_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","workflow_proposal_sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","shared_conductor_anchor_sha256":"8888888888888888888888888888888888888888888888888888888888888888","direct_execution_plan_semantic_sha256":"0000000000000000000000000000000000000000000000000000000000000000","workflow_execution_plan_semantic_sha256":"9999999999999999999999999999999999999999999999999999999999999999","budget_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cohort_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},"split":"holdout","replicate":2,"arm_order":"workflow_first","binding_sha256":"f2ff36e6b318fd008c8b23c2e0cf0e44fc959050ec1b7c40034fedb437543a30"}],"genesis_sha256":"fd48cd0512270bafe5a664a73bb36492087b861c23818a8299e57d8ee1b7be17"}"###;

fn digest(value: char) -> String {
    value.to_string().repeat(64)
}

fn fixture_genesis() -> CollaborationLearningOfflineGenesisV1 {
    CollaborationLearningOfflineGenesisV1::from_json(OFFLINE_GENESIS_JSON).unwrap()
}

fn fixture_comparison(
    split: CollaborationLearningSplitV1,
    replicate: u16,
    order: CollaborationLearningArmOrderV1,
) -> CollaborationLearningComparisonBindingV1 {
    let index = usize::from((replicate - 1) % 6);
    let case = match split {
        CollaborationLearningSplitV1::Train => ['3', '4', '5', '6', '7', '8'][index],
        CollaborationLearningSplitV1::Holdout => ['a', 'b', 'c', 'd', 'e', 'f'][index],
    };
    CollaborationLearningComparisonBindingV1::freeze(
        CollaborationLearningComparisonHashesV1 {
            source_commit_sha256: digest('1'),
            suite_sha256: digest('2'),
            case_sha256: digest(case),
            prestate_sha256: digest('4'),
            provider_sha256: digest('5'),
            model_pool_sha256: digest('6'),
            route_profile_sha256: digest('7'),
            prompt_profile_sha256: digest('d'),
            conductor_candidate_sha256: digest('e'),
            workflow_proposal_sha256: digest('f'),
            shared_conductor_anchor_sha256: digest('8'),
            direct_execution_plan_semantic_sha256: digest('0'),
            workflow_execution_plan_semantic_sha256: digest('9'),
            budget_sha256: digest('a'),
            cohort_sha256: digest('c'),
        },
        split,
        replicate,
        order,
    )
    .unwrap()
}

fn fixture_censor_entry(
    genesis: &CollaborationLearningOfflineGenesisV1,
    binding: CollaborationLearningComparisonBindingV1,
    sequence: u16,
    previous_entry_sha256: Option<String>,
    source: char,
    physical_agent_run_ids: &[&str],
) -> CollaborationLearningOfflineEntryV1 {
    let physical_run_sha256 = physical_agent_run_ids
        .iter()
        .map(|run_id| collaboration_learning_physical_run_sha256(run_id).unwrap())
        .collect();
    let censor = CollaborationLearningCensorReceiptV1::for_physical_runs(
        binding,
        digest(source),
        physical_run_sha256,
        CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
    )
    .unwrap();
    CollaborationLearningOfflineEntryV1::censor(
        genesis.digest().to_string(),
        sequence,
        previous_entry_sha256,
        censor,
    )
    .unwrap()
}

#[test]
fn private_immutable_records_do_not_clobber_and_manifest_is_atomic() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("journal");
    let (files, manifest) =
        JournalFiles::create_new(&root, "{\"schema\":\"genesis\"}", digest('a')).unwrap();

    assert!(JournalFiles::create_new(&root, "{}", digest('b')).is_err());
    assert!(write_immutable_private_file(
        &files.genesis_path(),
        b"tamper",
        "collaboration learning journal genesis",
    )
    .is_err());
    let entry_path = files
        .write_entry(1, &digest('b'), "{\"schema\":\"entry\"}")
        .unwrap();
    assert!(files
        .write_entry(1, &digest('b'), "{\"schema\":\"replacement\"}")
        .is_err());

    let (_, reopened) = JournalFiles::open(&root).unwrap();
    assert_eq!(reopened, manifest);
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(files.genesis_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(files.manifest_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(entry_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(files.entries_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn entry_scan_ignores_partial_temporary_files_but_rejects_other_objects() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("journal");
    let (files, _) = JournalFiles::create_new(&root, "{}", digest('a')).unwrap();
    fs::write(files.entries_path().join(".00001-partial.tmp"), b"partial").unwrap();
    assert!(files.entry_files().unwrap().is_empty());

    fs::create_dir(files.entries_path().join("unexpected")).unwrap();
    assert!(files.entry_files().is_err());
}

#[test]
fn manifest_tamper_and_missing_head_fail_closed() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("journal");
    let (files, mut manifest) = JournalFiles::create_new(&root, "{}", digest('a')).unwrap();

    manifest.entry_count = 1;
    assert!(manifest.validate().is_err());

    let mut encoded: serde_json::Value =
        serde_json::from_slice(&fs::read(files.manifest_path()).unwrap()).unwrap();
    encoded["entry_count"] = serde_json::json!(1);
    tools::write_private_file_atomically(
        &files.manifest_path(),
        &serde_json::to_vec_pretty(&encoded).unwrap(),
    )
    .unwrap();
    assert!(JournalFiles::open(&root).is_err());
}

#[test]
fn entry_scan_rejects_forks_before_decoding_payloads() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("journal");
    let (files, _) = JournalFiles::create_new(&root, "{}", digest('a')).unwrap();
    files
        .write_entry(1, &digest('b'), "{\"first\":true}")
        .unwrap();
    files
        .write_entry(1, &digest('c'), "{\"second\":true}")
        .unwrap();

    let error = files.read_entries().unwrap_err();
    assert!(error.contains("entry fork"));
}

#[test]
fn journal_open_and_record_reads_fail_closed_on_public_permissions() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("journal");
    let (files, _) = JournalFiles::create_new(&root, "{}", digest('a')).unwrap();

    #[cfg(unix)]
    {
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(JournalFiles::open(&root).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

        let path = files
            .write_entry(1, &digest('b'), "{\"schema\":\"entry\"}")
            .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(files.read_entries().is_err());
    }
}

#[test]
fn journal_paths_must_be_absolute() {
    assert!(JournalFiles::create_new(Path::new("relative"), "{}", digest('a')).is_err());
    assert!(JournalFiles::open(Path::new("relative")).is_err());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_recovers_pending_as_fixed_censor_without_retry_callback(
) {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("pending");
    let genesis = fixture_genesis();
    let binding = fixture_comparison(
        CollaborationLearningSplitV1::Train,
        2,
        CollaborationLearningArmOrderV1::DirectFirst,
    );
    let physical_agent_run_ids = vec![
        "pending-direct-run".to_string(),
        "pending-workflow-run".to_string(),
    ];
    let mut journal = CollaborationLearningExternalJournal::create_new(&root, genesis).unwrap();
    journal
        .record_pending(binding, digest('5'), physical_agent_run_ids.clone())
        .unwrap();
    assert!(journal.manifest.pending.is_some());
    drop(journal);

    let recovery = CollaborationLearningExternalJournal::recover(&root).unwrap();
    let synthesized = recovery.synthesized_censor_sha256().unwrap().to_string();
    assert_eq!(recovery.replay().entry_count(), 1);
    assert_eq!(recovery.replay().evidence_set().censored_count(), 1);
    let journal = recovery.into_journal();
    assert_eq!(journal.head_sha256(), Some(synthesized.as_str()));
    assert!(journal.manifest.pending.is_none());
    assert_eq!(
        journal.entries[0].censor_reason(),
        Some(CollaborationLearningCensorReasonV1::IncompleteInstrumentation)
    );
    let mut expected_physical_runs = physical_agent_run_ids
        .iter()
        .map(|run_id| collaboration_learning_physical_run_sha256(run_id).unwrap())
        .collect::<Vec<_>>();
    expected_physical_runs.sort();
    assert_eq!(
        journal.entries[0].physical_run_sha256().unwrap(),
        expected_physical_runs
    );
    assert_eq!(journal.files.entry_files().unwrap().len(), 1);
    drop(journal);

    let reopened = CollaborationLearningExternalJournal::recover(&root).unwrap();
    assert_eq!(reopened.replay().entry_count(), 1);
    assert_eq!(reopened.replay().head_sha256(), Some(synthesized.as_str()));
    assert!(reopened.synthesized_censor_sha256().is_none());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_adopts_one_orphan_and_rejects_fork_or_tamper(
) {
    let parent = tempfile::tempdir().unwrap();
    let genesis = fixture_genesis();

    let adopt_root = parent.path().join("adopt");
    let adopt_journal =
        CollaborationLearningExternalJournal::create_new(&adopt_root, genesis.clone()).unwrap();
    let orphan = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["orphan-run"],
    );
    adopt_journal
        .files
        .write_entry(1, orphan.digest(), &orphan.to_json().unwrap())
        .unwrap();
    drop(adopt_journal);
    let recovered = CollaborationLearningExternalJournal::recover(&adopt_root).unwrap();
    assert_eq!(recovered.replay().entry_count(), 1);
    assert_eq!(recovered.replay().head_sha256(), Some(orphan.digest()));
    assert!(recovered.synthesized_censor_sha256().is_none());

    let fork_root = parent.path().join("fork");
    let fork_journal =
        CollaborationLearningExternalJournal::create_new(&fork_root, genesis.clone()).unwrap();
    let fork_left = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["fork-left-run"],
    );
    let fork_right = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            3,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        1,
        None,
        '6',
        &["fork-right-run"],
    );
    for entry in [&fork_left, &fork_right] {
        fork_journal
            .files
            .write_entry(1, entry.digest(), &entry.to_json().unwrap())
            .unwrap();
    }
    drop(fork_journal);
    assert!(CollaborationLearningExternalJournal::recover(&fork_root).is_err());

    let tamper_root = parent.path().join("tamper");
    let tamper_journal =
        CollaborationLearningExternalJournal::create_new(&tamper_root, genesis.clone()).unwrap();
    let tampered_entry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["tampered-run"],
    );
    let mut tampered_json: serde_json::Value =
        serde_json::from_str(&tampered_entry.to_json().unwrap()).unwrap();
    tampered_json["entry_sha256"] = serde_json::json!(digest('f'));
    tamper_journal
        .files
        .write_entry(
            1,
            tampered_entry.digest(),
            &serde_json::to_string(&tampered_json).unwrap(),
        )
        .unwrap();
    drop(tamper_journal);
    assert!(CollaborationLearningExternalJournal::recover(&tamper_root).is_err());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_rejects_missing_head_mismatch_and_duplicate_physical_run(
) {
    let parent = tempfile::tempdir().unwrap();
    let genesis = fixture_genesis();

    let missing_root = parent.path().join("missing");
    let mut missing_journal =
        CollaborationLearningExternalJournal::create_new(&missing_root, genesis.clone()).unwrap();
    let missing_entry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["missing-run"],
    );
    missing_journal.append_entry(missing_entry.clone()).unwrap();
    missing_journal.append_entry(missing_entry.clone()).unwrap();
    missing_journal
        .complete_pending(missing_entry.clone())
        .unwrap();
    let conflicting_retry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            3,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        1,
        None,
        '6',
        &["conflicting-retry-run"],
    );
    assert!(missing_journal
        .append_entry(conflicting_retry.clone())
        .is_err());
    assert!(missing_journal.complete_pending(conflicting_retry).is_err());
    assert_eq!(missing_journal.entries.len(), 1);
    fs::remove_file(missing_journal.files.entry_path(1, missing_entry.digest())).unwrap();
    drop(missing_journal);
    assert!(CollaborationLearningExternalJournal::recover(&missing_root).is_err());

    let head_root = parent.path().join("head-mismatch");
    let mut head_journal =
        CollaborationLearningExternalJournal::create_new(&head_root, genesis.clone()).unwrap();
    let head_entry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["head-run"],
    );
    head_journal.append_entry(head_entry).unwrap();
    head_journal.manifest.head_entry_sha256 = Some(digest('f'));
    head_journal.manifest.reseal().unwrap();
    head_journal
        .files
        .write_manifest(&head_journal.manifest)
        .unwrap();
    drop(head_journal);
    assert!(CollaborationLearningExternalJournal::recover(&head_root).is_err());

    let duplicate_root = parent.path().join("duplicate-physical-run");
    let mut duplicate_journal =
        CollaborationLearningExternalJournal::create_new(&duplicate_root, genesis.clone()).unwrap();
    let first = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["replayed-physical-run"],
    );
    duplicate_journal.append_entry(first.clone()).unwrap();
    let duplicate = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            3,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        2,
        Some(first.digest().to_string()),
        '6',
        &["replayed-physical-run"],
    );
    duplicate_journal
        .files
        .write_entry(2, duplicate.digest(), &duplicate.to_json().unwrap())
        .unwrap();
    drop(duplicate_journal);
    assert!(CollaborationLearningExternalJournal::recover(&duplicate_root).is_err());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_adopts_exact_orphan_retry_without_creating_a_fork(
) {
    let parent = tempfile::tempdir().unwrap();
    let genesis = fixture_genesis();

    let exact_root = parent.path().join("exact-orphan-retry");
    let mut exact_journal =
        CollaborationLearningExternalJournal::create_new(&exact_root, genesis.clone()).unwrap();
    let exact_entry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["exact-orphan-run"],
    );
    exact_journal
        .files
        .write_entry(
            exact_entry.sequence(),
            exact_entry.digest(),
            &exact_entry.to_json().unwrap(),
        )
        .unwrap();
    exact_journal.append_entry(exact_entry.clone()).unwrap();
    assert_eq!(exact_journal.entries.len(), 1);
    assert_eq!(exact_journal.head_sha256(), Some(exact_entry.digest()));
    assert_eq!(exact_journal.files.entry_files().unwrap().len(), 1);

    let conflict_root = parent.path().join("conflicting-orphan-retry");
    let mut conflict_journal =
        CollaborationLearningExternalJournal::create_new(&conflict_root, genesis.clone()).unwrap();
    let conflicting_orphan = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            3,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        1,
        None,
        '6',
        &["conflicting-orphan-run"],
    );
    conflict_journal
        .files
        .write_entry(
            conflicting_orphan.sequence(),
            conflicting_orphan.digest(),
            &conflicting_orphan.to_json().unwrap(),
        )
        .unwrap();
    let desired_entry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["desired-orphan-run"],
    );
    assert!(conflict_journal.append_entry(desired_entry).is_err());
    assert_eq!(conflict_journal.files.entry_files().unwrap().len(), 1);
    assert!(CollaborationLearningExternalJournal::recover(&conflict_root).is_ok());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_rejects_append_after_frozen_prefix_but_allows_exact_retry(
) {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("terminal-prefix");
    let genesis = fixture_genesis();
    let mut journal =
        CollaborationLearningExternalJournal::create_new(&root, genesis.clone()).unwrap();
    let frozen_entry = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        1,
        None,
        '5',
        &["frozen-run"],
    );
    journal.append_entry(frozen_entry.clone()).unwrap();
    journal.append_entry(frozen_entry.clone()).unwrap();

    let after_frozen = fixture_censor_entry(
        &genesis,
        fixture_comparison(
            CollaborationLearningSplitV1::Train,
            3,
            CollaborationLearningArmOrderV1::WorkflowFirst,
        ),
        2,
        Some(frozen_entry.digest().to_string()),
        '6',
        &["after-frozen-run"],
    );
    assert!(journal.append_entry(after_frozen).is_err());
    assert!(journal
        .record_pending(
            fixture_comparison(
                CollaborationLearningSplitV1::Train,
                3,
                CollaborationLearningArmOrderV1::WorkflowFirst,
            ),
            digest('6'),
            vec!["after-frozen-run".to_string()],
        )
        .is_err());
    assert_eq!(journal.entries.len(), 1);
    assert_eq!(journal.files.entry_files().unwrap().len(), 1);
}
