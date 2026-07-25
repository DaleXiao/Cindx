# Cindx Agent Core V3 GPQA Matched Pilot 0.1.43

- Generated: `1785012030333`
- Source commit recorded in raw evidence: `unknown`
- Evaluated checkout: `e9beb4af1de9435822f017a45f86917ce32df2dc` (bound to the unchanged raw SHA by the committed provenance attestation)
- App crate version: `0.1.43`
- Raw evidence SHA-256: `4cb2b24ab6bc4a8048d2209122914b276ae8c0e16b6805b0c957b3d6fdef264c`
- Evaluation budget: 180s per model call; 300s per treatment.
- Scope: fixed-sample external-effect pilot, not a full leaderboard submission.

## Protocol

- **GPQA-Diamond:** deterministic stratified sample across Biology, Chemistry, and Physics; EvalScope-compatible zero-shot prompt; no tools; exact answer parsing.
- Raw benchmark prompts, expected answers, and complete model outputs remain outside the repository. Committed evidence contains hashes and scores only.
- References: [Fugu technical report](https://arxiv.org/abs/2606.21228), [GPQA repository](https://github.com/idavidrein/gpqa).

## GPQA-Diamond

| Treatment | n | Completed | Budgeted score | Completed-only | 95% CI | p50 latency | p95 latency | Failures | Token telemetry |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | ---: |
| cindx_auto | 12 | 9/12 | 66.67% | 88.89% | 39.06% to 86.19% | 44.8s | 150.0s | cancelled: 2, other: 1 | 9/12 |
| cindx_pro | 12 | 9/12 | 66.67% | 88.89% | 39.06% to 86.19% | 212.3s | 300.0s | other: 3 | 12/12 |
| direct_default | 12 | 9/12 | 75.00% | 100.00% | 46.77% to 91.11% | 28.2s | 180.0s | cancelled: 3 | 9/12 |

## GPQA-Diamond by Domain (Diagnostic)

This breakdown is diagnostic only: each cell contains 4 frozen questions, so differences are highly uncertain.

| Treatment | Domain | Completed | Correct | Budgeted score | Completed-only |
| --- | --- | ---: | ---: | ---: | ---: |
| cindx_auto | Biology | 3/4 | 2/4 | 50.00% | 66.67% |
| cindx_auto | Chemistry | 2/4 | 2/4 | 50.00% | 100.00% |
| cindx_auto | Physics | 4/4 | 4/4 | 100.00% | 100.00% |
| cindx_pro | Biology | 4/4 | 3/4 | 75.00% | 75.00% |
| cindx_pro | Chemistry | 1/4 | 1/4 | 25.00% | 100.00% |
| cindx_pro | Physics | 4/4 | 4/4 | 100.00% | 100.00% |
| direct_default | Biology | 4/4 | 4/4 | 100.00% | 100.00% |
| direct_default | Chemistry | 1/4 | 1/4 | 25.00% | 100.00% |
| direct_default | Physics | 4/4 | 4/4 | 100.00% | 100.00% |

## Observed Findings

- Under the fixed budget, the direct GPQA baseline scored **75.00%**, versus **66.67%** for Cindx Auto and **66.67%** for Cindx Pro. This pilot therefore does not demonstrate orchestration uplift.
- Cindx Pro completed **9/12** GPQA cases and answered **88.89%** of completed cases correctly; p50 latency was 212.3s and p95 was 300.0s, reaching the 300s treatment cap.
- Cindx Auto completed **9/12** GPQA cases and answered **88.89%** of completed cases correctly. Chemistry was the weakest diagnostic slice; the sample is too small for a domain-level conclusion.
- Token telemetry was recorded for **30/36** GPQA runs. Runs without final usage make treatment token totals lower bounds, so incomplete telemetry cannot support a complete cost-efficiency comparison.
- Paired GPQA significance is reported with exact McNemar tests to prevent overclaiming from 12 questions.

## GPQA Paired Comparisons (Exploratory)

Each comparison uses the same frozen questions and treats deadline failures as incorrect. Exact McNemar p-values are descriptive only at this sample size.

| Left | Right | n | Left only correct | Right only correct | Both correct | Both incorrect | Exact p |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| direct_default | cindx_auto | 12 | 3 | 2 | 6 | 1 | 1.000 |
| direct_default | cindx_pro | 12 | 2 | 1 | 7 | 2 | 1.000 |
| cindx_auto | cindx_pro | 12 | 1 | 1 | 7 | 3 | 1.000 |

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

## Matched Historical Comparison

The historical control is the same frozen 12-question sample from app `0.1.26` at commit `c628c3f`. This preserves question and option-order hashes, but it is not a randomized concurrent experiment: provider serving conditions may have changed between dates.

| Treatment | 0.1.26 score | 0.1.43 score | Delta | Completed | p50 latency | Cross-run discordance | Exact p |
| --- | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| direct_default | 75.00% | 75.00% | 0.00 pp | 9 -> 9 | 33.1s -> 28.2s | old-only 1, new-only 1 | 1.000 |
| cindx_auto | 58.33% | 66.67% | +8.34 pp | 7 -> 9 | 100.0s -> 44.8s | old-only 0, new-only 1 | 1.000 |
| cindx_pro | 33.33% | 66.67% | +33.34 pp | 4 -> 9 | 300.0s -> 212.3s | old-only 0, new-only 4 | 0.125 |

The Pro improvement is operationally meaningful but not statistically significant at `n=12`. The direct control kept the same aggregate score while one success changed in each direction, which demonstrates run-to-run provider variance. Cross-version deltas therefore cannot be attributed solely to the Agent Core V3 changes.

## Failure Review

1. **No measured orchestration uplift.** Direct scored `9/12`; Auto and Pro each scored `8/12`. Exact paired tests are all `p=1.000`, and the Wilson intervals overlap widely.
2. **Pro remains too expensive for this task shape.** It matched Auto's score while taking `212.3s` median versus Auto's `44.8s` and direct's `28.2s`. Its p95 still reached the `300s` cap.
3. **Task-graph terminal delivery is incomplete in the evaluation path.** Three Pro cases ended at the deadline with unresolved dependency pairs instead of synthesizing from completed partial evidence.
4. **Auto cancellation is not fail-soft enough.** Two Auto cases reported `worker cancelled after quorum` without delivering an answer; quorum cancellation should retain and return the best completed candidate.
5. **Collaboration can correlate on a wrong answer.** On one Biology case, direct was correct in `20.6s`, while both Auto and Pro completed with the same wrong option; Pro spent `212.3s`. More roles did not create independent evidence on that case.
6. **Orchestration does recover some direct failures.** Auto answered two Chemistry cases that direct missed at its deadline; Pro recovered one. Those gains were offset by failures or a wrong answer on direct-correct cases.
7. **Usage evidence is incomplete.** Token telemetry exists for `30/36` runs, so token totals cannot support a cost-efficiency claim.

## Proposed Discussion

The next implementation should be decided from these measured failure modes:

1. Route self-contained, no-tool questions through direct first; escalate only when calibrated uncertainty or disagreement predicts benefit.
2. Reserve enough deadline for a mandatory terminal synthesizer that accepts partial task-graph outputs instead of returning unresolved dependencies.
3. Make quorum cancellation return the best completed candidate atomically.
4. Add an independent disagreement/tie-break rule so collaboration cannot silently replace a fast correct answer with correlated consensus.
5. Repeat the frozen sample across multiple time windows, then run the full GPQA-Diamond set before making a product quality claim.
6. Require complete usage telemetry and source provenance for every provider-backed evaluation.

No Agent Core behavior was changed after observing these results. The only post-evaluation code changes harden provenance and make the report generator accurately support GPQA-only evidence.

## Reproduction

1. Obtain GPQA-Diamond from the pinned public repository revision.
2. Run the ignored Rust test `provider_backed_fugu_external_effect_pilot` with the documented dataset environment variables.
3. Run `scripts/analyze-fugu-effect-eval.py` on the private raw result to produce the sanitized JSON and this report.
