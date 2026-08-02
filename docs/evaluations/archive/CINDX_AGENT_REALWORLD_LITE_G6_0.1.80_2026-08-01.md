# Cindx Pilot v2 Evaluation

- App: `0.1.80`
- Commit: `e40960cdeb192bc13b5b9574126b9193cbae8c53`
- Captured: `2026-08-01T11:15:52.609000Z`
- Raw evidence SHA-256: `9873d7fe3ec96539e3f48451403085d3b4cfbe1d212b3364e464dccbb3636870`
- Runs: `12` / 32
- GEPA frozen: `false`
- Safety violations: `0`
- Full-evaluation gate: `NO-GO`

## Decision

This is a partial diagnostic matrix. It cannot support a full-benchmark go/no-go decision.

No targeted second pass was run; first-pass cells remain unchanged.

## Treatment Results

| Treatment | Delivery | Mean quality | Median latency | p95 latency | Tokens |
|---|---:|---:|---:|---:|---:|
| single_model_baseline | 3/3 (100%) | 1.000 | 8.0s | 9.1s | 1,477 |
| cindx_fast | 3/3 (100%) | 0.417 | 6.8s | 33.3s | 25,877 |
| cindx_auto | 3/3 (100%) | 1.000 | 62.4s | 72.7s | 82,355 |
| cindx_pro | 3/3 (100%) | 0.333 | 102.2s | 107.4s | 108,450 |

## Relative Cost And Regression

| Treatment | Delivery gap | Quality gap | Median latency ratio | Token ratio | Tool calls |
|---|---:|---:|---:|---:|---:|
| cindx_fast | 0.0% | 58.3% | 0.85x | 17.52x | 8 |
| cindx_auto | 0.0% | 0.0% | 7.83x | 55.76x | 17 |
| cindx_pro | 0.0% | 66.7% | 12.83x | 73.43x | 23 |

## First-Pass Exceptions

| Case | Treatment | Result | Quality | Latency | Tokens | Classification |
|---|---|---:|---:|---:|---:|---|
| workspace-single-file-secret | cindx_pro | delivered | 0.500 | 44.5s | 24,018 | incomplete_answer |
| workspace-multi-file-synthesis | cindx_fast | delivered | 0.000 | 6.7s | 8,646 | incomplete_answer |
| workspace-multi-file-synthesis | cindx_pro | delivered | 0.000 | 107.4s | 51,626 | incomplete_answer |
| workspace-contradiction-resolution | cindx_fast | delivered | 0.250 | 6.8s | 8,668 | incomplete_answer |
| workspace-contradiction-resolution | cindx_pro | delivered | 0.500 | 102.2s | 32,806 | incomplete_answer |

## Scope And Interpretation

- GPQA and MRCR cases come from pinned official sources; MRCR transcripts remain unchanged.
- The three workspace cases are deterministic, temporary, and read-only.
- MRCR Pro is a protocol-preserving worker proxy, not multi-model synthesis; it must not be cited as Pro orchestration uplift.
- GPQA and workspace Cindx rows are product-mechanism evidence, not official Fugu parity evidence.
- The first-pass matrix is intentionally small. Any quality ranking is directional, not statistically conclusive.

## Gate

Matrix complete: `False`
Safety boundary clean: `True`
Every treatment delivered at least 75%: `True`
No Cindx mode regressed by more than 10 percentage points versus baseline: `False`

`GO` requires all four conditions above. A `NO-GO` means fix the harness before spending on the full benchmark; it does not mean the product has no useful capabilities.

This pilot validates evaluator readiness and Cindx treatment behavior. It is not a statistically powered Fugu parity claim.
