//! Contract tests for anchor-matched absence evidence and its effect on
//! prompt-scoped grounding and any-tool obligations.
use super::*;

fn bind_workspace_readme_anchor(contract: &mut AgentTaskContract, epoch: u64) {
    contract.bind_prompt_evidence_targets(
        epoch,
        [(
            "workspace_grounding".to_string(),
            [crate::EvidenceTargetAnchor::Workspace(
                "readme.md".to_string(),
            )]
            .into_iter()
            .collect(),
        )]
        .into_iter()
        .collect(),
    );
}

#[test]
fn prompt_evidence_absence_satisfies_anchor_matched_failed_reads() {
    let mut contract = AgentTaskContract::default();
    let tools = vec![tool("file.read", ToolRisk::ReadOnly)];
    contract.replace_prompt_evidence_requirement(2, Some("workspace_grounding"), ["file.read"]);
    bind_workspace_readme_anchor(&mut contract, 2);

    assert!(!contract.record_prompt_tool_absence_observation_at(
        2,
        "file.read",
        "file.read",
        r#"{"path":"unrelated.md"}"#,
        "tool=file.read\nstatus=failed\noutput=\nfile not found",
    ));
    assert!(contract
        .completion_instruction_for_task(&tools)
        .expect("completion gate evaluates")
        .is_some());

    assert!(contract.record_prompt_tool_absence_observation_at(
        2,
        "file.read",
        "file.read",
        r#"{"path":"README.md"}"#,
        "tool=file.read\nstatus=failed\noutput=\nREADME.md does not exist",
    ));
    assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));

    let contexts = contract.prompt_evidence_contexts();
    assert_eq!(contexts.len(), 1);
    assert!(contexts[0].absent);
    assert!(contexts[0].observation.contains("does not exist"));
}

#[test]
fn prompt_any_tool_obligation_accepts_anchor_matched_absence() {
    let mut contract = AgentTaskContract::default();
    let tools = vec![tool("file.read", ToolRisk::ReadOnly)];
    contract.replace_prompt_required_any_tool_successes(
        2,
        [(
            "conductor_read_evidence".to_string(),
            ["file.read".to_string()].into_iter().collect(),
        )]
        .into_iter()
        .collect(),
    );
    contract.replace_prompt_evidence_requirement(2, Some("workspace_grounding"), ["file.read"]);
    bind_workspace_readme_anchor(&mut contract, 2);

    assert!(contract
        .completion_instruction_for_task(&tools)
        .expect("completion gate evaluates")
        .is_some());

    assert!(contract.record_prompt_tool_absence_observation_at(
        2,
        "file.read",
        "file.read",
        r#"{"path":"README.md"}"#,
        "tool=file.read\nstatus=failed\noutput=\nREADME.md does not exist",
    ));
    assert_eq!(contract.completion_instruction_for_task(&tools), Ok(None));
}

#[test]
fn prompt_evidence_absence_never_grants_a_free_pass() {
    let mut contract = AgentTaskContract::default();
    contract.replace_prompt_evidence_requirement(2, Some("workspace_grounding"), ["file.read"]);

    assert!(!contract.record_prompt_tool_absence_observation_at(
        2,
        "file.read",
        "file.read",
        r#"{"path":"README.md"}"#,
        "tool=file.read\nstatus=failed\noutput=\nmissing",
    ));

    bind_workspace_readme_anchor(&mut contract, 2);
    assert!(!contract.record_prompt_tool_absence_observation_at(
        9,
        "file.read",
        "file.read",
        r#"{"path":"README.md"}"#,
        "tool=file.read\nstatus=failed\noutput=\nmissing",
    ));
    assert!(!contract.record_prompt_tool_absence_observation_at(
        2,
        "file.search",
        "file.search",
        r#"{"query":"README"}"#,
        "tool=file.search\nstatus=failed\noutput=\nnothing",
    ));
    assert!(!contract.record_prompt_tool_absence_observation_at(
        2,
        "file.read",
        "file.read",
        r#"{"path":"README.md"}"#,
        "   ",
    ));
}
