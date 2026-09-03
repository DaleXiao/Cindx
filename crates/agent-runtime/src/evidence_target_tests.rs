use super::*;

#[test]
fn workspace_filename_does_not_accept_an_unrelated_file() {
    let anchors = evidence_target_anchors("Audit permission.rs.");

    assert!(anchors.contains(&EvidenceTargetAnchor::Workspace(
        "permission.rs".to_string()
    )));
    assert!(evidence_input_matches_anchors(
        r#"{"path":"apps/desktop/src-tauri/src/permission.rs"}"#,
        &anchors
    ));
    assert!(!evidence_input_matches_anchors(
        r#"{"path":"README.md"}"#,
        &anchors
    ));
}

#[test]
fn external_subjects_require_conservative_query_overlap() {
    let anchors = evidence_target_anchors(
        "Search the web for Rust async cancellation semantics and report the result.",
    );

    assert!(anchors.contains(&EvidenceTargetAnchor::ExternalSubject("rust".to_string())));
    assert!(evidence_input_matches_anchors(
        r#"{"query":"Rust async cancellation"}"#,
        &anchors
    ));
    assert!(!evidence_input_matches_anchors(
        r#"{"query":"Python package indexes"}"#,
        &anchors
    ));
    assert!(!evidence_input_matches_anchors(
        r#"{"query":"Rust package manager"}"#,
        &anchors
    ));
}

#[test]
fn explicit_url_requires_the_same_url_target() {
    let anchors = evidence_target_anchors(
        "Open https://docs.rs/tokio/latest/tokio/ and verify the documented behavior.",
    );

    assert!(evidence_input_matches_anchors(
        r#"{"url":"https://docs.rs/tokio/latest/tokio/"}"#,
        &anchors
    ));
    assert!(!evidence_input_matches_anchors(
        r#"{"url":"https://example.com/tokio"}"#,
        &anchors
    ));
}

#[test]
fn chinese_external_request_without_explicit_anchor_stays_compatible() {
    let anchors = evidence_target_anchors("请联网查询量子纠缠的最新进展并给出结论");

    assert!(anchors.is_empty());
    assert!(evidence_input_matches_anchors(
        r#"{"query":"任意保守查询"}"#,
        &anchors
    ));
}

#[test]
fn self_contained_material_is_not_mistaken_for_a_target() {
    let anchors = evidence_target_anchors(
        "Summarize only the self-contained material below; do not search the web.\n```text\nRust permission.rs https://example.com/reference\n```",
    );

    assert!(anchors.is_empty());
}

#[test]
fn explicit_relative_path_matches_an_absolute_tool_input() {
    let anchors = evidence_target_anchors(
        "Inspect crates/agent-runtime/src/task_contract.rs and explain the completion gate.",
    );

    assert!(evidence_input_matches_anchors(
        r#"{"path":"/Users/example/Cindx/crates/agent-runtime/src/task_contract.rs"}"#,
        &anchors
    ));
    assert!(!evidence_input_matches_anchors(
        r#"{"path":"/Users/example/Cindx/crates/agent-runtime/src/lib.rs"}"#,
        &anchors
    ));
}

#[test]
fn unicode_before_a_material_boundary_never_breaks_target_extraction() {
    let anchors =
        evidence_target_anchors("请审查 permission.rs。内容如下\n```text\nREADME.md\n```");

    assert_eq!(
        anchors,
        BTreeSet::from([EvidenceTargetAnchor::Workspace("permission.rs".to_string())])
    );
}

#[test]
fn generic_external_words_do_not_overconstrain_a_safe_query() {
    let anchors = evidence_target_anchors("Search the web for the latest Rust release version.");

    assert_eq!(
        anchors,
        BTreeSet::from([EvidenceTargetAnchor::ExternalSubject("rust".to_string())])
    );
    assert!(evidence_input_matches_anchors(
        r#"{"query":"Rust 1.90 announcement"}"#,
        &anchors
    ));
}

#[test]
fn persisted_target_witness_is_bound_to_epoch_tool_input_and_anchor_set() {
    let anchors = evidence_target_anchors(
        "Open https://docs.rs/tokio/latest/tokio/ and verify the documented behavior.",
    );
    let input = r#"{"url":"https://docs.rs/tokio/latest/tokio/"}"#;
    let input_fingerprint = crate::tool_input_fingerprint("browser.open", input);
    let witness = evidence_target_witness(input, &anchors, "browser.open", &input_fingerprint, 4)
        .expect("matching raw input should create a witness");
    let receipt = serde_json::json!({
        "permission_input_fingerprint": input_fingerprint,
        "evidence_target_witness": witness,
    })
    .to_string();

    assert!(evidence_target_witness_matches(
        &receipt,
        &anchors,
        "browser.open",
        4
    ));
    assert!(!evidence_target_witness_matches(
        &receipt,
        &anchors,
        "browser.open",
        5
    ));
    assert!(!evidence_target_witness_matches(
        &receipt,
        &anchors,
        "web.search",
        4
    ));
}

#[test]
fn a_line_reference_at_the_end_of_a_sentence_never_leaks_into_the_target() {
    // The punctuation that ends a clause must not defeat the `:line` strip: a
    // polluted target (`src/config.rs:12`) matches no real tool input, so the
    // obligation it anchors could never be satisfied, and the same parse is what
    // answer-citation binding relies on (P1-10).
    let anchors = evidence_target_anchors("Confirm the fix in src/config.rs:12.");

    assert!(anchors.contains(&EvidenceTargetAnchor::Workspace(
        "src/config.rs".to_string()
    )));
    assert!(!anchors.iter().any(|anchor| matches!(
        anchor,
        EvidenceTargetAnchor::Workspace(path) if path.contains(':')
    )));
    assert!(evidence_input_matches_anchors(
        r#"{"path":"src/config.rs"}"#,
        &anchors
    ));

    let ranged = evidence_target_anchors("See crates/lib.rs:40-58, then stop.");
    assert!(ranged.contains(&EvidenceTargetAnchor::Workspace(
        "crates/lib.rs".to_string()
    )));

    // The same tokens expose their line reference to citation binding.
    assert_eq!(
        token_workspace_path_and_line("src/config.rs:12."),
        Some(("src/config.rs".to_string(), Some(12), None))
    );
    assert_eq!(
        token_workspace_path_and_line("crates/lib.rs:40-58,"),
        Some(("crates/lib.rs".to_string(), Some(40), Some(58)))
    );
    assert_eq!(
        token_workspace_path_and_line("docs/CURRENT.md#L12"),
        Some(("docs/current.md".to_string(), Some(12), None))
    );
    // A bare time is not a path with a line reference.
    assert_eq!(token_workspace_path_and_line("12:30"), None);
}
