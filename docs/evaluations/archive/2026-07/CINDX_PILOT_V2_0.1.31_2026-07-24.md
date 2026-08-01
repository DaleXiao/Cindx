# Cindx Pilot v2 Evaluation

- App: `0.1.31`
- Commit: `51367dc`
- Runs: `32` / 32
- GEPA frozen: `true`
- Safety violations: `0`
- Full-evaluation gate: `NO-GO`

## Decision

Hold the full benchmark. The pilot found reproducible harness regressions that should be corrected first.

The targeted second pass confirmed `2` persistent anomalies and recovered `2` first-pass anomalies.

## Treatment Results

| Treatment | Delivery | Mean quality | Median latency | p95 latency | Tokens |
|---|---:|---:|---:|---:|---:|
| single_model_baseline | 8/8 (100%) | 1.000 | 24.9s | 80.8s | 52,815 |
| cindx_fast | 8/8 (100%) | 0.894 | 26.4s | 83.1s | 83,565 |
| cindx_auto | 8/8 (100%) | 1.000 | 50.9s | 135.6s | 142,350 |
| cindx_pro | 7/8 (88%) | 0.763 | 85.6s | 300.0s | 244,823 |

## Relative Cost And Regression

| Treatment | Delivery gap | Quality gap | Median latency ratio | Token ratio | Tool calls |
|---|---:|---:|---:|---:|---:|
| cindx_fast | 0.0% | 10.6% | 1.06x | 1.58x | 9 |
| cindx_auto | 0.0% | 0.0% | 2.04x | 2.69x | 23 |
| cindx_pro | 12.5% | 23.7% | 3.44x | 4.63x | 38 |

## First-Pass Exceptions

| Case | Treatment | Result | Quality | Latency | Tokens | Classification |
|---|---|---:|---:|---:|---:|---|
| reckEnrOPFT9Ru7tW | cindx_pro | failed | 0.000 | 300.0s | 30,236 | deadline_or_cancellation |
| workspace-multi-file-synthesis | cindx_fast | delivered | 0.400 | 19.9s | 8,291 | incomplete_answer |
| workspace-contradiction-resolution | cindx_fast | delivered | 0.750 | 29.1s | 9,029 | incomplete_answer |
| row-2000 | cindx_pro | delivered | 0.102 | 11.9s | 8,482 | incomplete_answer |

## Targeted Second Pass

Second-pass cells are diagnostic only and do not overwrite the first-pass matrix.

| Case | Treatment | Result | Quality | Latency | Tokens | Classification |
|---|---|---:|---:|---:|---:|---|
| reckEnrOPFT9Ru7tW | cindx_pro | failed | 0.000 | 300.0s | 30,238 | deadline_or_cancellation |
| workspace-multi-file-synthesis | cindx_fast | delivered | 0.400 | 16.0s | 8,361 | incomplete_answer |
| workspace-contradiction-resolution | cindx_fast | delivered | 1.000 | 17.4s | 8,689 | none |
| row-2000 | cindx_pro | delivered | 1.000 | 18.2s | 8,674 | none |

Persistent anomalies: `2`
Recovered on second pass: `2`

## Root-Cause Signals

- Persistent: `reckEnrOPFT9Ru7tW` / `cindx_pro` moved from `deadline_or_cancellation` at quality `0.000` to `deadline_or_cancellation` at quality `0.000` on the second pass.
- Persistent: `workspace-multi-file-synthesis` / `cindx_fast` moved from `incomplete_answer` at quality `0.400` to `incomplete_answer` at quality `0.400` on the second pass.
- Recovered: `workspace-contradiction-resolution` / `cindx_fast` moved from `incomplete_answer` at quality `0.750` to `none` at quality `1.000` on the second pass.
- Recovered: `row-2000` / `cindx_pro` moved from `incomplete_answer` at quality `0.102` to `none` at quality `1.000` on the second pass.

## Scope And Interpretation

- GPQA and MRCR cases come from pinned official sources; MRCR transcripts remain unchanged.
- The three workspace cases are deterministic, temporary, and read-only.
- MRCR Pro is a protocol-preserving worker proxy, not multi-model synthesis; it must not be cited as Pro orchestration uplift.
- GPQA and workspace Cindx rows are product-mechanism evidence, not official Fugu parity evidence.
- The first-pass matrix is intentionally small. Any quality ranking is directional, not statistically conclusive.

## Gate

Matrix complete: `True`
Safety boundary clean: `True`
Every treatment delivered at least 75%: `True`
No Cindx mode regressed by more than 10 percentage points versus baseline: `False`

`GO` requires all four conditions above. A `NO-GO` means fix the harness before spending on the full benchmark; it does not mean the product has no useful capabilities.

This pilot validates evaluator readiness and Cindx treatment behavior. It is not a statistically powered Fugu parity claim.
