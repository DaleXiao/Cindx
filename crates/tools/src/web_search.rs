use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use agent_core::{
    curl_policy_args, http_policy, HttpEgressProfile, Metadata, PermissionRequest, PermissionRisk,
    ToolEffectSemantics, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
    HTTP_STDERR_MAX_BYTES, HTTP_WEB_RESPONSE_MAX_BYTES,
};

use super::{
    builtin_tool_spec, parse_input, permission_request, required_input, tool_result, Tool,
    ToolError, WebSearchConfig,
};
use crate::stream_capture::capture_stream_limited;

pub(crate) const WEB_RESPONSE_MAX_BYTES: usize = HTTP_WEB_RESPONSE_MAX_BYTES;
pub(crate) const WEB_STDERR_MAX_BYTES: usize = HTTP_STDERR_MAX_BYTES;

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
        .with_effect_semantics(ToolEffectSemantics::ReadOnly)
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

fn fetch_url(url: &str) -> Result<String, ToolError> {
    let mut command = Command::new("/usr/bin/curl");
    // The fallback endpoint is a compile-time constant, so no address audit
    // applies, but its redirects are bounded and HTTPS-only through the shared
    // policy owner instead of curl's unlimited default.
    let policy = http_policy(HttpEgressProfile::PublicSearchFallback);
    command.args(curl_policy_args(&policy));
    command.arg(url);
    let output = run_command_with_limited_output(
        &mut command,
        policy.response_max_bytes,
        policy.stderr_max_bytes,
        "curl",
        None,
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
    let (mut command, secret_stdin) = search_api_request(config, query, max_results)?;
    let output = run_command_with_limited_output(
        &mut command,
        WEB_RESPONSE_MAX_BYTES,
        WEB_STDERR_MAX_BYTES,
        "search API request",
        secret_stdin.as_deref(),
    )?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let api_key = config.api_key.trim();
        let detail = if api_key.is_empty() {
            detail
        } else {
            detail.replace(api_key, "[redacted]")
        };
        return Err(ToolError::new(format!(
            "search API request failed: {detail}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn search_api_request(
    config: &WebSearchConfig,
    query: &str,
    max_results: usize,
) -> Result<(Command, Option<Vec<u8>>), ToolError> {
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
    let api_key = config.api_key.trim();
    if !api_key.is_empty() && endpoint.starts_with("http://") {
        return Err(ToolError::new(
            "web search endpoints with an API key must use HTTPS",
        ));
    }

    let uses_url_template = endpoint.contains("{query}") || endpoint.contains("{limit}");
    let url = endpoint
        .replace("{query}", &url_encode(query))
        .replace("{limit}", &max_results.to_string());
    // One owner decides the posture: a credentialed endpoint is HTTPS-only and
    // follows no redirect at all, so the bearer header can never be replayed to
    // another origin; an uncredentialed endpoint keeps plain-HTTP loopback
    // compatibility with a bounded redirect count.
    let policy = http_policy(if api_key.is_empty() {
        HttpEgressProfile::ConfiguredSearchApi
    } else {
        HttpEgressProfile::CredentialedSearchApi
    });
    let mut command = Command::new("/usr/bin/curl");
    command.args(curl_policy_args(&policy));
    command.arg("--fail");
    let secret_stdin = if api_key.is_empty() {
        None
    } else {
        command.args(["--header", "@-"]);
        Some(format!("Authorization: Bearer {api_key}\n").into_bytes())
    };
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
    Ok((command, secret_stdin))
}

#[derive(Debug)]
pub(crate) struct LimitedCommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) fn run_command_with_limited_output(
    command: &mut Command,
    stdout_max_bytes: usize,
    stderr_max_bytes: usize,
    label: &str,
    stdin_bytes: Option<&[u8]>,
) -> Result<LimitedCommandOutput, ToolError> {
    if stdin_bytes.is_some() {
        command.stdin(Stdio::piped());
    }
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
    let stdin_writer = if let Some(bytes) = stdin_bytes {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::new(format!("{label} stdin is unavailable")))?;
        let bytes = bytes.to_vec();
        Some(thread::spawn(move || stdin.write_all(&bytes)))
    } else {
        None
    };
    let status = child
        .wait()
        .map_err(|error| ToolError::new(format!("failed to wait for {label}: {error}")))?;
    if let Some(writer) = stdin_writer {
        writer
            .join()
            .map_err(|_| ToolError::new(format!("{label} stdin writer panicked")))?
            .map_err(|error| ToolError::new(format!("failed to write {label} stdin: {error}")))?;
    }
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

pub(crate) fn html_to_text(html: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_is_network_permissioned_but_effect_read_only() {
        let spec = WebSearchTool::default().spec();

        assert_eq!(spec.risk, ToolRisk::UsesNetwork);
        assert_eq!(spec.effect_semantics, ToolEffectSemantics::ReadOnly);
    }

    #[test]
    fn credentialed_search_request_keeps_api_key_out_of_curl_arguments() {
        let api_key = "argv-secret-value";
        let config = WebSearchConfig {
            endpoint: "https://search.example.test/api".to_string(),
            api_key: api_key.to_string(),
        };

        let (command, secret_stdin) = search_api_request(&config, "local agent", 8).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args.first().map(String::as_str), Some("-q"));
        assert!(args.iter().any(|arg| arg == "-L"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--max-redirs" && pair[1] == "0"));
        assert!(!args.iter().any(|arg| arg.contains(api_key)));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--header" && pair[1] == "@-"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--proto" && pair[1] == "=https"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--proto-redir" && pair[1] == "=https"));
        assert_eq!(
            secret_stdin.unwrap(),
            format!("Authorization: Bearer {api_key}\n").into_bytes()
        );
    }

    #[test]
    fn credentialed_search_rejects_plain_http_without_leaking_the_key() {
        let api_key = "must-not-appear-in-errors";
        let config = WebSearchConfig {
            endpoint: "http://127.0.0.1:8080/search".to_string(),
            api_key: api_key.to_string(),
        };

        let error = search_api_request(&config, "local agent", 8).unwrap_err();

        assert!(error.message.contains("must use HTTPS"));
        assert!(!error.message.contains(api_key));
    }

    #[test]
    fn unauthenticated_search_keeps_plain_http_local_endpoint_compatibility() {
        let config = WebSearchConfig {
            endpoint: "http://127.0.0.1:8080/search".to_string(),
            api_key: String::new(),
        };

        let (command, secret_stdin) = search_api_request(&config, "local agent", 8).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(secret_stdin.is_none());
        assert_eq!(args.first().map(String::as_str), Some("-q"));
        assert!(args.iter().any(|arg| arg == "-L"));
        assert!(args.iter().any(|arg| arg == &config.endpoint));
    }
}
