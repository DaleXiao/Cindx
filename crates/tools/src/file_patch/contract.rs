use agent_core::{ToolRisk, ToolSpec};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

use crate::workspace_file::sha256_bytes;

pub(super) const MAX_PATCH_FILE_BYTES: usize = 8 * 1024 * 1024;
pub(super) const PATCH_RECEIPT_SCHEMA: &str = "cindx.file-patch-receipt.v1";
pub(super) const PATCH_FAILURE_SCHEMA: &str = "cindx.file-patch-failure.v1";

pub(super) fn patch_spec() -> ToolSpec {
    ToolSpec::builtin(
        "file.patch",
        "file",
        "Atomically replace one exact UTF-8 range or one unique anchor in an existing workspace file. Requires the SHA-256 of the exact base file.",
        ToolRisk::WritesWorkspace,
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1, "description": "Existing workspace-relative UTF-8 file." },
                "expected_base_sha256": { "type": "string", "pattern": "^[0-9a-fA-F]{64}$", "description": "SHA-256 of the complete base file." },
                "replacement": { "type": "string", "description": "Replacement UTF-8 text; may be empty." },
                "start_byte": { "type": "integer", "minimum": 0, "maximum": MAX_PATCH_FILE_BYTES },
                "end_byte": { "type": "integer", "minimum": 0, "maximum": MAX_PATCH_FILE_BYTES },
                "expected_text": { "type": "string", "description": "Exact UTF-8 text expected in the selected byte range; may be empty for insertion." },
                "anchor": { "type": "string", "minLength": 1, "description": "Exact UTF-8 text that must occur exactly once." }
            },
            "required": ["path", "expected_base_sha256", "replacement"],
            "oneOf": [
                {
                    "required": ["start_byte", "end_byte", "expected_text"],
                    "not": { "required": ["anchor"] }
                },
                {
                    "required": ["anchor"],
                    "not": {
                        "anyOf": [
                            { "required": ["start_byte"] },
                            { "required": ["end_byte"] },
                            { "required": ["expected_text"] }
                        ]
                    }
                }
            ],
            "additionalProperties": false
        })
        .to_string(),
    )
    .with_output_schema(
        serde_json::json!({
            "type": "object",
            "properties": {
                "schema": { "const": PATCH_RECEIPT_SCHEMA },
                "path": { "type": "string" },
                "before_sha256": { "type": "string" },
                "after_sha256": { "type": "string" },
                "diff_sha256": { "type": "string" },
                "start_byte": { "type": "integer", "minimum": 0 },
                "end_byte": { "type": "integer", "minimum": 0 },
                "before_bytes": { "type": "integer", "minimum": 0 },
                "after_bytes": { "type": "integer", "minimum": 0 },
                "replaced_bytes": { "type": "integer", "minimum": 0 },
                "replacement_bytes": { "type": "integer", "minimum": 0 }
            },
            "required": ["schema", "path", "before_sha256", "after_sha256", "diff_sha256", "start_byte", "end_byte", "before_bytes", "after_bytes", "replaced_bytes", "replacement_bytes"],
            "additionalProperties": false
        })
        .to_string(),
    )
}

pub(super) struct PatchRequest {
    pub(super) path: String,
    pub(super) expected_base_sha256: String,
    replacement: String,
    selector: PatchSelector,
}

enum PatchSelector {
    Range {
        start_byte: usize,
        end_byte: usize,
        expected_text: String,
    },
    Anchor(String),
}

impl PatchRequest {
    pub(super) fn parse(input_json: &str) -> Result<Self, PatchIssue> {
        let value = serde_json::from_str::<serde_json::Value>(input_json).map_err(|_| {
            PatchIssue::new(
                "file_patch_invalid_input",
                "file.patch input must be one JSON object.",
            )
        })?;
        let object = value.as_object().ok_or_else(|| {
            PatchIssue::new(
                "file_patch_invalid_input",
                "file.patch input must be one JSON object.",
            )
        })?;
        let allowed = BTreeSet::from([
            "path",
            "expected_base_sha256",
            "replacement",
            "start_byte",
            "end_byte",
            "expected_text",
            "anchor",
        ]);
        if object.keys().any(|key| !allowed.contains(key.as_str())) {
            return Err(PatchIssue::new(
                "file_patch_invalid_input",
                "file.patch input contains an unsupported field.",
            ));
        }
        let path = required_string(object, "path", false)?;
        let expected_base_sha256 =
            required_string(object, "expected_base_sha256", false)?.to_ascii_lowercase();
        if !is_sha256(&expected_base_sha256) {
            return Err(PatchIssue::new(
                "file_patch_invalid_base_sha256",
                "expected_base_sha256 must contain exactly 64 hexadecimal characters.",
            ));
        }
        let replacement = required_string(object, "replacement", true)?;
        if replacement.len() > MAX_PATCH_FILE_BYTES {
            return Err(PatchIssue::new(
                "file_patch_replacement_too_large",
                "replacement exceeds the 8 MiB file.patch limit.",
            ));
        }

        let has_anchor = object.contains_key("anchor");
        let range_fields = ["start_byte", "end_byte", "expected_text"];
        let range_count = range_fields
            .iter()
            .filter(|field| object.contains_key(**field))
            .count();
        let selector = match (has_anchor, range_count) {
            (true, 0) => PatchSelector::Anchor(required_string(object, "anchor", false)?),
            (false, 3) => {
                let start_byte = required_usize(object, "start_byte")?;
                let end_byte = required_usize(object, "end_byte")?;
                if start_byte > end_byte || end_byte > MAX_PATCH_FILE_BYTES {
                    return Err(PatchIssue::new(
                        "file_patch_invalid_range",
                        "The patch byte range must satisfy start_byte <= end_byte <= 8 MiB.",
                    ));
                }
                PatchSelector::Range {
                    start_byte,
                    end_byte,
                    expected_text: required_string(object, "expected_text", true)?,
                }
            }
            (true, _) => {
                return Err(PatchIssue::new(
                    "file_patch_mixed_selectors",
                    "Provide either anchor or the complete byte-range selector, never both.",
                ))
            }
            (false, _) => {
                return Err(PatchIssue::new(
                    "file_patch_incomplete_selector",
                    "Provide one unique anchor or start_byte, end_byte, and expected_text.",
                ))
            }
        };
        Ok(Self {
            path,
            expected_base_sha256,
            replacement,
            selector,
        })
    }
}

pub(super) struct PatchPlan {
    pub(super) after: Vec<u8>,
    pub(super) before_sha256: String,
    pub(super) after_sha256: String,
    pub(super) diff_sha256: String,
    pub(super) start_byte: usize,
    pub(super) end_byte: usize,
    pub(super) before_bytes: usize,
    pub(super) replaced_bytes: usize,
    pub(super) replacement_bytes: usize,
}

impl PatchPlan {
    pub(super) fn build(base: &str, request: &PatchRequest) -> Result<Self, PatchIssue> {
        let (start_byte, end_byte, expected) =
            match &request.selector {
                PatchSelector::Range {
                    start_byte,
                    end_byte,
                    expected_text,
                } => (*start_byte, *end_byte, expected_text.as_str()),
                PatchSelector::Anchor(anchor) => {
                    let matches = anchor_matches(base, anchor);
                    match matches.as_slice() {
                        [] => {
                            return Err(PatchIssue::new(
                                "file_patch_anchor_missing",
                                "The requested anchor is not present in the current base file.",
                            ))
                        }
                        [start] => (*start, start.saturating_add(anchor.len()), anchor.as_str()),
                        _ => return Err(PatchIssue::new(
                            "file_patch_anchor_ambiguous",
                            "The requested anchor occurs more than once in the current base file.",
                        )),
                    }
                }
            };
        if end_byte > base.len()
            || !base.is_char_boundary(start_byte)
            || !base.is_char_boundary(end_byte)
        {
            return Err(PatchIssue::new(
                "file_patch_invalid_range",
                "The patch byte range is outside the file or splits a UTF-8 character.",
            ));
        }
        if &base[start_byte..end_byte] != expected {
            return Err(PatchIssue::new(
                "file_patch_selector_mismatch",
                "expected_text does not match the selected byte range in the current base file.",
            ));
        }
        let after_bytes = base
            .len()
            .saturating_sub(end_byte.saturating_sub(start_byte))
            .saturating_add(request.replacement.len());
        if after_bytes > MAX_PATCH_FILE_BYTES {
            return Err(PatchIssue::new(
                "file_patch_result_too_large",
                "The patched file would exceed the 8 MiB file.patch limit.",
            ));
        }
        let mut after = Vec::with_capacity(after_bytes);
        after.extend_from_slice(&base.as_bytes()[..start_byte]);
        after.extend_from_slice(request.replacement.as_bytes());
        after.extend_from_slice(&base.as_bytes()[end_byte..]);
        if after.as_slice() == base.as_bytes() {
            return Err(PatchIssue::new(
                "file_patch_no_change",
                "The requested patch would not change the file.",
            ));
        }
        Ok(Self {
            before_sha256: sha256_bytes(base.as_bytes()),
            after_sha256: sha256_bytes(&after),
            diff_sha256: patch_diff_sha256(
                start_byte,
                end_byte,
                expected.as_bytes(),
                request.replacement.as_bytes(),
            ),
            after,
            start_byte,
            end_byte,
            before_bytes: base.len(),
            replaced_bytes: end_byte.saturating_sub(start_byte),
            replacement_bytes: request.replacement.len(),
        })
    }
}

pub(super) struct PatchIssue {
    pub(super) code: &'static str,
    pub(super) message: &'static str,
    pub(super) retryable: bool,
    pub(super) observed_sha256: Option<String>,
}

impl PatchIssue {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: false,
            observed_sha256: None,
        }
    }

    pub(super) fn retryable(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            retryable: true,
            observed_sha256: None,
        }
    }

    pub(super) fn with_observed_sha256(mut self, observed_sha256: String) -> Self {
        self.observed_sha256 = Some(observed_sha256);
        self
    }
}

pub(super) fn input_path(input_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(input_json)
        .ok()?
        .get("path")?
        .as_str()
        .map(str::to_string)
}

fn required_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &'static str,
    allow_empty: bool,
) -> Result<String, PatchIssue> {
    let value = object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            PatchIssue::new(
                "file_patch_invalid_input",
                "file.patch is missing a required string field.",
            )
        })?;
    if !allow_empty && value.trim().is_empty() {
        return Err(PatchIssue::new(
            "file_patch_invalid_input",
            "file.patch contains an empty required field.",
        ));
    }
    Ok(value.to_string())
}

fn required_usize(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &'static str,
) -> Result<usize, PatchIssue> {
    object
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            PatchIssue::new(
                "file_patch_invalid_input",
                "file.patch byte offsets must be non-negative integers.",
            )
        })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn anchor_matches(base: &str, anchor: &str) -> Vec<usize> {
    let anchor = anchor.as_bytes();
    if anchor.is_empty() || anchor.len() > base.len() {
        return Vec::new();
    }
    base.as_bytes()
        .windows(anchor.len())
        .enumerate()
        .filter_map(|(start, candidate)| {
            let end = start + anchor.len();
            (candidate == anchor && base.is_char_boundary(start) && base.is_char_boundary(end))
                .then_some(start)
        })
        .take(2)
        .collect()
}

fn patch_diff_sha256(start: usize, end: usize, expected: &[u8], replacement: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cindx.file-patch-diff.v1\n");
    digest.update(start.to_string().as_bytes());
    digest.update(b"\n");
    digest.update(end.to_string().as_bytes());
    digest.update(b"\n");
    digest.update(expected);
    digest.update(b"\0");
    digest.update(replacement);
    format!("{:x}", digest.finalize())
}
