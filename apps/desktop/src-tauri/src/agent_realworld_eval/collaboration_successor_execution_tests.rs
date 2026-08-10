use super::execution::*;
use super::preflight::*;
use super::*;
use crate::configuration_models::ProviderConfig;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MANIFEST_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../benchmarks/agent/collaboration-successor-protocol-v1.json"
));
const SUITE_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../benchmarks/agent/collaboration-successor-v1.json"
));
const RUNNER_BYTES: &[u8] = b"fixture exact successor runner bytes";
const CREDENTIAL: &str = "fixture-provider-credential-never-persisted";
const ISSUED_AT_MS: u64 = 2_000;
const EXPIRES_AT_MS: u64 = ISSUED_AT_MS + 15 * 60 * 1_000;

fn protocol() -> ValidatedProtocol<'static> {
    parse_and_validate_protocol(MANIFEST_BYTES, SUITE_BYTES).expect("tracked successor protocol")
}

fn provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "fixture-provider".into(),
        provider_resource: "fixture-resource".into(),
        base_url: "https://provider.invalid/v1".into(),
        api_key: CREDENTIAL.into(),
        model: "owner-model".into(),
        conductor_model: "reasoning-model".into(),
        planner_model: "planner-model".into(),
        executor_model: "specialist-model".into(),
        reviewer_model: "verifier-model".into(),
        summarizer_model: "utility-model".into(),
        embedding_model: "embedding-model".into(),
        collaboration_policy: "auto_router".into(),
        context_window_tokens: 128_000,
        agent_system_prompt: "fixture system prompt".into(),
        ..ProviderConfig::default()
    }
}

struct Fixture {
    protocol: ValidatedProtocol<'static>,
    preflight: SuccessorPreflightReceipt,
    authorization_path: PathBuf,
    output_root: PathBuf,
}

fn fixture(parent: &Path, suffix: &str) -> Fixture {
    let protocol = protocol();
    let parent = parent.canonicalize().unwrap();
    let authorization_path = parent.join(format!("authorization-{suffix}.json"));
    let output_root = parent.join(format!("output-{suffix}"));
    let preflight = build_receipt(
        &protocol,
        SourceBindingReceipt {
            head: "1".repeat(40),
            tree: "2".repeat(40),
        },
        provider_binding(&provider_config()).unwrap(),
        materialize_cells(&protocol).unwrap(),
        &output_root,
        1_000,
    )
    .unwrap();
    Fixture {
        protocol,
        preflight,
        authorization_path,
        output_root,
    }
}

fn issue(fixture: &Fixture) -> AuthorizationV1 {
    issue_authorization(AuthorizationIssue {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        issued_at_ms: ISSUED_AT_MS,
        expires_at_ms: EXPIRES_AT_MS,
        nonce: &"a".repeat(64),
        credential: CREDENTIAL,
    })
    .unwrap()
}

fn validate(fixture: &Fixture) -> ValidatedAuthorizationV1 {
    load_and_validate_authorization(AuthorizationValidation {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        credential: CREDENTIAL,
        now_ms: ISSUED_AT_MS + 1,
    })
    .unwrap()
}

fn write_and_validate(fixture: &Fixture) -> ValidatedAuthorizationV1 {
    write_authorization_new(&fixture.authorization_path, &issue(fixture)).unwrap();
    validate(fixture)
}

fn journal_fixture(parent: &Path, suffix: &str) -> (SuccessorExecutionJournal, PathBuf) {
    let fixture = fixture(parent, suffix);
    let validated = write_and_validate(&fixture);
    let tombstone = consume_authorization_once(&validated, ISSUED_AT_MS + 2).unwrap();
    let journal =
        SuccessorExecutionJournal::create_new(&fixture.output_root, &validated, &tombstone)
            .unwrap();
    (journal, fixture.output_root)
}

fn usage() -> ArmObservedUsageV1 {
    ArmObservedUsageV1 {
        duration_ms: 10,
        model_calls: 2,
        tool_calls: 1,
        agent_turns: 2,
        physical_model_attempts: 2,
        total_tokens: 100,
    }
}

fn complete_arm(
    journal: &mut SuccessorExecutionJournal,
    ordinal: usize,
    kind: ArmKindV1,
    marker: char,
) {
    journal
        .reserve_arm(ordinal, kind, marker.to_string().repeat(64))
        .unwrap();
    journal
        .record_arm_terminal(
            ordinal,
            kind,
            "completed".into(),
            marker
                .to_ascii_uppercase()
                .to_ascii_lowercase()
                .to_string()
                .repeat(64),
            Some(usage()),
        )
        .unwrap();
}

#[test]
fn agent_collaboration_successor_execution_contract_authorization_is_canonical_private_and_fully_bound(
) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fixture(temp.path(), "canonical");
    let authorization = issue(&fixture);
    write_authorization_new(&fixture.authorization_path, &authorization).unwrap();
    let encoded = fs::read(&fixture.authorization_path).unwrap();
    assert_eq!(encoded, serde_json::to_vec(&authorization).unwrap());
    assert!(!String::from_utf8_lossy(&encoded).contains(CREDENTIAL));
    assert_eq!(authorization.pair_count(), 3);
    assert_eq!(authorization.run_count(), 6);
    assert_eq!(authorization.cell_count(), 3);
    assert_eq!(authorization.runner_sha256(), sha256_hex(RUNNER_BYTES));
    assert_eq!(authorization.validity_window_ms(), Some(900_000));
    assert_eq!(
        validate(&fixture)
            .authorization()
            .authorization_sha256()
            .len(),
        64
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.authorization_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn agent_collaboration_successor_execution_contract_rejects_tamper_expiry_runner_and_credential_drift(
) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fixture(temp.path(), "reject");
    write_authorization_new(&fixture.authorization_path, &issue(&fixture)).unwrap();

    let expired = AuthorizationValidation {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        credential: CREDENTIAL,
        now_ms: EXPIRES_AT_MS,
    };
    assert!(load_and_validate_authorization(expired).is_err());
    assert!(load_and_validate_authorization(AuthorizationValidation {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        credential: "different-credential",
        now_ms: ISSUED_AT_MS + 1,
    })
    .is_err());

    let runner_drift = AuthorizationValidation {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: b"different runner bytes",
        credential: CREDENTIAL,
        now_ms: ISSUED_AT_MS + 1,
    };
    assert!(load_and_validate_authorization(runner_drift).is_err());

    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&fixture.authorization_path).unwrap()).unwrap();
    value["app_version"] = serde_json::json!("tampered");
    fs::write(
        &fixture.authorization_path,
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert!(load_and_validate_authorization(AuthorizationValidation {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        credential: CREDENTIAL,
        now_ms: ISSUED_AT_MS + 1,
    })
    .is_err());
}

#[test]
fn agent_collaboration_successor_execution_contract_rejects_unbounded_authorization_window() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fixture(temp.path(), "ttl");
    assert!(issue_authorization(AuthorizationIssue {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &fixture.authorization_path,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        issued_at_ms: ISSUED_AT_MS,
        expires_at_ms: EXPIRES_AT_MS + 1,
        nonce: &"b".repeat(64),
        credential: CREDENTIAL,
    })
    .is_err());
}

#[test]
fn agent_collaboration_successor_execution_contract_consumes_unique_output_root_once_under_concurrency(
) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fixture(temp.path(), "concurrent");
    let validated = Arc::new(write_and_validate(&fixture));
    let first = {
        let validated = Arc::clone(&validated);
        std::thread::spawn(move || consume_authorization_once(&validated, ISSUED_AT_MS + 2))
    };
    let second = {
        let validated = Arc::clone(&validated);
        std::thread::spawn(move || consume_authorization_once(&validated, ISSUED_AT_MS + 2))
    };
    let outcomes = [
        first.join().unwrap().is_ok(),
        second.join().unwrap().is_ok(),
    ];
    assert_eq!(outcomes.into_iter().filter(|ok| *ok).count(), 1);
    assert!(fixture
        .output_root
        .join("successor-authorization-consumed.json")
        .is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.output_root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(
                fixture
                    .output_root
                    .join("successor-authorization-consumed.json"),
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }

    let alternate = fixture
        .output_root
        .with_file_name("alternate-authorization.json");
    assert!(issue_authorization(AuthorizationIssue {
        protocol: &fixture.protocol,
        preflight: &fixture.preflight,
        authorization_path: &alternate,
        output_root: &fixture.output_root,
        runner_bytes: RUNNER_BYTES,
        issued_at_ms: ISSUED_AT_MS,
        expires_at_ms: EXPIRES_AT_MS,
        nonce: &"c".repeat(64),
        credential: CREDENTIAL,
    })
    .is_err());
}

#[test]
fn agent_collaboration_successor_execution_contract_root_only_crash_recovers_frozen_without_execute(
) {
    let temp = tempfile::tempdir().unwrap();
    let fixture = fixture(temp.path(), "root-crash");
    let validated = write_and_validate(&fixture);
    let tombstone = consume_authorization_once(&validated, ISSUED_AT_MS + 2).unwrap();
    let (journal, recovery) = SuccessorExecutionJournal::recover(&fixture.output_root).unwrap();
    assert!(journal.is_none());
    assert_eq!(recovery, JournalRecoveryV1::Frozen);
    assert!(fixture
        .output_root
        .join("successor-execution-recovery.json")
        .is_file());
    assert!(
        SuccessorExecutionJournal::create_new(&fixture.output_root, &validated, &tombstone)
            .is_err()
    );
    assert!(!fixture
        .output_root
        .join("successor-execution-journal.json")
        .exists());
}

#[test]
fn agent_collaboration_successor_execution_contract_recovery_rejects_self_consistent_journal_not_anchored_to_tombstone(
) {
    let temp = tempfile::tempdir().unwrap();
    let (journal, root) = journal_fixture(temp.path(), "journal-anchor");
    drop(journal);
    let path = root.join("successor-execution-journal.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["authorization"]["app_version"] = serde_json::json!("tampered");
    value["authorization"]["authorization_sha256"] = serde_json::json!("");
    let mut authorization_bytes = b"cindx.collaboration-successor-authorization.v1\0".to_vec();
    authorization_bytes.extend(serde_json::to_vec(&value["authorization"]).unwrap());
    value["authorization"]["authorization_sha256"] =
        serde_json::json!(sha256_hex(&authorization_bytes));
    value["journal_sha256"] = serde_json::json!("");
    let mut journal_bytes = b"cindx.collaboration-successor-execution-journal.v1\0".to_vec();
    journal_bytes.extend(serde_json::to_vec(&value).unwrap());
    value["journal_sha256"] = serde_json::json!(sha256_hex(&journal_bytes));
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(SuccessorExecutionJournal::recover(&root).is_err());
}

#[test]
fn agent_collaboration_successor_execution_contract_crash_matrix_and_half_pair_never_resume() {
    let temp = tempfile::tempdir().unwrap();
    for stage in 0..=3 {
        let (mut journal, root) = journal_fixture(temp.path(), &format!("crash-{stage}"));
        if stage >= 1 {
            journal.reserve_campaign().unwrap();
        }
        if stage >= 2 {
            journal.reserve_cell(1).unwrap();
        }
        if stage >= 3 {
            complete_arm(&mut journal, 1, ArmKindV1::Direct, 'd');
        }
        drop(journal);
        let (recovered, recovery) = SuccessorExecutionJournal::recover(&root).unwrap();
        assert_eq!(recovery, JournalRecoveryV1::Frozen);
        let recovered = recovered.unwrap();
        assert_eq!(
            recovered.terminal_disposition(),
            Some(TerminalDispositionV1::Censored)
        );
        if stage == 3 {
            assert!(recovered.arm_is_planned(1, ArmKindV1::Workflow));
            assert_eq!(recovered.charged().runs, 1);
        }
        drop(recovered);
    }
}

#[test]
fn agent_collaboration_successor_execution_contract_live_journal_cannot_race_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let (mut live, root) = journal_fixture(temp.path(), "live-recovery-race");
    live.reserve_campaign().unwrap();
    live.reserve_cell(1).unwrap();
    let journal_path = root.join("successor-execution-journal.json");
    let before = fs::read(&journal_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.join("successor-execution.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    assert!(SuccessorExecutionJournal::recover(&root).is_err());
    assert_eq!(fs::read(&journal_path).unwrap(), before);

    drop(live);
    let (recovered, recovery) = SuccessorExecutionJournal::recover(&root).unwrap();
    assert_eq!(recovery, JournalRecoveryV1::Frozen);
    let mut recovered = recovered.unwrap();
    assert_eq!(
        recovered.terminal_disposition(),
        Some(TerminalDispositionV1::Censored)
    );
    assert!(recovered.reserve_cell(1).is_err());
    assert!(SuccessorExecutionJournal::recover(&root).is_err());

    drop(recovered);
    let (_, recovery) = SuccessorExecutionJournal::recover(&root).unwrap();
    assert_eq!(
        recovery,
        JournalRecoveryV1::Terminal(TerminalDispositionV1::Censored)
    );
}

#[test]
fn agent_collaboration_successor_execution_contract_six_arm_budget_is_checked_and_terminal_is_append_closed(
) {
    let temp = tempfile::tempdir().unwrap();
    let (mut journal, root) = journal_fixture(temp.path(), "complete");
    journal.reserve_campaign().unwrap();
    let orders = [
        [ArmKindV1::Direct, ArmKindV1::Workflow],
        [ArmKindV1::Workflow, ArmKindV1::Direct],
        [ArmKindV1::Direct, ArmKindV1::Workflow],
    ];
    for (index, order) in orders.into_iter().enumerate() {
        let ordinal = index + 1;
        journal.reserve_cell(ordinal).unwrap();
        complete_arm(
            &mut journal,
            ordinal,
            order[0],
            char::from(b'a' + index as u8),
        );
        complete_arm(
            &mut journal,
            ordinal,
            order[1],
            char::from(b'd' + index as u8),
        );
        journal
            .commit_pair(
                ordinal,
                char::from(b'1' + index as u8).to_string().repeat(64),
            )
            .unwrap();
        journal
            .commit_cell(
                ordinal,
                char::from(b'4' + index as u8).to_string().repeat(64),
            )
            .unwrap();
    }
    assert_eq!(journal.charged().runs, 6);
    assert_eq!(journal.charged().model_calls, 120);
    assert_eq!(journal.charged().tool_calls, 288);
    assert_eq!(journal.charged().agent_turns, 120);
    assert_eq!(journal.charged().physical_model_attempts, 480);
    assert_eq!(journal.charged().total_tokens, 503_316_480);
    assert_eq!(journal.observed().runs, 6);
    journal
        .finish_ready_for_independent_review("f".repeat(64))
        .unwrap();
    assert!(journal.reserve_cell(1).is_err());
    drop(journal);
    let (recovered, recovery) = SuccessorExecutionJournal::recover(&root).unwrap();
    assert_eq!(
        recovery,
        JournalRecoveryV1::Terminal(TerminalDispositionV1::ReadyForIndependentReview)
    );
    assert_eq!(
        recovered.unwrap().terminal_disposition(),
        Some(TerminalDispositionV1::ReadyForIndependentReview)
    );
}

#[test]
fn agent_collaboration_successor_execution_contract_rejects_resource_overflow_and_unknown_terminal()
{
    let overflow = ResourceTotalsV1 {
        total_tokens: u64::MAX,
        ..ResourceTotalsV1::default()
    };
    assert!(overflow
        .checked_add(ResourceTotalsV1 {
            total_tokens: 1,
            ..ResourceTotalsV1::default()
        })
        .is_err());

    let temp = tempfile::tempdir().unwrap();
    let (mut journal, _) = journal_fixture(temp.path(), "unknown");
    journal.reserve_campaign().unwrap();
    journal.reserve_cell(1).unwrap();
    journal
        .reserve_arm(1, ArmKindV1::Direct, "a".repeat(64))
        .unwrap();
    journal
        .record_arm_terminal(1, ArmKindV1::Direct, "failed".into(), "b".repeat(64), None)
        .unwrap();
    assert_eq!(journal.charged().runs, 1);
    assert_eq!(
        journal.terminal_disposition(),
        Some(TerminalDispositionV1::Censored)
    );
    assert!(journal
        .reserve_arm(1, ArmKindV1::Workflow, "c".repeat(64))
        .is_err());
}
