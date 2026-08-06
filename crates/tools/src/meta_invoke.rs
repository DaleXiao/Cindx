use agent_core::{
    PermissionRequest, ToolInvocation, ToolPostconditionEvidence, ToolResult, ToolRisk, ToolSpec,
};

use crate::meta_tools::meta_target;
use crate::{Tool, ToolError, ToolExecutionControl, ToolRegistry};

pub(super) struct ToolInvokeMeta {
    pub(super) catalog: ToolRegistry,
}

impl ToolInvokeMeta {
    pub(super) fn target_invocation(
        &self,
        invocation: &ToolInvocation,
    ) -> Result<ToolInvocation, ToolError> {
        let (name, arguments) = meta_target(&invocation.input_json)?;
        if name.starts_with("tool.") {
            return Err(ToolError::new("meta tools cannot invoke other meta tools"));
        }
        if self.catalog.get(&name).is_none() {
            return Err(ToolError::new(format!("unknown tool: {name}")));
        }
        Ok(ToolInvocation {
            id: invocation.id.clone(),
            task_id: invocation.task_id.clone(),
            tool_name: name,
            input_json: arguments.to_string(),
            proposed_by_model: invocation.proposed_by_model.clone(),
            metadata: invocation.metadata.clone(),
        })
    }
}

impl Tool for ToolInvokeMeta {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "tool.invoke",
            "meta",
            "Invoke a deferred tool by exact name with a JSON arguments object.",
            ToolRisk::SensitiveContext,
            agent_core::ToolSource::BuiltIn,
            agent_core::ToolExposure::Inline,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["name", "arguments"],
                "additionalProperties": false
            })
            .to_string(),
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let target = self.target_invocation(invocation).ok()?;
        self.catalog
            .get(&target.tool_name)
            .and_then(|tool| tool.permission_request(&target))
    }

    fn effect_spec(&self, invocation: &ToolInvocation) -> ToolSpec {
        self.target_invocation(invocation)
            .ok()
            .and_then(|target| {
                self.catalog
                    .get(&target.tool_name)
                    .map(|tool| tool.effect_spec(&target))
            })
            .unwrap_or_else(|| self.spec())
    }

    fn postcondition_evidence(
        &self,
        invocation: &ToolInvocation,
        result: &ToolResult,
    ) -> Option<ToolPostconditionEvidence> {
        crate::postcondition_evidence::delegated(self, invocation, result)
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let target = self.target_invocation(&invocation)?;
        self.catalog
            .get(&target.tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {}", target.tool_name)))?
            .execute(target)
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        let target = self.target_invocation(&invocation)?;
        self.catalog
            .get(&target.tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {}", target.tool_name)))?
            .execute_with_control(target, control)
    }
}
