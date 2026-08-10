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

The production graph is now deterministically constrained to one Specialist,
an optional planned Independent Verifier, and a non-model Owner handoff. Matched
evaluation projection rejects a Direct arm with workflow-worker exposure, a
Workflow arm without its planned treatment, or any reintroduced anchor/reviewer
competition. This is structure and observability evidence, not quality uplift.

Goal 3A adds a provider-free shadow outcome contract shared by Direct and
Workflow. It derives bounded reward only from externally verified behavior
after lifecycle, actual treatment exposure, preservation, safety, and resource
receipts validate. Valid failures remain zero-score evidence; missing or
tampered provenance is censored. The receipt does not write production learning
evidence or authorize routing, prompt, memory, canary, or promotion changes.
This closes a reward-plumbing prerequisite only; no provider run or matched
uplift result was produced.

The Goal 3B/3C contracts add provider-free structured policy, attribution,
aggregation, offline admission, canonical replay, and crash-safe journal
boundaries. Under the successor-only `realworld-eval` path, policy assignment
is joined to the actual pre-dispatch request payload identity and size, Specialist/Verifier
attempts, derived stopping, and the Goal 3A outcome receipt. Candidate lineage
changes one bounded axis; valid failures remain in the denominator, while
incomplete attribution is retained as a censor instead of being silently
dropped. The external journal stores private immutable records, rejects
missing/tampered/forked chains, and recovers pending capture as
`IncompleteInstrumentation` without rerunning a provider. Train and sealed
holdout evidence can reach review readiness, but only an independently bound
review can authorize an offline narrow-validation record. Nothing is served or
promoted.

All Goal 3C evidence is deterministic and provider-free. V12 remains invalid
and was not rerun; no collaboration uplift or provider-cost result was
produced.

Goal 3D now freezes one successor in
`benchmarks/agent/collaboration-successor-protocol-v1.json`, bound by SHA-256 to
the new `collaboration-successor-v1.json` suite. The fixed matrix is three
matched pairs / six runs in this order:

1. a Direct-first training pair against the 5,000-bps, fail-fast Workflow
   baseline;
2. a Workflow-first training pair for the only candidate, which keeps the same
   topology, verification, and repair policy and changes only context to
   7,500 bps;
3. a Direct-first, training-ineligible holdout pair for that candidate.

The manifest permits one candidate and requires a valid positive baseline
before its pair, then a passing candidate training result before holdout. It
reuses the existing conservative Workflow evaluation run budget: at most ten
minutes, 20 logical model calls, 48 tool calls, 20 Agent turns, 80 physical
model attempts, and `83,886,080` accounted tokens per run. The six-run campaign
aggregate is capped at one hour, 120 model calls, 288 tool calls, 120 turns,
480 physical attempts, and `503,316,480` accounted tokens. Any censor,
non-positive baseline, safety or preservation failure, resource regression, or
missing final uplift terminates and freezes the collaboration type; a started
physical run cannot be retried by this protocol.

The tracked manifest explicitly sets `execution_authorized=false`. The new
binary is preflight-only: it validates a clean source tree, tracked protocol and
case digests, complete redacted provider/model bindings, case prestates, and
new private output paths, then writes a private receipt with
`provider_calls_performed=0`. No private online authorization has been created,
and the online reservation/execution path is not wired. Therefore Goal 3D is a
frozen deterministic protocol, not provider evidence. It does not change V12,
production routing, prompt serving, or the current no-uplift decision boundary.

The next step still requires explicit authorization. Before any provider call,
the missing private authorization and online reservation/execution wiring must
bind the frozen preflight receipt without adding candidates, cases, retries,
budgets, or evaluator variants.

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
