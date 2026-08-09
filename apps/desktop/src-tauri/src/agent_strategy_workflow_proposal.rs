use super::PlannedAgentRun;
use crate::desktop_prelude::sha256_hex;
use agent_core::Metadata;

pub(super) fn apply_to_context(
    planned: &PlannedAgentRun,
    run_context: &mut Metadata,
) -> Result<(), String> {
    if let Some(workflow_plan) = planned.workflow_plan.as_ref() {
        let encoded = serde_json::to_string(workflow_plan)
            .map_err(|error| format!("workflow proposal serialization failed: {error}"))?;
        run_context.insert(
            "conductor_workflow_proposal_sha256".to_string(),
            sha256_hex(encoded.as_bytes()),
        );
        run_context.insert("conductor_workflow_proposal".to_string(), encoded);
        run_context.insert(
            "conductor_workflow_plan_source".to_string(),
            "run_decision".to_string(),
        );
    } else {
        run_context.remove("conductor_workflow_proposal_sha256");
        run_context.remove("conductor_workflow_proposal");
        run_context.remove("conductor_workflow_plan_source");
    }
    Ok(())
}
