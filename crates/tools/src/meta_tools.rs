use super::{Tool, ToolError, ToolRegistry};
use agent_core::{
    Metadata, PermissionRequest, ToolExecutionConcurrency, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSpec,
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
        .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
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
        .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
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
            "outputSchema": spec.output_schema_json
                .as_deref()
                .and_then(|schema| serde_json::from_str::<serde_json::Value>(schema).ok()),
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

pub(super) fn meta_target(input: &str) -> Result<(String, serde_json::Value), ToolError> {
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
