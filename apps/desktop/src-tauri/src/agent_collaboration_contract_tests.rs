use super::*;
use agent_core::ToolEffectSemantics;
use agent_runtime::{start_agent_loop, AgentRuntimeConfig};
use orchestrator::{
    AdaptiveWorkflow, AdaptiveWorkflowStep, WorkflowBudget, WorkflowVerificationState,
    WORKFLOW_VERIFICATION_RECEIPT_SCHEMA,
};

fn test_tool(name: &str, semantics: ToolEffectSemantics) -> ToolSpec {
    ToolSpec::builtin(
        name,
        "test",
        "test collaboration tool",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    )
    .with_effect_semantics(semantics)
}

fn evidence(
    call_id: &str,
    epoch: u64,
    collaboration_id: &str,
    status: &str,
) -> CollaborationEvidence {
    CollaborationEvidence {
        evidence_schema: crate::collaboration_service::COLLABORATION_TOOL_EVIDENCE_SCHEMA
            .to_string(),
        steer_epoch: Some(epoch),
        collaboration_id: collaboration_id.to_string(),
        source_step: "untrusted-runtime-label".to_string(),
        tool_call_id: call_id.to_string(),
        tool_name: "file.read".to_string(),
        request: r#"{"path":"Cargo.toml"}"#.to_string(),
        status: status.to_string(),
        output: "workspace observation".to_string(),
    }
}

fn collaboration_receipt(request: &str) -> AgentCollaboration {
    AgentCollaboration {
        id: "collaboration-contract".to_string(),
        policy: "adaptive".to_string(),
        guidance: "grounded report".to_string(),
        execution_contract: None,
        evidence_packet: None,
        grounding_receipts: vec![crate::collaboration_service::CollaborationGroundingReceipt {
            steer_epoch: 4,
            collaboration_id: "collaboration-contract".to_string(),
            source_step: "inspect".to_string(),
            tool_call_id: "call-target".to_string(),
            tool_name: "file.read".to_string(),
            request: request.to_string(),
            input_fingerprint: agent_runtime::tool_input_fingerprint("file.read", request),
            observation: "permission implementation".to_string(),
        }],
    }
}

fn target_context() -> Metadata {
    [
        (
            "effective_prompt_objective".to_string(),
            "Audit permission.rs".to_string(),
        ),
        ("steer_epoch".to_string(), "4".to_string()),
    ]
    .into_iter()
    .collect()
}

#[test]
fn agent_collaboration_contract_gate() {
    let read = test_tool("file.read", ToolEffectSemantics::ReadOnly);
    let discovery = test_tool("file.list", ToolEffectSemantics::ReadOnly);
    let mislabeled = test_tool("unsafe.read", ToolEffectSemantics::NonIdempotent);
    let tools = vec![read.clone(), discovery, mislabeled];
    let exploration = evidence_worker_tools(&tools);
    let direct_evidence = substantive_evidence_worker_tools(&tools);
    assert!(exploration.iter().any(|tool| tool.name == "file.read"));
    assert!(exploration.iter().any(|tool| tool.name == "file.list"));
    assert!(!exploration.iter().any(|tool| tool.name == "unsafe.read"));
    assert!(direct_evidence.iter().any(|tool| tool.name == "file.read"));
    assert!(!direct_evidence.iter().any(|tool| tool.name == "file.list"));

    let mut stale = evidence("stale", 3, "collaboration-1", "succeeded");
    let mut foreign = evidence("foreign", 4, "collaboration-2", "succeeded");
    let mut legacy = evidence("legacy", 4, "collaboration-1", "succeeded");
    legacy.evidence_schema.clear();
    let failed = evidence("failed", 4, "collaboration-1", "failed");
    let valid = evidence("valid", 4, "collaboration-1", "succeeded");
    stale.source_step = "root".to_string();
    foreign.source_step = "root".to_string();
    let inherited = BTreeMap::from([(
        "root".to_string(),
        vec![stale, foreign, legacy, failed, valid],
    )]);
    let projected = merge_collaboration_evidence(
        "verify",
        &["root".to_string()],
        &inherited,
        &[],
        "collaboration-1",
        4,
    );
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].source_step, "root");
    assert_eq!(projected[0].tool_call_id, "valid");

    let workflow = AdaptiveWorkflow {
        steps: vec![
            AdaptiveWorkflowStep {
                id: "root".to_string(),
                role: "worker".to_string(),
                model: "model-a".to_string(),
                subtask: "inspect the implementation".to_string(),
                access: Vec::new(),
            },
            AdaptiveWorkflowStep {
                id: "verify".to_string(),
                role: "verifier".to_string(),
                model: "model-a".to_string(),
                subtask: "verify the work".to_string(),
                access: vec!["root".to_string()],
            },
            AdaptiveWorkflowStep {
                id: "final".to_string(),
                role: "synthesizer".to_string(),
                model: "model-a".to_string(),
                subtask: "integrate the result".to_string(),
                access: vec!["root".to_string(), "verify".to_string()],
            },
        ],
    };
    let plan = WorkflowPlanIr::from_adaptive(
        "verification-contract",
        "Audit the implementation",
        "pro",
        "adaptive",
        "model-a",
        &workflow,
        WorkflowBudget {
            max_steps: 3,
            max_models: 1,
            max_model_turns_per_step: 3,
            max_tool_calls_per_step: 4,
            max_output_tokens_per_step: 2_048,
        },
    );
    let mut checkpoint = WorkflowExecutionCheckpoint::new("resume", plan, 1);
    checkpoint
        .complete_step(
            "root",
            "model-a",
            "root output".to_string(),
            "[]".to_string(),
            2,
        )
        .unwrap();
    checkpoint
        .complete_step(
            "verify",
            "model-a",
            "prose only".to_string(),
            "[]".to_string(),
            3,
        )
        .unwrap();
    assert_eq!(
        checkpoint.steps["verify"].semantic.verification,
        WorkflowVerificationState::Inconclusive
    );
    assert!(!checkpoint.workflow_verification_satisfied(true));

    assert!(
        !crate::adaptive_collaboration_finalization::owner_handoff_guidance_admitted(true, false)
    );

    let output = format!(
        "audit complete\nCINDX_VERIFICATION: {{\"schema\":\"{WORKFLOW_VERIFICATION_RECEIPT_SCHEMA}\",\"verdict\":\"passed\",\"reviewed_steps\":[\"root\"],\"evidence_refs\":[],\"unresolved\":[]}}"
    );
    let receipt = WorkflowVerificationReceipt::from_worker_output(&output)
        .expect("typed verification receipt should parse");
    checkpoint
        .complete_step_with_evidence(
            "verify",
            "model-a",
            output,
            "[]".to_string(),
            WorkflowEvidenceSummary::default(),
            Some(receipt),
            4,
        )
        .unwrap();
    assert_eq!(
        checkpoint.steps["verify"].semantic.verification,
        WorkflowVerificationState::Passed
    );
    assert!(checkpoint.workflow_verification_satisfied(true));
    assert!(
        crate::adaptive_collaboration_finalization::owner_handoff_guidance_admitted(true, true)
    );

    let runtime_tools = vec![read];
    let mut wrong_runtime = start_agent_loop(
        TaskId("wrong-collaboration-target".to_string()),
        "Audit permission.rs",
        AgentRuntimeConfig::default(),
    );
    let wrong = collaboration_receipt(r#"{"path":"README.md"}"#);
    apply_run_task_contract(
        &mut wrong_runtime,
        &target_context(),
        &runtime_tools,
        Some(&wrong),
    )
    .unwrap();
    assert!(AgentKernel::new(&mut wrong_runtime, &runtime_tools)
        .completion_gate_for_task()
        .unwrap()
        .is_some());

    let mut correct_runtime = start_agent_loop(
        TaskId("correct-collaboration-target".to_string()),
        "Audit permission.rs",
        AgentRuntimeConfig::default(),
    );
    let correct = collaboration_receipt(
        r#"{"path":"apps/desktop/src-tauri/src/permission.rs"}"#,
    );
    apply_run_task_contract(
        &mut correct_runtime,
        &target_context(),
        &runtime_tools,
        Some(&correct),
    )
    .unwrap();
    assert_eq!(
        AgentKernel::new(&mut correct_runtime, &runtime_tools).completion_gate_for_task(),
        Ok(None)
    );

    println!("cindx.agent-collaboration-contract.v1");
}
