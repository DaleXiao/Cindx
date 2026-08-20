use crate::{Tool, ToolError};
use agent_core::{
    Metadata, PermissionRequest, ToolInvocation, ToolOutcomeStatus, ToolResult, ToolRisk, ToolSpec,
};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TodoItem {
    content: String,
    status: String,
    priority: String,
}

const TODO_SCHEMA: &str = r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed"]},"priority":{"type":"string","enum":["high","medium","low"]}},"required":["content","status"]}}}}"#;

pub struct TodoTool {
    workspace_root: PathBuf,
}

impl TodoTool {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    fn store_path(&self) -> PathBuf {
        self.workspace_root.join(".cindx").join("todos.json")
    }

    fn load(&self) -> Vec<TodoItem> {
        let Ok(bytes) = std::fs::read(self.store_path()) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Vec::new();
        };
        value
            .as_array()
            .map(|items| items.iter().filter_map(item_from_value).collect())
            .unwrap_or_default()
    }
}

fn item_from_value(value: &serde_json::Value) -> Option<TodoItem> {
    let content = value.get("content")?.as_str()?.trim().to_string();
    if content.is_empty() {
        return None;
    }
    Some(TodoItem {
        content,
        status: value
            .get("status")
            .and_then(|status| status.as_str())
            .unwrap_or("pending")
            .to_string(),
        priority: value
            .get("priority")
            .and_then(|priority| priority.as_str())
            .unwrap_or("medium")
            .to_string(),
    })
}

impl Tool for TodoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::builtin(
            "todo.write",
            "todo",
            "Track the run's working memory as a flat todo list. Pass `todos` to replace the whole list; omit it to read the current list. Use it to plan and show progress; statuses are pending/in_progress/completed.",
            ToolRisk::ReadOnly,
            TODO_SCHEMA,
        )
    }

    fn permission_request(&self, _invocation: &ToolInvocation) -> Option<PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input: serde_json::Value = serde_json::from_str(&invocation.input_json)
            .map_err(|error| ToolError::new(format!("invalid todo input: {error}")))?;

        let todos = if let Some(list) = input.get("todos") {
            let Some(items) = list.as_array() else {
                return Err(ToolError::new("todo `todos` must be an array"));
            };
            let parsed: Vec<TodoItem> = items.iter().filter_map(item_from_value).collect();
            if parsed.len() != items.len() {
                return Err(ToolError::new(
                    "every todo needs non-empty content and a status",
                ));
            }
            let path = self.store_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    ToolError::new(format!("failed to create todo directory: {error}"))
                })?;
            }
            let encoded = parsed
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "content": item.content,
                        "status": item.status,
                        "priority": item.priority,
                    })
                })
                .collect::<Vec<_>>();
            let bytes = serde_json::to_vec_pretty(&encoded)
                .map_err(|error| ToolError::new(format!("failed to encode todos: {error}")))?;
            std::fs::write(&path, bytes)
                .map_err(|error| ToolError::new(format!("failed to write todos: {error}")))?;
            parsed
        } else {
            self.load()
        };

        let rendered = if todos.is_empty() {
            "Todo list is empty.".to_string()
        } else {
            todos
                .iter()
                .map(|item| format!("- [{}] ({}) {}", item.status, item.priority, item.content))
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            rendered,
            Metadata::new(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{TaskId, ToolCallId};

    fn invocation(id: &str, input: &str) -> ToolInvocation {
        ToolInvocation {
            id: ToolCallId(id.to_string()),
            task_id: TaskId("task".to_string()),
            tool_name: "todo.write".to_string(),
            input_json: input.to_string(),
            proposed_by_model: "test".to_string(),
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn todo_write_replaces_and_read_returns_list() {
        let dir = std::env::temp_dir().join(format!("cindx-todo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = TodoTool::new(dir.clone());

        let write = tool
            .execute(invocation(
                "w",
                r#"{"todos":[{"content":"add guard","status":"in_progress","priority":"high"}]}"#,
            ))
            .unwrap();
        assert!(write.output.contains("add guard"));

        let read = tool.execute(invocation("r", "{}")).unwrap();
        assert!(read.output.contains("[in_progress]"));
        assert!(read.output.contains("(high) add guard"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn todo_write_rejects_empty_content() {
        let dir = std::env::temp_dir().join(format!("cindx-todo-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = TodoTool::new(dir.clone());
        let result = tool.execute(invocation(
            "w",
            r#"{"todos":[{"content":"  ","status":"pending"}]}"#,
        ));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
