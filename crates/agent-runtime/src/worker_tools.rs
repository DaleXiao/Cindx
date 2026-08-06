use agent_core::{ToolEffectSemantics, ToolRisk, ToolSpec};

pub fn evidence_worker_tools(tools: &[ToolSpec]) -> Vec<ToolSpec> {
    tools
        .iter()
        .filter(|tool| {
            tool.risk == ToolRisk::ReadOnly
                && tool.effect_semantics == ToolEffectSemantics::ReadOnly
        })
        .cloned()
        .collect()
}

pub fn substantive_evidence_worker_tools(tools: &[ToolSpec]) -> Vec<ToolSpec> {
    evidence_worker_tools(tools)
        .into_iter()
        .filter(|tool| {
            !matches!(
                tool.name.as_str(),
                "file.list" | "browser.tabs" | "tool.search" | "tool.inspect"
            )
        })
        .collect()
}
