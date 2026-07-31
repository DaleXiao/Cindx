use agent_core::{Event, EventId, EventKind, Metadata, TaskId};
use agent_memory::{
    extract_durable_memories, merge_memory_records, recall_memories_at,
    validate_semantic_memory_batch, MemoryClaimOrigin, MemoryKind, MemoryLedger, MemoryRecord,
    MemoryRequirementScope, MemoryTrust, SemanticMemoryBatch, SemanticMemoryCandidate,
    SEMANTIC_MEMORY_BATCH_SCHEMA, USER_REQUIREMENT_EVIDENCE_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

const DEFAULT_SUITE: &str = include_str!("../../../benchmarks/agent/memory-v1.json");
const EVALUATION_PROJECT_ID: &str = "memory-evaluation-project";

#[derive(Debug, Deserialize)]
struct MemoryEvaluationSuite {
    schema: String,
    id: String,
    version: u64,
    cases: Vec<MemoryEvaluationCase>,
}

#[derive(Debug, Deserialize)]
struct MemoryEvaluationCase {
    id: String,
    requirement: String,
    #[serde(default)]
    distractors: Vec<String>,
    #[serde(default)]
    replacement: Option<String>,
    query: String,
    expected: String,
    #[serde(default)]
    expected_not: Option<String>,
    #[serde(default)]
    expect_no_recall: bool,
    #[serde(default = "default_true")]
    expect_persist: bool,
    #[serde(default)]
    security: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct MemoryEvaluationCaseReport {
    id: String,
    requirement_records: usize,
    active_requirement_records: usize,
    recalled_records: usize,
    deduplication_correct: bool,
    top_1_correct: bool,
    recall_at_3_correct: bool,
    supersession_correct: bool,
    no_recall_correct: bool,
    persistence_correct: bool,
    verbatim_evidence_correct: bool,
    recall_micros: u128,
}

#[derive(Debug, Serialize)]
struct MemoryEvaluationReport {
    schema: &'static str,
    suite_id: String,
    suite_version: u64,
    cases: usize,
    top_1_correct: usize,
    recall_at_3_correct: usize,
    trust_violations: usize,
    dedup_failures: usize,
    supersession_failures: usize,
    false_positive_failures: usize,
    false_persistence_failures: usize,
    verbatim_evidence_failures: usize,
    semantic_laundering_failures: usize,
    security_cases: usize,
    security_case_failures: usize,
    average_recall_micros: u128,
    max_recall_micros: u128,
    case_results: Vec<MemoryEvaluationCaseReport>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Cindx memory evaluation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut suite_path = None;
    let mut report_path = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--suite" => suite_path = args.next().map(PathBuf::from),
            "--report" => report_path = args.next().map(PathBuf::from),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let suite_text = match suite_path {
        Some(path) => fs::read_to_string(path).map_err(|error| error.to_string())?,
        None => DEFAULT_SUITE.to_string(),
    };
    let suite: MemoryEvaluationSuite =
        serde_json::from_str(&suite_text).map_err(|error| error.to_string())?;
    if suite.schema != "cindx.memory-evaluation.v1" || suite.cases.is_empty() {
        return Err("memory suite schema is invalid or empty".to_string());
    }

    let mut report = MemoryEvaluationReport {
        schema: "cindx.memory-evaluation-report.v1",
        suite_id: suite.id,
        suite_version: suite.version,
        cases: suite.cases.len(),
        top_1_correct: 0,
        recall_at_3_correct: 0,
        trust_violations: 0,
        dedup_failures: 0,
        supersession_failures: 0,
        false_positive_failures: 0,
        false_persistence_failures: 0,
        verbatim_evidence_failures: 0,
        semantic_laundering_failures: semantic_laundering_failures(),
        security_cases: suite.cases.iter().filter(|case| case.security).count(),
        security_case_failures: 0,
        average_recall_micros: 0,
        max_recall_micros: 0,
        case_results: Vec::with_capacity(suite.cases.len()),
    };
    let mut total_recall_micros = 0u128;
    for (index, case) in suite.cases.iter().enumerate() {
        let mut ledger = MemoryLedger::new(EVALUATION_PROJECT_ID);
        let mut source_events = BTreeMap::new();
        for source_index in 0..2 {
            let session_id = format!("source-{index}-{source_index}");
            merge_completed_events(
                &mut ledger,
                completed_events(&case.requirement, &session_id, source_index as u64),
                &session_id,
                &mut source_events,
            );
        }
        if let Some(replacement) = &case.replacement {
            let session_id = format!("source-{index}-replacement");
            merge_completed_events(
                &mut ledger,
                completed_events(replacement, &session_id, 10_000 + index as u64),
                &session_id,
                &mut source_events,
            );
        }
        for (distractor_index, distractor) in case.distractors.iter().enumerate() {
            let session_id = format!("source-{index}-distractor-{distractor_index}");
            merge_completed_events(
                &mut ledger,
                completed_events(
                    distractor,
                    &session_id,
                    20_000 + index as u64 * 100 + distractor_index as u64,
                ),
                &session_id,
                &mut source_events,
            );
        }
        let requirement_count = ledger
            .records
            .iter()
            .filter(|record| record.kind == MemoryKind::Requirement)
            .count();
        let active_requirement_count = ledger
            .records
            .iter()
            .filter(|record| {
                record.kind == MemoryKind::Requirement && record.superseded_by.is_none()
            })
            .count();
        let expected_requirement_count = usize::from(case.expect_persist)
            + usize::from(case.expect_persist && case.replacement.is_some())
            + case.distractors.len();
        let expected_active_count = usize::from(case.expect_persist) + case.distractors.len();
        let deduplication_correct = requirement_count == expected_requirement_count
            && active_requirement_count == expected_active_count;
        if !deduplication_correct {
            report.dedup_failures += 1;
        }
        let persistence_correct = case.expect_persist
            || ledger
                .records
                .iter()
                .all(|record| record.content != case.requirement);
        if !persistence_correct {
            report.false_persistence_failures += 1;
        }
        let verbatim_evidence_correct = ledger.records.iter().all(|record| {
            record.kind != MemoryKind::Requirement
                || independently_verify_requirement(record, &source_events)
        });
        if !verbatim_evidence_correct {
            report.verbatim_evidence_failures += 1;
        }

        let started_at = Instant::now();
        let recalls = recall_memories_at(&ledger, &case.query, Some("query-session"), 3, 1_000);
        let elapsed = started_at.elapsed().as_micros();
        total_recall_micros = total_recall_micros.saturating_add(elapsed);
        report.max_recall_micros = report.max_recall_micros.max(elapsed);
        let no_recall_correct = !case.expect_no_recall || recalls.is_empty();
        if !no_recall_correct {
            report.false_positive_failures += 1;
        }
        let top_1_correct = if case.expect_no_recall {
            recalls.is_empty()
        } else {
            recalls
                .first()
                .is_some_and(|recall| recall.record.content.contains(&case.expected))
        };
        if top_1_correct {
            report.top_1_correct += 1;
        }
        let recall_at_3_correct = if case.expect_no_recall {
            recalls.is_empty()
        } else {
            recalls
                .iter()
                .any(|recall| recall.record.content.contains(&case.expected))
        };
        if recall_at_3_correct {
            report.recall_at_3_correct += 1;
        }
        let supersession_correct = case.expected_not.as_ref().is_none_or(|stale| {
            ledger
                .records
                .iter()
                .any(|record| record.content.contains(stale) && record.superseded_by.is_some())
                && recalls
                    .iter()
                    .all(|recall| !recall.record.content.contains(stale))
        });
        if !supersession_correct {
            report.supersession_failures += 1;
        }
        if case.security
            && (!deduplication_correct
                || !top_1_correct
                || !recall_at_3_correct
                || !no_recall_correct
                || !persistence_correct
                || !verbatim_evidence_correct)
        {
            report.security_case_failures += 1;
        }
        report.trust_violations += recalls
            .iter()
            .filter(|recall| recall.record.trust != MemoryTrust::UserStated)
            .count();
        report.case_results.push(MemoryEvaluationCaseReport {
            id: case.id.clone(),
            requirement_records: requirement_count,
            active_requirement_records: active_requirement_count,
            recalled_records: recalls.len(),
            deduplication_correct,
            top_1_correct,
            recall_at_3_correct,
            supersession_correct,
            no_recall_correct,
            persistence_correct,
            verbatim_evidence_correct,
            recall_micros: elapsed,
        });
    }
    report.average_recall_micros = total_recall_micros / report.cases.max(1) as u128;
    let report_text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(path) = report_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&path, format!("{report_text}\n")).map_err(|error| error.to_string())?;
        println!("Report: {}", path.display());
    }
    println!(
        "Cindx memory benchmark {}-v{}: top1 {}/{} recall@3 {}/{} trust_violations={} dedup_failures={} supersession_failures={} false_positive_failures={} false_persistence_failures={} verbatim_evidence_failures={} semantic_laundering_failures={} security_cases={} security_case_failures={} avg={}us max={}us",
        report.suite_id,
        report.suite_version,
        report.top_1_correct,
        report.cases,
        report.recall_at_3_correct,
        report.cases,
        report.trust_violations,
        report.dedup_failures,
        report.supersession_failures,
        report.false_positive_failures,
        report.false_persistence_failures,
        report.verbatim_evidence_failures,
        report.semantic_laundering_failures,
        report.security_cases,
        report.security_case_failures,
        report.average_recall_micros,
        report.max_recall_micros,
    );
    for case in report.case_results.iter().filter(|case| {
        !case.deduplication_correct
            || !case.top_1_correct
            || !case.recall_at_3_correct
            || !case.supersession_correct
            || !case.no_recall_correct
            || !case.persistence_correct
            || !case.verbatim_evidence_correct
    }) {
        println!(
            "  failed {}: requirements={} active={} recalls={} top1={} recall@3={} supersession={} no_recall={} persistence={} verbatim_evidence={}",
            case.id,
            case.requirement_records,
            case.active_requirement_records,
            case.recalled_records,
            case.top_1_correct,
            case.recall_at_3_correct,
            case.supersession_correct,
            case.no_recall_correct,
            case.persistence_correct,
            case.verbatim_evidence_correct,
        );
    }
    if report.top_1_correct != report.cases
        || report.recall_at_3_correct != report.cases
        || report.trust_violations != 0
        || report.dedup_failures != 0
        || report.supersession_failures != 0
        || report.false_positive_failures != 0
        || report.false_persistence_failures != 0
        || report.verbatim_evidence_failures != 0
        || report.semantic_laundering_failures != 0
        || report.security_cases == 0
        || report.security_case_failures != 0
    {
        return Err("memory evaluation gate did not pass".to_string());
    }
    Ok(())
}

fn merge_completed_events(
    ledger: &mut MemoryLedger,
    events: Vec<Event>,
    session_id: &str,
    source_events: &mut BTreeMap<String, Event>,
) {
    source_events.extend(
        events
            .iter()
            .filter(|event| event.kind == EventKind::MessageAdded)
            .map(|event| (event.id.0.clone(), event.clone())),
    );
    merge_memory_records(
        ledger,
        extract_durable_memories(&events, EVALUATION_PROJECT_ID, session_id),
        64,
    );
}

fn independently_verify_requirement(
    record: &MemoryRecord,
    source_events: &BTreeMap<String, Event>,
) -> bool {
    record.user_requirement_evidence.iter().any(|evidence| {
        let Some(source_event) = source_events.get(&evidence.event_id) else {
            return false;
        };
        let Some(source) = source_event.metadata.get("content") else {
            return false;
        };
        let Some(session_id) = source_event.metadata.get("session_id") else {
            return false;
        };
        let Some((start, end)) = usize::try_from(evidence.quote_start_byte)
            .ok()
            .zip(usize::try_from(evidence.quote_end_byte).ok())
        else {
            return false;
        };
        evidence.schema == USER_REQUIREMENT_EVIDENCE_SCHEMA
            && evidence.origin == MemoryClaimOrigin::UserVerbatim
            && evidence.scope == MemoryRequirementScope::ProjectDurable
            && evidence.project_id == EVALUATION_PROJECT_ID
            && evidence.session_id == *session_id
            && evidence.event_id == record.provenance.event_id
            && evidence.session_id == record.provenance.session_id
            && source_event.kind == EventKind::MessageAdded
            && source_event.metadata.get("role").map(String::as_str) == Some("user")
            && source_event.metadata.get("project_id").map(String::as_str)
                == Some(EVALUATION_PROJECT_ID)
            && evidence.source_sha256 == sha256_hex(source.as_bytes())
            && start < end
            && source.is_char_boundary(start)
            && source.is_char_boundary(end)
            && source.get(start..end) == Some(record.content.as_str())
            && evidence.evidence_sha256
                == independent_evidence_sha256(
                    &evidence.schema,
                    &evidence.project_id,
                    &evidence.session_id,
                    &evidence.event_id,
                    &evidence.source_sha256,
                    evidence.quote_start_byte,
                    evidence.quote_end_byte,
                    &record.content,
                )
    })
}

#[allow(clippy::too_many_arguments)]
fn independent_evidence_sha256(
    schema: &str,
    project_id: &str,
    session_id: &str,
    event_id: &str,
    source_sha256: &str,
    quote_start_byte: u64,
    quote_end_byte: u64,
    quote: &str,
) -> String {
    let mut hasher = Sha256::new();
    for field in [
        schema.as_bytes(),
        project_id.as_bytes(),
        session_id.as_bytes(),
        event_id.as_bytes(),
        source_sha256.as_bytes(),
        &quote_start_byte.to_be_bytes(),
        &quote_end_byte.to_be_bytes(),
        quote.as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    format!("{:x}", hasher.finalize())
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn semantic_laundering_failures() -> usize {
    [
        (
            "Always keep builds local",
            "Builds must remain local",
            "semantic-paraphrase",
        ),
        (
            "For this task, always avoid building the app",
            "For this task, always avoid building the app",
            "semantic-task-local",
        ),
    ]
    .into_iter()
    .filter(|(source, candidate, session_id)| {
        let events = completed_events(source, session_id, 30_000);
        !validate_semantic_memory_batch(
            SemanticMemoryBatch {
                schema: SEMANTIC_MEMORY_BATCH_SCHEMA.to_string(),
                candidates: vec![SemanticMemoryCandidate {
                    kind: MemoryKind::Requirement,
                    content: (*candidate).to_string(),
                    importance: 100,
                    source_event_ids: vec![format!("event-user-{session_id}")],
                }],
            },
            &events,
            EVALUATION_PROJECT_ID,
            session_id,
        )
        .accepted
        .is_empty()
    })
    .count()
}

fn completed_events(requirement: &str, session_id: &str, offset: u64) -> Vec<Event> {
    let run_id = format!("run-{session_id}");
    let metadata = |role: Option<&str>, content: Option<&str>| {
        let mut metadata = [
            ("project_id".to_string(), EVALUATION_PROJECT_ID.to_string()),
            ("session_id".to_string(), session_id.to_string()),
            ("agent_run_id".to_string(), run_id.clone()),
        ]
        .into_iter()
        .collect::<Metadata>();
        if let Some(role) = role {
            metadata.insert("role".to_string(), role.to_string());
        }
        if let Some(content) = content {
            metadata.insert("content".to_string(), content.to_string());
        }
        metadata
    };
    vec![
        Event {
            id: EventId(format!("event-user-{session_id}")),
            task_id: TaskId("phase-16-agent-loop".to_string()),
            sequence: offset * 10 + 1,
            timestamp_ms: offset * 10 + 1,
            kind: EventKind::MessageAdded,
            summary: "User message".to_string(),
            metadata: metadata(Some("user"), Some(requirement)),
        },
        Event {
            id: EventId(format!("event-complete-{session_id}")),
            task_id: TaskId("phase-16-agent-loop".to_string()),
            sequence: offset * 10 + 2,
            timestamp_ms: offset * 10 + 2,
            kind: EventKind::TaskStatusChanged,
            summary: "Agent task completed".to_string(),
            metadata: metadata(None, None),
        },
    ]
}
