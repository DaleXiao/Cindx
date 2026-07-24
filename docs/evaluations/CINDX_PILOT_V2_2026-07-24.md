# Cindx Pilot v2 Evaluation

- App: `0.1.30`
- Commit: `6967aa6`
- Runs: `32` / 32
- GEPA frozen: `true`
- Safety violations: `0`
- Full-evaluation gate: `NO-GO`

## Decision

Hold the full benchmark. The pilot found reproducible harness regressions that should be corrected first.

The targeted second pass confirmed `3` persistent anomalies and recovered `2` first-pass anomalies.

## Treatment Results

| Treatment | Delivery | Mean quality | Median latency | p95 latency | Tokens |
|---|---:|---:|---:|---:|---:|
| single_model_baseline | 8/8 (100%) | 1.000 | 21.3s | 92.4s | 54,599 |
| cindx_fast | 8/8 (100%) | 0.894 | 23.2s | 144.6s | 84,296 |
| cindx_auto | 7/8 (88%) | 0.875 | 46.5s | 180.0s | 133,859 |
| cindx_pro | 6/8 (75%) | 0.750 | 74.4s | 300.0s | 240,032 |

## Relative Cost And Regression

| Treatment | Delivery gap | Quality gap | Median latency ratio | Token ratio | Tool calls |
|---|---:|---:|---:|---:|---:|
| cindx_fast | 0.0% | 10.6% | 1.09x | 1.54x | 8 |
| cindx_auto | 12.5% | 12.5% | 2.18x | 2.45x | 24 |
| cindx_pro | 25.0% | 25.0% | 3.49x | 4.40x | 40 |

## First-Pass Exceptions

| Case | Treatment | Result | Quality | Latency | Tokens | Classification |
|---|---|---:|---:|---:|---:|---|
| reckEnrOPFT9Ru7tW | cindx_auto | failed | 0.000 | 180.0s | 0 | deadline_or_cancellation |
| reckEnrOPFT9Ru7tW | cindx_pro | failed | 0.000 | 300.0s | 34,296 | deadline_or_cancellation |
| workspace-multi-file-synthesis | cindx_fast | delivered | 0.400 | 24.1s | 7,996 | incomplete_answer |
| workspace-contradiction-resolution | cindx_fast | delivered | 0.750 | 18.6s | 8,452 | incomplete_answer |
| row-1701 | cindx_pro | failed | 0.000 | 67.9s | 34,383 | empty_output |

## Targeted Second Pass

Second-pass cells are diagnostic only and do not overwrite the first-pass matrix.

| Case | Treatment | Result | Quality | Latency | Tokens | Classification |
|---|---|---:|---:|---:|---:|---|
| reckEnrOPFT9Ru7tW | cindx_auto | delivered | 1.000 | 152.0s | 15,800 | none |
| reckEnrOPFT9Ru7tW | cindx_pro | failed | 0.000 | 300.0s | 18,027 | deadline_or_cancellation |
| workspace-multi-file-synthesis | cindx_fast | delivered | 0.400 | 14.9s | 8,212 | incomplete_answer |
| workspace-contradiction-resolution | cindx_fast | delivered | 1.000 | 13.1s | 8,607 | none |
| row-1701 | cindx_pro | failed | 0.000 | 85.1s | 34,383 | empty_output |

Persistent anomalies: `3`
Recovered on second pass: `2`

## Root-Cause Signals

- Auto Chemistry recovered on the second pass, indicating deadline-sensitive variance rather than a deterministic wrong answer.
- Pro Chemistry exhausted the 300-second treatment deadline twice, confirming a persistent orchestration critical-path problem.
- Fast multi-file synthesis repeatedly spent its evidence turns listing directories and never read the decisive files.
- Pro MRCR at 141k characters returned the same token usage twice with an empty visible answer and no provider error; empty-success validation and long-context model routing are insufficient.
- The contradiction case recovered on the second pass; scoring is case-insensitive so capitalization alone does not count as failure.

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
