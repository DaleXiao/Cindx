use super::*;
use agent_core::{Metadata, ToolCallId};

const INPUT_A: &str = concat!(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
);
const INPUT_B: &str = concat!(
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
);

fn result_with_evidence(
    status: ToolOutcomeStatus,
    complete: bool,
    evidence: impl Into<String>,
    facts: &[(&str, &str)],
) -> ToolResult {
    let facts = facts
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    ToolResult {
        invocation_id: ToolCallId("call-1".to_string()),
        status,
        output: "excluded output".to_string(),
        content: Vec::new(),
        structured_output_json: Some("excluded structured output".to_string()),
        artifacts: Vec::new(),
        failure: None,
        model_observation: Some(ToolObservationV2::new(
            "workspace.read",
            "excluded summary",
            evidence,
            complete,
            facts,
        )),
        metadata: Metadata::new(),
    }
}

fn observe_read(
    cursor: &mut AdaptiveLoopCursor,
    tool_name: &str,
    input_fingerprint: &str,
    result: &ToolResult,
) -> AdaptiveLoopDisposition {
    cursor.observe_tool_result(
        0,
        tool_name,
        input_fingerprint,
        result,
        Some(&ToolEffectSemantics::ReadOnly),
        None,
    )
}

#[test]
fn repeated_semantic_reads_replan_once_then_terminal_across_a_long_cycle() {
    let mut cursor = AdaptiveLoopCursor::default();
    let actions = [INPUT_A, INPUT_B, "input-c", "input-d"];
    let reads = actions
        .iter()
        .enumerate()
        .map(|(index, _)| {
            result_with_evidence(
                ToolOutcomeStatus::Succeeded,
                true,
                format!("stable evidence {index}"),
                &[("url", "https://example.com")],
            )
        })
        .collect::<Vec<_>>();

    for (action, read) in actions.iter().zip(&reads) {
        assert_eq!(
            observe_read(&mut cursor, "browser.extract_text", action, read),
            AdaptiveLoopDisposition::Continue
        );
    }
    assert_eq!(
        observe_read(&mut cursor, "browser.extract_text", actions[0], &reads[0]),
        AdaptiveLoopDisposition::ReplanOnce
    );
    assert_eq!(
        observe_read(&mut cursor, "browser.extract_text", actions[1], &reads[1]),
        AdaptiveLoopDisposition::CommitTerminalResult,
        "continued duplicate evidence after the one replan must terminate a period-four cycle"
    );
}

#[test]
fn changed_semantic_evidence_is_progress_and_volatile_facts_are_not() {
    let mut cursor = AdaptiveLoopCursor::default();
    let first = result_with_evidence(
        ToolOutcomeStatus::Succeeded,
        true,
        "stable content",
        &[
            ("duration_ms", "1"),
            ("text_path", "/tmp/first"),
            ("url", "https://example.com/one"),
        ],
    );
    let volatile_only = result_with_evidence(
        ToolOutcomeStatus::Succeeded,
        true,
        "stable content",
        &[
            ("duration_ms", "9"),
            ("text_path", "/tmp/second"),
            ("url", "https://example.com/one"),
        ],
    );
    let stable_fact_changed = result_with_evidence(
        ToolOutcomeStatus::Succeeded,
        true,
        "stable content",
        &[
            ("duration_ms", "9"),
            ("text_path", "/tmp/second"),
            ("url", "https://example.com/two"),
        ],
    );
    let evidence_changed = result_with_evidence(
        ToolOutcomeStatus::Succeeded,
        true,
        "new content",
        &[
            ("duration_ms", "9"),
            ("text_path", "/tmp/second"),
            ("url", "https://example.com/two"),
        ],
    );

    assert_eq!(
        observe_read(&mut cursor, "browser.extract_text", INPUT_A, &first),
        AdaptiveLoopDisposition::Continue
    );
    assert_eq!(
        observe_read(&mut cursor, "browser.extract_text", INPUT_A, &volatile_only),
        AdaptiveLoopDisposition::ReplanOnce
    );
    assert_eq!(
        observe_read(
            &mut cursor,
            "browser.extract_text",
            INPUT_A,
            &stable_fact_changed,
        ),
        AdaptiveLoopDisposition::Continue
    );
    assert_eq!(cursor.no_gain_count(), 0);
    assert!(!cursor.replan_emitted());
    assert_eq!(
        observe_read(
            &mut cursor,
            "browser.extract_text",
            INPUT_A,
            &stable_fact_changed,
        ),
        AdaptiveLoopDisposition::ReplanOnce
    );
    assert_eq!(
        observe_read(
            &mut cursor,
            "browser.extract_text",
            INPUT_A,
            &evidence_changed,
        ),
        AdaptiveLoopDisposition::Continue
    );
    let mut guidance_changed = evidence_changed.clone();
    let observation = guidance_changed.model_observation.as_mut().unwrap();
    observation.summary = "new semantic summary".to_string();
    observation.next_action = Some("inspect a different target".to_string());
    assert_eq!(
        observe_read(
            &mut cursor,
            "browser.extract_text",
            INPUT_A,
            &guidance_changed,
        ),
        AdaptiveLoopDisposition::Continue
    );
    assert_eq!(cursor.no_gain_count(), 0);
    assert!(!cursor.replan_emitted());
}

#[test]
fn effectful_and_untrusted_results_are_semantic_history_barriers() {
    let read = result_with_evidence(ToolOutcomeStatus::Succeeded, true, "stable content", &[]);
    let mut incomplete = read.clone();
    incomplete
        .model_observation
        .as_mut()
        .unwrap()
        .evidence_complete = false;
    let mut denied = read.clone();
    denied.status = ToolOutcomeStatus::Denied;
    let mut untyped = read.clone();
    untyped.model_observation = None;

    for barrier in [&incomplete, &denied, &untyped] {
        let mut cursor = AdaptiveLoopCursor::default();
        assert_eq!(
            observe_read(&mut cursor, "file.read", INPUT_A, &read),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(
            observe_read(&mut cursor, "file.read", INPUT_B, barrier),
            AdaptiveLoopDisposition::Continue
        );
        assert_eq!(
            observe_read(&mut cursor, "file.read", INPUT_A, &read),
            AdaptiveLoopDisposition::Continue
        );
    }

    let mut cursor = AdaptiveLoopCursor::default();
    observe_read(&mut cursor, "file.read", INPUT_A, &read);
    assert_eq!(
        cursor.observe_tool_result(
            0,
            "file.write",
            INPUT_B,
            &read,
            Some(&ToolEffectSemantics::NonIdempotent),
            None,
        ),
        AdaptiveLoopDisposition::Continue
    );
    assert_eq!(
        observe_read(&mut cursor, "file.read", INPUT_A, &read),
        AdaptiveLoopDisposition::Continue
    );
}

#[test]
fn digest_facts_identify_large_reads_and_invalid_or_missing_identity_fails_open() {
    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let mut cursor = AdaptiveLoopCursor::default();
    let large_a = result_with_evidence(
        ToolOutcomeStatus::Succeeded,
        true,
        "x".repeat(MAX_SEMANTIC_EVIDENCE_BYTES + 1),
        &[("page_sha256", DIGEST_A)],
    );
    let large_b = result_with_evidence(
        ToolOutcomeStatus::Succeeded,
        true,
        "x".repeat(MAX_SEMANTIC_EVIDENCE_BYTES + 1),
        &[("page_sha256", DIGEST_B)],
    );
    assert_eq!(
        observe_read(&mut cursor, "file.read", INPUT_A, &large_a),
        AdaptiveLoopDisposition::Continue
    );
    assert_eq!(
        observe_read(&mut cursor, "file.read", INPUT_A, &large_a),
        AdaptiveLoopDisposition::ReplanOnce
    );
    assert_eq!(
        observe_read(&mut cursor, "file.read", INPUT_A, &large_b),
        AdaptiveLoopDisposition::Continue
    );

    for unidentified in [
        result_with_evidence(ToolOutcomeStatus::Succeeded, true, "", &[]),
        result_with_evidence(
            ToolOutcomeStatus::Succeeded,
            true,
            "",
            &[(
                "page_sha256",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )],
        ),
        result_with_evidence(
            ToolOutcomeStatus::Succeeded,
            true,
            "x".repeat(MAX_SEMANTIC_EVIDENCE_BYTES + 1),
            &[("revision", "1")],
        ),
    ] {
        let mut cursor = AdaptiveLoopCursor::default();
        for _ in 0..4 {
            assert_eq!(
                observe_read(&mut cursor, "file.read", INPUT_A, &unidentified),
                AdaptiveLoopDisposition::Continue
            );
        }
        assert!(cursor.recent_semantic_observations.is_empty());
    }
}

#[test]
fn semantic_history_is_bounded_scoped_and_contains_no_raw_evidence() {
    let mut cursor = AdaptiveLoopCursor::default();
    for index in 0..=MAX_RECENT_SEMANTIC_ACTIONS {
        let evidence = format!("SECRET_EVIDENCE_{index}");
        let observation = result_with_evidence(ToolOutcomeStatus::Succeeded, true, &evidence, &[]);
        assert_eq!(
            observe_read(
                &mut cursor,
                "browser.extract_text",
                &format!("input-{index}"),
                &observation,
            ),
            AdaptiveLoopDisposition::Continue
        );
    }
    assert_eq!(
        cursor.recent_semantic_observations.len(),
        MAX_RECENT_SEMANTIC_ACTIONS
    );

    let first = result_with_evidence(ToolOutcomeStatus::Succeeded, true, "SECRET_EVIDENCE_0", &[]);
    assert_eq!(
        observe_read(&mut cursor, "browser.extract_text", "input-0", &first),
        AdaptiveLoopDisposition::Continue,
        "the ninth distinct action evicts the oldest identity"
    );
    assert_eq!(
        observe_read(&mut cursor, "browser.capture", "input-0", &first),
        AdaptiveLoopDisposition::Continue,
        "tool identity is part of the action scope"
    );

    let encoded = serde_json::to_string(&cursor).unwrap();
    assert!(encoded.len() < 4_096);
    assert!(!encoded.contains("SECRET_EVIDENCE"));
}
