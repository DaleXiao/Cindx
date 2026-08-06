use agent_core::{ToolRisk, ToolSpec};

pub(super) const DEFAULT_LIST_RESULTS: usize = 200;
pub(super) const MAX_LIST_RESULTS: usize = 1_000;
pub(super) const MAX_LIST_DISCOVERY_ENTRIES: usize = 10_000;
pub(super) const MODEL_LIST_EVIDENCE_CHARS: usize = 5_200;

pub(super) fn file_list_spec() -> ToolSpec {
    ToolSpec::builtin(
        "file.list",
        "file",
        "List a stable, bounded page of files and directories inside the workspace. Optional glob filters entry names; continue with the returned scope-bound cursor.",
        ToolRisk::ReadOnly,
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": ".", "description": "Workspace-relative directory." },
                "glob": { "type": "string", "minLength": 1, "description": "Optional entry-name glob using * and ?." },
                "max_results": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_RESULTS, "default": DEFAULT_LIST_RESULTS },
                "cursor": { "type": "string", "minLength": 1, "description": "Opaque continuation cursor returned by an earlier call with the same path and glob." }
            },
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": "cindx.file-list-result.v2" },
                "path": { "type": "string" },
                "glob": { "type": ["string", "null"] },
                "entries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": { "enum": ["dir", "file"] },
                            "bytes": { "type": "integer", "minimum": 0 },
                            "name": { "type": "string" }
                        },
                        "required": ["kind", "bytes", "name"],
                        "additionalProperties": false
                    }
                },
                "failures": { "type": "array" },
                "discovered": { "type": "integer", "minimum": 0 },
                "discovery_limit": { "type": "integer", "minimum": 1 },
                "discovery_limit_reached": { "type": "boolean" },
                "returned": { "type": "integer", "minimum": 0 },
                "total": { "type": "integer", "minimum": 0 },
                "next_cursor": { "type": ["string", "null"] },
                "complete": { "type": "boolean" },
                "cancelled": { "type": "boolean" }
            },
            "required": ["schema", "path", "glob", "entries", "failures", "discovered", "discovery_limit", "discovery_limit_reached", "returned", "total", "next_cursor", "complete", "cancelled"],
            "additionalProperties": false
        })
        .to_string(),
    )
}
