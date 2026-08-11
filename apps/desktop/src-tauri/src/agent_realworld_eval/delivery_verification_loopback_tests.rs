use super::*;
use crate::agent_realworld_eval::delivery_verification_preflight::{
    build_receipt, DeliveryVerificationSourceBindingReceipt,
};
use crate::agent_realworld_eval::delivery_verification_requests::{
    prepare_owner_draft_request, DeliveryOwnerDraftRequestInput, DeliveryVerificationRequestBudget,
    MAX_DELIVERY_VERIFICATION_REQUEST_BYTES,
};
use serde_json::{json, Value};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const OWNER_MODEL: &str = "loopback-owner-model";
const VERIFIER_MODEL: &str = "loopback-verifier-model";
const WRONG_SERVED_MODEL: &str = "loopback-wrong-served-model";
const RUNNER_BYTES: &[u8] = b"provider-free delivery loopback runner fixture";
const RESPONSE_CONTENT: &str = "loopback response";
const REQUEST_PAYLOAD_DOMAIN: &[u8] = b"cindx.model-provider.request-payload.v1\0";

fn runner() -> DeliveryVerificationRunnerBinary {
    DeliveryVerificationRunnerBinary {
        bytes: RUNNER_BYTES.to_vec(),
        code_directory_sha256: sha256_hex(b"loopback runner code directory"),
    }
}

struct LoopbackAcceptedRequest {
    body: Vec<u8>,
    journal_at_accept: Value,
}

struct LoopbackRun {
    outcome: CallOutcome,
    record_case_result: Option<Result<(), String>>,
    accepted: LoopbackAcceptedRequest,
    final_journal: Value,
    semantic_request_sha256: String,
    semantic_request_bytes: u64,
    prepared_wire_sha256: String,
    prepared_wire_bytes: u64,
}

fn protocol() -> ValidatedProtocol<'static> {
    parse_and_validate_protocol(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-protocol-v2.json"
        )),
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../benchmarks/agent/delivery-verification-v1.json"
        )),
    )
    .unwrap()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
}

fn source() -> DeliveryVerificationSourceBindingReceipt {
    DeliveryVerificationSourceBindingReceipt {
        head: "0123456789abcdef0123456789abcdef01234567".into(),
        tree: "89abcdef0123456789abcdef0123456789abcdef".into(),
    }
}

fn provider_config(base_url: String) -> ProviderConfig {
    ProviderConfig {
        provider_id: "openai-compatible".into(),
        provider_resource: "delivery-loopback".into(),
        base_url,
        api_key: "loopback-secret-never-serialized".into(),
        executor_model: OWNER_MODEL.into(),
        reviewer_model: VERIFIER_MODEL.into(),
        context_window_tokens: 131_072,
        ..ProviderConfig::default()
    }
}

fn encode_preflight(receipt: &DeliveryVerificationPreflightReceipt) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(receipt).unwrap();
    bytes.push(b'\n');
    bytes
}

fn create_journal(
    protocol: &ValidatedProtocol<'_>,
    config: &ProviderConfig,
) -> (TempDir, PathBuf, DeliveryVerificationExecutionJournal) {
    let temp = tempfile::tempdir().unwrap();
    let temp_root = temp.path().canonicalize().unwrap();
    let preflight_path = temp_root.join("preflight.json");
    let authorization_path = temp_root.join("authorization.json");
    let output_root = temp_root.join("execution");
    let repo_root = repo_root();
    let issued_at_ms = now_millis().unwrap();
    let preflight = build_receipt(
        &protocol_snapshot(protocol),
        source(),
        provider_binding(config).unwrap(),
        &runner(),
        &output_root,
        issued_at_ms,
    )
    .unwrap();
    write_new_private_file(
        &preflight_path,
        &encode_preflight(&preflight),
        "loopback preflight",
    )
    .unwrap();
    let authorization =
        issue_delivery_verification_authorization(DeliveryVerificationAuthorizationIssue {
            protocol,
            preflight: &preflight,
            repo_root: &repo_root,
            preflight_path: &preflight_path,
            authorization_path: &authorization_path,
            output_root: &output_root,
            runner: &runner(),
            issued_at_ms,
            expires_at_ms: issued_at_ms + DELIVERY_AUTHORIZATION_TTL_MS,
            nonce: &sha256_hex(b"delivery loopback authorization nonce"),
            credential: &config.api_key,
        })
        .unwrap();
    write_delivery_verification_authorization_new(&repo_root, &authorization_path, &authorization)
        .unwrap();
    let consumed_at_ms = now_millis().unwrap();
    let validated = load_and_validate_delivery_verification_authorization(
        DeliveryVerificationAuthorizationValidation {
            protocol,
            preflight: &preflight,
            repo_root: &repo_root,
            preflight_path: &preflight_path,
            authorization_path: &authorization_path,
            output_root: &output_root,
            runner: &runner(),
            credential: &config.api_key,
            now_ms: consumed_at_ms,
        },
    )
    .unwrap();
    let tombstone =
        consume_delivery_verification_authorization_once(&validated, consumed_at_ms).unwrap();
    let mut journal =
        DeliveryVerificationExecutionJournal::create_new(&output_root, &validated, &tombstone)
            .unwrap();
    journal.reserve_campaign(now_millis().unwrap()).unwrap();
    (temp, output_root, journal)
}

fn owner_call(protocol: &ValidatedProtocol<'_>) -> PreparedDeliveryCall {
    let case = protocol.calibration_cases().next().unwrap();
    let prepared = prepare_owner_draft_request(DeliveryOwnerDraftRequestInput {
        objective: case.objective(),
        obligations: case.obligations(),
        evidence: case.evidence(),
        owner_model: OWNER_MODEL,
        budget: DeliveryVerificationRequestBudget {
            max_request_bytes: MAX_DELIVERY_VERIFICATION_REQUEST_BYTES,
            max_output_tokens: protocol.budget().max_owner_output_tokens,
        },
    })
    .unwrap();
    let binding = prepared.binding().clone();
    PreparedDeliveryCall {
        case_ordinal: case.ordinal(),
        stage: DeliveryVerificationCallStageV1::OwnerDraft,
        role: ModelRole::Executor,
        configured_model: OWNER_MODEL.into(),
        canonical_request_sha256: binding.canonical_request_sha256,
        canonical_request_bytes: binding.canonical_request_bytes,
        max_output_tokens: protocol.budget().max_owner_output_tokens,
        request: prepared.into_request(),
    }
}

fn request_payload_sha256(body: &[u8]) -> String {
    let mut bound = Vec::with_capacity(REQUEST_PAYLOAD_DOMAIN.len() + body.len());
    bound.extend_from_slice(REQUEST_PAYLOAD_DOMAIN);
    bound.extend_from_slice(body);
    sha256_hex(&bound)
}

fn journal_json(output_root: &Path) -> Result<Value, String> {
    let path = output_root.join(DELIVERY_EXECUTION_JOURNAL_FILE_NAME);
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid loopback journal: {error}"))
}

fn read_http_body(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("failed to set loopback read timeout: {error}"))?;
    let mut received = Vec::new();
    let mut chunk = [0_u8; 4_096];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| format!("failed to read loopback request headers: {error}"))?;
        if read == 0 {
            return Err("loopback request ended before its headers".into());
        }
        received.extend_from_slice(&chunk[..read]);
        if received.len() > 64 * 1024 {
            return Err("loopback request headers exceed their test bound".into());
        }
        if let Some(index) = received.windows(4).position(|value| value == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&received[..header_end])
        .map_err(|error| format!("loopback request headers are not UTF-8: {error}"))?;
    let content_length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .ok_or_else(|| "loopback request lacks a valid Content-Length".to_string())?;
    if content_length > 1024 * 1024 {
        return Err("loopback request body exceeds its test bound".into());
    }
    let total = header_end
        .checked_add(content_length)
        .ok_or_else(|| "loopback request length overflowed".to_string())?;
    while received.len() < total {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| format!("failed to read loopback request body: {error}"))?;
        if read == 0 {
            return Err("loopback request ended before its full body".into());
        }
        received.extend_from_slice(&chunk[..read]);
    }
    Ok(received[header_end..total].to_vec())
}

fn accept_with_deadline(listener: &TcpListener) -> Result<TcpStream, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("failed to configure loopback listener: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                return Err("loopback listener did not receive a request".into())
            }
            Err(error) => return Err(format!("failed to accept loopback request: {error}")),
        }
    }
}

fn spawn_loopback_server(
    listener: TcpListener,
    output_root: PathBuf,
    served_model: String,
) -> JoinHandle<Result<LoopbackAcceptedRequest, String>> {
    thread::spawn(move || {
        let mut stream = accept_with_deadline(&listener)?;
        let body = read_http_body(&mut stream);
        let journal_at_accept = journal_json(&output_root);
        let response_body = json!({
            "id": "loopback-response-1",
            "model": served_model,
            "system_fingerprint": "loopback-fingerprint",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": RESPONSE_CONTENT},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 7, "completion_tokens": 2, "total_tokens": 9}
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream
            .write_all(response.as_bytes())
            .map_err(|error| format!("failed to write loopback response: {error}"))?;
        stream
            .flush()
            .map_err(|error| format!("failed to flush loopback response: {error}"))?;
        Ok(LoopbackAcceptedRequest {
            body: body?,
            journal_at_accept: journal_at_accept?,
        })
    })
}

fn run_loopback(served_model: &str, record_structural_case: bool) -> LoopbackRun {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let protocol = protocol();
    let config = provider_config(base_url);
    let (_temp, output_root, mut journal) = create_journal(&protocol, &config);
    let call = owner_call(&protocol);
    let semantic_request_sha256 = call.canonical_request_sha256.clone();
    let semantic_request_bytes = call.canonical_request_bytes;
    let prepared_provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: config.base_url.clone(),
        api_key: config.api_key.clone(),
        model: call.configured_model.clone(),
        embedding_model: config.embedding_model.clone(),
        timeout_seconds: protocol
            .budget()
            .model_call_timeout_ms
            .div_ceil(1_000)
            .max(1),
    });
    let prepared = prepared_provider
        .prepare_non_streaming_request(&call.request)
        .unwrap();
    let (prepared_wire_sha256, prepared_wire_bytes) = prepared.payload_receipt();
    let prepared_wire_sha256 = prepared_wire_sha256.to_string();
    let prepared_wire_bytes = u64::try_from(prepared_wire_bytes).unwrap();
    let server = spawn_loopback_server(listener, output_root.clone(), served_model.into());

    let mut runtime = JournalRuntime::new(&mut journal, config, protocol.budget().clone());
    runtime.begin_case(1).unwrap();
    let outcome = runtime.dispatch(call);
    let record_case_result = record_structural_case.then(|| {
        let reason = match &outcome {
            CallOutcome::StructuralFailure(error) => error.clone(),
            other => panic!("expected structural failure, got {other:?}"),
        };
        runtime.record_case(&CaseOutcome {
            ordinal: 1,
            status: CaseStatus::StructuralFailure,
            owner_draft_sha256: None,
            owner_draft_bytes: None,
            control_output_sha256: None,
            treatment_output_sha256: None,
            control_passed: None,
            treatment_passed: None,
            observation_sha256: sha256_hex(b"loopback structural observation"),
            reason,
        })
    });
    drop(runtime);
    let accepted = server.join().unwrap().unwrap();
    let final_journal = journal_json(&output_root).unwrap();
    LoopbackRun {
        outcome,
        record_case_result,
        accepted,
        final_journal,
        semantic_request_sha256,
        semantic_request_bytes,
        prepared_wire_sha256,
        prepared_wire_bytes,
    }
}

fn first_call_state(journal: &Value) -> &Value {
    &journal["cases"][0]["calls"][0]["state"]
}

fn assert_exact_reserved_wire(run: &LoopbackRun) {
    let state = first_call_state(&run.accepted.journal_at_accept);
    assert_eq!(state["state"], "reserved");
    let reservation = &state["reservation"];
    let captured_wire_sha256 = request_payload_sha256(&run.accepted.body);
    assert_eq!(
        reservation["semantic_request_sha256"],
        run.semantic_request_sha256
    );
    assert_eq!(
        reservation["semantic_request_bytes"],
        run.semantic_request_bytes
    );
    assert_eq!(reservation["wire_payload_sha256"], run.prepared_wire_sha256);
    assert_eq!(reservation["wire_payload_bytes"], run.prepared_wire_bytes);
    assert_eq!(captured_wire_sha256, run.prepared_wire_sha256);
    assert_eq!(run.accepted.body.len() as u64, run.prepared_wire_bytes);
    assert_ne!(run.semantic_request_sha256, run.prepared_wire_sha256);
    assert_eq!(
        serde_json::from_slice::<Value>(&run.accepted.body).unwrap()["model"],
        OWNER_MODEL
    );
}

#[test]
fn agent_delivery_verification_execution_contract_loopback_dispatches_exact_prepared_wire_after_durable_reservation(
) {
    let run = run_loopback(OWNER_MODEL, false);
    assert_exact_reserved_wire(&run);
    match run.outcome {
        CallOutcome::Completed(completed) => {
            assert_eq!(completed.content, RESPONSE_CONTENT);
            assert_eq!(
                completed.served_model_sha256,
                sha256_hex(OWNER_MODEL.as_bytes())
            );
        }
        other => panic!("expected completed loopback call, got {other:?}"),
    }
    let state = first_call_state(&run.final_journal);
    assert_eq!(state["state"], "terminal");
    assert_eq!(state["receipt"]["status"], "completed");
    assert_eq!(
        state["receipt"]["request_payload_sha256"],
        run.prepared_wire_sha256
    );
    assert_eq!(
        state["reservation"]["wire_payload_sha256"],
        run.prepared_wire_sha256
    );
    assert_eq!(run.final_journal["observed"]["terminal_model_calls"], 1);
}

#[test]
fn agent_delivery_verification_execution_contract_loopback_preserves_primary_terminal_digest_error()
{
    let run = run_loopback(WRONG_SERVED_MODEL, true);
    assert_exact_reserved_wire(&run);
    let primary = match &run.outcome {
        CallOutcome::StructuralFailure(error) => error,
        other => panic!("expected terminal digest failure, got {other:?}"),
    };
    assert_eq!(
        primary,
        "completed delivery call lacks exact provider receipts"
    );
    let record_case_error = run.record_case_result.unwrap().unwrap_err();
    assert_eq!(&record_case_error, primary);
    assert!(!record_case_error.contains("pending or unreserved"));
    let state = first_call_state(&run.final_journal);
    assert_eq!(state["state"], "reserved");
    assert_ne!(
        state["reservation"]["configured_model_sha256"],
        sha256_hex(WRONG_SERVED_MODEL.as_bytes())
    );
}
