use super::*;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent should be created");
    }
    fs::write(path, content).expect("file should be written");
}

#[test]
fn custom_commands_contract_parses_frontmatter_fields() {
    let command = parse_custom_command_file(
        "run-tests",
        "project",
        "---\ndescription: Run the full test suite\neffort: FAST\n---\nRun $ARGUMENTS now.",
    );

    assert_eq!(command.name, "run-tests");
    assert_eq!(command.description, "Run the full test suite");
    assert_eq!(command.effort.as_deref(), Some("fast"));
    assert_eq!(command.template, "Run $ARGUMENTS now.");
    assert_eq!(command.scope, "project");
}

#[test]
fn custom_commands_contract_missing_frontmatter_uses_full_text() {
    let command = parse_custom_command_file("plain", "global", "Summarize the workspace.");

    assert_eq!(command.description, "");
    assert_eq!(command.effort, None);
    assert_eq!(command.template, "Summarize the workspace.");
}

#[test]
fn custom_commands_contract_invalid_effort_is_dropped() {
    let command = parse_custom_command_file(
        "invalid-effort",
        "project",
        "---\neffort: turbo\n---\nBody.",
    );

    assert_eq!(command.effort, None);
    assert_eq!(command.template, "Body.");
}

#[test]
fn custom_commands_contract_unclosed_frontmatter_is_treated_as_text() {
    let text = "---\ndescription: never closed\nBody stays whole.";
    let command = parse_custom_command_file("broken", "project", text);

    assert_eq!(command.description, "");
    assert_eq!(command.template, text.trim());
}

#[test]
fn custom_commands_contract_discovers_sorted_markdown_only_files() {
    let workspace = tempfile::tempdir().unwrap();
    let commands_dir = commands_directory(workspace.path());
    write_file(&commands_dir.join("beta.md"), "Beta body.");
    write_file(&commands_dir.join("alpha.md"), "Alpha body.");
    write_file(&commands_dir.join("notes.txt"), "Not markdown.");

    let commands = discover_custom_commands_in(&commands_dir, "project");

    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].name, "alpha");
    assert_eq!(commands[1].name, "beta");
}

#[test]
fn custom_commands_contract_project_scope_overrides_global_names() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    write_file(
        &commands_directory(workspace.path()).join("deploy.md"),
        "Project deploy.",
    );
    write_file(
        &commands_directory(global.path()).join("deploy.md"),
        "Global deploy.",
    );
    write_file(
        &commands_directory(global.path()).join("extra.md"),
        "Global only.",
    );

    let commands = discover_custom_commands(workspace.path(), global.path());

    assert_eq!(commands.len(), 2);
    let deploy = commands.iter().find(|c| c.name == "deploy").unwrap();
    assert_eq!(deploy.template, "Project deploy.");
    assert_eq!(deploy.scope, "project");
    let extra = commands.iter().find(|c| c.name == "extra").unwrap();
    assert_eq!(extra.scope, "global");
}

#[test]
fn custom_commands_contract_file_bytes_are_capped() {
    let workspace = tempfile::tempdir().unwrap();
    let oversized = "x".repeat(MAX_CUSTOM_COMMAND_FILE_BYTES + 4_096);
    write_file(
        &commands_directory(workspace.path()).join("big.md"),
        &oversized,
    );

    let commands = discover_custom_commands_in(&commands_directory(workspace.path()), "project");

    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].template.len(), MAX_CUSTOM_COMMAND_FILE_BYTES);
}

#[test]
fn custom_commands_contract_file_count_is_capped() {
    let workspace = tempfile::tempdir().unwrap();
    let commands_dir = commands_directory(workspace.path());
    for index in 0..(MAX_CUSTOM_COMMAND_FILES + 3) {
        write_file(
            &commands_dir.join(format!("cmd-{index:02}.md")),
            &format!("Body {index}."),
        );
    }

    let commands = discover_custom_commands_in(&commands_dir, "project");

    assert_eq!(commands.len(), MAX_CUSTOM_COMMAND_FILES);
}

#[test]
fn custom_commands_contract_missing_directories_yield_no_commands() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();

    let commands = discover_custom_commands(workspace.path(), global.path());

    assert!(commands.is_empty());
}

#[test]
fn custom_commands_contract_effort_uses_canonical_tiers_and_migrates_legacy() {
    for (frontmatter, expected) in [
        ("fast", Some("fast")),
        ("default", Some("default")),
        ("high", Some("high")),
        ("xhigh", Some("xhigh")),
        // Legacy labels migrate at the parse boundary; the stored view is
        // canonical so the frontend tier switch always recognizes it.
        ("auto", Some("default")),
        ("pro", Some("high")),
        ("turbo", None),
    ] {
        let command = parse_custom_command_file(
            "tier-probe",
            "project",
            &format!("---\neffort: {frontmatter}\n---\nBody."),
        );
        assert_eq!(
            command.effort.as_deref(),
            expected,
            "effort '{frontmatter}' must normalize to {expected:?}"
        );
    }
}
