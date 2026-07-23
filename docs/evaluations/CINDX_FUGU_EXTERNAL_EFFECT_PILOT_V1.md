# Cindx Fugu External Effect Pilot V1

- Generated: `1784785728241`
- Source commit: `cf7850510630b924b92043c30071f11fa1083bca`
- App crate version: `0.1.25`
- Raw evidence SHA-256: `e6c392ab8e97ea5fbee5da5db103a7c9c812e4b7dfc7484eeb6bccca8d0ac5d3`
- Evaluation budget: 180s per model call; 300s per treatment.
- Scope: fixed-sample external-effect pilot, not a full leaderboard submission.

## Protocol

- **GPQA-Diamond:** deterministic stratified sample across Biology, Chemistry, and Physics; EvalScope-compatible zero-shot prompt; no tools; exact answer parsing.
- **MRCR v2:** official multi-message transcript preserved; 8 needles; official random-prefix gate and Python `difflib.SequenceMatcher` ratio.
- Raw benchmark prompts, expected answers, and complete model outputs remain outside the repository. Committed evidence contains hashes and scores only.
- References: [Fugu technical report](https://arxiv.org/abs/2606.21228), [GPQA repository](https://github.com/idavidrein/gpqa), and [MRCR v2 dataset](https://huggingface.co/datasets/openai/mrcr).

## GPQA-Diamond

| Treatment | n | Completed | Budgeted score | Completed-only | 95% CI | p50 latency | p95 latency | Failures | Token telemetry |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | ---: |
| cindx_auto | 12 | 9/12 | 58.33% | 77.78% | 31.95% to 80.67% | 74.0s | 201.7s | cancelled: 3 | 0/12 |
| cindx_pro | 12 | 5/12 | 41.67% | 100.00% | 19.33% to 68.05% | 300.0s | 300.0s | cancelled: 7 | 8/12 |
| direct_default | 12 | 9/12 | 75.00% | 100.00% | 46.77% to 91.11% | 36.3s | 180.0s | cancelled: 2, other: 1 | 0/12 |

## MRCR v2 (8-needle)

| Treatment | n | Completed | Budgeted score | Completed-only | 95% CI | p50 latency | p95 latency | Failures | Token telemetry |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | ---: |
| cindx_auto_model_route | 4 | 4/4 | 99.98% | 99.98% | n/a | 50.3s | 69.6s | none | 0/4 |
| direct_default | 4 | 4/4 | 99.98% | 99.98% | n/a | 50.7s | 57.3s | none | 0/4 |

## GPQA-Diamond by Domain (Diagnostic)

This breakdown is diagnostic only: each cell contains four frozen questions, so differences are highly uncertain.

| Treatment | Domain | Completed | Correct | Budgeted score | Completed-only |
| --- | --- | ---: | ---: | ---: | ---: |
| cindx_auto | Biology | 3/4 | 2/4 | 50.00% | 66.67% |
| cindx_auto | Chemistry | 2/4 | 1/4 | 25.00% | 50.00% |
| cindx_auto | Physics | 4/4 | 4/4 | 100.00% | 100.00% |
| cindx_pro | Biology | 2/4 | 2/4 | 50.00% | 100.00% |
| cindx_pro | Chemistry | 0/4 | 0/4 | 0.00% | n/a |
| cindx_pro | Physics | 3/4 | 3/4 | 75.00% | 100.00% |
| direct_default | Biology | 4/4 | 4/4 | 100.00% | 100.00% |
| direct_default | Chemistry | 1/4 | 1/4 | 25.00% | 100.00% |
| direct_default | Physics | 4/4 | 4/4 | 100.00% | 100.00% |

## Observed Findings

- Under the fixed budget, the direct GPQA baseline scored **75.00%**, versus **58.33%** for Cindx Auto and **41.67%** for Cindx Pro. This pilot therefore does not demonstrate orchestration uplift.
- Cindx Pro completed **5/12** GPQA cases. Every completed case was correct (**100.00%** completed-only), while p50 and p95 both reached the 300s treatment cap. The dominant observed failure is delivery within budget, not completed-answer accuracy.
- Cindx Auto completed **9/12** GPQA cases and answered **77.78%** of completed cases correctly. Chemistry was the weakest diagnostic slice; the sample is too small for a domain-level conclusion.
- MRCR scored **99.98%** for direct and **99.98%** for Auto across all four frozen length points. Both treatments selected `qwen3.7-max`, so this confirms strong long-context behavior but measures no routing uplift.
- Token telemetry was absent for direct/Auto GPQA and MRCR calls, so this pilot cannot support a reliable cost-efficiency comparison.
- None of the paired GPQA comparisons reached conventional significance; the exact tests are included only to prevent overclaiming from 12 questions.

## GPQA Paired Comparisons (Exploratory)

Each comparison uses the same frozen questions and treats deadline failures as incorrect. Exact McNemar p-values are descriptive only at this sample size.

| Left | Right | n | Left only correct | Right only correct | Both correct | Both incorrect | Exact p |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| direct_default | cindx_auto | 12 | 2 | 0 | 7 | 3 | 0.500 |
| direct_default | cindx_pro | 12 | 4 | 0 | 5 | 3 | 0.125 |
| cindx_auto | cindx_pro | 12 | 2 | 0 | 5 | 5 | 0.500 |

## Interpretation Boundaries

- This pilot estimates behavior on a small, frozen sample. It must not be compared numerically with Fugu-Ultra's full-table scores as if sample size, model access, and serving stack were identical.
- GPQA treatments compare a direct configured model with Cindx Auto routing/workflow and Cindx Pro Conductor execution. Different role models are part of the product treatment and therefore also a confound when attributing uplift solely to orchestration.
- MRCR preserves the official raw multi-message protocol. `cindx_auto_model_route` evaluates model selection only; a Pro workflow was intentionally omitted because flattening or rewriting the transcript would change the benchmark protocol.
- LiveCodeBench and SciCode were not run because this machine did not expose a trusted disposable code-execution container. Untrusted generated code was never executed on the host.
- The frozen Fugu parity matrix remains separate; this pilot does not convert subset results into protocol-equivalent parity cells.

## Decision and Next Gate

- Do not claim Fugu parity or an Auto/Pro quality uplift from this pilot.
- Treat Pro deadline-aware scheduling and fail-soft aggregation as the highest-priority harness issue, then rerun the identical frozen sample before expanding it.
- Instrument complete usage telemetry and inspect Auto's answer aggregation, especially the Chemistry failures, before making cost or router-quality claims.
- Add a trusted disposable code sandbox before enabling LiveCodeBench or SciCode; generated benchmark code must remain off the host system.

## Reproduction

1. Obtain GPQA-Diamond from the pinned public repository revision and MRCR v2 from the official Hugging Face dataset.
2. Run the ignored Rust test `provider_backed_fugu_external_effect_pilot` with the documented dataset environment variables.
3. Run `scripts/analyze-fugu-effect-eval.py` on the private raw result to produce the sanitized JSON and this report.
