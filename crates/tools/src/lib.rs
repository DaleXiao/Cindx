use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_core::{
    Metadata, PermissionRequest, PermissionRequestId, PermissionRisk, TaskId, ToolArtifact,
    ToolCallId, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
use model_provider::{
    ImageGenerationRequest, OpenAiCompatibleImageConfig, OpenAiCompatibleImageProvider,
    MODEL_REQUEST_CANCELLED,
};
mod desktop_control;
mod file_batch;

#[cfg(test)]
use desktop_control::{
    browser_request_json, BrowserToolKind, BROWSER_CONTROL_REQUEST_SCHEMA,
    BROWSER_CONTROL_RESPONSE_SCHEMA, COMPUTER_CONTROL_RESPONSE_SCHEMA,
};
pub use desktop_control::{BrowserTool, ComputerTool};
pub use file_batch::ReadFilesTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: "tool_error".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: true,
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
        Self {
            should_cancel: Arc::new(should_cancel),
        }
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
        let mut result = self.execute(invocation)?;
        if control.should_cancel() {
            result.metadata.insert(
                "cancel_requested_after_completion".to_string(),
                "true".to_string(),
            );
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
        registry.register(Box::new(ReadFilesTool::new(workspace_root.clone())));
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
        let _ = self.try_register(tool);
    }

    pub fn try_register(&mut self, tool: Box<dyn Tool>) -> Result<bool, String> {
        let spec = tool.spec();
        spec.validate_input_schema()?;
        if self.tools.contains_key(&spec.name) {
            return Ok(false);
        }
        self.tools.insert(spec.name, Arc::from(tool));
        Ok(true)
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
    if spec.name == "image.generate" && prompt_requests_image_generation(query) {
        score += 1_000;
    }
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

pub fn prompt_requests_image_generation(prompt: &str) -> bool {
    let prompt = prompt.to_lowercase();
    const DIRECT_PATTERNS: &[&str] = &[
        "生图",
        "生成图片",
        "生成图像",
        "生成一张图",
        "生成一幅图",
        "画一张",
        "画一幅",
        "制作图片",
        "制作图像",
        "创建图片",
        "创建图像",
        "generate an image",
        "generate image",
        "create an image",
        "create image",
        "draw an image",
        "draw a picture",
        "make an image",
        "render an image",
    ];
    if DIRECT_PATTERNS
        .iter()
        .any(|pattern| prompt.contains(pattern))
    {
        return true;
    }

    let has_visual_noun = [
        "图片",
        "图像",
        "插画",
        "海报",
        "头像",
        "壁纸",
        "精灵图",
        "sprite",
        "illustration",
        "poster",
        "wallpaper",
        "image",
        "picture",
    ]
    .iter()
    .any(|token| prompt.contains(token));
    let has_generation_action = [
        "生成",
        "绘制",
        "画出",
        "制作",
        "设计",
        "create",
        "generate",
        "draw",
        "render",
        "illustrate",
    ]
    .iter()
    .any(|token| prompt.contains(token));
    has_visual_noun && has_generation_action
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
            "Read a bounded UTF-8 byte range inside the workspace. Large files return a continuation offset.",
            ToolRisk::ReadOnly,
            "path=<workspace-relative-path>\noffset_bytes=<optional byte offset, default 0>\nmax_bytes=<optional 1-262144, default 131072>",
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input = parse_input(&invocation.input_json);
        let path = required_input(&input, "path")?;
        let offset_bytes = parse_bounded_usize_input(&input, "offset_bytes", 0, 0, usize::MAX)?;
        let max_bytes = parse_bounded_usize_input(
            &input,
            "max_bytes",
            DEFAULT_FILE_READ_BYTES,
            1,
            MAX_FILE_READ_BYTES,
        )?;
        let resolved = resolve_workspace_path(&self.workspace_root, &path)?;
        let resolved = resolve_workspace_read_path(&self.workspace_root, &resolved)?;
        reject_sensitive_read_path(&self.workspace_root, &resolved)?;
        let mut file = fs::File::open(&resolved)
            .map_err(|error| ToolError::new(format!("failed to read file: {error}")))?;
        let total_bytes = file
            .metadata()
            .map_err(|error| ToolError::new(format!("failed to inspect file: {error}")))?
            .len();
        let offset = (offset_bytes as u64).min(total_bytes);
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| ToolError::new(format!("failed to seek file: {error}")))?;
        let mut bytes = Vec::with_capacity(max_bytes.saturating_add(4));
        file.take(max_bytes.saturating_add(4) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| ToolError::new(format!("failed to read file range: {error}")))?;
        let skipped_prefix = bytes
            .iter()
            .take(3)
            .take_while(|byte| **byte & 0b1100_0000 == 0b1000_0000)
            .count();
        if skipped_prefix > 0 {
            bytes.drain(..skipped_prefix);
        }
        let offset = offset.saturating_add(skipped_prefix as u64);
        let end = utf8_page_end(&bytes, max_bytes);
        bytes.truncate(end);
        let returned_bytes = bytes.len();
        let next_offset = offset.saturating_add(returned_bytes as u64);
        let truncated = next_offset < total_bytes;
        let mut output = String::from_utf8_lossy(&bytes).to_string();
        if truncated {
            if !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!(
                "\n[File read bounded at {returned_bytes} bytes. Continue with offset_bytes={next_offset}. Total file size: {total_bytes} bytes.]"
            ));
        }
        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path);
        metadata.insert("bytes".to_string(), total_bytes.to_string());
        metadata.insert("offset_bytes".to_string(), offset.to_string());
        metadata.insert("returned_bytes".to_string(), returned_bytes.to_string());
        metadata.insert("next_offset_bytes".to_string(), next_offset.to_string());
        metadata.insert("truncated".to_string(), truncated.to_string());

        Ok(tool_result(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            output,
            metadata,
        ))
    }
}

fn utf8_page_end(bytes: &[u8], max_bytes: usize) -> usize {
    let candidate = bytes.len().min(max_bytes);
    match std::str::from_utf8(&bytes[..candidate]) {
        Ok(_) => candidate,
        Err(error) if error.error_len().is_none() => {
            let valid = error.valid_up_to();
            if valid > 0 {
                return valid;
            }
            let width = utf8_sequence_width(bytes.first().copied().unwrap_or_default());
            if width > 1 && bytes.len() >= width && std::str::from_utf8(&bytes[..width]).is_ok() {
                width
            } else {
                candidate
            }
        }
        Err(_) => candidate,
    }
}

fn utf8_sequence_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
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
            let entry = entry.map_err(|error| {
                ToolError::new(format!("failed to read directory entry: {error}"))
            })?;
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
        search_directory(
            &self.workspace_root,
            &root,
            &query,
            max_results,
            &mut results,
        )?;

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
        let session_key = invocation
            .metadata
            .get("session_id")
            .map(|session_id| stable_hash(session_id).to_string())
            .unwrap_or_else(|| "unscoped".to_string());
        let version_key = stable_hash(&invocation.id.0).to_string();
        let snapshot_relative = PathBuf::from(".cindx")
            .join("output-history")
            .join(session_key)
            .join(version_key)
            .join(&path);
        let snapshot = self.workspace_root.join(&snapshot_relative);
        if let Some(parent) = snapshot.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(format!(
                    "failed to create output history directory: {error}"
                ))
            })?;
        }
        fs::write(&snapshot, content.as_bytes()).map_err(|error| {
            ToolError::new(format!("failed to preserve output version: {error}"))
        })?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolError::new(format!("failed to create parent directory: {error}"))
            })?;
        }
        fs::write(&resolved, content.as_bytes())
            .map_err(|error| ToolError::new(format!("failed to write file: {error}")))?;

        let mut metadata = Metadata::new();
        metadata.insert("path".to_string(), path);
        metadata.insert("source_path".to_string(), resolved.display().to_string());
        metadata.insert(
            "artifact_path".to_string(),
            snapshot_relative.display().to_string(),
        );
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
const DEFAULT_FILE_READ_BYTES: usize = 128 * 1024;
const MAX_FILE_READ_BYTES: usize = 256 * 1024;
const SHELL_STREAM_PREVIEW_BYTES: usize = 64 * 1024;
const SHELL_STREAM_ARTIFACT_MAX_BYTES: u64 = 32 * 1024 * 1024;
const WEB_RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;
const WEB_STDERR_MAX_BYTES: usize = 256 * 1024;
const SEARCH_FILE_SCAN_MAX_BYTES: u64 = 8 * 1024 * 1024;
const SEARCH_MATCH_PREVIEW_CHARS: usize = 512;

struct ShellCommandOutput {
    status: ExitStatus,
    stdout: BoundedStreamCapture,
    stderr: BoundedStreamCapture,
    timed_out: bool,
    cancelled: bool,
}

#[derive(Default)]
struct BoundedStreamCapture {
    preview: Vec<u8>,
    total_bytes: u64,
    artifact_bytes: u64,
    preview_truncated: bool,
    artifact_truncated: bool,
    artifact_path: Option<PathBuf>,
    artifact_error: Option<String>,
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

fn capture_process_stream(mut stream: impl Read, artifact_path: PathBuf) -> BoundedStreamCapture {
    let mut artifact = fs::File::create(&artifact_path).ok();
    let mut artifact_error = artifact
        .is_none()
        .then(|| format!("failed to create {}", artifact_path.display()));
    let head_limit = SHELL_STREAM_PREVIEW_BYTES / 2;
    let tail_limit = SHELL_STREAM_PREVIEW_BYTES.saturating_sub(head_limit);
    let mut head = Vec::with_capacity(head_limit);
    let mut tail = Vec::with_capacity(tail_limit);
    let mut buffer = [0u8; 16 * 1024];
    let mut total_bytes = 0u64;
    let mut artifact_bytes = 0u64;

    loop {
        let count = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                artifact_error
                    .get_or_insert_with(|| format!("failed to read process stream: {error}"));
                break;
            }
        };
        let chunk = &buffer[..count];
        total_bytes = total_bytes.saturating_add(count as u64);

        let mut retained_in_head = 0usize;
        if head.len() < head_limit {
            let retained = (head_limit - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..retained]);
            retained_in_head = retained;
        }
        tail.extend_from_slice(&chunk[retained_in_head..]);
        if tail.len() > tail_limit {
            let excess = tail.len() - tail_limit;
            tail.drain(..excess);
        }

        if let Some(file) = artifact.as_mut() {
            let remaining = SHELL_STREAM_ARTIFACT_MAX_BYTES.saturating_sub(artifact_bytes);
            let writable = (remaining as usize).min(chunk.len());
            if writable > 0 {
                if let Err(error) = file.write_all(&chunk[..writable]) {
                    artifact_error = Some(format!("failed to write process artifact: {error}"));
                    artifact = None;
                } else {
                    artifact_bytes = artifact_bytes.saturating_add(writable as u64);
                }
            }
        }
    }

    if let Some(file) = artifact.as_mut() {
        if let Err(error) = file.flush() {
            artifact_error = Some(format!("failed to flush process artifact: {error}"));
        }
    }
    let preview_truncated = total_bytes > (head.len() + tail.len()) as u64;
    let mut preview = head;
    if preview_truncated {
        preview.extend_from_slice(b"\n...[middle output omitted from preview]...\n");
    }
    preview.extend_from_slice(&tail);
    let artifact_truncated = total_bytes > artifact_bytes;
    let keep_artifact = preview_truncated && artifact_error.is_none() && artifact_bytes > 0;
    if !keep_artifact {
        let _ = fs::remove_file(&artifact_path);
    }

    BoundedStreamCapture {
        preview,
        total_bytes,
        artifact_bytes,
        preview_truncated,
        artifact_truncated,
        artifact_path: keep_artifact.then_some(artifact_path),
        artifact_error,
    }
}

fn run_shell_command(
    command: &str,
    cwd: &Path,
    timeout_seconds: u64,
    control: &ToolExecutionControl,
    artifact_dir: &Path,
) -> Result<ShellCommandOutput, ToolError> {
    fs::create_dir_all(artifact_dir).map_err(|error| {
        ToolError::new(format!("failed to create shell output directory: {error}"))
    })?;
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
    let stdout_path = artifact_dir.join("stdout.log");
    let stderr_path = artifact_dir.join("stderr.log");
    let stdout_reader = thread::spawn(move || capture_process_stream(stdout, stdout_path));
    let stderr_reader = thread::spawn(move || capture_process_stream(stderr, stderr_path));
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
        let artifact_dir = self
            .workspace_root
            .join(".cindx")
            .join("tool-output")
            .join(format!("{:016x}", stable_hash(&invocation.id.0)));
        let output = run_shell_command(
            &command,
            &resolved_cwd,
            timeout_seconds,
            control,
            &artifact_dir,
        )?;

        let mut combined = String::new();
        combined.push_str(&String::from_utf8_lossy(&output.stdout.preview));
        if !output.stderr.preview.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&String::from_utf8_lossy(&output.stderr.preview));
        }
        append_stream_capture_note(&mut combined, "stdout", &output.stdout);
        append_stream_capture_note(&mut combined, "stderr", &output.stderr);
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
            "stdout_bytes".to_string(),
            output.stdout.total_bytes.to_string(),
        );
        metadata.insert(
            "stderr_bytes".to_string(),
            output.stderr.total_bytes.to_string(),
        );
        metadata.insert(
            "output_truncated".to_string(),
            (output.stdout.preview_truncated || output.stderr.preview_truncated).to_string(),
        );
        metadata.insert(
            "exit_code".to_string(),
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
        );

        let mut result = tool_result(
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
        );
        for (label, capture) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
            if let Some(path) = &capture.artifact_path {
                result.artifacts.push(ToolArtifact {
                    path: path.display().to_string(),
                    mime_type: Some("text/plain".to_string()),
                    title: Some(format!("Shell {label}")),
                });
                result
                    .metadata
                    .insert(format!("{label}_artifact_path"), path.display().to_string());
                result.metadata.insert(
                    format!("{label}_artifact_bytes"),
                    capture.artifact_bytes.to_string(),
                );
                result.metadata.insert(
                    format!("{label}_artifact_truncated"),
                    capture.artifact_truncated.to_string(),
                );
            }
        }
        Ok(result)
    }
}

fn append_stream_capture_note(output: &mut String, label: &str, capture: &BoundedStreamCapture) {
    if !capture.preview_truncated && capture.artifact_error.is_none() {
        return;
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    if let Some(path) = &capture.artifact_path {
        output.push_str(&format!(
            "\n[{label} preview bounded; {} bytes produced. Captured {} bytes at {}{}]",
            capture.total_bytes,
            capture.artifact_bytes,
            path.display(),
            if capture.artifact_truncated {
                "; artifact reached the 32 MB safety limit, rerun a narrower command for omitted data"
            } else {
                ""
            }
        ));
    } else if let Some(error) = &capture.artifact_error {
        output.push_str(&format!(
            "\n[{label} preview bounded; {} bytes produced; artifact unavailable: {error}]",
            capture.total_bytes
        ));
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
            format!(
                "Generate one raster image using the user-configured model `{}` and save it in the active workspace. The model and provider are controlled by Settings and cannot be overridden in tool input.",
                self.config.model
            ),
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
        let (resolved_path, relative_path) =
            image_output_path(&self.workspace_root, requested_path, &image.mime_type)?;
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
            "x" | "y" | "delta_x" | "delta_y" | "limit" | "max_results" | "timeout_ms" => "integer",
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

fn parse_bounded_usize_input(
    input: &BTreeMap<String, String>,
    key: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, ToolError> {
    let value = match input.get(key) {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| ToolError::new(format!("{key} must be an integer")))?,
        None => default,
    };
    if value < minimum || value > maximum {
        return Err(ToolError::new(format!(
            "{key} must be between {minimum} and {maximum}"
        )));
    }
    Ok(value)
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
        id: PermissionRequestId(format!(
            "perm-{}",
            stable_hash(&format!("{action}:{scope:?}"))
        )),
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
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
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
        return Err(ToolError::new(
            "path escapes the workspace through a symbolic link",
        ));
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
            ".npmrc" | ".pypirc" | "credentials" | "credentials.json" | "id_rsa" | "id_ed25519"
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
        let entry = entry
            .map_err(|error| ToolError::new(format!("failed to read directory entry: {error}")))?;
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
        let metadata = entry
            .metadata()
            .map_err(|error| ToolError::new(format!("failed to read metadata: {error}")))?;
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
    let Ok(file) = fs::File::open(path) else {
        return Ok(());
    };
    let relative = path.strip_prefix(workspace_root).unwrap_or(path);
    let mut reader = BufReader::new(file.take(SEARCH_FILE_SCAN_MAX_BYTES));
    let mut line = String::new();
    let mut index = 0usize;
    loop {
        if results.len() >= max_results {
            break;
        }
        line.clear();
        let Ok(read) = reader.read_line(&mut line) else {
            break;
        };
        if read == 0 {
            break;
        }
        index += 1;
        if line.contains(query) {
            let preview: String = line
                .trim()
                .chars()
                .take(SEARCH_MATCH_PREVIEW_CHARS)
                .collect();
            results.push(format!("{}:{}:{}", relative.display(), index, preview));
        }
    }
    Ok(())
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
    let mut command = Command::new("/usr/bin/curl");
    command
        .arg("-L")
        .arg("--silent")
        .arg("--show-error")
        .arg("--max-time")
        .arg("25")
        .arg("--user-agent")
        .arg("LocalAgent/0.1")
        .arg(url);
    let output = run_command_with_limited_output(
        &mut command,
        WEB_RESPONSE_MAX_BYTES,
        WEB_STDERR_MAX_BYTES,
        "curl",
    )?;

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
    command.arg(&url);
    let output = run_command_with_limited_output(
        &mut command,
        WEB_RESPONSE_MAX_BYTES,
        WEB_STDERR_MAX_BYTES,
        "search API request",
    )?;
    if !output.status.success() {
        return Err(ToolError::new(format!(
            "search API request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[derive(Default)]
struct LimitedStreamCapture {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
    error: Option<String>,
}

fn capture_stream_limited(mut stream: impl Read, max_bytes: usize) -> LimitedStreamCapture {
    let mut capture = LimitedStreamCapture {
        bytes: Vec::with_capacity(max_bytes.min(64 * 1024)),
        ..LimitedStreamCapture::default()
    };
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                capture.error = Some(error.to_string());
                break;
            }
        };
        capture.total_bytes = capture.total_bytes.saturating_add(count as u64);
        let remaining = max_bytes.saturating_sub(capture.bytes.len());
        let retained = remaining.min(count);
        capture.bytes.extend_from_slice(&buffer[..retained]);
        capture.truncated |= retained < count;
    }
    capture
}

struct LimitedCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_command_with_limited_output(
    command: &mut Command,
    stdout_max_bytes: usize,
    stderr_max_bytes: usize,
    label: &str,
) -> Result<LimitedCommandOutput, ToolError> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ToolError::new(format!("failed to run {label}: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::new(format!("{label} stdout is unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::new(format!("{label} stderr is unavailable")))?;
    let stdout_reader = thread::spawn(move || capture_stream_limited(stdout, stdout_max_bytes));
    let stderr_reader = thread::spawn(move || capture_stream_limited(stderr, stderr_max_bytes));
    let status = child
        .wait()
        .map_err(|error| ToolError::new(format!("failed to wait for {label}: {error}")))?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| ToolError::new(format!("{label} stdout reader panicked")))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ToolError::new(format!("{label} stderr reader panicked")))?;
    if let Some(error) = stdout.error {
        return Err(ToolError::new(format!(
            "failed to read {label} stdout: {error}"
        )));
    }
    if stdout.truncated {
        return Err(ToolError::new(format!(
            "{label} response exceeded the {stdout_max_bytes} byte safety limit ({} bytes produced)",
            stdout.total_bytes
        )));
    }
    if let Some(error) = stderr.error {
        return Err(ToolError::new(format!(
            "failed to read {label} stderr: {error}"
        )));
    }
    Ok(LimitedCommandOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
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

    struct InvalidSchemaTool;

    struct CompletesWhileCancellationArrives {
        cancelled: Arc<AtomicBool>,
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

    impl Tool for CompletesWhileCancellationArrives {
        fn spec(&self) -> ToolSpec {
            ToolSpec::builtin(
                "test.completion_race",
                "test",
                "Completes while cancellation arrives",
                ToolRisk::ReadOnly,
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
            )
        }

        fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
            None
        }

        fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
            self.cancelled.store(true, Ordering::SeqCst);
            Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Succeeded,
                "side effect completed",
                Metadata::new(),
            ))
        }
    }

    impl Tool for InvalidSchemaTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::builtin(
                "invalid.schema",
                "test",
                "Invalid schema fixture",
                ToolRisk::ReadOnly,
                r#"{"type":"string"}"#,
            )
        }

        fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
            None
        }

        fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Succeeded,
                "invalid",
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
                r#"{"prompt":"A blue circle","output_path":"art/circle.png"}"#.to_string(),
            ))
            .expect("image generation should require permission");
        let (_, output_path) = image_output_path(&root, Some("art/circle.jpg"), "image/png")
            .expect("image output path should resolve");

        assert_eq!(spec.namespace, "image");
        assert!(spec.input_schema_json.contains("output_path"));
        assert!(!spec.input_schema_json.contains("\"model\""));
        assert!(spec.description.contains("image-model-a"));
        assert_eq!(permission.risk, PermissionRisk::Network);
        assert_eq!(permission.scope, "art/circle.png");
        assert!(!permission
            .metadata
            .values()
            .any(|value| value.contains("blue circle")));
        assert_eq!(output_path, "art/circle.png");
    }

    #[test]
    fn image_generation_intent_keeps_the_configured_tool_inline() {
        let mut registry = ToolRegistry::with_workspace_tools_and_services(
            temp_workspace(),
            WebSearchConfig::default(),
            Some(ImageGenerationConfig {
                base_url: "https://example.test/v1".to_string(),
                api_key: "secret".to_string(),
                model: "image-model-a".to_string(),
                timeout_seconds: 300,
            }),
        );
        for index in 0..30 {
            registry.register(Box::new(CatalogTool {
                name: format!("catalog.tool_{index}"),
            }));
        }
        registry.install_meta_tools();

        let plan = registry.exposure_plan("请生成一张写实的猫咪图片", 16_000);

        assert!(prompt_requests_image_generation("请生成一张写实的猫咪图片"));
        assert!(plan.inline.iter().any(|spec| spec.name == "image.generate"));
        assert!(!prompt_requests_image_generation("检查这张图片的尺寸"));
    }

    #[test]
    fn registry_rejects_invalid_schemas_and_preserves_the_first_tool_owner() {
        let mut registry = ToolRegistry::new();
        assert_eq!(
            registry.try_register(Box::new(CatalogTool {
                name: "catalog.unique".to_string(),
            })),
            Ok(true)
        );
        assert_eq!(
            registry.try_register(Box::new(CatalogTool {
                name: "catalog.unique".to_string(),
            })),
            Ok(false)
        );
        assert!(registry
            .try_register(Box::new(InvalidSchemaTool))
            .expect_err("invalid schema must be rejected")
            .contains("must describe an object"));
        assert!(registry.get("catalog.unique").is_some());
        assert!(registry.get("invalid.schema").is_none());
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
        assert_eq!(
            request.metadata.get("tool_name").map(String::as_str),
            Some("file.write")
        );
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
                encode_input(&[
                    ("path", "notes/today.txt"),
                    ("content", "hello workspace\nline two"),
                ]),
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
    fn file_read_paginates_utf8_without_splitting_characters() {
        let root = temp_workspace();
        fs::write(root.join("multibyte.txt"), "你好吗")
            .expect("multibyte fixture should be written");
        let reader = ReadFileTool::new(root);

        let first = reader
            .execute(invocation(
                "file.read",
                encode_input(&[
                    ("path", "multibyte.txt"),
                    ("offset_bytes", "0"),
                    ("max_bytes", "4"),
                ]),
            ))
            .expect("first page should succeed");
        let second = reader
            .execute(invocation(
                "file.read",
                encode_input(&[
                    ("path", "multibyte.txt"),
                    ("offset_bytes", "3"),
                    ("max_bytes", "4"),
                ]),
            ))
            .expect("second page should succeed");

        assert!(first.output.starts_with('你'));
        assert!(second.output.starts_with('好'));
        assert!(!first.output.contains('\u{fffd}'));
        assert!(!second.output.contains('\u{fffd}'));
        assert_eq!(
            first.metadata.get("next_offset_bytes").map(String::as_str),
            Some("3")
        );
        assert_eq!(
            second.metadata.get("next_offset_bytes").map(String::as_str),
            Some("6")
        );
    }

    #[test]
    fn write_file_preserves_an_immutable_session_output_version() {
        let root = temp_workspace();
        let writer = WriteFileTool::new(root.clone());
        let mut request = invocation(
            "file.write",
            encode_input(&[("path", "notes/versioned.txt"), ("content", "version one")]),
        );
        request
            .metadata
            .insert("session_id".to_string(), "session-alpha".to_string());

        let result = writer.execute(request).expect("write should succeed");
        let artifact_path = result
            .metadata
            .get("artifact_path")
            .expect("write should expose the immutable output version");
        let source_path = root.join("notes/versioned.txt").display().to_string();

        assert_eq!(
            result.metadata.get("source_path").map(String::as_str),
            Some(source_path.as_str())
        );
        assert_eq!(
            fs::read_to_string(root.join(artifact_path)).expect("snapshot should be readable"),
            "version one"
        );
        assert_eq!(
            fs::read_to_string(root.join("notes/versioned.txt"))
                .expect("workspace file should be readable"),
            "version one"
        );
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
    fn cancellation_after_completion_does_not_hide_a_completed_side_effect() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let observed_cancellation = Arc::clone(&cancelled);
        let control =
            ToolExecutionControl::new(move || observed_cancellation.load(Ordering::SeqCst));
        let tool = CompletesWhileCancellationArrives { cancelled };

        let result = tool
            .execute_with_control(
                invocation("test.completion_race", "{}".to_string()),
                &control,
            )
            .expect("the completed result should be preserved");

        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        assert_eq!(result.output, "side effect completed");
        assert_eq!(
            result
                .metadata
                .get("cancel_requested_after_completion")
                .map(String::as_str),
            Some("true")
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
    fn shell_large_output_uses_a_bounded_preview_and_artifact() {
        let root = temp_workspace();
        let shell = ShellRunTool::new(root.clone());
        let result = shell
            .execute(invocation(
                "shell.run",
                encode_input(&[("command", "/usr/bin/yes x | /usr/bin/head -c 100000")]),
            ))
            .expect("large shell output should succeed");

        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        assert!(result.output.len() < 70 * 1024);
        assert_eq!(
            result.metadata.get("output_truncated").map(String::as_str),
            Some("true")
        );
        let artifact = result
            .metadata
            .get("stdout_artifact_path")
            .expect("large stdout should have an artifact");
        assert_eq!(
            fs::metadata(artifact)
                .expect("stdout artifact should exist")
                .len(),
            100_000
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_times_out_and_reports_the_bound() {
        let shell = ShellRunTool::new(temp_workspace());
        let started = Instant::now();
        let result = shell
            .execute(invocation(
                "shell.run",
                encode_input(&[("command", "sleep 30"), ("timeout_seconds", "1")]),
            ))
            .expect("timeout should be returned as a tool result");

        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert!(result.output.contains("timed out after 1 seconds"));
        assert_eq!(
            result.metadata.get("timed_out").map(String::as_str),
            Some("true")
        );
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
        assert_eq!(
            result.metadata.get("cancelled").map(String::as_str),
            Some("true")
        );
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

        assert!(error
            .message
            .contains("CINDX_BROWSER_SIDECAR is not configured"));
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
    fn computer_action_requires_a_configured_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_COMPUTER_SIDECAR");
        let root = temp_workspace();
        let tool = ComputerTool::click(root.clone());

        let error = tool
            .execute(invocation(
                "computer.click",
                encode_input(&[
                    ("x", "0"),
                    ("y", "0"),
                    ("output_dir", ".cindx/computer-actions"),
                ]),
            ))
            .expect_err("computer action must not pretend it ran without a controller");

        assert!(error.message.contains("computer sidecar is not configured"));
        assert!(fs::read_dir(root.join(".cindx/computer-actions"))
            .expect("computer action directory should exist")
            .next()
            .is_none());
    }

    #[test]
    fn computer_screenshot_does_not_report_a_placeholder_as_success() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_COMPUTER_SIDECAR");
        env::set_var("CINDX_DISABLE_NATIVE_SCREENSHOT", "1");
        let root = temp_workspace();
        let tool = ComputerTool::screenshot(root.clone());

        let error = tool
            .execute(invocation(
                "computer.screenshot",
                encode_input(&[
                    ("redaction", "manual"),
                    ("output_dir", ".cindx/computer-actions"),
                ]),
            ))
            .expect_err("missing screenshot pixels must be an error");
        env::remove_var("CINDX_DISABLE_NATIVE_SCREENSHOT");

        assert!(error.message.contains("desktop screenshot failed"));
    }

    #[test]
    fn computer_action_executes_configured_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let sidecar = root.join("computer-sidecar-test.sh");
        fs::write(
            &sidecar,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::json!({
                    "schema": COMPUTER_CONTROL_RESPONSE_SCHEMA,
                    "ok": true,
                    "output": "computer-sidecar-test-ok",
                    "controller": "native_macos",
                    "artifacts": [],
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
        env::set_var("CINDX_COMPUTER_SIDECAR", &sidecar);
        let tool = ComputerTool::click(root);

        let result = tool
            .execute(invocation(
                "computer.click",
                encode_input(&[("x", "0"), ("y", "0")]),
            ))
            .expect("configured computer sidecar should execute");
        env::remove_var("CINDX_COMPUTER_SIDECAR");

        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        assert_eq!(result.output, "computer-sidecar-test-ok");
        assert_eq!(
            result.metadata.get("controller").map(String::as_str),
            Some("native_macos")
        );
    }

    #[cfg(unix)]
    #[test]
    fn computer_action_cancellation_stops_the_sidecar_promptly() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let sidecar = root.join("computer-sidecar-cancel.sh");
        fs::write(&sidecar, "#!/bin/sh\nsleep 30\n").expect("sidecar should write");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o700))
            .expect("sidecar should be executable");
        env::set_var("CINDX_COMPUTER_SIDECAR", &sidecar);
        let tool = ComputerTool::click(root);
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
                invocation("computer.click", encode_input(&[("x", "20"), ("y", "30")])),
                &control,
            )
            .expect("computer cancellation should return a tool result");
        trigger.join().expect("cancellation trigger should finish");
        env::remove_var("CINDX_COMPUTER_SIDECAR");

        assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn html_text_extraction_strips_tags_and_decodes_entities() {
        let text = html_to_text(
            "<html><body><h1>Local &amp; Agent</h1><p>RAG&nbsp;ready</p></body></html>",
        );

        assert!(text.contains("Local & Agent"));
        assert!(text.contains("RAG ready"));
        assert!(!text.contains("<h1>"));
    }

    #[test]
    fn url_validation_requires_http_scheme() {
        let invalid = parse_input(&encode_input(&[("url", "file:///tmp/page.html")]));
        let valid = parse_input(&encode_input(&[("url", "https://example.com")]));

        assert!(required_url(&invalid).is_err());
        assert_eq!(
            required_url(&valid).expect("url should pass"),
            "https://example.com"
        );
    }

    #[test]
    fn rejects_paths_that_escape_workspace() {
        let root = temp_workspace();
        let reader = ReadFileTool::new(root);

        let error = reader
            .execute(invocation(
                "file.read",
                encode_input(&[("path", "../secret")]),
            ))
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
