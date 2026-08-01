# Cindx Agent Engine 0.1.47 GPQA Matched Evaluation

- Generated: `1785063240245`
- Source commit: `5b1dcd69ea94289ba2721470260c4ca6283cf673`
- App crate version: `0.1.47`
- Raw evidence SHA-256: `1627cb2f3aa9187d73a84f1ec3f8026297e1ba26551fca20621714e4dca82bce`
- Evaluation budget: 180s per model call; 300s per treatment; 4096 output tokens per call.
- Scope: fixed-sample external-effect pilot, not a full leaderboard submission.

## Protocol

- **GPQA-Diamond:** deterministic stratified sample across Biology, Chemistry, and Physics; EvalScope-compatible zero-shot prompt; no tools; exact answer parsing.
- Raw benchmark prompts, expected answers, and complete model outputs remain outside the repository. Committed evidence contains hashes and scores only.
- References: [Fugu technical report](https://arxiv.org/abs/2606.21228), [GPQA repository](https://github.com/idavidrein/gpqa).

## GPQA-Diamond

| Treatment | n | Completed | Budgeted score | Completed-only | 95% CI | p50 latency | p95 latency | Failures | Token telemetry |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | ---: |
| cindx_auto | 12 | 10/12 | 75.00% | 90.00% | 46.77% to 91.11% | 120.0s | 180.0s | cancelled: 2 | 10/12 |
| cindx_pro | 12 | 9/12 | 66.67% | 88.89% | 39.06% to 86.19% | 160.1s | 300.0s | timeout: 3 | 12/12 |
| direct_default | 12 | 8/12 | 66.67% | 100.00% | 39.06% to 86.19% | 38.6s | 180.0s | cancelled: 4 | 8/12 |

## GPQA-Diamond by Domain (Diagnostic)

This breakdown is diagnostic only: each cell contains 4 frozen questions, so differences are highly uncertain.

| Treatment | Domain | Completed | Correct | Budgeted score | Completed-only |
| --- | --- | ---: | ---: | ---: | ---: |
| cindx_auto | Biology | 4/4 | 4/4 | 100.00% | 100.00% |
| cindx_auto | Chemistry | 2/4 | 1/4 | 25.00% | 50.00% |
| cindx_auto | Physics | 4/4 | 4/4 | 100.00% | 100.00% |
| cindx_pro | Biology | 3/4 | 3/4 | 75.00% | 100.00% |
| cindx_pro | Chemistry | 2/4 | 1/4 | 25.00% | 50.00% |
| cindx_pro | Physics | 4/4 | 4/4 | 100.00% | 100.00% |
| direct_default | Biology | 4/4 | 4/4 | 100.00% | 100.00% |
| direct_default | Chemistry | 0/4 | 0/4 | 0.00% | n/a |
| direct_default | Physics | 4/4 | 4/4 | 100.00% | 100.00% |

## Observed Findings

- Under the fixed budget, the direct GPQA baseline scored **66.67%**, versus **75.00%** for Cindx Auto and **66.67%** for Cindx Pro. An orchestration uplift was observed on this sample, but requires paired significance and replication before it can support a product claim.
- Cindx Pro completed **9/12** GPQA cases and answered **88.89%** of completed cases correctly; p50 latency was 160.1s and p95 was 300.0s, reaching the 300s treatment cap.
- Cindx Auto completed **10/12** GPQA cases and answered **90.00%** of completed cases correctly. Chemistry was the weakest diagnostic slice; the sample is too small for a domain-level conclusion.
- Token telemetry was recorded for **30/36** GPQA runs. Runs without final usage make treatment token totals lower bounds, so incomplete telemetry cannot support a complete cost-efficiency comparison.
- Paired GPQA significance is reported with exact McNemar tests to prevent overclaiming from 12 questions.

## GPQA Paired Comparisons (Exploratory)

Each comparison uses the same frozen questions and treats deadline failures as incorrect. Exact McNemar p-values are descriptive only at this sample size.

| Left | Right | n | Left only correct | Right only correct | Both correct | Both incorrect | Exact p |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| direct_default | cindx_auto | 12 | 0 | 1 | 8 | 3 | 1.000 |
| direct_default | cindx_pro | 12 | 1 | 1 | 7 | 3 | 1.000 |
| cindx_auto | cindx_pro | 12 | 1 | 0 | 8 | 3 | 1.000 |

## Interpretation Boundaries

- This pilot estimates behavior on a small, frozen sample. It must not be compared numerically with Fugu-Ultra's full-table scores as if sample size, model access, and serving stack were identical.
- GPQA treatments compare a direct configured model with Cindx Auto routing/workflow and Cindx Pro Conductor execution. Different role models are part of the product treatment and therefore also a confound when attributing uplift solely to orchestration.
- LiveCodeBench and SciCode were not run because this machine did not expose a trusted disposable code-execution container. Untrusted generated code was never executed on the host.
- The frozen Fugu parity matrix remains separate; this pilot does not convert subset results into protocol-equivalent parity cells.

## Decision and Next Gate

- Do not claim Fugu parity or an Auto/Pro quality uplift from this pilot.
- Treat Pro deadline-aware scheduling and fail-soft aggregation as the highest-priority harness issue, then rerun the identical frozen sample before expanding it.
- Instrument complete usage telemetry and inspect Auto's answer aggregation, especially the Chemistry failures, before making cost or router-quality claims.
- Add a trusted disposable code sandbox before enabling LiveCodeBench or SciCode; generated benchmark code must remain off the host system.

## Reproduction

1. Obtain GPQA-Diamond from the pinned public repository revision.
2. Run the ignored Rust test `provider_backed_fugu_external_effect_pilot` with the documented dataset environment variables.
3. Run `scripts/analyze-fugu-effect-eval.py` on the private raw result to produce the sanitized JSON and this report.
