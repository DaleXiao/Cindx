use super::*;
use crate::process_control::terminate_process_group;
use crate::stream_capture::capture_stream_limited;
use agent_core::{ToolArtifact, ToolEffectSemantics};
use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const BROWSER_CONTROL_REQUEST_SCHEMA: &str = "cindx.browser-control.v2";
pub(crate) const BROWSER_CONTROL_RESPONSE_SCHEMA: &str = "cindx.browser-control-result.v2";
pub(crate) const COMPUTER_CONTROL_REQUEST_SCHEMA: &str = "cindx.computer-control.v1";
pub(crate) const COMPUTER_CONTROL_RESPONSE_SCHEMA: &str = "cindx.computer-control-result.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserToolKind {
    Open,
    ExtractText,
    Capture,
    Click,
    TypeText,
    Scroll,
    Tabs,
    SelectTab,
    Close,
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
            Self::Close => "browser.close",
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
            Self::Close => "close",
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
            Self::Close => "Close the reusable browser session and release its browser process.",
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
            Self::Close => "session_id=<optional browser session>",
        }
    }

    fn risk(self) -> ToolRisk {
        match self {
            Self::TypeText => ToolRisk::SensitiveContext,
            Self::Close => ToolRisk::Destructive,
            _ => ToolRisk::UsesNetwork,
        }
    }

    fn effect_semantics(self) -> ToolEffectSemantics {
        match self {
            Self::ExtractText | Self::Capture | Self::Tabs => ToolEffectSemantics::ReadOnly,
            Self::SelectTab | Self::Close => ToolEffectSemantics::Idempotent,
            Self::Open | Self::Click | Self::TypeText | Self::Scroll => {
                ToolEffectSemantics::NonIdempotent
            }
        }
    }

    fn permission_risk(self) -> PermissionRisk {
        match self {
            Self::TypeText => PermissionRisk::Sensitive,
            Self::Close => PermissionRisk::Destructive,
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
            Self::Close => "Close this reusable browser session and discard open tabs.",
        }
    }
}

pub struct BrowserTool {
    workspace_root: PathBuf,
    kind: BrowserToolKind,
}

impl BrowserTool {
    pub(crate) fn new(workspace_root: impl Into<PathBuf>, kind: BrowserToolKind) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind,
        }
    }

    pub(crate) fn open(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Open)
    }

    pub(crate) fn extract_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::ExtractText)
    }

    pub(crate) fn capture(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Capture)
    }

    pub(crate) fn click(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Click)
    }

    pub(crate) fn type_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::TypeText)
    }

    pub(crate) fn scroll(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Scroll)
    }

    pub(crate) fn tabs(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Tabs)
    }

    pub(crate) fn select_tab(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::SelectTab)
    }

    pub(crate) fn close(workspace_root: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, BrowserToolKind::Close)
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
            ToolError::new(format!(
                "failed to create browser artifact directory: {error}"
            ))
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
            ToolError::new(format!(
                "failed to create browser session directory: {error}"
            ))
        })?;

        let action_id = format!(
            "browser-{}-{}",
            current_time_millis(),
            stable_hash(&format!(
                "{}:{}:{}",
                self.kind.action(),
                invocation.id.0,
                invocation.input_json
            ))
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
            return Err(ToolError::retryable(
                "tool_timeout",
                format!(
                    "browser sidecar exceeded {} ms",
                    timeout_ms.saturating_add(10_000)
                ),
            ));
        }

        let response: serde_json::Value =
            serde_json::from_str(&sidecar.stdout).map_err(|error| {
                ToolError::new(format!("invalid browser sidecar response: {error}"))
            })?;
        if response.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_CONTROL_RESPONSE_SCHEMA)
        {
            return Err(ToolError::new(
                "browser sidecar returned an unsupported schema",
            ));
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
        if let Some(duration) = response
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64)
        {
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
        .with_effect_semantics(self.kind.effect_semantics())
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
pub(crate) enum ComputerActionKind {
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
            Self::Screenshot => {
                "Capture a local desktop screenshot artifact with redaction metadata."
            }
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
    pub(crate) fn screenshot(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Screenshot,
        }
    }

    pub(crate) fn click(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Click,
        }
    }

    pub(crate) fn type_text(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::TypeText,
        }
    }

    pub(crate) fn key(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Key,
        }
    }

    pub(crate) fn scroll(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            kind: ComputerActionKind::Scroll,
        }
    }

    fn execute_inner(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        if control.should_cancel() {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Computer action cancelled before it started.",
                Metadata::new(),
            ));
        }
        let input = parse_input(&invocation.input_json);
        validate_computer_action_input(self.kind, &input)?;
        let output_dir = input
            .get("output_dir")
            .cloned()
            .unwrap_or_else(|| ".cindx/computer-actions".to_string());
        let resolved_dir = resolve_workspace_path(&self.workspace_root, &output_dir)?;
        fs::create_dir_all(&resolved_dir).map_err(|error| {
            ToolError::new(format!(
                "failed to create computer action directory: {error}"
            ))
        })?;

        let action_id = format!(
            "computer-{}-{}",
            current_time_millis(),
            stable_hash(&format!(
                "{}:{}:{}",
                self.kind.action(),
                invocation.id.0,
                invocation.input_json
            ))
        );
        let request_path = resolved_dir.join(format!(".{action_id}-request.json"));
        let request_json = computer_action_request_json(&action_id, self.kind, &input);
        write_private_file(&request_path, request_json.as_bytes())?;

        let sidecar_configured = env::var("CINDX_COMPUTER_SIDECAR")
            .ok()
            .is_some_and(|path| !path.trim().is_empty());
        if !sidecar_configured {
            let result = if self.kind == ComputerActionKind::Screenshot {
                execute_native_computer_screenshot(
                    invocation.id,
                    &self.workspace_root,
                    &output_dir,
                    &action_id,
                    &input,
                )
            } else {
                Err(ToolError::new(
                    "computer sidecar is not configured; the desktop action was not executed",
                ))
            };
            let _ = fs::remove_file(&request_path);
            return result;
        }

        let sidecar = run_json_sidecar_controlled(
            "CINDX_COMPUTER_SIDECAR",
            &request_path,
            control,
            Duration::from_secs(20),
        );
        let _ = fs::remove_file(&request_path);
        let sidecar = sidecar?;
        if sidecar.cancelled {
            return Ok(ToolResult::text(
                invocation.id,
                ToolOutcomeStatus::Cancelled,
                "Computer action cancelled.",
                [("action".to_string(), self.kind.action().to_string())]
                    .into_iter()
                    .collect(),
            ));
        }
        if sidecar.timed_out {
            return Err(ToolError::retryable(
                "tool_timeout",
                "computer sidecar exceeded 20000 ms",
            ));
        }

        let response: serde_json::Value =
            serde_json::from_str(&sidecar.stdout).map_err(|error| {
                ToolError::new(format!("invalid computer sidecar response: {error}"))
            })?;
        if response.get("schema").and_then(serde_json::Value::as_str)
            != Some(COMPUTER_CONTROL_RESPONSE_SCHEMA)
        {
            return Err(ToolError::new(
                "computer sidecar returned an unsupported schema",
            ));
        }
        let output = response
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("computer action completed")
            .to_string();
        if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(ToolError::new(output));
        }

        let artifacts = response
            .get("artifacts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|artifact| {
                let path = artifact.get("path")?.as_str()?;
                let relative = workspace_relative_path(&self.workspace_root, path)?;
                Some(ToolArtifact {
                    path: relative,
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
        if self.kind == ComputerActionKind::Screenshot {
            let screenshot = artifacts
                .iter()
                .find(|artifact| artifact.mime_type.as_deref() == Some("image/png"))
                .ok_or_else(|| {
                    ToolError::new("computer screenshot completed without a PNG artifact")
                })?;
            let screenshot_path = resolve_workspace_path(&self.workspace_root, &screenshot.path)?;
            if !screenshot_path.is_file()
                || screenshot_path
                    .metadata()
                    .map(|metadata| metadata.len() == 0)
                    .unwrap_or(true)
            {
                return Err(ToolError::new(
                    "computer screenshot completed without readable pixels",
                ));
            }
        }

        let mut metadata = Metadata::new();
        metadata.insert("action".to_string(), self.kind.action().to_string());
        metadata.insert("controller".to_string(), "native_macos".to_string());
        metadata.insert(
            "destructive".to_string(),
            input_is_true(&input, "destructive").to_string(),
        );
        if let Some(duration) = response
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64)
        {
            metadata.insert("duration_ms".to_string(), duration.to_string());
        }
        if let Some(artifact) = artifacts.first() {
            metadata.insert("artifact_path".to_string(), artifact.path.clone());
        }
        if self.kind == ComputerActionKind::Screenshot {
            let manifest = write_computer_redaction_manifest(
                &self.workspace_root,
                &output_dir,
                &action_id,
                artifacts
                    .first()
                    .map(|artifact| artifact.path.as_str())
                    .unwrap_or_default(),
                &input,
                "captured",
            )?;
            metadata.insert("redaction_manifest_path".to_string(), manifest);
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

impl Tool for ComputerTool {
    fn spec(&self) -> ToolSpec {
        let semantics = if self.kind == ComputerActionKind::Screenshot {
            ToolEffectSemantics::ReadOnly
        } else {
            ToolEffectSemantics::NonIdempotent
        };
        builtin_tool_spec(
            self.kind.tool_name(),
            self.kind.description(),
            self.kind.risk(),
            self.kind.schema(),
        )
        .with_effect_semantics(semantics)
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
            Err(ToolError::new(
                "browser.type requires a semantic target or selector",
            ))
        }
        BrowserToolKind::SelectTab => required_input(input, "tab_id").map(|_| ()),
        BrowserToolKind::ExtractText
        | BrowserToolKind::Capture
        | BrowserToolKind::Scroll
        | BrowserToolKind::Tabs
        | BrowserToolKind::Close => Ok(()),
    }
}

fn has_browser_target(input: &BTreeMap<String, String>) -> bool {
    ["selector", "role", "label", "placeholder", "text_target"]
        .iter()
        .any(|key| {
            input
                .get(*key)
                .is_some_and(|value| !value.trim().is_empty())
        })
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

pub(crate) fn browser_request_json(
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
        (
            "id".to_string(),
            serde_json::Value::String(action_id.to_string()),
        ),
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
        json_field("schema", COMPUTER_CONTROL_REQUEST_SCHEMA),
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

fn write_computer_redaction_manifest(
    workspace_root: &Path,
    output_dir: &str,
    action_id: &str,
    artifact_relative: &str,
    input: &BTreeMap<String, String>,
    status: &str,
) -> Result<String, ToolError> {
    let manifest_relative = format!("{output_dir}/{action_id}.redaction.json");
    let manifest_path = resolve_workspace_path(workspace_root, &manifest_relative)?;
    let redaction = input
        .get("redaction")
        .cloned()
        .unwrap_or_else(|| "manual".to_string());
    let manifest = format!(
        "{{{},{},{}}}\n",
        json_field("artifact_path", artifact_relative),
        json_field("redaction", &redaction),
        json_field("status", status)
    );
    write_private_file(&manifest_path, manifest.as_bytes())?;
    Ok(manifest_relative)
}

fn execute_native_computer_screenshot(
    invocation_id: ToolCallId,
    workspace_root: &Path,
    output_dir: &str,
    action_id: &str,
    input: &BTreeMap<String, String>,
) -> Result<ToolResult, ToolError> {
    let screenshot_relative = format!("{output_dir}/{action_id}.png");
    let screenshot_path = resolve_workspace_path(workspace_root, &screenshot_relative)?;
    let redaction = input
        .get("redaction")
        .cloned()
        .unwrap_or_else(|| "manual".to_string());
    if !try_native_screenshot(&screenshot_path)?
        || !screenshot_path.is_file()
        || screenshot_path
            .metadata()
            .map(|metadata| metadata.len() == 0)
            .unwrap_or(true)
    {
        return Err(ToolError::new(
            "desktop screenshot failed; grant Screen Recording permission or configure the computer sidecar",
        ));
    }
    let redaction_manifest_relative = write_computer_redaction_manifest(
        workspace_root,
        output_dir,
        action_id,
        &screenshot_relative,
        input,
        "captured",
    )?;

    let mut metadata = Metadata::new();
    metadata.insert("action".to_string(), "screenshot".to_string());
    metadata.insert("artifact_path".to_string(), screenshot_relative.clone());
    metadata.insert("screenshot_path".to_string(), screenshot_relative.clone());
    metadata.insert(
        "redaction_manifest_path".to_string(),
        redaction_manifest_relative,
    );
    metadata.insert("redaction".to_string(), redaction);
    metadata.insert("controller".to_string(), "native_macos".to_string());

    let mut result = tool_result(
        invocation_id,
        ToolOutcomeStatus::Succeeded,
        format!("desktop screenshot captured\nscreenshot={screenshot_relative}"),
        metadata,
    );
    result.artifacts.push(ToolArtifact {
        path: screenshot_relative,
        mime_type: Some("image/png".to_string()),
        title: Some("Desktop screenshot".to_string()),
    });
    Ok(result)
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
        Err(error) => Err(ToolError::new(format!(
            "failed to run screencapture: {error}"
        ))),
    }
}

pub(crate) struct ControlledSidecarOutput {
    pub(crate) stdout: String,
    pub(crate) cancelled: bool,
    pub(crate) timed_out: bool,
}

const SIDECAR_STDOUT_MAX_BYTES: usize = 16 * 1024 * 1024;
const SIDECAR_STDERR_MAX_BYTES: usize = 256 * 1024;

fn run_json_sidecar_controlled(
    env_key: &str,
    request_path: &Path,
    control: &ToolExecutionControl,
    hard_timeout: Duration,
) -> Result<ControlledSidecarOutput, ToolError> {
    run_sidecar_controlled(
        env_key,
        &[request_path.as_os_str().to_os_string()],
        control,
        hard_timeout,
    )
}

pub(crate) fn run_sidecar_controlled(
    env_key: &str,
    arguments: &[std::ffi::OsString],
    control: &ToolExecutionControl,
    hard_timeout: Duration,
) -> Result<ControlledSidecarOutput, ToolError> {
    let sidecar = env::var(env_key)
        .ok()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| ToolError::new(format!("{env_key} is not configured")))?;
    let sidecar_path = PathBuf::from(&sidecar);
    if !sidecar_path.is_file() {
        return Err(ToolError::new(format!("sidecar does not exist: {sidecar}")));
    }
    let mut command = if sidecar_path
        .extension()
        .and_then(|extension| extension.to_str())
        == Some("js")
    {
        let mut command =
            Command::new(env::var("CINDX_NODE").unwrap_or_else(|_| "node".to_string()));
        command.arg(&sidecar_path);
        command
    } else {
        Command::new(&sidecar_path)
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ToolError::new(format!("failed to start sidecar: {error}")))?;
    let process_id = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::new("sidecar stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::new("sidecar stderr is unavailable"))?;
    let stdout_reader =
        thread::spawn(move || capture_stream_limited(stdout, SIDECAR_STDOUT_MAX_BYTES));
    let stderr_reader =
        thread::spawn(move || capture_stream_limited(stderr, SIDECAR_STDERR_MAX_BYTES));
    let started = Instant::now();
    let (status, cancelled, timed_out) = loop {
        if control.should_cancel() {
            terminate_process_group(process_id, 15);
            thread::sleep(Duration::from_millis(40));
            terminate_process_group(process_id, 9);
            let _ = child.kill();
            let status = child
                .wait()
                .map_err(|error| ToolError::new(format!("failed to stop sidecar: {error}")))?;
            break (status, true, false);
        }
        if started.elapsed() >= hard_timeout {
            terminate_process_group(process_id, 15);
            thread::sleep(Duration::from_millis(40));
            terminate_process_group(process_id, 9);
            let _ = child.kill();
            let status = child.wait().map_err(|error| {
                ToolError::new(format!("failed to stop timed out sidecar: {error}"))
            })?;
            break (status, false, true);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| ToolError::new(format!("failed to poll sidecar: {error}")))?
        {
            break (status, false, false);
        }
        thread::sleep(Duration::from_millis(40));
    };
    if !cancelled && !timed_out {
        // A sidecar that exits can still leave descendants holding the pipes open.
        terminate_process_group(process_id, 9);
    }
    let stdout = stdout_reader
        .join()
        .map_err(|_| ToolError::new("sidecar stdout reader panicked"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| ToolError::new("sidecar stderr reader panicked"))?;
    if let Some(error) = stdout.error {
        return Err(ToolError::new(format!(
            "failed to read sidecar stdout: {error}"
        )));
    }
    if stdout.truncated {
        return Err(ToolError::new(format!(
            "sidecar response exceeded the {} byte safety limit ({} bytes produced)",
            SIDECAR_STDOUT_MAX_BYTES, stdout.total_bytes
        )));
    }
    if !cancelled && !timed_out && !status.success() {
        let mut message = String::from_utf8_lossy(&stderr.bytes).trim().to_string();
        if stderr.truncated {
            message.push_str(" [stderr truncated]");
        }
        return Err(ToolError::new(format!("sidecar failed: {message}")));
    }
    Ok(ControlledSidecarOutput {
        stdout: String::from_utf8_lossy(&stdout.bytes).trim().to_string(),
        cancelled,
        timed_out,
    })
}
