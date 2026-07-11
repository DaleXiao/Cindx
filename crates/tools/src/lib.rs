use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::{
    Metadata, PermissionRequest, PermissionRequestId, PermissionRisk, TaskId, ToolCallId,
    ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ToolError {}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest>;

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError>;
}

#[derive(Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

#[derive(Debug, Clone)]
pub struct ToolExposurePlan {
    pub inline: Vec<ToolSpec>,
    pub deferred: Vec<ToolSpec>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn with_workspace_tools(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        let mut registry = Self::new();
        registry.register(Box::new(ReadFileTool::new(workspace_root.clone())));
        registry.register(Box::new(ListDirectoryTool::new(workspace_root.clone())));
        registry.register(Box::new(SearchFilesTool::new(workspace_root.clone())));
        registry.register(Box::new(WriteFileTool::new(workspace_root.clone())));
        registry.register(Box::new(ShellRunTool::new(workspace_root.clone())));
        registry.register(Box::new(WebSearchTool));
        registry.register(Box::new(BrowserOpenTool));
        registry.register(Box::new(BrowserExtractTextTool));
        registry.register(Box::new(BrowserCaptureTool::new(workspace_root.clone())));
        registry.register(Box::new(BrowserActionTool::click(workspace_root.clone())));
        registry.register(Box::new(BrowserActionTool::type_text(workspace_root.clone())));
        registry.register(Box::new(BrowserActionTool::scroll(workspace_root.clone())));
        registry.register(Box::new(ComputerTool::screenshot(workspace_root.clone())));
        registry.register(Box::new(ComputerTool::click(workspace_root.clone())));
        registry.register(Box::new(ComputerTool::type_text(workspace_root.clone())));
        registry.register(Box::new(ComputerTool::key(workspace_root.clone())));
        registry.register(Box::new(ComputerTool::scroll(workspace_root)));
        registry
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.spec().name.clone(), Arc::from(tool));
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|tool| tool.as_ref())
    }

    pub fn install_meta_tools(&mut self) {
        if self.tools.contains_key("tool.search") {
            return;
        }
        let catalog = self.clone();
        self.register(Box::new(ToolSearchMeta {
            catalog: catalog.clone(),
        }));
        self.register(Box::new(ToolInspectMeta {
            catalog: catalog.clone(),
        }));
        self.register(Box::new(ToolInvokeMeta { catalog }));
    }

    pub fn exposure_plan(&self, prompt: &str, context_window: u64) -> ToolExposurePlan {
        let mut candidates = self
            .tools
            .values()
            .map(|tool| tool.spec())
            .filter(|spec| spec.namespace != "meta")
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.name.cmp(&right.name));

        let auto_count = candidates
            .iter()
            .filter(|spec| matches!(spec.exposure, agent_core::ToolExposure::Auto))
            .count();
        let max_auto = if context_window < 32_000 { 10 } else { 24 };
        if auto_count <= max_auto {
            return ToolExposurePlan {
                inline: candidates,
                deferred: Vec::new(),
            };
        }

        let query = prompt.to_ascii_lowercase();
        candidates.sort_by(|left, right| {
            tool_relevance(right, &query)
                .cmp(&tool_relevance(left, &query))
                .then(left.name.cmp(&right.name))
        });
        let mut inline = Vec::new();
        let mut deferred = Vec::new();
        let mut auto_inline = 0usize;
        for spec in candidates {
            match spec.exposure {
                agent_core::ToolExposure::Inline => inline.push(spec),
                agent_core::ToolExposure::Deferred => deferred.push(spec),
                agent_core::ToolExposure::Auto if auto_inline < max_auto => {
                    auto_inline += 1;
                    inline.push(spec);
                }
                agent_core::ToolExposure::Auto => deferred.push(spec),
            }
        }
        if !deferred.is_empty() {
            for name in ["tool.search", "tool.inspect", "tool.invoke"] {
                if let Some(tool) = self.tools.get(name) {
                    inline.push(tool.spec());
                }
            }
        }
        inline.sort_by(|left, right| left.name.cmp(&right.name));
        deferred.sort_by(|left, right| left.name.cmp(&right.name));
        ToolExposurePlan { inline, deferred }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn tool_relevance(spec: &ToolSpec, query: &str) -> usize {
    let mut score = 0;
    for token in query.split(|character: char| !character.is_alphanumeric()) {
        if token.len() < 3 {
            continue;
        }
        if spec.name.to_ascii_lowercase().contains(token) {
            score += 4;
        }
        if spec.namespace.to_ascii_lowercase().contains(token) {
            score += 3;
        }
        if spec.description.to_ascii_lowercase().contains(token) {
            score += 1;
        }
    }
    score
}

struct ToolSearchMeta {
    catalog: ToolRegistry,
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
        let namespace = input
            .get("namespace")
            .and_then(serde_json::Value::as_str);
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

struct ToolInspectMeta {
    catalog: ToolRegistry,
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

struct ToolInvokeMeta {
    catalog: ToolRegistry,
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

pub struct ReadFileTool {
    workspace_root: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "file.read",
            "Read a UTF-8 file inside the workspace.",
            ToolRisk::ReadOnly,
            "path=<workspace-relative-path>",
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let path = required_input(&input, "path")?;
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        let resolved = resolve_workspace_read_path(&self.workspace_root, &resolved)?;
        reject_sensitive_read_path(&self.workspace_root, &resolved)?;
        let output = fs::read_to_string(&resolved)
            .map_err(|error| ToolError::new(format!("failed to read file: {error}")))?;
        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path);
        metadata.insert("bytes".to_string(), output.len().to_string());

        Ok(tool_result(invocation.id, ToolOutcomeStatus::Succeeded, output, metadata))
    }
}

pub struct ListDirectoryTool {
    workspace_root: PathBuf,
}

impl ListDirectoryTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for ListDirectoryTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "file.list",
            "List files and directories inside the workspace.",
            ToolRisk::ReadOnly,
            "path=<optional workspace-relative-path>",
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let path = input
            .get("path")
            .cloned()
            .unwrap_or_else(|| ".".to_string());
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        let resolved = resolve_workspace_read_path(&self.workspace_root, &resolved)?;
        let mut rows = Vec::new();
        let entries = fs::read_dir(&resolved)
            .map_err(|error| ToolError::new(format!("failed to list directory: {error}")))?;

        for entry in entries {
            let entry =
                entry.map_err(|error| ToolError::new(format!("failed to read directory entry: {error}")))?;
            let metadata = entry
                .metadata()
                .map_err(|error| ToolError::new(format!("failed to read metadata: {error}")))?;
            let kind = if metadata.is_dir() { "dir" } else { "file" };
            rows.push(format!(
                "{}\t{}\t{}",
                kind,
                metadata.len(),
                entry.file_name().to_string_lossy()
            ));
        }
        rows.sort();

        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path);
        metadata.insert("entries".to_string(), rows.len().to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            rows.join("\n"),
            metadata,
        ))
    }
}

pub struct SearchFilesTool {
    workspace_root: PathBuf,
}

impl SearchFilesTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "file.search",
            "Search UTF-8 files inside the workspace for a literal query.",
            ToolRisk::ReadOnly,
            "query=<literal text>\npath=<optional workspace-relative path>\nmax_results=<optional number>",
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let query = required_input(&input, "query")?;
        let path = input
            .get("path")
            .cloned()
            .unwrap_or_else(|| ".".to_string());
        let max_results = input
            .get("max_results")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(50)
            .min(200);
        let root = resolve_workspace_path(&self.workspace_root, &path)?;
        let root = resolve_workspace_read_path(&self.workspace_root, &root)?;
        reject_sensitive_read_path(&self.workspace_root, &root)?;
        let mut results = Vec::new();
        search_directory(&self.workspace_root, &root, &query, max_results, &mut results)?;

        let mut metadata = Metadata::new();
        metadata.insert("query".to_string(), query);
        metadata.insert("path".to_string(), path);
        metadata.insert("matches".to_string(), results.len().to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            results.join("\n"),
            metadata,
        ))
    }
}

pub struct WriteFileTool {
    workspace_root: PathBuf,
}

impl WriteFileTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "file.write",
            "Write UTF-8 content to a file inside the workspace.",
            ToolRisk::WritesWorkspace,
            "path=<workspace-relative-path>\ncontent=<utf-8 content>",
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let path = input
            .get("path")
            .cloned()
            .unwrap_or_else(|| "<missing path>".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Write,
            "file.write",
            "Write a file in the selected workspace.",
            &path,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let path = required_input(&input, "path")?;
        let content = required_input(&input, "content")?;
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| ToolError::new(format!("failed to create parent directory: {error}")))?;
        }
        fs::write(&resolved, content.as_bytes())
            .map_err(|error| ToolError::new(format!("failed to write file: {error}")))?;

        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path);
        metadata.insert("bytes".to_string(), content.len().to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            "file written".to_string(),
            metadata,
        ))
    }
}

pub struct ShellRunTool {
    workspace_root: PathBuf,
}

impl ShellRunTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for ShellRunTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "shell.run",
            "Run a shell command in the workspace.",
            ToolRisk::ExecutesProcess,
            "command=<shell command>\ncwd=<optional workspace-relative path>",
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let command = input
            .get("command")
            .cloned()
            .unwrap_or_else(|| "<missing command>".to_string());
        let cwd = input.get("cwd").cloned().unwrap_or_else(|| ".".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Execute,
            "shell.run",
            "Run a local process in the selected workspace.",
            &cwd,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
                ("command".to_string(), command),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let command = required_input(&input, "command")?;
        let cwd = input.get("cwd").cloned().unwrap_or_else(|| ".".to_string());
        let resolved_cwd = resolve_workspace_path(&self.workspace_root, &cwd)?;
        let resolved_cwd = resolve_workspace_read_path(&self.workspace_root, &resolved_cwd)?;
        let output = Command::new("/bin/zsh")
            .arg("-lc")
            .arg(&command)
            .current_dir(&resolved_cwd)
            .output()
            .map_err(|error| ToolError::new(format!("failed to run shell command: {error}")))?;

        let mut combined = String::new();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&String::from_utf8_lossy(&output.stderr));
        }

        let mut metadata = Metadata::new();
        metadata.insert("command".to_string(), command);
        metadata.insert("cwd".to_string(), cwd);
        metadata.insert(
            "exit_code".to_string(),
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
        );

        Ok(tool_result(
            invocation.id,
            if output.status.success() {
                ToolOutcomeStatus::Succeeded
            } else {
                ToolOutcomeStatus::Failed
            },
            combined,
            metadata,
        ))
    }
}

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "web.search",
            "Search the web through a lightweight HTML endpoint.",
            ToolRisk::UsesNetwork,
            "query=<search query>\nmax_results=<optional number>",
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let query = input
            .get("query")
            .cloned()
            .unwrap_or_else(|| "<missing query>".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Network,
            "web.search",
            "Search the public web.",
            &query,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let query = required_input(&input, "query")?;
        let max_results = input
            .get("max_results")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(8)
            .clamp(1, 20);
        let url = format!("https://duckduckgo.com/html/?q={}", url_encode(&query));
        let html = fetch_url(&url)?;
        let text = trim_lines(&html_to_text(&html), max_results * 4);

        let mut metadata = Metadata::new();
        metadata.insert("query".to_string(), query);
        metadata.insert("url".to_string(), url);
        metadata.insert("max_results".to_string(), max_results.to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            text,
            metadata,
        ))
    }
}

pub struct BrowserOpenTool;

impl Tool for BrowserOpenTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "browser.open",
            "Open a URL in the system browser.",
            ToolRisk::UsesNetwork,
            "url=<http-or-https-url>",
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        browser_permission_request(invocation, "browser.open", "Open a URL in the system browser.")
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let url = required_url(&input)?;
        let output = Command::new("/usr/bin/open")
            .arg(&url)
            .output()
            .map_err(|error| ToolError::new(format!("failed to open browser: {error}")))?;
        let mut metadata = Metadata::new();
        metadata.insert("url".to_string(), url.clone());
        metadata.insert(
            "exit_code".to_string(),
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
        );

        Ok(tool_result(
            invocation.id,
            if output.status.success() {
                ToolOutcomeStatus::Succeeded
            } else {
                ToolOutcomeStatus::Failed
            },
            if output.status.success() {
                format!("opened {url}")
            } else {
                String::from_utf8_lossy(&output.stderr).to_string()
            },
            metadata,
        ))
    }
}

pub struct BrowserExtractTextTool;

impl Tool for BrowserExtractTextTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "browser.extract_text",
            "Fetch a webpage and extract readable text.",
            ToolRisk::UsesNetwork,
            "url=<http-or-https-url>",
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        browser_permission_request(invocation, "browser.extract_text", "Fetch and read a webpage.")
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let url = required_url(&input)?;
        let html = fetch_url(&url)?;
        let text = html_to_text(&html);
        let output = truncate_chars(&text, 12_000);
        let mut metadata = Metadata::new();
        metadata.insert("url".to_string(), url);
        metadata.insert("chars".to_string(), output.chars().count().to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            metadata,
        ))
    }
}

pub struct BrowserCaptureTool {
    workspace_root: PathBuf,
}

impl BrowserCaptureTool {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl Tool for BrowserCaptureTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "browser.capture",
            "Capture a webpage screenshot when Playwright is available, with HTML/text fallback.",
            ToolRisk::UsesNetwork,
            "url=<http-or-https-url>\noutput_dir=<optional workspace-relative directory>",
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        browser_permission_request(
            invocation,
            "browser.capture",
            "Capture a webpage observation artifact.",
        )
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let url = required_url(&input)?;
        let output_dir = input
            .get("output_dir")
            .cloned()
            .unwrap_or_else(|| ".cindx/browser-captures".to_string());
        let resolved_dir = resolve_workspace_path(&self.workspace_root, &output_dir)?;
        fs::create_dir_all(&resolved_dir)
            .map_err(|error| ToolError::new(format!("failed to create capture directory: {error}")))?;

        let capture_id = format!("capture-{}-{}", current_time_millis(), stable_hash(&url));
        let screenshot_relative = format!("{output_dir}/{capture_id}.png");
        let screenshot_path = resolve_workspace_path(&self.workspace_root, &screenshot_relative)?;
        let mut capture_kind = "html_snapshot".to_string();
        let mut artifact_relative = format!("{output_dir}/{capture_id}.html");

        if let Some(playwright) = find_playwright(&self.workspace_root) {
            let screenshot = Command::new(playwright)
                .arg("screenshot")
                .arg("--full-page")
                .arg(&url)
                .arg(&screenshot_path)
                .output();
            if matches!(screenshot, Ok(output) if output.status.success()) {
                capture_kind = "screenshot".to_string();
                artifact_relative = screenshot_relative;
            }
        }

        let html = fetch_url(&url)?;
        let text = html_to_text(&html);
        if capture_kind == "html_snapshot" {
            let html_path = resolve_workspace_path(&self.workspace_root, &artifact_relative)?;
            fs::write(&html_path, html.as_bytes())
                .map_err(|error| ToolError::new(format!("failed to write HTML snapshot: {error}")))?;
        }
        let text_relative = format!("{output_dir}/{capture_id}.txt");
        let text_path = resolve_workspace_path(&self.workspace_root, &text_relative)?;
        fs::write(&text_path, text.as_bytes())
            .map_err(|error| ToolError::new(format!("failed to write text snapshot: {error}")))?;

        let mut metadata = Metadata::new();
        metadata.insert("url".to_string(), url);
        metadata.insert("capture_kind".to_string(), capture_kind.clone());
        metadata.insert("artifact_path".to_string(), artifact_relative.clone());
        metadata.insert("text_path".to_string(), text_relative.clone());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            format!(
                "{capture_kind}\nartifact={artifact_relative}\ntext={text_relative}\n\n{}",
                truncate_chars(&text, 4_000)
            ),
            metadata,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserActionKind {
    Click,
    TypeText,
    Scroll,
}

impl BrowserActionKind {
    fn tool_name(self) -> &'static str {
        match self {
            Self::Click => "browser.click",
            Self::TypeText => "browser.type",
            Self::Scroll => "browser.scroll",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::TypeText => "type",
            Self::Scroll => "scroll",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Click => "Click a selector or screen coordinate through the browser sidecar.",
            Self::TypeText => "Type text into a browser target through the browser sidecar.",
            Self::Scroll => "Scroll the current browser page through the browser sidecar.",
        }
    }

    fn schema(self) -> &'static str {
        match self {
            Self::Click => {
                "url=<optional http-or-https-url>\nselector=<css selector or accessible target>\nx=<optional coordinate>\ny=<optional coordinate>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::TypeText => {
                "url=<optional http-or-https-url>\nselector=<css selector or accessible target>\ntext=<text to type>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::Scroll => {
                "url=<optional http-or-https-url>\ndelta_x=<optional pixels>\ndelta_y=<optional pixels, default 600>\noutput_dir=<optional workspace-relative directory>"
            }
        }
    }

    fn risk(self) -> ToolRisk {
        match self {
            Self::TypeText => ToolRisk::SensitiveContext,
            Self::Click | Self::Scroll => ToolRisk::UsesNetwork,
        }
    }

    fn permission_risk(self) -> PermissionRisk {
        match self {
            Self::TypeText => PermissionRisk::Sensitive,
            Self::Click | Self::Scroll => PermissionRisk::Network,
        }
    }

    fn permission_reason(self) -> &'static str {
        match self {
            Self::Click => "Click inside a browser session.",
            Self::TypeText => "Enter potentially sensitive text into a browser session.",
            Self::Scroll => "Scroll inside a browser session.",
        }
    }
}

pub struct BrowserActionTool {
    workspace_root: PathBuf,
    kind: BrowserActionKind,
}

impl BrowserActionTool {
    fn click(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: BrowserActionKind::Click,
        }
    }

    fn type_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: BrowserActionKind::TypeText,
        }
    }

    fn scroll(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: BrowserActionKind::Scroll,
        }
    }
}

impl Tool for BrowserActionTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            self.kind.tool_name(),
            self.kind.description(),
            self.kind.risk(),
            self.kind.schema(),
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let scope = browser_action_scope(self.kind, &input);
        Some(permission_request(
            &invocation.task_id,
            self.kind.permission_risk(),
            self.kind.tool_name(),
            self.kind.permission_reason(),
            &scope,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
                ("tool_input".to_string(), invocation.input_json.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        validate_browser_action_input(self.kind, &input)?;
        if let Some(url) = input.get("url").filter(|value| !value.trim().is_empty()) {
            required_url(&[("url".to_string(), url.clone())].into_iter().collect())?;
        }

        let output_dir = input
            .get("output_dir")
            .cloned()
            .unwrap_or_else(|| ".cindx/browser-actions".to_string());
        let resolved_dir = resolve_workspace_path(&self.workspace_root, &output_dir)?;
        fs::create_dir_all(&resolved_dir)
            .map_err(|error| ToolError::new(format!("failed to create browser action directory: {error}")))?;

        let action_id = format!(
            "browser-{}-{}",
            current_time_millis(),
            stable_hash(&invocation.input_json)
        );
        let request_relative = format!("{output_dir}/{action_id}.json");
        let request_path = resolve_workspace_path(&self.workspace_root, &request_relative)?;
        let request_json = browser_action_request_json(&action_id, self.kind, &input);
        fs::write(&request_path, request_json.as_bytes())
            .map_err(|error| ToolError::new(format!("failed to write browser action request: {error}")))?;

        let sidecar_output = run_json_sidecar("CINDX_BROWSER_SIDECAR", &request_path)?;
        let controller = if sidecar_output.is_some() {
            "sidecar"
        } else {
            "artifact"
        };
        let mut metadata = Metadata::new();
        metadata.insert("action".to_string(), self.kind.action().to_string());
        metadata.insert("artifact_path".to_string(), request_relative.clone());
        metadata.insert("controller".to_string(), controller.to_string());
        if let Some(url) = input.get("url") {
            metadata.insert("url".to_string(), url.clone());
        }
        if let Some(selector) = input.get("selector") {
            metadata.insert("selector".to_string(), selector.clone());
        }

        let output = sidecar_output.unwrap_or_else(|| {
            format!(
                "browser action queued for sidecar\ncontroller=artifact\naction={}\nrequest={request_relative}",
                self.kind.action()
            )
        });

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            metadata,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComputerActionKind {
    Screenshot,
    Click,
    TypeText,
    Key,
    Scroll,
}

impl ComputerActionKind {
    fn tool_name(self) -> &'static str {
        match self {
            Self::Screenshot => "computer.screenshot",
            Self::Click => "computer.click",
            Self::TypeText => "computer.type",
            Self::Key => "computer.key",
            Self::Scroll => "computer.scroll",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Click => "click",
            Self::TypeText => "type",
            Self::Key => "key",
            Self::Scroll => "scroll",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Screenshot => "Capture a local desktop screenshot artifact with redaction metadata.",
            Self::Click => "Click a local desktop coordinate through the computer-use controller.",
            Self::TypeText => "Type text through the computer-use controller.",
            Self::Key => "Press a keyboard shortcut through the computer-use controller.",
            Self::Scroll => "Scroll through the computer-use controller.",
        }
    }

    fn schema(self) -> &'static str {
        match self {
            Self::Screenshot => {
                "output_dir=<optional workspace-relative directory>\nredaction=<optional none|manual|sensitive_regions>\nregion=<optional x,y,width,height>"
            }
            Self::Click => {
                "x=<screen x>\ny=<screen y>\nbutton=<optional left|right>\ndestructive=<optional true|false>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::TypeText => {
                "text=<text to type>\ndestructive=<optional true|false>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::Key => {
                "key=<key or shortcut, e.g. Enter or Cmd+S>\ndestructive=<optional true|false>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::Scroll => {
                "delta_x=<optional pixels>\ndelta_y=<optional pixels, default 600>\noutput_dir=<optional workspace-relative directory>"
            }
        }
    }

    fn risk(self) -> ToolRisk {
        match self {
            Self::Key => ToolRisk::Destructive,
            Self::Screenshot | Self::Click | Self::TypeText | Self::Scroll => {
                ToolRisk::SensitiveContext
            }
        }
    }
}

pub struct ComputerTool {
    workspace_root: PathBuf,
    kind: ComputerActionKind,
}

impl ComputerTool {
    fn screenshot(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Screenshot,
        }
    }

    fn click(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Click,
        }
    }

    fn type_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::TypeText,
        }
    }

    fn key(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Key,
        }
    }

    fn scroll(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Scroll,
        }
    }
}

impl Tool for ComputerTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            self.kind.tool_name(),
            self.kind.description(),
            self.kind.risk(),
            self.kind.schema(),
        )
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let destructive = input_is_true(&input, "destructive");
        let risk = if destructive || self.kind == ComputerActionKind::Key {
            PermissionRisk::Destructive
        } else {
            PermissionRisk::Sensitive
        };
        Some(permission_request(
            &invocation.task_id,
            risk,
            self.kind.tool_name(),
            computer_permission_reason(self.kind, destructive),
            &computer_action_scope(self.kind, &input),
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
                ("tool_input".to_string(), invocation.input_json.clone()),
                ("destructive".to_string(), destructive.to_string()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        validate_computer_action_input(self.kind, &input)?;
        let output_dir = input
            .get("output_dir")
            .cloned()
            .unwrap_or_else(|| ".cindx/computer-actions".to_string());
        let resolved_dir = resolve_workspace_path(&self.workspace_root, &output_dir)?;
        fs::create_dir_all(&resolved_dir)
            .map_err(|error| ToolError::new(format!("failed to create computer action directory: {error}")))?;

        let action_id = format!(
            "computer-{}-{}",
            current_time_millis(),
            stable_hash(&invocation.input_json)
        );
        let request_relative = format!("{output_dir}/{action_id}.json");
        let request_path = resolve_workspace_path(&self.workspace_root, &request_relative)?;
        let request_json = computer_action_request_json(&action_id, self.kind, &input);
        fs::write(&request_path, request_json.as_bytes())
            .map_err(|error| ToolError::new(format!("failed to write computer action request: {error}")))?;

        if self.kind == ComputerActionKind::Screenshot {
            return execute_computer_screenshot(
                invocation.id,
                &self.workspace_root,
                &output_dir,
                &action_id,
                &request_relative,
                &request_path,
                &input,
            );
        }

        let sidecar_output = run_json_sidecar("CINDX_COMPUTER_SIDECAR", &request_path)?;
        let controller = if sidecar_output.is_some() {
            "sidecar"
        } else {
            "artifact"
        };
        let mut metadata = Metadata::new();
        metadata.insert("action".to_string(), self.kind.action().to_string());
        metadata.insert("artifact_path".to_string(), request_relative.clone());
        metadata.insert("controller".to_string(), controller.to_string());
        metadata.insert(
            "destructive".to_string(),
            input_is_true(&input, "destructive").to_string(),
        );

        let output = sidecar_output.unwrap_or_else(|| {
            format!(
                "computer action queued for sidecar\ncontroller=artifact\naction={}\nrequest={request_relative}",
                self.kind.action()
            )
        });

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            metadata,
        ))
    }
}

fn builtin_tool_spec(
    name: &str,
    description: &str,
    risk: ToolRisk,
    legacy_schema: &str,
) -> ToolSpec {
    let namespace = name.split('.').next().unwrap_or("builtin");
    ToolSpec::builtin(
        name,
        namespace,
        description,
        risk,
        object_schema_from_fields(legacy_schema),
    )
}

fn object_schema_from_fields(fields: &str) -> String {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for row in fields.lines() {
        let Some((name, descriptor)) = row.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let description = descriptor
            .trim()
            .trim_matches(|character| character == '<' || character == '>');
        if name.is_empty() {
            continue;
        }
        let value_type = match name {
            "destructive" => "boolean",
            "x" | "y" | "delta_x" | "delta_y" | "limit" | "max_results" => "integer",
            _ => "string",
        };
        properties.insert(
            name.to_string(),
            serde_json::json!({
                "type": value_type,
                "description": description,
            }),
        );
        if !description.to_ascii_lowercase().contains("optional") {
            required.push(serde_json::Value::String(name.to_string()));
        }
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
    .to_string()
}

pub fn parse_input(input: &str) -> BTreeMap<String, String> {
    if let Ok(serde_json::Value::Object(values)) = serde_json::from_str(input) {
        if values.len() == 1 {
            if let Some(serde_json::Value::String(legacy)) = values.get("input") {
                return parse_input(legacy);
            }
        }
        return values
            .into_iter()
            .map(|(key, value)| {
                let value = match value {
                    serde_json::Value::String(value) => value,
                    other => other.to_string(),
                };
                (key, value)
            })
            .collect();
    }

    let mut parsed = BTreeMap::new();
    let mut current_key: Option<String> = None;
    for line in input.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            current_key = Some(key.clone());
            parsed.insert(key, value.to_string());
        } else if let Some(key) = current_key.as_ref() {
            if let Some(value) = parsed.get_mut(key) {
                value.push('\n');
                value.push_str(line);
            }
        }
    }
    parsed
}

pub fn encode_input(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(key, value)| format!("{key}={}", value.replace('\r', "")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn required_input(input: &BTreeMap<String, String>, key: &str) -> Result<String, ToolError> {
    input
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| ToolError::new(format!("missing required input: {key}")))
}

fn permission_request(
    task_id: &TaskId,
    risk: PermissionRisk,
    action: &str,
    reason: &str,
    scope: &str,
    metadata: Metadata,
) -> PermissionRequest {
    PermissionRequest {
        id: PermissionRequestId(format!("perm-{}", stable_hash(&format!("{action}:{scope:?}")))),
        task_id: task_id.clone(),
        risk,
        action: action.to_string(),
        reason: reason.to_string(),
        scope: scope.to_string(),
        metadata,
    }
}

fn tool_result(
    invocation_id: ToolCallId,
    status: ToolOutcomeStatus,
    output: String,
    metadata: Metadata,
) -> ToolResult {
    ToolResult::text(invocation_id, status, output, metadata)
}

fn resolve_workspace_path(workspace_root: &Path, path: &str) -> Result<PathBuf, ToolError> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(ToolError::new("absolute paths are not allowed"));
    }

    for component in candidate.components() {
        if matches!(component, Component::ParentDir | Component::Prefix(_) | Component::RootDir) {
            return Err(ToolError::new("path escapes the workspace"));
        }
    }

    let canonical_root = fs::canonicalize(workspace_root)
        .map_err(|error| ToolError::new(format!("failed to resolve workspace: {error}")))?;
    let resolved = workspace_root.join(candidate);
    let mut existing_ancestor = resolved.as_path();
    let canonical_ancestor = loop {
        match fs::symlink_metadata(existing_ancestor) {
            Ok(_) => {
                break fs::canonicalize(existing_ancestor).map_err(|error| {
                    ToolError::new(format!("failed to resolve workspace path: {error}"))
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing_ancestor = existing_ancestor
                    .parent()
                    .ok_or_else(|| ToolError::new("path escapes the workspace"))?;
            }
            Err(error) => {
                return Err(ToolError::new(format!(
                    "failed to inspect workspace path: {error}"
                )));
            }
        }
    };
    if !canonical_ancestor.starts_with(&canonical_root) {
        return Err(ToolError::new(
            "path escapes the workspace through a symbolic link",
        ));
    }

    Ok(resolved)
}

fn resolve_workspace_read_path(workspace_root: &Path, path: &Path) -> Result<PathBuf, ToolError> {
    let canonical_root = fs::canonicalize(workspace_root)
        .map_err(|error| ToolError::new(format!("failed to resolve workspace: {error}")))?;
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| ToolError::new(format!("failed to resolve read path: {error}")))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ToolError::new("path escapes the workspace through a symbolic link"));
    }
    Ok(canonical_path)
}

fn reject_sensitive_read_path(workspace_root: &Path, path: &Path) -> Result<(), ToolError> {
    if is_sensitive_workspace_path(workspace_root, path) {
        Err(ToolError::new(
            "access to local credential files is blocked; configure providers in Settings",
        ))
    } else {
        Ok(())
    }
}

fn is_sensitive_workspace_path(workspace_root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(workspace_root).unwrap_or(path);
    let normalized = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let joined = normalized.join("/");
    if joined == ".cindx/provider.conf" || joined.ends_with("/.cindx/provider.conf") {
        return true;
    }

    let Some(file_name) = normalized.last().map(String::as_str) else {
        return false;
    };
    let is_env_file = (file_name == ".env" || file_name.starts_with(".env."))
        && !file_name.ends_with(".example")
        && !file_name.ends_with(".sample")
        && !file_name.ends_with(".template");
    is_env_file
        || matches!(
            file_name,
            ".npmrc"
                | ".pypirc"
                | "credentials"
                | "credentials.json"
                | "id_rsa"
                | "id_ed25519"
        )
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
}

fn search_directory(
    workspace_root: &Path,
    directory: &Path,
    query: &str,
    max_results: usize,
    results: &mut Vec<String>,
) -> Result<(), ToolError> {
    if results.len() >= max_results {
        return Ok(());
    }

    let metadata = fs::metadata(directory)
        .map_err(|error| ToolError::new(format!("failed to read search path: {error}")))?;
    if metadata.is_file() {
        search_file(workspace_root, directory, query, max_results, results)?;
        return Ok(());
    }

    let entries = fs::read_dir(directory)
        .map_err(|error| ToolError::new(format!("failed to search directory: {error}")))?;
    for entry in entries {
        if results.len() >= max_results {
            break;
        }
        let entry =
            entry.map_err(|error| ToolError::new(format!("failed to read directory entry: {error}")))?;
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name.to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| ToolError::new(format!("failed to read file type: {error}")))?;
        if file_type.is_symlink() {
            continue;
        }
        let metadata =
            entry.metadata().map_err(|error| ToolError::new(format!("failed to read metadata: {error}")))?;
        if metadata.is_dir() {
            search_directory(workspace_root, &path, query, max_results, results)?;
        } else if metadata.is_file() {
            search_file(workspace_root, &path, query, max_results, results)?;
        }
    }

    Ok(())
}

fn search_file(
    workspace_root: &Path,
    path: &Path,
    query: &str,
    max_results: usize,
    results: &mut Vec<String>,
) -> Result<(), ToolError> {
    if results.len() >= max_results {
        return Ok(());
    }
    if is_sensitive_workspace_path(workspace_root, path) {
        return Ok(());
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(());
    };
    let relative = path.strip_prefix(workspace_root).unwrap_or(path);
    for (index, line) in content.lines().enumerate() {
        if results.len() >= max_results {
            break;
        }
        if line.contains(query) {
            results.push(format!(
                "{}:{}:{}",
                relative.display(),
                index + 1,
                line.trim()
            ));
        }
    }
    Ok(())
}

fn browser_permission_request(
    invocation: &ToolInvocation,
    action: &str,
    reason: &str,
) -> Option<PermissionRequest> {
    let input = parse_input(&invocation.input_json);
    let url = input
        .get("url")
        .cloned()
        .unwrap_or_else(|| "<missing url>".to_string());
    Some(permission_request(
        &invocation.task_id,
        PermissionRisk::Network,
        action,
        reason,
        &url,
        [
            ("tool_call_id".to_string(), invocation.id.0.clone()),
            ("tool_name".to_string(), invocation.tool_name.clone()),
        ]
        .into_iter()
        .collect(),
    ))
}

fn browser_action_scope(kind: BrowserActionKind, input: &BTreeMap<String, String>) -> String {
    let url = input
        .get("url")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "current browser page".to_string());
    let target = input
        .get("selector")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| {
            let x = input.get("x")?;
            let y = input.get("y")?;
            Some(format!("x={x},y={y}"))
        })
        .unwrap_or_else(|| "viewport".to_string());

    format!("{} on {url} at {target}", kind.action())
}

fn validate_browser_action_input(
    kind: BrowserActionKind,
    input: &BTreeMap<String, String>,
) -> Result<(), ToolError> {
    match kind {
        BrowserActionKind::Click => {
            if input.get("selector").is_some_and(|value| !value.trim().is_empty()) {
                return Ok(());
            }
            if input.get("x").is_some_and(|value| !value.trim().is_empty())
                && input.get("y").is_some_and(|value| !value.trim().is_empty())
            {
                return Ok(());
            }
            Err(ToolError::new("browser.click requires selector or x/y"))
        }
        BrowserActionKind::TypeText => {
            required_input(input, "text")?;
            if input.get("selector").is_some_and(|value| !value.trim().is_empty()) {
                return Ok(());
            }
            if input.get("x").is_some_and(|value| !value.trim().is_empty())
                && input.get("y").is_some_and(|value| !value.trim().is_empty())
            {
                return Ok(());
            }
            Err(ToolError::new("browser.type requires selector or x/y"))
        }
        BrowserActionKind::Scroll => Ok(()),
    }
}

fn input_is_true(input: &BTreeMap<String, String>, key: &str) -> bool {
    input
        .get(key)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "y"
            )
        })
        .unwrap_or(false)
}

fn browser_action_request_json(
    action_id: &str,
    kind: BrowserActionKind,
    input: &BTreeMap<String, String>,
) -> String {
    let mut fields = vec![
        json_field("id", action_id),
        json_field("namespace", "browser"),
        json_field("action", kind.action()),
    ];
    for key in [
        "url",
        "selector",
        "text",
        "x",
        "y",
        "delta_x",
        "delta_y",
    ] {
        if let Some(value) = input.get(key).filter(|value| !value.trim().is_empty()) {
            fields.push(json_field(key, value));
        }
    }
    if kind == BrowserActionKind::Scroll && !input.contains_key("delta_y") {
        fields.push(json_field("delta_y", "600"));
    }

    format!("{{{}}}\n", fields.join(","))
}

fn computer_permission_reason(kind: ComputerActionKind, destructive: bool) -> &'static str {
    if destructive {
        return "Perform a desktop action marked as destructive.";
    }
    match kind {
        ComputerActionKind::Screenshot => "Capture the local desktop for visual inspection.",
        ComputerActionKind::Click => "Click inside a local desktop app.",
        ComputerActionKind::TypeText => "Type text into a local desktop app.",
        ComputerActionKind::Key => "Press a keyboard shortcut in a local desktop app.",
        ComputerActionKind::Scroll => "Scroll inside a local desktop app.",
    }
}

fn computer_action_scope(kind: ComputerActionKind, input: &BTreeMap<String, String>) -> String {
    match kind {
        ComputerActionKind::Screenshot => input
            .get("region")
            .filter(|value| !value.trim().is_empty())
            .map(|region| format!("screenshot region {region}"))
            .unwrap_or_else(|| "full desktop screenshot".to_string()),
        ComputerActionKind::Click => {
            let x = input.get("x").cloned().unwrap_or_else(|| "?".to_string());
            let y = input.get("y").cloned().unwrap_or_else(|| "?".to_string());
            format!("click at x={x}, y={y}")
        }
        ComputerActionKind::TypeText => "type into focused desktop field".to_string(),
        ComputerActionKind::Key => input
            .get("key")
            .map(|key| format!("press {key}"))
            .unwrap_or_else(|| "press keyboard shortcut".to_string()),
        ComputerActionKind::Scroll => {
            let x = input
                .get("delta_x")
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            let y = input
                .get("delta_y")
                .cloned()
                .unwrap_or_else(|| "600".to_string());
            format!("scroll delta_x={x}, delta_y={y}")
        }
    }
}

fn validate_computer_action_input(
    kind: ComputerActionKind,
    input: &BTreeMap<String, String>,
) -> Result<(), ToolError> {
    match kind {
        ComputerActionKind::Screenshot => Ok(()),
        ComputerActionKind::Click => {
            required_i64(input, "x")?;
            required_i64(input, "y")?;
            Ok(())
        }
        ComputerActionKind::TypeText => {
            required_input(input, "text")?;
            Ok(())
        }
        ComputerActionKind::Key => {
            required_input(input, "key")?;
            Ok(())
        }
        ComputerActionKind::Scroll => {
            optional_i64(input, "delta_x")?;
            optional_i64(input, "delta_y")?;
            Ok(())
        }
    }
}

fn required_i64(input: &BTreeMap<String, String>, key: &str) -> Result<i64, ToolError> {
    let value = required_input(input, key)?;
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| ToolError::new(format!("{key} must be an integer")))
}

fn optional_i64(input: &BTreeMap<String, String>, key: &str) -> Result<Option<i64>, ToolError> {
    let Some(value) = input.get(key).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    value
        .trim()
        .parse::<i64>()
        .map(Some)
        .map_err(|_| ToolError::new(format!("{key} must be an integer")))
}

fn computer_action_request_json(
    action_id: &str,
    kind: ComputerActionKind,
    input: &BTreeMap<String, String>,
) -> String {
    let mut fields = vec![
        json_field("id", action_id),
        json_field("namespace", "computer"),
        json_field("action", kind.action()),
    ];
    for key in [
        "x",
        "y",
        "button",
        "text",
        "key",
        "delta_x",
        "delta_y",
        "region",
        "destructive",
        "redaction",
    ] {
        if let Some(value) = input.get(key).filter(|value| !value.trim().is_empty()) {
            fields.push(json_field(key, value));
        }
    }
    if kind == ComputerActionKind::Scroll && !input.contains_key("delta_y") {
        fields.push(json_field("delta_y", "600"));
    }
    if kind == ComputerActionKind::Screenshot && !input.contains_key("redaction") {
        fields.push(json_field("redaction", "manual"));
    }

    format!("{{{}}}\n", fields.join(","))
}

fn execute_computer_screenshot(
    invocation_id: ToolCallId,
    workspace_root: &Path,
    output_dir: &str,
    action_id: &str,
    request_relative: &str,
    request_path: &Path,
    input: &BTreeMap<String, String>,
) -> Result<ToolResult, ToolError> {
    let screenshot_relative = format!("{output_dir}/{action_id}.png");
    let screenshot_path = resolve_workspace_path(workspace_root, &screenshot_relative)?;
    let redaction_manifest_relative = format!("{output_dir}/{action_id}.redaction.json");
    let redaction_manifest_path =
        resolve_workspace_path(workspace_root, &redaction_manifest_relative)?;
    let redaction = input
        .get("redaction")
        .cloned()
        .unwrap_or_else(|| "manual".to_string());
    let sidecar_output = run_json_sidecar("CINDX_COMPUTER_SIDECAR", request_path)?;
    let (controller, artifact_relative, output) = if let Some(output) = sidecar_output {
        ("sidecar".to_string(), screenshot_relative.clone(), output)
    } else if try_native_screenshot(&screenshot_path)? {
        (
            "native_macos".to_string(),
            screenshot_relative.clone(),
            format!("desktop screenshot captured\nscreenshot={screenshot_relative}"),
        )
    } else {
        let placeholder_relative = format!("{output_dir}/{action_id}.txt");
        let placeholder_path = resolve_workspace_path(workspace_root, &placeholder_relative)?;
        fs::write(
            &placeholder_path,
            "Screenshot request recorded. Enable screen recording permission, install a sidecar, or set CINDX_COMPUTER_SIDECAR to capture pixels.\n",
        )
        .map_err(|error| ToolError::new(format!("failed to write screenshot placeholder: {error}")))?;
        (
            "artifact".to_string(),
            placeholder_relative.clone(),
            format!("desktop screenshot request recorded\nartifact={placeholder_relative}"),
        )
    };

    let manifest = format!(
        "{{{},{},{},{}}}\n",
        json_field("screenshot_request", request_relative),
        json_field("artifact_path", &artifact_relative),
        json_field("redaction", &redaction),
        json_field("status", "pending_review")
    );
    fs::write(&redaction_manifest_path, manifest.as_bytes())
        .map_err(|error| ToolError::new(format!("failed to write redaction manifest: {error}")))?;

    let mut metadata = Metadata::new();
    metadata.insert("action".to_string(), "screenshot".to_string());
    metadata.insert("artifact_path".to_string(), request_relative.to_string());
    metadata.insert("screenshot_path".to_string(), artifact_relative);
    metadata.insert(
        "redaction_manifest_path".to_string(),
        redaction_manifest_relative,
    );
    metadata.insert("redaction".to_string(), redaction);
    metadata.insert("controller".to_string(), controller);

    Ok(tool_result(
        invocation_id,
        ToolOutcomeStatus::Succeeded,
        output,
        metadata,
    ))
}

fn try_native_screenshot(path: &Path) -> Result<bool, ToolError> {
    if env::var("CINDX_DISABLE_NATIVE_SCREENSHOT")
        .map(|value| input_value_is_true(&value))
        .unwrap_or(false)
    {
        return Ok(false);
    }
    let output = Command::new("/usr/sbin/screencapture")
        .arg("-x")
        .arg(path)
        .output();
    match output {
        Ok(output) => Ok(output.status.success()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ToolError::new(format!("failed to run screencapture: {error}"))),
    }
}

fn input_value_is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}

fn required_url(input: &BTreeMap<String, String>) -> Result<String, ToolError> {
    let url = required_input(input, "url")?;
    let normalized = url.trim().to_string();
    if normalized.contains('\n') || normalized.contains('\r') {
        return Err(ToolError::new("url must be a single line"));
    }
    if !(normalized.starts_with("https://") || normalized.starts_with("http://")) {
        return Err(ToolError::new("url must start with http:// or https://"));
    }

    Ok(normalized)
}

fn fetch_url(url: &str) -> Result<String, ToolError> {
    let output = Command::new("/usr/bin/curl")
        .arg("-L")
        .arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg("25")
        .arg("--user-agent")
        .arg("LocalAgent/0.1")
        .arg(url)
        .output()
        .map_err(|error| ToolError::new(format!("failed to run curl: {error}")))?;

    if !output.status.success() {
        return Err(ToolError::new(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn html_to_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    let mut entity = String::new();
    let mut in_entity = false;

    for character in html.chars() {
        if in_tag {
            if character == '>' {
                in_tag = false;
                text.push(' ');
            }
            continue;
        }
        if in_entity {
            if character == ';' {
                text.push_str(&decode_entity(&entity));
                entity.clear();
                in_entity = false;
            } else if entity.len() < 12 {
                entity.push(character);
            } else {
                text.push('&');
                text.push_str(&entity);
                entity.clear();
                in_entity = false;
                text.push(character);
            }
            continue;
        }

        match character {
            '<' => in_tag = true,
            '&' => in_entity = true,
            _ => text.push(character),
        }
    }
    if in_entity {
        text.push('&');
        text.push_str(&entity);
    }

    collapse_whitespace(&text)
}

fn decode_entity(entity: &str) -> String {
    match entity {
        "amp" => "&".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "quot" => "\"".to_string(),
        "apos" | "#39" => "'".to_string(),
        "nbsp" => " ".to_string(),
        other if other.starts_with("#x") => u32::from_str_radix(&other[2..], 16)
            .ok()
            .and_then(char::from_u32)
            .map(|value| value.to_string())
            .unwrap_or_default(),
        other if other.starts_with('#') => other[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|value| value.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn collapse_whitespace(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_space = true;
    for character in value.chars() {
        if character == '\n' {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            previous_was_space = true;
        } else if character.is_whitespace() {
            if !previous_was_space {
                output.push(' ');
                previous_was_space = true;
            }
        } else {
            output.push(character);
            previous_was_space = false;
        }
    }
    output.trim().to_string()
}

fn trim_lines(value: &str, max_lines: usize) -> String {
    value
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn url_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn json_field(key: &str, value: &str) -> String {
    format!("\"{}\":\"{}\"", json_escape(key), json_escape(value))
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other if other.is_control() => escaped.push_str(&format!("\\u{:04x}", other as u32)),
            other => escaped.push(other),
        }
    }
    escaped
}

fn run_json_sidecar(env_key: &str, request_path: &Path) -> Result<Option<String>, ToolError> {
    let Ok(sidecar) = env::var(env_key) else {
        return Ok(None);
    };
    if sidecar.trim().is_empty() {
        return Ok(None);
    }

    let sidecar_path = PathBuf::from(&sidecar);
    let mut command = if sidecar_path.extension().and_then(|extension| extension.to_str())
        == Some("js")
    {
        let mut command = Command::new(env::var("CINDX_NODE").unwrap_or_else(|_| "node".to_string()));
        command.arg(&sidecar_path);
        command
    } else {
        Command::new(&sidecar_path)
    };
    let output = command
        .arg(request_path)
        .output()
        .map_err(|error| ToolError::new(format!("failed to run sidecar: {error}")))?;
    if !output.status.success() {
        return Err(ToolError::new(format!(
            "sidecar failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(Some(if stdout.is_empty() {
        "sidecar executed action".to_string()
    } else {
        stdout
    }))
}

fn find_playwright(workspace_root: &Path) -> Option<PathBuf> {
    if let Ok(path) = env::var("CINDX_PLAYWRIGHT") {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    [
        workspace_root.join("node_modules").join(".bin").join("playwright"),
        workspace_root
            .join("apps")
            .join("desktop")
            .join("node_modules")
            .join(".bin")
            .join("playwright"),
    ]
    .into_iter()
    .find(|candidate| candidate.exists())
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_millis() as u64
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct CatalogTool {
        name: String,
    }

    impl Tool for CatalogTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::builtin(
                self.name.clone(),
                "catalog",
                format!("Catalog tool {}", self.name),
                ToolRisk::ReadOnly,
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
            )
        }

        fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
            None
        }

        fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Succeeded,
                self.name.clone(),
                Metadata::new(),
            ))
        }
    }

    fn temp_workspace() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cindx-tools-test-{}",
            stable_hash(&format!("{:?}", std::time::SystemTime::now()))
        ));
        fs::create_dir_all(&path).expect("workspace should be created");
        path
    }

    fn invocation(tool_name: &str, input_json: String) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId("call-1".to_string()),
            task_id: TaskId("task-1".to_string()),
            tool_name: tool_name.to_string(),
            input_json,
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn registry_returns_workspace_tool_specs() {
        let registry = ToolRegistry::with_workspace_tools(temp_workspace());
        let specs = registry.specs();

        assert!(specs.iter().any(|spec| spec.name == "file.read"));
        assert!(specs.iter().any(|spec| spec.name == "file.write"));
        assert!(specs.iter().any(|spec| spec.name == "shell.run"));
        assert!(specs.iter().any(|spec| spec.name == "web.search"));
        assert!(specs.iter().any(|spec| spec.name == "browser.capture"));
        assert!(specs.iter().any(|spec| spec.name == "browser.click"));
        assert!(specs.iter().any(|spec| spec.name == "browser.type"));
        assert!(specs.iter().any(|spec| spec.name == "browser.scroll"));
        assert!(specs.iter().any(|spec| spec.name == "computer.screenshot"));
        assert!(specs.iter().any(|spec| spec.name == "computer.click"));
        assert!(specs.iter().any(|spec| spec.name == "computer.type"));
        assert!(specs.iter().any(|spec| spec.name == "computer.key"));
        assert!(specs.iter().any(|spec| spec.name == "computer.scroll"));
        assert!(specs
            .iter()
            .all(|spec| spec.validate_input_schema().is_ok()));
    }

    #[test]
    fn large_catalog_defers_tools_and_injects_meta_tools() {
        let mut registry = ToolRegistry::new();
        for index in 0..30 {
            registry.register(Box::new(CatalogTool {
                name: format!("catalog.tool_{index}"),
            }));
        }
        registry.install_meta_tools();
        let plan = registry.exposure_plan("use catalog tool 29", 16_000);

        assert_eq!(plan.deferred.len(), 20);
        assert!(plan.inline.iter().any(|spec| spec.name == "tool.search"));
        assert!(plan.inline.iter().any(|spec| spec.name == "tool.inspect"));
        assert!(plan.inline.iter().any(|spec| spec.name == "tool.invoke"));
    }

    #[test]
    fn meta_invoke_preserves_target_permission_gate() {
        let root = temp_workspace();
        let mut registry = ToolRegistry::with_workspace_tools(root);
        registry.install_meta_tools();
        let invocation = invocation(
            "tool.invoke",
            serde_json::json!({
                "name": "file.write",
                "arguments": { "path": "note.txt", "content": "hello" }
            })
            .to_string(),
        );
        let request = registry
            .get("tool.invoke")
            .unwrap()
            .permission_request(&invocation)
            .expect("write target should require permission");

        assert_eq!(request.action, "file.write");
        assert_eq!(request.metadata.get("tool_name").map(String::as_str), Some("file.write"));
    }

    #[test]
    fn read_list_search_and_write_file_inside_workspace() {
        let root = temp_workspace();
        let writer = WriteFileTool::new(root.clone());
        let reader = ReadFileTool::new(root.clone());
        let lister = ListDirectoryTool::new(root.clone());
        let searcher = SearchFilesTool::new(root);

        writer
            .execute(invocation(
                "file.write",
                encode_input(&[("path", "notes/today.txt"), ("content", "hello workspace\nline two")]),
            ))
            .expect("write should succeed");

        let read = reader
            .execute(invocation(
                "file.read",
                encode_input(&[("path", "notes/today.txt")]),
            ))
            .expect("read should succeed");
        let listed = lister
            .execute(invocation("file.list", encode_input(&[("path", "notes")])))
            .expect("list should succeed");
        let searched = searcher
            .execute(invocation(
                "file.search",
                encode_input(&[("path", "."), ("query", "workspace")]),
            ))
            .expect("search should succeed");

        assert_eq!(read.output, "hello workspace\nline two");
        assert!(listed.output.contains("today.txt"));
        assert!(searched.output.contains("notes/today.txt:1"));
    }

    #[test]
    fn write_and_shell_request_permission() {
        let root = temp_workspace();
        let writer = WriteFileTool::new(root.clone());
        let shell = ShellRunTool::new(root);

        assert_eq!(
            writer
                .permission_request(&invocation(
                    "file.write",
                    encode_input(&[("path", "a.txt"), ("content", "a")])
                ))
                .expect("write should request permission")
                .risk,
            PermissionRisk::Write
        );
        assert_eq!(
            shell
                .permission_request(&invocation(
                    "shell.run",
                    encode_input(&[("command", "echo ok")])
                ))
                .expect("shell should request permission")
                .risk,
            PermissionRisk::Execute
        );
    }

    #[test]
    fn network_and_browser_tools_request_permission() {
        let web = WebSearchTool;
        let browser = BrowserExtractTextTool;
        let browser_type = BrowserActionTool::type_text(temp_workspace());
        let computer_screenshot = ComputerTool::screenshot(temp_workspace());
        let computer_key = ComputerTool::key(temp_workspace());

        assert_eq!(
            web.permission_request(&invocation(
                "web.search",
                encode_input(&[("query", "local agent")])
            ))
            .expect("web search should request permission")
            .risk,
            PermissionRisk::Network
        );
        assert_eq!(
            browser
                .permission_request(&invocation(
                    "browser.extract_text",
                    encode_input(&[("url", "https://example.com")])
                ))
                .expect("browser fetch should request permission")
                .risk,
            PermissionRisk::Network
        );
        assert_eq!(
            browser_type
                .permission_request(&invocation(
                    "browser.type",
                    encode_input(&[("selector", "#q"), ("text", "private input")])
                ))
                .expect("browser type should request permission")
                .risk,
            PermissionRisk::Sensitive
        );
        assert_eq!(
            computer_screenshot
                .permission_request(&invocation("computer.screenshot", String::new()))
                .expect("computer screenshot should request permission")
                .risk,
            PermissionRisk::Sensitive
        );
        assert_eq!(
            computer_key
                .permission_request(&invocation(
                    "computer.key",
                    encode_input(&[("key", "Cmd+Backspace")])
                ))
                .expect("computer key should request permission")
                .risk,
            PermissionRisk::Destructive
        );
    }

    #[test]
    fn browser_action_writes_sidecar_request_artifact() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_BROWSER_SIDECAR");
        let root = temp_workspace();
        let tool = BrowserActionTool::click(root.clone());

        let result = tool
            .execute(invocation(
                "browser.click",
                encode_input(&[
                    ("url", "https://example.com"),
                    ("selector", "button.primary"),
                    ("output_dir", ".cindx/browser-actions"),
                ]),
            ))
            .expect("browser action should be recorded");
        let artifact = result
            .metadata
            .get("artifact_path")
            .expect("artifact should be recorded");
        let request = fs::read_to_string(root.join(artifact)).expect("request should exist");

        assert_eq!(result.metadata.get("controller").map(String::as_str), Some("artifact"));
        assert!(request.contains("\"namespace\":\"browser\""));
        assert!(request.contains("\"action\":\"click\""));
        assert!(request.contains("button.primary"));
    }

    #[test]
    fn browser_action_executes_configured_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let sidecar = root.join("browser-sidecar-test.sh");
        fs::write(
            &sidecar,
            "#!/bin/sh\nprintf 'sidecar-test-ok request=%s\\n' \"$1\"\n",
        )
        .expect("sidecar should write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o700))
                .expect("sidecar should be executable");
        }
        env::set_var("CINDX_BROWSER_SIDECAR", &sidecar);
        let tool = BrowserActionTool::click(root);

        let result = tool
            .execute(invocation(
                "browser.click",
                encode_input(&[("selector", "button.primary")]),
            ))
            .expect("browser sidecar should execute");

        assert_eq!(result.metadata.get("controller").map(String::as_str), Some("sidecar"));
        assert!(result.output.contains("sidecar-test-ok"));
        env::remove_var("CINDX_BROWSER_SIDECAR");
    }

    #[test]
    fn computer_action_writes_sidecar_request_artifact() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_COMPUTER_SIDECAR");
        let root = temp_workspace();
        let tool = ComputerTool::click(root.clone());

        let result = tool
            .execute(invocation(
                "computer.click",
                encode_input(&[
                    ("x", "120"),
                    ("y", "240"),
                    ("output_dir", ".cindx/computer-actions"),
                ]),
            ))
            .expect("computer action should be recorded");
        let artifact = result
            .metadata
            .get("artifact_path")
            .expect("artifact should be recorded");
        let request = fs::read_to_string(root.join(artifact)).expect("request should exist");

        assert_eq!(result.metadata.get("controller").map(String::as_str), Some("artifact"));
        assert!(request.contains("\"namespace\":\"computer\""));
        assert!(request.contains("\"action\":\"click\""));
        assert!(request.contains("\"x\":\"120\""));
    }

    #[test]
    fn computer_screenshot_writes_redaction_manifest_when_native_disabled() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_COMPUTER_SIDECAR");
        env::set_var("CINDX_DISABLE_NATIVE_SCREENSHOT", "1");
        let root = temp_workspace();
        let tool = ComputerTool::screenshot(root.clone());

        let result = tool
            .execute(invocation(
                "computer.screenshot",
                encode_input(&[
                    ("redaction", "manual"),
                    ("output_dir", ".cindx/computer-actions"),
                ]),
            ))
            .expect("screenshot request should be recorded");
        let manifest = result
            .metadata
            .get("redaction_manifest_path")
            .expect("manifest should be recorded");
        let manifest_text = fs::read_to_string(root.join(manifest)).expect("manifest should exist");
        env::remove_var("CINDX_DISABLE_NATIVE_SCREENSHOT");

        assert_eq!(result.metadata.get("controller").map(String::as_str), Some("artifact"));
        assert!(manifest_text.contains("\"redaction\":\"manual\""));
        assert!(manifest_text.contains("\"status\":\"pending_review\""));
    }

    #[test]
    fn html_text_extraction_strips_tags_and_decodes_entities() {
        let text = html_to_text("<html><body><h1>Local &amp; Agent</h1><p>RAG&nbsp;ready</p></body></html>");

        assert!(text.contains("Local & Agent"));
        assert!(text.contains("RAG ready"));
        assert!(!text.contains("<h1>"));
    }

    #[test]
    fn url_validation_requires_http_scheme() {
        let invalid = parse_input(&encode_input(&[("url", "file:///tmp/page.html")]));
        let valid = parse_input(&encode_input(&[("url", "https://example.com")]));

        assert!(required_url(&invalid).is_err());
        assert_eq!(required_url(&valid).expect("url should pass"), "https://example.com");
    }

    #[test]
    fn rejects_paths_that_escape_workspace() {
        let root = temp_workspace();
        let reader = ReadFileTool::new(root);

        let error = reader
            .execute(invocation("file.read", encode_input(&[("path", "../secret")])))
            .expect_err("path escape should fail");

        assert!(error.message.contains("escapes"));
    }

    #[test]
    fn blocks_local_credential_files_from_read_and_search() {
        let root = temp_workspace();
        fs::create_dir_all(root.join(".cindx")).expect("config directory should be created");
        fs::write(root.join(".cindx/provider.conf"), "api_key=secret-value")
            .expect("config should be written");
        let reader = ReadFileTool::new(root.clone());
        let searcher = SearchFilesTool::new(root);

        let read_error = reader
            .execute(invocation(
                "file.read",
                encode_input(&[("path", ".cindx/provider.conf")]),
            ))
            .expect_err("credential read should fail");
        let search_error = searcher
            .execute(invocation(
                "file.search",
                encode_input(&[("path", ".cindx/provider.conf"), ("query", "api_key")]),
            ))
            .expect_err("credential search should fail");

        assert!(read_error.message.contains("credential"));
        assert!(search_error.message.contains("credential"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_reads_through_symlinks_that_leave_workspace() {
        use std::os::unix::fs::symlink;

        let root = temp_workspace();
        let outside = std::env::temp_dir().join(format!(
            "cindx-tools-outside-{}",
            stable_hash(&format!("{:?}", std::time::SystemTime::now()))
        ));
        fs::write(&outside, "outside secret").expect("outside file should be written");
        symlink(&outside, root.join("linked-secret")).expect("symlink should be created");
        let reader = ReadFileTool::new(root);

        let error = reader
            .execute(invocation(
                "file.read",
                encode_input(&[("path", "linked-secret")]),
            ))
            .expect_err("symlink escape should fail");

        assert!(error.message.contains("symbolic link"));
        let _ = fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_writes_and_shell_cwds_through_symlinks_that_leave_workspace() {
        use std::os::unix::fs::symlink;

        let root = temp_workspace();
        let outside = std::env::temp_dir().join(format!(
            "cindx-tools-outside-dir-{}",
            stable_hash(&format!("{:?}", std::time::SystemTime::now()))
        ));
        fs::create_dir_all(&outside).expect("outside directory should be created");
        symlink(&outside, root.join("linked-dir")).expect("symlink should be created");

        let writer = WriteFileTool::new(root.clone());
        let write_error = writer
            .execute(invocation(
                "file.write",
                encode_input(&[("path", "linked-dir/escaped.txt"), ("content", "outside")]),
            ))
            .expect_err("symlink write escape should fail");
        let shell = ShellRunTool::new(root);
        let shell_error = shell
            .execute(invocation(
                "shell.run",
                encode_input(&[("command", "pwd"), ("cwd", "linked-dir")]),
            ))
            .expect_err("symlink cwd escape should fail");

        assert!(write_error.message.contains("symbolic link"));
        assert!(shell_error.message.contains("symbolic link"));
        assert!(!outside.join("escaped.txt").exists());
        let _ = fs::remove_dir_all(outside);
    }
}
