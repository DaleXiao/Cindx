use crate::{
    pin_prompt_evidence_tools, prompt_evidence_scopes, tool_matches_evidence_scope,
    PromptEvidenceScope,
};
use agent_core::{Metadata, ToolEffectSemantics, ToolRisk, ToolSpec};
use std::collections::BTreeSet;

fn run_context(objective: &str) -> Metadata {
    [(
        "effective_prompt_objective".to_string(),
        objective.to_string(),
    )]
    .into_iter()
    .collect()
}

fn read_tool(name: &str) -> ToolSpec {
    ToolSpec::builtin(
        name,
        "test",
        "test",
        ToolRisk::ReadOnly,
        r#"{"type":"object"}"#,
    )
    .with_effect_semantics(ToolEffectSemantics::ReadOnly)
}

#[test]
fn prompt_evidence_scope_distinguishes_workspace_external_and_visual_work() {
    for (objective, expected) in [
        (
            "Audit this repository for unsafe shell calls",
            PromptEvidenceScope::Workspace,
        ),
        (
            "Search the web for the latest Tokio release",
            PromptEvidenceScope::External,
        ),
        (
            "Look at the current app window and identify the error",
            PromptEvidenceScope::Visual,
        ),
    ] {
        assert_eq!(
            prompt_evidence_scopes(&run_context(objective)),
            BTreeSet::from([expected])
        );
    }
    assert!(prompt_evidence_scopes(&run_context("Explain how web search works")).is_empty());
}

#[test]
fn accepted_steer_can_replace_an_obsolete_grounding_requirement() {
    let context = [
        (
            "effective_prompt_objective".to_string(),
            "Initial request:\nAudit this repository\n\nAccepted steering 1:\nActually, just write a poem"
                .to_string(),
        ),
        (
            "prompt_objective".to_string(),
            "Actually, just write a poem".to_string(),
        ),
        ("steer_epoch".to_string(), "1".to_string()),
    ]
    .into_iter()
    .collect();

    assert!(prompt_evidence_scopes(&context).is_empty());
}

#[test]
fn pinning_adds_only_a_matching_read_only_evidence_tool() {
    let catalog = vec![
        read_tool("file.list"),
        read_tool("file.read_many"),
        read_tool("web.search"),
    ];
    let mut inline = Vec::new();

    let scopes =
        pin_prompt_evidence_tools(&run_context("Audit this repository"), &catalog, &mut inline);

    assert_eq!(scopes, BTreeSet::from([PromptEvidenceScope::Workspace]));
    assert_eq!(
        inline
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["file.read_many"]
    );
    assert!(tool_matches_evidence_scope(
        &inline[0],
        PromptEvidenceScope::Workspace
    ));
    assert!(!tool_matches_evidence_scope(
        &catalog[0],
        PromptEvidenceScope::Workspace
    ));
}
