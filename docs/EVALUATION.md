# Evaluation and Claim Boundary

This is the maintained decision ledger. Detailed historical reports were
removed from the current tree because they duplicated status, included
superseded protocols, and were repeatedly read as current product facts. Their
exact contents remain available at Git commit `f2ce3b7` and earlier revisions.

## Evidence Levels

1. **Contract tests** prove schemas, invariants, replay, and fail-closed behavior.
2. **Deterministic suites** prove frozen cases and measurement plumbing without
   contacting a provider.
3. **Provider diagnostics** measure model behavior on a narrow frozen task set.
4. **Product evaluation** requires complete end-to-end runs, external effects,
   receipts, retained failures, and an explicit matched decision.
5. **Frontier/parity claims** require independent external comparison under a
   frozen, position-balanced, resource-accounted protocol.

Passing a lower level does not imply a higher-level result.

## Current Decision Ledger

| Evidence | Revision/version | Result and admitted claim |
| --- | --- | --- |
| Agent Real-World V5 | `a0000fa`, `0.2.22` | `VALID_BASELINE`. In the iso-budget adaptive-direct subset Auto gained one matched quality pass, preserved completion, reduced median paired latency by `3,585 ms`, and used about `10.0%` fewer total tokens. Workflow, learned profile, and distillation were not exercised; Pro is descriptive. |
| Memory-effect V2 | `cd64703`, `0.2.19` | `IMPROVED` for the frozen direct memory-on/off harness: all `9/9` pairs were evaluable, all `6/6` required-memory pairs improved, and all three decoy controls passed in both arms. This is not a general Agent-quality claim. |
| Dynamic Collaboration V1 | `bafbbd5`, `0.2.27` | `VALID_TARGETED_EVIDENCE`, `NO_GO_NOT_EXERCISED`. Every Auto/Pro run remained Direct. Auto used `1.6028x` median latency and `1.1208x` tokens versus Grounded Direct; Pro produced no matched quality or completion win. |
| Conductor ownership | `ae6dfcb`, `0.2.27` | `VALID_TARGETED_EVIDENCE`, `KEEP_HARNESS_FIX`. Complete batch readback closed the observed terminal-verification defect. Every classifiable route remained Direct, so no workflow or learning uplift was shown. |
| Direct-finalizer calibration | `2647daa`, `0.2.23` | `VALID_TARGETED_EVIDENCE`, `NO_GO_FOR_PROMOTION`. The candidate regressed a preservation case; GEPA, holdout, snapshot, deployment, and transfer did not run. |
| Workflow GEPA V5 | `86f7dd6`, `0.2.25` | `VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`. The learned profile changed training route and exercised Workflow, but unseen validation produced zero wins and two ties at `1.4337x` latency and `1.2966x` tokens. Nothing was promoted. |
| Workflow GEPA V7 | `bc37fa9`, `0.2.26` | `VALID_TARGETED_EVIDENCE`, `NO_GO_TRAINING`. Three candidates completed six training pairs with full task quality, but all pairs tied, all routes remained Direct, and no candidate passed the Pareto resource gate. |
| Workflow GEPA V12 | `ff8c238`, recorded in `0.2.30` | `INVALID_EVIDENCE`. The Direct arm retained route evidence but failed terminal completion; the Workflow arm stopped before strategy-event persistence. There is no matched pair, GO/NO-GO, candidate, snapshot, promotion, or capability conclusion. |
| GPQA matched diagnostic | `0.1.78` | On 12 frozen GPQA-Diamond questions, Direct scored `10/12`; Auto and Pro each scored `8/12`. This predates current code and is a reasoning diagnostic, not a current product baseline. |

Earlier Real-World versions, memory V1, GPQA/Core pilots, Fugu pilots, and
Workflow GEPA V2-V4/V9-V11 remain historical diagnostic material only. Their
failures are not silently converted into positive evidence.

## Current Product Gate

The evidence currently supports these statements:

- The direct/adaptive-direct path can preserve or narrowly improve the frozen
  V5 subset under its recorded revision.
- Durable memory helped the frozen matched direct tasks under its recorded
  revision and did not fail the decoy controls.
- Permission, run identity, recovery, task graph, context, memory, prompt
  serving, and evaluation boundaries have deterministic contract coverage.
- Current deterministic contracts require one typed strategy receipt before
  treatment execution and a matching terminal receipt before route evidence is
  accepted.
- The promotion system has rejected candidates that fail preservation,
  validation, lineage, or resource gates.

It does **not** support these statements:

- Auto or Pro generally outperform Fast.
- Multi-model Workflow currently improves product quality.
- GEPA has produced and deployed a better production profile.
- Auto-to-Pro transfer or distillation improves current Pro behavior.
- Cindx matches or approaches Fugu Ultra on an independent external benchmark.
- Deterministic green tests prove frontier intelligence.

## Fugu Comparison

The versioned contract is `benchmarks/fugu/fugu-v1.json`, with portable schemas
in `crates/orchestrator-eval`. It measures routing, diversity, evidence quality,
external effects, verification, efficiency, and failure handling. It is a
comparison harness, not parity evidence.

A parity claim requires independent tasks and evaluators, a frozen direct
control, equivalent provider and tool access, position balancing, complete
resource accounting, retained failures, safety qualification, and confidence
intervals. No current retained run satisfies that bar.

## GEPA and Self-Improvement

The product contains causal assignment, train/holdout gates, stable/canary
serving, rollback, transfer, and distillation contracts. Mechanism coverage is
not outcome evidence. A candidate may be admitted only when:

- the exact parent and treatment are frozen;
- candidate assignment and delivery receipts are complete;
- quality does not regress on preservation, train, or holdout tasks;
- safety, failure, and resource gates pass;
- the serving snapshot is bound to the same lineage;
- an independent test remains sealed until final admission.

No recent workflow candidate passed those gates. Production seed behavior was
not changed by V5, V7, or V12.

## Required Next Evidence

Do not rerun Workflow GEPA V12. The current source now links atomic strategy
selection and terminal transitions in one observable lifecycle for both
treatment arms, and the evaluation projector rejects missing, duplicate, or
mismatched lifecycle receipts. This is deterministic
instrumentation evidence only; it does not repair the frozen V12 attempt.

Complete the separately reviewed production-graph and learning-policy goals
before requesting the final narrow provider gate. Only then freeze a successor
protocol that:

1. uses one shared Conductor plan and varies only the execution treatment;
2. proves both arms persist strategy and terminal receipts;
3. runs a small matched pair before any candidate search;
4. stops immediately if instrumentation is incomplete;
5. opens candidate generation and holdout only after the causal pair is valid.

This is the shortest path to learning whether Workflow helps. Additional prompt
genes or evaluator variants before that boundary would add complexity without
answering the causal question.

## Running Evaluation

Deterministic contracts are included in the profiles documented in
[DEVELOPMENT.md](DEVELOPMENT.md). Frozen inputs live under:

```text
benchmarks/agent/
benchmarks/fugu/
benchmarks/system/
```

Provider binaries are feature-gated in the desktop crate. They must not be run
without explicit authorization, a clean pinned revision, a preflight-only pass,
and a bounded output directory outside the tracked tree. Sanitized summaries
must record version, commit, provider, cases, treatment, budgets, failures,
receipts, evidence digest, confounds, and decision.

Historical report retrieval example:

```sh
git show f2ce3b7:docs/evaluations/README.md
```
