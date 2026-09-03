use super::commands::{permission_recovery_run_context, persist_cold_permission_recovery};
use super::observations::{
    copy_replayed_contract_evidence_metadata, current_permission_observation_boundary,
    permission_checkpoint_message_boundary, permission_tool_observation_metadata,
    persisted_permission_observations, restore_permission_snapshot_from_boundary,
};
use super::resolution::persist_denied_permission_resolution_rows;
use super::*;
use crate::agent_loop_runtime::{
    apply_run_task_contract, workspace_verification_policy_for_run_context,
};
use crate::agent_read_model::agent_runtime_transcript_from_active_events;
use crate::app_state::AgentRecoveryEnvelope;
use crate::event_persistence::append_event;
use crate::project_session_persistence::metadata_with_context;
use crate::runtime_constants::AGENT_RECOVERY_SCHEMA;
use crate::runtime_values::{current_time_millis, phase16_task_id};
use agent_application::{AgentRecoveryIdentity, AgentRecoveryReason, AgentRecoveryState};
use agent_core::{
    Event, EventId, EventKind, Message, MessageRole, PermissionDecision, PermissionRequest,
    PermissionRequestId, PermissionResolution, PermissionRisk, TaskId, ToolOutcomeStatus, ToolRisk,
    ToolSpec, AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY, AGENT_RUN_IDENTITY_V1_SCHEMA,
    LOGICAL_AGENT_RUN_ID_METADATA_KEY,
};
use agent_runtime::{
    start_agent_loop, AgentKernel, AgentRunControl, AgentRuntimeConfig, AgentTaskStateSnapshot,
    AgentToolRequest, PromptEvidenceScope, WorkspaceVerificationPolicy,
    MAX_IDENTICAL_TOOL_FAILURES,
};
use agent_storage::{EventStore, PermissionStore, SqliteStore, StorageError};
use std::collections::BTreeSet;
use tools::ToolRegistry;

#[test]
fn permission_cold_recovery_preserves_verification_contract() {
    let mut run_context = Metadata::new();
    let conductor_contract = agent_application::effort_execution_contract("auto")
        .to_json()
        .expect("contract serializes");
    let request_metadata = [
        ("conductor_contract".to_string(), conductor_contract.clone()),
        ("tool_input".to_string(), "sensitive".to_string()),
    ]
    .into_iter()
    .collect();

    restore_permission_run_context(&mut run_context, &request_metadata);

    assert_eq!(
        run_context.get("conductor_contract").map(String::as_str),
        Some(conductor_contract.as_str())
    );
    assert!(!run_context.contains_key("tool_input"));
    assert_eq!(
        workspace_verification_policy_for_run_context(&run_context)
            .expect("restored contract decodes"),
        WorkspaceVerificationPolicy::RequiredAfterMutation
    );
}

#[test]
fn permission_cold_recovery_preserves_browser_execution_intent() {
    let objective = "Open the incident dashboard in the browser and create a JSON report";
    let mut run_context = Metadata::new();
    let request_metadata = [
        (
            "effective_prompt_objective".to_string(),
            objective.to_string(),
        ),
        ("task_class".to_string(), "browser".to_string()),
        ("tool_requirement".to_string(), "effects".to_string()),
        ("vision_required".to_string(), "false".to_string()),
    ]
    .into_iter()
    .collect();

    restore_permission_run_context(&mut run_context, &request_metadata);

    let root = std::env::temp_dir().join("cindx-permission-browser-tool-plan");
    let mut registry = ToolRegistry::with_workspace_tools(root);
    registry.install_meta_tools();
    let (tools, completion_intent) =
        crate::agent_loop_runtime::planned_agent_tools(&registry, &run_context, objective, 128_000);
    let names = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        run_context.get("tool_requirement").map(String::as_str),
        Some("effects")
    );
    assert!(completion_intent
        .evidence_scopes
        .contains(&PromptEvidenceScope::Browser));
    assert!(names.contains("browser.open"));
    assert!(names.contains("browser.extract_text"));
    assert!(names.contains("file.write"));
    assert!(!names.iter().any(|name| name.starts_with("computer.")));
}

fn permission_run_context(steer_epoch: u64, prompt_contract_epoch: u64) -> Metadata {
    [
        ("session_id".to_string(), "session-a".to_string()),
        (
            AGENT_RUN_IDENTITY_SCHEMA_METADATA_KEY.to_string(),
            AGENT_RUN_IDENTITY_V1_SCHEMA.to_string(),
        ),
        ("agent_run_id".to_string(), "run-a".to_string()),
        (
            LOGICAL_AGENT_RUN_ID_METADATA_KEY.to_string(),
            "logical-run-a".to_string(),
        ),
        ("steer_epoch".to_string(), steer_epoch.to_string()),
        (
            "prompt_contract_epoch".to_string(),
            prompt_contract_epoch.to_string(),
        ),
        (
            "effective_prompt_objective".to_string(),
            "核实当前屏幕上的保存按钮".to_string(),
        ),
    ]
    .into_iter()
    .collect()
}

fn message(role: MessageRole, content: &str, metadata: Metadata) -> Message {
    Message {
        role,
        content: content.to_string(),
        metadata,
    }
}

fn permission_message(
    call_id: &str,
    tool_name: &str,
    status: ToolOutcomeStatus,
    content: &str,
    run_context: &Metadata,
) -> Message {
    permission_message_with_id(
        &format!("permission-{call_id}"),
        call_id,
        tool_name,
        status,
        content,
        run_context,
    )
}

fn permission_message_with_id(
    permission_id: &str,
    call_id: &str,
    tool_name: &str,
    status: ToolOutcomeStatus,
    content: &str,
    run_context: &Metadata,
) -> Message {
    message(
        MessageRole::Tool,
        content,
        permission_tool_observation_metadata(
            &PermissionRequestId(permission_id.to_string()),
            call_id,
            tool_name,
            &status,
            r#"{"secret":"super-secret"}"#,
            None,
            None,
            None,
            run_context,
        ),
    )
}

fn permission_recovery_event(sequence: u64, kind: EventKind, metadata: Metadata) -> Event {
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence,
        kind,
        summary: String::new(),
        metadata,
    }
}

fn permission_recovery_envelope(
    resume_key: &str,
    task_state: AgentTaskStateSnapshot,
) -> AgentRecoveryEnvelope {
    AgentRecoveryEnvelope {
        schema: AGENT_RECOVERY_SCHEMA.to_string(),
        identity: AgentRecoveryIdentity {
            project_id: Some("project-a".to_string()),
            session_id: "session-a".to_string(),
            resume_key: resume_key.to_string(),
            source_run_id: "run-a".to_string(),
            logical_run_id: Some("logical-run-a".to_string()),
            user_turn_sequence: 1,
            prompt_fingerprint: task_state.user_prompt_fingerprint.clone(),
        },
        effort: "auto".to_string(),
        policy: "auto_router".to_string(),
        queue_id: None,
        state: AgentRecoveryState::Blocked,
        reason: AgentRecoveryReason::WaitingForPermission,
        attempts: 0,
        model_calls: 1,
        tool_calls: 0,
        material_checkpoints: 0,
        observations: 0,
        budget_extensions: 0,
        task_state: Some(task_state),
        resource_snapshot: None,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

#[test]
fn permission_recovery_keeps_logical_identity_on_the_physical_attempt() {
    let runtime = start_agent_loop(
        TaskId("permission-identity".to_string()),
        "inspect",
        AgentRuntimeConfig::default(),
    );
    let recovery =
        permission_recovery_envelope("identity", AgentTaskStateSnapshot::capture(&runtime));
    let recovered = permission_recovery_run_context(&permission_run_context(0, 0), &recovery, 1);

    assert_eq!(
        recovered.get("agent_run_id").map(String::as_str),
        Some("run-a")
    );
    assert_eq!(
        recovered
            .get(LOGICAL_AGENT_RUN_ID_METADATA_KEY)
            .map(String::as_str),
        Some("logical-run-a")
    );
    assert_eq!(
        recovered.get("source_agent_run_id").map(String::as_str),
        Some("run-a")
    );
}

#[test]
fn permission_checkpoint_boundary_uses_the_matching_blocked_event() {
    let message_event = |sequence, role: &str, content: &str| {
        permission_recovery_event(
            sequence,
            EventKind::MessageAdded,
            [
                ("role".to_string(), role.to_string()),
                ("content".to_string(), content.to_string()),
            ]
            .into_iter()
            .collect(),
        )
    };
    let mut runtime = start_agent_loop(
        TaskId("permission-boundary".to_string()),
        "inspect",
        AgentRuntimeConfig::default(),
    );
    let older_snapshot = AgentTaskStateSnapshot::capture(&runtime);
    runtime
        .messages
        .push(message(MessageRole::Assistant, "working", Metadata::new()));
    let active_snapshot = AgentTaskStateSnapshot::capture(&runtime);
    let active_recovery = permission_recovery_envelope("active", active_snapshot.clone());
    let missing_recovery = permission_recovery_envelope("missing", active_snapshot.clone());
    let blocked_event = |sequence, envelope: AgentRecoveryEnvelope| {
        permission_recovery_event(
            sequence,
            EventKind::TaskStatusChanged,
            [
                (
                    "recovery_resume_key".to_string(),
                    envelope.identity.resume_key.clone(),
                ),
                ("recovery_state".to_string(), "blocked".to_string()),
                (
                    "recovery_envelope".to_string(),
                    serde_json::to_string(&envelope).expect("envelope serializes"),
                ),
            ]
            .into_iter()
            .collect(),
        )
    };
    let events = vec![
        message_event(1, "user", "inspect"),
        blocked_event(2, permission_recovery_envelope("older", older_snapshot)),
        message_event(3, "assistant", "working"),
        blocked_event(4, active_recovery.clone()),
        message_event(5, "tool", "resolved after the checkpoint"),
        blocked_event(6, active_recovery.clone()),
    ];

    assert_eq!(
        permission_checkpoint_message_boundary(&events, &active_recovery, &active_snapshot),
        Some(2)
    );
    assert_eq!(
        permission_checkpoint_message_boundary(&events, &missing_recovery, &active_snapshot),
        None
    );
}

#[test]
fn startup_recovery_replays_a_committed_permission_denial_into_task_state() {
    let run_context = permission_run_context(4, 0);
    let prompt = "核实当前屏幕上的保存按钮";
    let mut runtime = start_agent_loop(
        TaskId("permission-startup-replay".to_string()),
        prompt,
        AgentRuntimeConfig::default(),
    );
    runtime.task_contract.require_tool_success("shell.run");
    let snapshot = AgentTaskStateSnapshot::capture(&runtime);
    let recovery = permission_recovery_envelope("startup-replay", snapshot);
    let mut user_metadata = metadata_with_context(
        [
            ("role".to_string(), "user".to_string()),
            ("content".to_string(), prompt.to_string()),
        ]
        .into_iter()
        .collect(),
        &run_context,
    );
    user_metadata.insert("project_id".to_string(), "project-a".to_string());
    let mut blocked_metadata = metadata_with_context(
        [
            (
                "recovery_resume_key".to_string(),
                recovery.identity.resume_key.clone(),
            ),
            ("recovery_state".to_string(), "blocked".to_string()),
            (
                "recovery_envelope".to_string(),
                serde_json::to_string(&recovery).expect("envelope serializes"),
            ),
        ]
        .into_iter()
        .collect(),
        &run_context,
    );
    blocked_metadata.insert("project_id".to_string(), "project-a".to_string());
    let denied = permission_message(
        "call-denied",
        "shell.run",
        ToolOutcomeStatus::Denied,
        "The user denied this tool call.",
        &run_context,
    );
    let mut denied_metadata = denied.metadata;
    denied_metadata.insert("role".to_string(), "tool".to_string());
    denied_metadata.insert("content".to_string(), denied.content);
    let events = vec![
        permission_recovery_event(1, EventKind::MessageAdded, user_metadata),
        permission_recovery_event(2, EventKind::TaskStatusChanged, blocked_metadata),
        permission_recovery_event(3, EventKind::MessageAdded, denied_metadata),
    ];

    let recovered = recovery_task_state_with_persisted_permission_denials(
        &events,
        &run_context,
        &recovery,
        None,
    )
    .expect("startup recovery should rebuild the typed denial");
    let ledger = recovered.task_contract.outcome_ledger_shadow(4);
    let blocked = ledger
        .obligations
        .iter()
        .find(|obligation| {
            obligation
                .blocker
                .as_ref()
                .is_some_and(|blocker| blocker.code == "user_permission_denied")
        })
        .expect("the denied required tool must remain blocked");
    assert_eq!(
        blocked.satisfaction,
        agent_runtime::OutcomeSatisfaction::Blocked
    );
    let encoded = serde_json::to_string(&recovered).expect("snapshot serializes");
    assert!(!encoded.contains("super-secret"));
    assert!(!encoded.contains("The user denied"));

    let transcript = agent_runtime_transcript_from_active_events(&events);
    let mut more_complete = recovered
        .restore_with_effective_objective(prompt, transcript.clone(), prompt)
        .expect("recovered denial state should restore");
    more_complete
        .task_contract
        .require_tool_success("file.read");
    let read_fingerprint =
        agent_runtime::tool_input_fingerprint("file.read", r#"{"path":"status.md"}"#);
    AgentKernel::new(
        &mut more_complete,
        &[ToolSpec::builtin(
            "file.read",
            "test",
            "Read status",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )],
    )
    .apply_persisted_tool_observation(
        agent_core::ToolCallId("read-after-denial".to_string()),
        "file.read",
        &read_fingerprint,
        None,
        None,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ReadOnly),
        "status evidence",
    );
    let preferred = AgentTaskStateSnapshot::capture(&more_complete);
    let selected = recovery_task_state_with_persisted_permission_denials(
        &events,
        &run_context,
        &recovery,
        Some(&preferred),
    )
    .expect("a newer snapshot containing the denial should be retained");
    assert_eq!(selected, preferred);
    assert!(selected
        .task_contract
        .outcome_ledger_shadow(4)
        .obligations
        .iter()
        .any(|obligation| {
            obligation.kind == agent_runtime::OutcomeObligationKind::RequiredTool
                && obligation.satisfaction == agent_runtime::OutcomeSatisfaction::Satisfied
        }));

    let denied_fingerprint = transcript
        .last()
        .and_then(|message| message.metadata.get("tool_input_fingerprint"))
        .cloned()
        .expect("the canonical denial should carry an input fingerprint");
    let (mut wrong_semantics, _) = restore_permission_snapshot_from_boundary(
        recovery
            .task_state
            .as_ref()
            .expect("blocked recovery should carry task state"),
        prompt,
        prompt,
        &transcript,
        1,
    )
    .expect("the blocked baseline should restore");
    AgentKernel::new(&mut wrong_semantics, &[]).apply_persisted_tool_observation_with_denial(
        agent_core::ToolCallId("wrong-capability-denial".to_string()),
        "shell.run",
        &denied_fingerprint,
        None,
        None,
        &ToolOutcomeStatus::Denied,
        None,
        "capability unavailable",
        Some(
            &agent_runtime::AgentActionDenialFeedback::capability_unavailable(
                "tool_capability_unavailable",
            ),
        ),
    );
    let wrong_semantics = AgentTaskStateSnapshot::capture(&wrong_semantics);
    assert!(!wrong_semantics
        .task_contract
        .has_user_permission_finalization("shell.run", &denied_fingerprint));
    let corrected = recovery_task_state_with_persisted_permission_denials(
        &events,
        &run_context,
        &recovery,
        Some(&wrong_semantics),
    )
    .expect("a semantically different denial must not suppress canonical replay");
    assert_ne!(corrected, wrong_semantics);
    assert!(corrected
        .task_contract
        .outcome_ledger_shadow(4)
        .obligations
        .iter()
        .any(|obligation| {
            obligation.blocker.as_ref().is_some_and(|blocker| {
                blocker.kind == agent_runtime::AgentActionDenialKind::UserPermission
                    && blocker.code == "user_permission_denied"
            })
        }));
}

#[test]
fn permission_observation_marker_is_runtime_only_and_lineage_bound() {
    let run_context = permission_run_context(4, 0);
    let valid = permission_message(
        "call-valid",
        "computer.screenshot",
        ToolOutcomeStatus::Succeeded,
        "captured",
        &run_context,
    );
    assert!(valid
        .metadata
        .values()
        .all(|value| !value.contains("super-secret")));

    let mut legacy = valid.clone();
    legacy.metadata.remove("permission_observation_schema");
    let mut forged = valid.clone();
    forged.metadata.insert(
        "permission_observation_provenance".to_string(),
        "model_claim".to_string(),
    );
    let mut wrong_run = valid.clone();
    wrong_run
        .metadata
        .insert("agent_run_id".to_string(), "run-b".to_string());
    let mut wrong_contract_epoch = valid.clone();
    wrong_contract_epoch
        .metadata
        .insert("prompt_contract_epoch".to_string(), "4".to_string());
    let mut invalid_status = valid.clone();
    invalid_status
        .metadata
        .insert("status".to_string(), "unknown".to_string());

    let observations = persisted_permission_observations(
        &[
            legacy,
            forged,
            wrong_run,
            wrong_contract_epoch,
            invalid_status,
            valid,
        ],
        &run_context,
    );

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].call_id.0, "call-valid");
    assert_eq!(observations[0].permission_id.0, "permission-call-valid");
    assert_eq!(observations[0].message_index, 5);
}

#[test]
fn permission_boundary_uses_unique_permission_id_when_call_ids_repeat() {
    let run_context = permission_run_context(4, 0);
    let transcript = vec![
        permission_message_with_id(
            "permission-old",
            "reused-call",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "old capture",
            &run_context,
        ),
        permission_message_with_id(
            "permission-current",
            "reused-call",
            "computer.screenshot",
            ToolOutcomeStatus::Succeeded,
            "current capture",
            &run_context,
        ),
    ];
    let observations = persisted_permission_observations(&transcript, &run_context);

    assert_eq!(
        current_permission_observation_boundary(
            &observations,
            &["permission-current".to_string()].into_iter().collect(),
            transcript.len(),
        ),
        1
    );
}

#[test]
fn persisted_permission_observations_replay_each_permission_only_once() {
    let run_context = permission_run_context(4, 0);
    let observation = permission_message_with_id(
        "permission-a",
        "call-a",
        "shell.run",
        ToolOutcomeStatus::Denied,
        "denied",
        &run_context,
    );

    let observations = persisted_permission_observations(
        &[observation.clone(), observation.clone()],
        &run_context,
    );

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].permission_id.0, "permission-a");

    let mut conflicting = observation.clone();
    conflicting
        .metadata
        .insert("tool_call_id".to_string(), "call-b".to_string());
    assert!(
        persisted_permission_observations(&[observation, conflicting], &run_context).is_empty()
    );
}

#[test]
fn cold_recovery_replays_all_permission_observations_without_duplicates() {
    let mut run_context = permission_run_context(4, 0);
    run_context.insert(
        "effective_prompt_objective".to_string(),
        "Initial request:\n核实当前屏幕上的保存按钮\n\nAccepted steering 1:\n继续核实当前屏幕上的保存按钮"
            .to_string(),
    );
    let tools = vec![ToolSpec::builtin(
        "computer.screenshot",
        "computer",
        "Capture the current screen",
        ToolRisk::SensitiveContext,
        r#"{"type":"object"}"#,
    )
    .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)];
    let mut original = start_agent_loop(
        TaskId("permission-recovery".to_string()),
        "核实当前屏幕上的保存按钮",
        AgentRuntimeConfig::default(),
    );
    original.messages.push(message(
        MessageRole::Assistant,
        "",
        [("tool_call_ids".to_string(), "call-a,call-b".to_string())]
            .into_iter()
            .collect(),
    ));
    apply_run_task_contract(&mut original, &run_context, &tools, None)
        .expect("contract applies before permission pause");
    AgentKernel::new(&mut original, &tools).require_tool_success("shell.run");
    let ledger_before_pause = original.task_contract.outcome_ledger_shadow(4);
    let snapshot = AgentTaskStateSnapshot::capture(&original);
    let original_message_count = original.messages.len();

    let mut transcript = original.messages.clone();
    transcript.push(message(
        MessageRole::User,
        "noop control message after the blocked checkpoint",
        [("steer".to_string(), "true".to_string())]
            .into_iter()
            .collect(),
    ));
    transcript.push(permission_message(
        "call-a",
        "computer.screenshot",
        ToolOutcomeStatus::Succeeded,
        "captured",
        &run_context,
    ));
    transcript.push(message(
        MessageRole::User,
        "Visual reference captured by computer.screenshot.",
        [("kind".to_string(), "visual_reference".to_string())]
            .into_iter()
            .collect(),
    ));
    transcript.push(permission_message(
        "call-b",
        "shell.run",
        ToolOutcomeStatus::Denied,
        "denied",
        &run_context,
    ));
    transcript.push(permission_message(
        "call-c",
        "shell.run",
        ToolOutcomeStatus::Denied,
        "denied again",
        &run_context,
    ));

    let observations = persisted_permission_observations(&transcript, &run_context);
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.call_id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a", "call-b", "call-c"]
    );
    let (mut restored, boundary) = restore_permission_snapshot_from_boundary(
        &snapshot,
        &original.user_prompt,
        original.prepared_task_state().effective_objective(),
        &transcript,
        original_message_count,
    )
    .expect("blocked checkpoint should restore from its recorded message boundary");
    assert_eq!(boundary, original_message_count);
    assert_eq!(restored.prepared_task_state().steer_epoch(), 4);
    assert_eq!(restored.prepared_task_state().contract_epoch(), 0);
    assert_ne!(
        restored.prepared_task_state().effective_objective(),
        restored.user_prompt
    );
    assert_eq!(
        restored.task_contract.outcome_ledger_shadow(4),
        ledger_before_pause,
        "permission pause and cold recovery must preserve the shadow contract"
    );

    apply_run_task_contract(&mut restored, &run_context, &tools, None)
        .expect("contract reapplies after recovery");
    for observation in observations
        .iter()
        .filter(|observation| observation.message_index >= boundary)
    {
        let risk =
            (observation.tool_name == "computer.screenshot").then_some(ToolRisk::SensitiveContext);
        let denial = matches!(observation.status, ToolOutcomeStatus::Denied)
            .then(agent_runtime::AgentActionDenialFeedback::user_permission);
        AgentKernel::new(&mut restored, &tools).apply_persisted_tool_observation_with_denial(
            observation.call_id.clone(),
            &observation.tool_name,
            &observation.input_fingerprint,
            observation.target_witness.as_deref(),
            observation.effect_witness.as_ref(),
            &observation.status,
            risk.as_ref(),
            &observation.observation,
            denial.as_ref(),
        );
        copy_replayed_contract_evidence_metadata(
            &restored,
            &mut transcript,
            observation.message_index,
        );
    }
    assert_eq!(
        AgentKernel::new(&mut restored, &tools).repeated_tool_failure_count(&AgentToolRequest {
            call_id: agent_core::ToolCallId("probe-same".to_string()),
            tool_name: "shell.run".to_string(),
            input: r#"{"secret":"super-secret"}"#.to_string(),
        }),
        MAX_IDENTICAL_TOOL_FAILURES
    );
    assert_eq!(
        AgentKernel::new(&mut restored, &tools).repeated_tool_failure_count(&AgentToolRequest {
            call_id: agent_core::ToolCallId("probe-changed".to_string()),
            tool_name: "shell.run".to_string(),
            input: r#"{"secret":"different"}"#.to_string(),
        }),
        0
    );
    restored.messages = transcript.clone();

    assert!(restored.messages.iter().any(|message| {
        message.metadata.get("tool_call_id").map(String::as_str) == Some("call-a")
            && message
                .metadata
                .contains_key(agent_runtime::CONTRACT_EVIDENCE_SEQUENCES_METADATA_KEY)
    }));

    assert_eq!(
        AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
        Ok(None)
    );
    assert_eq!(restored.messages, transcript);
    assert_eq!(
        restored
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .count(),
        3
    );
    let recovered_ledger = restored.task_contract.outcome_ledger_shadow(4);
    let screenshot_evidence = recovered_ledger
        .evidence
        .iter()
        .filter(|evidence| evidence.source == "computer.screenshot")
        .map(|evidence| evidence.kind)
        .collect::<Vec<_>>();
    assert_eq!(screenshot_evidence.len(), 2);
    assert!(screenshot_evidence.contains(&agent_runtime::ContractEvidenceKind::Grounding));
    assert!(
        screenshot_evidence.contains(&agent_runtime::ContractEvidenceKind::OtherTool),
        "a recovered observation without a bound interaction must fail closed: {screenshot_evidence:?}"
    );
    let shell_denial_evidence = recovered_ledger
        .evidence
        .iter()
        .filter(|evidence| {
            evidence.source == "shell.run"
                && evidence.kind == agent_runtime::ContractEvidenceKind::Denial
        })
        .collect::<Vec<_>>();
    assert_eq!(shell_denial_evidence.len(), 1);
    let shell_obligation = recovered_ledger
        .obligations
        .iter()
        .find(|obligation| {
            obligation
                .blocker
                .as_ref()
                .is_some_and(|blocker| blocker.code == "user_permission_denied")
        })
        .expect("cold recovery must preserve the typed permission blocker");
    assert_eq!(
        shell_obligation.satisfaction,
        agent_runtime::OutcomeSatisfaction::Blocked
    );
    assert_eq!(
        shell_obligation.evidence_sequence,
        Some(shell_denial_evidence[0].sequence)
    );
    let encoded_contract =
        serde_json::to_string(&restored.task_contract).expect("contract serializes");
    assert!(encoded_contract.contains("user_permission_denied"));
    assert!(!encoded_contract.contains("super-secret"));
    assert!(!encoded_contract.contains("denied again"));
}

#[test]
fn later_permission_pause_restores_after_prior_permission_evidence() {
    let run_context = permission_run_context(4, 0);
    let tools = vec![ToolSpec::builtin(
        "computer.screenshot",
        "computer",
        "Capture the current screen",
        ToolRisk::SensitiveContext,
        r#"{"type":"object"}"#,
    )
    .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)];
    let mut prior = start_agent_loop(
        TaskId("sequential-permission-recovery".to_string()),
        "核实当前屏幕上的保存按钮",
        AgentRuntimeConfig::default(),
    );
    apply_run_task_contract(&mut prior, &run_context, &tools, None)
        .expect("initial contract applies");
    AgentKernel::new(&mut prior, &tools).apply_tool_observation(
        &AgentToolRequest {
            call_id: agent_core::ToolCallId("call-a".to_string()),
            tool_name: "computer.screenshot".to_string(),
            input: r#"{"display":0}"#.to_string(),
        },
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::SensitiveContext),
        "captured first screen",
    );
    prior.messages.pop();
    prior.messages.push(permission_message(
        "call-a",
        "computer.screenshot",
        ToolOutcomeStatus::Succeeded,
        "captured first screen",
        &run_context,
    ));
    let snapshot = AgentTaskStateSnapshot::capture(&prior);
    let snapshot_boundary = prior.messages.len();

    let mut transcript = prior.messages.clone();
    transcript.push(message(
        MessageRole::Assistant,
        "continuing after the first approval",
        Metadata::new(),
    ));
    transcript.push(permission_message(
        "call-b",
        "shell.run",
        ToolOutcomeStatus::Denied,
        "denied",
        &run_context,
    ));
    let observations = persisted_permission_observations(&transcript, &run_context);
    let boundary = current_permission_observation_boundary(
        &observations,
        &["permission-call-b".to_string()].into_iter().collect(),
        transcript.len(),
    );
    assert_eq!(boundary, transcript.len() - 1);

    let (mut restored, restored_boundary) = restore_permission_snapshot_from_boundary(
        &snapshot,
        &prior.user_prompt,
        prior.prepared_task_state().effective_objective(),
        &transcript,
        snapshot_boundary,
    )
    .expect("latest blocked snapshot should match its recorded message boundary");
    assert_eq!(restored_boundary, snapshot_boundary);
    assert_eq!(
        AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
        Ok(None)
    );
}

#[test]
fn cold_permission_recovery_reconstructs_verified_workspace_postcondition() {
    let mut run_context = permission_run_context(0, 0);
    let conductor_contract = agent_application::effort_execution_contract("auto")
        .to_json()
        .expect("contract serializes");
    run_context.insert("conductor_contract".to_string(), conductor_contract);
    run_context.insert(
        "effective_prompt_objective".to_string(),
        "write the requested file and verify it".to_string(),
    );
    let tools = vec![
        ToolSpec::builtin(
            "file.write",
            "file",
            "Write a file",
            ToolRisk::WritesWorkspace,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::Verifiable {
            verifier: "workspace_file_content_v1".to_string(),
        }),
        ToolSpec::builtin(
            "file.read",
            "file",
            "Read a file",
            ToolRisk::ReadOnly,
            r#"{"type":"object"}"#,
        )
        .with_effect_semantics(agent_core::ToolEffectSemantics::ReadOnly)
        .with_postcondition_verifier(
            agent_core::PostconditionVerifierKind::WorkspaceExactReadbackV1,
        ),
    ];
    let mut original = start_agent_loop(
        TaskId("cold-workspace-postcondition".to_string()),
        "write the requested file and verify it",
        AgentRuntimeConfig::default(),
    );
    apply_run_task_contract(&mut original, &run_context, &tools, None)
        .expect("verification contract applies before the permission pause");
    let snapshot = AgentTaskStateSnapshot::capture(&original);
    let boundary = original.messages.len();

    let target = "private/super-secret-goal.md";
    let write_input = format!(r#"{{"path":"{target}","content":"never-persist-this-secret"}}"#);
    let read_input = format!(r#"{{"path":"{target}"}}"#);
    let permission_observation =
        |permission_id: &str, call_id: &str, tool_name: &str, input: &str, risk: &ToolRisk| {
            let effect_spec = tools.iter().find(|spec| spec.name == tool_name);
            let postcondition_evidence =
                (tool_name == "file.read").then(|| agent_core::ToolPostconditionEvidence {
                    kind: agent_core::PostconditionVerifierKind::WorkspaceExactReadbackV1,
                    target_input_json: input.to_string(),
                });
            message(
                MessageRole::Tool,
                "tool completed with bounded evidence",
                permission_tool_observation_metadata(
                    &PermissionRequestId(permission_id.to_string()),
                    call_id,
                    tool_name,
                    &ToolOutcomeStatus::Succeeded,
                    input,
                    Some(risk),
                    effect_spec,
                    postcondition_evidence.as_ref(),
                    &run_context,
                ),
            )
        };
    let mut transcript = original.messages.clone();
    transcript.push(permission_observation(
        "permission-write",
        "write",
        "file.write",
        &write_input,
        &ToolRisk::WritesWorkspace,
    ));
    transcript.push(permission_observation(
        "permission-read",
        "read",
        "file.read",
        &read_input,
        &ToolRisk::ReadOnly,
    ));

    for persisted in &transcript[boundary..] {
        let witness = persisted
            .metadata
            .get(agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY)
            .expect("successful effect should persist a recovery witness");
        assert!(witness.len() <= agent_runtime::MAX_PERSISTED_TOOL_EFFECT_WITNESS_BYTES);
        for secret in [
            target,
            "private",
            "super-secret",
            "never-persist-this-secret",
        ] {
            assert!(!witness.contains(secret));
            assert!(persisted
                .metadata
                .values()
                .all(|value| !value.contains(secret)));
        }
    }

    let observations = persisted_permission_observations(&transcript, &run_context);
    assert_eq!(observations.len(), 2);
    assert!(observations
        .iter()
        .all(|observation| observation.effect_witness.is_some()));
    let (mut restored, replay_boundary) = restore_permission_snapshot_from_boundary(
        &snapshot,
        &original.user_prompt,
        original.prepared_task_state().effective_objective(),
        &transcript,
        boundary,
    )
    .expect("cold recovery should restore the blocked snapshot");
    assert_eq!(replay_boundary, boundary);
    apply_run_task_contract(&mut restored, &run_context, &tools, None)
        .expect("verification contract reapplies after restart");
    let mut deltas = Vec::new();
    for observation in &observations {
        let risk = tools
            .iter()
            .find(|tool| tool.name == observation.tool_name)
            .map(|tool| &tool.risk);
        deltas.extend(
            AgentKernel::new(&mut restored, &tools).apply_persisted_tool_observation(
                observation.call_id.clone(),
                &observation.tool_name,
                &observation.input_fingerprint,
                observation.target_witness.as_deref(),
                observation.effect_witness.as_ref(),
                &observation.status,
                risk,
                &observation.observation,
            ),
        );
    }
    assert!(deltas.iter().any(|delta| {
        delta
            .kinds()
            .contains(&agent_runtime::AgentGoalDeltaKind::WorkspaceVerified)
    }));
    assert_eq!(
        AgentKernel::new(&mut restored, &tools).completion_gate_for_task(),
        Ok(None),
        "the cold-replayed write/read pair must close the postcondition"
    );
    assert!(restored
        .task_contract
        .outcome_ledger_shadow(0)
        .postconditions
        .iter()
        .any(|postcondition| {
            postcondition.status == agent_runtime::OutcomePostconditionStatus::Verified
        }));

    let mut legacy_transcript = transcript;
    for message in &mut legacy_transcript[boundary..] {
        message
            .metadata
            .remove(agent_runtime::TOOL_EFFECT_WITNESS_METADATA_KEY);
    }
    let legacy_observations = persisted_permission_observations(&legacy_transcript, &run_context);
    assert!(legacy_observations
        .iter()
        .all(|observation| observation.effect_witness.is_none()));
    let (mut legacy, _) = restore_permission_snapshot_from_boundary(
        &snapshot,
        &original.user_prompt,
        original.prepared_task_state().effective_objective(),
        &legacy_transcript,
        boundary,
    )
    .expect("old snapshots without a witness remain readable");
    apply_run_task_contract(&mut legacy, &run_context, &tools, None)
        .expect("old snapshot contract reapplies");
    for observation in &legacy_observations {
        let risk = tools
            .iter()
            .find(|tool| tool.name == observation.tool_name)
            .map(|tool| &tool.risk);
        AgentKernel::new(&mut legacy, &tools).apply_persisted_tool_observation(
            observation.call_id.clone(),
            &observation.tool_name,
            &observation.input_fingerprint,
            observation.target_witness.as_deref(),
            None,
            &observation.status,
            risk,
            &observation.observation,
        );
    }
    assert!(AgentKernel::new(&mut legacy, &tools)
        .completion_gate_for_task()
        .expect("legacy completion gate evaluates")
        .is_some());
}

fn permission_goal_delta() -> agent_runtime::AgentGoalDelta {
    let tools = vec![ToolSpec::builtin(
        "file.read",
        "test",
        "Read a file",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    )];
    let mut runtime = start_agent_loop(
        TaskId("cold-permission-credit".to_string()),
        "read the required file",
        AgentRuntimeConfig::default(),
    );
    runtime.task_contract.require_tool_success("file.read");
    AgentKernel::new(&mut runtime, &tools)
        .apply_tool_observation(
            &AgentToolRequest {
                call_id: agent_core::ToolCallId("read".to_string()),
                tool_name: "file.read".to_string(),
                input: r#"{"path":"goal.md"}"#.to_string(),
            },
            &ToolOutcomeStatus::Succeeded,
            Some(&ToolRisk::ReadOnly),
            "goal evidence",
        )
        .expect("the required tool should produce a Goal Delta")
}

#[test]
fn denied_permission_bundle_commits_and_rolls_back_as_one_unit() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let task_id = TaskId("permission-bundle".to_string());
    let request_id = PermissionRequestId("permission-bundle-deny".to_string());
    let request = PermissionRequest {
        id: request_id.clone(),
        task_id: task_id.clone(),
        risk: PermissionRisk::Execute,
        action: "shell.run".to_string(),
        reason: "Run a protected command".to_string(),
        scope: "/tmp/project".to_string(),
        metadata: Metadata::new(),
    };
    store
        .save_permission_request(request.clone(), 100)
        .expect("permission should persist");
    let resolution = PermissionResolution {
        request_id: request_id.clone(),
        decision: PermissionDecision::Deny,
        resolved_at_ms: 200,
        resolved_by: "local-user".to_string(),
    };
    let run_context = [
        ("session_id".to_string(), "session-bundle".to_string()),
        ("agent_run_id".to_string(), "run-bundle".to_string()),
        ("prompt_contract_epoch".to_string(), "0".to_string()),
    ]
    .into_iter()
    .collect::<Metadata>();
    let observation = "Tool shell.run denied: The user denied this tool call.";
    let message_metadata = permission_tool_observation_metadata(
        &request_id,
        "call-bundle",
        "shell.run",
        &ToolOutcomeStatus::Denied,
        r#"{"command":"touch protected"}"#,
        None,
        None,
        None,
        &run_context,
    );

    let injected = store.with_immediate_transaction(|store| {
        persist_denied_permission_resolution_rows(
            store,
            &request,
            &resolution,
            "call-bundle",
            "shell.run",
            observation,
            &message_metadata,
            &run_context,
        )?;
        Err::<(), _>(StorageError::new("injected post-bundle failure"))
    });
    assert!(injected.is_err());
    assert!(
        store.list_permission_audits().expect("audits should load")[0]
            .resolution
            .is_none()
    );
    assert!(store
        .list_by_task(&task_id)
        .expect("rolled-back events should load")
        .is_empty());

    store
        .with_immediate_transaction(|store| {
            persist_denied_permission_resolution_rows(
                store,
                &request,
                &resolution,
                "call-bundle",
                "shell.run",
                observation,
                &message_metadata,
                &run_context,
            )
        })
        .expect("denial bundle should commit");
    let audit = store
        .list_permission_audits()
        .expect("audits should load")
        .remove(0);
    assert_eq!(
        audit.resolution.map(|resolution| resolution.decision),
        Some(PermissionDecision::Deny)
    );
    let events = store
        .list_by_task(&task_id)
        .expect("bundle events should load");
    assert_eq!(events.len(), 3);
    assert!(events.iter().any(|event| {
        event.kind == EventKind::PermissionResolved
            && event.metadata.get("permission_id") == Some(&request_id.0)
    }));
    assert!(events.iter().any(|event| {
        event.kind == EventKind::ToolCallFinished
            && event.metadata.get("failure_code").map(String::as_str)
                == Some("user_permission_denied")
    }));
    assert!(events.iter().any(|event| {
        event.kind == EventKind::MessageAdded
            && event
                .metadata
                .get("action_denial_schema")
                .map(String::as_str)
                == Some(agent_runtime::ACTION_DENIAL_SCHEMA)
    }));
}

#[test]
fn cold_permission_recovery_persists_before_the_caller_credits_goal_delta() {
    let empty_commit_called = std::cell::Cell::new(false);
    persist_cold_permission_recovery(&[], false, || {
        empty_commit_called.set(true);
        Ok(())
    })
    .expect("an empty recovery should be a no-op");
    assert!(
        !empty_commit_called.get(),
        "a recovery without Goal Deltas should not add a snapshot write"
    );

    let delta = permission_goal_delta();
    let failed_control = AgentRunControl::new("auto");
    let failure = persist_cold_permission_recovery(std::slice::from_ref(&delta), true, || {
        Err("injected runtime snapshot failure".to_string())
    });
    assert!(failure.is_err());
    assert!(
        failed_control.record_goal_delta_at(0, &delta),
        "a failed runtime commit must not consume or credit the Goal Delta"
    );

    let committed_control = AgentRunControl::new("auto");
    persist_cold_permission_recovery(std::slice::from_ref(&delta), true, || Ok(()))
        .expect("the recovered runtime should persist");
    assert!(
        committed_control.record_goal_delta_at(0, &delta),
        "runtime persistence must not credit the live control before the recovery claim"
    );

    let denial_only_commit_called = std::cell::Cell::new(false);
    persist_cold_permission_recovery(&[], true, || {
        denial_only_commit_called.set(true);
        Ok(())
    })
    .expect("a denial-only recovery should persist its rebuilt contract");
    assert!(denial_only_commit_called.get());
}

fn subagent_patch_request(request_id: &str, run_context: &Metadata) -> PermissionRequest {
    let mut request = PermissionRequest {
        id: PermissionRequestId(request_id.to_string()),
        task_id: phase16_task_id(),
        risk: PermissionRisk::Write,
        action: "file.patch".to_string(),
        reason: "Write subagent \"fix the readme\" requests approval: Atomically patch an existing file in the selected workspace.".to_string(),
        scope: "README.md".to_string(),
        metadata: Metadata::new(),
    };
    request
        .metadata
        .insert("session_reusable".to_string(), "false".to_string());
    request.metadata.insert(
        crate::agent_subagent_runtime::SUBAGENT_PERMISSION_ORIGIN_KEY.to_string(),
        "true".to_string(),
    );
    request.metadata.insert(
        "tool_call_id".to_string(),
        format!("subagent:parent-call:{request_id}"),
    );
    request
        .metadata
        .insert("tool_name".to_string(), "file.patch".to_string());
    request.metadata = agent_application::merge_persistable_run_context(
        std::mem::take(&mut request.metadata),
        run_context,
    );
    request
}

fn seed_subagent_permission_run(
    store: &mut SqliteStore,
    run_context: &Metadata,
    request: &PermissionRequest,
) {
    append_event(
        store,
        &phase16_task_id(),
        EventKind::TaskStatusChanged,
        "Agent task started",
        metadata_with_context(
            [("prompt".to_string(), "fix the readme".to_string())]
                .into_iter()
                .collect(),
            run_context,
        ),
    )
    .expect("run should start");
    store
        .save_permission_request(request.clone(), current_time_millis())
        .expect("request should save");
}

#[test]
fn subagent_permission_resolves_in_place_without_executing_or_resuming() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = permission_run_context(0, 0);
    let request = subagent_patch_request("agent-perm-subagent", &run_context);
    seed_subagent_permission_run(&mut store, &run_context, &request);

    let state = super::resolution::resolve_subagent_permission_in_store(
        &mut store,
        &request,
        &PermissionDecision::AllowOnce,
        &run_context,
        "session-a",
    )
    .expect("resolution should succeed");

    assert!(state.pending_approvals.is_empty());
    let audit = store
        .list_permission_audits()
        .expect("audits should load")
        .into_iter()
        .find(|audit| audit.request.id == request.id)
        .expect("audit should exist");
    assert_eq!(
        audit.resolution.map(|resolution| resolution.decision),
        Some(PermissionDecision::AllowOnce)
    );
    let events = store
        .list_by_task(&phase16_task_id())
        .expect("events should load");
    assert!(events.iter().any(|event| {
        event.kind == EventKind::PermissionResolved
            && event.metadata.get("permission_id").map(String::as_str)
                == Some("agent-perm-subagent")
    }));
    // The in-place resolution executes nothing and writes no transcript
    // message: the waiting subagent thread performs the approved patch.
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        EventKind::ToolCallFinished | EventKind::ToolCallStarted
    )));
    assert!(!events.iter().any(|event| {
        event.kind == EventKind::MessageAdded
            && event.metadata.get("role").map(String::as_str) == Some("tool")
    }));
}

#[test]
fn subagent_permission_rejects_allow_for_session() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = permission_run_context(0, 0);
    let request = subagent_patch_request("agent-perm-subagent", &run_context);
    seed_subagent_permission_run(&mut store, &run_context, &request);

    let state = super::resolution::resolve_subagent_permission_in_store(
        &mut store,
        &request,
        &PermissionDecision::AllowForSession,
        &run_context,
        "session-a",
    )
    .expect("state should load");

    assert_eq!(
        state.last_error.as_deref(),
        Some("this permission can only be allowed once")
    );
    // The request stays durably unresolved: a subagent patch approval can
    // never create a session capability.
    let audit = store
        .list_permission_audits()
        .expect("audits should load")
        .into_iter()
        .find(|audit| audit.request.id == request.id)
        .expect("audit should exist");
    assert!(audit.resolution.is_none());
}

#[test]
fn subagent_permission_rejects_a_request_outside_the_active_pending_set() {
    let mut store = SqliteStore::in_memory().expect("store should open");
    let run_context = permission_run_context(0, 0);
    let request = subagent_patch_request("agent-perm-subagent", &run_context);
    seed_subagent_permission_run(&mut store, &run_context, &request);
    // The pending approval belongs to a different run, so the active run's
    // pending set does not contain it.
    let mut other_run_request = subagent_patch_request("agent-perm-other", &run_context);
    other_run_request
        .metadata
        .insert("agent_run_id".to_string(), "run-b".to_string());
    store
        .save_permission_request(other_run_request.clone(), current_time_millis())
        .expect("request should save");

    let state = super::resolution::resolve_subagent_permission_in_store(
        &mut store,
        &other_run_request,
        &PermissionDecision::AllowOnce,
        &run_context,
        "session-a",
    )
    .expect("state should load");

    assert_eq!(
        state.last_error.as_deref(),
        Some("permission does not belong to the active session")
    );
    let audit = store
        .list_permission_audits()
        .expect("audits should load")
        .into_iter()
        .find(|audit| audit.request.id == other_run_request.id)
        .expect("audit should exist");
    assert!(audit.resolution.is_none());
}
