use crate::grounding_policy::{prompt_evidence_scopes, PromptEvidenceScope};
use agent_core::{Metadata, ToolEffectSemantics, ToolSpec};
use std::collections::BTreeSet;

pub fn tool_matches_evidence_scope(tool: &ToolSpec, scope: PromptEvidenceScope) -> bool {
    if tool.effect_semantics != ToolEffectSemantics::ReadOnly
        || matches!(
            tool.name.as_str(),
            "file.list" | "browser.tabs" | "tool.search" | "tool.inspect"
        )
    {
        return false;
    }
    match scope {
        PromptEvidenceScope::Workspace => matches!(
            tool.name.as_str(),
            "file.read" | "file.read_many" | "file.search"
        ),
        PromptEvidenceScope::External => {
            matches!(tool.name.as_str(), "web.search" | "browser.extract_text")
        }
        PromptEvidenceScope::Browser => matches!(
            tool.name.as_str(),
            "browser.extract_text" | "browser.capture"
        ),
        PromptEvidenceScope::Visual => matches!(
            tool.name.as_str(),
            "browser.capture" | "computer.screenshot"
        ),
    }
}

pub fn pin_prompt_evidence_tools(
    run_context: &Metadata,
    catalog: &[ToolSpec],
    inline: &mut Vec<ToolSpec>,
) -> BTreeSet<PromptEvidenceScope> {
    let scopes = prompt_evidence_scopes(run_context);
    if scopes.is_empty() {
        return scopes;
    }
    let mut names = inline
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<BTreeSet<_>>();
    for scope in &scopes {
        if inline
            .iter()
            .any(|tool| tool_matches_evidence_scope(tool, *scope))
        {
            continue;
        }
        let preferred: &[&str] = match scope {
            PromptEvidenceScope::Workspace => &["file.read", "file.search", "file.read_many"],
            PromptEvidenceScope::External => &["web.search", "browser.extract_text"],
            PromptEvidenceScope::Browser => &["browser.extract_text", "browser.capture"],
            PromptEvidenceScope::Visual => &["computer.screenshot", "browser.capture"],
        };
        let candidate = preferred
            .iter()
            .find_map(|name| {
                catalog
                    .iter()
                    .find(|tool| tool.name == *name && tool_matches_evidence_scope(tool, *scope))
            })
            .or_else(|| {
                catalog
                    .iter()
                    .find(|tool| tool_matches_evidence_scope(tool, *scope))
            });
        if let Some(tool) = candidate {
            if names.insert(tool.name.clone()) {
                inline.push(tool.clone());
            }
        }
    }
    inline.sort_by(|left, right| left.name.cmp(&right.name));
    scopes
}
