use crate::{stable_hash, ToolError};

use super::file_list_model::{ListCursor, ListEntry, ListFailure};

pub(super) fn cursor_scope(path: &str, glob: Option<&str>) -> u64 {
    stable_hash(&format!(
        "cindx.file-list-cursor.v2\0{path}\0{}",
        glob.unwrap_or_default()
    ))
}

pub(super) fn encode_cursor(scope: u64, snapshot: u64, offset: usize) -> String {
    format!("v2:{scope:016x}:{snapshot:016x}:{offset}")
}

pub(super) fn decode_cursor(cursor: &str, expected_scope: u64) -> Result<ListCursor, ToolError> {
    let mut fields = cursor.split(':');
    let version = fields.next();
    let scope = fields.next();
    let snapshot = fields.next();
    let offset = fields.next();
    if version != Some("v2") || fields.next().is_some() {
        return Err(ToolError::new("invalid file.list cursor"));
    }
    let scope = scope
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(|| ToolError::new("invalid file.list cursor"))?;
    if scope != expected_scope {
        return Err(ToolError::new(
            "file.list cursor does not match the requested path and glob",
        ));
    }
    let snapshot = snapshot
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(|| ToolError::new("invalid file.list cursor"))?;
    let offset = offset
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| ToolError::new("invalid file.list cursor"))?;
    Ok(ListCursor { snapshot, offset })
}

pub(super) fn list_snapshot_hash(
    entries: &[&ListEntry],
    failures: &[ListFailure],
    cancelled: bool,
    discovery_limit_reached: bool,
) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    hash_part(
        &mut hash,
        if cancelled {
            b"cancelled"
        } else if discovery_limit_reached {
            b"discovery-limited"
        } else {
            b"complete"
        },
    );
    for entry in entries {
        hash_part(&mut hash, b"entry");
        hash_part(&mut hash, entry.name.as_bytes());
        hash_part(&mut hash, entry.kind.as_bytes());
        hash_part(&mut hash, &entry.bytes.to_le_bytes());
    }
    for failure in failures {
        hash_part(&mut hash, b"failure");
        hash_part(
            &mut hash,
            failure.name.as_deref().unwrap_or_default().as_bytes(),
        );
        hash_part(&mut hash, failure.message.as_bytes());
    }
    hash
}

fn hash_part(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.iter().copied().chain(std::iter::once(0xff)) {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(1_099_511_628_211);
    }
}
