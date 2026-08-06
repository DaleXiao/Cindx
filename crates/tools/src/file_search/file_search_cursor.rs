use std::fs;
use std::path::Path;

use super::file_search_types::{CandidateFile, SearchPosition, SEARCH_FILE_SCAN_MAX_BYTES};
use crate::file_query_contract_v3::{
    invalid_search_cursor, SearchCoverage, SearchCursor, SearchOptions,
};
use crate::{stable_hash, ToolError};

pub(super) fn option_binding_hash(workspace_root: &Path, options: &SearchOptions) -> u64 {
    stable_hash(
        &serde_json::json!({
            "schema": "cindx.file-search-options.v2",
            "workspace": fs::canonicalize(workspace_root)
                .unwrap_or_else(|_| workspace_root.to_path_buf())
                .to_string_lossy(),
            "query": options.query,
            "path": options.path,
            "regex": options.regex,
            "case_sensitive": options.case_sensitive,
            "glob": options.glob,
            "context_lines": options.context_lines,
            "max_results": options.max_results,
        })
        .to_string(),
    )
}

pub(super) fn candidate_snapshot_hash(
    candidates: &[CandidateFile],
    coverage: &SearchCoverage,
) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for candidate in candidates {
        update_hash(&mut hash, candidate.relative.as_bytes());
        update_hash(&mut hash, &candidate.size.to_le_bytes());
        update_hash(&mut hash, &candidate.modified_nanos.to_le_bytes());
    }
    update_hash(&mut hash, &[u8::from(coverage.discovery_limit_reached)]);
    hash
}

pub(super) fn cursor_start(
    candidates: &[CandidateFile],
    cursor: Option<&SearchCursor>,
) -> Result<(usize, SearchPosition), ToolError> {
    let Some(cursor) = cursor else {
        return Ok((0, SearchPosition::default()));
    };
    let index = candidates
        .iter()
        .position(|candidate| candidate.relative == cursor.next_path)
        .ok_or_else(|| invalid_search_cursor("cursor target is no longer searchable"))?;
    if cursor.next_byte as u64 > candidates[index].size.min(SEARCH_FILE_SCAN_MAX_BYTES) {
        return Err(invalid_search_cursor(
            "cursor byte offset is outside the searchable file prefix",
        ));
    }
    Ok((
        index,
        SearchPosition {
            line: cursor.next_line,
            byte: cursor.next_byte,
        },
    ))
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(1_099_511_628_211);
    }
}
