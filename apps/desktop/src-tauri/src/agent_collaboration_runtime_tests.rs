use super::*;
use crate::collaboration_service::CollaborationGroundingReceipt;

fn collaboration(execution_contract: Option<&str>) -> AgentCollaboration {
    AgentCollaboration {
        id: "collaboration-1".to_string(),
        policy: "adaptive".to_string(),
        guidance: "Use the verified team result.".to_string(),
        execution_contract: execution_contract.map(str::to_string),
        evidence_packet: None,
        grounding_receipts: Vec::new(),
    }
}

#[test]
fn collaboration_context_keeps_guidance_and_machine_contract_separate() {
    let mut history = Vec::new();
    append_agent_collaboration_context(
        &mut history,
        &collaboration(Some(r#"{"schema":"cindx.workflow-handoff.v1"}"#)),
    );

    assert_eq!(history.len(), 3);
    assert_eq!(
        history[1]
            .metadata
            .get("collaboration_stage")
            .map(String::as_str),
        Some("guidance")
    );
    assert_eq!(
        history[2].metadata.get("kind").map(String::as_str),
        Some("workflow_execution_contract")
    );
    assert_eq!(
        history[2].metadata.get("internal").map(String::as_str),
        Some("true")
    );
    assert_eq!(history[1].role, MessageRole::Reviewer);
    assert_eq!(history[2].role, MessageRole::Reviewer);
    assert!(history[2].content.contains("cindx.workflow-handoff.v1"));
    assert!(!history[1].content.contains("cindx.workflow-handoff.v1"));
}

#[test]
fn collaboration_context_omits_blank_machine_contract() {
    let mut history = Vec::new();
    append_agent_collaboration_context(&mut history, &collaboration(Some("  \n")));

    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1]
            .metadata
            .get("collaboration_stage")
            .map(String::as_str),
        Some("guidance")
    );
}

#[test]
fn collaboration_context_includes_bounded_candidate_evidence() {
    let mut collaboration = collaboration(None);
    collaboration.evidence_packet = Some(agent_runtime::AgentEvidencePacket::new(
        "question",
        [agent_runtime::AgentEvidenceCandidate::new(
            "worker-1",
            "reviewer",
            "completed",
            "Independent candidate",
        )],
    ));
    let mut history = Vec::new();
    append_agent_collaboration_context(&mut history, &collaboration);

    assert_eq!(history.len(), 3);
    assert_eq!(
        history[2].metadata.get("kind").map(String::as_str),
        Some("agent_evidence_packet")
    );
    assert_eq!(history[2].role, MessageRole::Reviewer);
    assert!(history[2].content.contains("Independent candidate"));
}

#[test]
fn collaboration_context_exposes_trusted_tool_observation_to_executor() {
    let mut collaboration = collaboration(None);
    collaboration
        .grounding_receipts
        .push(CollaborationGroundingReceipt {
            steer_epoch: 4,
            collaboration_id: collaboration.id.clone(),
            source_step: "inspect".to_string(),
            tool_call_id: "call-1".to_string(),
            tool_name: "file.read".to_string(),
            request: r#"{"path":"Cargo.toml"}"#.to_string(),
            input_fingerprint: "sha256".to_string(),
            observation: "version = 2".to_string(),
        });
    let mut history = Vec::new();

    append_agent_collaboration_context(&mut history, &collaboration);

    assert_eq!(history.len(), 3);
    assert_eq!(
        history[2].metadata.get("kind").map(String::as_str),
        Some("collaboration_tool_evidence")
    );
    assert_eq!(history[2].role, MessageRole::Reviewer);
    let evidence: serde_json::Value =
        serde_json::from_str(&history[2].content).expect("evidence is structured JSON");
    assert_eq!(evidence["observations"][0]["tool"], "file.read");
    assert_eq!(evidence["observations"][0]["observation"], "version = 2");
    assert!(!history[2].content.contains("Cargo.toml"));
}
