//! Deterministic contract tests for the agent-eval skeleton. No provider
//! network calls: every run uses the scripted provider.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_eval::{
    check_postconditions, parse_suite, run_case, CaseBudget, CaseCategory, CaseReport, EvalCase,
    FixtureFile, Postcondition, ScriptedProvider, ScriptedStep, ScriptedToolCall, SuiteReport,
};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_root(test_name: &str) -> PathBuf {
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!(
        "cindx-agent-eval-{test_name}-{}-{sequence}-{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&root).expect("temp root should be created");
    root
}

fn suite_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("suite/general_v1.json")
}

fn load_seed_suite() -> Vec<EvalCase> {
    let json = std::fs::read_to_string(suite_path()).expect("seed suite should be readable");
    parse_suite(&json).expect("seed suite should parse")
}

fn tool_call(name: &str, arguments: serde_json::Value) -> ScriptedToolCall {
    ScriptedToolCall {
        name: name.to_string(),
        arguments_json: arguments.to_string(),
    }
}

fn read_step(path: &str) -> ScriptedStep {
    ScriptedStep::ToolCalls(vec![tool_call(
        "file.read",
        serde_json::json!({ "path": path }),
    )])
}

fn write_step(path: &str, content: &str) -> ScriptedStep {
    ScriptedStep::ToolCalls(vec![tool_call(
        "file.write",
        serde_json::json!({ "path": path, "content": content }),
    )])
}

fn shell_step(command: &str) -> ScriptedStep {
    ScriptedStep::ToolCalls(vec![tool_call(
        "shell.run",
        serde_json::json!({ "command": command }),
    )])
}

/// The exact script that satisfies each seeded case. The queue order is part
/// of the frozen suite contract.
fn scripted_provider_for(case_id: &str) -> ScriptedProvider {
    let steps = match case_id {
        "code_edit-fix-function" => vec![
            read_step("src/greeter.py"),
            write_step(
                "src/greeter.py",
                "def greet(name):\n    return \"Hello, \" + name\n\n\ndef farewell(name):\n    return \"Goodbye \" + name + \"!\"\n",
            ),
            ScriptedStep::Final(
                "Updated farewell in src/greeter.py to include the exclamation mark.".to_string(),
            ),
        ],
        "code_edit-rewrite-config" => vec![
            read_step("config/app.json"),
            write_step(
                "config/app.json",
                "{\n  \"mode\": \"release\",\n  \"retries\": 3\n}\n",
            ),
            ScriptedStep::Final(
                "Rewrote config/app.json for release mode with 3 retries.".to_string(),
            ),
        ],
        "multi_step-summarize-notes" => vec![
            read_step("notes/todo.txt"),
            read_step("notes/done.txt"),
            write_step(
                "summary.md",
                "done:\n- read the contract\ntodo:\n- ship the eval crate\n- write the docs\n",
            ),
            ScriptedStep::Final("Wrote summary.md from the notes directory.".to_string()),
        ],
        "multi_step-collect-versions" => vec![
            read_step("versions/libs.txt"),
            read_step("versions/app.txt"),
            write_step("versions/combined.txt", "agent-eval=0.1.0\nserde=1\nsha2=0.10\n"),
            ScriptedStep::Final(
                "Combined both version files into versions/combined.txt.".to_string(),
            ),
        ],
        "recovery-invalid-path" => vec![
            write_step("../outside.txt", "leak\n"),
            write_step("report.txt", "baseline\nrecovered\n"),
            ScriptedStep::Final(
                "Recovered after the rejected path and updated report.txt.".to_string(),
            ),
        ],
        "recovery-missing-file" => vec![
            read_step("data/expected.csv"),
            read_step("data/actual.csv"),
            ScriptedStep::Final(
                "The actual data has one row after the header: 1,2.".to_string(),
            ),
        ],
        "shell-write-output" => vec![
            shell_step("echo eval-ok > out.txt"),
            ScriptedStep::Final("Wrote out.txt via shell.run.".to_string()),
        ],
        "output-final-answer" => {
            vec![ScriptedStep::Final("The magic token is XYZZY-42.".to_string())]
        }
        other => panic!("no scripted provider for case {other}"),
    };
    ScriptedProvider::new(steps)
}

#[test]
fn schema_round_trip_preserves_cases() {
    let case = EvalCase {
        id: "round-trip".to_string(),
        category: CaseCategory::MultiStep,
        prompt: "do the thing".to_string(),
        fixture: vec![FixtureFile {
            path: "a/b.txt".to_string(),
            content: "content\n".to_string(),
        }],
        allowed_tools: vec!["file.read".to_string(), "file.write".to_string()],
        postconditions: vec![
            Postcondition::FileContains {
                path: "a/b.txt".to_string(),
                needle: "content".to_string(),
            },
            Postcondition::FileEquals {
                path: "a/b.txt".to_string(),
                content: "content\n".to_string(),
            },
            Postcondition::CommandExitCode {
                cmd: "true".to_string(),
                code: 0,
            },
            Postcondition::OutputContains {
                needle: "done".to_string(),
            },
        ],
        budget: CaseBudget {
            max_turns: 3,
            max_tool_calls: 4,
        },
    };

    let encoded = serde_json::to_string(&case).expect("case serializes");
    let decoded: EvalCase = serde_json::from_str(&encoded).expect("case deserializes");
    assert_eq!(case, decoded);

    let suite_encoded = serde_json::to_string(&vec![case.clone()]).expect("suite serializes");
    let decoded_suite = parse_suite(&suite_encoded).expect("suite deserializes");
    assert_eq!(decoded_suite, vec![case]);
}

#[test]
fn seed_suite_parses_with_eight_unique_cases_covering_every_category() {
    let cases = load_seed_suite();
    assert_eq!(cases.len(), 8, "general_v1 ships exactly eight cases");

    let mut ids = cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 8, "case ids must be unique");

    let categories = cases
        .iter()
        .map(|case| case.category)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        categories,
        std::collections::BTreeSet::from([
            CaseCategory::CodeEdit,
            CaseCategory::MultiStep,
            CaseCategory::Recovery,
            CaseCategory::Shell,
            CaseCategory::Output,
        ])
    );

    for case in &cases {
        assert!(
            !case.postconditions.is_empty(),
            "case {} must carry postconditions",
            case.id
        );
        assert!(case.budget.max_turns >= 1);
    }
}

#[test]
fn postcondition_checks_judge_each_type_deterministically() {
    let root = temp_root("postconditions");
    std::fs::write(root.join("note.txt"), "alpha beta\n").expect("fixture should be written");

    let checks = check_postconditions(
        &root,
        "final answer with the token",
        &[
            Postcondition::FileContains {
                path: "note.txt".to_string(),
                needle: "beta".to_string(),
            },
            Postcondition::FileContains {
                path: "note.txt".to_string(),
                needle: "gamma".to_string(),
            },
            Postcondition::FileContains {
                path: "missing.txt".to_string(),
                needle: "beta".to_string(),
            },
            Postcondition::FileEquals {
                path: "note.txt".to_string(),
                content: "alpha beta\n".to_string(),
            },
            Postcondition::FileEquals {
                path: "note.txt".to_string(),
                content: "alpha\n".to_string(),
            },
            Postcondition::CommandExitCode {
                cmd: "true".to_string(),
                code: 0,
            },
            Postcondition::CommandExitCode {
                cmd: "exit 3".to_string(),
                code: 0,
            },
            Postcondition::CommandExitCode {
                cmd: "exit 3".to_string(),
                code: 3,
            },
            Postcondition::OutputContains {
                needle: "the token".to_string(),
            },
            Postcondition::OutputContains {
                needle: "not present".to_string(),
            },
        ],
    );

    let verdicts: Vec<bool> = checks.iter().map(|check| check.passed).collect();
    assert_eq!(
        verdicts,
        vec![true, false, false, true, false, true, false, true, true, false]
    );
    for check in &checks {
        assert!(!check.detail.is_empty(), "every check explains itself");
    }
    // The failed exit-code check carries the observed code in its detail.
    assert!(checks[6].detail.contains("exit code 3"));
}

#[test]
fn command_postcondition_runs_inside_the_case_workspace() {
    let root = temp_root("command-cwd");
    std::fs::create_dir_all(root.join("sub")).expect("directory should be created");
    std::fs::write(root.join("sub/marker.txt"), "here\n").expect("fixture should be written");

    let checks = check_postconditions(
        &root,
        "",
        &[Postcondition::CommandExitCode {
            cmd: "test -f sub/marker.txt".to_string(),
            code: 0,
        }],
    );
    assert!(checks[0].passed, "relative paths resolve in the workspace");
}

#[test]
fn runner_passes_the_full_seed_suite_with_scripted_providers() {
    let root = temp_root("seed-suite");
    let cases = load_seed_suite();

    let mut reports = Vec::new();
    for case in &cases {
        let mut provider = scripted_provider_for(&case.id);
        let report = run_case(case, &mut provider, &root);
        assert!(
            report.error.is_none(),
            "case {} errored: {:?}",
            case.id,
            report.error
        );
        assert!(
            provider.remaining() == 0,
            "case {} consumed its script",
            case.id
        );
        reports.push(report);
    }

    let suite = SuiteReport::new(reports);
    assert_eq!(suite.total, 8);
    assert_eq!(
        suite.passed_count,
        8,
        "every seeded case passes: {}",
        suite.summary()
    );
    assert!(suite.all_passed());
    assert!(suite.summary().contains("suite: 8/8 cases passed"));
    assert!(suite.to_json().contains("\"passed_count\": 8"));

    // Accounting sanity: scripted tool calls and turns are observed.
    let by_id = |id: &str| suite.cases.iter().find(|case| case.id == id).unwrap();
    assert_eq!(by_id("code_edit-fix-function").tool_calls, 2);
    assert_eq!(by_id("multi_step-summarize-notes").tool_calls, 3);
    assert_eq!(by_id("recovery-invalid-path").tool_calls, 2);
    assert_eq!(by_id("shell-write-output").tool_calls, 1);
    assert_eq!(by_id("output-final-answer").tool_calls, 0);
    assert!(by_id("output-final-answer").turns >= 1);
    // The recovery case never materialized the rejected escape path.
    assert!(!root.join("recovery-invalid-path/../outside.txt").exists());
    assert!(!root.join("outside.txt").exists());
}

#[test]
fn runner_reports_tool_call_budget_exhaustion() {
    let root = temp_root("tool-budget");
    let case = EvalCase {
        id: "tool-budget".to_string(),
        category: CaseCategory::Shell,
        prompt: "run two commands".to_string(),
        fixture: vec![],
        allowed_tools: vec!["shell.run".to_string()],
        postconditions: vec![Postcondition::OutputContains {
            needle: "done".to_string(),
        }],
        budget: CaseBudget {
            max_turns: 6,
            max_tool_calls: 1,
        },
    };
    let mut provider = ScriptedProvider::new(vec![
        shell_step("printf one"),
        shell_step("printf two"),
        ScriptedStep::Final("done".to_string()),
    ]);

    let report = run_case(&case, &mut provider, &root);

    assert!(!report.passed);
    assert_eq!(report.tool_calls, 1, "only the first call executed");
    let error = report.error.expect("budget exhaustion is reported");
    assert!(
        error.contains("tool_call_budget_exhausted"),
        "error: {error}"
    );
}

#[test]
fn runner_reports_turn_budget_exhaustion() {
    let root = temp_root("turn-budget");
    let case = EvalCase {
        id: "turn-budget".to_string(),
        category: CaseCategory::Shell,
        prompt: "keep working".to_string(),
        fixture: vec![],
        allowed_tools: vec!["shell.run".to_string()],
        postconditions: vec![],
        budget: CaseBudget {
            max_turns: 1,
            max_tool_calls: 8,
        },
    };
    let mut provider = ScriptedProvider::new(vec![
        shell_step("printf one"),
        shell_step("printf two"),
        ScriptedStep::Final("never reached".to_string()),
    ]);

    let report = run_case(&case, &mut provider, &root);

    assert!(!report.passed);
    let error = report.error.expect("turn exhaustion is reported");
    assert!(error.contains("turn_budget_exhausted"), "error: {error}");
}

#[test]
fn runner_fails_closed_when_the_script_is_exhausted() {
    let root = temp_root("script-exhausted");
    let case = EvalCase {
        id: "script-exhausted".to_string(),
        category: CaseCategory::Shell,
        prompt: "run a command and continue".to_string(),
        fixture: vec![],
        allowed_tools: vec!["shell.run".to_string()],
        postconditions: vec![Postcondition::OutputContains {
            needle: "answer".to_string(),
        }],
        budget: CaseBudget {
            max_turns: 4,
            max_tool_calls: 4,
        },
    };
    // The script covers the tool round but has nothing left for the follow-up
    // model turn, so the provider must fail closed instead of improvising.
    let mut provider = ScriptedProvider::new(vec![shell_step("printf one")]);

    let report = run_case(&case, &mut provider, &root);

    assert!(!report.passed);
    assert_eq!(report.tool_calls, 1, "the scripted round still executed");
    let error = report.error.expect("script exhaustion is reported");
    assert!(error.contains("script_exhausted"), "error: {error}");
}

#[test]
fn runner_rejects_unknown_allowed_tools_before_the_loop() {
    let root = temp_root("unknown-tool");
    let case = EvalCase {
        id: "unknown-tool".to_string(),
        category: CaseCategory::Output,
        prompt: "answer".to_string(),
        fixture: vec![],
        allowed_tools: vec!["file.read".to_string(), "totally.missing".to_string()],
        postconditions: vec![],
        budget: CaseBudget {
            max_turns: 2,
            max_tool_calls: 1,
        },
    };
    let mut provider = ScriptedProvider::new(vec![ScriptedStep::Final("answer".to_string())]);

    let report = run_case(&case, &mut provider, &root);

    assert!(!report.passed);
    let error = report.error.expect("unknown tools fail closed");
    assert!(error.contains("unknown_tool"), "error: {error}");
    assert_eq!(report.turns, 0, "the loop never started");
}

#[test]
fn runner_rejects_fixture_paths_that_escape_the_case_workspace() {
    let root = temp_root("fixture-escape");
    let case = EvalCase {
        id: "fixture-escape".to_string(),
        category: CaseCategory::Output,
        prompt: "answer".to_string(),
        fixture: vec![FixtureFile {
            path: "../escape.txt".to_string(),
            content: "leak".to_string(),
        }],
        allowed_tools: vec![],
        postconditions: vec![],
        budget: CaseBudget {
            max_turns: 2,
            max_tool_calls: 0,
        },
    };
    let mut provider = ScriptedProvider::new(vec![ScriptedStep::Final("answer".to_string())]);

    let report = run_case(&case, &mut provider, &root);

    assert!(!report.passed);
    let error = report.error.expect("escape fixtures fail closed");
    assert!(error.contains("fixture_path_escape"), "error: {error}");
    assert!(!root.join("escape.txt").exists());
}

#[test]
fn runner_rejects_case_ids_that_escape_the_workspace_root() {
    // An external directory with a sentinel that must never be deleted: an
    // absolute or `..` case id would otherwise point `remove_dir_all` at it.
    let external = temp_root("case-id-external");
    let sentinel = external.join("keep.txt");
    std::fs::write(&sentinel, "do-not-delete\n").expect("sentinel written");
    let root = temp_root("case-id-escape");

    let absolute_case = EvalCase {
        id: external.to_string_lossy().to_string(),
        category: CaseCategory::Output,
        prompt: "answer".to_string(),
        fixture: vec![],
        allowed_tools: vec![],
        postconditions: vec![],
        budget: CaseBudget {
            max_turns: 1,
            max_tool_calls: 0,
        },
    };
    let mut provider = ScriptedProvider::new(vec![ScriptedStep::Final("answer".to_string())]);
    let report = run_case(&absolute_case, &mut provider, &root);
    assert!(!report.passed);
    let error = report.error.expect("absolute case id fails closed");
    assert!(error.contains("case_id_escape"), "error: {error}");

    let traversal_case = EvalCase {
        id: "../../etc".to_string(),
        ..absolute_case.clone()
    };
    let mut provider = ScriptedProvider::new(vec![ScriptedStep::Final("answer".to_string())]);
    let report = run_case(&traversal_case, &mut provider, &root);
    assert!(!report.passed);
    assert!(report.error.unwrap_or_default().contains("case_id_escape"));

    // Zero deletion: the external sentinel is byte-for-byte intact.
    assert!(sentinel.exists(), "external directory must not be deleted");
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "do-not-delete\n"
    );
}

#[test]
fn cases_are_workspace_isolated_and_reruns_are_reset() {
    let root = temp_root("isolation");
    let alpha = EvalCase {
        id: "alpha".to_string(),
        category: CaseCategory::CodeEdit,
        prompt: "write a marker file".to_string(),
        fixture: vec![FixtureFile {
            path: "seed.txt".to_string(),
            content: "alpha seed\n".to_string(),
        }],
        allowed_tools: vec!["file.write".to_string()],
        postconditions: vec![Postcondition::FileContains {
            path: "marker_a.txt".to_string(),
            needle: "from alpha".to_string(),
        }],
        budget: CaseBudget {
            max_turns: 4,
            max_tool_calls: 4,
        },
    };
    let beta = EvalCase {
        id: "beta".to_string(),
        category: CaseCategory::Output,
        prompt: "answer".to_string(),
        fixture: vec![FixtureFile {
            path: "seed.txt".to_string(),
            content: "beta seed\n".to_string(),
        }],
        allowed_tools: vec![],
        postconditions: vec![Postcondition::FileContains {
            path: "seed.txt".to_string(),
            needle: "beta seed".to_string(),
        }],
        budget: CaseBudget {
            max_turns: 2,
            max_tool_calls: 0,
        },
    };

    let mut alpha_provider = ScriptedProvider::new(vec![
        write_step("marker_a.txt", "from alpha\n"),
        ScriptedStep::Final("alpha done".to_string()),
    ]);
    let mut beta_provider =
        ScriptedProvider::new(vec![ScriptedStep::Final("beta done".to_string())]);

    let alpha_report = run_case(&alpha, &mut alpha_provider, &root);
    let beta_report = run_case(&beta, &mut beta_provider, &root);

    assert!(alpha_report.passed, "{alpha_report:?}");
    assert!(beta_report.passed, "{beta_report:?}");

    let alpha_root = root.join("alpha");
    let beta_root = root.join("beta");
    // Beta kept its own seed content and never saw alpha's marker.
    assert_eq!(
        std::fs::read_to_string(beta_root.join("seed.txt")).unwrap(),
        "beta seed\n"
    );
    assert!(!beta_root.join("marker_a.txt").exists());
    // Alpha keeps its own seed and marker.
    assert!(alpha_root.join("marker_a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(alpha_root.join("seed.txt")).unwrap(),
        "alpha seed\n"
    );

    // Rerunning alpha resets its workspace: no state accumulates across runs.
    std::fs::write(alpha_root.join("stale.txt"), "stale\n").expect("stale file written");
    let mut rerun_provider = ScriptedProvider::new(vec![
        write_step("marker_a.txt", "from alpha\n"),
        ScriptedStep::Final("alpha done".to_string()),
    ]);
    let rerun_report = run_case(&alpha, &mut rerun_provider, &root);
    assert!(rerun_report.passed, "{rerun_report:?}");
    assert!(!alpha_root.join("stale.txt").exists());
}

#[test]
fn case_report_serializes_for_suite_json_output() {
    let report = CaseReport {
        id: "json-case".to_string(),
        passed: true,
        checks: vec![],
        tool_calls: 2,
        turns: 3,
        error: None,
    };
    let suite = SuiteReport::new(vec![report]);
    let json = suite.to_json();
    let decoded: SuiteReport = serde_json::from_str(&json).expect("suite json round-trips");
    assert_eq!(decoded, suite);
}
