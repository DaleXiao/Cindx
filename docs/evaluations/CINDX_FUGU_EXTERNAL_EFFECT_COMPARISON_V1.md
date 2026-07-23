# Cindx External Effect Before/After Comparison V1

## Decision Summary

The rerun does **not** show a broad Fast/Auto/Pro improvement on the frozen external-effect pilot.

| Product effort | Comparable treatment | Result | Evidence |
| --- | --- | --- | --- |
| Fast | `direct_default` / Single default-model path | Essentially unchanged; slightly faster median | GPQA stayed at 9/12 delivered and correct. p50 improved from 36.3s to 33.1s; p95 remained at 180.0s. |
| Auto | `cindx_auto` | Mixed; no net quality improvement and worse delivery | GPQA budgeted score stayed at 7/12, but completion fell from 9/12 to 7/12 and p50 rose from 74.0s to 100.0s. Completed-only accuracy rose from 77.8% to 100%. MRCR latency improved. |
| Pro | `cindx_pro` | Regressed on this run | GPQA delivered/correct fell from 5/12 to 4/12. p50 and p95 remained at the 300s cap. |

Fast is represented by the prior and current direct default-model treatment because product Fast requests the Single policy and selects the configured default model. The earlier report did not label this treatment `Fast`, so this is a behaviorally equivalent calibration rather than a renamed historical measurement.

## Reproducibility

| Field | Previous | Current |
| --- | --- | --- |
| App version | 0.1.25 | 0.1.26 |
| Source commit | `cf7850510630b924b92043c30071f11fa1083bca` | `c628c3fe4dc0435b2d64b7c91d383164a5a577ed` |
| GPQA sample | 12 frozen questions | Same 12 questions |
| MRCR sample | 4 frozen 8-needle cases | Same 4 cases |
| Model-call / treatment budget | 180s / 300s | 180s / 300s |
| Model roles | qwen3.7-max, qwen3.7-plus, glm-5.2, deepseek-v4-pro, deepseek-v4-flash | Same |

All 44 paired run inputs and expected-answer hashes match exactly across evaluations: 36 GPQA treatment runs and 8 MRCR runs. Raw prompts and outputs remain outside the repository.

## GPQA-Diamond

Deadline failures count as zero because a correct answer that is not delivered within the fixed product budget is not a successful task.

| Effort | Metric | Previous | Current | Delta |
| --- | --- | ---: | ---: | ---: |
| Fast | Completed | 9/12 | 9/12 | 0 |
| Fast | Budgeted score | 75.00% | 75.00% | 0.00 pp |
| Fast | Completed-only accuracy | 100.00% | 100.00% | 0.00 pp |
| Fast | p50 latency | 36.3s | 33.1s | -3.2s |
| Fast | p95 latency | 180.0s | 180.0s | unchanged |
| Auto | Completed | 9/12 | 7/12 | -2 |
| Auto | Budgeted score | 58.33% | 58.33% | 0.00 pp |
| Auto | Completed-only accuracy | 77.78% | 100.00% | +22.22 pp |
| Auto | p50 latency | 74.0s | 100.0s | +26.0s |
| Auto | p95 latency | 201.7s | 203.6s | +1.9s |
| Pro | Completed | 5/12 | 4/12 | -1 |
| Pro | Budgeted score | 41.67% | 33.33% | -8.34 pp |
| Pro | Completed-only accuracy | 100.00% | 100.00% | 0.00 pp |
| Pro | p50 latency | 300.0s | 300.0s | unchanged |
| Pro | p95 latency | 300.0s | 300.0s | unchanged |

The paired outcome sets reinforce the aggregate result:

- Fast: 9 questions correct in both runs; 3 failed in both runs.
- Auto: 6 correct in both, 1 previous-only correct, 1 current-only correct, and 4 incorrect or undelivered in both. Net budgeted score is unchanged.
- Pro: 4 correct in both, 1 previous-only correct, no current-only correct, and 7 incorrect or undelivered in both.

The sample is too small for a stable leaderboard claim. None of these before/after differences provides conventional statistical evidence of improvement.

## MRCR v2 Long Context

| Treatment | Metric | Previous | Current | Delta |
| --- | --- | ---: | ---: | ---: |
| Direct / Fast-equivalent | Completed | 4/4 | 4/4 | 0 |
| Direct / Fast-equivalent | Score | 99.98% | 99.98% | unchanged |
| Direct / Fast-equivalent | p50 latency | 50.7s | 47.3s | -3.4s |
| Direct / Fast-equivalent | p95 latency | 57.3s | 96.4s | +39.1s |
| Auto model route | Completed | 4/4 | 4/4 | 0 |
| Auto model route | Score | 99.98% | 99.98% | unchanged |
| Auto model route | p50 latency | 50.3s | 41.0s | -9.3s |
| Auto model route | p95 latency | 69.6s | 56.2s | -13.4s |

MRCR confirms that the raw long-context path remains accurate. Auto's model-route latency improved on all four frozen length points, but both treatments selected `qwen3.7-max`; this is not evidence of multi-model quality uplift.

## Cost Telemetry

Current GPQA observed token totals are 19,161 for Fast-equivalent, 84,773 for Auto, and 256,062 for Pro. These are lower bounds because some cancelled runs lack final usage. The previous run also lacked enough usage telemetry for a valid before/after cost comparison.

## Interpretation

1. Fast remains the most reliable effort on this pilot. Its median latency improved modestly, but quality and deadline completion did not change.
2. Auto improved the correctness of answers it managed to deliver and improved MRCR route latency, but lost two GPQA deliveries. It is mixed, not a net improvement.
3. Pro still has a deadline-aware execution problem. Multi-model work consumes substantially more observed tokens, reaches the 300s cap, and delivered one fewer correct answer than before.
4. The highest-leverage harness fix remains fail-soft aggregation: preserve the best already-valid candidate and return it before the deadline when later planner/reviewer/arbiter stages stall.
5. A second repeated run is needed to separate provider variance from a persistent regression. Do not claim Fugu-Ultra parity or orchestration uplift from this sample.

## Evidence

- Previous sanitized result: `docs/evaluations/CINDX_FUGU_EXTERNAL_EFFECT_PILOT_V1.json`
- Current sanitized result: `docs/evaluations/CINDX_FUGU_EXTERNAL_EFFECT_RERUN_V2.json`
- Current standalone report: `docs/evaluations/CINDX_FUGU_EXTERNAL_EFFECT_RERUN_V2.md`
- Current raw evidence SHA-256: `63332784be1ef4ce991e4696aea8b972d98ee446391724bace69db4f876271f4`
