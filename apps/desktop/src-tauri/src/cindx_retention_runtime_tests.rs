use crate::cindx_retention_runtime::{
    apply_cindx_retention, measure_cindx_usage, plan_cindx_retention, run_cindx_retention_sweep,
    CindxFile, CindxUsage, CINDX_AGGREGATE_QUOTA_BYTES, CINDX_RETENTION_TTL_MS,
};
use std::fs;
use std::path::{Path, PathBuf};

fn write_file(path: &Path, bytes: usize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent directory should be created");
    }
    fs::write(path, vec![b'x'; bytes]).expect("fixture file should write");
}

fn candidate(relative: &str, bytes: u64, modified_ms: u64) -> CindxFile {
    CindxFile {
        relative: PathBuf::from(relative),
        bytes,
        modified_ms,
    }
}

#[test]
fn an_absent_cindx_tree_measures_as_empty_and_plans_nothing() {
    let workspace = tempfile::tempdir().expect("temp workspace");

    let usage = measure_cindx_usage(workspace.path());
    let plan = plan_cindx_retention(&usage, 1_000_000);

    assert_eq!(usage, CindxUsage::default());
    assert_eq!(plan.expired.len(), 0);
    assert_eq!(plan.evicted.len(), 0);
    assert!(!plan.quota_exceeded);
    assert_eq!(plan.bytes_reclaimed, 0);
}

#[test]
fn aged_regenerable_classes_expire_and_semantic_state_never_does() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let root = workspace.path();
    // Regenerable recovery and capture classes.
    write_file(&root.join(".cindx/undo-history/session-a/v1/src/a.rs"), 40);
    write_file(&root.join(".cindx/output-history/session-a/v1/a.rs"), 41);
    write_file(&root.join(".cindx/tool-output/hash-a/stdout.txt"), 42);
    write_file(&root.join(".cindx/screenshots/shot.png"), 43);
    write_file(&root.join(".cindx/browser-captures/page.txt"), 44);
    // Semantic state: never a candidate, however old.
    write_file(
        &root.join(".cindx/knowledge-generations/gen-1/rag-index.tsv"),
        900,
    );
    write_file(&root.join(".cindx/instructions/guide.md"), 901);
    write_file(&root.join(".cindx/commands/deploy.md"), 902);
    write_file(&root.join(".cindx/todos.json"), 903);
    write_file(&root.join(".cindx/context/session-a.md"), 904);
    write_file(&root.join(".cindx/agent-trace.jsonl"), 905);

    let usage = measure_cindx_usage(root);
    assert_eq!(usage.files.len(), 5, "candidates were {:?}", usage.files);
    assert_eq!(usage.eligible_bytes, 40 + 41 + 42 + 43 + 44);
    assert!(
        usage.total_bytes >= usage.eligible_bytes + 900 + 901 + 902 + 903 + 904 + 905,
        "the aggregate counts semantic state too: {}",
        usage.total_bytes
    );
    assert!(!usage.truncated);

    // Age the tree by moving the clock, not the files: the planner is pure over
    // (mtime, now), so a future `now` is the same decision an old file forces.
    let future_ms = usage.files[0].modified_ms + CINDX_RETENTION_TTL_MS + 60_000;
    let plan = plan_cindx_retention(&usage, future_ms);
    assert_eq!(plan.expired.len(), 5);
    assert!(plan.evicted.is_empty());
    assert!(!plan.quota_exceeded);

    let outcome = run_cindx_retention_sweep(root, future_ms);
    assert_eq!(outcome.removed, 5);
    assert!(
        outcome.failures.is_empty(),
        "failures were {:?}",
        outcome.failures
    );
    assert!(!root.join(".cindx/undo-history").exists());
    assert!(!root.join(".cindx/screenshots/shot.png").exists());
    // Every semantic store survived, byte for byte.
    assert!(root
        .join(".cindx/knowledge-generations/gen-1/rag-index.tsv")
        .is_file());
    assert!(root.join(".cindx/instructions/guide.md").is_file());
    assert!(root.join(".cindx/commands/deploy.md").is_file());
    assert!(root.join(".cindx/todos.json").is_file());
    assert!(root.join(".cindx/context/session-a.md").is_file());
    assert!(root.join(".cindx/agent-trace.jsonl").is_file());
    assert_eq!(
        fs::read(root.join(".cindx/instructions/guide.md"))
            .expect("guide should read")
            .len(),
        901
    );
    // The `.cindx` root itself is never removed.
    assert!(root.join(".cindx").is_dir());

    // A fresh tree does not expire: a recent snapshot is still undoable.
    let fresh = measure_cindx_usage(root);
    assert!(fresh.files.is_empty());
    write_file(&root.join(".cindx/undo-history/session-b/v1/src/b.rs"), 10);
    let fresh = measure_cindx_usage(root);
    let now_ms = fresh.files[0].modified_ms;
    let plan = plan_cindx_retention(&fresh, now_ms);
    assert!(plan.expired.is_empty(), "a recent snapshot must survive");
    assert!(plan.evicted.is_empty());
}

#[test]
fn over_quota_trees_evict_the_oldest_candidates_first_and_stop_when_they_fit() {
    let per_file = CINDX_AGGREGATE_QUOTA_BYTES / 4 + 1;
    let usage = CindxUsage {
        total_bytes: per_file * 5,
        eligible_bytes: per_file * 5,
        files: vec![
            candidate("undo-history/newest.rs", per_file, 5_000),
            candidate("undo-history/oldest.rs", per_file, 1_000),
            candidate("tool-output/middle.rs", per_file, 3_000),
            candidate("output-history/older.rs", per_file, 2_000),
            candidate("screenshots/newest.png", per_file, 4_000),
        ],
        entries_visited: 5,
        truncated: false,
    };
    // Nothing is old enough to expire, so the quota alone drives the decision.
    let plan = plan_cindx_retention(&usage, 5_001);

    assert!(plan.expired.is_empty());
    assert!(plan.quota_exceeded);
    assert_eq!(
        plan.evicted,
        vec![
            PathBuf::from("undo-history/oldest.rs"),
            PathBuf::from("output-history/older.rs"),
        ],
        "eviction is oldest-first and stops as soon as the tree fits"
    );
    assert_eq!(plan.bytes_reclaimed, per_file * 2);
}

#[test]
fn expiry_runs_before_eviction_so_an_aged_tree_does_not_evict_fresh_files() {
    let per_file = CINDX_AGGREGATE_QUOTA_BYTES / 2 + 1;
    let aged_ms = 1_000u64;
    let fresh_ms = aged_ms + CINDX_RETENTION_TTL_MS;
    let usage = CindxUsage {
        total_bytes: per_file * 3,
        eligible_bytes: per_file * 3,
        files: vec![
            candidate("undo-history/aged-a.rs", per_file, aged_ms),
            candidate("undo-history/aged-b.rs", per_file, aged_ms),
            candidate("undo-history/fresh.rs", per_file, fresh_ms),
        ],
        entries_visited: 3,
        truncated: false,
    };

    let plan = plan_cindx_retention(&usage, fresh_ms + 1);

    assert_eq!(plan.expired.len(), 2);
    // The two expiries alone bring the tree under the quota.
    assert!(!plan.quota_exceeded || plan.evicted.is_empty());
    assert!(
        !plan
            .evicted
            .contains(&PathBuf::from("undo-history/fresh.rs")),
        "a fresh snapshot is never evicted when expiry already fits the quota"
    );
}

#[test]
fn a_symlink_is_neither_measured_as_content_nor_deleted_through() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let root = workspace.path();
    let outside = tempfile::tempdir().expect("outside directory");
    let outside_file = outside.path().join("precious.rs");
    fs::write(&outside_file, "irreplaceable user content").expect("outside file should write");
    let outside_dir = outside.path().join("precious-dir");
    write_file(&outside_dir.join("nested.rs"), 500);

    fs::create_dir_all(root.join(".cindx/tool-output")).expect("tool-output should be created");
    std::os::unix::fs::symlink(&outside_file, root.join(".cindx/tool-output/link.rs"))
        .expect("file symlink should be planted");
    std::os::unix::fs::symlink(&outside_dir, root.join(".cindx/undo-history-link"))
        .expect("directory symlink should be planted");
    write_file(&root.join(".cindx/undo-history/real.rs"), 60);

    let usage = measure_cindx_usage(root);
    // Only the real file is measured: the symlink is not content, and the
    // directory link is never descended into.
    assert_eq!(usage.total_bytes, 60, "usage was {usage:?}");
    assert_eq!(usage.files.len(), 1);

    let future_ms = usage.files[0].modified_ms + CINDX_RETENTION_TTL_MS + 60_000;
    let outcome = run_cindx_retention_sweep(root, future_ms);

    assert_eq!(outcome.removed, 1);
    assert!(!root.join(".cindx/undo-history/real.rs").exists());
    // Zero external change: the linked file and the linked tree are untouched.
    assert_eq!(
        fs::read_to_string(&outside_file).expect("outside file should still read"),
        "irreplaceable user content"
    );
    assert!(outside_dir.join("nested.rs").is_file());
    assert_eq!(
        fs::read(outside_dir.join("nested.rs"))
            .expect("nested should read")
            .len(),
        500
    );
    // The planted links themselves are left alone rather than deleted through.
    assert!(root.join(".cindx/tool-output/link.rs").exists());
    assert!(root.join(".cindx/undo-history-link").exists());
}

#[test]
fn a_plan_carrying_an_escaping_path_is_refused_at_deletion_time() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let root = workspace.path();
    let outside = root.join("outside.rs");
    fs::write(&outside, "must survive").expect("outside file should write");
    write_file(&root.join(".cindx/tool-output/real.txt"), 10);

    let plan = crate::cindx_retention_runtime::CindxRetirementPlan {
        expired: vec![
            PathBuf::from("../outside.rs"),
            PathBuf::from("/etc/hosts"),
            root.join("outside.rs"),
        ],
        evicted: vec![PathBuf::from("tool-output/real.txt")],
        bytes_reclaimed: 0,
        quota_exceeded: false,
        truncated_scan: false,
    };

    let outcome = apply_cindx_retention(root, &plan);

    assert_eq!(
        outcome.removed, 1,
        "only the contained candidate is removed"
    );
    assert_eq!(outcome.bytes_reclaimed, 10);
    assert_eq!(
        outcome.failures.len(),
        3,
        "failures were {:?}",
        outcome.failures
    );
    assert_eq!(
        fs::read_to_string(&outside).expect("outside file should survive"),
        "must survive"
    );
}

#[test]
fn a_file_that_disappeared_between_planning_and_deletion_is_reported_not_fatal() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let root = workspace.path();
    write_file(&root.join(".cindx/tool-output/gone.txt"), 10);
    write_file(&root.join(".cindx/tool-output/here.txt"), 12);

    let usage = measure_cindx_usage(root);
    let future_ms = usage.files[0].modified_ms + CINDX_RETENTION_TTL_MS + 60_000;
    let mut plan = plan_cindx_retention(&usage, future_ms);
    assert_eq!(plan.expired.len(), 2);
    fs::remove_file(root.join(".cindx/tool-output/gone.txt")).expect("file should remove");
    // A vanished candidate must not stop the rest of the sweep.
    plan.expired
        .push(PathBuf::from("tool-output/never-existed.txt"));

    let outcome = apply_cindx_retention(root, &plan);

    assert_eq!(outcome.removed, 1);
    assert_eq!(outcome.bytes_reclaimed, 12);
    assert_eq!(outcome.failures.len(), 2);
    assert!(!root.join(".cindx/tool-output/here.txt").exists());
}
