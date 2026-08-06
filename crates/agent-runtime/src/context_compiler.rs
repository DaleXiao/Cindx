use crate::context_engine::ContextSourceKind;
use agent_core::Message;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const CONTEXT_COMPILER_RECEIPT_SCHEMA: &str = "cindx.context-compiler-receipt.v1";
pub const CONTEXT_COMPILER_POLICY: &str = "authoritative-effective-objective-v1";
pub const MAX_CONTEXT_COMPILER_RECEIPT_BYTES: usize = 4 * 1024;

const MAX_OBJECTIVE_TERMS: usize = 128;
const MAX_CONTEXT_TERMS: usize = 512;
const MAX_TERM_CHARACTERS: usize = 64;
const MAX_OBJECTIVE_RELEVANCE_CHARACTERS: usize = 2_048;
const MAX_CONTEXT_RELEVANCE_CHARACTERS: usize = 8_192;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompilerCounts {
    pub candidate_sources: u64,
    pub selected_sources: u64,
    pub omitted_sources: u64,
    pub original_messages: u64,
    pub projected_messages: u64,
    pub omitted_messages: u64,
    pub truncated_messages: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompilerHardInvariants {
    pub hard_limit_satisfied: bool,
    pub current_request_preserved: bool,
    pub protected_sources_satisfied: bool,
    pub tool_round_integrity_satisfied: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompilerOperationCounts {
    pub messages_scanned: u64,
    pub candidates_ranked: u64,
    pub lexical_segments_scanned: u64,
    pub text_characters_indexed: u64,
    pub term_characters_indexed: u64,
    pub objective_terms_indexed: u64,
    pub context_terms_indexed: u64,
    pub term_membership_checks: u64,
    pub cjk_bigrams_indexed: u64,
}

impl ContextCompilerOperationCounts {
    #[cfg(test)]
    fn total(&self) -> u64 {
        self.messages_scanned
            .saturating_add(self.candidates_ranked)
            .saturating_add(self.lexical_segments_scanned)
            .saturating_add(self.text_characters_indexed)
            .saturating_add(self.term_characters_indexed)
            .saturating_add(self.objective_terms_indexed)
            .saturating_add(self.context_terms_indexed)
            .saturating_add(self.term_membership_checks)
            .saturating_add(self.cjk_bigrams_indexed)
    }

    pub(crate) fn merge(&mut self, previous: &Self) {
        self.messages_scanned = self
            .messages_scanned
            .saturating_add(previous.messages_scanned);
        self.candidates_ranked = self
            .candidates_ranked
            .saturating_add(previous.candidates_ranked);
        self.lexical_segments_scanned = self
            .lexical_segments_scanned
            .saturating_add(previous.lexical_segments_scanned);
        self.text_characters_indexed = self
            .text_characters_indexed
            .saturating_add(previous.text_characters_indexed);
        self.term_characters_indexed = self
            .term_characters_indexed
            .saturating_add(previous.term_characters_indexed);
        self.objective_terms_indexed = self
            .objective_terms_indexed
            .saturating_add(previous.objective_terms_indexed);
        self.context_terms_indexed = self
            .context_terms_indexed
            .saturating_add(previous.context_terms_indexed);
        self.term_membership_checks = self
            .term_membership_checks
            .saturating_add(previous.term_membership_checks);
        self.cjk_bigrams_indexed = self
            .cjk_bigrams_indexed
            .saturating_add(previous.cjk_bigrams_indexed);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompilerReceipt {
    pub schema: String,
    pub policy: String,
    pub objective_fingerprint: String,
    pub relevance_applied: bool,
    pub selected_source_tokens: BTreeMap<String, u64>,
    pub omitted_source_tokens: BTreeMap<String, u64>,
    pub selected_source_counts: BTreeMap<String, u64>,
    pub omitted_source_counts: BTreeMap<String, u64>,
    pub counts: ContextCompilerCounts,
    pub hard_invariants: ContextCompilerHardInvariants,
    pub operation_counts: ContextCompilerOperationCounts,
    pub canonical_digest: String,
}

impl ContextCompilerReceipt {
    pub fn to_bounded_json(&self) -> Option<String> {
        if !self.digest_valid() {
            return None;
        }
        let encoded = serde_json::to_string(self).ok()?;
        (encoded.len() <= MAX_CONTEXT_COMPILER_RECEIPT_BYTES).then_some(encoded)
    }

    pub fn digest_valid(&self) -> bool {
        self.canonical_digest.len() == 64
            && self.canonical_digest == self.computed_canonical_digest()
    }

    pub(crate) fn seal(&mut self) {
        self.canonical_digest = self.computed_canonical_digest();
    }

    pub(crate) fn merge_operation_counts(&mut self, previous: &Self) {
        self.operation_counts.merge(&previous.operation_counts);
        self.seal();
    }

    fn computed_canonical_digest(&self) -> String {
        let mut canonical = self.clone();
        canonical.canonical_digest.clear();
        let encoded = serde_json::to_vec(&canonical)
            .expect("context compiler receipt has a canonical JSON representation");
        let digest = Sha256::digest(encoded);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RankedContextCandidate {
    pub index: usize,
    pub protected: bool,
    pub relevance: u16,
    pub priority: u8,
}

pub(crate) struct ContextCompiler<'objective> {
    objective: &'objective str,
    objective_fingerprint: &'objective str,
    operation_counts: ContextCompilerOperationCounts,
}

impl<'objective> ContextCompiler<'objective> {
    pub(crate) fn new(objective: &'objective str, objective_fingerprint: &'objective str) -> Self {
        Self {
            objective,
            objective_fingerprint,
            operation_counts: ContextCompilerOperationCounts::default(),
        }
    }

    pub(crate) fn rank_context_sources(
        &mut self,
        messages: &[Message],
    ) -> Vec<RankedContextCandidate> {
        let objective_index = context_relevance_terms(
            self.objective,
            MAX_OBJECTIVE_TERMS,
            MAX_OBJECTIVE_RELEVANCE_CHARACTERS,
            true,
            true,
        );
        let objective_terms = objective_index.terms;
        self.operation_counts.objective_terms_indexed = objective_terms.len() as u64;
        self.operation_counts.cjk_bigrams_indexed = objective_index.cjk_bigrams;
        self.operation_counts.lexical_segments_scanned = objective_index.lexical_segments;
        self.operation_counts.text_characters_indexed = objective_index.text_characters;
        self.operation_counts.term_characters_indexed = objective_index.term_characters;

        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        for (index, message) in messages.iter().enumerate().rev() {
            self.operation_counts.messages_scanned =
                self.operation_counts.messages_scanned.saturating_add(1);
            let Some(source) = ContextSourceKind::from_message(message) else {
                continue;
            };
            if !seen.insert(system_context_key(message, index)) {
                continue;
            }
            let relevance =
                context_relevance_score(message, &objective_terms, &mut self.operation_counts);
            candidates.push(RankedContextCandidate {
                index,
                protected: source.is_protected(),
                relevance,
                priority: source.priority(),
            });
        }
        self.operation_counts.candidates_ranked = candidates.len() as u64;
        candidates.sort_by(|left, right| {
            right
                .protected
                .cmp(&left.protected)
                .then(right.relevance.cmp(&left.relevance))
                .then(right.priority.cmp(&left.priority))
                .then(right.index.cmp(&left.index))
        });
        candidates
    }

    pub(crate) fn into_receipt(self, relevance_applied: bool) -> ContextCompilerReceipt {
        let mut receipt = ContextCompilerReceipt {
            schema: CONTEXT_COMPILER_RECEIPT_SCHEMA.to_string(),
            policy: CONTEXT_COMPILER_POLICY.to_string(),
            objective_fingerprint: self.objective_fingerprint.to_string(),
            relevance_applied,
            selected_source_tokens: BTreeMap::new(),
            omitted_source_tokens: BTreeMap::new(),
            selected_source_counts: BTreeMap::new(),
            omitted_source_counts: BTreeMap::new(),
            counts: ContextCompilerCounts::default(),
            hard_invariants: ContextCompilerHardInvariants::default(),
            operation_counts: self.operation_counts,
            canonical_digest: String::new(),
        };
        receipt.seal();
        receipt
    }
}

pub(crate) fn context_source_identity(message: &Message) -> Option<String> {
    let source = ContextSourceKind::from_message(message)?;
    if source == ContextSourceKind::GroundingEvidence {
        return Some(format!(
            "{}:{}:{}:{}",
            source.as_str(),
            message
                .metadata
                .get("kind")
                .map(String::as_str)
                .unwrap_or("unknown"),
            message
                .metadata
                .get("prompt_contract_epoch")
                .map(String::as_str)
                .unwrap_or("0"),
            message
                .metadata
                .get("requirement_ids_json")
                .or_else(|| message.metadata.get("requirement_id"))
                .or_else(|| message.metadata.get("collaboration_id"))
                .map(String::as_str)
                .unwrap_or("default"),
        ));
    }
    Some(source.as_str().to_string())
}

pub(crate) fn context_source_counts(messages: &[Message]) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for source in messages
        .iter()
        .skip(1)
        .filter_map(ContextSourceKind::from_message)
    {
        *counts.entry(source.as_str().to_string()).or_default() += 1;
    }
    counts
}

pub(crate) fn context_source_counts_for_indices(
    messages: &[Message],
    indices: &[usize],
) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for source in indices
        .iter()
        .filter_map(|index| messages.get(*index))
        .filter_map(ContextSourceKind::from_message)
    {
        *counts.entry(source.as_str().to_string()).or_default() += 1;
    }
    counts
}

fn system_context_key(message: &Message, index: usize) -> String {
    if let Some(identity) = context_source_identity(message) {
        if ContextSourceKind::from_message(message).is_some_and(ContextSourceKind::is_protected) {
            return identity;
        }
    }
    message
        .metadata
        .get("kind")
        .or_else(|| message.metadata.get("collaboration_stage"))
        .cloned()
        .unwrap_or_else(|| format!("system-{index}"))
}

fn context_relevance_score(
    message: &Message,
    objective_terms: &BTreeSet<String>,
    operations: &mut ContextCompilerOperationCounts,
) -> u16 {
    if objective_terms.is_empty() {
        return 0;
    }
    let message_index = context_relevance_terms(
        &message.content,
        MAX_CONTEXT_TERMS,
        MAX_CONTEXT_RELEVANCE_CHARACTERS,
        true,
        false,
    );
    let message_terms = message_index.terms;
    operations.context_terms_indexed = operations
        .context_terms_indexed
        .saturating_add(message_terms.len() as u64);
    operations.cjk_bigrams_indexed = operations
        .cjk_bigrams_indexed
        .saturating_add(message_index.cjk_bigrams);
    operations.lexical_segments_scanned = operations
        .lexical_segments_scanned
        .saturating_add(message_index.lexical_segments);
    operations.text_characters_indexed = operations
        .text_characters_indexed
        .saturating_add(message_index.text_characters);
    operations.term_characters_indexed = operations
        .term_characters_indexed
        .saturating_add(message_index.term_characters);
    let mut overlap = 0usize;
    let mut identifier_overlap = 0usize;
    for term in &message_terms {
        operations.term_membership_checks = operations.term_membership_checks.saturating_add(1);
        if objective_terms.contains(term) {
            overlap += 1;
            identifier_overlap += usize::from(is_context_identifier(term));
        }
    }
    if overlap == 0 {
        return 0;
    }
    let coverage = overlap.saturating_mul(30) / objective_terms.len().max(1);
    overlap
        .saturating_mul(12)
        .saturating_add(coverage)
        .saturating_add(identifier_overlap.saturating_mul(50))
        .min(u16::MAX as usize) as u16
}

struct RelevanceTermIndex {
    terms: BTreeSet<String>,
    cjk_bigrams: u64,
    lexical_segments: u64,
    text_characters: u64,
    term_characters: u64,
}

fn context_relevance_terms(
    text: &str,
    limit: usize,
    character_limit: usize,
    include_tail: bool,
    strip_synthetic_steer_ordinals: bool,
) -> RelevanceTermIndex {
    let mut terms = BTreeSet::new();
    let mut cjk_bigrams = 0u64;
    let mut lexical_segments = 0u64;
    let mut text_characters = 0u64;
    let mut term_characters = 0u64;
    let windows = relevance_windows(text, character_limit, include_tail);
    let window_count = 1 + usize::from(windows.tail.is_some());
    let window_term_limit = if window_count > 1 {
        (limit / window_count).max(1)
    } else {
        limit
    };
    for (window_index, window) in [Some(windows.head), windows.tail]
        .into_iter()
        .flatten()
        .enumerate()
    {
        text_characters = text_characters.saturating_add(window.chars().count() as u64);
        let maximum_terms = terms.len().saturating_add(window_term_limit).min(limit);
        let mut add_term = |raw_term: &str| {
            lexical_segments = lexical_segments.saturating_add(1);
            let mut bounded = raw_term
                .trim_matches(|character: char| matches!(character, '.' | '/' | ':' | '-' | '@'))
                .chars();
            let mut consumed_characters = 0u64;
            let term = bounded
                .by_ref()
                .take(MAX_TERM_CHARACTERS)
                .inspect(|_| consumed_characters = consumed_characters.saturating_add(1))
                .flat_map(char::to_lowercase)
                .collect::<String>();
            let overlong = bounded.next().is_some();
            term_characters = term_characters
                .saturating_add(consumed_characters)
                .saturating_add(u64::from(overlong));
            let character_count = term.chars().count();
            if !term.is_empty()
                && !overlong
                && (character_count > 1 || term.chars().any(char::is_numeric))
                && !is_context_stopword(&term)
            {
                terms.insert(term.clone());
            }
            let mut previous_cjk = None;
            for character in term.chars() {
                if is_cjk(character) {
                    if let Some(previous) = previous_cjk {
                        if terms.len() >= maximum_terms {
                            break;
                        }
                        terms.insert([previous, character].iter().collect());
                        cjk_bigrams = cjk_bigrams.saturating_add(1);
                    }
                    previous_cjk = Some(character);
                } else {
                    previous_cjk = None;
                }
            }
            terms.len() >= maximum_terms
        };
        let segment_limit = window_term_limit.saturating_mul(2);
        let reversed = include_tail && window_count > 1 && window_index + 1 == window_count;
        if strip_synthetic_steer_ordinals {
            let raw_terms = if reversed {
                window
                    .rsplit(relevance_term_boundary)
                    .filter(|term| !term.is_empty())
                    .take(segment_limit)
                    .collect::<Vec<_>>()
            } else {
                window
                    .split(relevance_term_boundary)
                    .filter(|term| !term.is_empty())
                    .take(segment_limit)
                    .collect::<Vec<_>>()
            };
            for (index, raw_term) in raw_terms.iter().enumerate() {
                if synthetic_steer_ordinal(&raw_terms, index, reversed) {
                    continue;
                }
                if add_term(raw_term) {
                    break;
                }
            }
        } else if reversed {
            for raw_term in window
                .rsplit(relevance_term_boundary)
                .filter(|term| !term.is_empty())
                .take(segment_limit)
            {
                if add_term(raw_term) {
                    break;
                }
            }
        } else {
            for raw_term in window
                .split(relevance_term_boundary)
                .filter(|term| !term.is_empty())
                .take(segment_limit)
            {
                if add_term(raw_term) {
                    break;
                }
            }
        }
    }
    RelevanceTermIndex {
        terms,
        cjk_bigrams,
        lexical_segments,
        text_characters,
        term_characters,
    }
}

fn relevance_term_boundary(character: char) -> bool {
    !(character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | ':' | '@'))
}

fn synthetic_steer_ordinal(raw_terms: &[&str], index: usize, reversed: bool) -> bool {
    let ordinal = trim_relevance_term(raw_terms[index]);
    if ordinal.is_empty() || !ordinal.chars().all(|character| character.is_ascii_digit()) {
        return false;
    }
    let (accepted, steering) = if reversed {
        (raw_terms.get(index + 2), raw_terms.get(index + 1))
    } else {
        let Some(steering_index) = index.checked_sub(1) else {
            return false;
        };
        let Some(accepted_index) = index.checked_sub(2) else {
            return false;
        };
        (raw_terms.get(accepted_index), raw_terms.get(steering_index))
    };
    accepted.is_some_and(|term| trim_relevance_term(term).eq_ignore_ascii_case("accepted"))
        && steering.is_some_and(|term| trim_relevance_term(term).eq_ignore_ascii_case("steering"))
}

fn trim_relevance_term(term: &str) -> &str {
    term.trim_matches(|character: char| matches!(character, '.' | '/' | ':' | '-' | '@'))
}

struct RelevanceWindows<'text> {
    head: &'text str,
    tail: Option<&'text str>,
}

fn relevance_windows(
    text: &str,
    character_limit: usize,
    include_tail: bool,
) -> RelevanceWindows<'_> {
    let is_long = text.char_indices().nth(character_limit).is_some();
    if !is_long {
        return RelevanceWindows {
            head: text,
            tail: None,
        };
    }
    let head_characters = if include_tail {
        character_limit / 2
    } else {
        character_limit
    };
    let head_end = text
        .char_indices()
        .nth(head_characters)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    if !include_tail {
        return RelevanceWindows {
            head: &text[..head_end],
            tail: None,
        };
    }
    let tail_characters = character_limit.saturating_sub(head_characters);
    let tail_start = text
        .char_indices()
        .rev()
        .nth(tail_characters.saturating_sub(1))
        .map(|(index, _)| index)
        .unwrap_or(head_end);
    RelevanceWindows {
        head: &text[..head_end],
        tail: Some(&text[tail_start..]),
    }
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

fn is_context_stopword(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "and"
            | "accepted"
            | "are"
            | "as"
            | "at"
            | "be"
            | "by"
            | "for"
            | "from"
            | "in"
            | "initial"
            | "is"
            | "it"
            | "of"
            | "on"
            | "or"
            | "request"
            | "steering"
            | "that"
            | "the"
            | "this"
            | "to"
            | "was"
            | "what"
            | "when"
            | "where"
            | "which"
            | "with"
    )
}

fn is_context_identifier(term: &str) -> bool {
    term.chars()
        .any(|character| matches!(character, '_' | '-' | '.' | '/' | ':' | '@'))
        || term.chars().any(char::is_numeric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MessageRole, Metadata};

    fn optional_context(stage: &str, content: impl Into<String>) -> Message {
        Message {
            role: MessageRole::System,
            content: content.into(),
            metadata: [("collaboration_stage".to_string(), stage.to_string())]
                .into_iter()
                .collect::<Metadata>(),
        }
    }

    #[test]
    fn cjk_bigrams_rank_semantically_overlapping_context() {
        let messages = vec![
            optional_context("export", "导出报表并调整日期格式"),
            optional_context("login", "登录按钮验证与账号恢复流程"),
        ];
        let fingerprint = "a".repeat(64);
        let mut compiler = ContextCompiler::new("修复登录界面的按钮", &fingerprint);

        let ranked = compiler.rank_context_sources(&messages);

        assert_eq!(ranked[0].index, 1);
        assert!(ranked[0].relevance > ranked[1].relevance);
        assert!(compiler.operation_counts.cjk_bigrams_indexed > 0);
    }

    #[test]
    fn authoritative_objective_indexes_bounded_head_and_latest_steer_tail() {
        let middle = (0..300)
            .map(|index| format!("middle_token_{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let tail_noise = (0..300)
            .map(|index| format!("t{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let objective = format!(
            "Initial request: preserve_head_requirement {middle}\nAccepted steering: {tail_noise} apply_tail_requirement"
        );
        let indexed = context_relevance_terms(
            &objective,
            MAX_OBJECTIVE_TERMS,
            MAX_OBJECTIVE_RELEVANCE_CHARACTERS,
            true,
            true,
        );

        assert!(indexed.terms.contains("preserve_head_requirement"));
        assert!(indexed.terms.contains("apply_tail_requirement"));
        assert!(!indexed.terms.contains("initial"));
        assert!(!indexed.terms.contains("request"));
        assert!(!indexed.terms.contains("accepted"));
        assert!(!indexed.terms.contains("steering"));
        assert!(indexed.terms.len() <= MAX_OBJECTIVE_TERMS);

        let continue_objective =
            format!("Initial request: preserve_head_requirement {middle}\nAccepted steering: 继续");
        let continue_indexed = context_relevance_terms(
            &continue_objective,
            MAX_OBJECTIVE_TERMS,
            MAX_OBJECTIVE_RELEVANCE_CHARACTERS,
            true,
            true,
        );
        assert!(continue_indexed.terms.contains("preserve_head_requirement"));
    }

    #[test]
    fn synthetic_steer_ordinal_is_not_a_relevance_identifier() {
        let objective =
            "Initial request: inspect port 8080\nAccepted steering 7: verify the listener";
        let indexed = context_relevance_terms(
            objective,
            MAX_OBJECTIVE_TERMS,
            MAX_OBJECTIVE_RELEVANCE_CHARACTERS,
            true,
            true,
        );

        assert!(indexed.terms.contains("8080"));
        assert!(!indexed.terms.contains("7"));
        let mut operations = ContextCompilerOperationCounts::default();
        let ordinal_context = optional_context("ordinal", "synthetic ordinal 7");
        assert_eq!(
            context_relevance_score(&ordinal_context, &indexed.terms, &mut operations),
            0
        );
    }

    #[test]
    fn relevant_context_tail_beats_a_newer_same_priority_distractor() {
        let messages = vec![
            optional_context(
                "older-relevant",
                format!("{} rare_tail_marker", "unrelated prefix ".repeat(2_000)),
            ),
            optional_context("newer-distractor", "recent but unrelated context"),
        ];
        let fingerprint = "d".repeat(64);
        let mut compiler = ContextCompiler::new("inspect rare_tail_marker", &fingerprint);

        let ranked = compiler.rank_context_sources(&messages);

        assert_eq!(ranked[0].index, 0);
        assert!(ranked[0].relevance > ranked[1].relevance);
        assert!(
            compiler.operation_counts.text_characters_indexed
                <= MAX_OBJECTIVE_RELEVANCE_CHARACTERS as u64
                    + 2 * MAX_CONTEXT_RELEVANCE_CHARACTERS as u64
        );
    }

    #[test]
    fn giant_single_segment_uses_bounded_relevance_prefix() {
        let huge = "登".repeat(1_000_000);
        let indexed = context_relevance_terms(
            &huge,
            MAX_CONTEXT_TERMS,
            MAX_CONTEXT_RELEVANCE_CHARACTERS,
            false,
            false,
        );

        assert!(indexed.terms.len() <= MAX_CONTEXT_TERMS);
        assert!(indexed.text_characters <= MAX_CONTEXT_RELEVANCE_CHARACTERS as u64);
        assert!(indexed.term_characters <= (MAX_TERM_CHARACTERS + 1) as u64);
        assert!(indexed.cjk_bigrams <= MAX_TERM_CHARACTERS.saturating_sub(1) as u64);
    }

    #[test]
    fn contract_receipt_is_bounded_and_contains_no_source_text() {
        let sentinel = "RAW_OBJECTIVE_MUST_NOT_LEAK";
        let messages = vec![optional_context("contract", sentinel)];
        let fingerprint = "b".repeat(64);
        let mut compiler = ContextCompiler::new(sentinel, &fingerprint);
        let _ = compiler.rank_context_sources(&messages);
        let receipt = compiler.into_receipt(true);
        let encoded = receipt.to_bounded_json().expect("receipt remains bounded");

        assert_eq!(receipt.schema, CONTEXT_COMPILER_RECEIPT_SCHEMA);
        assert_eq!(receipt.policy, CONTEXT_COMPILER_POLICY);
        assert!(!encoded.contains(sentinel));
        assert!(encoded.len() <= MAX_CONTEXT_COMPILER_RECEIPT_BYTES);
    }

    #[test]
    fn operation_counts_scale_with_candidates_not_wall_clock() {
        let compile = |candidate_count: usize| {
            let messages = (0..candidate_count)
                .map(|index| {
                    optional_context(
                        &format!("optional-{index}"),
                        format!("登录按钮 account-{index} {}", "detail ".repeat(700)),
                    )
                })
                .collect::<Vec<_>>();
            let fingerprint = "c".repeat(64);
            let mut compiler = ContextCompiler::new("修复登录按钮", &fingerprint);
            let ranked = compiler.rank_context_sources(&messages);
            assert_eq!(ranked.len(), candidate_count);
            compiler.operation_counts
        };
        let small = compile(32);
        let large = compile(64);

        assert_eq!(small.messages_scanned, 32);
        assert_eq!(large.messages_scanned, 64);
        assert_eq!(small.candidates_ranked, 32);
        assert_eq!(large.candidates_ranked, 64);
        assert!(small.context_terms_indexed <= 32 * MAX_CONTEXT_TERMS as u64);
        assert!(large.context_terms_indexed <= 64 * MAX_CONTEXT_TERMS as u64);
        assert!(
            small.text_characters_indexed
                <= MAX_OBJECTIVE_RELEVANCE_CHARACTERS as u64
                    + 32 * MAX_CONTEXT_RELEVANCE_CHARACTERS as u64
        );
        assert!(
            large.text_characters_indexed
                <= MAX_OBJECTIVE_RELEVANCE_CHARACTERS as u64
                    + 64 * MAX_CONTEXT_RELEVANCE_CHARACTERS as u64
        );
        assert!(large.total() <= small.total().saturating_mul(2).saturating_add(256));
        println!(
            "{{\"schema\":\"cindx.context-compiler-scaling.v1\",\"small_operations\":{},\"large_operations\":{},\"wall_clock_used\":false}}",
            small.total(),
            large.total()
        );
    }
}
