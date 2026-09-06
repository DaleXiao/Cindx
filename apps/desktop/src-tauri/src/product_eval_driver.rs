//! The Phase 4 one-shot product-path evaluation driver (feature
//! `product-eval`; never part of a shipping build).
//!
//! Modes:
//! - `preflight` — provider-free: binds HEAD/tree, app version, suite digest,
//!   frozen matrix digest, driver binary digest, and the redacted provider
//!   authority into a receipt with `provider_calls_performed: 0`. No spend.
//! - `rehearse` — provider-free: drives the full frozen matrix against the
//!   local fake provider to validate the instrumentation and measure the real
//!   per-arm call/token arithmetic before any authorization is consumed.
//! - `execute` — the authorized one-shot run: validates the preflight receipt
//!   against the current binary and tree, holds a power assertion
//!   (`caffeinate -i -w <pid>`), drives the matrix against the configured
//!   provider with live budget enforcement, censors sleep-interrupted cells,
//!   and retains every failure in the denominator. No-retry: a started run
//!   cannot be rerun to rescue a censored or failed cell.
//!
//! Per-run receipts come from the shipping telemetry journal
//! (`RunTelemetryReceiptV1`), never from harness-side estimation.
#![cfg(feature = "product-eval")]

use crate::product_path_eval::{
    drive_product_path_run, materialize_fixture, FakeOpenAiServer, ProductPathRunRequest,
};
use agent_eval::{
    check_postconditions, parse_suite, run_matched_cells, CaseReceipts, CaseReport, EvalCase,
    MatchedCellContext,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const PREFLIGHT_RECEIPT_SCHEMA: &str = "cindx.phase4-preflight.v1";
pub const RUN_RECEIPT_SCHEMA: &str = "cindx.phase4-run-receipt.v1";

/// The frozen owner-confirmed budget (EVALUATION.md: confirmed 2026-09-04;
/// the provider-call cap was amended 200 → 700 on 2026-09-06 against the
/// measured 64-cell rehearsal floor of 256 HTTP calls and the judge-eligible
/// provider binding; tokens and wall clock unchanged).
pub const PROTOCOL_MAX_PROVIDER_CALLS: u64 = 700;
pub const PROTOCOL_MAX_TOTAL_TOKENS: u64 = 2_000_000;
pub const PROTOCOL_WALL_BUDGET_MS: u64 = 4 * 60 * 60 * 1000;

/// A wall-minus-monotonic drift beyond this censors the cell: the host slept,
/// so its latency and possibly its work are structurally invalid.
pub const SUSPENSION_DRIFT_CENSOR_MS: u64 = 30_000;

/// The frozen arm matrix: single-agent versus subagent delegation across
/// every effort tier, exactly as the protocol clause names them.
pub const FROZEN_ARMS: [&str; 8] = [
    "single-fast",
    "single-default",
    "single-high",
    "single-xhigh",
    "subagent-fast",
    "subagent-default",
    "subagent-high",
    "subagent-xhigh",
];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read {path:?}: {error}"))?;
    Ok(sha256_bytes(&bytes))
}

fn git_output(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("git {:?} failed: {error}", args))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_arm(arm: &str) -> Result<(bool, String), String> {
    match arm.split_once('-') {
        Some(("single", tier)) => Ok((false, tier.to_string())),
        Some(("subagent", tier)) => Ok((true, tier.to_string())),
        _ => Err(format!(
            "arm `{arm}` is not single-<tier> or subagent-<tier>"
        )),
    }
}

fn matrix_digest(case_ids: &[String]) -> String {
    let canonical = serde_json::to_string(&(FROZEN_ARMS.to_vec(), case_ids.to_vec()))
        .expect("matrix serializes");
    sha256_bytes(canonical.as_bytes())
}

fn load_suite(path: &Path) -> Result<(Vec<EvalCase>, String), String> {
    let raw =
        std::fs::read_to_string(path).map_err(|error| format!("suite unreadable: {error}"))?;
    let digest = sha256_bytes(raw.as_bytes());
    let cases = parse_suite(&raw).map_err(|error| format!("suite parse failed: {error}"))?;
    if cases.is_empty() {
        return Err("suite contains no cases".to_string());
    }
    Ok((cases, digest))
}

/// The redacted provider binding: identity and shape of the authority the run
/// would use, never the credential itself.
fn redacted_provider_binding(
    config: &crate::configuration_models::ProviderConfig,
) -> serde_json::Value {
    let config = config.clone();
    let key_present = !config.api_key.trim().is_empty();
    let key_prefix = if key_present {
        sha256_bytes(config.api_key.as_bytes())[..8].to_string()
    } else {
        String::new()
    };
    serde_json::json!({
        "provider_id": config.provider_id,
        "base_url": config.base_url,
        "model": config.model,
        "fast_model": config.fast_model,
        "executor_model": config.executor_model,
        "reviewer_model": config.reviewer_model,
        "embedding_model": config.embedding_model,
        "approval_policy": config.approval_policy,
        "guardian_auto_approval": config.guardian_auto_approval,
        "direct_judge_fail_closed": config.direct_judge_fail_closed,
        "plan_first_enabled": config.plan_first_enabled,
        "api_key_present": key_present,
        "api_key_sha256_prefix": key_prefix,
    })
}

/// Digest of the machine's skill catalog (`~/.cindx/skills` plus the
/// preferences file): the one HOME-derived input the data-root isolation does
/// not cover, so the frozen identity binds it explicitly and execute refuses
/// on drift. Bounded walk; an absent catalog digests as "absent".
fn skills_catalog_digest() -> String {
    let home = match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home),
        Err(_) => return "no-home".to_string(),
    };
    let skills_root = home.join(".cindx").join("skills");
    let mut entries: Vec<String> = Vec::new();
    if skills_root.is_dir() {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut stack = vec![skills_root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if files.len() < 512 {
                    files.push(path);
                }
            }
        }
        files.sort();
        for file in files {
            let relative = file
                .strip_prefix(&skills_root)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| file.display().to_string());
            let digest = std::fs::read(&file)
                .map(|bytes| sha256_bytes(&bytes))
                .unwrap_or_else(|_| "unreadable".to_string());
            entries.push(format!("{relative}\0{digest}"));
        }
    } else {
        entries.push("absent".to_string());
    }
    if let Ok(preferences) = std::fs::read(home.join(".cindx").join("skill-preferences.json")) {
        entries.push(format!("preferences\0{}", sha256_bytes(&preferences)));
    }
    entries.sort();
    sha256_bytes(entries.join("\n").as_bytes())
}

/// The suite's command oracles run through the host toolchain; the frozen
/// identity binds what preflight saw and execute refuses on drift or absence.
fn host_toolchain_binding() -> serde_json::Value {
    let python3 = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    let sh = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("echo ok")
        .output()
        .ok()
        .filter(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "ok"
        })
        .map(|_| "present".to_string())
        .unwrap_or_default();
    serde_json::json!({ "python3": python3, "sh": sh })
}

fn preflight(
    args: &[String],
    provider_authority: &crate::configuration_models::ProviderConfig,
) -> i32 {
    let (suite_path, out_dir) = match common_flags(args) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("preflight: {error}");
            return 2;
        }
    };
    let (cases, suite_digest) = match load_suite(&suite_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("preflight: {error}");
            return 2;
        }
    };
    let tracked_dirty = git_output(&["status", "--porcelain=v1"])
        .map(|status| {
            status
                .lines()
                .filter(|line| !line.starts_with("??"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let untracked = git_output(&["status", "--porcelain=v1"])
        .map(|status| {
            status
                .lines()
                .filter(|line| line.starts_with("??"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let binary_path = std::env::current_exe().expect("driver binary path");
    let binary_digest = match sha256_file(&binary_path) {
        Ok(digest) => digest,
        Err(error) => {
            eprintln!("preflight: {error}");
            return 2;
        }
    };
    let case_ids: Vec<String> = cases.iter().map(|case| case.id.clone()).collect();
    let receipt = serde_json::json!({
        "schema": PREFLIGHT_RECEIPT_SCHEMA,
        "created_at_ms": now_ms(),
        "mode": "preflight",
        "provider_calls_performed": 0,
        "git": {
            "head": git_output(&["rev-parse", "HEAD"]).unwrap_or_default(),
            "tree": git_output(&["rev-parse", "HEAD^{tree}"]).unwrap_or_default(),
            "tracked_clean": tracked_dirty.is_empty(),
            "tracked_dirty": tracked_dirty,
            "untracked": untracked,
        },
        "app_version": env!("CARGO_PKG_VERSION"),
        "suite": { "path": suite_path.display().to_string(), "sha256": suite_digest, "case_ids": case_ids },
        "matrix": { "arms": FROZEN_ARMS.to_vec(), "cells": cases.len() * FROZEN_ARMS.len(), "sha256": matrix_digest(&case_ids) },
        "binary": { "path": binary_path.display().to_string(), "sha256": binary_digest },
        "provider_binding": redacted_provider_binding(provider_authority),
        "skills_catalog_sha256": skills_catalog_digest(),
        "host_toolchain": host_toolchain_binding(),
        "eval_normalization": {
            "approval_policy": "all",
            "plan_first_enabled": false,
            "guardian_auto_approval": false,
            "note": "interactive conveniences normalized off for every arm, mirroring the scheduled-run precedent",
        },
        "budget": {
            "max_provider_calls": PROTOCOL_MAX_PROVIDER_CALLS,
            "max_total_tokens": PROTOCOL_MAX_TOTAL_TOKENS,
            "wall_budget_ms": PROTOCOL_WALL_BUDGET_MS,
        },
    });
    write_receipt(&out_dir, "preflight-receipt.json", &receipt)
}

fn common_flags(args: &[String]) -> Result<(PathBuf, PathBuf), String> {
    let mut suite = None;
    let mut out = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--suite" => {
                suite = args.get(index + 1).map(PathBuf::from);
                index += 1;
            }
            "--out" => {
                out = args.get(index + 1).map(PathBuf::from);
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }
    Ok((
        suite.ok_or("--suite <path> is required")?,
        out.ok_or("--out <dir> is required")?,
    ))
}

fn write_receipt(out_dir: &Path, name: &str, receipt: &serde_json::Value) -> i32 {
    if let Err(error) = std::fs::create_dir_all(out_dir) {
        eprintln!("cannot create {out_dir:?}: {error}");
        return 2;
    }
    let path = out_dir.join(name);
    let payload = serde_json::to_string_pretty(receipt).expect("receipt serializes");
    if let Err(error) = std::fs::write(&path, payload) {
        eprintln!("cannot write {path:?}: {error}");
        return 2;
    }
    println!("{}", path.display());
    0
}

/// The shared cell executor: materialize the fixture, drive the shipping run
/// path, judge with the case's sealed postconditions, and price the cell from
/// the run's own telemetry receipt.
fn run_eval_cell(
    cell: MatchedCellContext<'_>,
    base_config: &crate::configuration_models::ProviderConfig,
) -> CaseReport {
    let id = cell.case.id.clone();
    let (delegation, tier) = match parse_arm(cell.arm) {
        Ok(parsed) => parsed,
        Err(error) => {
            return error_report(&id, &error);
        }
    };
    // A cell root is single-use: a stale workspace from an earlier run could
    // fabricate postcondition passes, so any previous cell is removed first.
    let _ = std::fs::remove_dir_all(&cell.cell_root);
    let workspace = cell.cell_root.join("workspace");
    if let Err(error) = std::fs::create_dir_all(&workspace) {
        return error_report(&id, &format!("cell workspace failed: {error}"));
    }
    let fixture: Vec<(String, String)> = cell
        .case
        .fixture
        .iter()
        .map(|file| (file.path.clone(), file.content.clone()))
        .collect();
    materialize_fixture(&workspace, &fixture);
    let mut config = base_config.clone();
    // Unattended: non-destructive tools auto-approve; destructive risk stays
    // human-gated by product invariant, and a cell that parks on it is
    // recorded as a failure instead of being silently rescued.
    config.approval_policy = "all".to_string();
    // Interactive conveniences are normalized off for every arm, mirroring the
    // scheduled-run precedent ("the plan-then-confirm gate is an interactive
    // Composer feature and never applies here"): plan-first would park the
    // run on a confirmation card, and guardian auto-approval would add
    // unpriced reviewer calls. Both normalizations are recorded in the
    // receipt's provider binding.
    config.plan_first_enabled = false;
    config.guardian_auto_approval = false;

    let outcome = drive_product_path_run(ProductPathRunRequest {
        prompt: cell.case.prompt.clone(),
        effort: tier,
        no_delegation: !delegation,
        provider_config: config,
        workspace_root: workspace.clone(),
        store_path: cell.cell_root.join("state.sqlite3"),
    });

    let drift_ms = outcome.wall_ms.saturating_sub(outcome.monotonic_ms);
    let slept_ms = outcome.host_slept_ms.max(drift_ms);
    let censored = slept_ms > SUSPENSION_DRIFT_CENSOR_MS;
    // Count the delegations the run actually executed from its durable
    // events, so a subagent-arm report can prove the contrast it measures.
    let delegations = outcome
        .state
        .as_ref()
        .map(|state| {
            let store = state.store.lock().expect("store lock");
            crate::agent_read_model::agent_events_for_session(
                &store,
                &crate::runtime_values::phase16_task_id(),
                Some(&outcome.session_id),
            )
            .map(|events| {
                events
                    .iter()
                    .filter(|event| {
                        event
                            .summary
                            .to_ascii_lowercase()
                            .contains("subagent finished")
                            || event
                                .metadata
                                .values()
                                .any(|value| value.contains("cindx.agent.subagent-run.v1"))
                    })
                    .count()
            })
            .unwrap_or(0)
        })
        .unwrap_or(0);
    let telemetry = outcome.telemetry.as_ref();
    let receipts = CaseReceipts {
        model_calls: telemetry
            .map(|receipt| receipt.model_calls as usize)
            .unwrap_or(0),
        // Kernel-consistent counting (agent-eval runner semantics): provider,
        // partial, and estimated usage all count as reported; only `unknown`
        // does not. The four counters are disjoint per-attempt provenance
        // classes, never overlapping quantities to subtract.
        usage_reported: telemetry
            .map(|receipt| {
                (receipt.usage_provider_attempts
                    + receipt.usage_provider_partial_attempts
                    + receipt.usage_estimated_attempts) as usize
            })
            .unwrap_or(0),
        prompt_tokens: telemetry.map(|receipt| receipt.prompt_tokens).unwrap_or(0),
        completion_tokens: telemetry
            .map(|receipt| receipt.completion_tokens)
            .unwrap_or(0),
        total_tokens: telemetry
            .map(|receipt| receipt.prompt_tokens + receipt.completion_tokens)
            .unwrap_or(0),
        tool_calls: telemetry
            .map(|receipt| receipt.tool_calls as usize)
            .unwrap_or(0),
        network_tool_calls: outcome.network_tool_calls,
        wall_clock_ms: outcome.wall_ms,
        delegations,
    };
    let checks = check_postconditions(&workspace, &outcome.final_answer, &cell.case.postconditions);
    let mut error = outcome.run_error.clone();
    if censored {
        error = Some(format!(
            "censored: host slept {slept_ms}ms during the cell (limit {SUSPENSION_DRIFT_CENSOR_MS}ms)"
        ));
    } else if error.is_none() && outcome.status != "completed" {
        error = Some(format!(
            "run ended `{}` instead of completed",
            outcome.status
        ));
    }
    let passed = error.is_none() && checks.iter().all(|check| check.passed);
    CaseReport {
        id,
        passed,
        checks,
        tool_calls: receipts.tool_calls,
        turns: telemetry
            .map(|receipt| receipt.agent_turns as usize)
            .unwrap_or(0),
        error,
        receipts,
    }
}

fn error_report(id: &str, error: &str) -> CaseReport {
    CaseReport {
        id: id.to_string(),
        passed: false,
        checks: Vec::new(),
        tool_calls: 0,
        turns: 0,
        error: Some(error.to_string()),
        receipts: CaseReceipts::default(),
    }
}

struct BudgetLedger {
    calls: u64,
    tokens: u64,
    /// Real-time start: the wall budget must keep counting through a host
    /// sleep, which a pausing monotonic clock would under-count.
    started_wall_ms: u64,
    breached: Option<String>,
}

impl BudgetLedger {
    fn new() -> Self {
        Self {
            calls: 0,
            tokens: 0,
            started_wall_ms: now_ms(),
            breached: None,
        }
    }

    fn remaining_ok(&self) -> Option<String> {
        if let Some(reason) = &self.breached {
            return Some(reason.clone());
        }
        if self.calls >= PROTOCOL_MAX_PROVIDER_CALLS {
            return Some(format!(
                "not-run: provider-call budget {PROTOCOL_MAX_PROVIDER_CALLS} reached"
            ));
        }
        if self.tokens >= PROTOCOL_MAX_TOTAL_TOKENS {
            return Some(format!(
                "not-run: token budget {PROTOCOL_MAX_TOTAL_TOKENS} reached"
            ));
        }
        let elapsed_ms = now_ms().saturating_sub(self.started_wall_ms);
        if elapsed_ms >= PROTOCOL_WALL_BUDGET_MS {
            return Some(format!(
                "not-run: wall budget {PROTOCOL_WALL_BUDGET_MS}ms reached"
            ));
        }
        None
    }

    fn charge(&mut self, report: &CaseReport) {
        self.calls += report.receipts.model_calls as u64;
        self.tokens += report.receipts.total_tokens;
    }
}

fn run_matrix(
    cases: &[EvalCase],
    workspace_root: &Path,
    base_config: &crate::configuration_models::ProviderConfig,
    enforce_budget: bool,
) -> (agent_eval::MatchedArmReport, BudgetLedger) {
    let mut ledger = BudgetLedger::new();
    let report = run_matched_cells(
        cases,
        &FROZEN_ARMS
            .iter()
            .map(|arm| arm.to_string())
            .collect::<Vec<_>>(),
        workspace_root,
        |cell| {
            if enforce_budget {
                if let Some(reason) = ledger.remaining_ok() {
                    ledger.breached = Some(reason.clone());
                    return error_report(&cell.case.id, &reason);
                }
            }
            let report = run_eval_cell(cell, base_config);
            if enforce_budget {
                ledger.charge(&report);
            }
            report
        },
    );
    (report, ledger)
}

fn per_arm_summary(report: &agent_eval::MatchedArmReport) -> serde_json::Value {
    let mut arms = serde_json::Map::new();
    for arm in &report.arms {
        let (mut cells, mut passed, mut calls, mut tokens, mut wall) =
            (0u64, 0u64, 0u64, 0u64, 0u64);
        let mut delegations = 0u64;
        for case in &report.cases {
            for run in &case.arms {
                if &run.arm != arm {
                    continue;
                }
                cells += 1;
                passed += u64::from(run.report.passed);
                calls += run.report.receipts.model_calls as u64;
                tokens += run.report.receipts.total_tokens;
                wall += run.report.receipts.wall_clock_ms;
                delegations += run.report.receipts.delegations as u64;
            }
        }
        arms.insert(
            arm.clone(),
            serde_json::json!({
                "cells": cells,
                "passed": passed,
                "model_calls": calls,
                "total_tokens": tokens,
                "wall_ms": wall,
                "delegations": delegations,
            }),
        );
    }
    serde_json::Value::Object(arms)
}

fn rehearse(args: &[String]) -> i32 {
    let (suite_path, out_dir) = match common_flags(args) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("rehearse: {error}");
            return 2;
        }
    };
    let (cases, suite_digest) = match load_suite(&suite_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("rehearse: {error}");
            return 2;
        }
    };
    let server = FakeOpenAiServer::start("Rehearsed completion for evaluation.");
    let config = fake_provider_config(&server.base_url);
    let workspace_root = out_dir.join("cells");
    let started = Instant::now();
    let (report, _ledger) = run_matrix(&cases, &workspace_root, &config, false);
    let counters = server.counters();
    let report_path = out_dir.join("rehearsal-report.json");
    let report_json = report.to_json();
    let report_digest = sha256_bytes(report_json.as_bytes());
    if let Err(error) =
        std::fs::create_dir_all(&out_dir).and_then(|()| std::fs::write(&report_path, report_json))
    {
        eprintln!("cannot write {report_path:?}: {error}");
        return 2;
    }
    let case_ids: Vec<String> = cases.iter().map(|case| case.id.clone()).collect();
    let receipt = serde_json::json!({
        "schema": RUN_RECEIPT_SCHEMA,
        "mode": "rehearse",
        "created_at_ms": now_ms(),
        "provider_calls_performed": 0,
        "suite_sha256": suite_digest,
        "matrix_sha256": matrix_digest(&case_ids),
        "report_sha256": report_digest,
        "cells": report.cases.len() * FROZEN_ARMS.len(),
        "position_balanced": report.is_position_balanced(),
        "wall_ms": started.elapsed().as_millis() as u64,
        "fake_server": { "chat_requests": counters.chat, "embedding_requests": counters.embeddings },
        "per_arm": per_arm_summary(&report),
    });
    println!("rehearsal report: {}", report_path.display());
    write_receipt(&out_dir, "rehearsal-receipt.json", &receipt)
}

fn fake_provider_config(base_url: &str) -> crate::configuration_models::ProviderConfig {
    crate::configuration_models::ProviderConfig {
        base_url: base_url.to_string(),
        api_key: "rehearsal-key".to_string(),
        model: "fake-model".to_string(),
        fast_model: "fake-model".to_string(),
        executor_model: "fake-model".to_string(),
        reviewer_model: "fake-model".to_string(),
        embedding_model: "fake-model".to_string(),
        ..Default::default()
    }
}

fn execute(
    args: &[String],
    provider_authority: crate::configuration_models::ProviderConfig,
) -> i32 {
    let (suite_path, out_dir) = match common_flags(args) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("execute: {error}");
            return 2;
        }
    };
    let preflight_path = flag_value(args, "--preflight");
    let Some(preflight_path) = preflight_path else {
        eprintln!("execute: --preflight <receipt.json> is required");
        return 2;
    };
    let rehearsal_path = flag_value(args, "--rehearsal");
    let Some(rehearsal_path) = rehearsal_path else {
        eprintln!(
            "execute: --rehearsal <receipt.json> is required — the frozen authorization's condition (a) is a green provider-free rehearsal of THIS suite/matrix on THIS revision"
        );
        return 2;
    };
    let preflight: serde_json::Value = match std::fs::read_to_string(&preflight_path)
        .map_err(|error| error.to_string())
        .and_then(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("execute: preflight receipt unreadable: {error}");
            return 2;
        }
    };
    // Fail closed: the run must execute exactly what the preflight bound.
    if preflight["schema"] != PREFLIGHT_RECEIPT_SCHEMA {
        eprintln!("execute: unexpected preflight schema");
        return 2;
    }
    let (cases, suite_digest) = match load_suite(&suite_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("execute: {error}");
            return 2;
        }
    };
    let case_ids: Vec<String> = cases.iter().map(|case| case.id.clone()).collect();

    // Condition (a) of the frozen authorization: a green rehearsal of this
    // exact suite and matrix. Green means every cell's run completed (report
    // errors null — postcondition failures are expected against the fake
    // provider), the matrix was position-balanced, and the receipt digests
    // match what is about to be executed.
    let rehearsal: serde_json::Value = match std::fs::read_to_string(&rehearsal_path)
        .map_err(|error| error.to_string())
        .and_then(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("execute: rehearsal receipt unreadable: {error}");
            return 2;
        }
    };
    let rehearsal_report_path = rehearsal_path.with_file_name("rehearsal-report.json");
    let rehearsal_report: agent_eval::MatchedArmReport =
        match std::fs::read_to_string(&rehearsal_report_path)
            .map_err(|error| error.to_string())
            .and_then(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
        {
            Ok(report) => report,
            Err(error) => {
                eprintln!("execute: rehearsal report unreadable: {error}");
                return 2;
            }
        };
    let mut rehearsal_problems = Vec::new();
    if rehearsal["schema"] != RUN_RECEIPT_SCHEMA || rehearsal["mode"] != "rehearse" {
        rehearsal_problems.push("not a rehearsal receipt");
    }
    if rehearsal["suite_sha256"] != suite_digest {
        rehearsal_problems.push("rehearsal ran a different suite digest");
    }
    if rehearsal["matrix_sha256"] != matrix_digest(&case_ids) {
        rehearsal_problems.push("rehearsal ran a different matrix digest");
    }
    if rehearsal["cells"].as_u64() != Some((cases.len() * FROZEN_ARMS.len()) as u64) {
        rehearsal_problems.push("rehearsal cell count does not match the frozen matrix");
    }
    if rehearsal["position_balanced"] != true {
        rehearsal_problems.push("rehearsal was not position-balanced");
    }
    if rehearsal["report_sha256"] != sha256_file(&rehearsal_report_path).unwrap_or_default() {
        rehearsal_problems.push("rehearsal report digest does not match its receipt");
    }
    // Green means the INSTRUMENT worked, not that the scripted fake model
    // satisfied the cases: a fake that only reads can never satisfy a
    // write-requiring prompt, so run-outcome errors (`run ended ...`) are the
    // expected fake-provider class. Instrument-class errors (censoring,
    // not-run, composition failures) and cells that never charged a model
    // call (no telemetry receipt) are what must be zero.
    let instrument_errored_cells = rehearsal_report
        .cases
        .iter()
        .flat_map(|case| case.arms.iter())
        .filter(|run| match &run.report.error {
            None => false,
            Some(error) => !error.starts_with("run ended `"),
        })
        .count();
    if instrument_errored_cells > 0 {
        rehearsal_problems.push("rehearsal had instrument-class cell errors");
    }
    let silent_cells = rehearsal_report
        .cases
        .iter()
        .flat_map(|case| case.arms.iter())
        .filter(|run| run.report.receipts.model_calls == 0)
        .count();
    if silent_cells > 0 {
        rehearsal_problems.push("rehearsal cells produced no telemetry receipts");
    }
    if !rehearsal_problems.is_empty() {
        eprintln!(
            "execute refuses: rehearsal not green ({})",
            rehearsal_problems.join("; ")
        );
        return 2;
    }

    let head = git_output(&["rev-parse", "HEAD"]).unwrap_or_default();
    let binary_path = std::env::current_exe().expect("driver binary path");
    let binary_digest = sha256_file(&binary_path).unwrap_or_default();
    let config = provider_authority;
    if config.api_key.trim().is_empty() || config.base_url.trim().is_empty() {
        eprintln!(
            "execute: provider config is incomplete (approve the keychain read once at start)"
        );
        return 2;
    }
    let mut mismatch = Vec::new();
    if preflight["git"]["head"] != head {
        mismatch.push("git head drifted since preflight");
    }
    if !preflight["git"]["tracked_clean"].as_bool().unwrap_or(false) {
        mismatch.push("preflight was taken on a dirty tree");
    }
    if preflight["suite"]["sha256"] != suite_digest {
        mismatch.push("suite digest drifted since preflight");
    }
    if preflight["matrix"]["sha256"] != matrix_digest(&case_ids) {
        mismatch.push("matrix digest drifted since preflight");
    }
    if preflight["binary"]["sha256"] != binary_digest {
        mismatch.push("driver binary digest drifted since preflight");
    }
    // The frozen identity includes the provider/model authority and the
    // machine's skill catalog: execute refuses to spend the one-shot against
    // anything other than what the preflight bound.
    let current_binding = redacted_provider_binding(&config);
    for field in [
        "provider_id",
        "base_url",
        "model",
        "fast_model",
        "executor_model",
        "reviewer_model",
        "embedding_model",
        "approval_policy",
        "guardian_auto_approval",
        "direct_judge_fail_closed",
        "plan_first_enabled",
        "api_key_present",
        "api_key_sha256_prefix",
    ] {
        if preflight["provider_binding"][field] != current_binding[field] {
            mismatch.push("provider binding drifted since preflight");
            break;
        }
    }
    if preflight["skills_catalog_sha256"] != skills_catalog_digest() {
        mismatch.push("skills catalog drifted since preflight");
    }
    if preflight["host_toolchain"] != host_toolchain_binding() {
        mismatch.push("host toolchain drifted or is unavailable since preflight");
    }
    if !mismatch.is_empty() {
        eprintln!("execute refuses: {}", mismatch.join("; "));
        return 2;
    }

    // The protocol's host power assertion: caffeinate watches this process and
    // exits with it.
    let caffeinate = std::process::Command::new("caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .spawn()
        .ok();

    let workspace_root = out_dir.join("cells");
    let started_wall = now_ms();
    let started = Instant::now();
    let (report, ledger) = run_matrix(&cases, &workspace_root, &config, true);
    let wall_ms = started.elapsed().as_millis() as u64;
    if let Err(error) = std::fs::create_dir_all(&out_dir)
        .and_then(|()| std::fs::write(out_dir.join("report.json"), report.to_json()))
    {
        eprintln!("cannot write report: {error}");
        return 2;
    }
    let censored: Vec<serde_json::Value> = report
        .cases
        .iter()
        .flat_map(|case| case.arms.iter())
        .filter(|run| run.report.error.as_deref().is_some_and(|error| error.starts_with("censored")))
        .map(|run| serde_json::json!({ "case": run.report.id, "arm": run.arm, "error": run.report.error }))
        .collect();
    let receipt = serde_json::json!({
        "schema": RUN_RECEIPT_SCHEMA,
        "mode": "execute",
        "created_at_ms": now_ms(),
        "preflight": {
            "path": preflight_path.display().to_string(),
            "sha256": sha256_file(&preflight_path).unwrap_or_default(),
        },
        "rehearsal": {
            "path": rehearsal_path.display().to_string(),
            "sha256": sha256_file(&rehearsal_path).unwrap_or_default(),
            "report_sha256": sha256_file(&rehearsal_report_path).unwrap_or_default(),
        },
        // Lower bound: telemetry-charged model attempts only. Embedding calls
        // (workspace indexing/retrieval on default/high/xhigh) are real
        // provider calls that the ledger does not charge; the rehearsal
        // receipt prices them separately from the fake server's counters.
        "counted_model_calls": ledger.calls,
        "counted_model_calls_are_lower_bound": true,
        "total_tokens": ledger.tokens,
        "budget": {
            "max_provider_calls": PROTOCOL_MAX_PROVIDER_CALLS,
            "max_total_tokens": PROTOCOL_MAX_TOTAL_TOKENS,
            "wall_budget_ms": PROTOCOL_WALL_BUDGET_MS,
            "breached": ledger.breached,
        },
        "started_at_ms": started_wall,
        "wall_ms": wall_ms,
        "monotonic_ms": started.elapsed().as_millis() as u64,
        "caffeinate": caffeinate.as_ref().map(|child| child.id()),
        "cells": report.cases.len() * FROZEN_ARMS.len(),
        "position_balanced": report.is_position_balanced(),
        "per_arm": per_arm_summary(&report),
        "censored_cells": censored,
        // Embedding calls (workspace indexing/retrieval on default/high/xhigh)
        // are not part of RunTelemetryReceiptV1's model-attempt ledger; they
        // are real provider calls and this receipt records that they are not
        // individually counted here. The rehearsal receipt prices them from
        // the fake server's endpoint counters.
        "embedding_calls_individually_counted": false,
    });
    let code = write_receipt(&out_dir, "execute-receipt.json", &receipt);
    println!(
        "execute wall {wall_ms}ms, provider calls {}, tokens {}",
        ledger.calls, ledger.tokens
    );
    code
}

fn flag_value(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .map(PathBuf::from)
}

pub fn product_eval_main(argv: Vec<String>) -> i32 {
    let Some(mode) = argv.first().cloned() else {
        eprintln!("usage: product-eval <preflight|rehearse|execute> --suite <path> --out <dir> [--preflight <receipt>]");
        return 2;
    };
    let args = &argv[1..];
    // Note: a bounded keychain read that times out appends its degraded-start
    // line to the REAL data root's startup.log (the product's own logger runs
    // before isolation); this is the driver's one accepted write outside the
    // out directory, and it is log-only.
    //
    // Bind the machine's real provider authority BEFORE isolating the data
    // root: preflight redacts it into the receipt and execute runs against it,
    // while every data-root-derived side effect (telemetry journal,
    // personalization, project-instruction config, memory read store) stays
    // hermetic inside the eval out directory.
    let provider_authority = matches!(mode.as_str(), "preflight" | "execute").then(|| {
        let mut config = crate::configuration_persistence::load_provider_config();
        crate::provider_secret_store::fill_api_key_from_keychain(&mut config);
        // The bounded keychain read (1.5s) cannot wait out an authorization
        // prompt, so the driver retries on a pace the operator can meet. The
        // window is --key-wait seconds (default 60 preflight / 900 execute):
        // a signed driver identity makes one "Always Allow" persist forever,
        // and a long execute window lets an unattended chain survive until the
        // operator returns and approves — every failed attempt stays
        // provider-call-free, and the mode's own config check fails closed if
        // the key never arrives.
        // --key-wait <seconds> bounds the whole window (default 60 preflight /
        // 900 execute). Pace: five quick 12s retries for an operator at the
        // screen, then 5-minute pacing so an overnight wait cannot stack a
        // dialog every few seconds. Every attempt is provider-call-free.
        let key_wait_seconds = flag_value(args, "--key-wait")
            .and_then(|value| value.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(if mode == "execute" { 900 } else { 60 });
        let deadline = Instant::now() + std::time::Duration::from_secs(key_wait_seconds);
        let mut attempt = 0u32;
        while config.api_key.trim().is_empty() && Instant::now() < deadline {
            attempt += 1;
            let pace_secs = if attempt <= 5 { 12 } else { 300 };
            eprintln!(
                "provider key not readable yet (attempt {attempt}, window {key_wait_seconds}s) — approve the keychain prompt if one is showing; retrying in {pace_secs}s"
            );
            std::thread::sleep(std::time::Duration::from_secs(pace_secs));
            crate::provider_secret_store::fill_api_key_from_keychain(&mut config);
        }
        config
    });
    if let Some(out) = flag_value(args, "--out").map(|path| path.join("data")) {
        if let Err(error) = std::fs::create_dir_all(&out) {
            eprintln!("cannot create data root {out:?}: {error}");
            return 2;
        }
        std::env::set_var("CINDX_DATA_DIR", &out);
        // The memory-recall path opens the app read store at the data root,
        // exactly like production; a fresh eval environment must have it
        // present-but-empty, which is the honest fresh-install posture and
        // identical for every arm.
        if let Err(error) = agent_storage::SqliteStore::open(out.join("state.sqlite3")) {
            eprintln!("cannot initialize eval data store: {error}");
            return 2;
        }
    }
    match mode.as_str() {
        "preflight" => preflight(
            args,
            provider_authority
                .as_ref()
                .expect("preflight binds the authority"),
        ),
        "rehearse" => rehearse(args),
        "execute" => execute(
            args,
            provider_authority.expect("execute binds the authority"),
        ),
        other => {
            eprintln!("unknown mode `{other}`");
            2
        }
    }
}
