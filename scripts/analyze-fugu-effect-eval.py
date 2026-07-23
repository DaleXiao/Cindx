#!/usr/bin/env python3
"""Sanitize and summarize a private Cindx external-effect evaluation run."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import statistics
from collections import Counter, defaultdict
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any

RAW_SCHEMA = "cindx.external_effect_eval.raw.v1"
SANITIZED_SCHEMA = "cindx.external_effect_eval.sanitized.v1"


def percentile(values: list[float], quantile: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    rank = max(0, math.ceil(quantile * len(ordered)) - 1)
    return ordered[rank]


def wilson_interval(successes: int, total: int, z: float = 1.959963984540054) -> list[float] | None:
    if total <= 0:
        return None
    proportion = successes / total
    denominator = 1 + (z * z / total)
    center = (proportion + z * z / (2 * total)) / denominator
    margin = (
        z
        * math.sqrt(
            proportion * (1 - proportion) / total + z * z / (4 * total * total)
        )
        / denominator
    )
    return [max(0.0, center - margin), min(1.0, center + margin)]


def score_run(run: dict[str, Any]) -> float:
    if run["benchmark"] == "gpqa_diamond":
        return float(run.get("exact_score") or 0.0)
    if run["benchmark"] == "mrcr_v2_8_needle":
        if not run.get("prefix_valid"):
            return 0.0
        return SequenceMatcher(None, run.get("output", ""), run.get("expected", "")).ratio()
    raise ValueError(f"unsupported benchmark: {run['benchmark']}")


def sanitize_run(run: dict[str, Any]) -> dict[str, Any]:
    score = score_run(run)
    return {
        key: value
        for key, value in run.items()
        if key not in {"expected", "output"}
    } | {
        "score": score,
        "error_class": classify_error(run.get("error")),
    }


def classify_error(error: str | None) -> str | None:
    if not error:
        return None
    lowered = error.lower()
    if "timeout" in lowered or "timed out" in lowered or "deadline" in lowered:
        return "timeout"
    if "context" in lowered and ("length" in lowered or "window" in lowered):
        return "context_limit"
    if "plan" in lowered or "validation" in lowered:
        return "plan_or_protocol"
    if "cancel" in lowered:
        return "cancelled"
    if "http" in lowered or "curl" in lowered or "transport" in lowered:
        return "transport"
    return "other"


def aggregate(runs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    groups: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        groups[(run["benchmark"], run["treatment"])].append(run)

    summaries: list[dict[str, Any]] = []
    for (benchmark, treatment), items in sorted(groups.items()):
        scores = [float(item["score"]) for item in items]
        completed_scores = [
            float(item["score"]) for item in items if bool(item.get("succeeded"))
        ]
        latencies = [float(item["latency_ms"]) for item in items]
        first_tokens = [
            float(item["first_token_latency_ms"])
            for item in items
            if item.get("first_token_latency_ms") is not None
        ]
        exact_successes = sum(score == 1.0 for score in scores)
        token_complete = sum(int(item.get("total_tokens", 0)) > 0 for item in items)
        error_counts = Counter(
            item["error_class"] for item in items if item.get("error_class")
        )
        summaries.append(
            {
                "benchmark": benchmark,
                "treatment": treatment,
                "n": len(items),
                "completed": sum(bool(item.get("succeeded")) for item in items),
                "completion_rate": sum(bool(item.get("succeeded")) for item in items)
                / len(items)
                if items
                else 0.0,
                "mean_score": statistics.fmean(scores) if scores else None,
                "completed_mean_score": statistics.fmean(completed_scores)
                if completed_scores
                else None,
                "median_score": statistics.median(scores) if scores else None,
                "exact_successes": exact_successes if benchmark == "gpqa_diamond" else None,
                "wilson_95": wilson_interval(exact_successes, len(items))
                if benchmark == "gpqa_diamond"
                else None,
                "latency_ms_p50": percentile(latencies, 0.50),
                "latency_ms_p95": percentile(latencies, 0.95),
                "first_token_latency_ms_p50": percentile(first_tokens, 0.50),
                "total_tokens_sum": sum(int(item.get("total_tokens", 0)) for item in items),
                "token_telemetry_complete": token_complete,
                "token_telemetry_rate": token_complete / len(items) if items else 0.0,
                "error_counts": dict(sorted(error_counts.items())),
            }
        )
    return summaries


def exact_mcnemar_p_value(left_only: int, right_only: int) -> float:
    discordant = left_only + right_only
    if discordant == 0:
        return 1.0
    tail = sum(
        math.comb(discordant, index)
        for index in range(min(left_only, right_only) + 1)
    ) / (2**discordant)
    return min(1.0, 2 * tail)


def paired_gpqa_comparisons(runs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    by_treatment: dict[str, dict[str, bool]] = defaultdict(dict)
    for run in runs:
        if run["benchmark"] == "gpqa_diamond":
            by_treatment[run["treatment"]][run["case_id"]] = run["score"] == 1.0
    comparisons: list[dict[str, Any]] = []
    treatment_pairs = [
        ("direct_default", "cindx_auto"),
        ("direct_default", "cindx_pro"),
        ("cindx_auto", "cindx_pro"),
    ]
    for left, right in treatment_pairs:
        shared = sorted(set(by_treatment[left]) & set(by_treatment[right]))
        left_only = sum(by_treatment[left][case] and not by_treatment[right][case] for case in shared)
        right_only = sum(by_treatment[right][case] and not by_treatment[left][case] for case in shared)
        both_correct = sum(by_treatment[left][case] and by_treatment[right][case] for case in shared)
        both_incorrect = len(shared) - left_only - right_only - both_correct
        comparisons.append(
            {
                "left": left,
                "right": right,
                "n": len(shared),
                "left_only_correct": left_only,
                "right_only_correct": right_only,
                "both_correct": both_correct,
                "both_incorrect": both_incorrect,
                "exact_mcnemar_p": exact_mcnemar_p_value(left_only, right_only),
            }
        )
    return comparisons


def gpqa_domain_breakdown(runs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    groups: dict[tuple[str, str], list[dict[str, Any]]] = defaultdict(list)
    for run in runs:
        if run["benchmark"] == "gpqa_diamond":
            groups[(run["treatment"], run["category"])].append(run)

    rows: list[dict[str, Any]] = []
    for (treatment, category), items in sorted(groups.items()):
        completed = [item for item in items if item.get("succeeded")]
        rows.append(
            {
                "treatment": treatment,
                "category": category,
                "n": len(items),
                "completed": len(completed),
                "exact_successes": sum(item["score"] == 1.0 for item in items),
                "budgeted_score": statistics.fmean(item["score"] for item in items),
                "completed_score": statistics.fmean(item["score"] for item in completed)
                if completed
                else None,
            }
        )
    return rows


def fmt_percent(value: float | None) -> str:
    return "n/a" if value is None else f"{value * 100:.2f}%"


def fmt_ms(value: float | None) -> str:
    if value is None:
        return "n/a"
    if value >= 1000:
        return f"{value / 1000:.1f}s"
    return f"{value:.0f}ms"


def render_markdown(report: dict[str, Any], output_path: Path) -> None:
    aggregate_by_benchmark: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in report["aggregates"]:
        aggregate_by_benchmark[row["benchmark"]].append(row)

    lines = [
        "# Cindx Fugu External Effect Pilot V1",
        "",
        f"- Generated: `{report['generated_at_ms']}`",
        f"- Source commit: `{report['git_commit']}`",
        f"- App crate version: `{report['app_version']}`",
        f"- Raw evidence SHA-256: `{report['raw_evidence_sha256']}`",
        f"- Evaluation budget: {report['evaluation_limits'].get('model_call_timeout_seconds', 'n/a')}s per model call; {report['evaluation_limits'].get('treatment_deadline_seconds', 'n/a')}s per treatment.",
        "- Scope: fixed-sample external-effect pilot, not a full leaderboard submission.",
        "",
        "## Protocol",
        "",
        "- **GPQA-Diamond:** deterministic stratified sample across Biology, Chemistry, and Physics; EvalScope-compatible zero-shot prompt; no tools; exact answer parsing.",
        "- **MRCR v2:** official multi-message transcript preserved; 8 needles; official random-prefix gate and Python `difflib.SequenceMatcher` ratio.",
        "- Raw benchmark prompts, expected answers, and complete model outputs remain outside the repository. Committed evidence contains hashes and scores only.",
        "- References: [Fugu technical report](https://arxiv.org/abs/2606.21228), [GPQA repository](https://github.com/idavidrein/gpqa), and [MRCR v2 dataset](https://huggingface.co/datasets/openai/mrcr).",
        "",
    ]

    for benchmark, title in [
        ("gpqa_diamond", "GPQA-Diamond"),
        ("mrcr_v2_8_needle", "MRCR v2 (8-needle)"),
    ]:
        lines.extend(
            [
                f"## {title}",
                "",
                "| Treatment | n | Completed | Budgeted score | Completed-only | 95% CI | p50 latency | p95 latency | Failures | Token telemetry |",
                "| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | ---: |",
            ]
        )
        for row in aggregate_by_benchmark.get(benchmark, []):
            confidence = "n/a"
            if row.get("wilson_95"):
                confidence = f"{fmt_percent(row['wilson_95'][0])} to {fmt_percent(row['wilson_95'][1])}"
            lines.append(
                "| {treatment} | {n} | {completed}/{n} | {score} | {completed_score} | {confidence} | {p50} | {p95} | {failures} | {telemetry}/{n} |".format(
                    treatment=row["treatment"],
                    n=row["n"],
                    completed=row["completed"],
                    score=fmt_percent(row["mean_score"]),
                    completed_score=fmt_percent(row["completed_mean_score"]),
                    confidence=confidence,
                    p50=fmt_ms(row["latency_ms_p50"]),
                    p95=fmt_ms(row["latency_ms_p95"]),
                    failures=", ".join(
                        f"{kind}: {count}" for kind, count in row["error_counts"].items()
                    )
                    or "none",
                    telemetry=row["token_telemetry_complete"],
                )
            )
        lines.append("")

    lines.extend(
        [
            "## GPQA-Diamond by Domain (Diagnostic)",
            "",
            "This breakdown is diagnostic only: each cell contains four frozen questions, so differences are highly uncertain.",
            "",
            "| Treatment | Domain | Completed | Correct | Budgeted score | Completed-only |",
            "| --- | --- | ---: | ---: | ---: | ---: |",
        ]
    )
    for row in report["gpqa_domains"]:
        lines.append(
            "| {treatment} | {category} | {completed}/{n} | {exact_successes}/{n} | {budgeted_score} | {completed_score} |".format(
                **{
                    **row,
                    "budgeted_score": fmt_percent(row["budgeted_score"]),
                    "completed_score": fmt_percent(row["completed_score"]),
                }
            )
        )
    lines.append("")

    aggregates = {
        (row["benchmark"], row["treatment"]): row for row in report["aggregates"]
    }
    direct_gpqa = aggregates[("gpqa_diamond", "direct_default")]
    auto_gpqa = aggregates[("gpqa_diamond", "cindx_auto")]
    pro_gpqa = aggregates[("gpqa_diamond", "cindx_pro")]
    direct_mrcr = aggregates[("mrcr_v2_8_needle", "direct_default")]
    auto_mrcr = aggregates[("mrcr_v2_8_needle", "cindx_auto_model_route")]
    mrcr_models = sorted(
        {
            model
            for run in report["runs"]
            if run["benchmark"] == "mrcr_v2_8_needle"
            for model in run.get("models", [])
        }
    )
    gpqa_telemetry = sum(
        row["token_telemetry_complete"]
        for row in report["aggregates"]
        if row["benchmark"] == "gpqa_diamond"
    )
    gpqa_runs = sum(
        row["n"]
        for row in report["aggregates"]
        if row["benchmark"] == "gpqa_diamond"
    )
    mrcr_telemetry = sum(
        row["token_telemetry_complete"]
        for row in report["aggregates"]
        if row["benchmark"] == "mrcr_v2_8_needle"
    )
    mrcr_runs = sum(
        row["n"]
        for row in report["aggregates"]
        if row["benchmark"] == "mrcr_v2_8_needle"
    )
    lines.extend(
        [
            "## Observed Findings",
            "",
            f"- Under the fixed budget, the direct GPQA baseline scored **{fmt_percent(direct_gpqa['mean_score'])}**, versus **{fmt_percent(auto_gpqa['mean_score'])}** for Cindx Auto and **{fmt_percent(pro_gpqa['mean_score'])}** for Cindx Pro. This pilot therefore does not demonstrate orchestration uplift.",
            f"- Cindx Pro completed **{pro_gpqa['completed']}/{pro_gpqa['n']}** GPQA cases. Every completed case was correct (**{fmt_percent(pro_gpqa['completed_mean_score'])}** completed-only), while p50 and p95 both reached the {report['evaluation_limits'].get('treatment_deadline_seconds', 'n/a')}s treatment cap. The dominant observed failure is delivery within budget, not completed-answer accuracy.",
            f"- Cindx Auto completed **{auto_gpqa['completed']}/{auto_gpqa['n']}** GPQA cases and answered **{fmt_percent(auto_gpqa['completed_mean_score'])}** of completed cases correctly. Chemistry was the weakest diagnostic slice; the sample is too small for a domain-level conclusion.",
            f"- MRCR scored **{fmt_percent(direct_mrcr['mean_score'])}** for direct and **{fmt_percent(auto_mrcr['mean_score'])}** for Auto across all four frozen length points. Both treatments selected `{', '.join(mrcr_models)}`, so this confirms strong long-context behavior but measures no routing uplift.",
            f"- Token telemetry was recorded for **{gpqa_telemetry}/{gpqa_runs}** GPQA runs and **{mrcr_telemetry}/{mrcr_runs}** MRCR runs. Runs without final usage make treatment token totals lower bounds, so this pilot cannot support a complete cost-efficiency comparison.",
            "- None of the paired GPQA comparisons reached conventional significance; the exact tests are included only to prevent overclaiming from 12 questions.",
            "",
        ]
    )

    lines.extend(
        [
            "## GPQA Paired Comparisons (Exploratory)",
            "",
            "Each comparison uses the same frozen questions and treats deadline failures as incorrect. Exact McNemar p-values are descriptive only at this sample size.",
            "",
            "| Left | Right | n | Left only correct | Right only correct | Both correct | Both incorrect | Exact p |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for row in report["paired_gpqa"]:
        lines.append(
            "| {left} | {right} | {n} | {left_only_correct} | {right_only_correct} | {both_correct} | {both_incorrect} | {exact_mcnemar_p:.3f} |".format(
                **row
            )
        )
    lines.append("")

    lines.extend(
        [
            "## Interpretation Boundaries",
            "",
            "- This pilot estimates behavior on a small, frozen sample. It must not be compared numerically with Fugu-Ultra's full-table scores as if sample size, model access, and serving stack were identical.",
            "- GPQA treatments compare a direct configured model with Cindx Auto routing/workflow and Cindx Pro Conductor execution. Different role models are part of the product treatment and therefore also a confound when attributing uplift solely to orchestration.",
            "- MRCR preserves the official raw multi-message protocol. `cindx_auto_model_route` evaluates model selection only; a Pro workflow was intentionally omitted because flattening or rewriting the transcript would change the benchmark protocol.",
            "- LiveCodeBench and SciCode were not run because this machine did not expose a trusted disposable code-execution container. Untrusted generated code was never executed on the host.",
            "- The frozen Fugu parity matrix remains separate; this pilot does not convert subset results into protocol-equivalent parity cells.",
            "",
            "## Decision and Next Gate",
            "",
            "- Do not claim Fugu parity or an Auto/Pro quality uplift from this pilot.",
            "- Treat Pro deadline-aware scheduling and fail-soft aggregation as the highest-priority harness issue, then rerun the identical frozen sample before expanding it.",
            "- Instrument complete usage telemetry and inspect Auto's answer aggregation, especially the Chemistry failures, before making cost or router-quality claims.",
            "- Add a trusted disposable code sandbox before enabling LiveCodeBench or SciCode; generated benchmark code must remain off the host system.",
            "",
            "## Reproduction",
            "",
            "1. Obtain GPQA-Diamond from the pinned public repository revision and MRCR v2 from the official Hugging Face dataset.",
            "2. Run the ignored Rust test `provider_backed_fugu_external_effect_pilot` with the documented dataset environment variables.",
            "3. Run `scripts/analyze-fugu-effect-eval.py` on the private raw result to produce the sanitized JSON and this report.",
            "",
        ]
    )
    output_path.write_text("\n".join(lines), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("raw", type=Path)
    parser.add_argument("sanitized", type=Path)
    parser.add_argument("--markdown", type=Path)
    args = parser.parse_args()

    raw_bytes = args.raw.read_bytes()
    raw = json.loads(raw_bytes)
    if raw.get("schema") != RAW_SCHEMA:
        raise SystemExit(f"unexpected raw schema: {raw.get('schema')}")
    sanitized_runs = [sanitize_run(run) for run in raw["runs"]]
    report = {
        "schema": SANITIZED_SCHEMA,
        "generated_at_ms": raw["generated_at_ms"],
        "git_commit": raw["git_commit"],
        "app_version": raw["app_version"],
        "provider_endpoint": raw["provider_endpoint"],
        "configured_models": raw["configured_models"],
        "evaluation_limits": raw.get("evaluation_limits", {}),
        "raw_evidence_sha256": hashlib.sha256(raw_bytes).hexdigest(),
        "sources": raw["sources"],
        "aggregates": aggregate(sanitized_runs),
        "gpqa_domains": gpqa_domain_breakdown(sanitized_runs),
        "paired_gpqa": paired_gpqa_comparisons(sanitized_runs),
        "runs": sanitized_runs,
    }
    args.sanitized.parent.mkdir(parents=True, exist_ok=True)
    args.sanitized.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if args.markdown:
        args.markdown.parent.mkdir(parents=True, exist_ok=True)
        render_markdown(report, args.markdown)


if __name__ == "__main__":
    main()
