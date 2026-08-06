use super::{AgentTaskContract, PostconditionVerificationReceipt};
use agent_core::{
    PostconditionVerifierKind, ToolEffectSemantics, ToolOutcomeStatus, ToolPostconditionEvidence,
    ToolRisk, ToolSpec,
};

pub(crate) const POSTCONDITION_SCOPE: &str = "contract-test-run:0";

pub(crate) fn record_workspace_mutation(
    contract: &mut AgentTaskContract,
    tool_name: &str,
    input_json: &str,
) {
    let spec = ToolSpec::builtin(
        tool_name,
        "test",
        "test workspace mutation",
        ToolRisk::WritesWorkspace,
        r#"{"type":"object"}"#,
    )
    .with_effect_semantics(ToolEffectSemantics::Verifiable {
        verifier: "workspace_file_content_v1".to_string(),
    });
    let _ = contract.record_tool_outcome_transition(
        0,
        0,
        tool_name,
        input_json,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::WritesWorkspace),
        Some(&spec),
        None,
        None,
        "workspace mutation completed",
        None,
        Some(POSTCONDITION_SCOPE),
        true,
    );
}

pub(crate) fn record_quality_verification(
    contract: &mut AgentTaskContract,
    tool_name: &str,
    input_json: &str,
) -> Option<PostconditionVerificationReceipt> {
    let spec = ToolSpec::builtin(
        tool_name,
        "test",
        "test quality verification",
        ToolRisk::ExecutesProcess,
        r#"{"type":"object"}"#,
    )
    .with_postcondition_verifier(PostconditionVerifierKind::WorkspaceQualityCheckV1);
    let evidence = ToolPostconditionEvidence {
        kind: PostconditionVerifierKind::WorkspaceQualityCheckV1,
        target_input_json: r#"{"path":"."}"#.to_string(),
    };
    contract.record_tool_outcome_transition(
        0,
        0,
        tool_name,
        input_json,
        &ToolOutcomeStatus::Succeeded,
        Some(&ToolRisk::ExecutesProcess),
        Some(&spec),
        Some(&evidence),
        None,
        "quality verification passed",
        None,
        Some(POSTCONDITION_SCOPE),
        true,
    )
}

pub(crate) fn record_interaction_transition(
    contract: &mut AgentTaskContract,
    tool_name: &str,
    input_json: &str,
    risk: ToolRisk,
) -> Option<PostconditionVerificationReceipt> {
    let spec = ToolSpec::builtin(
        tool_name,
        "test",
        "test interaction",
        risk.clone(),
        r#"{"type":"object"}"#,
    );
    contract.record_tool_outcome_transition(
        0,
        0,
        tool_name,
        input_json,
        &ToolOutcomeStatus::Succeeded,
        Some(&risk),
        Some(&spec),
        None,
        None,
        "interaction transition completed",
        None,
        None,
        true,
    )
}
