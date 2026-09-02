//! Provider-backed baseline runner for the general-agent suite.
//!
//! Reads `CINDX_EVAL_BASE_URL`, `CINDX_EVAL_API_KEY`, `CINDX_EVAL_MODEL` from the
//! environment (an OpenAI-compatible chat-completions endpoint) and drives the
//! seeded suite through the real kernel + tools. This is a measurement tool: it
//! prints a summary report and per-case failures. It makes real provider calls
//! and is only run with explicit authorization.

use agent_core::{Message, MessageRole, Metadata, ModelResponse, ModelToolCall};
use agent_eval::{parse_suite, run_case, EvalError, EvalModelProvider, SuiteReport};
use std::io::Write;
use std::process::Command;

struct CurlProvider {
    base_url: String,
    api_key: String,
    model: String,
}

impl CurlProvider {
    fn from_env() -> Result<Self, EvalError> {
        let base_url = std::env::var("CINDX_EVAL_BASE_URL")
            .map_err(|_| EvalError::new("missing_env", "CINDX_EVAL_BASE_URL not set"))?;
        let api_key = std::env::var("CINDX_EVAL_API_KEY")
            .map_err(|_| EvalError::new("missing_env", "CINDX_EVAL_API_KEY not set"))?;
        let model = std::env::var("CINDX_EVAL_MODEL")
            .map_err(|_| EvalError::new("missing_env", "CINDX_EVAL_MODEL not set"))?;
        Ok(Self {
            base_url,
            api_key,
            model,
        })
    }
}

fn message_to_json(message: &Message) -> serde_json::Value {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::Reviewer => "assistant",
    };
    let mut obj = serde_json::json!({ "role": role, "content": message.content });
    if message.role == MessageRole::Assistant {
        if let Some(raw) = message.metadata.get("raw_tool_calls_json") {
            if let Ok(calls) = serde_json::from_str::<serde_json::Value>(raw) {
                obj["tool_calls"] = calls;
            }
        }
    }
    if message.role == MessageRole::Tool {
        if let Some(id) = message.metadata.get("tool_call_id") {
            obj["tool_call_id"] = serde_json::json!(id);
        }
    }
    obj
}

impl EvalModelProvider for CurlProvider {
    fn name(&self) -> &str {
        "curl-openai-compatible"
    }

    fn complete(&mut self, request: &agent_core::ModelRequest) -> Result<ModelResponse, EvalError> {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                let parameters = serde_json::from_str::<serde_json::Value>(&tool.input_schema_json)
                    .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": parameters,
                    }
                })
            })
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0,
            "messages": request.messages.iter().map(message_to_json).collect::<Vec<_>>(),
            "tools": tools,
        });
        let mut child = Command::new("/usr/bin/curl")
            .arg("-sS")
            .arg("--max-time")
            .arg("120")
            .arg("-X")
            .arg("POST")
            .arg(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .arg("-H")
            .arg(format!("Authorization: Bearer {}", self.api_key))
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("--data-binary")
            .arg("@-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| EvalError::new("curl_spawn", error.to_string()))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(body.to_string().as_bytes())
                .map_err(|error| EvalError::new("curl_write", error.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| EvalError::new("curl_wait", error.to_string()))?;
        if !output.status.success() {
            return Err(EvalError::new(
                "curl_status",
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| EvalError::new("bad_json", error.to_string()))?;
        let message = parsed["choices"][0]["message"].clone();
        let content = message["content"].as_str().unwrap_or_default().to_string();
        let raw_calls = message.get("tool_calls").cloned();
        let mut tool_calls = Vec::new();
        if let Some(calls) = raw_calls.as_ref().and_then(|c| c.as_array()) {
            for (index, call) in calls.iter().enumerate() {
                let name = call["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let arguments_json = call["function"]["arguments"]
                    .as_str()
                    .unwrap_or("{}")
                    .to_string();
                let id = call["id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("call_{index}"));
                tool_calls.push(ModelToolCall {
                    id,
                    name,
                    arguments_json,
                });
            }
        }
        let finish = if tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        Ok(ModelResponse {
            message: Message {
                role: MessageRole::Assistant,
                content,
                metadata: Metadata::new(),
            },
            raw_tool_calls_json: raw_calls.map(|c| c.to_string()),
            tool_calls,
            metadata: [("finish_reason".to_string(), finish.to_string())]
                .into_iter()
                .collect(),
        })
    }
}

fn main() {
    // This example makes real provider calls and materializes per-case
    // workspaces on disk. It refuses to run without explicit one-shot
    // authorization so it can never be triggered accidentally (for example by a
    // blanket `cargo run --examples`).
    if std::env::var("CINDX_EVAL_AUTHORIZE").as_deref() != Ok("yes") {
        eprintln!(
            "refusing to run the online provider eval without authorization;\n\
             set CINDX_EVAL_AUTHORIZE=yes (plus CINDX_EVAL_BASE_URL/API_KEY/MODEL) to run it explicitly"
        );
        std::process::exit(2);
    }
    let suite_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "crates/agent-eval/suite/general_v1.json".to_string());
    let template = match CurlProvider::from_env() {
        Ok(provider) => provider,
        Err(error) => {
            eprintln!("eval provider unavailable: {error}");
            std::process::exit(2);
        }
    };
    let suite_text = std::fs::read_to_string(&suite_path).expect("read suite");
    let cases = parse_suite(&suite_text).expect("parse suite");
    // Per-run exclusive root: never a fixed shared path, so concurrent runs and
    // stale state cannot collide and any deletion is scoped to this run only.
    let workspace_root = std::env::temp_dir().join(format!(
        "cindx-eval-real-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    let mut report = SuiteReport {
        cases: Vec::new(),
        passed_count: 0,
        total: 0,
    };
    for case in &cases {
        let id = case.id.clone();
        // Each case gets a fresh provider so a stuck queue cannot leak across cases.
        let mut provider = CurlProvider {
            base_url: template.base_url.clone(),
            api_key: template.api_key.clone(),
            model: template.model.clone(),
        };
        let case_report = run_case(case, &mut provider, &workspace_root);
        println!(
            "[{}] passed={} tool_calls={} turns={} error={}",
            id,
            case_report.passed,
            case_report.tool_calls,
            case_report.turns,
            case_report.error.clone().unwrap_or_default()
        );
        for check in &case_report.checks {
            if !check.passed {
                println!("    failed check: {}", check.detail);
            }
        }
        let passed = case_report.passed;
        report.cases.push(case_report);
        report.total += 1;
        if passed {
            report.passed_count += 1;
        }
    }
    println!("\n{}", report.summary());
    // Best-effort cleanup of this run's exclusive root.
    let _ = std::fs::remove_dir_all(&workspace_root);
}
