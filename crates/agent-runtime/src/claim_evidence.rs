//! Deterministic citation-to-evidence binding for a delivered answer (P1-10).
//!
//! The grounding spine is structural: it proves a required tool ran, a mutation
//! was verified, and the evidence anchors a prompt demanded were observed. It
//! never checked what the ANSWER cites — citations were an instruction ("cite
//! findings as `path:line`") with no verification behind them, and the outcome
//! ledger records a delivered claim as
//! `OutcomeClaimEvidenceStatus::AvailableNotEntailed` exactly because that
//! relation was uncomputed.
//!
//! This module closes the location-level half of that gap with no provider call:
//! it parses the answer's workspace citations, binds each one to the locations
//! the run actually observed (stamped on the durable tool message at dispatch),
//! and returns a typed receipt with three dispositions — supported, unsupported
//! (no observed file-tool call covered that path), and contradicted (the run did
//! try to observe that path and the call did not succeed, so its contents were
//! never available).
//!
//! It is location-level binding, NOT natural-language entailment: a citation can
//! bind to real evidence while the prose around it is still wrong. Content-level
//! entailment stays open, and the receipt is additive judge evidence — nothing
//! here blocks delivery on its own.

use crate::evidence_target::{token_workspace_path_and_line, tokens, workspace_targets_match};
use agent_core::{Message, MessageRole, Metadata};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const CLAIM_EVIDENCE_SCHEMA: &str = "cindx.agent.claim-evidence.v1";
pub const OBSERVED_LOCATIONS_SCHEMA: &str = "cindx.tool-observed-locations.v1";

const OBSERVED_LOCATIONS_SCHEMA_KEY: &str = "tool_observed_locations_schema";
const OBSERVED_LOCATIONS_KEY: &str = "tool_observed_locations";
const MAX_CITATIONS: usize = 64;
const MAX_FINDINGS: usize = 8;
const MAX_OBSERVED_LOCATIONS: usize = 512;
const MAX_OBSERVED_LOCATIONS_PER_CALL: usize = 8;
const MAX_PATH_CHARS: usize = 512;
const MAX_REASON_CHARS: usize = 160;

/// The file tools whose arguments name workspace locations the run really
/// observed. Everything else is out of scope, so an unmatched citation is
/// reported as unsupported and the facts line names the covered tools: a reader
/// can then tell a gap in coverage from a gap in evidence.
const OBSERVED_LOCATION_TOOLS: &[(&str, ObservedLocationKind)] = &[
    ("file.read", ObservedLocationKind::Read),
    ("file.read_many", ObservedLocationKind::Read),
    ("file.search", ObservedLocationKind::Search),
    ("file.list", ObservedLocationKind::Search),
    ("file.glob", ObservedLocationKind::Search),
    ("file.write", ObservedLocationKind::Mutation),
    ("file.patch", ObservedLocationKind::Mutation),
    ("file.patch_batch", ObservedLocationKind::Mutation),
];

/// How a location became observed, which decides how strongly it can support a
/// citation: a read or a mutation puts the file's own content in the run's
/// evidence, while a search, listing, or glob only covers a subtree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedLocationKind {
    Read,
    Search,
    Mutation,
}

impl ObservedLocationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Search => "search",
            Self::Mutation => "mutation",
        }
    }

    /// Only a directory-scoped observation can support a citation to a file
    /// beneath it; a read or mutation names one exact file.
    fn covers_subtree(self) -> bool {
        self == Self::Search
    }
}

/// One workspace location a tool call actually observed, plus whether that call
/// succeeded. A failed or denied read is still recorded: it is the evidence that
/// contradicts a citation to that path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedLocation {
    pub path: String,
    pub kind: ObservedLocationKind,
    pub succeeded: bool,
}

/// A workspace citation parsed out of a delivered answer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerCitation {
    pub path: String,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
}

impl AnswerCitation {
    /// The citation as the answer wrote it, for bounded display in findings.
    pub fn display(&self) -> String {
        match (self.start_line, self.end_line) {
            (Some(start), Some(end)) => format!("{}:{start}-{end}", self.path),
            (Some(start), None) => format!("{}:{start}", self.path),
            _ => self.path.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationStatus {
    Supported,
    Unsupported,
    Contradicted,
}

impl CitationStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Contradicted => "contradicted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationFinding {
    pub citation: AnswerCitation,
    pub status: CitationStatus,
    pub reason: String,
}

/// The typed result of binding one answer's citations to the run's observed
/// locations. `findings` holds the problems (contradicted first, then
/// unsupported) up to [`MAX_FINDINGS`]; supported citations are counted only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimEvidenceReceipt {
    pub schema: String,
    pub answer_sha256: String,
    pub citations_checked: usize,
    pub supported: usize,
    pub unsupported: usize,
    pub contradicted: usize,
    pub observed_locations: usize,
    pub findings_truncated: bool,
    pub findings: Vec<CitationFinding>,
    pub receipt_sha256: String,
}

impl ClaimEvidenceReceipt {
    /// True when every parsed citation bound to evidence the run really has.
    pub fn fully_supported(&self) -> bool {
        self.citations_checked > 0 && self.unsupported == 0 && self.contradicted == 0
    }
}

/// The locations one tool call observed, derived from its arguments. Only the
/// whitelisted file tools contribute, at most [`MAX_OBSERVED_LOCATIONS_PER_CALL`]
/// locations each, so a hostile or huge argument blob cannot inflate the record.
pub fn observed_locations_for_call(
    tool_name: &str,
    input: &str,
    succeeded: bool,
) -> Vec<ObservedLocation> {
    let Some(kind) = OBSERVED_LOCATION_TOOLS
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, kind)| *kind)
    else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input) else {
        return Vec::new();
    };

    let mut paths = BTreeSet::new();
    if kind == ObservedLocationKind::Search && tool_name == "file.glob" {
        // A glob names a pattern, not a location: the literal prefix before the
        // first wildcard is the subtree the run actually enumerated.
        if let Some(pattern) = value.get("pattern").and_then(|pattern| pattern.as_str()) {
            paths.insert(glob_pattern_root(pattern));
        }
    } else {
        collect_path_values(&value, &mut paths);
    }

    paths
        .into_iter()
        .take(MAX_OBSERVED_LOCATIONS_PER_CALL)
        .map(|path| ObservedLocation {
            path: bounded_path(&path),
            kind,
            succeeded,
        })
        .collect()
}

/// Stamps a tool message with the locations that call observed. Finalization
/// reads them back through [`observed_locations_from_messages`], so the binding
/// works from the durable record instead of re-parsing observations.
pub fn insert_observed_locations(metadata: &mut Metadata, locations: &[ObservedLocation]) {
    if locations.is_empty() {
        return;
    }
    let Ok(encoded) = serde_json::to_string(locations) else {
        return;
    };
    metadata.insert(
        OBSERVED_LOCATIONS_SCHEMA_KEY.to_string(),
        OBSERVED_LOCATIONS_SCHEMA.to_string(),
    );
    metadata.insert(OBSERVED_LOCATIONS_KEY.to_string(), encoded);
}

/// The internal System-message kind the delegation join stamps with the
/// child's observed locations. A child's own tool messages live and die in
/// its isolated scope; without this carrier, files only a subagent read
/// would bind as unsupported citations and the delivery judge would be told
/// false "never read" facts about work the child actually did.
pub const SUBAGENT_RESULT_MESSAGE_KIND: &str = "subagent_result";

/// Recovers every stamped location from a run's messages, deduplicated and
/// bounded. Only messages this runtime stamped are read; a message without the
/// schema contributes nothing. Carriers are owner-dispatched Tool messages and
/// the delegation join's `subagent_result` System message — both stamped
/// exclusively by this runtime, never by model output.
pub fn observed_locations_from_messages(messages: &[Message]) -> Vec<ObservedLocation> {
    let mut recovered = BTreeSet::new();
    for message in messages {
        let is_delegation_carrier = message.role == MessageRole::System
            && message.metadata.get("kind").map(String::as_str)
                == Some(SUBAGENT_RESULT_MESSAGE_KIND);
        if message.role != MessageRole::Tool && !is_delegation_carrier {
            continue;
        }
        if message
            .metadata
            .get(OBSERVED_LOCATIONS_SCHEMA_KEY)
            .is_none_or(|schema| schema != OBSERVED_LOCATIONS_SCHEMA)
        {
            continue;
        }
        let Some(encoded) = message.metadata.get(OBSERVED_LOCATIONS_KEY) else {
            continue;
        };
        let Ok(locations) = serde_json::from_str::<Vec<ObservedLocation>>(encoded) else {
            continue;
        };
        recovered.extend(locations);
        if recovered.len() >= MAX_OBSERVED_LOCATIONS {
            break;
        }
    }
    recovered.into_iter().take(MAX_OBSERVED_LOCATIONS).collect()
}

/// The distinct workspace citations an answer makes, bounded to
/// [`MAX_CITATIONS`]. Parsing is shared with the prompt-evidence anchors, so the
/// two can never disagree about what counts as a path.
pub fn answer_citations(answer: &str) -> Vec<AnswerCitation> {
    let mut citations = BTreeSet::new();
    for token in tokens(answer) {
        if let Some((path, start_line, end_line)) = token_workspace_path_and_line(token) {
            citations.insert(AnswerCitation {
                path,
                start_line,
                end_line,
            });
        }
        if citations.len() == MAX_CITATIONS {
            break;
        }
    }
    citations.into_iter().collect()
}

/// Binds an answer's citations to the locations the run observed and returns the
/// typed receipt. Deterministic and provider-free.
pub fn bind_answer_citations(answer: &str, observed: &[ObservedLocation]) -> ClaimEvidenceReceipt {
    let citations = answer_citations(answer);
    let mut findings = citations
        .iter()
        .map(|citation| bind_one_citation(citation, observed))
        .collect::<Vec<_>>();
    let supported = findings
        .iter()
        .filter(|finding| finding.status == CitationStatus::Supported)
        .count();
    let contradicted = findings
        .iter()
        .filter(|finding| finding.status == CitationStatus::Contradicted)
        .count();
    let unsupported = findings.len() - supported - contradicted;

    // Problems first, hardest first: a contradiction outranks a gap.
    findings.retain(|finding| finding.status != CitationStatus::Supported);
    findings.sort_by(|left, right| {
        right
            .status
            .is_contradicted()
            .cmp(&left.status.is_contradicted())
            .then_with(|| left.citation.path.cmp(&right.citation.path))
    });
    let findings_truncated = findings.len() > MAX_FINDINGS;
    findings.truncate(MAX_FINDINGS);

    let answer_sha256 = sha256_hex(answer.as_bytes());
    let mut receipt = ClaimEvidenceReceipt {
        schema: CLAIM_EVIDENCE_SCHEMA.to_string(),
        answer_sha256,
        citations_checked: citations.len(),
        supported,
        unsupported,
        contradicted,
        observed_locations: observed.len(),
        findings_truncated,
        findings,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt_digest(&receipt);
    receipt
}

/// The bounded facts block appended to the delivery judge's trusted execution
/// record. Empty when the answer made no workspace citation, so an answer
/// without citations pays nothing and the judge prompt is unchanged.
pub fn claim_evidence_facts(receipt: &ClaimEvidenceReceipt) -> String {
    if receipt.citations_checked == 0 {
        return String::new();
    }
    let mut facts = format!(
        "- Answer citations: {} parsed, {} supported, {} unsupported, {} contradicted (bound to {} observed file-tool locations; location-level binding, not content entailment)",
        receipt.citations_checked,
        receipt.supported,
        receipt.unsupported,
        receipt.contradicted,
        receipt.observed_locations
    );
    for finding in &receipt.findings {
        facts.push_str(&format!(
            "\n  - {} `{}`: {}",
            finding.status.label(),
            finding.citation.display(),
            finding.reason
        ));
    }
    if receipt.findings_truncated {
        facts.push_str("\n  - further findings omitted at the bound");
    }
    facts
}

impl CitationStatus {
    fn is_contradicted(self) -> bool {
        self == Self::Contradicted
    }
}

fn bind_one_citation(citation: &AnswerCitation, observed: &[ObservedLocation]) -> CitationFinding {
    let matched = observed
        .iter()
        .filter(|location| location_covers(&citation.path, location))
        .collect::<Vec<_>>();
    // Exact-file evidence outranks subtree coverage, and success outranks failure.
    if let Some(location) = matched
        .iter()
        .find(|location| location.succeeded && !location.kind.covers_subtree())
    {
        return finding(
            citation,
            CitationStatus::Supported,
            format!(
                "the run's successful {} observed {}",
                location.kind.label(),
                location.path
            ),
        );
    }
    if let Some(location) = matched.iter().find(|location| location.succeeded) {
        return finding(
            citation,
            CitationStatus::Supported,
            format!(
                "covered by the run's successful {} of {}",
                location.kind.label(),
                location.path
            ),
        );
    }
    if let Some(location) = matched.iter().find(|location| !location.succeeded) {
        return finding(
            citation,
            CitationStatus::Contradicted,
            format!(
                "the run's {} of {} did not succeed, so its contents were never observed",
                location.kind.label(),
                location.path
            ),
        );
    }
    finding(
        citation,
        CitationStatus::Unsupported,
        "no observed file-tool call covered this path".to_string(),
    )
}

fn finding(citation: &AnswerCitation, status: CitationStatus, reason: String) -> CitationFinding {
    CitationFinding {
        citation: citation.clone(),
        status,
        reason: bounded_reason(&reason),
    }
}

/// Whether one observed location covers a cited path. Paths are compared with the
/// same suffix-aware rule the evidence anchors use, so a citation written
/// workspace-relative matches an absolute tool argument and vice versa; a
/// directory-scoped observation also covers the subtree beneath it.
fn location_covers(citation: &str, location: &ObservedLocation) -> bool {
    let evidence = normalized_path(&location.path);
    if evidence.is_empty() {
        return false;
    }
    if workspace_targets_match(citation, &evidence)
        || workspace_targets_match(&evidence, citation)
        || evidence == citation
    {
        return true;
    }
    location.kind.covers_subtree()
        && (evidence == "." || citation.starts_with(&format!("{evidence}/")))
}

fn normalized_path(path: &str) -> String {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let normalized = normalized.trim_start_matches("./").trim_end_matches('/');
    normalized.to_string()
}

fn bounded_path(path: &str) -> String {
    path.chars().take(MAX_PATH_CHARS).collect()
}

fn bounded_reason(reason: &str) -> String {
    reason.chars().take(MAX_REASON_CHARS).collect()
}

/// The literal directory prefix a glob pattern enumerates: everything before the
/// first wildcard, cut at the last separator. A pattern with no literal prefix
/// enumerates the whole workspace.
fn glob_pattern_root(pattern: &str) -> String {
    let literal_end = pattern
        .find(['*', '?', '['])
        .unwrap_or(pattern.len())
        .max(1)
        .min(pattern.len());
    let literal = &pattern[..literal_end];
    match literal.rfind('/') {
        Some(index) if index > 0 => literal[..index].trim_end_matches('/').to_string(),
        _ => ".".to_string(),
    }
}

/// Collects every string under a `path` key at any nesting depth, plus the
/// bare-string items of a batch `paths` array — the exact input grammar
/// `file.read_many` accepts (bare strings or `{path, offset_bytes}` objects).
/// A binder that speaks less grammar than the tool reports successful reads
/// as unobserved (run-2 root cause: the delivery judge received false
/// "file never read" facts for every string-form `read_many` call).
fn collect_path_values(value: &serde_json::Value, paths: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if key == "path" {
                    // Trimmed to match the tools' own `non_empty_path`
                    // normalization, so a padded argument still binds.
                    if let Some(path) = child.as_str().map(str::trim) {
                        if !path.is_empty() {
                            paths.insert(path.to_string());
                        }
                    }
                } else if key == "paths" {
                    if let Some(items) = child.as_array() {
                        for item in items {
                            match item.as_str().map(str::trim) {
                                Some(path) if !path.is_empty() => {
                                    paths.insert(path.to_string());
                                }
                                _ => collect_path_values(item, paths),
                            }
                        }
                    } else {
                        collect_path_values(child, paths);
                    }
                } else {
                    collect_path_values(child, paths);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_path_values(item, paths);
            }
        }
        _ => {}
    }
}

fn receipt_digest(receipt: &ClaimEvidenceReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CLAIM_EVIDENCE_SCHEMA.as_bytes());
    hasher.update(receipt.answer_sha256.as_bytes());
    for value in [
        receipt.citations_checked,
        receipt.supported,
        receipt.unsupported,
        receipt.contradicted,
        receipt.observed_locations,
    ] {
        hasher.update(value.to_le_bytes());
    }
    hasher.update([u8::from(receipt.findings_truncated)]);
    for finding in &receipt.findings {
        hasher.update(finding.citation.display().as_bytes());
        hasher.update(finding.status.label().as_bytes());
        hasher.update(finding.reason.as_bytes());
    }
    sha256_hex(&hasher.finalize())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(path: &str, kind: ObservedLocationKind, succeeded: bool) -> ObservedLocation {
        ObservedLocation {
            path: path.to_string(),
            kind,
            succeeded,
        }
    }

    fn tool_message(metadata: Metadata) -> Message {
        Message {
            role: MessageRole::Tool,
            content: "observation".to_string(),
            metadata,
        }
    }

    #[test]
    fn parses_citations_in_the_forms_answers_use() {
        let answer = "See `crates/agent-rag/src/lib.rs:1142` and \
                      apps/desktop/src-tauri/src/lib.rs:40-58, plus \
                      [docs/CURRENT.md#L12](docs/CURRENT.md) and \
                      README.md. Not a path: 12:30 or https://example.com/a.md";

        let citations = answer_citations(answer);
        let rendered = citations
            .iter()
            .map(AnswerCitation::display)
            .collect::<Vec<_>>();

        assert!(
            rendered.contains(&"crates/agent-rag/src/lib.rs:1142".to_string()),
            "citations were {rendered:?}"
        );
        assert!(
            rendered.contains(&"apps/desktop/src-tauri/src/lib.rs:40-58".to_string()),
            "citations were {rendered:?}"
        );
        assert!(
            rendered.contains(&"docs/current.md:12".to_string()),
            "citations were {rendered:?}"
        );
        assert!(
            rendered.contains(&"readme.md".to_string()),
            "citations were {rendered:?}"
        );
        // A bare time and an external URL are not workspace citations.
        assert!(
            !rendered.iter().any(|value| value.contains("12:30")),
            "citations were {rendered:?}"
        );
        assert!(
            !rendered.iter().any(|value| value.contains("example.com")),
            "citations were {rendered:?}"
        );
        // Deduplicated: repeating a citation does not inflate the count.
        let repeated = answer_citations("README.md README.md readme.md");
        assert_eq!(repeated.len(), 1);
    }

    #[test]
    fn citation_count_is_bounded() {
        let answer = (0..200)
            .map(|index| format!("crates/mod{index}.rs:1"))
            .collect::<Vec<_>>()
            .join(" ");

        let citations = answer_citations(&answer);

        assert_eq!(citations.len(), MAX_CITATIONS);
    }

    #[test]
    fn binds_a_citation_to_the_read_that_observed_it() {
        let observed = vec![
            location(
                "/Users/dev/Cindx/crates/agent-rag/src/lib.rs",
                ObservedLocationKind::Read,
                true,
            ),
            location("docs", ObservedLocationKind::Search, true),
        ];

        let receipt = bind_answer_citations(
            "The batch planner is at crates/agent-rag/src/lib.rs:1142 and \
             docs/CURRENT.md:3 records the baseline.",
            &observed,
        );

        assert_eq!(receipt.schema, CLAIM_EVIDENCE_SCHEMA);
        assert_eq!(receipt.citations_checked, 2);
        assert_eq!(receipt.supported, 2, "receipt was {receipt:?}");
        assert_eq!(receipt.unsupported, 0);
        assert_eq!(receipt.contradicted, 0);
        assert!(receipt.findings.is_empty());
        assert!(receipt.fully_supported());
        // A workspace-relative citation matched an absolute tool argument.
        assert_eq!(receipt.observed_locations, 2);
    }

    #[test]
    fn reports_a_citation_the_run_never_observed_as_unsupported() {
        let observed = vec![location(
            "crates/agent-rag/src/lib.rs",
            ObservedLocationKind::Read,
            true,
        )];

        let receipt = bind_answer_citations(
            "Fixed in crates/agent-rag/src/lib.rs:10 and src/invented.rs:77.",
            &observed,
        );

        assert_eq!(receipt.citations_checked, 2);
        assert_eq!(receipt.supported, 1);
        assert_eq!(receipt.unsupported, 1);
        assert_eq!(receipt.contradicted, 0);
        assert!(!receipt.fully_supported());
        assert_eq!(receipt.findings.len(), 1);
        assert_eq!(receipt.findings[0].status, CitationStatus::Unsupported);
        assert_eq!(receipt.findings[0].citation.display(), "src/invented.rs:77");
        assert!(!claim_evidence_facts(&receipt).is_empty());
        assert!(claim_evidence_facts(&receipt).contains("src/invented.rs:77"));
    }

    #[test]
    fn reports_a_citation_whose_read_failed_as_contradicted() {
        let observed = vec![location(
            "src/missing.rs",
            ObservedLocationKind::Read,
            false,
        )];

        let receipt = bind_answer_citations(
            "The bug is at src/missing.rs:12 and src/missing.rs:40.",
            &observed,
        );

        // Both citations point at a path whose read did not succeed.
        assert_eq!(receipt.citations_checked, 2);
        assert_eq!(receipt.contradicted, 2);
        assert_eq!(receipt.unsupported, 0);
        assert_eq!(receipt.findings.len(), 2);
        assert!(receipt.findings[0].reason.contains("did not succeed"));
        // A contradiction outranks a gap in the bounded findings list.
        let mixed = bind_answer_citations(
            "See src/never-seen.rs:1 and src/missing.rs:12.",
            &[location(
                "src/missing.rs",
                ObservedLocationKind::Read,
                false,
            )],
        );
        assert_eq!(mixed.findings[0].status, CitationStatus::Contradicted);
        assert_eq!(mixed.findings[1].status, CitationStatus::Unsupported);
    }

    #[test]
    fn a_successful_read_outranks_a_failed_one_for_the_same_path() {
        let observed = vec![
            location("src/retry.rs", ObservedLocationKind::Read, false),
            location("src/retry.rs", ObservedLocationKind::Read, true),
        ];

        let receipt = bind_answer_citations("Confirmed at src/retry.rs:5.", &observed);

        assert_eq!(receipt.supported, 1);
        assert_eq!(receipt.contradicted, 0);
    }

    #[test]
    fn directory_scoped_evidence_covers_only_its_subtree() {
        let observed = vec![location("crates", ObservedLocationKind::Search, true)];

        let inside = bind_answer_citations("See crates/agent-rag/src/lib.rs:1.", &observed);
        let outside = bind_answer_citations("See apps/desktop/src/lib.rs:1.", &observed);

        assert_eq!(inside.supported, 1);
        assert_eq!(outside.unsupported, 1);

        // A read is exact-file evidence and never covers a sibling.
        let read = vec![location(
            "crates/agent-rag/src/lib.rs",
            ObservedLocationKind::Read,
            true,
        )];
        let sibling = bind_answer_citations("See crates/agent-rag/src/other.rs:1.", &read);
        assert_eq!(sibling.unsupported, 1);
    }

    #[test]
    fn derives_observed_locations_from_whitelisted_file_tool_arguments() {
        let read = observed_locations_for_call(
            "file.read",
            r#"{"path":"crates/agent-rag/src/lib.rs","offset_bytes":0}"#,
            true,
        );
        assert_eq!(
            read,
            vec![location(
                "crates/agent-rag/src/lib.rs",
                ObservedLocationKind::Read,
                true
            )]
        );

        // The batch tool nests one `path` per entry.
        let batch = observed_locations_for_call(
            "file.read_many",
            r#"{"paths":[{"path":"a.rs","offset_bytes":0},{"path":"b.rs","offset_bytes":0}]}"#,
            true,
        );
        assert_eq!(batch.len(), 2);
        assert!(batch
            .iter()
            .all(|item| item.kind == ObservedLocationKind::Read));

        // The batch tool equally accepts bare strings (and mixes both forms);
        // the binder must speak the tool's full grammar or successful reads
        // go unobserved and the judge is told the files were never read.
        let string_batch = observed_locations_for_call(
            "file.read_many",
            r#"{"paths":["src/config.py","src/api.py"]}"#,
            true,
        );
        assert_eq!(string_batch.len(), 2);
        assert!(string_batch
            .iter()
            .all(|item| item.kind == ObservedLocationKind::Read));
        let mixed_batch = observed_locations_for_call(
            "file.read_many",
            r#"{"paths":["a.rs",{"path":"b.rs","offset_bytes":10}]}"#,
            true,
        );
        assert_eq!(mixed_batch.len(), 2);

        // Padded entries bind trimmed, matching the tool's own
        // `non_empty_path` normalization.
        let padded = observed_locations_for_call("file.read_many", r#"{"paths":[" a.rs "]}"#, true);
        assert_eq!(
            padded,
            vec![location("a.rs", ObservedLocationKind::Read, true)]
        );

        // A patch batch is mutation evidence for every target.
        let patch_batch = observed_locations_for_call(
            "file.patch_batch",
            r#"{"operations":[{"path":"c.rs"},{"path":"d.rs"}]}"#,
            true,
        );
        assert_eq!(patch_batch.len(), 2);
        assert!(patch_batch
            .iter()
            .all(|item| item.kind == ObservedLocationKind::Mutation));

        // A glob names a pattern: the literal prefix is the enumerated subtree.
        let glob = observed_locations_for_call("file.glob", r#"{"pattern":"src/**/*.rs"}"#, true);
        assert_eq!(
            glob,
            vec![location("src", ObservedLocationKind::Search, true)]
        );
        let rooted = observed_locations_for_call("file.glob", r#"{"pattern":"**/*.rs"}"#, true);
        assert_eq!(rooted[0].path, ".");

        // A failed read is still recorded, as contradiction evidence.
        let failed = observed_locations_for_call("file.read", r#"{"path":"gone.rs"}"#, false);
        assert!(!failed[0].succeeded);

        // Out of scope: other tools, malformed input, and unbounded fan-out.
        assert!(
            observed_locations_for_call("shell.run", r#"{"command":"ls a.rs"}"#, true).is_empty()
        );
        assert!(observed_locations_for_call("file.read", "not json", true).is_empty());
        let many = (0..40)
            .map(|index| format!(r#"{{"path":"file{index}.rs"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let wide = observed_locations_for_call(
            "file.read_many",
            &format!(r#"{{"paths":[{many}]}}"#),
            true,
        );
        assert_eq!(wide.len(), MAX_OBSERVED_LOCATIONS_PER_CALL);
    }

    #[test]
    fn observed_locations_round_trip_through_the_tool_message_record() {
        let mut stamped = Metadata::new();
        insert_observed_locations(
            &mut stamped,
            &[location("src/a.rs", ObservedLocationKind::Read, true)],
        );
        let mut unstamped = Metadata::new();
        unstamped.insert("tool_name".to_string(), "file.read".to_string());
        let mut assistant = Metadata::new();
        insert_observed_locations(
            &mut assistant,
            &[location("src/ignored.rs", ObservedLocationKind::Read, true)],
        );
        let messages = vec![
            tool_message(stamped),
            tool_message(unstamped),
            Message {
                role: MessageRole::Assistant,
                content: "answer".to_string(),
                metadata: assistant,
            },
        ];

        let recovered = observed_locations_from_messages(&messages);

        // Only the stamped tool message contributes; a corrupt stamp is skipped.
        assert_eq!(
            recovered,
            vec![location("src/a.rs", ObservedLocationKind::Read, true)]
        );

        // The delegation carrier is the one System message that contributes:
        // the join stamps the child's observed locations on the
        // `subagent_result` instruction. Any other System message stays
        // invisible to the binder even when stamped.
        let mut carrier = Metadata::new();
        carrier.insert("kind".to_string(), SUBAGENT_RESULT_MESSAGE_KIND.to_string());
        insert_observed_locations(
            &mut carrier,
            &[location("src/child.rs", ObservedLocationKind::Read, true)],
        );
        let mut impostor = carrier.clone();
        impostor.insert("kind".to_string(), "other_instruction".to_string());
        let carried = observed_locations_from_messages(&[
            Message {
                role: MessageRole::System,
                content: "Subagent result".to_string(),
                metadata: carrier,
            },
            Message {
                role: MessageRole::System,
                content: "other".to_string(),
                metadata: impostor,
            },
        ]);
        assert_eq!(
            carried,
            vec![location("src/child.rs", ObservedLocationKind::Read, true)]
        );

        let mut corrupt = Metadata::new();
        corrupt.insert(
            OBSERVED_LOCATIONS_SCHEMA_KEY.to_string(),
            OBSERVED_LOCATIONS_SCHEMA.to_string(),
        );
        corrupt.insert(OBSERVED_LOCATIONS_KEY.to_string(), "{".to_string());
        assert!(observed_locations_from_messages(&[tool_message(corrupt)]).is_empty());
        // An empty stamp writes nothing.
        let mut empty = Metadata::new();
        insert_observed_locations(&mut empty, &[]);
        assert!(!empty.contains_key(OBSERVED_LOCATIONS_KEY));
    }

    #[test]
    fn an_answer_without_citations_produces_no_facts_block() {
        let receipt = bind_answer_citations(
            "Done. The build is green and the tests pass.",
            &[location("src/a.rs", ObservedLocationKind::Read, true)],
        );

        assert_eq!(receipt.citations_checked, 0);
        assert!(!receipt.fully_supported());
        assert!(claim_evidence_facts(&receipt).is_empty());
    }

    #[test]
    fn findings_are_bounded_and_the_receipt_digest_is_deterministic() {
        let answer = (0..30)
            .map(|index| format!("src/unobserved{index}.rs:{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let observed = vec![location("src/seen.rs", ObservedLocationKind::Read, true)];

        let receipt = bind_answer_citations(&answer, &observed);
        let again = bind_answer_citations(&answer, &observed);

        assert_eq!(receipt.unsupported, 30);
        assert_eq!(receipt.findings.len(), MAX_FINDINGS);
        assert!(receipt.findings_truncated);
        assert!(claim_evidence_facts(&receipt).contains("further findings omitted"));
        assert_eq!(receipt.receipt_sha256, again.receipt_sha256);
        assert_eq!(receipt.answer_sha256, again.answer_sha256);

        // A different answer changes the bound answer digest.
        let other = bind_answer_citations(&format!("{answer} tail"), &observed);
        assert_ne!(other.answer_sha256, receipt.answer_sha256);
        assert_ne!(other.receipt_sha256, receipt.receipt_sha256);
        assert!(receipt.receipt_sha256.len() == 64);
    }
}
