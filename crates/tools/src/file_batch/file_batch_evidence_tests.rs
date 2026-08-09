use std::fs;

use agent_core::{Metadata, PostconditionVerifierKind, TaskId, ToolCallId, ToolInvocation};

use crate::Tool;

use super::ReadFilesTool;

fn invocation(input: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        id: ToolCallId("batch-read-evidence".to_string()),
        task_id: TaskId("task".to_string()),
        tool_name: "file.read_many".to_string(),
        input_json: input.to_string(),
        proposed_by_model: "test".to_string(),
        metadata: Metadata::new(),
    }
}

#[test]
fn complete_batch_read_produces_exact_readback_evidence_for_every_path() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("one.txt"), "one").expect("write one");
    fs::write(root.path().join("two.txt"), "two").expect("write two");
    let tool = ReadFilesTool::new(root.path());
    let invocation = invocation(serde_json::json!({ "paths": ["one.txt", "two.txt"] }));
    let result = tool.execute(invocation.clone()).expect("batch read");

    let evidence = tool
        .postcondition_evidence(&invocation, &result)
        .expect("complete batch should prove exact readback");
    assert_eq!(
        evidence.kind,
        PostconditionVerifierKind::WorkspaceExactReadbackV1
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&evidence.target_input_json)
            .expect("evidence target should be JSON"),
        serde_json::json!({ "paths": ["one.txt", "two.txt"] })
    );
}

#[test]
fn partial_or_offset_batch_read_cannot_claim_exact_readback() {
    let root = tempfile::tempdir().expect("workspace");
    fs::write(root.path().join("long.txt"), "0123456789").expect("write fixture");
    let tool = ReadFilesTool::new(root.path());
    for input in [
        serde_json::json!({ "paths": ["long.txt"], "max_bytes_per_file": 3 }),
        serde_json::json!({ "paths": [{ "path": "long.txt", "offset_bytes": 2 }] }),
    ] {
        let invocation = invocation(input);
        let result = tool.execute(invocation.clone()).expect("batch read");
        assert!(tool.postcondition_evidence(&invocation, &result).is_none());
    }
}
