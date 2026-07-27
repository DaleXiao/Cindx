use super::{Tool, ToolError, ToolRegistry};
use agent_core::{
    Metadata, PermissionRequest, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};

pub(super) struct ToolSearchMeta {
    pub(super) catalog: ToolRegistry,
}

impl Tool for ToolSearchMeta {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "tool.search",
            "meta",
            "Search the deferred Cindx tool catalog by query or namespace.",
            ToolRisk::ReadOnly,
            agent_core::ToolSource::BuiltIn,
            agent_core::ToolExposure::Inline,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "namespace": { "type": "string" }
                },
                "additionalProperties": false
            })
            .to_string(),
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input: serde_json::Value = serde_json::from_str(&invocation.input_json)
            .map_err(|error| ToolError::new(format!("invalid tool search input: {error}")))?;
        let query = input
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let namespace = input.get("namespace").and_then(serde_json::Value::as_str);
        let mut rows = self
            .catalog
            .specs()
            .into_iter()
            .filter(|spec| spec.namespace != "meta")
            .filter(|spec| namespace.is_none_or(|namespace| spec.namespace == namespace))
            .filter(|spec| {
                query.is_empty()
                    || format!("{} {} {}", spec.name, spec.namespace, spec.description)
                        .to_ascii_lowercase()
                        .contains(&query)
            })
            .map(|spec| format!("{}\t{}\t{}", spec.name, spec.namespace, spec.description))
            .collect::<Vec<_>>();
        rows.sort();
        rows.truncate(20);
        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            if rows.is_empty() {
                "No matching tools.".to_string()
            } else {
                rows.join("\n")
            },
            Metadata::new(),
        ))
    }
}

pub(super) struct ToolInspectMeta {
    pub(super) catalog: ToolRegistry,
}

impl Tool for ToolInspectMeta {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "tool.inspect",
            "meta",
            "Inspect one deferred tool's description and JSON schema before invoking it.",
            ToolRisk::ReadOnly,
            agent_core::ToolSource::BuiltIn,
            agent_core::ToolExposure::Inline,
            serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
                "additionalProperties": false
            })
            .to_string(),
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let (name, _) = meta_target(&invocation.input_json)?;
        let spec = self
            .catalog
            .get(&name)
            .map(Tool::spec)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {name}")))?;
        let output = serde_json::json!({
            "name": spec.name,
            "namespace": spec.namespace,
            "description": spec.description,
            "inputSchema": serde_json::from_str::<serde_json::Value>(&spec.input_schema_json).unwrap_or_default(),
            "risk": format!("{:?}", spec.risk),
        })
        .to_string();
        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            Metadata::new(),
        ))
    }
}

pub(super) struct ToolInvokeMeta {
    pub(super) catalog: ToolRegistry,
}

impl ToolInvokeMeta {
    fn target_invocation(&self, invocation: &ToolInvocation) -> Result<ToolInvocation, ToolError> {
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

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let target = self.target_invocation(&invocation)?;
        self.catalog
            .get(&target.tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {}", target.tool_name)))?
            .execute(target)
    }
}

fn meta_target(input: &str) -> Result<(String, serde_json::Value), ToolError> {
    let input: serde_json::Value = serde_json::from_str(input)
        .map_err(|error| ToolError::new(format!("invalid meta tool input: {error}")))?;
    let name = input
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| ToolError::new("meta tool requires a target name"))?
        .to_string();
    let arguments = input
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !arguments.is_object() {
        return Err(ToolError::new("meta tool arguments must be an object"));
    }
    Ok((name, arguments))
}
