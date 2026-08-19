use agent_core::{
    Metadata, PermissionRequest, PermissionRequestId, PermissionRisk, ToolContent, ToolExposure,
    ToolFailure, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSource, ToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tools::{write_private_file_atomically, Tool, ToolError, ToolExecutionControl};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MCP_HTTP_STDOUT_MAX_BYTES: usize = 16 * 1024 * 1024;
const MCP_HTTP_STDERR_MAX_BYTES: usize = 256 * 1024;
const MCP_STDIO_RESPONSE_MAX_BYTES: usize = 16 * 1024 * 1024;
const MCP_TOOL_PREVIEW_MAX_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct LimitedRead {
    bytes: Vec<u8>,
    total_bytes: u64,
    truncated: bool,
    error: Option<String>,
}

fn read_limited_and_drain(mut stream: impl Read, max_bytes: usize) -> LimitedRead {
    let mut result = LimitedRead {
        bytes: Vec::with_capacity(max_bytes.min(64 * 1024)),
        ..LimitedRead::default()
    };
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                result.error = Some(error.to_string());
                break;
            }
        };
        result.total_bytes = result.total_bytes.saturating_add(count as u64);
        let remaining = max_bytes.saturating_sub(result.bytes.len());
        let retained = remaining.min(count);
        result.bytes.extend_from_slice(&buffer[..retained]);
        result.truncated |= retained < count;
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpError {
    pub message: String,
}

impl McpError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for McpError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_approval: bool,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub transport: McpTransportConfig,
}

/// Candidate locations of MCP configs written by other tools on this machine
/// (Claude Desktop, Claude Code, Cursor, and a workspace `.mcp.json`). Importing
/// these lets Cindx reuse servers the user already installed elsewhere.
pub fn external_mcp_config_candidate_paths(workspace_root: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join("Library/Application Support/Claude/claude_desktop_config.json"));
        paths.push(home.join(".claude.json"));
        paths.push(home.join(".claude/settings.json"));
        paths.push(home.join(".cursor/mcp.json"));
        paths.push(home.join(".config/mcp/config.json"));
        paths.push(home.join(".config/opencode/opencode.jsonc"));
        paths.push(home.join(".config/opencode/opencode.json"));
        paths.push(home.join(".config/opencode/config.json"));
        paths.push(home.join(".opencode/config.json"));
    }
    if let Some(root) = workspace_root {
        paths.push(root.join(".mcp.json"));
    }
    paths
}

/// Strip `//` and `/* */` comments (JSONC) without touching string literals, so
/// configs such as opencode's `opencode.jsonc` parse as JSON.
pub fn strip_jsonc_comments(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while let Some(character) = chars.next() {
        if in_line_comment {
            if character == '\n' {
                in_line_comment = false;
                output.push(character);
            }
            continue;
        }
        if in_block_comment {
            if character == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block_comment = false;
            }
            continue;
        }
        if in_string {
            output.push(character);
            if character == '\\' {
                if let Some(escaped) = chars.next() {
                    output.push(escaped);
                }
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                output.push(character);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                in_line_comment = true;
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                in_block_comment = true;
            }
            _ => output.push(character),
        }
    }
    output
}

fn external_server_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "external".to_string()
    } else {
        slug
    }
}

fn external_server_config_from_entry(name: &str, entry: &Value) -> Option<McpServerConfig> {
    let obj = entry.as_object()?;
    if obj.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let env = obj
        .get("env")
        .and_then(Value::as_object)
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
                .collect()
        })
        .unwrap_or_default();
    // `command` may be a string plus `args` (Claude/Cursor) or a single array
    // whose first element is the binary (opencode).
    let stdio = if let Some(command) = obj.get("command").and_then(Value::as_str) {
        let args = obj
            .get("args")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Some((command.to_string(), args))
    } else if let Some(parts) = obj.get("command").and_then(Value::as_array) {
        let mut parts = parts.iter().filter_map(Value::as_str);
        let command = parts.next()?.to_string();
        let args = parts.map(str::to_string).collect();
        Some((command, args))
    } else {
        None
    };
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let transport = if let Some((command, args)) = stdio {
        McpTransportConfig::Stdio { command, args, env }
    } else if matches!(kind.as_str(), "http" | "https" | "sse" | "remote" | "streamable-http")
        || obj.get("url").is_some()
    {
        let url = obj.get("url").and_then(Value::as_str)?;
        let headers = obj
            .get("headers")
            .and_then(Value::as_object)
            .map(|pairs| {
                pairs
                    .iter()
                    .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        McpTransportConfig::StreamableHttp {
            url: url.to_string(),
            headers,
        }
    } else {
        return None;
    };
    Some(McpServerConfig {
        id: format!("ext-{}", external_server_id(name)),
        name: name.to_string(),
        enabled: true,
        require_approval: true,
        timeout_ms: 30_000,
        transport,
    })
}

/// Parse a foreign MCP config document (a `mcpServers` map, or a bare
/// name->server object) into Cindx server configs. Unparseable entries are
/// skipped rather than failing the whole import.
pub fn parse_external_mcp_servers(raw: &str) -> Vec<McpServerConfig> {
    let Ok(document) = serde_json::from_str::<Value>(&strip_jsonc_comments(raw)) else {
        return Vec::new();
    };
    let servers_map = document
        .get("mcpServers")
        .or_else(|| document.get("mcp_servers"))
        .or_else(|| document.get("mcp"))
        .cloned()
        .unwrap_or_else(|| document.clone());
    let Some(entries) = servers_map.as_object() else {
        return Vec::new();
    };
    let mut servers: Vec<McpServerConfig> = entries
        .iter()
        .filter_map(|(name, entry)| external_server_config_from_entry(name, entry))
        .collect();
    servers.sort_by(|left, right| left.id.cmp(&right.id));
    servers
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalogSnapshot {
    pub server_id: String,
    pub refreshed_at_ms: u64,
    pub tools: Vec<McpToolDescriptor>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct McpCatalogCache {
    #[serde(default)]
    servers: BTreeMap<String, McpCatalogSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerState {
    pub config: McpServerConfig,
    pub tool_count: usize,
    pub refreshed_at_ms: Option<u64>,
    pub last_error: Option<String>,
}

type PendingResponse = Sender<Result<Value, McpError>>;

struct SharedProcess {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Mutex<BTreeMap<u64, PendingResponse>>,
    alive: AtomicBool,
}

struct McpStdioClient {
    shared: Arc<SharedProcess>,
    next_id: AtomicU64,
    timeout: Duration,
}

struct McpHttpClient {
    url: String,
    headers: BTreeMap<String, String>,
    session_id: Mutex<Option<String>>,
    next_id: AtomicU64,
    timeout: Duration,
}

enum McpClientConnection {
    Stdio(McpStdioClient),
    Http(McpHttpClient),
}

fn wait_for_child_with_control(
    mut child: Child,
    timeout: Duration,
    control: Option<&ToolExecutionControl>,
) -> Result<Output, McpError> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| McpError::new("MCP HTTP stdout is unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| McpError::new("MCP HTTP stderr is unavailable"))?;
    let stdout_reader =
        thread::spawn(move || read_limited_and_drain(&mut stdout, MCP_HTTP_STDOUT_MAX_BYTES));
    let stderr_reader =
        thread::spawn(move || read_limited_and_drain(&mut stderr, MCP_HTTP_STDERR_MAX_BYTES));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if control.is_some_and(ToolExecutionControl::should_cancel) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(McpError::new("MCP HTTP request cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(40)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(McpError::new(format!(
                    "MCP HTTP request timed out after {} ms",
                    timeout.as_millis()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(McpError::new(format!(
                    "failed to inspect MCP HTTP request: {error}"
                )));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| McpError::new("MCP HTTP stdout reader panicked"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| McpError::new("MCP HTTP stderr reader panicked"))?;
    if let Some(error) = stdout.error {
        return Err(McpError::new(format!(
            "failed to read MCP HTTP stdout: {error}"
        )));
    }
    if stdout.truncated {
        return Err(McpError::new(format!(
            "MCP HTTP response exceeded the {} byte safety limit ({} bytes produced)",
            MCP_HTTP_STDOUT_MAX_BYTES, stdout.total_bytes
        )));
    }
    if let Some(error) = stderr.error {
        return Err(McpError::new(format!(
            "failed to read MCP HTTP stderr: {error}"
        )));
    }
    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

impl McpStdioClient {
    fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        let McpTransportConfig::Stdio { command, args, env } = &config.transport else {
            return Err(McpError::new(
                "Streamable HTTP MCP is configured but unavailable in this build",
            ));
        };
        if command.trim().is_empty() {
            return Err(McpError::new("MCP stdio command is empty"));
        }

        let mut process = Command::new(command);
        process
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = process.spawn().map_err(|error| {
            McpError::new(format!("failed to start MCP server {command}: {error}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::new("MCP server stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::new("MCP server stdout is unavailable"))?;
        if let Some(stderr) = child.stderr.take() {
            thread::spawn(move || {
                let _ = io::copy(&mut BufReader::new(stderr), &mut io::sink());
            });
        }

        let shared = Arc::new(SharedProcess {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(BTreeMap::new()),
            alive: AtomicBool::new(true),
        });
        let reader_shared = Arc::clone(&shared);
        thread::spawn(move || read_responses(stdout, reader_shared));

        let client = Self {
            shared,
            next_id: AtomicU64::new(1),
            timeout: Duration::from_millis(config.timeout_ms.max(1_000)),
        };
        client.initialize()?;
        Ok(client)
    }

    fn initialize(&self) -> Result<(), McpError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "Cindx", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        self.notify("notifications/initialized", json!({}))
    }

    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_ref()
                .map(|cursor| json!({ "cursor": cursor }))
                .unwrap_or_else(|| json!({}));
            let result = self.request("tools/list", params)?;
            let rows = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| McpError::new("MCP tools/list returned no tools array"))?;
            for row in rows {
                let name = row
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| McpError::new("MCP tool is missing a name"))?;
                let input_schema = row
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(empty_object_schema);
                tools.push(McpToolDescriptor {
                    name: name.to_string(),
                    description: row
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or(name)
                        .to_string(),
                    input_schema: normalize_object_schema(input_schema),
                    output_schema: row.get("outputSchema").cloned(),
                });
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.call_tool_with_control(name, arguments, None)
    }

    fn call_tool_with_control(
        &self,
        name: &str,
        arguments: Value,
        control: Option<&ToolExecutionControl>,
    ) -> Result<Value, McpError> {
        self.request_with_control(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
            control,
        )
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.request_with_control(method, params, None)
    }

    fn request_with_control(
        &self,
        method: &str,
        params: Value,
        control: Option<&ToolExecutionControl>,
    ) -> Result<Value, McpError> {
        if !self.shared.alive.load(Ordering::SeqCst) {
            return Err(McpError::new("MCP server connection is closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.shared
            .pending
            .lock()
            .map_err(|error| McpError::new(format!("MCP pending map poisoned: {error}")))?
            .insert(id, sender);
        if let Err(error) = self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })) {
            if let Ok(mut pending) = self.shared.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        let deadline = Instant::now() + self.timeout;
        loop {
            if control.is_some_and(ToolExecutionControl::should_cancel) {
                if let Ok(mut pending) = self.shared.pending.lock() {
                    pending.remove(&id);
                }
                let _ = self.notify(
                    "notifications/cancelled",
                    json!({ "requestId": id, "reason": "Cindx run cancelled" }),
                );
                return Err(McpError::new("MCP request cancelled"));
            }
            let now = Instant::now();
            if now >= deadline {
                if let Ok(mut pending) = self.shared.pending.lock() {
                    pending.remove(&id);
                }
                self.close();
                return Err(McpError::new(format!(
                    "MCP request {method} timed out after {} ms",
                    self.timeout.as_millis()
                )));
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(40));
            match receiver.recv_timeout(wait) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(McpError::new("MCP response channel closed"));
                }
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn write_message(&self, value: &Value) -> Result<(), McpError> {
        let mut stdin = self
            .shared
            .stdin
            .lock()
            .map_err(|error| McpError::new(format!("MCP stdin lock poisoned: {error}")))?;
        serde_json::to_writer(&mut *stdin, value)
            .map_err(|error| McpError::new(format!("failed to encode MCP request: {error}")))?;
        stdin
            .write_all(b"\n")
            .and_then(|_| stdin.flush())
            .map_err(|error| McpError::new(format!("failed to write MCP request: {error}")))
    }

    fn close(&self) {
        self.shared.alive.store(false, Ordering::SeqCst);
        if let Ok(mut child) = self.shared.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        self.close();
    }
}

impl McpHttpClient {
    fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        let McpTransportConfig::StreamableHttp { url, headers } = &config.transport else {
            return Err(McpError::new("MCP server is not configured for HTTP"));
        };
        let connection = Self {
            url: url.clone(),
            headers: headers.clone(),
            session_id: Mutex::new(None),
            next_id: AtomicU64::new(1),
            timeout: Duration::from_millis(config.timeout_ms.max(1_000)),
        };
        connection.initialize()?;
        Ok(connection)
    }

    fn initialize(&self) -> Result<(), McpError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "Cindx", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        self.notify("notifications/initialized", json!({}))
    }

    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_ref()
                .map(|cursor| json!({ "cursor": cursor }))
                .unwrap_or_else(|| json!({}));
            let result = self.request("tools/list", params)?;
            tools.extend(parse_tool_descriptors(&result)?);
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.call_tool_with_control(name, arguments, None)
    }

    fn call_tool_with_control(
        &self,
        name: &str,
        arguments: Value,
        control: Option<&ToolExecutionControl>,
    ) -> Result<Value, McpError> {
        self.request_with_control(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            control,
        )
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.request_with_control(method, params, None)
    }

    fn request_with_control(
        &self,
        method: &str,
        params: Value,
        control: Option<&ToolExecutionControl>,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.post_with_control(
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
            Some(id),
            control,
        )?
        .ok_or_else(|| McpError::new(format!("MCP HTTP request {method} returned no response")))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.post(
            json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            None,
        )?;
        Ok(())
    }

    fn post(&self, payload: Value, request_id: Option<u64>) -> Result<Option<Value>, McpError> {
        self.post_with_control(payload, request_id, None)
    }

    fn post_with_control(
        &self,
        payload: Value,
        request_id: Option<u64>,
        control: Option<&ToolExecutionControl>,
    ) -> Result<Option<Value>, McpError> {
        let mut header_lines = vec![
            "Accept: application/json, text/event-stream".to_string(),
            "Content-Type: application/json".to_string(),
            format!("MCP-Protocol-Version: {MCP_PROTOCOL_VERSION}"),
        ];
        for (name, value) in &self.headers {
            header_lines.push(format!("{name}: {value}"));
        }
        if let Some(session_id) = self
            .session_id
            .lock()
            .map_err(|error| McpError::new(format!("MCP HTTP session lock poisoned: {error}")))?
            .clone()
        {
            header_lines.push(format!("MCP-Session-Id: {session_id}"));
        }
        let header_path = std::env::temp_dir().join(format!(
            "cindx-mcp-headers-{}-{}",
            std::process::id(),
            current_time_millis()
        ));
        write_private_text(&header_path, &header_lines.join("\n"))?;
        let mut command = Command::new("/usr/bin/curl");
        command
            .arg("--silent")
            .arg("--show-error")
            .arg("--include")
            .arg("--request")
            .arg("POST")
            .arg("--max-time")
            .arg(format!("{:.3}", self.timeout.as_secs_f64()))
            .arg("--header")
            .arg(format!("@{}", header_path.display()))
            .arg("--data-binary")
            .arg("@-")
            .arg(&self.url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_file(&header_path);
                return Err(McpError::new(format!(
                    "failed to start MCP HTTP request: {error}"
                )));
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(payload.to_string().as_bytes()) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&header_path);
                return Err(McpError::new(format!(
                    "failed to write MCP HTTP body: {error}"
                )));
            }
        }
        let output = wait_for_child_with_control(child, self.timeout, control);
        let _ = fs::remove_file(&header_path);
        let output = output?;
        if !output.status.success() {
            return Err(McpError::new(format!(
                "MCP HTTP transport failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        let (status, headers, body) = parse_http_response(&raw)?;
        if let Some(session_id) = header_value(&headers, "mcp-session-id") {
            *self.session_id.lock().map_err(|error| {
                McpError::new(format!("MCP HTTP session lock poisoned: {error}"))
            })? = Some(session_id);
        }
        if !(200..300).contains(&status) {
            return Err(McpError::new(format!("MCP HTTP {status}: {body}")));
        }
        if request_id.is_none() || body.trim().is_empty() {
            return Ok(None);
        }
        let content_type = header_value(&headers, "content-type").unwrap_or_default();
        let message = if content_type.contains("text/event-stream") {
            parse_sse_response(body, request_id.unwrap())?
        } else {
            serde_json::from_str::<Value>(body).map_err(|error| {
                McpError::new(format!("invalid MCP HTTP JSON response: {error}"))
            })?
        };
        if let Some(error) = message.get("error") {
            return Err(McpError::new(format!("MCP error: {error}")));
        }
        Ok(message.get("result").cloned())
    }
}

impl McpClientConnection {
    fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        match &config.transport {
            McpTransportConfig::Stdio { .. } => McpStdioClient::connect(config).map(Self::Stdio),
            McpTransportConfig::StreamableHttp { .. } => {
                McpHttpClient::connect(config).map(Self::Http)
            }
        }
    }

    fn is_alive(&self) -> bool {
        match self {
            Self::Stdio(client) => client.shared.alive.load(Ordering::SeqCst),
            Self::Http(_) => true,
        }
    }

    fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        match self {
            Self::Stdio(client) => client.list_tools(),
            Self::Http(client) => client.list_tools(),
        }
    }

    fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        match self {
            Self::Stdio(client) => client.call_tool(name, arguments),
            Self::Http(client) => client.call_tool(name, arguments),
        }
    }

    fn call_tool_with_control(
        &self,
        name: &str,
        arguments: Value,
        control: &ToolExecutionControl,
    ) -> Result<Value, McpError> {
        match self {
            Self::Stdio(client) => client.call_tool_with_control(name, arguments, Some(control)),
            Self::Http(client) => client.call_tool_with_control(name, arguments, Some(control)),
        }
    }
}

fn parse_sse_response(body: &str, request_id: u64) -> Result<Value, McpError> {
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let value: Value = serde_json::from_str(data.trim())
            .map_err(|error| McpError::new(format!("invalid MCP SSE data: {error}")))?;
        if value.get("id").and_then(Value::as_u64) == Some(request_id) {
            return Ok(value);
        }
    }
    Err(McpError::new(
        "MCP SSE response did not include the request id",
    ))
}

fn parse_http_response(raw: &str) -> Result<(u16, BTreeMap<String, String>, &str), McpError> {
    let mut remaining = raw;
    loop {
        let (head, body) = remaining
            .split_once("\r\n\r\n")
            .or_else(|| remaining.split_once("\n\n"))
            .ok_or_else(|| McpError::new("MCP HTTP response has no header boundary"))?;
        let mut lines = head.lines();
        let status_line = lines
            .next()
            .ok_or_else(|| McpError::new("MCP HTTP response has no status line"))?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| McpError::new(format!("invalid MCP HTTP status: {status_line}")))?;
        if (100..200).contains(&status) {
            remaining = body;
            continue;
        }
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        return Ok((status, headers, body));
    }
}

fn header_value(headers: &BTreeMap<String, String>, name: &str) -> Option<String> {
    headers.get(&name.to_ascii_lowercase()).cloned()
}

fn parse_tool_descriptors(result: &Value) -> Result<Vec<McpToolDescriptor>, McpError> {
    let rows = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::new("MCP tools/list returned no tools array"))?;
    rows.iter()
        .map(|row| {
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::new("MCP tool is missing a name"))?;
            Ok(McpToolDescriptor {
                name: name.to_string(),
                description: row
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or(name)
                    .to_string(),
                input_schema: normalize_object_schema(
                    row.get("inputSchema")
                        .cloned()
                        .unwrap_or_else(empty_object_schema),
                ),
                output_schema: row.get("outputSchema").cloned(),
            })
        })
        .collect()
}

fn read_responses(stdout: std::process::ChildStdout, shared: Arc<SharedProcess>) {
    let mut reader = BufReader::new(stdout);
    let terminal_error = loop {
        let (line, truncated) = match read_bounded_line(&mut reader, MCP_STDIO_RESPONSE_MAX_BYTES) {
            Ok(Some(line)) => line,
            Ok(None) => break "MCP server closed its stdout".to_string(),
            Err(error) => break format!("failed to read MCP response: {error}"),
        };
        if truncated {
            break format!(
                "MCP response exceeded the {} byte safety limit",
                MCP_STDIO_RESPONSE_MAX_BYTES
            );
        }
        let line = String::from_utf8_lossy(&line);
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let sender = shared
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&id));
        if let Some(sender) = sender {
            let result = if let Some(error) = message.get("error") {
                Err(McpError::new(format!("MCP error: {error}")))
            } else {
                message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| McpError::new("MCP response is missing result"))
            };
            let _ = sender.send(result);
        }
    };
    shared.alive.store(false, Ordering::SeqCst);
    if let Ok(mut child) = shared.child.lock() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Ok(mut pending) = shared.pending.lock() {
        let senders = std::mem::take(&mut *pending);
        for (_, sender) in senders {
            let _ = sender.send(Err(McpError::new(terminal_error.clone())));
        }
    }
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<(Vec<u8>, bool)>> {
    let mut line = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut truncated = false;
    let mut saw_bytes = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if saw_bytes {
                Ok(Some((line, truncated)))
            } else {
                Ok(None)
            };
        }
        saw_bytes = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let body_len = newline.unwrap_or(available.len());
        let remaining = max_bytes.saturating_sub(line.len());
        let retained = remaining.min(body_len);
        line.extend_from_slice(&available[..retained]);
        truncated |= retained < body_len;
        let consumed = newline.map_or(available.len(), |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some((line, truncated)));
        }
    }
}

#[derive(Default)]
pub struct McpRuntime {
    clients: Mutex<BTreeMap<String, Arc<McpClientConnection>>>,
}

impl McpRuntime {
    fn client(&self, config: &McpServerConfig) -> Result<Arc<McpClientConnection>, McpError> {
        let mut clients = self
            .clients
            .lock()
            .map_err(|error| McpError::new(format!("MCP runtime lock poisoned: {error}")))?;
        if let Some(client) = clients.get(&config.id).filter(|client| client.is_alive()) {
            return Ok(Arc::clone(client));
        }
        clients.remove(&config.id);
        let client = Arc::new(McpClientConnection::connect(config)?);
        clients.insert(config.id.clone(), Arc::clone(&client));
        Ok(client)
    }

    pub fn disconnect(&self, server_id: &str) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.remove(server_id);
        }
    }
}

pub struct McpCatalogService {
    config_path: PathBuf,
    cache_path: PathBuf,
    servers: Vec<McpServerConfig>,
    cache: McpCatalogCache,
    runtime: Arc<McpRuntime>,
}

impl McpCatalogService {
    pub fn load(config_path: impl Into<PathBuf>, cache_path: impl Into<PathBuf>) -> Self {
        let config_path = config_path.into();
        let cache_path = cache_path.into();
        let servers = read_json::<Vec<McpServerConfig>>(&config_path).unwrap_or_default();
        let cache = read_json::<McpCatalogCache>(&cache_path).unwrap_or_default();
        Self {
            config_path,
            cache_path,
            servers,
            cache,
            runtime: Arc::new(McpRuntime::default()),
        }
    }

    pub fn states(&self) -> Vec<McpServerState> {
        self.servers
            .iter()
            .cloned()
            .map(|config| {
                let snapshot = self.cache.servers.get(&config.id);
                McpServerState {
                    config,
                    tool_count: snapshot.map(|snapshot| snapshot.tools.len()).unwrap_or(0),
                    refreshed_at_ms: snapshot.map(|snapshot| snapshot.refreshed_at_ms),
                    last_error: snapshot.and_then(|snapshot| snapshot.last_error.clone()),
                }
            })
            .collect()
    }

    pub fn save_servers(&mut self, servers: Vec<McpServerConfig>) -> Result<(), McpError> {
        validate_server_configs(&servers)?;
        for previous in &self.servers {
            if !servers
                .iter()
                .any(|server| server.id == previous.id && server == previous)
            {
                self.runtime.disconnect(&previous.id);
            }
        }
        self.servers = servers;
        write_private_json(&self.config_path, &self.servers)
    }

    pub fn upsert_server(&mut self, server: McpServerConfig) -> Result<(), McpError> {
        let mut servers = self.servers.clone();
        if let Some(existing) = servers.iter_mut().find(|existing| existing.id == server.id) {
            *existing = server;
        } else {
            servers.push(server);
        }
        self.save_servers(servers)
    }

    pub fn update_policy(
        &mut self,
        server_id: &str,
        enabled: bool,
        require_approval: bool,
    ) -> Result<(), McpError> {
        let mut servers = self.servers.clone();
        let server = servers
            .iter_mut()
            .find(|server| server.id == server_id)
            .ok_or_else(|| McpError::new(format!("unknown MCP server: {server_id}")))?;
        server.enabled = enabled;
        server.require_approval = require_approval;
        self.save_servers(servers)
    }

    pub fn remove_server(&mut self, server_id: &str) -> Result<(), McpError> {
        let mut servers = self.servers.clone();
        let before = servers.len();
        servers.retain(|server| server.id != server_id);
        if servers.len() == before {
            return Err(McpError::new(format!("unknown MCP server: {server_id}")));
        }
        self.runtime.disconnect(server_id);
        self.cache.servers.remove(server_id);
        self.save_servers(servers)?;
        write_private_json(&self.cache_path, &self.cache)
    }

    pub fn refresh_server(&mut self, server_id: &str) -> Result<McpCatalogSnapshot, McpError> {
        let config = self
            .servers
            .iter()
            .find(|server| server.id == server_id)
            .cloned()
            .ok_or_else(|| McpError::new(format!("unknown MCP server: {server_id}")))?;
        if !config.enabled {
            return Err(McpError::new("MCP server is disabled"));
        }
        let now = current_time_millis();
        let snapshot = match self
            .runtime
            .client(&config)
            .and_then(|client| client.list_tools())
        {
            Ok(tools) => McpCatalogSnapshot {
                server_id: server_id.to_string(),
                refreshed_at_ms: now,
                tools,
                last_error: None,
            },
            Err(error) => McpCatalogSnapshot {
                server_id: server_id.to_string(),
                refreshed_at_ms: now,
                tools: self
                    .cache
                    .servers
                    .get(server_id)
                    .map(|snapshot| snapshot.tools.clone())
                    .unwrap_or_default(),
                last_error: Some(error.message.clone()),
            },
        };
        self.cache
            .servers
            .insert(server_id.to_string(), snapshot.clone());
        write_private_json(&self.cache_path, &self.cache)?;
        if let Some(error) = &snapshot.last_error {
            Err(McpError::new(error.clone()))
        } else {
            Ok(snapshot)
        }
    }

    pub fn cached_tools(&self) -> Vec<Box<dyn Tool>> {
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();
        for server in self.servers.iter().filter(|server| server.enabled) {
            let Some(snapshot) = self.cache.servers.get(&server.id) else {
                continue;
            };
            for descriptor in &snapshot.tools {
                tools.push(Box::new(McpRemoteTool {
                    server: server.clone(),
                    descriptor: descriptor.clone(),
                    runtime: Arc::clone(&self.runtime),
                }));
            }
        }
        tools
    }
}

struct McpRemoteTool {
    server: McpServerConfig,
    descriptor: McpToolDescriptor,
    runtime: Arc<McpRuntime>,
}

impl McpRemoteTool {
    fn execute_remote(
        &self,
        invocation: ToolInvocation,
        control: Option<&ToolExecutionControl>,
    ) -> Result<ToolResult, ToolError> {
        let arguments = serde_json::from_str::<Value>(&invocation.input_json)
            .map_err(|error| ToolError::new(format!("invalid MCP tool arguments: {error}")))?;
        if !arguments.is_object() {
            return Err(ToolError::new("MCP tool arguments must be a JSON object"));
        }
        let result = self
            .runtime
            .client(&self.server)
            .and_then(|client| match control {
                Some(control) => {
                    client.call_tool_with_control(&self.descriptor.name, arguments, control)
                }
                None => client.call_tool(&self.descriptor.name, arguments),
            })
            .map_err(|error| ToolError::new(error.message))?;
        Ok(mcp_tool_result(
            invocation,
            &self.server,
            &self.descriptor,
            result,
        ))
    }
}

impl Tool for McpRemoteTool {
    fn spec(&self) -> ToolSpec {
        let mut spec = ToolSpec::new(
            mcp_wire_name(&self.server.name, &self.descriptor.name),
            format!("mcp:{}", self.server.name),
            self.descriptor.description.clone(),
            ToolRisk::SensitiveContext,
            ToolSource::Mcp {
                server_id: self.server.id.clone(),
            },
            if self.server.require_approval {
                ToolExposure::Inline
            } else {
                ToolExposure::Auto
            },
            normalize_object_schema(self.descriptor.input_schema.clone()).to_string(),
        );
        spec.output_schema_json = self.descriptor.output_schema.as_ref().map(Value::to_string);
        spec
    }

    fn permission_request(&self, invocation: &ToolInvocation) -> Option<PermissionRequest> {
        if !self.server.require_approval {
            return None;
        }
        Some(PermissionRequest {
            id: PermissionRequestId(String::new()),
            task_id: invocation.task_id.clone(),
            risk: PermissionRisk::Sensitive,
            action: invocation.tool_name.clone(),
            reason: format!(
                "Allow MCP server {} to run tool {}.",
                self.server.name, self.descriptor.name
            ),
            scope: format!("mcp:{}/{}", self.server.id, self.descriptor.name),
            metadata: [
                ("tool_call_id".to_string(), invocation.id.0.clone()),
                ("tool_name".to_string(), invocation.tool_name.clone()),
                ("tool_input".to_string(), invocation.input_json.clone()),
                ("mcp_server_id".to_string(), self.server.id.clone()),
                ("mcp_tool_name".to_string(), self.descriptor.name.clone()),
            ]
            .into_iter()
            .collect(),
        })
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        self.execute_remote(invocation, None)
    }

    fn execute_with_control(
        &self,
        invocation: ToolInvocation,
        control: &ToolExecutionControl,
    ) -> Result<ToolResult, ToolError> {
        self.execute_remote(invocation, Some(control))
    }
}

fn mcp_tool_result(
    invocation: ToolInvocation,
    server: &McpServerConfig,
    descriptor: &McpToolDescriptor,
    result: Value,
) -> ToolResult {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut content = Vec::new();
    let mut output = String::new();
    let mut output_bytes = 0u64;
    let mut output_truncated = false;
    for item in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
                let preview =
                    append_mcp_preview(&mut output, text, &mut output_bytes, &mut output_truncated);
                if !preview.is_empty() {
                    content.push(ToolContent::Text(preview));
                }
            }
            Some("image") => {
                content.push(ToolContent::Image {
                    mime_type: item
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    data: item
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
                append_mcp_preview(
                    &mut output,
                    "[MCP image output]",
                    &mut output_bytes,
                    &mut output_truncated,
                );
            }
            Some("resource") => {
                let resource = item.get("resource").unwrap_or(item);
                let uri = resource
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let text = resource
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let rendered = text.as_deref().unwrap_or(&uri);
                let preview = append_mcp_preview(
                    &mut output,
                    rendered,
                    &mut output_bytes,
                    &mut output_truncated,
                );
                content.push(ToolContent::Resource {
                    uri: uri.clone(),
                    text: text.map(|_| preview),
                });
            }
            _ => {
                let rendered = item.to_string();
                let preview = append_mcp_preview(
                    &mut output,
                    &rendered,
                    &mut output_bytes,
                    &mut output_truncated,
                );
                if !preview.is_empty() {
                    content.push(ToolContent::Text(preview));
                }
            }
        }
    }
    if output.is_empty() {
        let rendered = result.to_string();
        append_mcp_preview(
            &mut output,
            &rendered,
            &mut output_bytes,
            &mut output_truncated,
        );
    }
    if output_truncated {
        output.push_str("\n\n[MCP output preview bounded. The complete response is available as a structured artifact.]");
    }
    let status = if is_error {
        ToolOutcomeStatus::Failed
    } else {
        ToolOutcomeStatus::Succeeded
    };
    let mut metadata = Metadata::new();
    metadata.insert("mcp_server_id".to_string(), server.id.clone());
    metadata.insert("mcp_tool_name".to_string(), descriptor.name.clone());
    metadata.insert("output_bytes".to_string(), output_bytes.to_string());
    metadata.insert("output_truncated".to_string(), output_truncated.to_string());
    let structured_output_json = if output_truncated {
        Some(result.to_string())
    } else {
        result.get("structuredContent").map(Value::to_string)
    };
    ToolResult {
        invocation_id: invocation.id,
        status,
        output: output.clone(),
        content: if content.is_empty() {
            vec![ToolContent::Text(output.clone())]
        } else {
            content
        },
        structured_output_json,
        artifacts: Vec::new(),
        failure: is_error.then(|| ToolFailure {
            code: "mcp_tool_error".to_string(),
            message: output,
            retryable: false,
        }),
        model_observation: None,
        metadata,
    }
}

fn append_mcp_preview(
    output: &mut String,
    value: &str,
    total_bytes: &mut u64,
    truncated: &mut bool,
) -> String {
    let separator_bytes = (!output.is_empty()) as u64;
    *total_bytes = total_bytes
        .saturating_add(separator_bytes)
        .saturating_add(value.len() as u64);
    if !output.is_empty() && output.len() < MCP_TOOL_PREVIEW_MAX_BYTES {
        output.push('\n');
    }
    let remaining = MCP_TOOL_PREVIEW_MAX_BYTES.saturating_sub(output.len());
    let mut end = remaining.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let preview = value[..end].to_string();
    output.push_str(&preview);
    *truncated |= end < value.len();
    preview
}

fn validate_server_configs(servers: &[McpServerConfig]) -> Result<(), McpError> {
    let mut ids = BTreeMap::new();
    for server in servers {
        if server.id.trim().is_empty() || server.name.trim().is_empty() {
            return Err(McpError::new("MCP server id and name are required"));
        }
        if ids.insert(server.id.clone(), ()).is_some() {
            return Err(McpError::new(format!(
                "duplicate MCP server id: {}",
                server.id
            )));
        }
        match &server.transport {
            McpTransportConfig::Stdio { command, .. } if command.trim().is_empty() => {
                return Err(McpError::new(format!(
                    "MCP server {} has an empty command",
                    server.name
                )));
            }
            McpTransportConfig::StreamableHttp { url, .. }
                if !(url.starts_with("http://") || url.starts_with("https://")) =>
            {
                return Err(McpError::new(format!(
                    "MCP server {} has an invalid HTTP URL",
                    server.name
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<(), McpError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| McpError::new(format!("failed to encode MCP config: {error}")))?;
    write_private_text(path, &text)
}

fn write_private_text(path: &Path, text: &str) -> Result<(), McpError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            McpError::new(format!("failed to create MCP config directory: {error}"))
        })?;
    }
    write_private_file_atomically(path, text.as_bytes())
        .map_err(|error| McpError::new(format!("failed to write {}: {error}", path.display())))
}

fn normalize_object_schema(schema: Value) -> Value {
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        schema
    } else {
        empty_object_schema()
    }
}

fn empty_object_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": true })
}

fn mcp_wire_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", wire_segment(server), wire_segment(tool))
}

fn wire_segment(value: &str) -> String {
    let mut segment = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            segment.push(character);
        } else if !segment.ends_with('_') {
            segment.push('_');
        }
    }
    segment.trim_matches('_').to_string()
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn default_true() -> bool {
    true
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_external_mcp_servers_handles_stdio_and_http_entries() {
        let raw = r#"{
          "mcpServers": {
            "filesystem": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
              "env": {"HOME": "/Users/x"}
            },
            "remote": { "type": "http", "url": "https://example.com/mcp" },
            "broken": { "nothing": true }
          }
        }"#;
        let servers = parse_external_mcp_servers(raw);
        assert_eq!(servers.len(), 2);
        let fs = servers.iter().find(|s| s.name == "filesystem").unwrap();
        match &fs.transport {
            McpTransportConfig::Stdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
                assert_eq!(env.get("HOME").map(String::as_str), Some("/Users/x"));
            }
            _ => panic!("filesystem should be stdio"),
        }
        assert!(fs.enabled);
        assert!(fs.require_approval);
        let remote = servers.iter().find(|s| s.name == "remote").unwrap();
        match &remote.transport {
            McpTransportConfig::StreamableHttp { url, .. } => {
                assert_eq!(url, "https://example.com/mcp");
            }
            _ => panic!("remote should be http"),
        }
    }

    #[test]
    fn parse_external_mcp_servers_handles_opencode_jsonc_array_command() {
        let raw = r#"{
          // opencode config uses "mcp" and an array command
          "mcp": {
            "cua-driver": {
              "type": "local",
              "command": ["/usr/bin/true", "mcp"],
              "enabled": true
            },
            "disabled-one": { "command": ["/usr/bin/true"], "enabled": false }
          }
        }"#;
        let servers = parse_external_mcp_servers(raw);
        assert_eq!(servers.len(), 1);
        let cua = &servers[0];
        assert_eq!(cua.name, "cua-driver");
        match &cua.transport {
            McpTransportConfig::Stdio { command, args, .. } => {
                assert_eq!(command, "/usr/bin/true");
                assert_eq!(args, &vec!["mcp".to_string()]);
            }
            _ => panic!("cua-driver should be stdio"),
        }
    }

    #[test]
    fn strip_jsonc_comments_preserves_strings_with_slashes() {
        let raw = r#"{ "url": "https://x.example//path", /* block */ "k": "//not-comment" } // tail"#;
        let stripped = strip_jsonc_comments(raw);
        let value: Value = serde_json::from_str(&stripped).expect("valid after strip");
        assert_eq!(value["url"], "https://x.example//path");
        assert_eq!(value["k"], "//not-comment");
    }

    #[test]
    fn parse_external_mcp_servers_accepts_bare_object_and_skips_invalid() {
        let raw = r#"{ "gh": { "command": "gh-mcp" }, "bad": {} }"#;
        let servers = parse_external_mcp_servers(raw);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "gh");
        assert_eq!(servers[0].id, "ext-gh");
    }

    #[cfg(unix)]
    #[test]
    fn controlled_child_wait_cancels_promptly() {
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exec sleep 30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("test process should start");
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = Arc::clone(&cancelled);
        let control = ToolExecutionControl::new(move || cancellation.load(Ordering::SeqCst));
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            cancelled.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let error = wait_for_child_with_control(child, Duration::from_secs(30), Some(&control))
            .expect_err("controlled process should cancel");
        trigger.join().expect("cancellation trigger should finish");

        assert!(error.message.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn creates_stable_mcp_wire_names() {
        assert_eq!(
            mcp_wire_name("Git Hub", "search/issues"),
            "mcp__Git_Hub__search_issues"
        );
    }

    #[test]
    fn validates_unique_server_ids() {
        let server = McpServerConfig {
            id: "one".to_string(),
            name: "One".to_string(),
            enabled: true,
            require_approval: true,
            timeout_ms: 1_000,
            transport: McpTransportConfig::Stdio {
                command: "server".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        };
        assert!(validate_server_configs(std::slice::from_ref(&server)).is_ok());
        assert!(validate_server_configs(&[server.clone(), server]).is_err());
    }

    #[test]
    fn cache_only_catalog_builds_remote_tools_without_connecting() {
        let root = std::env::temp_dir().join(format!("cindx-mcp-test-{}", current_time_millis()));
        let config_path = root.join("servers.json");
        let cache_path = root.join("catalog.json");
        let server = McpServerConfig {
            id: "mock".to_string(),
            name: "Mock".to_string(),
            enabled: true,
            require_approval: true,
            timeout_ms: 1_000,
            transport: McpTransportConfig::Stdio {
                command: "does-not-run-during-cache-read".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        };
        write_private_json(&config_path, &vec![server]).unwrap();
        write_private_json(
            &cache_path,
            &McpCatalogCache {
                servers: [(
                    "mock".to_string(),
                    McpCatalogSnapshot {
                        server_id: "mock".to_string(),
                        refreshed_at_ms: 1,
                        tools: vec![McpToolDescriptor {
                            name: "hello".to_string(),
                            description: "Say hello".to_string(),
                            input_schema: empty_object_schema(),
                            output_schema: None,
                        }],
                        last_error: None,
                    },
                )]
                .into_iter()
                .collect(),
            },
        )
        .unwrap();
        let service = McpCatalogService::load(config_path, cache_path);
        let tools = service.cached_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].spec().name, "mcp__Mock__hello");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_streamable_http_json_and_sse_responses() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMCP-Session-Id: session-1\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}";
        let (status, headers, body) = parse_http_response(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(
            header_value(&headers, "MCP-Session-Id").as_deref(),
            Some("session-1")
        );
        assert!(body.contains("\"result\""));

        let sse =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[]}}\n\n";
        let message = parse_sse_response(sse, 7).unwrap();
        assert_eq!(message["result"]["tools"], json!([]));
    }

    #[test]
    fn bounded_line_reader_drains_an_oversized_response() {
        let input = format!("{}\nnext\n", "x".repeat(32));
        let mut reader = BufReader::new(input.as_bytes());

        let (first, truncated) = read_bounded_line(&mut reader, 8)
            .expect("first line should read")
            .expect("first line should exist");
        let (second, second_truncated) = read_bounded_line(&mut reader, 8)
            .expect("second line should read")
            .expect("second line should exist");

        assert_eq!(first, b"xxxxxxxx");
        assert!(truncated);
        assert_eq!(second, b"next");
        assert!(!second_truncated);
    }

    #[test]
    fn mcp_large_text_result_keeps_a_bounded_preview_and_raw_artifact_payload() {
        let server = McpServerConfig {
            id: "server-1".to_string(),
            name: "Server".to_string(),
            enabled: true,
            require_approval: false,
            timeout_ms: 1_000,
            transport: McpTransportConfig::Stdio {
                command: "server".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        };
        let descriptor = McpToolDescriptor {
            name: "large".to_string(),
            description: "Large result".to_string(),
            input_schema: empty_object_schema(),
            output_schema: None,
        };
        let invocation = ToolInvocation {
            id: agent_core::ToolCallId("call-large".to_string()),
            task_id: agent_core::TaskId("task-1".to_string()),
            tool_name: "mcp__Server__large".to_string(),
            input_json: "{}".to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        };
        let result = mcp_tool_result(
            invocation,
            &server,
            &descriptor,
            json!({
                "content": [{ "type": "text", "text": "x".repeat(300 * 1024) }]
            }),
        );

        assert!(result.output.len() < MCP_TOOL_PREVIEW_MAX_BYTES + 256);
        assert_eq!(
            result.metadata.get("output_truncated").map(String::as_str),
            Some("true")
        );
        assert!(result
            .structured_output_json
            .as_deref()
            .is_some_and(|value| value.len() > 300 * 1024));
    }
}
