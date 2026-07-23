# Cindx Fugu External Effect Upgrade V3

- Generated: `1784816688373`
- Source commit: `unknown`
- App crate version: `0.1.26`
- Raw evidence SHA-256: `8f31e0171073d69528dde53322033d40e0ebed0362b3f6bc84199a444335858e`
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
| cindx_auto | 3 | 2/3 | 66.67% | 100.00% | 20.77% to 93.85% | 35.5s | 180.0s | cancelled: 1 | 2/3 |
| cindx_pro | 3 | 2/3 | 66.67% | 100.00% | 20.77% to 93.85% | 67.4s | 300.0s | cancelled: 1 | 3/3 |
| direct_default | 3 | 3/3 | 100.00% | 100.00% | 43.85% to 100.00% | 28.7s | 35.3s | none | 3/3 |

## MRCR v2 (8-needle)

| Treatment | n | Completed | Budgeted score | Completed-only | 95% CI | p50 latency | p95 latency | Failures | Token telemetry |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | ---: |
| cindx_auto_model_route | 4 | 4/4 | 99.98% | 99.98% | n/a | 48.1s | 78.8s | none | 4/4 |
| direct_default | 4 | 4/4 | 99.98% | 99.98% | n/a | 56.9s | 59.0s | none | 4/4 |

## GPQA-Diamond by Domain (Diagnostic)

This breakdown is diagnostic only: each cell contains 1 frozen question, so differences are highly uncertain.

| Treatment | Domain | Completed | Correct | Budgeted score | Completed-only |
| --- | --- | ---: | ---: | ---: | ---: |
| cindx_auto | Biology | 1/1 | 1/1 | 100.00% | 100.00% |
| cindx_auto | Chemistry | 0/1 | 0/1 | 0.00% | n/a |
| cindx_auto | Physics | 1/1 | 1/1 | 100.00% | 100.00% |
| cindx_pro | Biology | 1/1 | 1/1 | 100.00% | 100.00% |
| cindx_pro | Chemistry | 0/1 | 0/1 | 0.00% | n/a |
| cindx_pro | Physics | 1/1 | 1/1 | 100.00% | 100.00% |
| direct_default | Biology | 1/1 | 1/1 | 100.00% | 100.00% |
| direct_default | Chemistry | 1/1 | 1/1 | 100.00% | 100.00% |
| direct_default | Physics | 1/1 | 1/1 | 100.00% | 100.00% |

## Observed Findings

- Under the fixed budget, the direct GPQA baseline scored **100.00%**, versus **66.67%** for Cindx Auto and **66.67%** for Cindx Pro. This pilot therefore does not demonstrate orchestration uplift.
- Cindx Pro completed **2/3** GPQA cases. Every completed case was correct (**100.00%** completed-only); p50 latency was 67.4s and p95 was 300.0s, reaching the 300s treatment cap. The dominant observed failure is delivery within budget, not completed-answer accuracy.
- Cindx Auto completed **2/3** GPQA cases and answered **100.00%** of completed cases correctly. Chemistry was the weakest diagnostic slice; the sample is too small for a domain-level conclusion.
- MRCR scored **99.98%** for direct and **99.98%** for Auto across all 4 frozen length points. Both treatments selected `qwen3.7-max`, so this confirms strong long-context behavior but measures no routing uplift.
- Token telemetry was recorded for **8/9** GPQA runs and **8/8** MRCR runs. Runs without final usage make treatment token totals lower bounds, so this pilot cannot support a complete cost-efficiency comparison.
- None of the paired GPQA comparisons reached conventional significance; the exact tests are included only to prevent overclaiming from 3 questions.

## GPQA Paired Comparisons (Exploratory)

Each comparison uses the same frozen questions and treats deadline failures as incorrect. Exact McNemar p-values are descriptive only at this sample size.

| Left | Right | n | Left only correct | Right only correct | Both correct | Both incorrect | Exact p |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| direct_default | cindx_auto | 3 | 1 | 0 | 2 | 0 | 1.000 |
| direct_default | cindx_pro | 3 | 1 | 0 | 2 | 0 | 1.000 |
| cindx_auto | cindx_pro | 3 | 0 | 0 | 2 | 1 | 1.000 |

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
