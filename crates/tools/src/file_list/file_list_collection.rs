use std::{fs, io};

use super::file_list_model::{ListEntry, ListFailure, ListSnapshot};

pub(super) fn collect_directory_entries(
    entries: fs::ReadDir,
    glob: Option<&str>,
    discovery_limit: usize,
    should_cancel: &dyn Fn() -> bool,
    inspect: &dyn Fn(&fs::DirEntry) -> io::Result<(String, u64)>,
) -> ListSnapshot {
    let mut snapshot = ListSnapshot {
        discovery_limit,
        ..ListSnapshot::default()
    };
    for entry in entries {
        if should_cancel() {
            snapshot.cancelled = true;
            break;
        }
        if snapshot.discovered >= discovery_limit {
            snapshot.discovery_limit_reached = true;
            break;
        }
        snapshot.discovered += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                snapshot.failures.push(ListFailure {
                    name: None,
                    message: format!("failed to read directory entry: {error}"),
                });
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if glob.is_some_and(|pattern| !glob_matches(pattern, &name)) {
            continue;
        }
        match inspect(&entry) {
            Ok((kind, bytes)) => snapshot.entries.push(ListEntry { kind, bytes, name }),
            Err(error) => snapshot.failures.push(ListFailure {
                name: Some(name),
                message: format!("failed to read metadata: {error}"),
            }),
        }
    }
    snapshot
        .entries
        .sort_by(|left, right| left.name.cmp(&right.name).then(left.kind.cmp(&right.kind)));
    snapshot.failures.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.message.cmp(&right.message))
    });
    snapshot
}

pub(super) fn inspect_entry(entry: &fs::DirEntry) -> io::Result<(String, u64)> {
    let metadata = entry.metadata()?;
    Ok((
        if metadata.is_dir() { "dir" } else { "file" }.to_string(),
        metadata.len(),
    ))
}

pub(super) fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        if token == '*' {
            current[0] = previous[0];
        }
        for index in 1..=value.len() {
            current[index] = match token {
                '*' => previous[index] || current[index - 1],
                '?' => previous[index - 1],
                literal => previous[index - 1] && literal == value[index - 1],
            };
        }
        previous = current;
    }
    previous[value.len()]
}
