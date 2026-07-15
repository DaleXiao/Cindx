use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_core::{
    Metadata, PermissionRequest, PermissionRequestId, PermissionRisk, TaskId, ToolCallId,
    ToolArtifact, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
use model_provider::{
    ImageGenerationRequest, OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider,
    MODEL_REQUEST_CANCELLED,
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

#[derive(Clone)]
pub struct ToolExecutionControl {
    should_cancel: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ToolExecutionControl {
    pub fn new(should_cancel: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self { should_cancel: Arc::new(should_cancel) }
    }

    pub fn never_cancelled() -> Self {
        Self::new(|| false)
    }

    pub fn should_cancel(&self) -> bool {
        (self.should_cancel)()
    }
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest>;

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError>;

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Tool execution cancelled before it started.",
                Metadata::new(),
            ));
        }
        let result = self.execute(invocation)?;
        if control.should_cancel() {
            return Ok(ToolResult::text(
                result.invocation_id,
                ToolOutcomeStatus::Cancelled,
                "Tool execution cancelled.",
                Metadata::new(),
            ));
        }
        Ok(result)
    }
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebSearchConfig {
    pub endpoint: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageGenerationConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

impl ImageGenerationConfig {
    pub fn is_ready(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn with_workspace_tools(workspace_root: impl Into<PathBuf>) -> Self {
        Self::with_workspace_tools_and_web_search(workspace_root, WebSearchConfig::default())
    }

    pub fn with_workspace_tools_and_web_search(
        workspace_root: impl Into<PathBuf>,
        web_search_config: WebSearchConfig,
    ) -> Self {
        Self::with_workspace_tools_and_services(workspace_root, web_search_config, None)
    }

    pub fn with_workspace_tools_and_services(
        workspace_root: impl Into<PathBuf>,
        web_search_config: WebSearchConfig,
        image_generation_config: Option<ImageGenerationConfig>,
    ) -> Self {
        let workspace_root = workspace_root.into();
        let mut registry = Self::new();
        registry.register(Box::new(ReadFileTool::new(workspace_root.clone())));
        registry.register(Box::new(ListDirectoryTool::new(workspace_root.clone())));
        registry.register(Box::new(SearchFilesTool::new(workspace_root.clone())));
        registry.register(Box::new(WriteFileTool::new(workspace_root.clone())));
        registry.register(Box::new(ShellRunTool::new(workspace_root.clone())));
        registry.register(Box::new(WebSearchTool::new(web_search_config)));
        if let Some(config) = image_generation_config.filter(ImageGenerationConfig::is_ready) {
            registry.register(Box::new(ImageGenerationTool::new(
                workspace_root.clone(),
                config,
            )));
        }
        registry.register(Box::new(BrowserTool::open(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::extract_text(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::capture(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::click(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::type_text(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::scroll(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::tabs(workspace_root.clone())));
        registry.register(Box::new(BrowserTool::select_tab(workspace_root.clone())));
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

const DEFAULT_SHELL_TIMEOUT_SECONDS: u64 = 120;
const MAX_SHELL_TIMEOUT_SECONDS: u64 = 600;
const SHELL_POLL_INTERVAL: Duration = Duration::from_millis(40);

struct ShellCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    cancelled: bool,
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
fn terminate_process_group(process_id: u32, signal: i32) {
    unsafe {
        let _ = kill(-(process_id as i32), signal);
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_process_id: u32, _signal: i32) {}

fn read_process_stream(mut stream: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    let _ = stream.read_to_end(&mut output);
    output
}

fn run_shell_command(
    command: &str,
    cwd: &Path,
    timeout_seconds: u64,
    control: &ToolExecutionControl,
) -> Result<ShellCommandOutput, ToolError> {
    let mut process = Command::new("/bin/zsh");
    process
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }

    let mut child = process
        .spawn()
        .map_err(|error| ToolError::new(format!("failed to run shell command: {error}")))?;
    let process_id = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::new("failed to capture shell stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::new("failed to capture shell stderr"))?;
    let stdout_reader = thread::spawn(move || read_process_stream(stdout));
    let stderr_reader = thread::spawn(move || read_process_stream(stderr));
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let mut timed_out = false;
    let mut cancelled = false;

    let status = loop {
        if control.should_cancel() {
            cancelled = true;
            terminate_process_group(process_id, 15);
            thread::sleep(Duration::from_millis(120));
            terminate_process_group(process_id, 9);
            let _ = child.kill();
            break child.wait().map_err(|error| {
                ToolError::new(format!("failed to stop cancelled shell command: {error}"))
            })?;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(SHELL_POLL_INTERVAL),
            Ok(None) => {
                timed_out = true;
                terminate_process_group(process_id, 15);
                thread::sleep(Duration::from_millis(120));
                terminate_process_group(process_id, 9);
                let _ = child.kill();
                break child.wait().map_err(|error| {
                    ToolError::new(format!("failed to stop timed out shell command: {error}"))
                })?;
            }
            Err(error) => {
                terminate_process_group(process_id, 9);
                let _ = child.kill();
                let _ = child.wait();
                return Err(ToolError::new(format!(
                    "failed to inspect shell command: {error}"
                )));
            }
        }
    };

    // A completed shell may leave background descendants holding the output pipes open.
    terminate_process_group(process_id, 15);
    thread::sleep(Duration::from_millis(40));
    terminate_process_group(process_id, 9);

    Ok(ShellCommandOutput {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
        timed_out,
        cancelled,
    })
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
            "Run a bounded foreground shell command in the workspace. Background processes are terminated when the command finishes.",
            ToolRisk::ExecutesProcess,
            "command=<shell command>\ncwd=<optional workspace-relative path>\ntimeout_seconds=<optional 1-600, default 120>",
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
        self.execute_with_control(invocation, &ToolExecutionControl::never_cancelled())
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Shell command cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        let command = required_input(&input, "command")?;
        let cwd = input.get("cwd").cloned().unwrap_or_else(|| ".".to_string());
        let timeout_seconds = match input.get("timeout_seconds") {
            Some(value) => value.parse::<u64>().map_err(|_| {
                ToolError::new("timeout_seconds must be an integer between 1 and 600")
            })?,
            None => DEFAULT_SHELL_TIMEOUT_SECONDS,
        };
        if !(1..=MAX_SHELL_TIMEOUT_SECONDS).contains(&timeout_seconds) {
            return Err(ToolError::new(
                "timeout_seconds must be an integer between 1 and 600",
            ));
        }
        let resolved_cwd = resolve_workspace_path(&self.workspace_root, &cwd)?;
        let resolved_cwd = resolve_workspace_read_path(&self.workspace_root, &resolved_cwd)?;
        let output = run_shell_command(&command, &resolved_cwd, timeout_seconds, control)?;

        let mut combined = String::new();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        if output.timed_out {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str(&format!(
                "Command timed out after {timeout_seconds} seconds. Background processes were stopped."
            ));
        } else if output.cancelled {
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str("Command cancelled. The process group was stopped.");
        }

        let mut metadata = Metadata::new();
        metadata.insert("command".to_string(), command);
        metadata.insert("cwd".to_string(), cwd);
        metadata.insert("timeout_seconds".to_string(), timeout_seconds.to_string());
        metadata.insert("timed_out".to_string(), output.timed_out.to_string());
        metadata.insert("cancelled".to_string(), output.cancelled.to_string());
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
            if output.cancelled {
                ToolOutcomeStatus::Cancelled
            } else if output.status.success() && !output.timed_out {
                ToolOutcomeStatus::Succeeded
            } else {
                ToolOutcomeStatus::Failed
            },
            combined,
            metadata,
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct WebSearchTool {
    config: WebSearchConfig,
}

impl WebSearchTool {
    pub fn new(config: WebSearchConfig) -> Self {
        Self { config }
    }
}

impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "web.search",
            "Search the web through the configured search API or the built-in public fallback.",
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
        let (text, url, provider) = if self.config.endpoint.trim().is_empty() {
            let url = format!("https://duckduckgo.com/html/?q={}", url_encode(&query));
            let html = fetch_url(&url)?;
            (
                trim_lines(&html_to_text(&html), max_results * 4),
                url,
                "public_fallback".to_string(),
            )
        } else {
            let endpoint = self.config.endpoint.trim().to_string();
            let response = fetch_search_api(&self.config, &query, max_results)?;
            (
                response.chars().take(64_000).collect(),
                endpoint,
                "configured_api".to_string(),
            )
        };

        let mut metadata = Metadata::new();
        metadata.insert("query".to_string(), query);
        metadata.insert("url".to_string(), url);
        metadata.insert("max_results".to_string(), max_results.to_string());
        metadata.insert("provider".to_string(), provider);

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            text,
            metadata,
        ))
    }
}

pub struct ImageGenerationTool {
    workspace_root: PathBuf,
    config: ImageGenerationConfig,
}

impl ImageGenerationTool {
    pub fn new(workspace_root: impl Into<PathBuf>, config: ImageGenerationConfig) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            config,
        }
    }
}

impl Tool for ImageGenerationTool {
    fn spec(&self) -> ToolSpec {
        let mut spec = ToolSpec::new(
            "image.generate",
            "image",
            "Generate one raster image with the configured image model and save it in the active workspace.",
            ToolRisk::UsesNetwork,
            agent_core::ToolSource::BuiltIn,
            agent_core::ToolExposure::Auto,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "A detailed visual description of the image to generate."
                    },
                    "size": {
                        "type": "string",
                        "description": "Optional provider-supported size such as 1024x1024."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Optional workspace-relative output path. The file extension is normalized to the returned image format."
                    }
                },
                "required": ["prompt"],
                "additionalProperties": false
            })
            .to_string(),
        );
        spec.output_schema_json = Some(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "mimeType": { "type": "string" },
                    "model": { "type": "string" }
                },
                "required": ["path", "mimeType", "model"]
            })
            .to_string(),
        );
        spec
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        let input = parse_input(&invocation.input_json);
        let output_path = input
            .get("output_path")
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "generated-images/".to_string());
        Some(permission_request(
            &invocation.task_id,
            PermissionRisk::Network,
            "image.generate",
            "Send a visual prompt to the configured image provider and write the result in the workspace.",
            &output_path,
            [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
                ("model".to_string(), self.config.model.clone()),
            ]
            .into_iter()
            .collect(),
        ))
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_with_control(invocation, &ToolExecutionControl::never_cancelled())
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Image generation cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        let prompt = required_input(&input, "prompt")?;
        let size = input
            .get("size")
            .filter(|value| !value.trim().is_empty())
            .cloned();
        let provider = OpenAiCompatibleImageProvider::new(OpenAiCompatibleImageConfig {
            base_url: self.config.base_url.clone(),
            api_key: self.config.api_key.clone(),
            model: self.config.model.clone(),
            timeout_seconds: self.config.timeout_seconds.max(1),
        });
        let response = match provider.generate_cancellable(
            ImageGenerationRequest {
                prompt,
                size: size.clone(),
                metadata: Metadata::new(),
            },
            || control.should_cancel(),
        ) {
            Ok(response) => response,
            Err(error) if error.message == MODEL_REQUEST_CANCELLED => {
                return Ok(ToolResult::text(
                    invocation.id,
                    ToolOutcomeStatus::Cancelled,
                    "Image generation cancelled.",
                    Metadata::new(),
                ));
            }
            Err(error) => return Err(ToolError::new(error.message)),
        };
        let image = response
            .images
            .into_iter()
            .next()
            .ok_or_else(|| ToolError::new("image provider returned no image"))?;
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Image generation cancelled.",
                Metadata::new(),
            ));
        }

        let requested_path = input.get("output_path").map(String::as_str);
        let (resolved_path, relative_path) = image_output_path(
            &self.workspace_root,
            requested_path,
            &image.mime_type,
        )?;
        if let Some(parent) = resolved_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(format!("failed to create image output directory: {error}"))
            })?;
        }
        fs::write(&resolved_path, &image.bytes)
            .map_err(|error| ToolError::new(format!("failed to write generated image: {error}")))?;

        let mut metadata = Metadata::new();
        metadata.insert("artifact_path".to_string(), relative_path.clone());
        metadata.insert("model".to_string(), response.model.clone());
        metadata.insert("mime_type".to_string(), image.mime_type.clone());
        metadata.insert("bytes".to_string(), image.bytes.len().to_string());
        if let Some(size) = size {
            metadata.insert("size".to_string(), size);
        }
        let title = resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Generated image")
            .to_string();
        let mut result = ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            format!("Generated image: {relative_path}"),
            metadata,
        );
        result.artifacts.push(ToolArtifact {
            path: relative_path.clone(),
            mime_type: Some(image.mime_type.clone()),
            title: Some(title),
        });
        result.structured_output_json = Some(
            serde_json::json!({
                "path": relative_path,
                "mimeType": image.mime_type,
                "model": response.model,
                "revisedPrompt": image.revised_prompt
            })
            .to_string(),
        );
        Ok(result)
    }
}

fn image_output_path(
    workspace_root: &Path,
    requested_path: Option<&str>,
    mime_type: &str,
) -> Result<(PathBuf, String), ToolError> {
    let extension = match mime_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/png" => "png",
        _ => return Err(ToolError::new("unsupported generated image format")),
    };
    let timestamp = current_time_millis();
    let mut relative = requested_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!("generated-images/image-{timestamp}.{extension}"))
        });
    if relative.file_name().is_none() || requested_path.is_some_and(|value| value.ends_with('/')) {
        relative.push(format!("image-{timestamp}.{extension}"));
    } else {
        relative.set_extension(extension);
    }
    let relative_path = relative.to_string_lossy().to_string();
    let resolved = resolve_workspace_path(workspace_root, &relative_path)?;
    Ok((resolved, relative_path))
}

const BROWSER_CONTROL_REQUEST_SCHEMA: &str = "cindx.browser-control.v2";
const BROWSER_CONTROL_RESPONSE_SCHEMA: &str = "cindx.browser-control-result.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserToolKind {
    Open,
    ExtractText,
    Capture,
    Click,
    TypeText,
    Scroll,
    Tabs,
    SelectTab,
}

impl BrowserToolKind {
    fn tool_name(self) -> &'static str {
        match self {
            Self::Open => "browser.open",
            Self::ExtractText => "browser.extract_text",
            Self::Capture => "browser.capture",
            Self::Click => "browser.click",
            Self::TypeText => "browser.type",
            Self::Scroll => "browser.scroll",
            Self::Tabs => "browser.tabs",
            Self::SelectTab => "browser.select_tab",
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::ExtractText => "extract_text",
            Self::Capture => "capture",
            Self::Click => "click",
            Self::TypeText => "type",
            Self::Scroll => "scroll",
            Self::Tabs => "tabs",
            Self::SelectTab => "select_tab",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Open => "Open a URL in a reusable CDP browser session.",
            Self::ExtractText => {
                "Read dynamic page text and an accessibility snapshot from the current browser session."
            }
            Self::Capture => "Capture screenshot and text artifacts from the current browser session.",
            Self::Click => "Click a semantic target, CSS selector, or coordinate with Playwright auto-waiting.",
            Self::TypeText => "Fill a semantic browser target with text through Playwright.",
            Self::Scroll => "Scroll the current page or a selected scroll container.",
            Self::Tabs => "List tabs in the reusable browser session.",
            Self::SelectTab => "Select and focus one tab by its CDP target id.",
        }
    }

    fn schema(self) -> &'static str {
        match self {
            Self::Open => {
                "url=<http-or-https-url>\nnew_tab=<optional true|false>\nwait_until=<optional load|domcontentloaded|networkidle|commit>\ntimeout_ms=<optional milliseconds>\nsession_id=<optional browser session>\nheadless=<optional true|false>"
            }
            Self::ExtractText => {
                "url=<optional http-or-https-url>\ntab_id=<optional CDP target id>\nframe=<optional frame name or URL fragment>\nselector=<optional CSS selector>\nrole=<optional accessible role>\nname=<optional accessible name>\nlabel=<optional form label>\nplaceholder=<optional placeholder>\ntext_target=<optional visible target text>\ntimeout_ms=<optional milliseconds>\nsession_id=<optional browser session>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::Capture => {
                "url=<optional http-or-https-url>\ntab_id=<optional CDP target id>\nfull_page=<optional true|false>\ntimeout_ms=<optional milliseconds>\nsession_id=<optional browser session>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::Click => {
                "tab_id=<optional CDP target id>\nframe=<optional frame name or URL fragment>\nselector=<optional CSS selector>\nrole=<optional accessible role>\nname=<optional accessible name>\nlabel=<optional form label>\ntext_target=<optional visible target text>\nexact=<optional true|false>\nx=<optional coordinate>\ny=<optional coordinate>\ndownload=<optional true|false>\nwait_for=<optional CSS selector visible after click>\ntimeout_ms=<optional milliseconds>\nsession_id=<optional browser session>\noutput_dir=<optional workspace-relative directory>"
            }
            Self::TypeText => {
                "text=<text to enter>\ntab_id=<optional CDP target id>\nframe=<optional frame name or URL fragment>\nselector=<optional CSS selector>\nrole=<optional accessible role>\nname=<optional accessible name>\nlabel=<optional form label>\nplaceholder=<optional placeholder>\ntext_target=<optional visible target text>\nexact=<optional true|false>\nappend=<optional true|false>\npress_enter=<optional true|false>\ntimeout_ms=<optional milliseconds>\nsession_id=<optional browser session>"
            }
            Self::Scroll => {
                "tab_id=<optional CDP target id>\nframe=<optional frame name or URL fragment>\nselector=<optional CSS selector>\ndelta_x=<optional pixels>\ndelta_y=<optional pixels, default 600>\ntimeout_ms=<optional milliseconds>\nsession_id=<optional browser session>"
            }
            Self::Tabs => "session_id=<optional browser session>\nheadless=<optional true|false>",
            Self::SelectTab => {
                "tab_id=<CDP target id>\nsession_id=<optional browser session>"
            }
        }
    }

    fn risk(self) -> ToolRisk {
        match self {
            Self::TypeText => ToolRisk::SensitiveContext,
            _ => ToolRisk::UsesNetwork,
        }
    }

    fn permission_risk(self) -> PermissionRisk {
        match self {
            Self::TypeText => PermissionRisk::Sensitive,
            _ => PermissionRisk::Network,
        }
    }

    fn permission_reason(self) -> &'static str {
        match self {
            Self::Open => "Navigate a reusable browser session.",
            Self::ExtractText => "Read the current browser page.",
            Self::Capture => "Capture the current browser page.",
            Self::Click => "Click inside a browser session.",
            Self::TypeText => "Enter potentially sensitive text into a browser session.",
            Self::Scroll => "Scroll inside a browser session.",
            Self::Tabs => "Inspect open browser tabs.",
            Self::SelectTab => "Focus an open browser tab.",
        }
    }
}

pub struct BrowserTool {
    workspace_root: PathBuf,
    kind: BrowserToolKind,
}

impl BrowserTool {
    fn new(workspace_root: impl Into<PathBuf>, kind: BrowserToolKind) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind,
        }
    }

    fn open(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Open)
    }

    fn extract_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::ExtractText)
    }

    fn capture(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Capture)
    }

    fn click(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Click)
    }

    fn type_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::TypeText)
    }

    fn scroll(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Scroll)
    }

    fn tabs(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Tabs)
    }

    fn select_tab(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::SelectTab)
    }

    fn execute_inner(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        validate_browser_input(self.kind, &input)?;
        if let Some(url) = input.get("url").filter(|value| !value.trim().is_empty()) {
            required_url(&[("url".to_string(), url.clone())].into_iter().collect())?;
        }

        let output_dir = input
            .get("output_dir")
            .cloned()
            .unwrap_or_else(|| ".cindx/browser-artifacts".to_string());
        let resolved_output_dir = resolve_workspace_path(&self.workspace_root, &output_dir)?;
        fs::create_dir_all(&resolved_output_dir).map_err(|error| {
            ToolError::new(format!("failed to create browser artifact directory: {error}"))
        })?;
        let session_id = input
            .get("session_id")
            .filter(|value| !value.trim().is_empty())
            .or_else(|| invocation.metadata.get("session_id"))
            .cloned()
            .unwrap_or_else(|| invocation.task_id.0.clone());
        let session_key = browser_session_key(&session_id);
        let session_relative = format!(".cindx/browser-sessions/{session_key}");
        let session_dir = resolve_workspace_path(&self.workspace_root, &session_relative)?;
        fs::create_dir_all(&session_dir).map_err(|error| {
            ToolError::new(format!("failed to create browser session directory: {error}"))
        })?;

        let action_id = format!(
            "browser-{}-{}",
            current_time_millis(),
            stable_hash(&format!("{}:{}", self.kind.action(), invocation.input_json))
        );
        let request_path = resolved_output_dir.join(format!(".{action_id}-request.json"));
        let request_json = browser_request_json(
            &action_id,
            self.kind,
            &session_id,
            &session_dir,
            &resolved_output_dir,
            &input,
        )?;
        write_private_file(&request_path, request_json.as_bytes())?;
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(30_000)
            .clamp(1_000, 120_000);
        let sidecar = run_json_sidecar_controlled(
            "CINDX_BROWSER_SIDECAR",
            &request_path,
            control,
            Duration::from_millis(timeout_ms.saturating_add(10_000)),
        );
        let _ = fs::remove_file(&request_path);
        let sidecar = sidecar?;
        if sidecar.cancelled {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Browser action cancelled.",
                [
                    ("action".to_string(), self.kind.action().to_string()),
                    ("session_id".to_string(), session_id),
                ]
                .into_iter()
                .collect(),
            ));
        }
        if sidecar.timed_out {
            return Err(ToolError::new(format!(
                "browser sidecar exceeded {} ms",
                timeout_ms.saturating_add(10_000)
            )));
        }

        let response: serde_json::Value = serde_json::from_str(&sidecar.stdout)
            .map_err(|error| ToolError::new(format!("invalid browser sidecar response: {error}")))?;
        if response.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_CONTROL_RESPONSE_SCHEMA)
        {
            return Err(ToolError::new("browser sidecar returned an unsupported schema"));
        }
        let output = response
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("browser action completed")
            .to_string();
        if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(ToolError::new(output));
        }

        let mut metadata = Metadata::new();
        metadata.insert("action".to_string(), self.kind.action().to_string());
        metadata.insert("controller".to_string(), "cdp_playwright".to_string());
        metadata.insert("session_id".to_string(), session_id);
        metadata.insert("session_path".to_string(), session_relative);
        if let Some(page) = response.get("page") {
            for (source, target) in [("id", "tab_id"), ("url", "url"), ("title", "title")] {
                if let Some(value) = page.get(source).and_then(serde_json::Value::as_str) {
                    metadata.insert(target.to_string(), value.to_string());
                }
            }
        }
        if let Some(duration) = response.get("duration_ms").and_then(serde_json::Value::as_u64) {
            metadata.insert("duration_ms".to_string(), duration.to_string());
        }
        if let Some(trace_path) = response
            .get("trace_path")
            .and_then(serde_json::Value::as_str)
            .and_then(|path| workspace_relative_path(&self.workspace_root, path))
        {
            metadata.insert("trace_path".to_string(), trace_path);
        }

        let artifacts = response
            .get("artifacts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|artifact| {
                let path = artifact.get("path")?.as_str()?;
                Some(ToolArtifact {
                    path: workspace_relative_path(&self.workspace_root, path)?,
                    mime_type: artifact
                        .get("mime_type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    title: artifact
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect::<Vec<_>>();
        if let Some(artifact) = artifacts.first() {
            metadata.insert("artifact_path".to_string(), artifact.path.clone());
        }
        if let Some(text) = artifacts
            .iter()
            .find(|artifact| artifact.mime_type.as_deref() == Some("text/plain"))
        {
            metadata.insert("text_path".to_string(), text.path.clone());
        }

        let mut result = ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            metadata,
        );
        result.structured_output_json = Some(sidecar.stdout);
        result.artifacts = artifacts;
        Ok(result)
    }
}

impl Tool for BrowserTool {
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
        Some(permission_request(
            &invocation.task_id,
            self.kind.permission_risk(),
            self.kind.tool_name(),
            self.kind.permission_reason(),
            &browser_scope(self.kind, &input),
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
        self.execute_inner(invocation, &ToolExecutionControl::never_cancelled())
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        self.execute_inner(invocation, control)
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
            "destructive" | "new_tab" | "headless" | "full_page" | "exact" | "download"
            | "append" | "press_enter" => "boolean",
            "x" | "y" | "delta_x" | "delta_y" | "limit" | "max_results" | "timeout_ms" => {
                "integer"
            }
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

fn browser_scope(kind: BrowserToolKind, input: &BTreeMap<String, String>) -> String {
    let url = input
        .get("url")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "current browser page".to_string());
    let target = input
        .iter()
        .find(|(key, value)| {
            matches!(
                key.as_str(),
                "selector" | "role" | "label" | "placeholder" | "text_target" | "tab_id"
            ) && !value.trim().is_empty()
        })
        .map(|(key, value)| format!("{key}={value}"))
        .or_else(|| {
            let x = input.get("x")?;
            let y = input.get("y")?;
            Some(format!("x={x},y={y}"))
        })
        .unwrap_or_else(|| "viewport".to_string());

    format!("{} on {url} at {target}", kind.action())
}

fn validate_browser_input(
    kind: BrowserToolKind,
    input: &BTreeMap<String, String>,
) -> Result<(), ToolError> {
    match kind {
        BrowserToolKind::Open => required_input(input, "url").map(|_| ()),
        BrowserToolKind::Click => {
            if has_browser_target(input) {
                return Ok(());
            }
            if input.get("x").is_some_and(|value| !value.trim().is_empty())
                && input.get("y").is_some_and(|value| !value.trim().is_empty())
            {
                return Ok(());
            }
            Err(ToolError::new("browser.click requires selector or x/y"))
        }
        BrowserToolKind::TypeText => {
            required_input(input, "text")?;
            if has_browser_target(input) {
                return Ok(());
            }
            Err(ToolError::new("browser.type requires a semantic target or selector"))
        }
        BrowserToolKind::SelectTab => required_input(input, "tab_id").map(|_| ()),
        BrowserToolKind::ExtractText
        | BrowserToolKind::Capture
        | BrowserToolKind::Scroll
        | BrowserToolKind::Tabs => Ok(()),
    }
}

fn has_browser_target(input: &BTreeMap<String, String>) -> bool {
    ["selector", "role", "label", "placeholder", "text_target"]
        .iter()
        .any(|key| input.get(*key).is_some_and(|value| !value.trim().is_empty()))
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

fn browser_request_json(
    action_id: &str,
    kind: BrowserToolKind,
    session_id: &str,
    session_dir: &Path,
    output_dir: &Path,
    input: &BTreeMap<String, String>,
) -> Result<String, ToolError> {
    let mut request = serde_json::Map::from_iter([
        (
            "schema".to_string(),
            serde_json::Value::String(BROWSER_CONTROL_REQUEST_SCHEMA.to_string()),
        ),
        ("id".to_string(), serde_json::Value::String(action_id.to_string())),
        (
            "action".to_string(),
            serde_json::Value::String(kind.action().to_string()),
        ),
        (
            "session_id".to_string(),
            serde_json::Value::String(session_id.to_string()),
        ),
        (
            "session_dir".to_string(),
            serde_json::Value::String(session_dir.display().to_string()),
        ),
        (
            "output_dir".to_string(),
            serde_json::Value::String(output_dir.display().to_string()),
        ),
    ]);
    for key in [
        "url",
        "new_tab",
        "wait_until",
        "tab_id",
        "frame",
        "selector",
        "role",
        "name",
        "label",
        "placeholder",
        "text_target",
        "exact",
        "text",
        "append",
        "press_enter",
        "x",
        "y",
        "delta_x",
        "delta_y",
        "download",
        "wait_for",
        "full_page",
        "timeout_ms",
        "headless",
    ] {
        if let Some(value) = input.get(key).filter(|value| !value.trim().is_empty()) {
            request.insert(key.to_string(), serde_json::Value::String(value.clone()));
        }
    }
    if kind == BrowserToolKind::Scroll && !input.contains_key("delta_y") {
        request.insert(
            "delta_y".to_string(),
            serde_json::Value::String("600".to_string()),
        );
    }

    serde_json::to_string(&request)
        .map(|json| format!("{json}\n"))
        .map_err(|error| ToolError::new(format!("failed to encode browser request: {error}")))
}

fn browser_session_key(session_id: &str) -> String {
    let slug = session_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(40)
        .collect::<String>();
    format!(
        "{}-{:x}",
        if slug.is_empty() { "session" } else { &slug },
        stable_hash(session_id)
    )
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), ToolError> {
    fs::write(path, contents)
        .map_err(|error| ToolError::new(format!("failed to write browser request: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            ToolError::new(format!("failed to protect browser request: {error}"))
        })?;
    }
    Ok(())
}

fn workspace_relative_path(workspace_root: &Path, artifact_path: &str) -> Option<String> {
    let canonical_root = fs::canonicalize(workspace_root).ok()?;
    let canonical_artifact = fs::canonicalize(artifact_path).ok()?;
    canonical_artifact
        .strip_prefix(canonical_root)
        .ok()
        .map(|path| path.display().to_string())
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

fn fetch_search_api(
    config: &WebSearchConfig,
    query: &str,
    max_results: usize,
) -> Result<String, ToolError> {
    let endpoint = config.endpoint.trim();
    if endpoint.contains('\n')
        || endpoint.contains('\r')
        || !(endpoint.starts_with("https://") || endpoint.starts_with("http://"))
    {
        return Err(ToolError::new(
            "web search endpoint must be a single-line HTTP or HTTPS URL",
        ));
    }
    if config.api_key.contains('\n') || config.api_key.contains('\r') {
        return Err(ToolError::new("web search API key must be a single line"));
    }

    let uses_url_template = endpoint.contains("{query}") || endpoint.contains("{limit}");
    let url = endpoint
        .replace("{query}", &url_encode(query))
        .replace("{limit}", &max_results.to_string());
    let mut command = Command::new("/usr/bin/curl");
    command
        .arg("-L")
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail")
        .arg("--max-time")
        .arg("25")
        .arg("--user-agent")
        .arg("Cindx/1");
    if !config.api_key.trim().is_empty() {
        command
            .arg("--header")
            .arg(format!("Authorization: Bearer {}", config.api_key.trim()));
    }
    if !uses_url_template {
        command
            .arg("--request")
            .arg("POST")
            .arg("--header")
            .arg("Content-Type: application/json")
            .arg("--data")
            .arg(
                serde_json::json!({
                    "query": query,
                    "max_results": max_results
                })
                .to_string(),
            );
    }
    let output = command
        .arg(&url)
        .output()
        .map_err(|error| ToolError::new(format!("failed to run search API request: {error}")))?;
    if !output.status.success() {
        return Err(ToolError::new(format!(
            "search API request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
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

struct ControlledSidecarOutput {
    stdout: String,
    cancelled: bool,
    timed_out: bool,
}

fn run_json_sidecar_controlled(
    env_key: &str,
    request_path: &Path,
    control: &ToolExecutionControl,
    hard_timeout: Duration,
) -> Result<ControlledSidecarOutput, ToolError> {
    let sidecar = env::var(env_key)
        .ok()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| ToolError::new(format!("{env_key} is not configured")))?;
    let sidecar_path = PathBuf::from(&sidecar);
    if !sidecar_path.is_file() {
        return Err(ToolError::new(format!("browser sidecar does not exist: {sidecar}")));
    }
    let mut command = if sidecar_path.extension().and_then(|extension| extension.to_str())
        == Some("js")
    {
        let mut command = Command::new(env::var("CINDX_NODE").unwrap_or_else(|_| "node".to_string()));
        command.arg(&sidecar_path);
        command
    } else {
        Command::new(&sidecar_path)
    };
    let mut child = command
        .arg(request_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ToolError::new(format!("failed to start browser sidecar: {error}")))?;
    let started = Instant::now();
    let (cancelled, timed_out) = loop {
        if control.should_cancel() {
            let _ = child.kill();
            break (true, false);
        }
        if started.elapsed() >= hard_timeout {
            let _ = child.kill();
            break (false, true);
        }
        if child
            .try_wait()
            .map_err(|error| ToolError::new(format!("failed to poll browser sidecar: {error}")))?
            .is_some()
        {
            break (false, false);
        }
        thread::sleep(Duration::from_millis(40));
    };
    let output = child
        .wait_with_output()
        .map_err(|error| ToolError::new(format!("failed to collect browser sidecar: {error}")))?;
    if !cancelled && !timed_out && !output.status.success() {
        return Err(ToolError::new(format!(
            "browser sidecar failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(ControlledSidecarOutput {
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        cancelled,
        timed_out,
    })
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
    use std::sync::atomic::{AtomicBool, Ordering};
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
        assert!(specs.iter().any(|spec| spec.name == "browser.open"));
        assert!(specs.iter().any(|spec| spec.name == "browser.extract_text"));
        assert!(specs.iter().any(|spec| spec.name == "browser.capture"));
        assert!(specs.iter().any(|spec| spec.name == "browser.click"));
        assert!(specs.iter().any(|spec| spec.name == "browser.type"));
        assert!(specs.iter().any(|spec| spec.name == "browser.scroll"));
        assert!(specs.iter().any(|spec| spec.name == "browser.tabs"));
        assert!(specs.iter().any(|spec| spec.name == "browser.select_tab"));
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
    fn configured_image_model_registers_a_permissioned_generation_tool() {
        let root = temp_workspace();
        let registry = ToolRegistry::with_workspace_tools_and_services(
            root.clone(),
            WebSearchConfig::default(),
            Some(ImageGenerationConfig {
                base_url: "https://example.test/v1".to_string(),
                api_key: "secret".to_string(),
                model: "image-model-a".to_string(),
                timeout_seconds: 300,
            }),
        );
        let tool = registry
            .get("image.generate")
            .expect("configured image tool should be registered");
        let spec = tool.spec();
        let permission = tool
            .permission_request(&invocation(
                "image.generate",
                r#"{"prompt":"A blue circle","output_path":"art/circle.png"}"#
                    .to_string(),
            ))
            .expect("image generation should require permission");
        let (_, output_path) = image_output_path(&root, Some("art/circle.jpg"), "image/png")
            .expect("image output path should resolve");

        assert_eq!(spec.namespace, "image");
        assert!(spec.input_schema_json.contains("output_path"));
        assert_eq!(permission.risk, PermissionRisk::Network);
        assert_eq!(permission.scope, "art/circle.png");
        assert!(!permission
            .metadata
            .values()
            .any(|value| value.contains("blue circle")));
        assert_eq!(output_path, "art/circle.png");
    }

    #[test]
    fn configured_web_search_rejects_non_http_endpoint() {
        let tool = WebSearchTool::new(WebSearchConfig {
            endpoint: "file:///tmp/search".to_string(),
            api_key: "secret".to_string(),
        });
        let error = tool
            .execute(invocation(
                "web.search",
                encode_input(&[("query", "local agent")]),
            ))
            .expect_err("non-HTTP endpoint must fail before a request is sent");
        assert!(error.message.contains("HTTP or HTTPS"));
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

    #[cfg(unix)]
    #[test]
    fn shell_stops_background_processes_after_command_completion() {
        let shell = ShellRunTool::new(temp_workspace());
        let started = Instant::now();
        let result = shell
            .execute(invocation(
                "shell.run",
                encode_input(&[
                    ("command", "sleep 30 & echo started"),
                    ("timeout_seconds", "5"),
                ]),
            ))
            .expect("background command should finish");

        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        assert!(result.output.contains("started"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn shell_times_out_and_reports_the_bound() {
        let shell = ShellRunTool::new(temp_workspace());
        let started = Instant::now();
        let result = shell
            .execute(invocation(
                "shell.run",
                encode_input(&[
                    ("command", "sleep 30"),
                    ("timeout_seconds", "1"),
                ]),
            ))
            .expect("timeout should be returned as a tool result");

        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert!(result.output.contains("timed out after 1 seconds"));
        assert_eq!(result.metadata.get("timed_out").map(String::as_str), Some("true"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn shell_cancellation_stops_the_process_group_promptly() {
        let shell = ShellRunTool::new(temp_workspace());
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = Arc::clone(&cancelled);
        let control = ToolExecutionControl::new(move || cancellation.load(Ordering::SeqCst));
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            cancelled.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = shell
            .execute_with_control(
                invocation(
                    "shell.run",
                    encode_input(&[("command", "sleep 30"), ("timeout_seconds", "30")]),
                ),
                &control,
            )
            .expect("cancellation should be returned as a tool result");
        trigger.join().expect("cancellation trigger should finish");

        assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
        assert_eq!(result.metadata.get("cancelled").map(String::as_str), Some("true"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn network_and_browser_tools_request_permission() {
        let web = WebSearchTool::default();
        let browser = BrowserTool::extract_text(temp_workspace());
        let browser_type = BrowserTool::type_text(temp_workspace());
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
    fn browser_control_requires_a_configured_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_BROWSER_SIDECAR");
        let root = temp_workspace();
        let tool = BrowserTool::click(root);

        let error = tool
            .execute(invocation(
                "browser.click",
                encode_input(&[("selector", "button.primary")]),
            ))
            .expect_err("browser action must not pretend it ran without a controller");

        assert!(error.message.contains("CINDX_BROWSER_SIDECAR is not configured"));
    }

    #[test]
    fn browser_action_executes_configured_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let sidecar = root.join("browser-sidecar-test.sh");
        let trace = root.join("browser-trace.json");
        fs::write(&trace, "{}\n").expect("trace should write");
        fs::write(
            &sidecar,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::json!({
                    "schema": BROWSER_CONTROL_RESPONSE_SCHEMA,
                    "ok": true,
                    "output": "sidecar-test-ok",
                    "controller": "cdp_playwright",
                    "artifacts": [],
                    "trace_path": trace.display().to_string(),
                    "duration_ms": 1
                })
            ),
        )
        .expect("sidecar should write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o700))
                .expect("sidecar should be executable");
        }
        env::set_var("CINDX_BROWSER_SIDECAR", &sidecar);
        let tool = BrowserTool::click(root);

        let result = tool
            .execute(invocation(
                "browser.click",
                encode_input(&[("selector", "button.primary")]),
            ))
            .expect("browser sidecar should execute");

        assert_eq!(
            result.metadata.get("controller").map(String::as_str),
            Some("cdp_playwright")
        );
        assert!(result.output.contains("sidecar-test-ok"));
        assert!(result.metadata.contains_key("trace_path"));
        env::remove_var("CINDX_BROWSER_SIDECAR");
    }

    #[test]
    fn browser_request_preserves_session_tab_frame_and_semantic_target() {
        let root = temp_workspace();
        let session_dir = root.join("session");
        let output_dir = root.join("artifacts");
        let input = parse_input(&encode_input(&[
            ("tab_id", "target-1"),
            ("frame", "child"),
            ("role", "button"),
            ("name", "Continue"),
            ("wait_for", "#complete"),
            ("timeout_ms", "9000"),
        ]));

        let request = browser_request_json(
            "browser-test",
            BrowserToolKind::Click,
            "session-alpha",
            &session_dir,
            &output_dir,
            &input,
        )
        .expect("browser request should encode");
        let request: serde_json::Value =
            serde_json::from_str(&request).expect("browser request should be JSON");

        assert_eq!(request["schema"], BROWSER_CONTROL_REQUEST_SCHEMA);
        assert_eq!(request["action"], "click");
        assert_eq!(request["session_id"], "session-alpha");
        assert_eq!(request["tab_id"], "target-1");
        assert_eq!(request["frame"], "child");
        assert_eq!(request["role"], "button");
        assert_eq!(request["name"], "Continue");
        assert_eq!(request["wait_for"], "#complete");
        assert_eq!(request["timeout_ms"], "9000");
        assert_eq!(request["session_dir"], session_dir.display().to_string());
        assert_eq!(request["output_dir"], output_dir.display().to_string());
    }

    #[test]
    fn browser_sidecar_execution_is_cancellable() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let sidecar = root.join("browser-sidecar-slow.sh");
        fs::write(&sidecar, "#!/bin/sh\nsleep 30\n").expect("sidecar should write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o700))
                .expect("sidecar should be executable");
        }
        env::set_var("CINDX_BROWSER_SIDECAR", &sidecar);
        let tool = BrowserTool::click(root);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = Arc::clone(&cancelled);
        let control = ToolExecutionControl::new(move || cancellation.load(Ordering::SeqCst));
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            cancelled.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();

        let result = tool
            .execute_with_control(
                invocation(
                    "browser.click",
                    encode_input(&[("selector", "button.primary")]),
                ),
                &control,
            )
            .expect("browser cancellation should return a tool result");
        trigger.join().expect("cancellation trigger should finish");

        assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2));
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
