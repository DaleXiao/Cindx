use super::*;
use crate::runtime_values::phase16_task_id;
use agent_core::EventId;
use std::path::PathBuf;

const SESSION: &str = "session-undo-test";

fn event_sequence_store(database: &Path) -> SqliteStore {
    SqliteStore::open(database).expect("writable store should open")
}

fn tool_finished_event(
    sequence: u64,
    tool_call_id: &str,
    tool: &str,
    path: &str,
    action: &str,
    undo_before_path: Option<&str>,
    artifact_path: Option<&str>,
) -> Event {
    let mut metadata: agent_core::Metadata = [
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), tool.to_string()),
        ("status".to_string(), "succeeded".to_string()),
        ("result_path".to_string(), path.to_string()),
        ("result_undo_action".to_string(), action.to_string()),
        ("session_id".to_string(), SESSION.to_string()),
    ]
    .into_iter()
    .collect();
    if let Some(before) = undo_before_path {
        metadata.insert("result_undo_before_path".to_string(), before.to_string());
    }
    if let Some(artifact) = artifact_path {
        metadata.insert("result_artifact_path".to_string(), artifact.to_string());
    }
    Event {
        id: EventId(format!("event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::ToolCallFinished,
        summary: format!("Tool call finished: {tool}"),
        metadata,
    }
}

fn write_file(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent should be created");
    }
    fs::write(path, content).expect("file should be written");
}

struct MutationFixture {
    workspace: PathBuf,
    database: PathBuf,
}

impl MutationFixture {
    fn new(tag: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("cindx-undo-test-{tag}-{}", current_time_millis()));
        let workspace = base.join("workspace");
        let database = base.join("state.sqlite3");
        fs::create_dir_all(&workspace).expect("workspace should be created");
        Self {
            workspace,
            database,
        }
    }
}

struct MutationSpec<'a> {
    tool_call_id: &'a str,
    tool: &'a str,
    path: &'a str,
    action: &'a str,
    prior: Option<&'a [u8]>,
    after: &'a [u8],
}

fn seed_mutation(
    store: &mut SqliteStore,
    fixture: &MutationFixture,
    sequence: u64,
    spec: MutationSpec<'_>,
) {
    let MutationSpec {
        tool_call_id,
        tool,
        path,
        action,
        prior,
        after,
    } = spec;
    let target = fixture.workspace.join(path);
    write_file(&target, after);
    let artifact_relative = format!(".cindx/output-history/{tool_call_id}/{path}");
    write_file(&fixture.workspace.join(&artifact_relative), after);
    let undo_relative = prior.map(|_| format!(".cindx/undo-history/{tool_call_id}/{path}"));
    if let (Some(prior_bytes), Some(relative)) = (prior, undo_relative.as_ref()) {
        write_file(&fixture.workspace.join(relative), prior_bytes);
    }
    let event = tool_finished_event(
        sequence,
        tool_call_id,
        tool,
        path,
        action,
        undo_relative.as_deref(),
        Some(artifact_relative.as_str()),
    );
    store
        .append_next_event(
            event.id.clone(),
            event.task_id.clone(),
            event.timestamp_ms,
            event.kind,
            event.summary.clone(),
            event.metadata.clone(),
        )
        .expect("event should append");
}

#[test]
fn projection_orders_successful_file_mutations() {
    let events = vec![
        tool_finished_event(
            3,
            "call-b",
            "file.patch",
            "b.txt",
            "patched",
            Some("u/b.txt"),
            Some("o/b.txt"),
        ),
        tool_finished_event(
            1,
            "call-a",
            "file.write",
            "a.txt",
            "created",
            None,
            Some("o/a.txt"),
        ),
        tool_finished_event(
            2,
            "call-c",
            "shell.run",
            "ignored",
            "overwritten",
            None,
            None,
        ),
        {
            let mut failed = tool_finished_event(
                4,
                "call-d",
                "file.write",
                "d.txt",
                "overwritten",
                Some("u/d.txt"),
                Some("o/d.txt"),
            );
            failed
                .metadata
                .insert("status".to_string(), "failed".to_string());
            failed
        },
    ];

    let entries = project_workspace_undo_entries(&events);

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].tool_call_id, "call-a");
    assert_eq!(entries[0].action, "created");
    assert_eq!(entries[1].tool_call_id, "call-b");
    assert_eq!(entries[1].tool, "file.patch");
    assert_eq!(entries[1].undo_before_path.as_deref(), Some("u/b.txt"));
}

#[test]
fn undo_created_file_deletes_it() {
    let fixture = MutationFixture::new("created");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-create",
            tool: "file.write",
            path: "notes/new.txt",
            action: "created",
            prior: None,
            after: b"fresh content",
        },
    );

    let state = change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect("undo should succeed");

    assert!(!fixture.workspace.join("notes/new.txt").exists());
    assert!(state.can_redo);
    assert!(!state.can_undo);
}

#[test]
fn undo_overwritten_file_restores_prior_content() {
    let fixture = MutationFixture::new("overwrite");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-overwrite",
            tool: "file.write",
            path: "notes/today.txt",
            action: "overwritten",
            prior: Some(b"original version"),
            after: b"replacement version",
        },
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("notes/today.txt")).unwrap(),
        b"original version"
    );
}

#[test]
fn undo_patch_restores_prior_content() {
    let fixture = MutationFixture::new("patch");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-patch",
            tool: "file.patch",
            path: "src/lib.rs",
            action: "patched",
            prior: Some(b"fn old() {}"),
            after: b"fn new() {}",
        },
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("src/lib.rs")).unwrap(),
        b"fn old() {}"
    );
}

#[test]
fn patch_without_undo_snapshot_is_disclosed_not_undoable() {
    let fixture = MutationFixture::new("patch-disclosed");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-patch",
            tool: "file.patch",
            path: "src/lib.rs",
            action: "patched",
            prior: None,
            after: b"fn new() {}",
        },
    );

    let state = get_workspace_undo_state_for_session(&store, &fixture.workspace, SESSION)
        .expect("state should load");
    assert_eq!(state.entries.len(), 1);
    assert!(!state.entries[0].undoable);
    assert!(!state.can_undo);

    let error = change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect_err("undo should be refused when the capture failed");
    assert!(error.contains("no undo snapshot"));
    assert_eq!(
        fs::read(fixture.workspace.join("src/lib.rs")).unwrap(),
        b"fn new() {}"
    );
}

#[test]
fn redo_reapplies_undone_change() {
    let fixture = MutationFixture::new("redo");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-overwrite",
            tool: "file.write",
            path: "notes/today.txt",
            action: "overwritten",
            prior: Some(b"original version"),
            after: b"replacement version",
        },
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");
    let state = change_undo_stack(&mut store, &fixture.workspace, SESSION, true)
        .expect("redo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("notes/today.txt")).unwrap(),
        b"replacement version"
    );
    assert!(state.can_undo);
    assert!(!state.can_redo);
}

#[test]
fn undo_is_blocked_when_file_changed_externally() {
    let fixture = MutationFixture::new("conflict");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-overwrite",
            tool: "file.write",
            path: "notes/today.txt",
            action: "overwritten",
            prior: Some(b"original version"),
            after: b"replacement version",
        },
    );

    write_file(
        &fixture.workspace.join("notes/today.txt"),
        b"edited outside cindx",
    );

    let error = change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect_err("undo should be blocked");
    assert!(error.contains("changed outside this run"));
    assert_eq!(
        fs::read(fixture.workspace.join("notes/today.txt")).unwrap(),
        b"edited outside cindx"
    );
}

#[test]
fn nothing_to_undo_reports_a_clear_error() {
    let fixture = MutationFixture::new("empty");
    let mut store = event_sequence_store(&fixture.database);

    let error = change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect_err("empty undo stack should error");
    assert!(error.contains("nothing to undo"));
}

#[test]
fn undo_stack_follows_last_in_first_out_across_mutations() {
    let fixture = MutationFixture::new("lifo");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-first",
            tool: "file.write",
            path: "a.txt",
            action: "overwritten",
            prior: Some(b"a-original"),
            after: b"a-changed",
        },
    );
    seed_mutation(
        &mut store,
        &fixture,
        2,
        MutationSpec {
            tool_call_id: "call-second",
            tool: "file.write",
            path: "b.txt",
            action: "overwritten",
            prior: Some(b"b-original"),
            after: b"b-changed",
        },
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect("first undo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("b.txt")).unwrap(),
        b"b-original"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-changed"
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect("second undo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-original"
    );
}

struct BatchFileSpec<'a> {
    path: &'a str,
    prior: &'a [u8],
    after: &'a [u8],
}

fn seed_batch_mutation(
    store: &mut SqliteStore,
    fixture: &MutationFixture,
    sequence: u64,
    tool_call_id: &str,
    files: &[BatchFileSpec<'_>],
) {
    let group: Vec<serde_json::Value> = files
        .iter()
        .map(|file| {
            let target = fixture.workspace.join(file.path);
            write_file(&target, file.after);
            let artifact_relative = format!(".cindx/output-history/{tool_call_id}/{}", file.path);
            write_file(&fixture.workspace.join(&artifact_relative), file.after);
            let undo_relative = format!(".cindx/undo-history/{tool_call_id}/{}", file.path);
            write_file(&fixture.workspace.join(&undo_relative), file.prior);
            serde_json::json!({
                "path": file.path,
                "undo_before_path": undo_relative,
                "after_artifact_path": artifact_relative,
            })
        })
        .collect();
    let display = files
        .iter()
        .map(|file| file.path)
        .collect::<Vec<_>>()
        .join(", ");
    let metadata: agent_core::Metadata = [
        ("tool_call_id".to_string(), tool_call_id.to_string()),
        ("tool".to_string(), "file.patch_batch".to_string()),
        ("status".to_string(), "succeeded".to_string()),
        ("result_path".to_string(), display),
        ("result_undo_action".to_string(), "patched".to_string()),
        (
            "result_undo_group".to_string(),
            serde_json::to_string(&group).expect("group should encode"),
        ),
        ("session_id".to_string(), SESSION.to_string()),
    ]
    .into_iter()
    .collect();
    let event = Event {
        id: EventId(format!("event-{sequence}")),
        task_id: phase16_task_id(),
        sequence,
        timestamp_ms: sequence * 10,
        kind: EventKind::ToolCallFinished,
        summary: "Tool call finished: file.patch_batch".to_string(),
        metadata,
    };
    store
        .append_next_event(
            event.id.clone(),
            event.task_id.clone(),
            event.timestamp_ms,
            event.kind,
            event.summary.clone(),
            event.metadata.clone(),
        )
        .expect("event should append");
}

#[test]
fn batch_patch_projects_as_one_group_undo_entry() {
    let fixture = MutationFixture::new("batch-project");
    let mut store = event_sequence_store(&fixture.database);
    seed_batch_mutation(
        &mut store,
        &fixture,
        1,
        "call-batch",
        &[
            BatchFileSpec {
                path: "a.txt",
                prior: b"a-original",
                after: b"a-patched",
            },
            BatchFileSpec {
                path: "dir/b.txt",
                prior: b"b-original",
                after: b"b-patched",
            },
        ],
    );
    let events = agent_events_for_session(&store, &phase16_task_id(), Some(SESSION))
        .expect("events should load");

    let entries = project_workspace_undo_entries(&events);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].tool, "file.patch_batch");
    assert_eq!(entries[0].action, "patched");
    assert_eq!(entries[0].files.len(), 2);
    assert_eq!(entries[0].files[1].path, "dir/b.txt");
    let state = get_workspace_undo_state_for_session(&store, &fixture.workspace, SESSION)
        .expect("state should load");
    assert!(state.can_undo);
    assert!(state.entries[0].undoable);
}

#[test]
fn undo_batch_restores_the_whole_group() {
    let fixture = MutationFixture::new("batch-undo");
    let mut store = event_sequence_store(&fixture.database);
    seed_batch_mutation(
        &mut store,
        &fixture,
        1,
        "call-batch",
        &[
            BatchFileSpec {
                path: "a.txt",
                prior: b"a-original",
                after: b"a-patched",
            },
            BatchFileSpec {
                path: "dir/b.txt",
                prior: b"b-original",
                after: b"b-patched",
            },
        ],
    );

    let state = change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect("group undo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-original"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"b-original"
    );
    assert!(!state.can_undo);
    assert!(state.can_redo);
}

#[test]
fn batch_undo_is_blocked_when_any_group_file_changed_externally() {
    let fixture = MutationFixture::new("batch-conflict");
    let mut store = event_sequence_store(&fixture.database);
    seed_batch_mutation(
        &mut store,
        &fixture,
        1,
        "call-batch",
        &[
            BatchFileSpec {
                path: "a.txt",
                prior: b"a-original",
                after: b"a-patched",
            },
            BatchFileSpec {
                path: "dir/b.txt",
                prior: b"b-original",
                after: b"b-patched",
            },
        ],
    );
    write_file(
        &fixture.workspace.join("dir/b.txt"),
        b"edited outside cindx",
    );

    let error = change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect_err("group undo should be blocked");

    assert!(error.contains("changed outside this run"));
    // The failed group undo restores nothing, not even the untouched file.
    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-patched"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"edited outside cindx"
    );
}

#[test]
fn redo_reapplies_the_whole_batch_group() {
    let fixture = MutationFixture::new("batch-redo");
    let mut store = event_sequence_store(&fixture.database);
    seed_batch_mutation(
        &mut store,
        &fixture,
        1,
        "call-batch",
        &[
            BatchFileSpec {
                path: "a.txt",
                prior: b"a-original",
                after: b"a-patched",
            },
            BatchFileSpec {
                path: "dir/b.txt",
                prior: b"b-original",
                after: b"b-patched",
            },
        ],
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");
    let state = change_undo_stack(&mut store, &fixture.workspace, SESSION, true)
        .expect("group redo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-patched"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"b-patched"
    );
    assert!(state.can_undo);
    assert!(!state.can_redo);
}

#[test]
fn undo_registry_survives_store_reopen() {
    println!("{WORKSPACE_UNDO_SCHEMA}");
    let fixture = MutationFixture::new("persist");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-overwrite",
            tool: "file.write",
            path: "notes/today.txt",
            action: "overwritten",
            prior: Some(b"original version"),
            after: b"replacement version",
        },
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");
    drop(store);

    let reopened = event_sequence_store(&fixture.database);
    let state = get_workspace_undo_state_for_session(&reopened, &fixture.workspace, SESSION)
        .expect("state should load");
    assert!(!state.can_undo);
    assert!(state.can_redo);
    assert_eq!(state.entries.len(), 1);
    assert!(state.entries[0].undone);
}

#[test]
fn projection_carries_the_run_attribution_for_thread_attachment() {
    let mut first = tool_finished_event(1, "call-a", "file.write", "a.txt", "created", None, None);
    first
        .metadata
        .insert("agent_run_id".to_string(), "run-a".to_string());
    let second = tool_finished_event(
        2,
        "call-b",
        "file.patch",
        "b.txt",
        "patched",
        Some("u/b.txt"),
        None,
    );

    let entries = project_workspace_undo_entries(&[first, second]);

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].run_id.as_deref(), Some("run-a"));
    assert_eq!(
        entries[1].run_id, None,
        "legacy events without attribution stay unattributed"
    );
}

#[test]
#[cfg(unix)]
fn undo_group_write_failure_rolls_back_already_restored_files() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = MutationFixture::new("batch-rollback");
    let mut store = event_sequence_store(&fixture.database);
    seed_batch_mutation(
        &mut store,
        &fixture,
        1,
        "call-batch",
        &[
            BatchFileSpec {
                path: "a.txt",
                prior: b"a-original",
                after: b"a-patched",
            },
            BatchFileSpec {
                path: "dir/b.txt",
                prior: b"b-original",
                after: b"b-patched",
            },
        ],
    );

    // Make the second file's directory unwritable: group validation still
    // passes (reads and hashes work), but restoring b.txt fails mid-group.
    let dir = fixture.workspace.join("dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("dir should chmod");
    let result = change_undo_stack(&mut store, &fixture.workspace, SESSION, false);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("dir should restore");

    let error = result.expect_err("group undo should fail on the unwritable file");
    assert!(
        error.contains("failed to restore"),
        "unexpected error: {error}"
    );

    // The rollback leaves the whole group in its applied state...
    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-patched"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"b-patched"
    );
    // ...and the registry never recorded the failed undo, so a retry works.
    let state = get_workspace_undo_state_for_session(&store, &fixture.workspace, SESSION)
        .expect("state should load");
    assert!(state.can_undo);
    assert!(!state.can_redo);

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false)
        .expect("retry after rollback should succeed");
    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-original"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"b-original"
    );
}

#[test]
fn undo_publishes_files_without_leaving_temp_artifacts() {
    let fixture = MutationFixture::new("atomic-undo");
    let mut store = event_sequence_store(&fixture.database);
    seed_mutation(
        &mut store,
        &fixture,
        1,
        MutationSpec {
            tool_call_id: "call-atomic",
            tool: "file.patch",
            path: "notes.md",
            action: "patched",
            prior: Some(b"before"),
            after: b"after",
        },
    );

    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");

    assert_eq!(
        fs::read(fixture.workspace.join("notes.md")).unwrap(),
        b"before"
    );
    let leftovers: Vec<_> = fs::read_dir(&fixture.workspace)
        .expect("workspace should list")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files leaked: {leftovers:?}");
}

#[test]
#[cfg(unix)]
fn redo_group_write_failure_rolls_back_reapplied_files() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = MutationFixture::new("batch-redo-rollback");
    let mut store = event_sequence_store(&fixture.database);
    seed_batch_mutation(
        &mut store,
        &fixture,
        1,
        "call-batch",
        &[
            BatchFileSpec {
                path: "a.txt",
                prior: b"a-original",
                after: b"a-patched",
            },
            BatchFileSpec {
                path: "dir/b.txt",
                prior: b"b-original",
                after: b"b-patched",
            },
        ],
    );
    change_undo_stack(&mut store, &fixture.workspace, SESSION, false).expect("undo should succeed");

    let dir = fixture.workspace.join("dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("dir should chmod");
    let result = change_undo_stack(&mut store, &fixture.workspace, SESSION, true);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("dir should restore");

    let error = result.expect_err("group redo should fail on the unwritable file");
    assert!(
        error.contains("failed to restore"),
        "unexpected error: {error}"
    );

    // The rollback leaves the whole group in its undone state...
    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-original"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"b-original"
    );
    // ...and the registry still lists the entry as undone, so redo retries.
    let state = get_workspace_undo_state_for_session(&store, &fixture.workspace, SESSION)
        .expect("state should load");
    assert!(!state.can_undo);
    assert!(state.can_redo);

    change_undo_stack(&mut store, &fixture.workspace, SESSION, true)
        .expect("redo retry after rollback should succeed");
    assert_eq!(
        fs::read(fixture.workspace.join("a.txt")).unwrap(),
        b"a-patched"
    );
    assert_eq!(
        fs::read(fixture.workspace.join("dir/b.txt")).unwrap(),
        b"b-patched"
    );
}
