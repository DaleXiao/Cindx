use super::*;
use agent_core::{EventId, TaskId};
use orchestrator::{TaskClass, WorkspaceRetrievalChannel};

fn run_context(decision: AgentRunDecision) -> Metadata {
    [(
        "run_decision".to_string(),
        serde_json::to_string(&decision).expect("decision should serialize"),
    )]
    .into_iter()
    .collect()
}

fn successful_tool(tool: &str, path: &str) -> Event {
    Event {
        id: EventId("tool-event".to_string()),
        task_id: TaskId("task".to_string()),
        sequence: 1,
        timestamp_ms: 1,
        kind: EventKind::ToolCallFinished,
        summary: "Tool finished".to_string(),
        metadata: [
            ("tool".to_string(), tool.to_string()),
            ("status".to_string(), "succeeded".to_string()),
            ("result_path".to_string(), path.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn direct_general_text_run_skips_model_curation() {
    let context = run_context(AgentRunDecision::direct("model"));

    assert!(!semantic_memory_model_is_warranted(&context, &[]));
    assert!(!semantic_memory_model_is_warranted(
        &Metadata::new(),
        &[]
    ));
}

#[test]
fn workflow_or_retrieval_run_admits_model_curation() {
    let mut research = AgentRunDecision::direct("model");
    research.task_class = TaskClass::Research;
    assert!(!semantic_memory_model_is_warranted(
        &run_context(research.clone()),
        &[]
    ));
    research.execution = AgentExecutionMode::Workflow;
    assert!(semantic_memory_model_is_warranted(
        &run_context(research),
        &[]
    ));

    let mut retrieval = AgentRunDecision::direct("model");
    retrieval.retrieval.query = "project decision".to_string();
    retrieval
        .retrieval
        .channels
        .insert(WorkspaceRetrievalChannel::FileSearch);
    assert!(semantic_memory_model_is_warranted(
        &run_context(retrieval),
        &[]
    ));
}

#[test]
fn durable_effect_admits_curation_but_transient_read_does_not() {
    let context = run_context(AgentRunDecision::direct("model"));
    let mut planned_effect = AgentRunDecision::direct("model");
    planned_effect.tool_requirement = AgentToolRequirement::Effects;

    assert!(semantic_memory_model_is_warranted(
        &run_context(planned_effect),
        &[]
    ));
    assert!(semantic_memory_model_is_warranted(
        &context,
        &[successful_tool("file.write", "src/lib.rs")]
    ));
    assert!(!semantic_memory_model_is_warranted(
        &context,
        &[successful_tool("shell.run", "README.md")]
    ));
}
