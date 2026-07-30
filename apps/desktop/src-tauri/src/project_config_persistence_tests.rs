use super::*;

fn config_for(root: &Path, suffix: &str) -> ProjectSessionConfig {
    let project_id = format!("project-{suffix}");
    let session_id = format!("session-{suffix}");
    ProjectSessionConfig {
        active_project_id: project_id.clone(),
        active_session_id: session_id.clone(),
        projects: vec![ProjectRecord {
            id: project_id.clone(),
            name: format!("Project {suffix}"),
            root: root.display().to_string(),
            detail: "workspace project".to_string(),
            created_at_ms: 10,
            updated_at_ms: 20,
        }],
        sessions: vec![SessionRecord {
            id: session_id,
            project_id,
            name: format!("Session {suffix}"),
            title_state: SessionTitleState::Manual,
            detail: "timeline + chat".to_string(),
            effort: "auto".to_string(),
            seen_event_sequence: 9,
            created_at_ms: 11,
            updated_at_ms: 21,
            archived_at_ms: None,
        }],
    }
}

fn assert_no_staging_files(directory: &Path, target_name: &str) {
    let prefix = format!(".{target_name}.");
    let leftovers = fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "staging files remain: {leftovers:?}");
}

#[test]
fn atomic_workspace_replace_publishes_complete_private_file() {
    let directory = tempfile::tempdir().unwrap();
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).unwrap();
    let path = directory.path().join("workspace.conf");
    fs::write(&path, b"root=old\ntruncated-data").unwrap();

    save_workspace_config_to_path(
        &path,
        &WorkspaceConfig {
            root: workspace_root.clone(),
        },
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("root={}\n", workspace_root.display())
    );
    assert_no_staging_files(directory.path(), "workspace.conf");
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn failed_atomic_publish_preserves_existing_file_and_cleans_staging() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("projects.conf");
    let original = b"active_project_id=old\nactive_session_id=old\n";
    fs::write(&path, original).unwrap();

    let result = write_private_file_atomically_with(
        &path,
        b"active_project_id=new\nactive_session_id=new\n",
        |_temporary, _target| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected publish failure",
            ))
        },
    );

    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_no_staging_files(directory.path(), "projects.conf");
}

#[test]
fn project_session_commit_round_trips_before_publishing_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let old_root = directory.path().join("old");
    let new_root = directory.path().join("new");
    fs::create_dir(&old_root).unwrap();
    fs::create_dir(&new_root).unwrap();
    let path = directory.path().join("projects.conf");
    let mut current = config_for(&old_root, "old");
    let candidate = config_for(&new_root, "new");

    commit_project_session_config_to_path(&path, &mut current, candidate.clone()).unwrap();

    assert_eq!(current, candidate);
    assert_eq!(
        load_project_session_config_from_path(&path, &new_root),
        candidate
    );
    assert_no_staging_files(directory.path(), "projects.conf");
}

#[test]
fn active_project_root_overrides_divergent_workspace_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let fallback_root = directory.path().join("fallback");
    let project_root = directory.path().join("project");
    fs::create_dir(&fallback_root).unwrap();
    fs::create_dir(&project_root).unwrap();
    let mut workspace = WorkspaceConfig {
        root: fallback_root,
    };
    let projects = config_for(&project_root, "authoritative");

    apply_authoritative_project_root(&mut workspace, &projects);

    assert_eq!(workspace.root, fs::canonicalize(project_root).unwrap());
}

#[test]
fn strict_commits_keep_current_values_when_publish_fails() {
    let directory = tempfile::tempdir().unwrap();
    let old_root = directory.path().join("old");
    let new_root = directory.path().join("new");
    fs::create_dir(&old_root).unwrap();
    fs::create_dir(&new_root).unwrap();

    let workspace_target = directory.path().join("workspace.conf");
    fs::create_dir(&workspace_target).unwrap();
    let mut workspace = WorkspaceConfig {
        root: old_root.clone(),
    };
    let workspace_candidate = WorkspaceConfig {
        root: new_root.clone(),
    };
    assert!(commit_workspace_config_to_path(
        &workspace_target,
        &mut workspace,
        workspace_candidate,
    )
    .is_err());
    assert_eq!(workspace.root, old_root);
    assert!(workspace_target.is_dir());
    assert_no_staging_files(directory.path(), "workspace.conf");

    let project_target = directory.path().join("projects.conf");
    fs::create_dir(&project_target).unwrap();
    let mut projects = config_for(&workspace.root, "old");
    let original = projects.clone();
    assert!(commit_project_session_config_to_path(
        &project_target,
        &mut projects,
        config_for(&new_root, "new"),
    )
    .is_err());
    assert_eq!(projects, original);
    assert!(project_target.is_dir());
    assert_no_staging_files(directory.path(), "projects.conf");
}

#[test]
fn workspace_cache_failure_still_publishes_authoritative_memory_value() {
    let directory = tempfile::tempdir().unwrap();
    let old_root = directory.path().join("old");
    let new_root = directory.path().join("new");
    fs::create_dir(&old_root).unwrap();
    fs::create_dir(&new_root).unwrap();
    let path = directory.path().join("workspace.conf");
    fs::create_dir(&path).unwrap();
    let mut current = WorkspaceConfig { root: old_root };
    let candidate = WorkspaceConfig {
        root: new_root.clone(),
    };

    publish_workspace_config_cache_to_path(&path, &mut current, candidate);

    assert_eq!(current.root, new_root);
    assert!(path.is_dir());
    assert_no_staging_files(directory.path(), "workspace.conf");
}
