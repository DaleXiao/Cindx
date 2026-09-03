//! Aggregate quota and TTL retention for the workspace `.cindx` tree (P2-06).
//!
//! Every `.cindx` store was bounded per file, per generation count, or per
//! session delete, but nothing bounded the total. `undo-history` was the sharpest
//! gap: it is not in the managed-artifact root list and no event key names it, so
//! the retirement scanner that runs on a session or project delete never saw it,
//! and its snapshots accumulated for the life of the workspace.
//!
//! This module owns the aggregate view: measure the whole tree without following
//! a single symlink, expire the regenerable recovery and capture classes past a
//! TTL, and evict oldest-first when the aggregate quota is exceeded.
//!
//! Semantic state is never a candidate. Knowledge generations, memory vector
//! generations, project instructions, custom commands, installed skills, todos,
//! context checkpoints, and the trace export are not regenerable by a sweep, so
//! they are measured but never expired or evicted: the quota reports the truth
//! about the tree while only the regenerable classes pay for it.

use crate::app_state::AppState;
use crate::managed_artifact_lifecycle::remove_empty_parents;
use crate::persistence_runtime::append_startup_log;
use crate::runtime_values::current_time_millis;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

const CINDX_DIRECTORY: &str = ".cindx";
/// Aggregate budget for one workspace's whole `.cindx` tree.
pub(crate) const CINDX_AGGREGATE_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Age past which a regenerable recovery or capture file may be expired. Undo and
/// recovery are recent-change features, and both already disclose a missing
/// snapshot as "not undoable", so an aged snapshot is a loss of convenience, never
/// a loss of correctness.
pub(crate) const CINDX_RETENTION_TTL_MS: u64 = 14 * 24 * 60 * 60 * 1000;
/// Walk bound: a pathological tree is measured partially and says so, rather than
/// turning a startup sweep into an unbounded scan.
const MAX_ENTRIES_VISITED: usize = 20_000;
/// Planning bound on how many candidate files one sweep retains.
const MAX_CANDIDATE_FILES: usize = 4_096;
/// Reported failure bound, so one broken tree cannot fill the startup log.
const MAX_REPORTED_FAILURES: usize = 8;
/// Startup delay before the sweep, so it never competes with window reveal.
const SWEEP_STARTUP_DELAY_SECS: u64 = 20;

/// The regenerable classes a sweep may expire or evict. Everything else under
/// `.cindx` is semantic state and is measured only.
const RETENTION_ELIGIBLE_ROOTS: [&str; 5] = [
    "browser-captures",
    "output-history",
    "screenshots",
    "tool-output",
    "undo-history",
];

/// One candidate file, addressed relative to the `.cindx` root so a plan can
/// never carry an absolute path out of the workspace it was measured in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CindxFile {
    pub(crate) relative: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) modified_ms: u64,
}

/// What one workspace's `.cindx` tree costs, and which of it a sweep may reclaim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CindxUsage {
    /// Every regular file under `.cindx`, eligible or not.
    pub(crate) total_bytes: u64,
    /// The share held by the regenerable classes.
    pub(crate) eligible_bytes: u64,
    pub(crate) files: Vec<CindxFile>,
    pub(crate) entries_visited: usize,
    /// The walk or the candidate list hit its bound, so the measurement is
    /// partial and the plan is conservative.
    pub(crate) truncated: bool,
}

/// A pure decision over a measurement: what expires by TTL, and — only if the
/// aggregate quota is still exceeded afterwards — what is evicted oldest-first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CindxRetirementPlan {
    pub(crate) expired: Vec<PathBuf>,
    pub(crate) evicted: Vec<PathBuf>,
    pub(crate) bytes_reclaimed: u64,
    pub(crate) quota_exceeded: bool,
    pub(crate) truncated_scan: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CindxRetirementOutcome {
    pub(crate) removed: usize,
    pub(crate) bytes_reclaimed: u64,
    pub(crate) total_bytes_before: u64,
    pub(crate) truncated_scan: bool,
    pub(crate) failures: Vec<String>,
}

/// Measures one workspace's `.cindx` tree. The walk descends only into real
/// directories: a symlink is measured as itself and never followed, so a planted
/// link can neither inflate the total with outside content nor become a deletion
/// target.
pub(crate) fn measure_cindx_usage(workspace_root: &Path) -> CindxUsage {
    let root = workspace_root.join(CINDX_DIRECTORY);
    let mut usage = CindxUsage::default();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        if usage.entries_visited >= MAX_ENTRIES_VISITED {
            usage.truncated = true;
            break;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            usage.entries_visited += 1;
            if usage.entries_visited > MAX_ENTRIES_VISITED {
                usage.truncated = true;
                break;
            }
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            // Symlinks, sockets, and fifos are not measured as content and are
            // never candidates.
            if !metadata.is_file() {
                continue;
            }
            usage.total_bytes += metadata.len();
            if !is_retention_eligible(relative) {
                continue;
            }
            usage.eligible_bytes += metadata.len();
            if usage.files.len() >= MAX_CANDIDATE_FILES {
                usage.truncated = true;
                continue;
            }
            usage.files.push(CindxFile {
                relative: relative.to_path_buf(),
                bytes: metadata.len(),
                modified_ms: modified_ms(&metadata),
            });
        }
    }
    usage
}

/// Decides what one measurement owes: everything past the TTL expires, and if the
/// aggregate quota is still exceeded, the oldest remaining candidates are evicted
/// until it fits. Deterministic and side-effect free.
pub(crate) fn plan_cindx_retention(usage: &CindxUsage, now_ms: u64) -> CindxRetirementPlan {
    let mut plan = CindxRetirementPlan {
        truncated_scan: usage.truncated,
        ..CindxRetirementPlan::default()
    };
    let mut surviving = Vec::new();
    let mut remaining_bytes = usage.total_bytes;
    for file in &usage.files {
        // An unreadable or future mtime is not treated as aged: only a file whose
        // recorded age really exceeds the TTL expires.
        let aged_out = file.modified_ms > 0
            && now_ms.saturating_sub(file.modified_ms) > CINDX_RETENTION_TTL_MS;
        if !aged_out {
            surviving.push(file);
            continue;
        }
        plan.expired.push(file.relative.clone());
        plan.bytes_reclaimed += file.bytes;
        remaining_bytes = remaining_bytes.saturating_sub(file.bytes);
    }

    plan.quota_exceeded = remaining_bytes > CINDX_AGGREGATE_QUOTA_BYTES;
    if plan.quota_exceeded {
        surviving.sort_by(|left, right| {
            left.modified_ms
                .cmp(&right.modified_ms)
                .then_with(|| left.relative.cmp(&right.relative))
        });
        for file in surviving {
            if remaining_bytes <= CINDX_AGGREGATE_QUOTA_BYTES {
                break;
            }
            plan.evicted.push(file.relative.clone());
            plan.bytes_reclaimed += file.bytes;
            remaining_bytes = remaining_bytes.saturating_sub(file.bytes);
        }
    }
    plan
}

/// Removes the planned files. Each removal re-checks containment and type at the
/// moment of deletion, so a plan built from an earlier measurement can never
/// delete through a path that changed shape in between.
pub(crate) fn apply_cindx_retention(
    workspace_root: &Path,
    plan: &CindxRetirementPlan,
) -> CindxRetirementOutcome {
    let root = workspace_root.join(CINDX_DIRECTORY);
    let mut outcome = CindxRetirementOutcome::default();
    for relative in plan.expired.iter().chain(plan.evicted.iter()) {
        match remove_retained_file(&root, relative) {
            Ok(bytes) => {
                outcome.removed += 1;
                outcome.bytes_reclaimed += bytes;
            }
            Err(error) => {
                if outcome.failures.len() < MAX_REPORTED_FAILURES {
                    outcome
                        .failures
                        .push(format!("{}: {error}", relative.display()));
                }
            }
        }
    }
    outcome
}

/// Measures, plans, and applies retention for one workspace.
pub(crate) fn run_cindx_retention_sweep(
    workspace_root: &Path,
    now_ms: u64,
) -> CindxRetirementOutcome {
    let usage = measure_cindx_usage(workspace_root);
    let plan = plan_cindx_retention(&usage, now_ms);
    let mut outcome = apply_cindx_retention(workspace_root, &plan);
    outcome.total_bytes_before = usage.total_bytes;
    outcome.truncated_scan = usage.truncated;
    outcome
}

/// One bounded sweep per known workspace, off the main thread and after startup
/// has settled. At most once per launch: the walk is bounded, the policy is
/// age-based, and launches are frequent enough that no marker file needs to be
/// written into `.cindx` to rate-limit it.
pub(crate) fn start_cindx_retention_sweep(app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("cindx-retention-sweep".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(SWEEP_STARTUP_DELAY_SECS));
            for root in retention_sweep_roots(&app) {
                let outcome = run_cindx_retention_sweep(&root, current_time_millis());
                if outcome.removed > 0 || !outcome.failures.is_empty() {
                    append_startup_log(&format!(
                        "cindx retention {}: removed {} file(s), reclaimed {} of {} bytes{}{}",
                        root.display(),
                        outcome.removed,
                        outcome.bytes_reclaimed,
                        outcome.total_bytes_before,
                        if outcome.truncated_scan {
                            " (partial scan)"
                        } else {
                            ""
                        },
                        if outcome.failures.is_empty() {
                            String::new()
                        } else {
                            format!("; failures: {}", outcome.failures.join("; "))
                        }
                    ));
                }
            }
        });
}

/// The active workspace plus every registered project root, deduplicated. A root
/// with no `.cindx` directory measures as empty and costs nothing.
fn retention_sweep_roots(app: &tauri::AppHandle) -> Vec<PathBuf> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let mut roots = Vec::new();
    if let Ok(config) = state.workspace_config.lock() {
        roots.push(config.root.clone());
    }
    if let Ok(config) = state.project_session_config.lock() {
        for project in &config.projects {
            roots.push(PathBuf::from(&project.root));
        }
    }
    let mut unique = Vec::new();
    for root in roots {
        if root.as_os_str().is_empty() || unique.contains(&root) {
            continue;
        }
        unique.push(root);
    }
    unique
}

/// Whether a measured file belongs to one of the regenerable classes, decided by
/// the first path component under `.cindx`.
fn is_retention_eligible(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .is_some_and(|name| RETENTION_ELIGIBLE_ROOTS.contains(&name))
}

fn remove_retained_file(root: &Path, relative: &Path) -> Result<u64, String> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err("candidate is not contained by the .cindx root".to_string());
    }
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("candidate is not a regular file".to_string());
    }
    let bytes = metadata.len();
    fs::remove_file(&path).map_err(|error| error.to_string())?;
    // Leave no empty directory skeleton behind, but never above the `.cindx` root.
    let _ = remove_empty_parents(path.parent(), root);
    Ok(bytes)
}

fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
