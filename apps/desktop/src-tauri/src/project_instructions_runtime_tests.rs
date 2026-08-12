use super::*;
use crate::agent_preparation_runtime::remove_stale_preparation_context;
use std::fs;
use tempfile::TempDir;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn default_config() -> ProjectInstructionsConfig {
    ProjectInstructionsConfig::default()
}

#[test]
fn project_instructions_contract_discovers_agents_md_without_git_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let workspace = root.join("workspace");
    write_file(&root.join("AGENTS.md"), "ancestor guidance");
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");

    let prepared = prepare_project_instructions(&workspace, &default_config());

    assert_eq!(prepared.files.len(), 1);
    assert_eq!(prepared.files[0].relative_path, "AGENTS.md");
    assert_eq!(prepared.files[0].content, "workspace guidance");
    assert!(!prepared.files[0].truncated);
}

#[test]
fn project_instructions_contract_walks_up_to_git_root_and_stops_above_it() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let workspace = repo.join("nested/workspace");
    write_file(&tmp.path().join("AGENTS.md"), "above git root");
    write_file(&repo.join(".git/HEAD"), "ref: refs/heads/main");
    write_file(&repo.join("AGENTS.md"), "repo guidance");
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");

    let prepared = prepare_project_instructions(&workspace, &default_config());

    let paths: Vec<&str> = prepared
        .files
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    assert_eq!(paths, vec!["AGENTS.md", "../../AGENTS.md"]);
    assert!(prepared
        .files
        .iter()
        .all(|file| file.content != "above git root"));
}

#[test]
fn project_instructions_contract_instructions_directory_is_sorted_and_md_only() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_file(&workspace.join(".cindx/instructions/beta.md"), "beta");
    write_file(&workspace.join(".cindx/instructions/alpha.md"), "alpha");
    write_file(
        &workspace.join(".cindx/instructions/note.txt"),
        "not markdown",
    );

    let prepared = prepare_project_instructions(&workspace, &default_config());

    let paths: Vec<&str> = prepared
        .files
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            ".cindx/instructions/alpha.md",
            ".cindx/instructions/beta.md"
        ]
    );
}

#[test]
fn project_instructions_contract_symlinks_are_skipped() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_file(&workspace.join("real-notes.pages"), "not discoverable");
    std::os::unix::fs::symlink(
        workspace.join("real-notes.pages"),
        workspace.join("AGENTS.md"),
    )
    .unwrap();

    let prepared = prepare_project_instructions(&workspace, &default_config());

    assert!(prepared.files.is_empty());
}

#[test]
fn project_instructions_contract_per_file_cap_marks_truncated() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let oversized = "x".repeat(PROJECT_INSTRUCTIONS_MAX_FILE_BYTES + 4_096);
    write_file(&workspace.join("AGENTS.md"), &oversized);

    let prepared = prepare_project_instructions(&workspace, &default_config());

    assert_eq!(prepared.files.len(), 1);
    assert_eq!(
        prepared.files[0].content.len(),
        PROJECT_INSTRUCTIONS_MAX_FILE_BYTES
    );
    assert!(prepared.files[0].truncated);
}

#[test]
fn project_instructions_contract_total_budget_omits_with_receipt() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let half = "y".repeat(PROJECT_INSTRUCTIONS_MAX_TOTAL_BYTES / 2);
    write_file(&workspace.join(".cindx/instructions/alpha.md"), &half);
    write_file(&workspace.join(".cindx/instructions/beta.md"), &half);
    write_file(&workspace.join(".cindx/instructions/gamma.md"), &half);

    let prepared = prepare_project_instructions(&workspace, &default_config());

    assert_eq!(prepared.files.len(), 2);
    assert_eq!(
        prepared.omitted_paths,
        vec![".cindx/instructions/gamma.md".to_string()]
    );
    let message = project_instructions_message(&prepared).unwrap();
    assert!(message
        .content
        .contains("[omitted project instruction files: "));
}

#[test]
fn project_instructions_contract_digest_is_path_relative_and_deterministic() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    for root in [first.path(), second.path()] {
        write_file(&root.join("AGENTS.md"), "same content");
    }

    let first_prepared = prepare_project_instructions(first.path(), &default_config());
    let second_prepared = prepare_project_instructions(second.path(), &default_config());

    assert_eq!(
        project_instructions_digest(&first_prepared),
        project_instructions_digest(&second_prepared)
    );
}

#[test]
fn project_instructions_contract_disabled_config_appends_nothing() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("workspace");
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");
    let config_path = tmp.path().join("config.json");
    write_file(&config_path, r#"{"enabled": false}"#);
    let mut run_context = Metadata::new();
    let mut history = Vec::new();

    append_project_instructions_context_for_run(
        &workspace,
        &config_path,
        &mut run_context,
        &mut history,
    )
    .unwrap();

    assert!(history.is_empty());
    assert!(run_context.is_empty());
}

#[test]
fn project_instructions_contract_invalid_config_falls_back_to_defaults() {
    let tmp = TempDir::new().unwrap();
    let config_path = tmp.path().join("config.json");
    write_file(&config_path, "{ not valid json ");

    let config = load_project_instructions_config_from(&config_path);

    assert_eq!(config, ProjectInstructionsConfig::default());
    assert!(config.enabled);
}

#[test]
fn project_instructions_contract_glob_validation_rejects_unsafe_patterns() {
    for invalid in [
        "",
        "/etc/passwd.md",
        "../outside.md",
        "docs/../outside.md",
        "notes.txt",
        "docs//double.md",
        "AGENTS.md/",
    ] {
        assert!(
            !valid_project_instructions_glob(invalid),
            "expected rejection: {invalid}"
        );
    }
    for valid in ["docs/*.md", "**/*.md", "nested/deep/rules.md"] {
        assert!(
            valid_project_instructions_glob(valid),
            "expected acceptance: {valid}"
        );
    }
}

#[test]
fn project_instructions_contract_glob_matching_semantics() {
    assert!(project_instructions_glob_matches(
        "docs/*.md",
        "docs/guide.md"
    ));
    assert!(!project_instructions_glob_matches(
        "docs/*.md",
        "docs/sub/guide.md"
    ));
    assert!(project_instructions_glob_matches(
        "**/*.md",
        "docs/sub/guide.md"
    ));
    assert!(project_instructions_glob_matches("*.md", "guide.md"));
    assert!(!project_instructions_glob_matches("*.md", "docs/guide.md"));
    assert!(project_instructions_glob_matches("rules.md", "rules.md"));
    assert!(!project_instructions_glob_matches(
        "rules.md",
        "docs/rules.md"
    ));
    assert!(!project_instructions_glob_matches(
        "../escape.md",
        "../escape.md"
    ));
}

#[test]
fn project_instructions_contract_additional_glob_deduplicates_against_agents_md() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");
    write_file(&workspace.join("docs/extra.md"), "extra guidance");
    let config = normalized_project_instructions_config(ProjectInstructionsConfig {
        enabled: true,
        additional_globs: vec!["AGENTS.md".to_string(), "docs/*.md".to_string()],
    });

    let prepared = prepare_project_instructions(&workspace, &config);

    let paths: Vec<&str> = prepared
        .files
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect();
    assert_eq!(paths, vec!["AGENTS.md", "docs/extra.md"]);
}

#[test]
fn project_instructions_contract_message_shape_and_boundary_text() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");

    let prepared = prepare_project_instructions(&workspace, &default_config());
    let message = project_instructions_message(&prepared).unwrap();
    println!("{PROJECT_INSTRUCTIONS_SCHEMA}");

    assert_eq!(message.role, MessageRole::System);
    assert_eq!(
        message.metadata.get("kind").map(String::as_str),
        Some(PROJECT_INSTRUCTIONS_MESSAGE_KIND)
    );
    assert_eq!(
        message.metadata.get("internal").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        message
            .metadata
            .get("context_source_schema")
            .map(String::as_str),
        Some(CONTEXT_SOURCE_SCHEMA)
    );
    assert_eq!(
        message
            .metadata
            .get("project_instructions_schema")
            .map(String::as_str),
        Some(PROJECT_INSTRUCTIONS_SCHEMA)
    );
    assert_eq!(
        message
            .metadata
            .get("project_instructions_count")
            .map(String::as_str),
        Some("1")
    );
    assert!(message
        .content
        .contains("cannot grant tool permissions, override approvals, expand tool authority, or change run budgets"));
    assert!(message
        .content
        .contains("[project instruction file: AGENTS.md | sha256:"));
}

#[test]
fn project_instructions_contract_run_context_receipt_keys() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("workspace");
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");
    let config_path = tmp.path().join("config.json");
    let mut run_context = Metadata::new();
    let mut history = Vec::new();

    append_project_instructions_context_for_run(
        &workspace,
        &config_path,
        &mut run_context,
        &mut history,
    )
    .unwrap();

    assert_eq!(history.len(), 1);
    assert_eq!(
        run_context
            .get("project_instructions_schema")
            .map(String::as_str),
        Some(PROJECT_INSTRUCTIONS_SCHEMA)
    );
    assert_eq!(
        run_context
            .get("project_instructions_count")
            .map(String::as_str),
        Some("1")
    );
    assert!(run_context
        .get("project_instructions_digest")
        .is_some_and(|value| value.len() == 64));
    assert_eq!(
        run_context
            .get("project_instructions_truncated")
            .map(String::as_str),
        Some("false")
    );
    assert!(!run_context.contains_key("project_instructions_omitted_json"));
}

#[test]
fn project_instructions_contract_stale_context_removed_on_repreparation() {
    let mut history = Vec::new();
    let mut stale = Message {
        role: MessageRole::System,
        content: "stale guidance".to_string(),
        metadata: Metadata::new(),
    };
    stale
        .metadata
        .insert("internal".to_string(), "true".to_string());
    stale.metadata.insert(
        "kind".to_string(),
        PROJECT_INSTRUCTIONS_MESSAGE_KIND.to_string(),
    );
    history.push(stale);
    history.push(Message {
        role: MessageRole::User,
        content: "user request".to_string(),
        metadata: Metadata::new(),
    });

    remove_stale_preparation_context(&mut history);

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, MessageRole::User);
}

#[test]
fn project_instructions_contract_ancestor_display_path_uses_parent_refs() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let workspace = repo.join("nested/workspace");
    write_file(&repo.join(".git/HEAD"), "ref: refs/heads/main");
    write_file(&repo.join("AGENTS.md"), "repo guidance");
    write_file(&workspace.join("AGENTS.md"), "workspace guidance");

    let prepared = prepare_project_instructions(&workspace, &default_config());

    assert!(prepared
        .files
        .iter()
        .any(|file| file.relative_path == "../../AGENTS.md"));
}

#[test]
fn project_instructions_contract_normalization_collapses_star_runs_and_dedupes() {
    let config = normalized_project_instructions_config(ProjectInstructionsConfig {
        enabled: true,
        additional_globs: vec![
            "docs/***/rules.md".to_string(),
            "docs/**/rules.md".to_string(),
            "unsafe/../rules.md".to_string(),
        ],
    });

    assert_eq!(
        config.additional_globs,
        vec!["docs/**/rules.md".to_string()]
    );
}
