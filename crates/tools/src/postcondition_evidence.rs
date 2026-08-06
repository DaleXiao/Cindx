use agent_core::{
    PostconditionVerifierKind, ToolInvocation, ToolOutcomeStatus, ToolPostconditionEvidence,
    ToolResult,
};

use crate::meta_invoke::ToolInvokeMeta;
use crate::parse_input;

pub(crate) fn file_read(
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<ToolPostconditionEvidence> {
    if result.invocation_id != invocation.id
        || !matches!(result.status, ToolOutcomeStatus::Succeeded)
    {
        return None;
    }
    let input = parse_input(&invocation.input_json);
    let requested_path = input.get("path")?.trim();
    if requested_path.is_empty() {
        return None;
    }
    let structured =
        serde_json::from_str::<serde_json::Value>(result.structured_output_json.as_deref()?)
            .ok()?;
    let path = structured.get("path")?.as_str()?;
    let returned_bytes = structured.get("returned_bytes")?.as_u64()?;
    let total_bytes = structured.get("total_bytes")?.as_u64()?;
    if structured.get("schema")?.as_str()? != "cindx.file-read-result.v1"
        || path != requested_path
        || structured.get("offset_bytes")?.as_u64()? != 0
        || structured.get("truncated")?.as_bool()?
        || returned_bytes != total_bytes
    {
        return None;
    }
    Some(ToolPostconditionEvidence {
        kind: PostconditionVerifierKind::WorkspaceExactReadbackV1,
        target_input_json: serde_json::json!({ "path": path }).to_string(),
    })
}

pub(crate) fn delegated(
    tool: &ToolInvokeMeta,
    invocation: &ToolInvocation,
    result: &ToolResult,
) -> Option<ToolPostconditionEvidence> {
    let target = tool.target_invocation(invocation).ok()?;
    tool.catalog
        .get(&target.tool_name)?
        .postcondition_evidence(&target, result)
}
