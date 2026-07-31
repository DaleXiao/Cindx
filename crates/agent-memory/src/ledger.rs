use crate::memory_text::memory_terms;
use crate::{MemoryKind, MemoryLedger, MemoryMergeStats, MemoryRecord, MemoryTrust};

pub fn merge_memory_records(
    ledger: &mut MemoryLedger,
    candidates: impl IntoIterator<Item = MemoryRecord>,
    max_records: usize,
) -> MemoryMergeStats {
    let mut stats = MemoryMergeStats::default();
    for mut candidate in candidates {
        if candidate.provenance.project_id != ledger.project_id || !candidate.is_recall_eligible() {
            continue;
        }
        if let Some(existing_index) = ledger
            .records
            .iter()
            .position(|record| record.fingerprint == candidate.fingerprint)
        {
            let changed;
            {
                let existing = &mut ledger.records[existing_index];
                let before = existing.clone();
                let candidate_wins = record_order(&candidate) > record_order(existing)
                    || (!existing.is_recall_eligible() && candidate.is_recall_eligible());
                if candidate_wins {
                    existing.provenance = candidate.provenance.clone();
                    existing.trust = candidate.trust;
                    existing.content = candidate.content.clone();
                    existing.superseded_by = None;
                    existing.superseded_at_ms = None;
                }
                merge_unique_bounded(
                    &mut existing.source_event_ids,
                    &candidate.source_event_ids,
                    candidate_wins,
                    8,
                );
                merge_unique_bounded(
                    &mut existing.source_session_ids,
                    &candidate.source_session_ids,
                    candidate_wins,
                    8,
                );
                merge_unique_bounded(
                    &mut existing.user_requirement_evidence,
                    &candidate.user_requirement_evidence,
                    candidate_wins,
                    8,
                );
                existing.updated_at_ms = existing.updated_at_ms.max(candidate.updated_at_ms);
                existing.importance = existing.importance.max(candidate.importance);
                changed = *existing != before;
            }
            let active = ledger.records[existing_index].clone();
            let reconciled = reconcile_requirement_conflicts(
                &mut ledger.records,
                &mut candidate,
                Some(existing_index),
                &active,
            );
            stats.updated += usize::from(changed) + reconciled;
            continue;
        }

        let active = candidate.clone();
        stats.updated +=
            reconcile_requirement_conflicts(&mut ledger.records, &mut candidate, None, &active);
        ledger.records.push(candidate);
        stats.inserted += 1;
    }

    ledger.records.sort_by(|left, right| {
        left.superseded_by
            .is_some()
            .cmp(&right.superseded_by.is_some())
            .then_with(|| right.importance.cmp(&left.importance))
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| right.recall_count.cmp(&left.recall_count))
            .then_with(|| left.id.cmp(&right.id))
    });
    let limit = max_records.max(1);
    if ledger.records.len() > limit {
        stats.evicted = ledger.records.len() - limit;
        ledger.records.truncate(limit);
    }
    stats
}

fn merge_unique_bounded<T: Clone + PartialEq>(
    existing: &mut Vec<T>,
    incoming: &[T],
    incoming_first: bool,
    limit: usize,
) {
    let mut merged = Vec::with_capacity(limit);
    let mut append = |values: &[T]| {
        for value in values {
            if merged.len() >= limit {
                break;
            }
            if !merged.contains(value) {
                merged.push(value.clone());
            }
        }
    };
    if incoming_first {
        append(incoming);
        append(existing);
    } else {
        append(existing);
        append(incoming);
    }
    *existing = merged;
}

fn reconcile_requirement_conflicts(
    records: &mut [MemoryRecord],
    incoming: &mut MemoryRecord,
    incoming_index: Option<usize>,
    active: &MemoryRecord,
) -> usize {
    if !active.has_verified_user_requirement() {
        return 0;
    }

    let mut newer_conflict: Option<&MemoryRecord> = None;
    for (index, record) in records.iter().enumerate() {
        if Some(index) == incoming_index
            || record.kind != MemoryKind::Requirement
            || record.trust != MemoryTrust::UserStated
            || !record.has_verified_user_requirement()
            || record.provenance.project_id != active.provenance.project_id
            || !requirements_conflict(&record.content, &active.content)
        {
            continue;
        }
        if record_order(record) > record_order(active)
            && newer_conflict
                .as_ref()
                .is_none_or(|current| record_order(record) > record_order(current))
        {
            newer_conflict = Some(record);
        }
    }

    if let Some(newer) = newer_conflict {
        incoming.superseded_by = Some(newer.id.clone());
        incoming.superseded_at_ms = Some(newer.updated_at_ms);
        return 0;
    }

    let mut changed = 0;
    for (index, record) in records.iter_mut().enumerate() {
        if Some(index) == incoming_index
            || record.kind != MemoryKind::Requirement
            || record.trust != MemoryTrust::UserStated
            || !record.has_verified_user_requirement()
            || record.provenance.project_id != active.provenance.project_id
            || record_order(record) > record_order(active)
            || !requirements_conflict(&record.content, &active.content)
        {
            continue;
        }
        if record.superseded_by.as_deref() != Some(active.id.as_str())
            || record.superseded_at_ms != Some(active.updated_at_ms)
        {
            record.superseded_by = Some(active.id.clone());
            record.superseded_at_ms = Some(active.updated_at_ms);
            changed += 1;
        }
    }
    changed
}

fn record_order(record: &MemoryRecord) -> (u64, u64, &str) {
    (
        record.provenance.timestamp_ms,
        record.provenance.sequence,
        record.id.as_str(),
    )
}

fn requirements_conflict(left: &str, right: &str) -> bool {
    match (
        explicit_requirement_scope(left),
        explicit_requirement_scope(right),
    ) {
        (Some(left_scope), Some(right_scope)) if left_scope == right_scope => return true,
        _ => {}
    }

    let left_negative = has_negative_directive(left);
    let right_negative = has_negative_directive(right);
    if left_negative == right_negative || !shares_negatable_action(left, right) {
        return false;
    }
    let left_terms = memory_terms(left);
    let right_terms = memory_terms(right);
    let minimum = left_terms.len().min(right_terms.len());
    minimum >= 2 && left_terms.intersection(&right_terms).count() * 100 >= minimum * 70
}

fn explicit_requirement_scope(content: &str) -> Option<&'static str> {
    let lower = content.to_lowercase();
    if [
        "call me",
        "my name is",
        "address me as",
        "叫我",
        "称呼我",
        "我的名字是",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return Some("personalization:user_name");
    }
    if ["response tone", "reply tone", "语气", "口吻"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return Some("personalization:response_tone");
    }
    if [
        "response length",
        "reply length",
        "concise response",
        "detailed response",
        "回复长度",
        "回答长度",
        "简洁回复",
        "详细回复",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return Some("personalization:response_length");
    }
    if [
        "dark mode",
        "light mode",
        "system theme",
        "夜间模式",
        "白天模式",
        "跟随系统",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return Some("personalization:appearance_theme");
    }
    None
}

fn has_negative_directive(content: &str) -> bool {
    let lower = content.to_lowercase();
    [
        "do not ",
        "don't ",
        "never ",
        "must not ",
        "不要",
        "不能",
        "不允许",
        "禁止",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn shares_negatable_action(left: &str, right: &str) -> bool {
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    [
        "enable", "use", "show", "include", "open", "allow", "change", "move", "remove", "delete",
        "开启", "使用", "显示", "包含", "打开", "允许", "修改", "移动", "删除",
    ]
    .iter()
    .any(|action| left.contains(action) && right.contains(action))
}

#[cfg(test)]
mod tests {
    use super::requirements_conflict;

    #[test]
    fn supersession_is_scoped_and_conservative() {
        assert!(requirements_conflict("Call me Dale", "Call me Alex"));
        assert!(requirements_conflict(
            "Always enable dark mode",
            "Do not enable dark mode"
        ));
        assert!(!requirements_conflict(
            "Never move the traffic lights",
            "Always keep the traffic lights aligned"
        ));
        assert!(!requirements_conflict(
            "Keep the sidebar white",
            "Keep the titlebar white"
        ));
    }
}
