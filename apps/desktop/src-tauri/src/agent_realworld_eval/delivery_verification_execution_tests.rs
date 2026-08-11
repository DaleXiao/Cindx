use super::delivery_verification_authorization::*;
use super::delivery_verification_execution_journal::*;
use super::delivery_verification_preflight::{
    build_receipt, protocol_snapshot, provider_binding, DeliveryVerificationPreflightReceipt,
    DeliveryVerificationSourceBindingReceipt,
};
use super::delivery_verification_protocol::{
    parse_and_validate_protocol, ValidatedProtocol, DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH,
    DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH,
};
use super::delivery_verification_runner_binary::{
    code_directory_sha256_for_bytes, read_current_delivery_execute, running_code_directory_sha256,
    DeliveryVerificationRunnerBinary,
};
use crate::configuration_models::ProviderConfig;
use agent_core::ModelRole;
use orchestrator::sha256_hex;
use serde_json::Value;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
use tempfile::TempDir;

const PREFLIGHT_AT_MS: u64 = 1_000;
const ISSUED_AT_MS: u64 = 10_000;
const EXPIRES_AT_MS: u64 = ISSUED_AT_MS + DELIVERY_AUTHORIZATION_TTL_MS;
const CONSUMED_AT_MS: u64 = 20_000;
const CAMPAIGN_AT_MS: u64 = 30_000;
const RUNNER_BYTES: &[u8] = b"provider-free exact delivery execute fixture";
#[cfg(target_os = "macos")]
const RUNNER_SWAP_CHILD_ENV: &str = "CINDX_DELIVERY_VERIFICATION_V2_RUNNER_SWAP_TEST";

const STAGES: [DeliveryVerificationCallStageV1; 4] = [
    DeliveryVerificationCallStageV1::OwnerDraft,
    DeliveryVerificationCallStageV1::VerifierInitial,
    DeliveryVerificationCallStageV1::OwnerRepair,
    DeliveryVerificationCallStageV1::VerifierRecheck,
];

struct Fixture {
    _temp: TempDir,
    repo_root: PathBuf,
    preflight_path: PathBuf,
    authorization_path: PathBuf,
    output_root: PathBuf,
    preflight: DeliveryVerificationPreflightReceipt,
    authorization: DeliveryVerificationAuthorizationV1,
    config: ProviderConfig,
}

fn digest(value: impl AsRef<[u8]>) -> String {
    sha256_hex(value.as_ref())
}

fn runner(bytes: &[u8]) -> DeliveryVerificationRunnerBinary {
    DeliveryVerificationRunnerBinary {
        bytes: bytes.to_vec(),
        code_directory_sha256: digest("runner code directory"),
    }
}

#[cfg(target_os = "macos")]
fn run_runner_swap_child_if_requested() -> bool {
    let Some(root) = std::env::var_os(RUNNER_SWAP_CHILD_ENV).map(PathBuf::from) else {
        return false;
    };
    fs::write(root.join("ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("continue").exists() {
        assert!(Instant::now() < deadline, "runner swap child timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    let error = read_current_delivery_execute().unwrap_err();
    assert!(error.contains("running delivery execute image differs"));
    true
}

#[cfg(target_os = "macos")]
fn assert_running_image_rejects_execute_path_swap() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join(format!(
        "{}{}",
        super::delivery_verification_protocol::DELIVERY_VERIFICATION_EXECUTE_BINARY_NAME,
        std::env::consts::EXE_SUFFIX
    ));
    let current_executable = std::env::current_exe().unwrap();
    fs::copy(&current_executable, &executable).unwrap();
    let replacement = temp.path().join("replacement");
    fs::copy(&current_executable, &replacement).unwrap();
    assert!(Command::new("/usr/bin/codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--identifier",
            "cindx.delivery-verification.runner-swap",
            "--timestamp=none",
        ])
        .arg(&replacement)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success());
    let current_identity =
        code_directory_sha256_for_bytes(&fs::read(&current_executable).unwrap()).unwrap();
    let replacement_identity =
        code_directory_sha256_for_bytes(&fs::read(&replacement).unwrap()).unwrap();
    assert_ne!(current_identity, replacement_identity);
    #[cfg(unix)]
    {
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o500)).unwrap();
    }
    let mut child = Command::new(&executable)
        .arg("agent_delivery_verification_execution_contract_authorization_binds_exact_frozen_authority")
        .arg("--nocapture")
        .env(RUNNER_SWAP_CHILD_ENV, temp.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !temp.path().join("ready").exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("runner swap child exited before its barrier: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("runner swap child did not reach its barrier");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::rename(&replacement, &executable).unwrap();
    fs::write(temp.path().join("continue"), b"continue").unwrap();
    assert!(child.wait().unwrap().success());
}

fn source() -> DeliveryVerificationSourceBindingReceipt {
    DeliveryVerificationSourceBindingReceipt {
        head: "0123456789abcdef0123456789abcdef01234567".into(),
        tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
    }
}

fn provider_config() -> ProviderConfig {
    ProviderConfig {
        provider_id: "openai-compatible".into(),
        provider_resource: "delivery-resource".into(),
        base_url: "https://provider.example/v1".into(),
        api_key: "fixture-secret-never-serialized".into(),
        executor_model: "delivery-owner-model".into(),
        reviewer_model: "delivery-verifier-model".into(),
        context_window_tokens: 131_072,
        ..ProviderConfig::default()
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn encode_preflight(receipt: &DeliveryVerificationPreflightReceipt) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(receipt).unwrap();
    bytes.push(b'\n');
    bytes
}

fn with_fixture<R>(test: impl FnOnce(&ValidatedProtocol<'_>, Fixture) -> R) -> R {
    let repo_root = repo_root();
    let manifest = fs::read(repo_root.join(DELIVERY_VERIFICATION_PROTOCOL_RELATIVE_PATH)).unwrap();
    let suite = fs::read(repo_root.join(DELIVERY_VERIFICATION_SUITE_RELATIVE_PATH)).unwrap();
    let protocol = parse_and_validate_protocol(&manifest, &suite).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let temp_root = temp.path().canonicalize().unwrap();
    let preflight_path = temp_root.join("preflight.json");
    let authorization_path = temp_root.join("authorization.json");
    let output_root = temp_root.join("execution");
    let config = provider_config();
    let preflight = build_receipt(
        &protocol_snapshot(&protocol),
        source(),
        provider_binding(&config).unwrap(),
        &runner(RUNNER_BYTES),
        &output_root,
        PREFLIGHT_AT_MS,
    )
    .unwrap();
    write_new_private_file(
        &preflight_path,
        &encode_preflight(&preflight),
        "test delivery preflight",
    )
    .unwrap();
    let authorization =
        issue_delivery_verification_authorization(DeliveryVerificationAuthorizationIssue {
            protocol: &protocol,
            preflight: &preflight,
            repo_root: &repo_root,
            preflight_path: &preflight_path,
            authorization_path: &authorization_path,
            output_root: &output_root,
            runner: &runner(RUNNER_BYTES),
            issued_at_ms: ISSUED_AT_MS,
            expires_at_ms: EXPIRES_AT_MS,
            nonce: &digest("authorization nonce"),
            credential: &config.api_key,
        })
        .unwrap();
    write_delivery_verification_authorization_new(&repo_root, &authorization_path, &authorization)
        .unwrap();
    test(
        &protocol,
        Fixture {
            _temp: temp,
            repo_root,
            preflight_path,
            authorization_path,
            output_root,
            preflight,
            authorization,
            config,
        },
    )
}

fn validate_authorization(
    protocol: &ValidatedProtocol<'_>,
    fixture: &Fixture,
    runner: &DeliveryVerificationRunnerBinary,
    credential: &str,
    now_ms: u64,
) -> Result<ValidatedDeliveryVerificationAuthorizationV1, String> {
    load_and_validate_delivery_verification_authorization(
        DeliveryVerificationAuthorizationValidation {
            protocol,
            preflight: &fixture.preflight,
            repo_root: &fixture.repo_root,
            preflight_path: &fixture.preflight_path,
            authorization_path: &fixture.authorization_path,
            output_root: &fixture.output_root,
            runner,
            credential,
            now_ms,
        },
    )
}

fn new_journal(
    protocol: &ValidatedProtocol<'_>,
    fixture: &Fixture,
) -> (
    ValidatedDeliveryVerificationAuthorizationV1,
    ConsumedDeliveryVerificationAuthorizationV1,
    DeliveryVerificationExecutionJournal,
) {
    let validated = validate_authorization(
        protocol,
        fixture,
        &runner(RUNNER_BYTES),
        &fixture.config.api_key,
        CONSUMED_AT_MS,
    )
    .unwrap();
    let tombstone =
        consume_delivery_verification_authorization_once(&validated, CONSUMED_AT_MS).unwrap();
    let journal = DeliveryVerificationExecutionJournal::create_new(
        &fixture.output_root,
        &validated,
        &tombstone,
    )
    .unwrap();
    (validated, tombstone, journal)
}

fn journal_json(output_root: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(output_root.join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME)).unwrap(),
    )
    .unwrap()
}

fn reservation_input(
    authorization: &DeliveryVerificationAuthorizationV1,
    case_ordinal: usize,
    stage: DeliveryVerificationCallStageV1,
) -> DeliveryVerificationCallReservationInputV1 {
    let (role, configured_model_sha256, max_output_tokens) = match stage {
        DeliveryVerificationCallStageV1::OwnerDraft
        | DeliveryVerificationCallStageV1::OwnerRepair => (
            ModelRole::Executor,
            authorization.provider.owner_model_sha256.clone(),
            authorization.budget.max_owner_output_tokens,
        ),
        DeliveryVerificationCallStageV1::VerifierInitial
        | DeliveryVerificationCallStageV1::VerifierRecheck => (
            ModelRole::Reviewer,
            authorization.provider.verifier_model_sha256.clone(),
            authorization.budget.max_verifier_output_tokens,
        ),
    };
    let request = format!("case={case_ordinal};stage={stage:?}");
    let wire_payload = format!("wire:{request}");
    DeliveryVerificationCallReservationInputV1 {
        case_ordinal,
        stage,
        role,
        configured_model_sha256,
        semantic_request_sha256: digest(request.as_bytes()),
        semantic_request_bytes: request.len() as u64,
        wire_payload_sha256: digest(wire_payload.as_bytes()),
        wire_payload_bytes: wire_payload.len() as u64,
        max_output_tokens,
        reserved_at_ms: CAMPAIGN_AT_MS + 100 + case_ordinal as u64 * 10 + stage_index(stage) as u64,
    }
}

fn stage_index(stage: DeliveryVerificationCallStageV1) -> usize {
    STAGES.iter().position(|value| *value == stage).unwrap()
}

fn complete_stage(
    journal: &mut DeliveryVerificationExecutionJournal,
    authorization: &DeliveryVerificationAuthorizationV1,
    case_ordinal: usize,
    stage: DeliveryVerificationCallStageV1,
    total_tokens: u64,
) -> DeliveryVerificationResponseArtifactV1 {
    let input = reservation_input(authorization, case_ordinal, stage);
    let request_payload_sha256 = input.wire_payload_sha256.clone();
    let configured_model_sha256 = input.configured_model_sha256.clone();
    let reserved_at_ms = input.reserved_at_ms;
    let permit = journal.reserve_call(input).unwrap();
    let response = format!(
        "response case={} call={}",
        case_ordinal,
        permit.global_call_ordinal()
    );
    journal
        .record_call_terminal(
            DeliveryVerificationCallTerminalInputV1 {
                permit,
                status: DeliveryVerificationCallTerminalStatusV1::Completed,
                failure_class: None,
                retryable: None,
                provider_status_code: Some(200),
                latency_ms: 7,
                request_payload_sha256: Some(request_payload_sha256),
                response_semantic_sha256: Some(digest(response.as_bytes())),
                provider_response_id_sha256: Some(digest(format!(
                    "response-id-{case_ordinal}-{stage:?}"
                ))),
                provider_response_model_sha256: Some(configured_model_sha256),
                provider_system_fingerprint_sha256: Some(digest("provider-system-fingerprint")),
                provider_receipt_status: Some("observed".into()),
                usage: Some(DeliveryVerificationCallUsageV1 {
                    prompt_tokens: total_tokens.saturating_sub(1),
                    completion_tokens: 1,
                    total_tokens,
                    usage_source: "provider".into(),
                    usage_estimated: false,
                }),
                terminal_at_ms: reserved_at_ms + 1,
            },
            Some(response.as_bytes()),
        )
        .unwrap()
        .expect("completed call should retain its response artifact")
}

fn complete_case_input(
    case_ordinal: usize,
    owner: &DeliveryVerificationResponseArtifactV1,
    treatment: &DeliveryVerificationResponseArtifactV1,
    completed_at_ms: u64,
) -> DeliveryVerificationCaseTerminalInputV1 {
    DeliveryVerificationCaseTerminalInputV1 {
        case_ordinal,
        status: DeliveryVerificationCaseTerminalStatusV1::Complete,
        control_passed: Some(false),
        treatment_passed: Some(true),
        owner_draft_sha256: Some(owner.sha256.clone()),
        owner_draft_bytes: Some(owner.bytes),
        control_output_sha256: Some(owner.sha256.clone()),
        treatment_output_sha256: Some(treatment.sha256.clone()),
        observation_sha256: digest(format!("observation-{case_ordinal}")),
        completed_at_ms,
    }
}

fn terminal_complete_case(
    journal: &mut DeliveryVerificationExecutionJournal,
    case_ordinal: usize,
    owner: &DeliveryVerificationResponseArtifactV1,
    treatment: &DeliveryVerificationResponseArtifactV1,
) {
    journal
        .record_case_terminal(complete_case_input(
            case_ordinal,
            owner,
            treatment,
            CAMPAIGN_AT_MS + 1_000 + case_ordinal as u64,
        ))
        .unwrap();
}

fn terminal_structural_case(
    journal: &mut DeliveryVerificationExecutionJournal,
    case_ordinal: usize,
) {
    journal
        .record_case_terminal(DeliveryVerificationCaseTerminalInputV1 {
            case_ordinal,
            status: DeliveryVerificationCaseTerminalStatusV1::StructuralFailure,
            control_passed: None,
            treatment_passed: None,
            owner_draft_sha256: None,
            owner_draft_bytes: None,
            control_output_sha256: None,
            treatment_output_sha256: None,
            observation_sha256: digest(format!("structural-observation-{case_ordinal}")),
            completed_at_ms: CAMPAIGN_AT_MS + 1_000 + case_ordinal as u64,
        })
        .unwrap();
}

#[test]
fn agent_delivery_verification_execution_contract_authorization_binds_exact_frozen_authority() {
    #[cfg(target_os = "macos")]
    {
        if run_runner_swap_child_if_requested() {
            return;
        }
        let current_bytes = fs::read(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(
            code_directory_sha256_for_bytes(&current_bytes).unwrap(),
            running_code_directory_sha256().unwrap()
        );
        assert_running_image_rejects_execute_path_swap();
    }
    with_fixture(|protocol, fixture| {
        println!(
            "{}\n{}",
            DELIVERY_AUTHORIZATION_SCHEMA, DELIVERY_EXECUTION_JOURNAL_SCHEMA
        );
        fixture.authorization.validate_static().unwrap();
        assert_eq!(fixture.authorization.cases.len(), 32);
        assert_eq!(fixture.authorization.protocol_id, protocol.protocol_id());
        assert_eq!(fixture.authorization.runner_sha256, digest(RUNNER_BYTES));
        assert_eq!(
            fixture.authorization.runner_bytes,
            RUNNER_BYTES.len() as u64
        );
        assert_eq!(
            fixture.authorization.runner_code_directory_sha256,
            runner(RUNNER_BYTES).code_directory_sha256
        );
        assert_eq!(
            fixture.authorization.expires_at_ms - fixture.authorization.issued_at_ms,
            DELIVERY_AUTHORIZATION_TTL_MS
        );
        let encoded = serde_json::to_string(&fixture.authorization).unwrap();
        for secret in [
            fixture.config.api_key.as_str(),
            fixture.config.base_url.as_str(),
            fixture.config.executor_model.as_str(),
            fixture.config.reviewer_model.as_str(),
        ] {
            assert!(!encoded.contains(secret));
        }
        assert!(read_current_delivery_execute().is_err());

        let mut consumed_v1 = fixture.authorization.clone();
        consumed_v1.protocol_id =
            super::delivery_verification_protocol::CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID
                .into();
        consumed_v1.authorization_sha256 = authorization_digest(&consumed_v1).unwrap();
        assert!(consumed_v1.validate_static().is_err());

        let (_, mut tombstone, _) = new_journal(protocol, &fixture);
        tombstone.authorization.protocol_id =
            super::delivery_verification_protocol::CONSUMED_DELIVERY_VERIFICATION_PROTOCOL_V1_ID
                .into();
        tombstone.authorization.authorization_sha256 =
            authorization_digest(&tombstone.authorization).unwrap();
        tombstone.authorization_sha256 = tombstone.authorization.authorization_sha256.clone();
        tombstone.tombstone_sha256 = tombstone_digest(&tombstone).unwrap();
        assert!(tombstone.validate_for(&tombstone.authorization).is_err());
    });
}

#[test]
fn agent_delivery_verification_execution_contract_authorization_requires_exact_fifteen_minute_ttl()
{
    with_fixture(|protocol, fixture| {
        for expires_at_ms in [EXPIRES_AT_MS - 1, EXPIRES_AT_MS + 1] {
            let path = fixture
                ._temp
                .path()
                .join(format!("bad-authorization-{expires_at_ms}.json"));
            assert!(issue_delivery_verification_authorization(
                DeliveryVerificationAuthorizationIssue {
                    protocol,
                    preflight: &fixture.preflight,
                    repo_root: &fixture.repo_root,
                    preflight_path: &fixture.preflight_path,
                    authorization_path: &path,
                    output_root: &fixture.output_root,
                    runner: &runner(RUNNER_BYTES),
                    issued_at_ms: ISSUED_AT_MS,
                    expires_at_ms,
                    nonce: &digest("bad ttl nonce"),
                    credential: &fixture.config.api_key,
                }
            )
            .is_err());
        }
        assert!(validate_authorization(
            protocol,
            &fixture,
            &runner(RUNNER_BYTES),
            &fixture.config.api_key,
            ISSUED_AT_MS - 1,
        )
        .is_err());
        assert!(validate_authorization(
            protocol,
            &fixture,
            &runner(RUNNER_BYTES),
            &fixture.config.api_key,
            EXPIRES_AT_MS,
        )
        .is_err());
    });
}

#[test]
fn agent_delivery_verification_execution_contract_authorization_rejects_drift_and_rehashed_preflight_tamper(
) {
    with_fixture(|protocol, fixture| {
        assert!(validate_authorization(
            protocol,
            &fixture,
            &runner(b"changed runner bytes"),
            &fixture.config.api_key,
            CONSUMED_AT_MS,
        )
        .is_err());
        assert!(validate_authorization(
            protocol,
            &fixture,
            &runner(RUNNER_BYTES),
            "changed credential",
            CONSUMED_AT_MS,
        )
        .is_err());

        let mut preflight = fixture.preflight.clone();
        preflight.online_runner_frozen = false;
        preflight.receipt_sha256 = frozen_preflight_digest(&preflight).unwrap();
        let preflight_path = fixture._temp.path().join("tampered-preflight.json");
        write_new_private_file(
            &preflight_path,
            &encode_preflight(&preflight),
            "tampered test preflight",
        )
        .unwrap();
        assert!(issue_delivery_verification_authorization(
            DeliveryVerificationAuthorizationIssue {
                protocol,
                preflight: &preflight,
                repo_root: &fixture.repo_root,
                preflight_path: &preflight_path,
                authorization_path: &fixture._temp.path().join("tampered-authorization.json"),
                output_root: &fixture.output_root,
                runner: &runner(RUNNER_BYTES),
                issued_at_ms: ISSUED_AT_MS,
                expires_at_ms: EXPIRES_AT_MS,
                nonce: &digest("tampered preflight nonce"),
                credential: &fixture.config.api_key,
            }
        )
        .is_err());
    });
}

#[test]
fn agent_delivery_verification_execution_contract_authorization_is_private_canonical_no_clobber_no_symlink(
) {
    with_fixture(|protocol, fixture| {
        assert_eq!(
            fs::read(&fixture.authorization_path).unwrap(),
            canonical_json(&fixture.authorization, "test authorization").unwrap()
        );
        assert!(write_delivery_verification_authorization_new(
            &fixture.repo_root,
            &fixture.authorization_path,
            &fixture.authorization,
        )
        .is_err());
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&fixture.authorization_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            fs::set_permissions(
                &fixture.authorization_path,
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            assert!(validate_authorization(
                protocol,
                &fixture,
                &runner(RUNNER_BYTES),
                &fixture.config.api_key,
                CONSUMED_AT_MS,
            )
            .is_err());
            fs::remove_file(&fixture.authorization_path).unwrap();
            symlink(&fixture.preflight_path, &fixture.authorization_path).unwrap();
            assert!(validate_authorization(
                protocol,
                &fixture,
                &runner(RUNNER_BYTES),
                &fixture.config.api_key,
                CONSUMED_AT_MS,
            )
            .is_err());
        }
    });
}

#[test]
fn agent_delivery_verification_execution_contract_authorization_consumes_once_atomically() {
    with_fixture(|protocol, fixture| {
        let second_authorization_path = fixture._temp.path().join("authorization-second.json");
        let second_authorization =
            issue_delivery_verification_authorization(DeliveryVerificationAuthorizationIssue {
                protocol,
                preflight: &fixture.preflight,
                repo_root: &fixture.repo_root,
                preflight_path: &fixture.preflight_path,
                authorization_path: &second_authorization_path,
                output_root: &fixture.output_root,
                runner: &runner(RUNNER_BYTES),
                issued_at_ms: ISSUED_AT_MS,
                expires_at_ms: EXPIRES_AT_MS,
                nonce: &digest("second authorization nonce"),
                credential: &fixture.config.api_key,
            })
            .unwrap();
        write_delivery_verification_authorization_new(
            &fixture.repo_root,
            &second_authorization_path,
            &second_authorization,
        )
        .unwrap();
        let second_validated = load_and_validate_delivery_verification_authorization(
            DeliveryVerificationAuthorizationValidation {
                protocol,
                preflight: &fixture.preflight,
                repo_root: &fixture.repo_root,
                preflight_path: &fixture.preflight_path,
                authorization_path: &second_authorization_path,
                output_root: &fixture.output_root,
                runner: &runner(RUNNER_BYTES),
                credential: &fixture.config.api_key,
                now_ms: CONSUMED_AT_MS,
            },
        )
        .unwrap();
        let validated = Arc::new(
            validate_authorization(
                protocol,
                &fixture,
                &runner(RUNNER_BYTES),
                &fixture.config.api_key,
                CONSUMED_AT_MS,
            )
            .unwrap(),
        );
        let barrier = Arc::new(Barrier::new(2));
        let attempts = (0..2)
            .map(|_| {
                let validated = Arc::clone(&validated);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    consume_delivery_verification_authorization_once(&validated, CONSUMED_AT_MS)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|attempt| attempt.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(attempts.iter().filter(|attempt| attempt.is_ok()).count(), 1);
        let tombstone = attempts.into_iter().find_map(Result::ok).unwrap();
        tombstone.validate_for(&fixture.authorization).unwrap();
        let consumed_authority = consumed_authority_path(&fixture.output_root).unwrap();
        assert_eq!(
            fs::read(&consumed_authority).unwrap(),
            canonical_json(&tombstone, "test consumed authority marker").unwrap()
        );
        let relocated_output = fixture._temp.path().join("relocated-execution");
        fs::rename(&fixture.output_root, &relocated_output).unwrap();
        assert!(load_and_validate_delivery_verification_authorization(
            DeliveryVerificationAuthorizationValidation {
                protocol,
                preflight: &fixture.preflight,
                repo_root: &fixture.repo_root,
                preflight_path: &fixture.preflight_path,
                authorization_path: &second_authorization_path,
                output_root: &fixture.output_root,
                runner: &runner(RUNNER_BYTES),
                credential: &fixture.config.api_key,
                now_ms: CONSUMED_AT_MS,
            },
        )
        .is_err());
        assert!(consume_delivery_verification_authorization_once(
            &second_validated,
            CONSUMED_AT_MS,
        )
        .is_err());
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&relocated_output)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(relocated_output.join(DELIVERY_TOMBSTONE_FILE_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(consumed_authority)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    });
}

#[test]
fn agent_delivery_verification_execution_contract_journal_reserves_campaign_before_case_or_call() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        assert!(journal.reserve_case(1, CAMPAIGN_AT_MS + 1).is_err());
        assert!(journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[0]))
            .is_err());
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        assert!(journal.reserve_campaign(CAMPAIGN_AT_MS + 1).is_err());
        assert_eq!(journal.charged().logical_model_calls, 0);
        assert_eq!(
            journal_json(&fixture.output_root)["schema"],
            DELIVERY_EXECUTION_JOURNAL_SCHEMA
        );
        #[cfg(unix)]
        for path in [
            fixture
                .output_root
                .join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME),
            fixture.output_root.join(DELIVERY_EXECUTION_LOCK_FILE_NAME),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    });
}

#[test]
fn agent_delivery_verification_execution_contract_call_reservation_charges_before_dispatch() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let input = reservation_input(&validated.authorization, 1, STAGES[0]);
        let semantic_request_sha256 = input.semantic_request_sha256.clone();
        let semantic_request_bytes = input.semantic_request_bytes;
        let wire_payload_sha256 = input.wire_payload_sha256.clone();
        let wire_payload_bytes = input.wire_payload_bytes;
        let reserved_output = input.max_output_tokens;
        assert_ne!(semantic_request_sha256, wire_payload_sha256);
        assert_ne!(semantic_request_bytes, wire_payload_bytes);
        let _permit = journal.reserve_call(input).unwrap();
        assert_eq!(
            journal.charged(),
            DeliveryVerificationChargedResourcesV1 {
                logical_model_calls: 1,
                physical_model_attempts: 1,
                reserved_output_tokens: reserved_output,
            }
        );
        let value = journal_json(&fixture.output_root);
        assert_eq!(
            value["schema"],
            "cindx.agent-eval.delivery-verification-execution-journal.v2"
        );
        assert_eq!(value["charged"]["physical_model_attempts"], 1);
        assert_eq!(value["cases"][0]["calls"][0]["state"]["state"], "reserved");
        let reservation = &value["cases"][0]["calls"][0]["state"]["reservation"];
        assert_eq!(
            reservation["semantic_request_sha256"],
            semantic_request_sha256
        );
        assert_eq!(
            reservation["semantic_request_bytes"],
            semantic_request_bytes
        );
        assert_eq!(reservation["wire_payload_sha256"], wire_payload_sha256);
        assert_eq!(reservation["wire_payload_bytes"], wire_payload_bytes);
        assert!(reservation.get("canonical_request_sha256").is_none());
        assert!(reservation.get("canonical_request_bytes").is_none());
    });
}

#[test]
fn agent_delivery_verification_execution_contract_call_reservation_rejects_wrong_authority_and_order(
) {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();

        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.role = ModelRole::Reviewer;
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.configured_model_sha256 = digest("wrong-model");
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.max_output_tokens += 1;
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.semantic_request_bytes = 512 * 1024 + 1;
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.semantic_request_sha256 = "not-a-sha256".into();
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.wire_payload_bytes = 0;
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.wire_payload_bytes = 512 * 1024 + 1;
        assert!(journal.reserve_call(wrong).is_err());
        let mut wrong = reservation_input(&validated.authorization, 1, STAGES[0]);
        wrong.wire_payload_sha256 = "not-a-sha256".into();
        assert!(journal.reserve_call(wrong).is_err());
        assert!(journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[1]))
            .is_err());
        assert_eq!(journal.charged().logical_model_calls, 0);
        journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[0]))
            .unwrap();
        assert_eq!(journal.charged().physical_model_attempts, 1);
    });
}

#[test]
fn agent_delivery_verification_execution_contract_completed_call_retains_exact_artifact_usage_and_latency(
) {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let artifact = complete_stage(&mut journal, &validated.authorization, 1, STAGES[0], 11);
        assert_eq!(journal.observed().terminal_model_calls, 1);
        assert_eq!(journal.observed().latency_ms, 7);
        assert_eq!(journal.observed().total_tokens, 11);
        let receipt =
            &journal_json(&fixture.output_root)["cases"][0]["calls"][0]["state"]["receipt"];
        let reservation =
            &journal_json(&fixture.output_root)["cases"][0]["calls"][0]["state"]["reservation"];
        assert_eq!(receipt["provider_receipt_status"], "observed");
        assert_eq!(receipt["usage"]["usage_source"], "provider");
        assert_eq!(receipt["usage"]["usage_estimated"], false);
        assert_eq!(receipt["response_artifact_sha256"], artifact.sha256);
        assert_eq!(
            receipt["request_payload_sha256"],
            reservation["wire_payload_sha256"]
        );
        assert_ne!(
            receipt["request_payload_sha256"],
            reservation["semantic_request_sha256"]
        );
        assert!(!fs::read(&artifact.path).unwrap().is_empty());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(artifact.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    });
}

#[test]
fn agent_delivery_verification_execution_contract_invalid_completed_receipt_writes_no_artifact_or_terminal(
) {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let input = reservation_input(&validated.authorization, 1, STAGES[0]);
        let semantic_request_sha256 = input.semantic_request_sha256.clone();
        let configured_model_sha256 = input.configured_model_sha256.clone();
        let terminal_at_ms = input.reserved_at_ms + 1;
        let permit = journal.reserve_call(input).unwrap();
        let response = b"response rejected before artifact persistence";
        let artifact_path = fixture.output_root.join("case-01-call-001-response.bin");

        let error = journal
            .record_call_terminal(
                DeliveryVerificationCallTerminalInputV1 {
                    permit,
                    status: DeliveryVerificationCallTerminalStatusV1::Completed,
                    failure_class: None,
                    retryable: None,
                    provider_status_code: Some(200),
                    latency_ms: 7,
                    request_payload_sha256: Some(semantic_request_sha256),
                    response_semantic_sha256: Some(digest(response)),
                    provider_response_id_sha256: Some(digest("response-id")),
                    provider_response_model_sha256: Some(configured_model_sha256),
                    provider_system_fingerprint_sha256: Some(digest("system-fingerprint")),
                    provider_receipt_status: Some("observed".into()),
                    usage: Some(DeliveryVerificationCallUsageV1 {
                        prompt_tokens: 1,
                        completion_tokens: 1,
                        total_tokens: 2,
                        usage_source: "provider".into(),
                        usage_estimated: false,
                    }),
                    terminal_at_ms,
                },
                Some(response),
            )
            .unwrap_err();

        assert_eq!(
            error,
            "completed delivery call lacks exact provider receipts"
        );
        assert!(!artifact_path.exists());
        assert_eq!(journal.observed().terminal_model_calls, 0);
        let value = journal_json(&fixture.output_root);
        assert_eq!(value["cases"][0]["calls"][0]["state"]["state"], "reserved");
        assert!(value["cases"][0]["calls"][0]["state"]
            .get("receipt")
            .is_none());
    });
}

#[test]
fn agent_delivery_verification_execution_contract_failed_call_is_retained_and_never_retried() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let input = reservation_input(&validated.authorization, 1, STAGES[0]);
        let terminal_at_ms =
            CAMPAIGN_AT_MS + validated.authorization.budget.campaign_timeout_ms + 1;
        let permit = journal.reserve_call(input).unwrap();
        journal
            .record_call_terminal(
                DeliveryVerificationCallTerminalInputV1 {
                    permit,
                    status: DeliveryVerificationCallTerminalStatusV1::ProviderFailure,
                    failure_class: Some("transport_timeout".into()),
                    retryable: Some(true),
                    provider_status_code: None,
                    latency_ms: 9,
                    request_payload_sha256: None,
                    response_semantic_sha256: None,
                    provider_response_id_sha256: None,
                    provider_response_model_sha256: None,
                    provider_system_fingerprint_sha256: None,
                    provider_receipt_status: None,
                    usage: None,
                    terminal_at_ms,
                },
                None,
            )
            .unwrap();
        assert!(journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[0]))
            .is_err());
        assert!(journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[1]))
            .is_err());
        let receipt =
            &journal_json(&fixture.output_root)["cases"][0]["calls"][0]["state"]["receipt"];
        assert_eq!(receipt["failure_class"], "transport_timeout");
        assert_eq!(receipt["retryable"], true);
        assert_eq!(journal.charged().physical_model_attempts, 1);
        let value = journal_json(&fixture.output_root);
        assert_eq!(value["terminal"]["disposition"], "inconclusive");
        assert_eq!(value["terminal"]["reason"], "campaign_timeout_exceeded");
    });
}

#[test]
fn agent_delivery_verification_execution_contract_initial_pass_closes_optional_call_slots() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let owner = complete_stage(&mut journal, &validated.authorization, 1, STAGES[0], 2);
        complete_stage(&mut journal, &validated.authorization, 1, STAGES[1], 2);
        let mut arbitrary = complete_case_input(1, &owner, &owner, CAMPAIGN_AT_MS + 1_001);
        arbitrary.owner_draft_sha256 = Some(digest("arbitrary owner digest"));
        arbitrary.control_output_sha256 = arbitrary.owner_draft_sha256.clone();
        assert!(journal.record_case_terminal(arbitrary).is_err());
        let late = complete_case_input(
            1,
            &owner,
            &owner,
            CAMPAIGN_AT_MS + validated.authorization.budget.campaign_timeout_ms + 1,
        );
        assert!(journal.record_case_terminal(late).is_err());
        terminal_complete_case(&mut journal, 1, &owner, &owner);
        let value = journal_json(&fixture.output_root);
        assert_eq!(
            value["cases"][0]["calls"][2]["state"]["state"],
            "not_required"
        );
        assert_eq!(
            value["cases"][0]["calls"][3]["state"]["state"],
            "not_required"
        );
        assert_eq!(
            value["cases"][0]["state"]["receipt"]["resources"]["terminal_model_calls"],
            2
        );
        assert_eq!(journal.observed().total_tokens, 4);
    });
}

#[test]
fn agent_delivery_verification_execution_contract_repair_path_records_four_distinct_calls() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let owner = complete_stage(&mut journal, &validated.authorization, 1, STAGES[0], 2);
        complete_stage(&mut journal, &validated.authorization, 1, STAGES[1], 2);
        let repair = complete_stage(&mut journal, &validated.authorization, 1, STAGES[2], 2);
        assert!(journal
            .record_case_terminal(complete_case_input(
                1,
                &owner,
                &repair,
                CAMPAIGN_AT_MS + 1_001,
            ))
            .is_err());
        complete_stage(&mut journal, &validated.authorization, 1, STAGES[3], 2);
        let mut arbitrary = complete_case_input(1, &owner, &repair, CAMPAIGN_AT_MS + 1_001);
        arbitrary.treatment_output_sha256 = Some(digest("arbitrary treatment digest"));
        assert!(journal.record_case_terminal(arbitrary).is_err());
        terminal_complete_case(&mut journal, 1, &owner, &repair);
        assert_eq!(journal.charged().logical_model_calls, 4);
        assert_eq!(journal.observed().terminal_model_calls, 4);
        assert_eq!(journal.observed().total_tokens, 8);
        assert!(journal_json(&fixture.output_root)["cases"][0]["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call["state"]["state"] == "terminal"));
    });
}

#[test]
fn agent_delivery_verification_execution_contract_resource_overflow_terminalizes_without_new_call()
{
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        complete_stage(
            &mut journal,
            &validated.authorization,
            1,
            STAGES[0],
            validated.authorization.budget.max_total_tokens_per_case + 1,
        );
        assert!(journal.is_terminal());
        assert!(journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[1]))
            .is_err());
        let value = journal_json(&fixture.output_root);
        assert_eq!(value["resource_limit_exceeded"], true);
        assert_eq!(value["terminal"]["disposition"], "inconclusive");
    });
}

#[test]
fn agent_delivery_verification_execution_contract_calibration_gate_blocks_and_closes_holdout() {
    with_fixture(|protocol, fixture| {
        let (_, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        for ordinal in 1..=8 {
            journal
                .reserve_case(ordinal, CAMPAIGN_AT_MS + ordinal as u64)
                .unwrap();
            terminal_structural_case(&mut journal, ordinal);
        }
        assert!(journal.reserve_case(9, CAMPAIGN_AT_MS + 9).is_err());
        let deadline = CAMPAIGN_AT_MS + fixture.authorization.budget.campaign_timeout_ms;
        assert!(journal
            .record_calibration_decision(
                "terminal_futility",
                digest("late calibration counts"),
                deadline + 1,
            )
            .is_err());
        journal
            .record_calibration_decision(
                "terminal_futility",
                digest("calibration counts"),
                CAMPAIGN_AT_MS + 2_000,
            )
            .unwrap();
        assert!(journal
            .finish(
                DeliveryVerificationCampaignDispositionV1::TerminalFutility,
                "late calibration gate",
                digest("late calibration evidence"),
                deadline + 1,
            )
            .is_err());
        journal
            .finish(
                DeliveryVerificationCampaignDispositionV1::TerminalFutility,
                "calibration gate closed",
                digest("calibration evidence"),
                CAMPAIGN_AT_MS + 2_001,
            )
            .unwrap();
        let value = journal_json(&fixture.output_root);
        assert_eq!(value["terminal"]["disposition"], "terminal_futility");
        assert!(value["cases"].as_array().unwrap()[8..]
            .iter()
            .all(|case| case["state"]["state"] == "skipped"));
    });
}

#[test]
fn agent_delivery_verification_execution_contract_fixed_campaign_records_exactly_128_calls() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        for ordinal in 1..=32 {
            journal
                .reserve_case(ordinal, CAMPAIGN_AT_MS + ordinal as u64)
                .unwrap();
            let owner = complete_stage(
                &mut journal,
                &validated.authorization,
                ordinal,
                STAGES[0],
                2,
            );
            complete_stage(
                &mut journal,
                &validated.authorization,
                ordinal,
                STAGES[1],
                2,
            );
            let repair = complete_stage(
                &mut journal,
                &validated.authorization,
                ordinal,
                STAGES[2],
                2,
            );
            complete_stage(
                &mut journal,
                &validated.authorization,
                ordinal,
                STAGES[3],
                2,
            );
            terminal_complete_case(&mut journal, ordinal, &owner, &repair);
            if ordinal == 8 {
                journal
                    .record_calibration_decision(
                        "open_holdout",
                        digest("open holdout counts"),
                        CAMPAIGN_AT_MS + 2_000,
                    )
                    .unwrap();
            }
        }
        journal
            .record_holdout_decision(
                "no_evidence",
                digest("holdout counts"),
                CAMPAIGN_AT_MS + 3_000,
            )
            .unwrap();
        journal
            .finish(
                DeliveryVerificationCampaignDispositionV1::NoEvidence,
                "frozen holdout decision",
                digest("holdout evidence"),
                CAMPAIGN_AT_MS + 3_001,
            )
            .unwrap();
        assert_eq!(journal.charged().logical_model_calls, 128);
        assert_eq!(journal.charged().physical_model_attempts, 128);
        assert_eq!(journal.observed().terminal_model_calls, 128);
        assert_eq!(
            journal_json(&fixture.output_root)["terminal"]["disposition"],
            "no_evidence"
        );
    });
}

#[test]
fn agent_delivery_verification_execution_contract_recovery_terminalizes_tombstone_and_pending_call()
{
    with_fixture(|protocol, fixture| {
        let validated = validate_authorization(
            protocol,
            &fixture,
            &runner(RUNNER_BYTES),
            &fixture.config.api_key,
            CONSUMED_AT_MS,
        )
        .unwrap();
        consume_delivery_verification_authorization_once(&validated, CONSUMED_AT_MS).unwrap();
        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(&fixture.output_root, CAMPAIGN_AT_MS,)
                .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Censored
            )
        );
        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(
                &fixture.output_root,
                CAMPAIGN_AT_MS + 1,
            )
            .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Censored
            )
        );
    });

    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let _permit = journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[0]))
            .unwrap();
        let deadline = CAMPAIGN_AT_MS + validated.authorization.budget.campaign_timeout_ms;
        drop(journal);
        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(&fixture.output_root, deadline + 1,)
                .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Censored
            )
        );
        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(&fixture.output_root, deadline + 2,)
                .unwrap(),
            DeliveryVerificationRecoveryV1::Terminal(
                DeliveryVerificationCampaignDispositionV1::Censored
            )
        );
    });
}

#[test]
fn agent_delivery_verification_execution_contract_recovery_rejects_legacy_v1_journal_shape() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        journal
            .reserve_call(reservation_input(&validated.authorization, 1, STAGES[0]))
            .unwrap();

        let mut legacy = journal_json(&fixture.output_root);
        legacy["schema"] =
            Value::String("cindx.agent-eval.delivery-verification-execution-journal.v1".into());
        let reservation = legacy["cases"][0]["calls"][0]["state"]["reservation"]
            .as_object_mut()
            .unwrap();
        let semantic_sha256 = reservation.remove("semantic_request_sha256").unwrap();
        let semantic_bytes = reservation.remove("semantic_request_bytes").unwrap();
        reservation.remove("wire_payload_sha256").unwrap();
        reservation.remove("wire_payload_bytes").unwrap();
        reservation.insert("canonical_request_sha256".into(), semantic_sha256);
        reservation.insert("canonical_request_bytes".into(), semantic_bytes);
        replace_private_file(
            &fixture
                .output_root
                .join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME),
            &canonical_json(&legacy, "legacy delivery execution journal").unwrap(),
            "legacy delivery execution journal",
        )
        .unwrap();
        drop(journal);

        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(
                &fixture.output_root,
                CAMPAIGN_AT_MS + 1_000,
            )
            .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Invalid
            )
        );
    });
}

#[test]
fn agent_delivery_verification_execution_contract_recovery_rejects_tampered_response_artifact() {
    with_fixture(|protocol, fixture| {
        let (validated, _, mut journal) = new_journal(protocol, &fixture);
        journal.reserve_campaign(CAMPAIGN_AT_MS).unwrap();
        journal.reserve_case(1, CAMPAIGN_AT_MS + 1).unwrap();
        let artifact = complete_stage(&mut journal, &validated.authorization, 1, STAGES[0], 2);
        replace_private_file(
            &artifact.path,
            b"tampered response artifact",
            "tampered delivery response artifact",
        )
        .unwrap();
        drop(journal);

        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(
                &fixture.output_root,
                CAMPAIGN_AT_MS + 1_000,
            )
            .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Invalid
            )
        );
    });
}

#[test]
fn agent_delivery_verification_execution_contract_recovery_rejects_lock_mode_and_symlink_tamper() {
    with_fixture(|protocol, fixture| {
        let (_, _, journal) = new_journal(protocol, &fixture);
        assert!(DeliveryVerificationExecutionJournal::recover(
            &fixture.output_root,
            CAMPAIGN_AT_MS,
        )
        .is_err());
        drop(journal);
        #[cfg(unix)]
        fs::set_permissions(
            fixture
                .output_root
                .join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(
                &fixture.output_root,
                CAMPAIGN_AT_MS + 1,
            )
            .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Invalid
            )
        );
    });

    #[cfg(unix)]
    with_fixture(|protocol, fixture| {
        let (_, _, journal) = new_journal(protocol, &fixture);
        drop(journal);
        let journal_path = fixture
            .output_root
            .join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME);
        fs::remove_file(&journal_path).unwrap();
        symlink(&fixture.preflight_path, &journal_path).unwrap();
        assert_eq!(
            DeliveryVerificationExecutionJournal::recover(
                &fixture.output_root,
                CAMPAIGN_AT_MS + 2,
            )
            .unwrap(),
            DeliveryVerificationRecoveryV1::RecoveryTerminal(
                DeliveryVerificationCampaignDispositionV1::Invalid
            )
        );
        assert_eq!(
            fs::metadata(
                fixture
                    .output_root
                    .join(DELIVERY_EXECUTION_RECOVERY_FILE_NAME)
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    });
}
