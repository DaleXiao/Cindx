#!/usr/bin/env python3
"""Sanitize and summarize the private Cindx Pilot v2 evidence file."""

from __future__ import annotations

import argparse
import copy
import difflib
import json
import math
import statistics
from collections import defaultdict
from pathlib import Path


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, math.ceil(len(ordered) * fraction) - 1))
    return ordered[index]


def run_score(run: dict) -> float:
    if run.get("task_score") is not None:
        return float(run["task_score"])
    if not run.get("succeeded"):
        return 0.0
    if run.get("benchmark") == "cindx_read_only_workspace":
        expected = [
            value.strip().lower()
            for value in run.get("expected", "").splitlines()
            if value.strip()
        ]
        output = run.get("output", "").lower()
        return (
            sum(value in output for value in expected) / len(expected)
            if expected
            else 0.0
        )
    exact = run.get("exact_score")
    if exact is not None:
        return float(exact)
    if run.get("benchmark") == "mrcr_v2_8_needle":
        if not run.get("prefix_valid"):
            return 0.0
        return difflib.SequenceMatcher(
            None, run.get("expected", ""), run.get("output", "")
        ).ratio()
    return 0.0


def treatment_summary(runs: list[dict]) -> dict:
    latencies = [float(run.get("latency_ms", 0)) for run in runs]
    scores = [run_score(run) for run in runs]
    delivered = sum(bool(run.get("succeeded")) for run in runs)
    return {
        "runs": len(runs),
        "delivered": delivered,
        "delivery_rate": round(delivered / len(runs), 4) if runs else 0.0,
        "mean_quality": round(statistics.fmean(scores), 4) if scores else 0.0,
        "median_latency_ms": round(statistics.median(latencies)) if latencies else 0,
        "p95_latency_ms": round(percentile(latencies, 0.95)) if latencies else 0,
        "total_tokens": sum(int(run.get("total_tokens", 0)) for run in runs),
        "tool_calls": sum(
            len(step.get("tool_calls", []))
            for run in runs
            for step in run.get("steps", [])
        ),
        "safety_violations": sum(int(run.get("safety_violations", 0)) for run in runs),
        "workflow_trace_coverage": round(
            sum(bool(run.get("steps")) for run in runs) / len(runs), 4
        )
        if runs
        else 0.0,
    }


def failure_kind(run: dict) -> str:
    if run.get("succeeded") and run_score(run) < 1.0:
        return "incomplete_answer"
    error = (run.get("error") or "").lower()
    if "cancelled" in error or "timeout" in error:
        return "deadline_or_cancellation"
    if not run.get("succeeded") and not (run.get("output") or "").strip():
        return "empty_output"
    if not run.get("succeeded"):
        return "execution_failure"
    return "none"


def diagnostic_run(run: dict) -> dict:
    return {
        "replicate": int(run.get("replicate", 1)),
        "benchmark": run.get("benchmark"),
        "case_id": run.get("case_id"),
        "treatment": run.get("treatment"),
        "succeeded": bool(run.get("succeeded")),
        "task_score": round(run_score(run), 6),
        "latency_ms": int(run.get("latency_ms", 0)),
        "total_tokens": int(run.get("total_tokens", 0)),
        "failure_kind": failure_kind(run),
        "safety_violations": int(run.get("safety_violations", 0)),
    }


def sanitize(report: dict, rerun_report: dict | None = None) -> dict:
    clean = copy.deepcopy(report)
    for run in clean.get("runs", []):
        run["task_score"] = round(run_score(run), 6)
        run.pop("expected", None)
        run.pop("output", None)
        for step in run.get("steps", []):
            step.pop("prompt", None)
            step.pop("output", None)
            for call in step.get("tool_calls", []):
                call["request_sha256"] = __import__("hashlib").sha256(
                    call.get("request", "").encode()
                ).hexdigest()
                call["response_sha256"] = __import__("hashlib").sha256(
                    call.get("response", "").encode()
                ).hexdigest()
                call.pop("request", None)
                call.pop("response", None)
    groups: dict[str, list[dict]] = defaultdict(list)
    for run in clean.get("runs", []):
        groups[run["treatment"]].append(run)
    summaries = {name: treatment_summary(values) for name, values in groups.items()}
    all_runs = clean.get("runs", [])
    complete_matrix = len(all_runs) == 32 and all(
        len(groups.get(treatment, [])) == 8
        for treatment in (
            "single_model_baseline",
            "cindx_fast",
            "cindx_auto",
            "cindx_pro",
        )
    )
    no_safety_violations = all(
        int(run.get("safety_violations", 0)) == 0 for run in all_runs
    )
    delivery_floor_met = bool(summaries) and all(
        summary["delivery_rate"] >= 0.75 for summary in summaries.values()
    )
    baseline = summaries.get("single_model_baseline", {})
    relative_to_baseline = {}
    for treatment in ("cindx_fast", "cindx_auto", "cindx_pro"):
        summary = summaries.get(treatment, {})
        relative_to_baseline[treatment] = {
            "delivery_gap": round(
                float(baseline.get("delivery_rate", 0.0))
                - float(summary.get("delivery_rate", 0.0)),
                4,
            ),
            "quality_gap": round(
                float(baseline.get("mean_quality", 0.0))
                - float(summary.get("mean_quality", 0.0)),
                4,
            ),
            "latency_ratio": round(
                float(summary.get("median_latency_ms", 0))
                / max(float(baseline.get("median_latency_ms", 0)), 1.0),
                3,
            ),
            "token_ratio": round(
                float(summary.get("total_tokens", 0))
                / max(float(baseline.get("total_tokens", 0)), 1.0),
                3,
            ),
        }
    noninferiority_met = bool(relative_to_baseline) and all(
        comparison["delivery_gap"] <= 0.10
        and comparison["quality_gap"] <= 0.10
        for comparison in relative_to_baseline.values()
    )
    first_pass_diagnostics = [
        diagnostic_run(run)
        for run in report.get("runs", [])
        if not run.get("succeeded") or run_score(run) < 1.0
    ]
    targeted_reruns = []
    if rerun_report is not None:
        if rerun_report.get("schema") != report.get("schema"):
            raise SystemExit("targeted rerun schema does not match first pass")
        targeted_reruns = [
            diagnostic_run(run) for run in rerun_report.get("runs", [])
        ]
    first_pass_by_key = {
        (item["case_id"], item["treatment"]): item
        for item in first_pass_diagnostics
    }
    persistent_anomalies = []
    recovered_anomalies = []
    for item in targeted_reruns:
        key = (item["case_id"], item["treatment"])
        if key not in first_pass_by_key:
            continue
        if item["succeeded"] and item["task_score"] == 1.0:
            recovered_anomalies.append(item)
        else:
            persistent_anomalies.append(item)
    clean["analysis"] = {
        "treatments": summaries,
        "matrix_complete": complete_matrix,
        "no_safety_violations": no_safety_violations,
        "delivery_floor_met": delivery_floor_met,
        "noninferiority_threshold": 0.10,
        "noninferiority_met": noninferiority_met,
        "relative_to_baseline": relative_to_baseline,
        "first_pass_diagnostics": first_pass_diagnostics,
        "targeted_reruns": targeted_reruns,
        "persistent_anomalies": persistent_anomalies,
        "recovered_anomalies": recovered_anomalies,
        "go_for_full_evaluation": complete_matrix
        and no_safety_violations
        and delivery_floor_met
        and noninferiority_met,
        "interpretation_boundary": "This pilot validates evaluator readiness and Cindx treatment behavior. It is not a statistically powered Fugu parity claim.",
    }
    return clean


def markdown_report(report: dict) -> str:
    analysis = report["analysis"]
    expected_runs = int(
        report.get("evaluation_limits", {}).get("first_pass_run_cap", 32)
    )
    if not analysis["matrix_complete"]:
        decision = (
            "This is a partial diagnostic matrix. It cannot support a full-benchmark "
            "go/no-go decision."
        )
    elif analysis["go_for_full_evaluation"]:
        decision = "Proceed to the full benchmark."
    else:
        decision = (
            "Hold the full benchmark. The complete pilot found reproducible harness "
            "regressions that should be corrected first."
        )
    lines = [
        "# Cindx Pilot v2 Evaluation",
        "",
        f"- App: `{report.get('app_version', 'unknown')}`",
        f"- Commit: `{report.get('git_commit', 'unknown')}`",
        f"- Runs: `{len(report.get('runs', []))}` / {expected_runs}",
        f"- GEPA frozen: `{str(report.get('gepa_frozen', False)).lower()}`",
        f"- Safety violations: `{sum(item['safety_violations'] for item in analysis['treatments'].values())}`",
        f"- Full-evaluation gate: `{'GO' if analysis['go_for_full_evaluation'] else 'NO-GO'}`",
        "",
        "## Decision",
        "",
        decision,
        "",
        f"The targeted second pass confirmed `{len(analysis['persistent_anomalies'])}` persistent anomalies and "
        f"recovered `{len(analysis['recovered_anomalies'])}` first-pass anomalies.",
        "",
        "## Treatment Results",
        "",
        "| Treatment | Delivery | Mean quality | Median latency | p95 latency | Tokens |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    order = ["single_model_baseline", "cindx_fast", "cindx_auto", "cindx_pro"]
    for treatment in order:
        summary = analysis["treatments"].get(treatment)
        if not summary:
            continue
        lines.append(
            f"| {treatment} | {summary['delivered']}/{summary['runs']} ({summary['delivery_rate']:.0%}) | "
            f"{summary['mean_quality']:.3f} | {summary['median_latency_ms'] / 1000:.1f}s | "
            f"{summary['p95_latency_ms'] / 1000:.1f}s | {summary['total_tokens']:,} |"
        )
    comparisons = analysis["relative_to_baseline"]
    lines.extend(
        [
            "",
            "## Relative Cost And Regression",
            "",
            "| Treatment | Delivery gap | Quality gap | Median latency ratio | Token ratio | Tool calls |",
            "|---|---:|---:|---:|---:|---:|",
        ]
    )
    for treatment in order[1:]:
        if treatment not in comparisons or treatment not in analysis["treatments"]:
            continue
        comparison = comparisons[treatment]
        summary = analysis["treatments"][treatment]
        lines.append(
            f"| {treatment} | {comparison['delivery_gap']:.1%} | "
            f"{comparison['quality_gap']:.1%} | {comparison['latency_ratio']:.2f}x | "
            f"{comparison['token_ratio']:.2f}x | {summary['tool_calls']} |"
        )
    lines.extend(
        [
            "",
            "## First-Pass Exceptions",
            "",
            "| Case | Treatment | Result | Quality | Latency | Tokens | Classification |",
            "|---|---|---:|---:|---:|---:|---|",
        ]
    )
    for item in analysis["first_pass_diagnostics"]:
        lines.append(
            f"| {item['case_id']} | {item['treatment']} | "
            f"{'delivered' if item['succeeded'] else 'failed'} | {item['task_score']:.3f} | "
            f"{item['latency_ms'] / 1000:.1f}s | {item['total_tokens']:,} | "
            f"{item['failure_kind']} |"
        )
    if analysis["targeted_reruns"]:
        lines.extend(
            [
                "",
                "## Targeted Second Pass",
                "",
                "Second-pass cells are diagnostic only and do not overwrite the first-pass matrix.",
                "",
                "| Case | Treatment | Result | Quality | Latency | Tokens | Classification |",
                "|---|---|---:|---:|---:|---:|---|",
            ]
        )
        for item in analysis["targeted_reruns"]:
            lines.append(
                f"| {item['case_id']} | {item['treatment']} | "
                f"{'delivered' if item['succeeded'] else 'failed'} | {item['task_score']:.3f} | "
                f"{item['latency_ms'] / 1000:.1f}s | {item['total_tokens']:,} | "
                f"{item['failure_kind']} |"
            )
        lines.extend(
            [
                "",
                f"Persistent anomalies: `{len(analysis['persistent_anomalies'])}`",
                f"Recovered on second pass: `{len(analysis['recovered_anomalies'])}`",
                "",
                "## Root-Cause Signals",
                "",
            ]
        )
        first_pass_by_key = {
            (item["case_id"], item["treatment"]): item
            for item in analysis["first_pass_diagnostics"]
        }
        for outcome, label in (
            (analysis["persistent_anomalies"], "Persistent"),
            (analysis["recovered_anomalies"], "Recovered"),
        ):
            for item in outcome:
                first = first_pass_by_key[(item["case_id"], item["treatment"])]
                lines.append(
                    f"- {label}: `{item['case_id']}` / `{item['treatment']}` moved from "
                    f"`{first['failure_kind']}` at quality `{first['task_score']:.3f}` to "
                    f"`{item['failure_kind']}` at quality `{item['task_score']:.3f}` on the second pass."
                )
    lines.extend(
        [
            "",
            "## Scope And Interpretation",
            "",
            "- GPQA and MRCR cases come from pinned official sources; MRCR transcripts remain unchanged.",
            "- The three workspace cases are deterministic, temporary, and read-only.",
            "- MRCR Pro is a protocol-preserving worker proxy, not multi-model synthesis; it must not be cited as Pro orchestration uplift.",
            "- GPQA and workspace Cindx rows are product-mechanism evidence, not official Fugu parity evidence.",
            "- The first-pass matrix is intentionally small. Any quality ranking is directional, not statistically conclusive.",
            "",
            "## Gate",
            "",
            f"Matrix complete: `{analysis['matrix_complete']}`",
            f"Safety boundary clean: `{analysis['no_safety_violations']}`",
            f"Every treatment delivered at least 75%: `{analysis['delivery_floor_met']}`",
            f"No Cindx mode regressed by more than 10 percentage points versus baseline: `{analysis['noninferiority_met']}`",
            "",
            "`GO` requires all four conditions above. A `NO-GO` means fix the harness before spending on the full benchmark; it does not mean the product has no useful capabilities.",
            "",
            analysis["interpretation_boundary"],
            "",
        ]
    )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("raw", type=Path)
    parser.add_argument("sanitized_json", type=Path)
    parser.add_argument("markdown", type=Path)
    parser.add_argument("--rerun", type=Path)
    args = parser.parse_args()
    report = json.loads(args.raw.read_text())
    if report.get("schema") not in {
        "cindx.pilot_v2.raw.v1",
        "cindx.pilot_v2.raw.v2",
    }:
        raise SystemExit("unexpected Pilot v2 schema")
    rerun_report = json.loads(args.rerun.read_text()) if args.rerun else None
    clean = sanitize(report, rerun_report)
    args.sanitized_json.parent.mkdir(parents=True, exist_ok=True)
    args.markdown.parent.mkdir(parents=True, exist_ok=True)
    args.sanitized_json.write_text(json.dumps(clean, indent=2, ensure_ascii=False) + "\n")
    args.markdown.write_text(markdown_report(clean))


if __name__ == "__main__":
    main()
