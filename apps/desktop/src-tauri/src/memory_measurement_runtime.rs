use crate::desktop_prelude::*;
use crate::runtime_constants::AGENT_MEMORY_RECALL_LIMIT;

pub(crate) fn replay_project_memory_measurement(
    ledger: &mut MemoryLedger,
    event: &Event,
    project_id: &str,
) {
    if event.kind != EventKind::RetrievalPerformed
        || event.metadata.get("project_id").map(String::as_str) != Some(project_id)
    {
        return;
    }
    match event.summary.as_str() {
        "Project memory recalled"
            if event.metadata.get("action").map(String::as_str) == Some("memory_recall") =>
        {
            let Some(memory_ids) = strict_memory_ids(event, "memory_ids", false) else {
                return;
            };
            let Some(selected_count) = strict_memory_count(event, "selected_count") else {
                return;
            };
            let Some(recalled_at_ms) = measurement_timestamp(event, "recalled_at_ms") else {
                return;
            };
            if selected_count != memory_ids.len()
                || memory_ids.iter().any(|id| {
                    !ledger
                        .records
                        .iter()
                        .any(|record| record.id.as_str() == *id)
                })
            {
                return;
            }
            let recalled = memory_ids.into_iter().collect::<BTreeSet<_>>();
            for record in &mut ledger.records {
                if recalled.contains(record.id.as_str()) {
                    record.recall_count = record.recall_count.saturating_add(1);
                    record.last_recalled_at_ms = Some(recalled_at_ms);
                }
            }
        }
        "Project memory utilization measured"
            if event.metadata.get("action").map(String::as_str) == Some("memory_use") =>
        {
            let Some(selected_ids) = strict_memory_ids(event, "memory_ids", false) else {
                return;
            };
            let Some(used_ids) = strict_memory_ids(event, "used_memory_ids", true) else {
                return;
            };
            let (Some(selected_count), Some(used_count)) = (
                strict_memory_count(event, "selected_count"),
                strict_memory_count(event, "used_count"),
            ) else {
                return;
            };
            let Some(observed_at_ms) = measurement_timestamp(event, "observed_at_ms") else {
                return;
            };
            let selected = selected_ids.into_iter().collect::<BTreeSet<_>>();
            if selected_count != selected.len()
                || used_count != used_ids.len()
                || used_ids.iter().any(|id| !selected.contains(*id))
                || selected.iter().any(|id| {
                    !ledger
                        .records
                        .iter()
                        .any(|record| record.id.as_str() == *id)
                })
            {
                return;
            }
            let used = used_ids.into_iter().collect::<BTreeSet<_>>();
            for record in &mut ledger.records {
                if used.contains(record.id.as_str()) {
                    record.observed_use_count = record.observed_use_count.saturating_add(1);
                    record.last_observed_use_at_ms = Some(observed_at_ms);
                }
            }
        }
        _ => {}
    }
}

fn strict_memory_ids<'a>(event: &'a Event, key: &str, allow_empty: bool) -> Option<Vec<&'a str>> {
    let encoded = event.metadata.get(key)?;
    if encoded.is_empty() {
        return allow_empty.then(Vec::new);
    }
    let mut unique = BTreeSet::new();
    let mut ids = Vec::new();
    for id in encoded.split(',') {
        if id.is_empty()
            || id.trim() != id
            || id.len() > 256
            || !unique.insert(id)
            || ids.len() >= AGENT_MEMORY_RECALL_LIMIT
        {
            return None;
        }
        ids.push(id);
    }
    Some(ids)
}

fn strict_memory_count(event: &Event, key: &str) -> Option<usize> {
    event.metadata.get(key)?.parse::<usize>().ok()
}

fn measurement_timestamp(event: &Event, key: &str) -> Option<u64> {
    match event.metadata.get(key) {
        Some(timestamp) => timestamp.parse::<u64>().ok(),
        None => Some(event.timestamp_ms),
    }
}
