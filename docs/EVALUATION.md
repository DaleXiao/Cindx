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
| Collaboration successor V1 / Goal 3E | `12a3ea2`, `0.2.32` | `INVALID_EVIDENCE`, `CENSORED`. The one-shot capability was consumed and the first baseline Direct arm was reserved, but the product run ended before selection with zero selected decisions. The terminal producer correctly persisted explicit `not_selected`; no treatment or Owner execution occurred. The selected-only projector misclassified that legal state as a malformed receipt, so zero runs were admitted, no matched pair exists, and Workflow, candidate, and holdout did not run. The protocol cannot be retried and supports no uplift, cost, latency, or capability conclusion. |
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

## Delivery Verification Experiment

The current source adds default-off, `realworld-eval`-only construction,
projection, and a tracked provider-protocol authority for a final-draft
treatment. Control preserves the exact bytes of one shared Owner draft. The
treatment binds that draft and its ordered obligation/evidence context to a
model-distinct Independent Verifier. A pass preserves the Owner bytes; a
revision permits exactly one same-Owner repair and one Verifier recheck.

The frozen suite contains eight calibration cases and 24 model-hidden holdout
cases, balanced across unsupported claims, omitted obligations, contradictory
authorities, and preservation of exact `false`, `null`, and zero values. Cases
require deterministic multi-evidence calculation or rule selection. The
primary oracle is exact external JSON plus required/forbidden predicates; it is
bound into case and suite digests but omitted from model-facing input. The
calibration gate opens holdout only after all eight pairs complete with at
least two control failures, at least one treatment-only win, and no loss,
structural failure, or treatment execution failure. Calibration cannot tune
the prompt, models, or threshold. The 24-case holdout reports evidence of
uplift only for all-valid `W >= 5, L = 0`; the boundary case `5/0` has a
one-sided exact paired p-value of `1/32 = 0.03125`. Anything weaker is
no-evidence, regression, incomplete, or invalid rather than a positive claim.

The protocol fixes tool-free requests, no transport retries, at most four
physical calls per case, 128 calls total, 64,000 tokens per case, 2,048,000
tokens for the campaign, and a six-hour campaign ceiling. Its preflight only
validates the clean source, tracked bytes, case order, budgets, hidden-oracle
aggregate, redacted configured Executor/Reviewer identities, and new external
paths before writing a new private receipt. The v2 preflight also hashes the
exact sibling v2 execute binary and records its fixed name and byte count. That
receipt also binds the full SHA-256 CodeDirectory derived from a verified
private copy of those exact bytes. It states `provider_calls=0`,
`online_runner_frozen=true`, and `execution_authorized=false`.

The source contains provider-free authorization, one-shot execution, durable
campaign/call reservation, accounting, and fail-closed recovery contracts. The
authorization stage must match the execute bytes already frozen by preflight
and binds them together with canonical preflight, current
source/provider/model/credential authority, output root, and a short validity
window. Execution must also match the frozen CodeDirectory to the kernel-backed
identity of its running process before it can consume state. A single
output-authority marker, derived from the canonical output path and created
atomically in its parent, is shared by all authorization paths and remains after
the output root is moved or deleted. These deterministic contracts do not
invoke an external provider. The marker is local fail-closed state, not an
external anti-rollback guarantee against deletion or restoration of every
private file by the same user identity.

The successor manifest
`benchmarks/agent/delivery-verification-protocol-v2.json` has raw SHA-256
`3b1b758330403410486c28650f012b278859f680b55de23bc7aad0e09f6c4982`.
It references the unchanged v1 suite bytes
`b9672f7075d896e3c48673607c622604c0f8d6fb2b6df4499281b0741124fbcd`;
the ordered cases, hidden oracle, and budget remain respectively
`8d4ab335142e655dd53d93511bf6b4da0bb559fb99ca87cb66aef0c47b872913`,
`af47cdf41554956882e5fc42bd432a5c8dabff49573f2ce8f7181878e69d2b2d`,
and `4b72b261ce347f7ec0ab80328f6ed57050a04260bf81c6f0930d9c08f5c710fe`.
The invalid v1 result did not change any case, order, oracle, budget, threshold,
or retry rule.

### Consumed v1 result

The frozen v1 instance was authorized and consumed exactly once at source
`5373e654bd4ab0e5334a62ff199157f6b4daaf47`. Its retained public authorities
are:

- preflight receipt digest
  `47d9407465877994fe6994ed549456e334d011eb385dc5604e7dcd3947cf438e`;
- exact execute-binary digest
  `b6917770e39257dab4907e4202069b948c1a488da44075a958cdf961459722e8`;
- one-shot authorization digest
  `bca1c994e5c41eaee8c9c936c598315a8caf24dcaa316e07000bff091d375242`;
- terminal journal digest
  `67192fb14ad824d2967f20c54020060a498abacc7c6ee99a84b5c1e75a136070`;
- terminal receipt digest
  `df95c6a6913773d42de021271c1cbf499672042c510d728f551f74e3c8ccc414`;
- terminal evidence digest
  `ba2c9a6e6582c33a6eb542c5f4320c5debda66319a061ed9ffd97cb4927ebb00`.

The journal closed `CENSORED` / `INVALID-INSTRUMENTATION` during the first
calibration Owner call. The request reservation bound a canonical semantic
request digest, while the uncommitted provider result metadata supplied the
actual HTTP request-body digest under the separate
`cindx.model-provider.request-payload.v1` domain. Terminal validation
incorrectly required those two distinct digests to be equal. The provider-free
tests missed this because their fixture copied one synthetic digest into both
fields instead of crossing the real preparation boundary.

The journal charged one logical call, one physical attempt, and one 4,096-token
output reservation, but accepted zero terminal model-call receipts and no
usage, latency, provider identity, or case receipt. A response artifact was
written before terminal validation but is not bound by an accepted receipt and
is therefore excluded. No calibration decision or holdout call occurred, and
there are zero valid matched pairs. This result supports neither uplift nor
no-evidence, regression, answer quality, latency, usage, or cost claims. The
frozen v1 protocol is consumed and will not be rerun. Production finalization,
Workflow, Settings, serving, learning, promotion, GEPA, the installed App, and
the published release remain unchanged.

## Required Next Evidence

Do not rerun Delivery Verification v1. Current source now prepares one immutable
non-streaming wire body before dispatch, reserves semantic-request and
domain-separated wire-payload digests as distinct authorities in a v2 journal,
dispatches those exact bytes, and retains the primary terminal validation
failure. Provider-free local-loopback coverage crosses the real model-provider
prepare, reserve, HTTP dispatch, receipt, and terminal path; it does not
substitute one digest for both identities.

The v1 preflight, authorization, and execute entrypoints reject the consumed
protocol before live state is accessed. The separately named v2 preflight,
authorization, and execute entrypoints bind the successor authority, but only
provider-free preflight is permitted at this stage. There is no v2
authorization, execution, or provider result. Any provider attempt requires a
new explicit one-shot authorization for the exact merged-source preflight and
execute full-file and CodeDirectory digests. GEPA and production serving remain
out of scope.

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

The tracked manifest explicitly sets `execution_authorized=false`. The
preflight binary validates a clean source tree, tracked protocol and case
digests, complete redacted provider/model bindings, case prestates, and new
private output paths, then writes a private receipt with
`provider_calls_performed=0`. It remains unable to authorize execution.

Goal 3E implements the missing once-authorized control plane without changing
that authority. A separate provider-free binary can mint one private 15-minute
authorization bound to the canonical preflight, exact execute binary, current
clean source and provider/model configuration, frozen cells and budgets, and a
new one-shot output root. The execute binary must consume that capability and
durably reserve the campaign, each cell, and each arm before model work. Any
expiry, drift, tamper, reuse, interruption, censor, non-positive baseline,
candidate failure, resource failure, or absent holdout uplift stops terminally;
recovery cannot retry a started physical run.

The exact Goal 3E instance was then authorized and executed once. Its sanitized
authority is:

- source `12a3ea2105029d9df30781b1d2bf0cc832855a29`, tree
  `22151a52f4906043b040ab6667f5e99a8b567b50`, version `0.2.32`;
- manifest `befe6ee366f5d91cfac024b8d79493a616776cfbf2990cd071f16912a2ae61df`
  and suite `12c6ac24d3bad5d99421f53bc7c42c2ee303941e55a31a95ff7af45ff4e44fc1`;
- preflight receipt `82d3cd899ffe793b3df5ef091fdc1379b10c3c009d097e6f6ee191dd14a56f87`,
  execute runner `90b37b722166fc2dc8b63f4bfa06a6ab5275b25c3b4438e7b13e35d50d1b4117`,
  and authorization `34a87b2a137d5a3e7b3d51e844c2f3f888b036f02a11fbc8930e558844800f53`;
- redacted provider identity `28db4f61122b0583b64308a476cc00923d81921719d5de4da270ed4daee1a043`,
  configuration `ad6971a91692069b7b0ba93cebac182695a4ff218afca5e654a8e1ad3272f9ee`,
  capture pool `73611876a040101a100423235ed40437bf45fcc70f62aa17ec4aa3ae30e8c734`,
  and ordered pool `d02e5f8be5b3cb4c28c13f18eba2c60ebadc0973a0134dcd670368707a22c404`;
- run, outcome, and campaign budget receipts
  `aacf624fb32fd3cb5c397f5548bb8b3f665e38799cc597eaa47894c7c329da31`,
  `59bf01edde1db9ed4219be9a1c687b9a10439e947ac19f4eedc4ae6f500e7f57`,
  and `fd9e4c89936743a2575940a81c1a720d72538fccca5bc9d3abed3f97956869f2`.

The terminal journal closed at revision 4 with tombstone digest
`7add0265028b0baf918bc3f314398cb8bf4eab63ae2ea00223ad5e2ace6b001e`,
journal digest
`04c838031c9bc43be82aec4e97954b26fb501ecf53f138de7afa8a6e87db95c7`
and terminal digest
`b6e66f754fa20980b7e5d61f493af846895b687995ccd65fcfab929f0c5dc003`.
It charged one run's maximum reservation but admitted zero observed runs and
zero observed calls, attempts, tokens, or duration. The baseline Direct arm was
reserved; its Workflow arm and both later cells remained unstarted. The
product run terminated before selection with zero selected decisions. Its
terminal producer correctly persisted the explicit pre-decision `not_selected`
state; the treatment was never selected and Owner did not execute. The
selected-only outcome projector misclassified that legal state as a malformed
receipt and failed closed before a valid Pair/Censor observation could be
formed. This is instrumentation evidence only: Goal 3E is consumed,
`CENSORED`, and must not be rerun. Current source now classifies that legal
pre-decision state explicitly and covers the real terminal-producer-to-outcome-
projector seam; this corrects the diagnostic but cannot turn the consumed run
into evidence. No Goal 3F or provider run is authorized. V12 remains unchanged
and invalid.

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
