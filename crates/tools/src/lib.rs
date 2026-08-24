use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use std::{
    env, fs, thread,
    time::{Duration, Instant},
};

use agent_core::{
    Metadata, PermissionRequest, ToolEffectSemantics, ToolInvocation, ToolOutcomeStatus,
    ToolPostconditionEvidence, ToolResult, ToolRisk, ToolSpec,
};
mod browser_session_retirement;
mod desktop_control;
mod file_batch;
mod file_glob;
mod file_list;
mod file_patch;
mod file_query_contract_v3;
mod file_search;
mod file_tools;
mod image_generation;
mod meta_invoke;
mod meta_tools;
mod postcondition_evidence;
mod private_file;
mod process_capture;
mod process_contract;
mod process_control;
mod process_cpu;
mod process_runtime;
mod process_supervisor;
#[cfg(test)]
mod process_tests;
mod process_time;
mod process_tools;
mod shell;
mod shell_postcondition;
mod stream_capture;
mod subagent_tool;
mod todo_tool;
mod tool_contract_v2;
mod tool_support;
mod web_fetch;
mod web_search;
mod workspace_file;

pub use browser_session_retirement::retire_browser_session;
#[cfg(test)]
use desktop_control::{
    browser_request_json, BrowserToolKind, BROWSER_CONTROL_REQUEST_SCHEMA,
    BROWSER_CONTROL_RESPONSE_SCHEMA, COMPUTER_CONTROL_RESPONSE_SCHEMA,
};
pub use desktop_control::{BrowserTool, ComputerTool};
pub use file_batch::ReadFilesTool;
pub use file_glob::GlobFilesTool;
pub use file_list::ListDirectoryTool;
pub use file_patch::PatchFileTool;
pub use file_search::SearchFilesTool;
pub use file_tools::{ReadFileTool, WriteFileTool};
pub use image_generation::ImageGenerationTool;
pub use private_file::write_private_file_atomically;
pub use process_runtime::ProcessManager;
pub use process_tools::{
    ProcessInputTool, ProcessPollTool, ProcessStartTool, ProcessTerminateTool,
};
pub use shell::ShellRunTool;
pub use subagent_tool::SubagentTaskTool;
pub use todo_tool::TodoTool;
pub use tool_support::{encode_input, parse_input};
pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;

use meta_invoke::ToolInvokeMeta;
use meta_tools::{ToolInspectMeta, ToolSearchMeta};
pub(crate) use tool_support::{
    bounded_model_text, builtin_tool_spec, current_time_millis, input_value_is_true, json_field,
    model_observation, parse_bounded_usize_input, permission_request, required_input, required_url,
    resolve_workspace_path, resolve_workspace_read_path, stable_hash, tool_result,
};

#[cfg(test)]
use agent_core::{
    PermissionRequestId, PermissionRisk, PostconditionVerifierKind, TaskId, ToolCallId,
    ToolExecutionConcurrency,
};
#[cfg(test)]
use image_generation::image_output_path;
#[cfg(test)]
use web_search::html_to_text;

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

    fn effect_spec(&self, _invocation: &ToolInvocation) -> ToolSpec {
        self.spec()
    }

    fn postcondition_evidence(
        &self,
        _invocation: &ToolInvocation,
        _result: &ToolResult,
    ) -> Option<ToolPostconditionEvidence> {
        None
    }

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
pub struct ToolExposureIntent {
    pub preferred_namespaces: BTreeSet<String>,
    pub deferred_namespaces: BTreeSet<String>,
    pub required_tools: BTreeSet<String>,
    pub prefer_read_only: bool,
    pub prefer_effects: bool,
}

impl ToolExposureIntent {
    pub fn is_empty(&self) -> bool {
        self.preferred_namespaces.is_empty()
            && self.deferred_namespaces.is_empty()
            && self.required_tools.is_empty()
            && !self.prefer_read_only
            && !self.prefer_effects
    }
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
        Self::with_workspace_tools_and_services_and_process_manager(
            workspace_root,
            web_search_config,
            image_generation_config,
            Arc::new(ProcessManager::new()),
        )
    }

    pub fn with_workspace_tools_and_services_and_process_manager(
        workspace_root: impl Into<PathBuf>,
        web_search_config: WebSearchConfig,
        image_generation_config: Option<ImageGenerationConfig>,
        process_manager: Arc<ProcessManager>,
    ) -> Self {
        let workspace_root = workspace_root.into();
        let mut registry = Self::new();
        registry.register(Box::new(ReadFileTool::new(workspace_root.clone())));
        registry.register(Box::new(ReadFilesTool::new(workspace_root.clone())));
        registry.register(Box::new(ListDirectoryTool::new(workspace_root.clone())));
        registry.register(Box::new(SearchFilesTool::new(workspace_root.clone())));
        registry.register(Box::new(GlobFilesTool::new(workspace_root.clone())));
        registry.register(Box::new(PatchFileTool::new(workspace_root.clone())));
        registry.register(Box::new(WriteFileTool::new(workspace_root.clone())));
        registry.register(Box::new(TodoTool::new(workspace_root.clone())));
        registry.register(Box::new(SubagentTaskTool::new()));
        registry.register(Box::new(ShellRunTool::new(workspace_root.clone())));
        registry.register(Box::new(ProcessStartTool::new(
            workspace_root.clone(),
            Arc::clone(&process_manager),
        )));
        registry.register(Box::new(ProcessPollTool::new(Arc::clone(&process_manager))));
        registry.register(Box::new(ProcessInputTool::new(Arc::clone(
            &process_manager,
        ))));
        registry.register(Box::new(ProcessTerminateTool::new(process_manager)));
        registry.register(Box::new(WebSearchTool::new(web_search_config)));
        registry.register(Box::new(WebFetchTool));
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
        registry.register(Box::new(BrowserTool::close(workspace_root.clone())));
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
        spec.validate()?;
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

    pub fn permissionless_read_tool(
        &self,
        invocation: &ToolInvocation,
    ) -> Result<&dyn Tool, ToolError> {
        let tool = self
            .get(&invocation.tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {}", invocation.tool_name)))?;
        let effect = tool.effect_spec(invocation);
        if !matches!(effect.risk, ToolRisk::ReadOnly)
            || !matches!(effect.effect_semantics, ToolEffectSemantics::ReadOnly)
        {
            return Err(ToolError::new(
                "isolated workers may execute only read-only tools",
            ));
        }
        if tool.permission_request(invocation).is_some() {
            return Err(ToolError::new(
                "isolated workers cannot execute tools that require user permission",
            ));
        }
        Ok(tool)
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
        self.exposure_plan_with_intent(prompt, context_window, &ToolExposureIntent::default())
    }

    pub fn exposure_plan_with_intent(
        &self,
        prompt: &str,
        context_window: u64,
        intent: &ToolExposureIntent,
    ) -> ToolExposurePlan {
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
        let default_max_auto = if context_window < 32_000 { 10 } else { 24 };
        if intent.is_empty() && auto_count <= default_max_auto {
            return ToolExposurePlan {
                inline: candidates,
                deferred: Vec::new(),
            };
        }

        let max_auto = if intent.is_empty() {
            default_max_auto
        } else {
            default_max_auto.min(12)
        };
        let query = prompt.to_ascii_lowercase();
        candidates.sort_by(|left, right| {
            tool_relevance_with_intent(right, &query, intent)
                .cmp(&tool_relevance_with_intent(left, &query, intent))
                .then(left.name.cmp(&right.name))
        });
        let mut inline = Vec::new();
        let mut deferred = Vec::new();
        let mut auto_inline = 0usize;
        for spec in candidates {
            let required = intent.required_tools.contains(&spec.name);
            let explicitly_deferred = intent.deferred_namespaces.contains(&spec.namespace);
            match spec.exposure {
                agent_core::ToolExposure::Inline => inline.push(spec),
                _ if required => {
                    auto_inline += 1;
                    inline.push(spec);
                }
                _ if explicitly_deferred => deferred.push(spec),
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

/// Render a compact, cheap index of deferred tools so the model knows what is
/// available on demand without paying for every full schema up front. The model
/// loads a full schema with `tool.inspect` and calls it with `tool.invoke`.
pub fn render_deferred_tool_index(deferred: &[ToolSpec]) -> String {
    if deferred.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "Additional tools are available on demand (not loaded to save context). If the request matches one, call tool.inspect with its name to load the full schema, then tool.invoke to use it; do not ask the user which tool to use:\n",
    );
    for spec in deferred {
        out.push_str(&format!(
            "- {}: {}\n",
            spec.name,
            first_line(&spec.description)
        ));
    }
    out
}

fn first_line(value: &str) -> String {
    value
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(160)
        .collect()
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

fn tool_relevance_with_intent(spec: &ToolSpec, query: &str, intent: &ToolExposureIntent) -> usize {
    let mut score = tool_relevance(spec, query);
    if intent.required_tools.contains(&spec.name) {
        score += 10_000;
    }
    if intent.preferred_namespaces.contains(&spec.namespace) {
        score += 1_000;
    }
    if intent.prefer_read_only && spec.effect_semantics == ToolEffectSemantics::ReadOnly {
        score += 100;
    }
    if intent.prefer_effects
        && spec.effect_semantics != ToolEffectSemantics::ReadOnly
        && (spec.namespace != "process" || intent.preferred_namespaces.contains("process"))
    {
        score += 100;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn deferred_tool_index_is_compact_and_discoverable() {
        let deferred = vec![ToolSpec::builtin(
            "mermaid.render",
            "diagram",
            "Render a Mermaid diagram to an image.\nSecond line is dropped.",
            ToolRisk::ReadOnly,
            "{}",
        )];
        let index = render_deferred_tool_index(&deferred);
        assert!(index.contains("mermaid.render"));
        assert!(index.contains("Render a Mermaid diagram to an image."));
        assert!(!index.contains("Second line"));
        assert!(index.contains("tool.inspect"));
        assert!(render_deferred_tool_index(&[]).is_empty());
    }

    struct CatalogTool {
        name: String,
    }

    struct InvalidSchemaTool;

    struct InvalidConcurrencyTool;

    struct PermissionedReadTool;

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

    impl Tool for InvalidConcurrencyTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::builtin(
                "invalid.concurrency",
                "test",
                "Invalid concurrency fixture",
                ToolRisk::ReadOnly,
                r#"{"type":"object","properties":{}}"#,
            )
            .with_effect_semantics(ToolEffectSemantics::Idempotent)
            .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
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

    impl Tool for PermissionedReadTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::builtin(
                "test.permissioned_read",
                "test",
                "Read-only fixture that still requires user permission",
                ToolRisk::ReadOnly,
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
            )
        }

        fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
            Some(PermissionRequest {
                id: PermissionRequestId("permissioned-read".to_string()),
                task_id: invocation.task_id.clone(),
                risk: PermissionRisk::Sensitive,
                action: invocation.tool_name.clone(),
                reason: "Fixture requires explicit permission".to_string(),
                scope: "test".to_string(),
                metadata: Metadata::new(),
            })
        }

        fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Succeeded,
                "must not execute without permission",
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
        assert!(specs.iter().any(|spec| spec.name == "file.glob"));
        assert!(specs.iter().any(|spec| spec.name == "file.patch"));
        assert!(specs.iter().any(|spec| spec.name == "file.write"));
        assert!(specs.iter().any(|spec| spec.name == "shell.run"));
        assert!(specs.iter().any(|spec| spec.name == "web.search"));
        assert!(specs.iter().any(|spec| spec.name == "web.fetch"));
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
        assert!(specs
            .iter()
            .find(|spec| spec.name == "file.read")
            .is_some_and(|spec| spec
                .postcondition_verifiers
                .contains(&PostconditionVerifierKind::WorkspaceExactReadbackV1)));
        assert!(specs
            .iter()
            .find(|spec| spec.name == "shell.run")
            .is_some_and(|spec| spec
                .postcondition_verifiers
                .contains(&PostconditionVerifierKind::WorkspaceQualityCheckV1)));

        for (name, integer_field) in [
            ("file.read", "offset_bytes"),
            ("file.glob", "max_results"),
            ("file.search", "max_results"),
            ("shell.run", "timeout_seconds"),
            ("browser.extract_text", "timeout_ms"),
        ] {
            let spec = specs
                .iter()
                .find(|spec| spec.name == name)
                .expect("migrated tool spec should exist");
            let input_schema: serde_json::Value =
                serde_json::from_str(&spec.input_schema_json).expect("input schema should parse");
            assert_eq!(
                input_schema["properties"][integer_field]["type"],
                serde_json::Value::String("integer".to_string())
            );
            assert_eq!(
                input_schema["additionalProperties"],
                serde_json::Value::Bool(false)
            );
            assert!(spec.output_schema_json.is_some());
        }

        for name in ["file.read", "file.read_many", "file.search", "file.glob"] {
            assert_eq!(
                specs
                    .iter()
                    .find(|spec| spec.name == name)
                    .map(|spec| spec.execution_concurrency),
                Some(ToolExecutionConcurrency::IndependentRead)
            );
        }
        for name in [
            "file.list",
            "file.patch",
            "file.write",
            "shell.run",
            "web.search",
            "web.fetch",
            "browser.open",
        ] {
            assert_eq!(
                specs
                    .iter()
                    .find(|spec| spec.name == name)
                    .map(|spec| spec.execution_concurrency),
                Some(ToolExecutionConcurrency::Serialized)
            );
        }
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

        assert!(registry
            .try_register(Box::new(InvalidConcurrencyTool))
            .expect_err("unsafe concurrency declaration must be rejected")
            .contains("read-only risk and effect semantics"));
        assert!(registry.get("invalid.concurrency").is_none());
    }

    #[test]
    fn permissionless_read_gate_rejects_tools_that_still_require_user_approval() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(PermissionedReadTool));
        let invocation = invocation("test.permissioned_read", "{}".to_string());

        let error = registry
            .permissionless_read_tool(&invocation)
            .err()
            .expect("isolated execution must not bypass a permission request");

        assert!(error.message.contains("require user permission"));
    }

    #[test]
    fn permissionless_read_gate_accepts_a_genuinely_unprivileged_read() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CatalogTool {
            name: "catalog.read".to_string(),
        }));
        let invocation = invocation("catalog.read", "{}".to_string());

        assert!(registry.permissionless_read_tool(&invocation).is_ok());
    }

    #[test]
    fn permissionless_read_gate_accepts_file_glob_as_a_pure_read() {
        let registry = ToolRegistry::with_workspace_tools(temp_workspace());
        let invocation = invocation("file.glob", encode_input(&[("pattern", "src/**/*.rs")]));

        assert!(registry.permissionless_read_tool(&invocation).is_ok());
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
        for name in ["tool.search", "tool.inspect"] {
            assert_eq!(
                plan.inline
                    .iter()
                    .find(|spec| spec.name == name)
                    .map(|spec| spec.execution_concurrency),
                Some(ToolExecutionConcurrency::IndependentRead)
            );
        }
        assert_eq!(
            plan.inline
                .iter()
                .find(|spec| spec.name == "tool.invoke")
                .map(|spec| spec.execution_concurrency),
            Some(ToolExecutionConcurrency::Serialized)
        );
    }

    #[test]
    fn execution_intent_keeps_browser_tools_focused_and_defers_computer_controls() {
        let mut registry = ToolRegistry::with_workspace_tools(temp_workspace());
        registry.install_meta_tools();
        let intent = ToolExposureIntent {
            preferred_namespaces: BTreeSet::from(["browser".to_string()]),
            deferred_namespaces: BTreeSet::from(["computer".to_string()]),
            required_tools: BTreeSet::from([
                "browser.open".to_string(),
                "browser.extract_text".to_string(),
            ]),
            prefer_effects: true,
            ..ToolExposureIntent::default()
        };

        let plan = registry.exposure_plan_with_intent(
            "Open the incident dashboard in the browser and create a JSON report",
            128_000,
            &intent,
        );
        let inline = plan
            .inline
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<BTreeSet<_>>();

        assert!(inline.contains("browser.open"));
        assert!(inline.contains("browser.extract_text"));
        assert!(inline.contains("file.write"));
        assert!(!inline.iter().any(|name| name.starts_with("computer.")));
        assert!(inline.contains("tool.search"));
        assert!(plan.deferred.iter().any(|tool| tool.name == "computer.key"));
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
    fn meta_invoke_preserves_target_effect_semantics() {
        let root = temp_workspace();
        let mut registry = ToolRegistry::with_workspace_tools(root);
        registry.install_meta_tools();
        let meta = registry.get("tool.invoke").expect("meta tool registered");

        let read = invocation(
            "tool.invoke",
            serde_json::json!({
                "name": "file.read",
                "arguments": { "path": "note.txt" }
            })
            .to_string(),
        );
        assert_eq!(
            meta.effect_spec(&read).effect_semantics,
            ToolEffectSemantics::ReadOnly
        );
        assert_eq!(
            meta.effect_spec(&read).execution_concurrency,
            ToolExecutionConcurrency::IndependentRead
        );

        let write = invocation(
            "tool.invoke",
            serde_json::json!({
                "name": "file.write",
                "arguments": { "path": "note.txt", "content": "hello" }
            })
            .to_string(),
        );
        assert_eq!(
            meta.effect_spec(&write).effect_semantics,
            ToolEffectSemantics::Verifiable {
                verifier: "workspace_file_content_v1".to_string(),
            }
        );
        assert_eq!(
            meta.effect_spec(&write).execution_concurrency,
            ToolExecutionConcurrency::Serialized
        );
    }

    #[test]
    fn meta_invoke_delegates_trusted_postcondition_evidence_to_its_target() {
        let root = temp_workspace();
        fs::write(root.join("note.txt"), "verified").expect("fixture should be written");
        let mut registry = ToolRegistry::with_workspace_tools(root);
        registry.install_meta_tools();
        let meta = registry.get("tool.invoke").expect("meta tool registered");
        let request = invocation(
            "tool.invoke",
            serde_json::json!({
                "name": "file.read",
                "arguments": { "path": "note.txt" }
            })
            .to_string(),
        );
        let result = meta
            .execute(request.clone())
            .expect("delegated read should succeed");
        let evidence = meta
            .postcondition_evidence(&request, &result)
            .expect("meta tool should delegate evidence to the real target");

        assert_eq!(
            evidence.kind,
            PostconditionVerifierKind::WorkspaceExactReadbackV1
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&evidence.target_input_json)
                .expect("evidence target should be JSON"),
            serde_json::json!({ "path": "note.txt" })
        );
    }

    #[test]
    fn meta_invoke_delegates_cooperative_cancellation_to_its_target() {
        let root = temp_workspace();
        fs::write(root.join("note.txt"), "needle\n").expect("fixture should be written");
        let mut registry = ToolRegistry::with_workspace_tools(root);
        registry.install_meta_tools();
        let meta = registry.get("tool.invoke").expect("meta tool registered");
        let checks = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&checks);
        let control =
            ToolExecutionControl::new(move || observed.fetch_add(1, Ordering::SeqCst) >= 2);
        let request = invocation(
            "tool.invoke",
            serde_json::json!({
                "name": "file.search",
                "arguments": { "path": ".", "query": "needle" }
            })
            .to_string(),
        );

        let result = meta
            .execute_with_control(request, &control)
            .expect("cancelled target should return a structured result");

        assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
        assert!(checks.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn file_write_overwrite_captures_undo_before_state() {
        let root = temp_workspace();
        fs::write(root.join("note.txt"), "original content").expect("fixture should be written");
        let writer = WriteFileTool::new(root.clone());

        let result = writer
            .execute(invocation(
                "file.write",
                encode_input(&[("path", "note.txt"), ("content", "replacement content")]),
            ))
            .expect("write should succeed");

        assert_eq!(
            result.metadata.get("undo_action").map(String::as_str),
            Some("overwritten")
        );
        let undo_relative = result
            .metadata
            .get("undo_before_path")
            .expect("undo snapshot path recorded");
        assert_eq!(
            fs::read(root.join(undo_relative)).expect("undo snapshot exists"),
            b"original content"
        );
        assert_eq!(
            result
                .metadata
                .get("undo_before_sha256")
                .map(|value| value.len()),
            Some(64)
        );
        assert_eq!(
            fs::read(root.join(result.metadata.get("artifact_path").unwrap()))
                .expect("output history snapshot exists"),
            b"replacement content"
        );
    }

    #[test]
    fn file_write_creation_marks_created_without_undo_snapshot() {
        let root = temp_workspace();
        let writer = WriteFileTool::new(root.clone());

        let result = writer
            .execute(invocation(
                "file.write",
                encode_input(&[("path", "fresh.txt"), ("content", "new file")]),
            ))
            .expect("write should succeed");

        assert_eq!(
            result.metadata.get("undo_action").map(String::as_str),
            Some("created")
        );
        assert!(!result.metadata.contains_key("undo_before_path"));
        assert!(!result.metadata.contains_key("undo_before_sha256"));
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
    fn only_complete_file_readback_produces_exact_postcondition_evidence() {
        let root = temp_workspace();
        fs::write(root.join("note.txt"), "complete evidence").expect("fixture should be written");
        let reader = ReadFileTool::new(root);
        let complete_request = invocation(
            "file.read",
            serde_json::json!({ "path": "note.txt" }).to_string(),
        );
        let complete_result = reader
            .execute(complete_request.clone())
            .expect("complete read should succeed");
        let evidence = reader
            .postcondition_evidence(&complete_request, &complete_result)
            .expect("complete exact readback should produce evidence");
        assert_eq!(
            evidence.kind,
            PostconditionVerifierKind::WorkspaceExactReadbackV1
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&evidence.target_input_json)
                .expect("evidence target should be JSON"),
            serde_json::json!({ "path": "note.txt" })
        );

        let partial_request = invocation(
            "file.read",
            serde_json::json!({ "path": "note.txt", "max_bytes": 4 }).to_string(),
        );
        let partial_result = reader
            .execute(partial_request.clone())
            .expect("partial read should succeed");
        assert!(reader
            .postcondition_evidence(&partial_request, &partial_result)
            .is_none());
    }

    #[test]
    fn list_and_search_do_not_claim_exact_readback_evidence() {
        let root = temp_workspace();
        fs::write(root.join("note.txt"), "needle").expect("fixture should be written");
        let lister = ListDirectoryTool::new(root.clone());
        let list_request = invocation("file.list", serde_json::json!({ "path": "." }).to_string());
        let list_result = lister
            .execute(list_request.clone())
            .expect("list should succeed");
        assert!(lister
            .postcondition_evidence(&list_request, &list_result)
            .is_none());

        let searcher = SearchFilesTool::new(root.clone());
        let search_request = invocation(
            "file.search",
            serde_json::json!({ "path": ".", "query": "needle" }).to_string(),
        );
        let search_result = searcher
            .execute(search_request.clone())
            .expect("search should succeed");
        assert!(searcher
            .postcondition_evidence(&search_request, &search_result)
            .is_none());
    }

    #[test]
    fn migrated_output_schema_is_discoverable_through_tool_inspect() {
        let catalog = ToolRegistry::with_workspace_tools(temp_workspace());
        let inspect = ToolInspectMeta {
            catalog: catalog.clone(),
        };
        let result = inspect
            .execute(invocation(
                "tool.inspect",
                serde_json::json!({ "name": "file.read" }).to_string(),
            ))
            .expect("tool inspection should succeed");
        let output: serde_json::Value =
            serde_json::from_str(&result.output).expect("inspection should return JSON");

        assert_eq!(
            output["outputSchema"]["properties"]["schema"]["const"],
            "cindx.file-read-result.v2"
        );
    }

    #[test]
    fn file_search_cooperatively_stops_during_a_scan() {
        let root = temp_workspace();
        fs::create_dir_all(root.join("notes")).expect("fixture directory should be created");
        fs::write(root.join("notes/one.txt"), "needle\n").expect("fixture should be written");
        let searcher = SearchFilesTool::new(root);
        let checks = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&checks);
        let control =
            ToolExecutionControl::new(move || observed.fetch_add(1, Ordering::SeqCst) >= 2);

        let result = searcher
            .execute_with_control(
                invocation(
                    "file.search",
                    encode_input(&[("path", "."), ("query", "needle")]),
                ),
                &control,
            )
            .expect("cancelled search should return a structured result");

        assert_eq!(result.status, ToolOutcomeStatus::Cancelled);
        assert!(checks.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn file_search_samples_cancellation_outside_the_per_line_hot_loop() {
        let root = temp_workspace();
        let content = (0..300)
            .map(|index| format!("ordinary line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(root.join("many-lines.txt"), content).expect("fixture should be written");
        let searcher = SearchFilesTool::new(root);
        let checks = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&checks);
        let control = ToolExecutionControl::new(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            false
        });

        let result = searcher
            .execute_with_control(
                invocation(
                    "file.search",
                    encode_input(&[("path", "."), ("query", "absent query")]),
                ),
                &control,
            )
            .expect("search should succeed");

        assert_eq!(result.status, ToolOutcomeStatus::Succeeded);
        assert!(checks.load(Ordering::SeqCst) <= 12);
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
    fn file_read_exposes_a_complete_base_hash_without_changing_raw_output() {
        let root = temp_workspace();
        fs::write(root.join("base.txt"), "complete base").expect("hash fixture should be written");
        let result = ReadFileTool::new(root)
            .execute(invocation(
                "file.read",
                encode_input(&[("path", "base.txt")]),
            ))
            .expect("complete read should succeed");
        let structured: serde_json::Value = serde_json::from_str(
            result
                .structured_output_json
                .as_deref()
                .expect("file.read should expose structured output"),
        )
        .expect("file.read output should be valid JSON");
        let expected = workspace_file::sha256_bytes(b"complete base");

        assert_eq!(result.output, "complete base");
        assert_eq!(structured["page_sha256"], expected);
        assert_eq!(structured["sha256"], expected);
        assert_eq!(result.metadata.get("sha256"), Some(&expected));
    }

    #[test]
    fn file_read_hashes_the_whole_bounded_file_only_when_requested() {
        let root = temp_workspace();
        fs::write(root.join("base.txt"), "complete base").expect("hash fixture should be written");
        let reader = ReadFileTool::new(root);
        let default_page = reader
            .execute(invocation(
                "file.read",
                encode_input(&[("path", "base.txt"), ("max_bytes", "4")]),
            ))
            .expect("default partial read should succeed");
        let requested_page = reader
            .execute(invocation(
                "file.read",
                encode_input(&[
                    ("path", "base.txt"),
                    ("max_bytes", "4"),
                    ("include_sha256", "true"),
                ]),
            ))
            .expect("hashed partial read should succeed");
        let default_structured: serde_json::Value =
            serde_json::from_str(default_page.structured_output_json.as_deref().unwrap()).unwrap();
        let requested_structured: serde_json::Value =
            serde_json::from_str(requested_page.structured_output_json.as_deref().unwrap())
                .unwrap();

        assert!(default_structured["sha256"].is_null());
        assert_eq!(
            requested_structured["sha256"],
            workspace_file::sha256_bytes(b"complete base")
        );
        assert_eq!(default_page.output, requested_page.output);
    }

    #[test]
    fn file_read_continuation_tracks_model_visible_bytes_without_skipping() {
        let root = temp_workspace();
        fs::write(root.join("long.txt"), "a".repeat(20_000))
            .expect("long fixture should be written");
        let result = ReadFileTool::new(root)
            .execute(invocation(
                "file.read",
                encode_input(&[("path", "long.txt")]),
            ))
            .expect("long read should succeed");
        let observation = result
            .model_observation
            .expect("file.read should expose a typed model observation");

        assert_eq!(result.output.len(), 20_000);
        assert_eq!(
            result.metadata.get("next_offset_bytes").map(String::as_str),
            Some("20000")
        );
        assert_eq!(
            observation
                .facts
                .get("next_offset_bytes")
                .map(String::as_str),
            Some("5120")
        );
        assert!(!observation.evidence_complete);
        assert!(observation
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("offset_bytes=5120")));
    }

    #[test]
    fn file_search_marks_limited_results_incomplete() {
        let root = temp_workspace();
        fs::write(root.join("matches.txt"), "needle one\nneedle two\n")
            .expect("search fixture should be written");
        let result = SearchFilesTool::new(root)
            .execute(invocation(
                "file.search",
                encode_input(&[("path", "."), ("query", "needle"), ("max_results", "1")]),
            ))
            .expect("limited search should succeed");
        let observation = result
            .model_observation
            .expect("file.search should expose a typed model observation");

        assert_eq!(
            result
                .metadata
                .get("result_limit_reached")
                .map(String::as_str),
            Some("true")
        );
        assert!(!observation.evidence_complete);
        assert!(observation
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("Narrow")));
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
        let observation = result
            .model_observation
            .as_ref()
            .expect("shell should expose a typed model observation");
        assert_eq!(
            observation
                .facts
                .get("output_truncated")
                .map(String::as_str),
            Some("true")
        );
        let model_artifact = observation
            .facts
            .get("stdout_artifact")
            .expect("model observation should reference complete stdout");
        assert!(!model_artifact.starts_with(root.to_string_lossy().as_ref()));
        assert!(observation
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("file.read")));
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
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.code.as_str()),
            Some("shell_timeout")
        );
        assert!(
            !result
                .model_observation
                .as_ref()
                .expect("timeout should expose a typed observation")
                .evidence_complete
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn shell_nonzero_exit_exposes_a_stable_failure_code() {
        let result = ShellRunTool::new(temp_workspace())
            .execute(invocation(
                "shell.run",
                encode_input(&[("command", "printf failure >&2; exit 7")]),
            ))
            .expect("nonzero exit should return a tool result");

        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.code.as_str()),
            Some("shell_exit_nonzero")
        );
        assert_eq!(
            result
                .model_observation
                .as_ref()
                .and_then(|observation| observation.facts.get("exit_code"))
                .map(String::as_str),
            Some("7")
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_signal_exposes_a_distinct_incomplete_failure() {
        let result = ShellRunTool::new(temp_workspace())
            .execute(invocation(
                "shell.run",
                encode_input(&[("command", "kill -TERM $$")]),
            ))
            .expect("signal termination should return a tool result");

        assert_eq!(result.status, ToolOutcomeStatus::Failed);
        assert_eq!(
            result.failure.as_ref().map(|failure| failure.code.as_str()),
            Some("shell_signal")
        );
        let observation = result
            .model_observation
            .as_ref()
            .expect("signal termination should expose a typed observation");
        assert!(!observation.evidence_complete);
        assert_eq!(
            observation.facts.get("termination").map(String::as_str),
            Some("signal")
        );
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
        let observation = result
            .model_observation
            .as_ref()
            .expect("cancelled shell should expose a typed observation");
        assert!(!observation.evidence_complete);
        assert!(observation
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("partial")));
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
    fn browser_extract_rejects_ambiguous_targets_before_sidecar_execution() {
        let tool = BrowserTool::extract_text(temp_workspace());
        let error = tool
            .execute(invocation(
                "browser.extract_text",
                encode_input(&[("selector", "main"), ("text_target", "Settings")]),
            ))
            .expect_err("ambiguous targets should be rejected locally");

        assert!(error.message.contains("at most one target strategy"));
    }

    #[test]
    fn browser_extract_preserves_page_identity_and_continuation_guidance() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let sidecar = root.join("browser-extract-test.sh");
        let trace = root.join("browser-trace.json");
        let text = root.join("browser-text.txt");
        fs::write(&trace, "{}\n").expect("trace should write");
        fs::write(&text, "complete retained browser text\n").expect("text artifact should write");
        let output = format!(
            "BODY_HEAD{}BODY_TAIL\n\nAccessibility snapshot:\nARIA_TAIL",
            "x".repeat(8_000)
        );
        fs::write(
            &sidecar,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                serde_json::json!({
                    "schema": BROWSER_CONTROL_RESPONSE_SCHEMA,
                    "ok": true,
                    "id": "browser-extract-test",
                    "action": "extract_text",
                    "session_id": "task-1",
                    "controller": "cdp_playwright",
                    "page": {
                        "id": "tab-1",
                        "url": "https://example.com/docs",
                        "title": "Example docs"
                    },
                    "output": output,
                    "artifacts": [{
                        "path": text.display().to_string(),
                        "mime_type": "text/plain",
                        "title": "Browser text"
                    }],
                    "trace_path": trace.display().to_string(),
                    "duration_ms": 12
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
        let result = BrowserTool::extract_text(root)
            .execute(invocation("browser.extract_text", "{}".to_string()))
            .expect("browser extract sidecar should execute");
        env::remove_var("CINDX_BROWSER_SIDECAR");
        let observation = result
            .model_observation
            .expect("browser extract should expose a typed observation");

        assert_eq!(
            observation.facts.get("url").map(String::as_str),
            Some("https://example.com/docs")
        );
        assert_eq!(
            observation.facts.get("tab_id").map(String::as_str),
            Some("tab-1")
        );
        assert!(observation.evidence.contains("BODY_HEAD"));
        assert!(observation.evidence.contains("ARIA_TAIL"));
        assert!(!observation.evidence_complete);
        assert!(observation
            .next_action
            .as_deref()
            .is_some_and(|action| action.contains("file.read")));
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
    fn browser_session_retirement_is_path_scoped_and_contract_checked() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        let root = temp_workspace();
        let session_relative = PathBuf::from(".cindx/browser-sessions/session-retire");
        let session_dir = root.join(&session_relative);
        fs::create_dir_all(session_dir.join("profile")).expect("session profile should exist");
        let sidecar = root.join("browser-retirement-test.sh");
        let trace = root.join("browser-retirement-args.txt");
        fs::write(
            &sidecar,
            format!(
                "#!/bin/sh\nprintf '%s\\n%s\\n' \"$1\" \"$2\" > '{}'\nprintf '%s\\n' '{}'\n",
                trace.display(),
                serde_json::json!({
                    "schema": "cindx.browser-session-retirement.v1",
                    "retired": true,
                    "session_dir": session_dir.display().to_string(),
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

        retire_browser_session(&root, &session_relative).expect("retirement should succeed");

        let arguments = fs::read_to_string(&trace).expect("sidecar arguments should be recorded");
        assert_eq!(
            arguments.lines().collect::<Vec<_>>(),
            [
                "--retire-session",
                session_dir.to_str().expect("path should encode")
            ]
        );
        let resumed_relative = PathBuf::from(".cindx/browser-sessions/session-resume");
        fs::create_dir_all(root.join(".cindx/browser-sessions/.retired/session-resume"))
            .expect("interrupted quarantine should exist");
        retire_browser_session(&root, &resumed_relative)
            .expect("interrupted retirement should invoke the sidecar");
        let resumed_arguments =
            fs::read_to_string(&trace).expect("resumed sidecar arguments should be recorded");
        assert_eq!(
            resumed_arguments.lines().collect::<Vec<_>>(),
            [
                "--retire-session",
                root.join(&resumed_relative)
                    .to_str()
                    .expect("path should encode")
            ]
        );
        let error = retire_browser_session(
            &root,
            Path::new(".cindx/browser-sessions/session-retire/nested"),
        )
        .expect_err("nested retirement must be rejected");
        assert!(error.message.contains("direct"));
        let reserved = retire_browser_session(&root, Path::new(".cindx/browser-sessions/.retired"))
            .expect_err("retirement quarantine must be reserved");
        assert!(reserved.message.contains("direct"));
        env::remove_var("CINDX_BROWSER_SIDECAR");
    }

    #[test]
    fn absent_browser_session_retirement_is_idempotent_without_a_sidecar() {
        let _guard = ENV_LOCK.lock().expect("env lock should be available");
        env::remove_var("CINDX_BROWSER_SIDECAR");
        let root = temp_workspace();

        retire_browser_session(&root, Path::new(".cindx/browser-sessions/already-absent"))
            .expect("absent session retirement should be idempotent");
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
    fn browser_close_request_targets_the_reusable_session() {
        let root = temp_workspace();
        let request = browser_request_json(
            "browser-close",
            BrowserToolKind::Close,
            "session-alpha",
            &root.join("session"),
            &root.join("artifacts"),
            &BTreeMap::new(),
        )
        .expect("browser close request should encode");
        let request: serde_json::Value =
            serde_json::from_str(&request).expect("browser close request should be JSON");

        assert_eq!(request["action"], "close");
        assert_eq!(request["session_id"], "session-alpha");
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
