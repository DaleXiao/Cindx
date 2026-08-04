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
        (
            "Open https://example.com in the browser tool and inspect the rendered page",
            PromptEvidenceScope::Browser,
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
fn local_source_fields_do_not_become_external_evidence_requests() {
    let objective = "Inspect docs/requirements.md and config/service.json, then create out/migration-report.json with a sources field listing both authoritative input files.";

    assert_eq!(
        prompt_evidence_scopes(&run_context(objective)),
        BTreeSet::from([PromptEvidenceScope::Workspace])
    );

    assert_eq!(
        prompt_evidence_scopes(&run_context(
            "Inspect config/service.json and cite external sources for the proposed update",
        )),
        BTreeSet::from([
            PromptEvidenceScope::Workspace,
            PromptEvidenceScope::External,
        ])
    );

    assert_eq!(
        prompt_evidence_scopes(&run_context("Provide sources about Rust code")),
        BTreeSet::from([PromptEvidenceScope::External])
    );

    assert_eq!(
        prompt_evidence_scopes(&run_context(
            "Review README.md and provide sources for the security recommendations",
        )),
        BTreeSet::from([
            PromptEvidenceScope::Workspace,
            PromptEvidenceScope::External,
        ])
    );

    assert_eq!(
        prompt_evidence_scopes(&run_context(
            "Review README.md and provide sources from authoritative upstream project files",
        )),
        BTreeSet::from([
            PromptEvidenceScope::Workspace,
            PromptEvidenceScope::External,
        ])
    );
}

#[test]
fn dynamic_browser_class_requires_browser_evidence_without_rewriting_the_prompt() {
    let mut context = run_context("Investigate the rendered incident dashboard");
    context.insert("task_class".to_string(), "browser".to_string());

    assert_eq!(
        prompt_evidence_scopes(&context),
        BTreeSet::from([PromptEvidenceScope::Browser])
    );
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
