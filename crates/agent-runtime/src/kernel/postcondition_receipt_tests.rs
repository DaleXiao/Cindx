use super::*;
use crate::{
    start_agent_loop, AgentRuntimeConfig, AgentTaskStateSnapshot, ContractEvidenceKind,
    OutcomePostconditionKind, PersistedToolEffectWitness, PreparedTaskState,
    PromptCompletionIntent,
};
use agent_core::{
    PostconditionVerifierKind, TaskId, ToolCallId, ToolEffectSemantics, ToolPostconditionEvidence,
};

const POSTCONDITION_SCOPE: &str = "logical-test-run:0";
const WORKSPACE_FILE_CONTENT_VERIFIER: &str = "workspace_file_content_v1";

fn tool_request(call_id: &str, tool_name: &str, input: &str) -> AgentToolRequest {
    AgentToolRequest {
        call_id: ToolCallId(call_id.to_string()),
        tool_name: tool_name.to_string(),
        input: input.to_string(),
    }
}

fn workspace_write_spec(name: &str) -> ToolSpec {
    ToolSpec::builtin(
        name,
        "test",
        "test workspace writer",
        ToolRisk::WritesWorkspace,
        r#"{"type":"object"}"#,
    )
    .with_effect_semantics(ToolEffectSemantics::Verifiable {
        verifier: WORKSPACE_FILE_CONTENT_VERIFIER.to_string(),
    })
}

fn exact_read_spec(name: &str) -> ToolSpec {
    ToolSpec::builtin(
        name,
        "test",
        "test exact reader",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    )
    .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceExactReadbackV1)
}

fn quality_check_spec(name: &str) -> ToolSpec {
    ToolSpec::builtin(
        name,
        "test",
        "test quality check",
        ToolRisk::ExecutesProcess,
        r#"{"type":"object"}"#,
    )
    .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceQualityCheckV1)
}

fn exact_read_evidence(path: &str) -> ToolPostconditionEvidence {
    ToolPostconditionEvidence {
        kind: PostconditionVerifierKind::WorkspaceExactReadbackV1,
        target_input_json: serde_json::json!({ "path": path }).to_string(),
    }
}

fn quality_check_evidence() -> ToolPostconditionEvidence {
    ToolPostconditionEvidence {
        kind: PostconditionVerifierKind::WorkspaceQualityCheckV1,
        target_input_json: serde_json::json!({ "path": "." }).to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_contract_transition(
    state: &mut AgentLoopState,
    request: &AgentToolRequest,
    status: &ToolOutcomeStatus,
    risk: &ToolRisk,
    spec: &ToolSpec,
    evidence: Option<&ToolPostconditionEvidence>,
    observation: &str,
    scope: &str,
) -> AgentToolObservationTransition {
    AgentKernel::new(state, &[])
        .with_postcondition_scope(Some(scope))
        .apply_tool_observation_transition_with_contract(
            request,
            status,
            Some(risk),
            Some(spec),
            evidence,
            observation,
            None,
        )
}

fn write_then_exact_read(
    state: &mut AgentLoopState,
    path: &str,
) -> PostconditionVerificationReceipt {
    let write_spec = workspace_write_spec("file.write");
    let write = tool_request(
        "write",
        "file.write",
        &serde_json::json!({ "path": path }).to_string(),
    );
    let action = apply_contract_transition(
        state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated",
        POSTCONDITION_SCOPE,
    );
    assert!(action.postcondition_verification.is_none());

    let read_spec = exact_read_spec("file.read");
    let read = tool_request(
        "read",
        "file.read",
        &serde_json::json!({ "path": path }).to_string(),
    );
    let evidence = exact_read_evidence(path);
    apply_contract_transition(
        state,
        &read,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &read_spec,
        Some(&evidence),
        "complete file contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .expect("trusted exact readback should mint a receipt")
}

#[test]
fn trusted_exact_readback_emits_a_bounded_verifiable_receipt() {
    let mut state = start_agent_loop(
        TaskId("typed-workspace-receipt".to_string()),
        "update README and verify it",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);

    let receipt = write_then_exact_read(&mut state, "README.md");
    assert_eq!(receipt.kind, OutcomePostconditionKind::WorkspaceMutation);
    assert!(receipt.observation_sequence > receipt.action_sequence);
    assert!(state
        .task_contract
        .verify_postcondition_receipt(&receipt, 0, 0));
    assert!(state.task_contract.latest_mutation_verified());

    let encoded = serde_json::to_string(&receipt).expect("receipt serializes");
    assert!(encoded.len() <= crate::MAX_POSTCONDITION_VERIFICATION_RECEIPT_BYTES);
    assert!(!encoded.contains("README.md"));
    assert!(!encoded.contains("complete file contents"));

    let mut metadata = Metadata::new();
    assert!(receipt.insert_metadata(&mut metadata));
    assert_eq!(
        PostconditionVerificationReceipt::from_metadata(&metadata),
        Some(receipt.clone())
    );
    metadata
        .get_mut(crate::POSTCONDITION_VERIFICATION_METADATA_KEY)
        .expect("receipt metadata exists")
        .push('x');
    assert!(PostconditionVerificationReceipt::from_metadata(&metadata).is_none());
    let mut tampered = receipt;
    tampered.verifier_source = "shell.run".to_string();
    assert!(!state
        .task_contract
        .verify_postcondition_receipt(&tampered, 0, 0));
}

#[test]
fn exact_readback_is_bound_to_the_latest_action_target_and_contract_epoch() {
    let mut state = start_agent_loop(
        TaskId("typed-workspace-target-binding".to_string()),
        "update two files and verify the latest mutation",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write");
    for (call_id, path) in [("write-a", "A.md"), ("write-b", "B.md")] {
        let write = tool_request(
            call_id,
            "file.write",
            &serde_json::json!({ "path": path }).to_string(),
        );
        assert!(apply_contract_transition(
            &mut state,
            &write,
            &ToolOutcomeStatus::Succeeded,
            &ToolRisk::WritesWorkspace,
            &write_spec,
            None,
            "updated",
            POSTCONDITION_SCOPE,
        )
        .postcondition_verification
        .is_none());
    }

    let read_spec = exact_read_spec("file.read");
    let stale_target = tool_request("read-a", "file.read", r#"{"path":"A.md"}"#);
    let stale_evidence = exact_read_evidence("A.md");
    assert!(apply_contract_transition(
        &mut state,
        &stale_target,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &read_spec,
        Some(&stale_evidence),
        "A contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_none());
    assert!(!state.task_contract.latest_mutation_verified());

    let next_context = [
        ("steer_epoch".to_string(), "1".to_string()),
        ("prompt_contract_epoch".to_string(), "1".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    state.replace_prepared_task_state(PreparedTaskState::from_run_context(
        &next_context,
        "new objective",
        PromptCompletionIntent::default(),
    ));
    let latest_target = tool_request("read-b", "file.read", r#"{"path":"B.md"}"#);
    let latest_evidence = exact_read_evidence("B.md");
    assert!(apply_contract_transition(
        &mut state,
        &latest_target,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &read_spec,
        Some(&latest_evidence),
        "B contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_none());
    assert!(!state.task_contract.latest_mutation_verified());
}

#[test]
fn multi_target_mutation_requires_complete_cumulative_coverage() {
    let mut state = start_agent_loop(
        TaskId("typed-multi-target".to_string()),
        "update both files and verify them",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write_many");
    let write = tool_request(
        "write-many",
        "file.write_many",
        r#"{"paths":["A.md","B.md"]}"#,
    );
    apply_contract_transition(
        &mut state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated both",
        POSTCONDITION_SCOPE,
    );

    let read_spec = exact_read_spec("file.read");
    for (index, path) in ["A.md", "B.md"].into_iter().enumerate() {
        let read = tool_request(
            &format!("read-{index}"),
            "file.read",
            &serde_json::json!({ "path": path }).to_string(),
        );
        let evidence = exact_read_evidence(path);
        let transition = apply_contract_transition(
            &mut state,
            &read,
            &ToolOutcomeStatus::Succeeded,
            &ToolRisk::ReadOnly,
            &read_spec,
            Some(&evidence),
            "complete contents",
            POSTCONDITION_SCOPE,
        );
        assert_eq!(transition.postcondition_verification.is_some(), index == 1);
        assert_eq!(state.task_contract.latest_mutation_verified(), index == 1);
    }
}

#[test]
fn untrusted_shell_and_file_list_cannot_verify_but_trusted_quality_check_can() {
    let mut state = start_agent_loop(
        TaskId("typed-trusted-verifiers".to_string()),
        "update and verify",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write");
    let write = tool_request("write", "file.write", r#"{"path":"README.md"}"#);
    apply_contract_transition(
        &mut state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated",
        POSTCONDITION_SCOPE,
    );

    let list_spec = ToolSpec::builtin(
        "file.list",
        "test",
        "list",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    );
    let list = tool_request("list", "file.list", r#"{"path":"."}"#);
    assert!(apply_contract_transition(
        &mut state,
        &list,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &list_spec,
        None,
        "README.md",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_none());

    let shell_spec = quality_check_spec("shell.run");
    let echo = tool_request("echo", "shell.run", r#"{"command":"echo test"}"#);
    assert!(apply_contract_transition(
        &mut state,
        &echo,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ExecutesProcess,
        &shell_spec,
        None,
        "test",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_none());
    assert!(!state.task_contract.latest_mutation_verified());

    let test = tool_request(
        "test",
        "shell.run",
        r#"{"command":"cargo test --quiet","cwd":"."}"#,
    );
    let quality_evidence = quality_check_evidence();
    assert!(apply_contract_transition(
        &mut state,
        &test,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ExecutesProcess,
        &shell_spec,
        Some(&quality_evidence),
        "tests passed",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_some());
    assert!(state.task_contract.latest_mutation_verified());
}

#[test]
fn recovered_signed_shell_evidence_retains_verification_authority() {
    let mut state = start_agent_loop(
        TaskId("typed-recovered-shell-verifier".to_string()),
        "update and recover the verification result",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write");
    let write = tool_request("write", "file.write", r#"{"path":"README.md"}"#);
    apply_contract_transition(
        &mut state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated",
        POSTCONDITION_SCOPE,
    );

    let shell_input = r#"{"command":"cargo test --quiet","cwd":"."}"#;
    let shell_spec = quality_check_spec("shell.run");
    let evidence = quality_check_evidence();
    let encoded = PersistedToolEffectWitness::capture_with_postcondition_evidence(
        "shell.run",
        shell_input,
        Some(&ToolRisk::ExecutesProcess),
        POSTCONDITION_SCOPE,
        Some(&shell_spec),
        Some(&evidence),
    )
    .and_then(|witness| witness.encode())
    .expect("trusted shell evidence should persist");
    let recovered = PersistedToolEffectWitness::decode(&encoded)
        .expect("signed typed shell evidence should recover");
    let fingerprint = crate::tool_input_fingerprint("shell.run", shell_input);

    AgentKernel::new(&mut state, std::slice::from_ref(&shell_spec))
        .apply_persisted_tool_observation(
            ToolCallId("recovered-shell".to_string()),
            "shell.run",
            &fingerprint,
            None,
            Some(&recovered),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ExecutesProcess),
            "tests passed",
        );

    assert!(state.task_contract.latest_mutation_verified());
}

#[test]
fn target_witness_cannot_cross_logical_run_scope() {
    let mut state = start_agent_loop(
        TaskId("typed-scope-isolation".to_string()),
        "update and verify",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write");
    let write = tool_request("write", "file.write", r#"{"path":"A.md"}"#);
    apply_contract_transition(
        &mut state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated",
        "logical-run-a:0",
    );

    let read_spec = exact_read_spec("file.read");
    let read = tool_request("read", "file.read", r#"{"path":"A.md"}"#);
    let evidence = exact_read_evidence("A.md");
    assert!(apply_contract_transition(
        &mut state,
        &read,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &read_spec,
        Some(&evidence),
        "A contents",
        "logical-run-b:0",
    )
    .postcondition_verification
    .is_none());
    assert!(!state.task_contract.latest_mutation_verified());
}

#[test]
fn deferred_tool_uses_its_dynamic_effect_contract_for_verification() {
    let mut state = start_agent_loop(
        TaskId("typed-deferred-contract".to_string()),
        "update and verify through deferred tools",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write = tool_request(
        "deferred-write",
        "tool.invoke",
        r#"{"name":"file.write","arguments":{"path":"A.md"}}"#,
    );
    apply_contract_transition(
        &mut state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &workspace_write_spec("file.write"),
        None,
        "updated",
        POSTCONDITION_SCOPE,
    );

    let read = tool_request(
        "deferred-read",
        "tool.invoke",
        r#"{"name":"file.read","arguments":{"path":"A.md"}}"#,
    );
    let evidence = exact_read_evidence("A.md");
    let receipt = apply_contract_transition(
        &mut state,
        &read,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &exact_read_spec("file.read"),
        Some(&evidence),
        "A contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .expect("trusted deferred target should preserve verifier authority");
    assert_eq!(receipt.verifier_source, "file.read");
    assert!(state.task_contract.latest_mutation_verified());
}

#[test]
fn browser_and_computer_receipts_require_the_matching_surface_verifier() {
    let mut state = start_agent_loop(
        TaskId("typed-interaction-receipt".to_string()),
        "interact and verify",
        AgentRuntimeConfig::default(),
    );
    let browser_action = tool_request("browser-action", "browser.click", "{}");
    AgentKernel::new(&mut state, &[]).apply_tool_observation_transition(
        &browser_action,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::UsesNetwork),
        "clicked",
    );
    let wrong_observer = tool_request("browser-tabs", "browser.tabs", "{}");
    assert!(AgentKernel::new(&mut state, &[])
        .apply_tool_observation_transition(
            &wrong_observer,
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "tab list",
        )
        .postcondition_verification
        .is_none());
    let browser_observer = tool_request("browser-capture", "browser.capture", "{}");
    assert_eq!(
        AgentKernel::new(&mut state, &[])
            .apply_tool_observation_transition(
                &browser_observer,
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "captured page",
            )
            .postcondition_verification
            .map(|receipt| receipt.kind),
        Some(OutcomePostconditionKind::BrowserInteraction)
    );

    let computer_action = tool_request("computer-action", "computer.click", "{}");
    AgentKernel::new(&mut state, &[]).apply_tool_observation_transition(
        &computer_action,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::UsesNetwork),
        "clicked",
    );
    let computer_observer = tool_request("computer-screenshot", "computer.screenshot", "{}");
    assert_eq!(
        AgentKernel::new(&mut state, &[])
            .apply_tool_observation_transition(
                &computer_observer,
                &ToolOutcomeStatus::Succeeded,
                Some(&ToolRisk::ReadOnly),
                "captured desktop",
            )
            .postcondition_verification
            .map(|receipt| receipt.kind),
        Some(OutcomePostconditionKind::ComputerInteraction)
    );
}

#[test]
fn recovered_typed_workspace_action_uses_the_same_scoped_verifier_contract() {
    let mut state = start_agent_loop(
        TaskId("typed-recovered-workspace".to_string()),
        "recover the write and verify it",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_input = r#"{"path":"private/recovered.md","content":"do-not-retain"}"#;
    let input_fingerprint = crate::tool_input_fingerprint("file.write", write_input);
    let effect_witness = PersistedToolEffectWitness::capture(
        "file.write",
        write_input,
        Some(&ToolRisk::WritesWorkspace),
        POSTCONDITION_SCOPE,
    )
    .expect("permission boundary should persist a bounded effect witness");
    let write_spec = workspace_write_spec("file.write");

    AgentKernel::new(&mut state, std::slice::from_ref(&write_spec))
        .with_postcondition_scope(Some(POSTCONDITION_SCOPE))
        .apply_persisted_tool_observation(
            ToolCallId("recovered-write".to_string()),
            "file.write",
            &input_fingerprint,
            None,
            Some(&effect_witness),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
            "recovered write completed",
        );
    assert!(!state.task_contract.latest_mutation_verified());
    let encoded = serde_json::to_string(&state.task_contract).expect("contract serializes");
    assert!(!encoded.contains("private/recovered.md"));
    assert!(!encoded.contains("do-not-retain"));

    let read_spec = exact_read_spec("file.read");
    let live_read = tool_request(
        "live-read",
        "file.read",
        r#"{"path":"private/recovered.md"}"#,
    );
    let evidence = exact_read_evidence("private/recovered.md");
    assert!(apply_contract_transition(
        &mut state,
        &live_read,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &read_spec,
        Some(&evidence),
        "recovered contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_some());
    assert!(state.task_contract.latest_mutation_verified());
}

#[test]
fn untyped_recovered_action_clears_stale_binding_and_fails_closed() {
    let mut state = start_agent_loop(
        TaskId("legacy-recovered-workspace".to_string()),
        "recover the legacy write and verify it",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write");
    let initial = tool_request("initial", "file.write", r#"{"path":"A.md"}"#);
    apply_contract_transition(
        &mut state,
        &initial,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated A",
        POSTCONDITION_SCOPE,
    );

    let legacy_input = r#"{"path":"B.md","content":"old"}"#;
    let fingerprint = crate::tool_input_fingerprint("file.write", legacy_input);
    let current = PersistedToolEffectWitness::capture(
        "file.write",
        legacy_input,
        Some(&ToolRisk::WritesWorkspace),
        POSTCONDITION_SCOPE,
    )
    .and_then(|witness| witness.encode())
    .expect("current witness encodes");
    let mut legacy_value = serde_json::from_str::<serde_json::Value>(&current).expect("valid json");
    let object = legacy_value.as_object_mut().expect("witness is an object");
    object.insert(
        "schema".to_string(),
        serde_json::Value::String("cindx.tool-effect-witness.v1".to_string()),
    );
    object.remove("postconditionTargetWitness");
    object.remove("typedPostconditionBinding");
    let legacy = PersistedToolEffectWitness::decode(&legacy_value.to_string())
        .expect("legacy v1 witness remains conservatively recoverable");

    AgentKernel::new(&mut state, std::slice::from_ref(&write_spec))
        .with_postcondition_scope(Some(POSTCONDITION_SCOPE))
        .apply_persisted_tool_observation(
            ToolCallId("legacy-write".to_string()),
            "file.write",
            &fingerprint,
            None,
            Some(&legacy),
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::WritesWorkspace),
            "legacy write completed",
        );

    let read_spec = exact_read_spec("file.read");
    let read_a = tool_request("read-a", "file.read", r#"{"path":"A.md"}"#);
    let evidence = exact_read_evidence("A.md");
    assert!(apply_contract_transition(
        &mut state,
        &read_a,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &read_spec,
        Some(&evidence),
        "A contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_none());
    assert!(!state.task_contract.latest_mutation_verified());
}

#[test]
fn invalid_receipt_transition_rolls_back_verification_atomically() {
    let mut state = start_agent_loop(
        TaskId("typed-transactional-verification".to_string()),
        "update and verify",
        AgentRuntimeConfig::default(),
    );
    state
        .task_contract
        .merge_workspace_verification_policy(WorkspaceVerificationPolicy::RequiredAfterMutation);
    let write_spec = workspace_write_spec("file.write");
    let write = tool_request("write", "file.write", r#"{"path":"A.md"}"#);
    apply_contract_transition(
        &mut state,
        &write,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::WritesWorkspace,
        &write_spec,
        None,
        "updated",
        POSTCONDITION_SCOPE,
    );

    let oversized_name = "r".repeat(129);
    let invalid_spec = exact_read_spec(&oversized_name);
    let read = tool_request("read", &oversized_name, r#"{"path":"A.md"}"#);
    let evidence = exact_read_evidence("A.md");
    assert!(apply_contract_transition(
        &mut state,
        &read,
        &ToolOutcomeStatus::Succeeded,
        &ToolRisk::ReadOnly,
        &invalid_spec,
        Some(&evidence),
        "A contents",
        POSTCONDITION_SCOPE,
    )
    .postcondition_verification
    .is_none());
    assert!(!state.task_contract.latest_mutation_verified());
    assert_eq!(
        state.task_contract.evidence().last().map(|item| item.kind),
        Some(ContractEvidenceKind::Read)
    );
}

#[test]
fn receipt_evidence_survives_bounded_history_and_corruption_is_rejected_on_decode() {
    let mut state = start_agent_loop(
        TaskId("typed-persisted-validation".to_string()),
        "update and verify",
        AgentRuntimeConfig::default(),
    );
    let receipt = write_then_exact_read(&mut state, "A.md");
    let read_spec = exact_read_spec("file.read");
    for index in 0..160 {
        let read = tool_request(
            &format!("noise-{index}"),
            "file.read",
            &serde_json::json!({ "path": format!("noise-{index}.md") }).to_string(),
        );
        apply_contract_transition(
            &mut state,
            &read,
            &ToolOutcomeStatus::Succeeded,
            &ToolRisk::ReadOnly,
            &read_spec,
            None,
            "unrelated read",
            POSTCONDITION_SCOPE,
        );
    }
    assert!(state
        .task_contract
        .verify_postcondition_receipt(&receipt, 0, 0));

    let encoded = AgentTaskStateSnapshot::capture(&state)
        .to_json()
        .expect("valid bounded checkpoint encodes");
    let restored = AgentTaskStateSnapshot::from_json(&encoded).expect("valid receipt restores");
    assert!(restored
        .task_contract
        .verify_postcondition_receipt(&receipt, 0, 0));

    let mut corrupted = serde_json::from_str::<serde_json::Value>(&encoded).expect("valid json");
    *corrupted
        .pointer_mut("/taskContract/postconditionVerificationReceipts/0/receiptDigest")
        .expect("receipt digest exists") = serde_json::Value::String("0".repeat(64));
    let corrupted = corrupted.to_string();
    let error = AgentTaskStateSnapshot::from_json(&corrupted)
        .expect_err("corrupted persisted receipt must fail closed");
    assert!(error.to_string().contains("invalid postcondition state"));
}
