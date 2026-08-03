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
