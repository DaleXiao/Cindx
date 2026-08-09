# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| [Dynamic collaboration V1 result](evaluations/CINDX_DYNAMIC_COLLABORATION_V1_0.2.27_2026-08-09.md) | `0.2.27` | One preregistered 2-case x 4-treatment x 1-replicate route-blind matrix under the [frozen protocol](evaluations/CINDX_DYNAMIC_COLLABORATION_V1_PROTOCOL_0.2.27_2026-08-09.md); fresh-seed Auto/Pro; no reruns or profile mutation | `VALID_TARGETED_EVIDENCE`, `NO_GO_NOT_EXERCISED`: all six product cells had complete receipts, full quality and external effects, and zero safety violations, but every Auto/Pro cell executed Direct; Auto added latency and tokens, Pro added no quality, and no incremental collaboration claim is admitted |
| [Agent Real-World V5](evaluations/CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.md) | `0.2.22` | 6 frozen product cases; Oracle Reference/Grounded Direct/Auto/Pro; 72 position-balanced provider-backed runs | `VALID_BASELINE`, zero incomplete evidence, setup failures, timeouts, and safety violations; iso-budget adaptive-direct is `IMPROVED` with one additional quality-pass pair, unchanged completion, lower median latency, and fewer total tokens; workflow, learned-profile, and distillation are `NOT_EXERCISED`; Pro remains descriptive |
| [Conductor ownership targeted evaluation](evaluations/CINDX_CONDUCTOR_OWNERSHIP_V1_0.2.27_2026-08-09.md) | `0.2.27` | Frozen old/current train diagnosis plus 2-case control/candidate holdout; 32 retained provider-backed cells | `VALID_TARGETED_EVIDENCE`, `KEEP_HARNESS_FIX`: complete batch readback closes the observed terminal-verification failure, candidate product completion is 6/6 with full quality and zero safety violations, and receipt evidence is complete; every classifiable route is Direct, so Workflow, learned-profile, GEPA, distillation, and frontier uplift are not established |
| [Agent Real-World V5 (previous)](evaluations/CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.md) | `0.2.11` | Same frozen V5 matrix on its recorded revision | `VALID_BASELINE`; adaptive-direct was `NEUTRAL`, workflow, learned-profile, and distillation were `NOT_EXERCISED`, and Pro was descriptive |
| [Agent Real-World V4](evaluations/CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.md) | `0.2.9` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 position-balanced provider-backed runs with current-run tool, strategy, and provider receipts | `VALID_BASELINE`, zero setup failures, and zero safety violations; Auto/Pro pass their paired quality, completion, latency, and token gates, but the collector classifies continuation tool events in two Fast runs as outside the current logical run, so the receipt gate fails closed and broad orchestration uplift remains `NO-GO`; no learned artifact was supplied |
| [Agent Real-World V3](evaluations/CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.md) | `0.2.3` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 position-balanced provider-backed runs with strategy and provider receipts | `VALID_BASELINE`, zero setup failures, and zero safety violations; receipt evidence fails closed on five browser timeouts, Auto/Pro lose completion despite quality gains, and broad orchestration uplift remains `NO-GO`; no learned artifact was supplied |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md) | `0.1.98` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 provider-backed runs | `VALID_BASELINE` with zero setup failures and zero safety violations; Auto/Pro trade quality gains for materially lower completion and higher latency, so broad orchestration uplift remains `NO-GO` |
| [13A repair calibration](evaluations/CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md) | `0.1.95` | Exact repair rerun: 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | All 6 completed with 7/7 checks and no safety violation; false external grounding is closed on the frozen case; `CALIBRATION_GO` for matrix expansion only |
| [13A calibration pilot](evaluations/CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md) | `0.1.94` | 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | Coding passed in all modes; all long-horizon effects verified but terminal completion failed on the shared external-grounding contract; `CALIBRATION_NO_GO` for matrix expansion |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md) | `0.1.82` | Prior 72-cell V2 attempt | Four infrastructure-failed RAG/memory cells make it `INVALID_BASELINE`; retained for diagnosis only |
| [Agent Real-World V1](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md) | `0.1.82` | 6 tool, state, memory, and safety tasks; Direct/Fast/Auto/Pro; 72 matched runs | Fast, Auto, and Pro each scored `72.2%`; Auto and Pro added no quality over Fast and had lower completion plus higher latency; `NO-GO` for orchestration uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| [Memory-effect V2](evaluations/CINDX_AGENT_MEMORY_EFFECT_V2_0.2.19_2026-08-06.md) | `0.2.19` | 3 frozen cases; matched direct memory-on/off; 3 repeats; 18 provider-backed cells and 9 pairs | `IMPROVED`: 18/18 completed, 9/9 pairs evaluable, required-memory cases improved 6/6, and all 3 recalled-decoy controls passed both arms, with zero invalid evidence, confounds, regressions, safety violations, or setup failures; limited to the frozen matched direct harness |
| [Memory-effect V1](evaluations/CINDX_AGENT_MEMORY_EFFECT_V1_0.2.19_2026-08-06.md) | `0.2.19` | 3 frozen cases; matched direct memory-on/off; 3 repeats; 18 provider-backed cells and 9 pairs | `INVALID_EVIDENCE`: 18/18 completed, but one required memory-off cell invoked `skill.search` outside the frozen allowlist, leaving 8/9 pairs evaluable; five evaluable required pairs descriptively favored memory-on and all three controls passed, but no causal memory-uplift claim is admitted |
| [Direct-finalizer GEPA calibration](evaluations/CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.md) | `0.2.23` | Four preregistered Gate A pairs; exact prompt-only parent/candidate treatment; 16 retained provider receipts and 20 conservative call reservations | `VALID_TARGETED_EVIDENCE`, `NO_GO_FOR_PROMOTION`: the candidate regressed deterministic verification on one preservation case, so GEPA, holdout, snapshot creation, deployment, and transfer did not run |
| [Workflow GEPA V2 invalid attempt](evaluations/CINDX_WORKFLOW_GEPA_V2_INVALID_0.2.25_2026-08-08.md) | `0.2.25` | One authorized provider attempt; one seed task completed before the second was rejected by a source-spelling assertion | `INVALID_EVALUATOR`: coding checks prescribed implementation text and research checks exposed expected answers; no candidate, holdout, learned route, Task Graph, control, report, or snapshot exists |
| [Workflow GEPA V3 invalid attempt](evaluations/CINDX_WORKFLOW_GEPA_V3_INVALID_0.2.25_2026-08-08.md) | `0.2.25` | One authorized provider attempt; one seed passed before the second was rejected by an undisclosed numeric-port requirement | `INVALID_TASK_SPEC`: no mutation, validation, untouched product test, control, report, or snapshot exists; the attempt supports no capability or regression conclusion |
| [Workflow GEPA V4 invalid attempt](evaluations/CINDX_WORKFLOW_GEPA_V4_INVALID_0.2.25_2026-08-08.md) | `0.2.25` | One authorized provider attempt; both training tasks completed and produced a candidate snapshot before the first validation pair | `INVALID_EVALUATOR`: the matched gate compared path-bound revisions for separate isolated roots, so no validation or test arm ran; the snapshot was not admitted or promoted and supports no capability conclusion |
| [Workflow GEPA V4 invalid task-spec attempt](evaluations/CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.md) | `0.2.25` | Second authorized provider attempt; 2 training runs and 2 matched validation pairs on clean source `ee408db`; untouched test and Grounded Direct control remained gated | `INVALID_TASK_SPEC`: the failed research case hid lower-case identifier and exact-phrase requirements absent from its public contract, so the raw tie, quality, and efficiency receipts are diagnostic only and support no capability or regression conclusion; no production profile changed |
| [Workflow GEPA V5 provider evaluation](evaluations/CINDX_WORKFLOW_GEPA_V5_0.2.25_2026-08-08.md) | `0.2.25` | First authorized V5 run; 3 train candidates, 6 matched train pairs, and 2 unseen validation pairs on clean source `86f7dd6`; control and test remained sealed | `VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`: the selected profile causally changed the train route and exercised Workflow, but validation produced 0 wins and 2 ties at `1.4337x` latency and `1.2966x` tokens; no snapshot or production profile was published |
| [Workflow GEPA V6 frozen protocol](evaluations/CINDX_WORKFLOW_GEPA_V6_PROTOCOL_0.2.25_2026-08-08.md) | current source | 8 new frozen tasks; quality-or-Pareto-safe train admission; unseen validation; untouched test; exact causal receipts and task-level workflow budgets | `PROTOCOL_ONLY`: deterministic contract tests pass, but no V6 provider run exists and no quality, efficiency, promotion, or frontier claim is admitted |
| [Workflow GEPA V7 frozen protocol](evaluations/CINDX_WORKFLOW_GEPA_V7_PROTOCOL_0.2.25_2026-08-09.md) | current source | Same 8 behavior contracts without prescribed routes; observation-driven one-or-two-gene proposals; external action journal; strict product and campaign caps | The frozen protocol is exercised by the separate `0.2.26` provider report; its decision is `NO_GO_TRAINING`, with no quality, promotion, or frontier claim |
| [Workflow GEPA V7 interrupted campaign](evaluations/CINDX_WORKFLOW_GEPA_V7_INTERRUPTED_0.2.25_2026-08-09.md) | `0.2.25` | One authorized clean-source run; 2 completed train seeds followed by one bounded mutation-search reservation | `INTERRUPTED_CAMPAIGN`: mutation stopped on `no_progress` before a candidate population existed; no matched candidate pair, validation, test, control, snapshot, profile change, or capability conclusion |
| [Workflow GEPA V7 provider evaluation](evaluations/CINDX_WORKFLOW_GEPA_V7_0.2.26_2026-08-09.md) | `0.2.26` | One authorized clean-source run; 2 train seeds, 3 distinct candidates, and 6 matched train pairs; validation, test, and control remained sealed | `VALID_TARGETED_EVIDENCE`, `NO_GO_TRAINING`: all runs completed with full quality and zero safety violations, but all 6 pairs tied, all candidates remained Direct, and none met the Pareto resource gate; no snapshot or production profile was published |
| [Workflow GEPA V9 invalid instrumentation attempt](evaluations/CINDX_WORKFLOW_GEPA_V9_ROUTE_CAUSAL_PROTOCOL_0.2.28_2026-08-09.md) | `0.2.28` | One authorized clean-source attempt; first product cell reserved before receipt projection rejected the deliberate matched constraint | `INVALID_INSTRUMENTATION`: zero completed product receipts and no quality, candidate, snapshot, promotion, or capability conclusion |
| [Workflow GEPA V10 invalid instrumentation attempt](evaluations/CINDX_WORKFLOW_GEPA_V10_ROUTE_CAUSAL_PROTOCOL_0.2.28_2026-08-09.md) | `0.2.28` | One coding pair completed before the first research arm exposed independently planned treatment shapes | `INVALID_INSTRUMENTATION`: provider compliance and plan variation remained confounded with execution; no report, candidate, snapshot, promotion, or capability conclusion |
| [Workflow GEPA V11 invalid evidence attempt](evaluations/CINDX_WORKFLOW_GEPA_V11_ROUTE_CAUSAL_PROTOCOL_0.2.28_2026-08-09.md) | `0.2.28` | Shared workflow-plan anchor and runtime-only Direct projection; one first-arm reservation on clean source `24b11ac` | `INVALID_EVIDENCE`: direct-finalizer assignment evidence could not bind a terminal completion, so zero product receipts and zero matched pairs were admitted; no GO/NO-GO, candidate, snapshot, promotion, or capability conclusion |
| [Workflow GEPA V12 frozen protocol](evaluations/CINDX_WORKFLOW_GEPA_V12_ROUTE_CAUSAL_PROTOCOL_0.2.29_2026-08-09.md) | current source | V11's shared-plan treatment with independent route/task-graph and direct-finalizer evidence projection; all tasks, budgets, gates, and one-shot rules unchanged | `PROTOCOL_ONLY`: deterministic regression checks cover the V11 failure shape; no V12 provider-backed GO/NO-GO or capability result exists yet |
| Goal Delta control contract | current source | Deterministic obligation, grounding, postcondition, repeated-satisfaction, failure, and bounded-receipt cases | Ordinary tool success no longer earns budget credit; control-plane evidence only, with no provider-backed quality claim |
| Typed denial and bounded replan contract | current source | Deterministic permission/policy/capability denial, epoch isolation, persistence, evidence visibility, terminal disclosure, and no-credit cases | Denial can converge to an honest blocked result without becoming success; no provider-backed convergence or intelligence-uplift claim |
| Typed Pro failure curriculum contract | current source | Deterministic canonical timeout/denial/no-progress projection, redaction, scope/epoch binding, successful anchor, diversity, replay, and shared six-packet cap | Failed runs can inform challenger generation without becoming positive evidence or teachers; no provider-backed intelligence-uplift claim |
| Typed collaboration execution contract | current source | Deterministic worker tool-policy separation, current-epoch evidence projection, target binding, duplicate contribution rejection, and cited verification receipts | Workflow evidence and verdict authority fail closed; no provider-backed collaboration-quality or intelligence-uplift claim |
| Semantic read no-gain contract | current source | Deterministic exact-action/content identity, non-adjacent repeat, changed-evidence, effect barrier, fail-open, cold-reset, and eight-entry scaling cases | Repeated successful reads can request bounded replanning without caching or progress credit; no provider-backed quality or efficiency uplift claim |
| Causal Router v2 shadow and ExecutionPlan v2 | current source | Deterministic exact-input fingerprints, Conductor-owned typed demand, complete executable action identities, tri-state effect authority, indexed exact-context and generalized route-shape matched evidence, 4 KiB shadow receipt, 2-lookup/2-action scaling bounds, typed candidate-to-action authority, and V1 replay compatibility | Routing provenance and bounded shadow diagnostics are verified. The compatibility policy cannot rewrite a V2 action; no provider-backed quality or Auto/Pro uplift claim |
| Authoritative Context Compiler contract | current source | Deterministic effective-objective optional-context ranking, protected-source and current-request invariants, no-text 4 KiB receipt, and operation-count scaling bounds | Request attribution and resource bounds are verified; no provider-backed answer-quality, GEPA, or Auto/Pro uplift claim |
| Direct-finalizer causal/evolution contracts | current source | One-gene Auto/Pro Direct phenotype, exact assignment/request/delivery attribution, matched prompt-only treatment, strict reviewer and GEPA decision receipts, durable 58-call cap, and frozen 6-train/8-holdout campaign | Mechanism and causal boundaries are verified; quality uplift requires the explicit provider-backed campaign |
| Prompt profile shipping contract | current source | Deterministic compact deployment, deletion fences and monotonic generations, authoritative recovery, injected serving-write failure with canonical-learning preservation, logical-run stable/canary assignment, exact live-assignment/lease attribution, typed fallback, behavior-only route phenotype, authority ceilings, and operation-count bounds | Proves learned-profile wiring from route through Task Graph, recoverable serving projection, and hard safety/budget boundaries only; provider-backed uplift remains unverified |
| Workflow GEPA route-causal campaign V12 | current source | Frozen V7 task splits and product gates; shared conductor candidate and workflow proposal; runtime-only Direct/Workflow treatment; independent route and finalizer evidence; route-only candidates; durable external journal and 33-run maximum | Deterministic causal boundaries are frozen; V11 remains `INVALID_EVIDENCE`, and V12 has no provider-backed result yet |
| Memory-effect V2 contract | current source | V1's 3 cases and 18-cell matched design, adding the observed permissionless read-only `skill.search` tool to the frozen allowlist | Deterministic receipt and analyzer checks fail closed on any other tool; the exact `0.2.19` V2 matrix demonstrates bounded memory benefit under its frozen matched direct harness, while V1 remains `INVALID_EVIDENCE` |
| Memory lifecycle contract | current source | Matched-regression recall quarantine with expiry, verified-user authority, pin and requirement retention, observed-use preference, and hard capacity bounds | Proves deterministic forgetting and recall eligibility only; it does not establish provider answer-quality uplift |
| Delivery-frontier runtime contract | current source | Production adaptive scheduling follows only nodes required by the synthesis or verification target and stops with disconnected speculation still pending | Proves bounded graph execution and avoids orphan model work; it does not establish provider collaboration-quality uplift |
| Deterministic quality gates | current source | Runtime, memory, semantic-curation loss/retry/panic recovery, queue/steer, permission, restart recovery, process CPU/output/time budgets, projection, and performance contracts | Control-plane evidence only |

The `0.2.22` V5 matrix is the latest complete provider-backed product baseline.
All 72 cells were retained, with complete provider and strategy evidence, zero
setup failure, zero timeout, and zero safety violation. Its `VALID_BASELINE`
status means the matrix is structurally usable; it is not a capability `GO`.
Against the iso-budget Grounded Direct product baseline, Auto improved one of
18 matched quality outcomes (`+5.6 pp`), preserved completion, had a
matched-pair median latency delta of `-3,585 ms`, and used about `10.0%` fewer
total tokens. The frozen claim gates therefore classify adaptive-direct as
`IMPROVED` on this exact matrix; they do not identify which individual control
caused the gain.
No Auto run exercised workflow collaboration, and no frozen learned artifact
was supplied, so workflow, learned-profile, and distillation remain
`NOT_EXERCISED`. Pro improved one matched quality outcome and completed one more
matched run, but remains descriptive because its native budget differs.

The `0.1.95` 13A repair calibration remains the before-matrix confirmation
that false external grounding was closed on its exact six-run subset. The
older invalid V2 attempt and complete V1 matrix remain historical evidence for
their source revisions. Oracle Reference is a no-tools answer reference with
fixture evidence supplied inline, not an equal product treatment. The GPQA run
is an older difficult-question diagnostic without tools.

The V2 raw schema did not capture learned profile or GEPA identities. The
result therefore cannot be attributed to GEPA, transfer, self-distillation, or
a particular promoted profile, and it does not establish Fugu Ultra parity.

The current source contains an Auto-to-Pro transfer gate. Its deterministic
tests establish evidence qualification, lineage isolation,
position-balanced comparison, dual-gate promotion, frozen provenance, and
strict paired high-information mutation input. The ordinary and transfer gates
also reject candidate-only failure and absolute holdout quality, latency, or
token regression against the stable profile, even if relative reviewer reward
is positive. Those are mechanism checks, not answer-quality evidence.
Historical transfer evidence did not execute the per-run route decision layer.
The authorized Workflow GEPA V2 and first V4 attempts are `INVALID_EVALUATOR`;
the authorized V3 attempt is independently `INVALID_TASK_SPEC`. V3 hid required behavior from
the Agent, used holdout observations while selecting a candidate, and relied on
read-only prompt-evaluation outcomes rather than matched complete product runs.
V4 reached candidate generation, but a path-bound workspace comparison stopped
before validation. None of these invalid attempts supports a capability or
regression claim.

The second authorized V4 run is independently `INVALID_TASK_SPEC`. Its candidate
completed both matched validation cases and carried the exact learned profile
and route receipts, but the failed research case required lower-case identifiers
and an exact phrase that were absent from the public task. The raw route,
latency, token, and terminal receipts remain diagnostic, but its quality result
cannot reject or admit the candidate. The gate stopped before untouched test
and Grounded Direct control, and no profile was promoted.

The feature-gated Workflow GEPA V7 campaign separates search, validation, and
untouched product test. It keeps the eight V6 behavior contracts but removes
all public route labels: the Conductor must decide whether collaboration is
useful from the task and available evidence. Two public training tasks first
produce redacted seed reflection evidence. The same frozen evidence generates
three candidates with distinct route phenotypes through at most six
trajectory-grounded proposals; each retained proposal must change exactly one
or two measured genes. Each candidate then receives matched full-product train
pairs on one coding and one research task. A candidate is train-eligible only
if every cell completes with full quality and zero safety violations, exact
learned-profile and route receipts appear in both cells, and it records either
an externally verified quality win with both resource ratios at most `1.25`, or
a Pareto-safe 5% improvement in one resource while the other remains at most
`1.05`. Route contrast and Task Graph use are descriptive causal receipts, not
product gain. Instance-wise Pareto selection ranks quality and verified success
first. Validation and test evidence cannot select a candidate.

The train-selected frozen snapshot remains in the campaign's private temporary
directory while it must pass two unseen validation pairs before four untouched
test tasks are opened. The external snapshot is published only after validation,
Grounded Direct control, and the final test all pass. Final evidence consists of two counterbalanced seed/candidate
product repeats per test case on separately materialized identical workspaces.
Promotion evidence requires complete quality acceptance for every candidate
cell, quality wins on at least two distinct unseen cases, zero quality losses or
safety violations, no completion regression, bounded latency and token ratios,
exact candidate profile and route receipts on every candidate run, and a
passing Grounded Direct control. Validation requires both resource ratios at
most `1.25` and either a quality win or a Pareto-safe 10% resource gain.
Provider-backed semantic-memory extraction and cloud
embedding are disabled only inside this isolated campaign; deterministic local
memory projection remains. The campaign emits an external candidate snapshot
but does not claim or perform production promotion. The first provider-backed
V5 run is retained as `VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`: the exact
candidate profile changed the training research route from Direct to Workflow,
but unseen validation produced two quality ties, no Workflow execution,
`1.4337x` latency, and `1.2966x` tokens. Control and untouched test remained
sealed, no snapshot was published, and no uplift or promotion claim is admitted.
The V7 protocol adds a private external hash-chained journal before every
provider action, a 20-call/10-minute product cap, and campaign caps of two hours,
35 product runs, 700 product model calls, and 12 mutation calls. Shipping
Fast/Auto/Pro budgets are unchanged. The first authorized V7 campaign completed
both training seeds, then stopped on `no_progress` during mutation search before
any candidate population existed. Its pending journal action blocks replay.
After the provider stream liveness path and candidate-population history were
fixed without changing the frozen product gates, the separate `0.2.26` run
generated three distinct candidates and completed all six matched train pairs.
Every pair tied on quality, every candidate remained Direct, and none met the
Pareto resource threshold, so validation, test, control, snapshot publication,
and production promotion remained sealed. This is valid targeted no-go evidence,
not an intelligence-uplift result.

Current source retains that frozen V7 product suite but emits
`cindx.workflow-gepa-product-evidence.v12`. The first arm in each pair requests
one workflow-capable conductor plan; the second arm reuses that exact private
in-memory anchor. Workflow executes it, while Direct is a runtime projection of
that same candidate. Arm order is counterbalanced. Exact conductor-candidate and
workflow-proposal hashes, task, workspace, profile, models, and budget must
match. Missing, duplicated, drifted, or incorrectly executed arms fail closed.
If Workflow does
not win at least one externally verified matched quality outcome, the campaign
stops before mutation. When a positive treatment exists, GEPA may change only
the new `route_directive`; executable workflow, safety, tool, and budget genes
remain fixed. Candidate selection is train-only, and the existing unseen
validation, Grounded Direct control, untouched test, and resource gates remain
sealed in order. This is a causal-boundary and preregistration improvement, not
a capability result. The one authorized V11 attempt reserved its first arm but
admitted zero product receipts because direct-finalizer assignment evidence
could not bind a terminal completion. It is `INVALID_EVIDENCE`, supplies no
GO/NO-GO, and leaves production routing unchanged. V12 projects route/task-graph
evidence independently from direct-finalizer execution evidence, so a failed
product task remains a zero-quality outcome when the route evidence itself is
complete. It preserves V11's frozen suite, treatment, thresholds, and budgets
and has not yet been run. V9 failed receipt projection. V10 completed one pair
but then exposed that independently requesting Direct
and Workflow confounded planning with execution. The three failures are not
quality, efficiency, promotion, or frontier evidence.

The Pro-to-Auto path additionally replays the current Pro champion's two gates,
transfers only the complete bounded structural delta relative to the stable Pro
it defeated, and uses an immutable, dual-sided staged canary with conflict-safe
evidence replay. These are deterministic eligibility and rollback mechanisms;
they do not establish that a distilled Auto profile improves provider-backed
answer quality.

The current Pro mutation path also has an isolated negative failure curriculum.
Its exact contract proves that only typed current-epoch timeout, denial, and
no-progress facts survive redaction; a successful scientific observation must
anchor their use; selection remains diverse, deterministic, and within the
existing six-packet context limit; and the same failures remain ineligible for
positive, teacher, canary-success, promotion, or distillation roles. This proves
the learning boundary, not that model-generated challengers improve answer
quality.

## Evidence Levels

### 1. Contract tests

Rust, frontend, sidecar, and structure tests verify deterministic behavior such
as parsing, permission scope, cancellation, lifecycle reduction, persistence,
and graph invariants. They answer “does this contract hold?” They do not answer
“is this Agent better?”

### 2. Deterministic evaluation suites

Versioned manifests under `benchmarks/` validate routing contracts, memory
recall fixtures, evaluation schemas, resource invariants, and safety behavior.
Synthetic or fixture-based scores must remain labeled as contract results.
The memory-effect V1 contract additionally proves that both arms use the same
evaluation-only matched route: the current compatible executor model with a
fixed direct, read-only, relevant-memory plan and query. Memory-off is imposed
only after that route is recorded and changes only the effective memory plan.
All 18 cells remain in the denominator, outputs and provider receipts are
revalidated, every case denies mutations and restricts receipts to frozen
read-only evidence tools, and the irrelevant-memory control must actually
recall its decoy while passing in both arms before a positive decision is
possible. This isolates memory utility and recalled-context resistance rather
than Auto router quality. It does not call a provider in deterministic profiles
and does not itself prove useful memory.
The `causal-router-v2-contract` and `causal-router-v2-scaling` gates establish
deterministic policy identity, receipt integrity, counterfactual availability,
effect-authority enforcement, and bounded operation counts. They do not show
that the selected route produces a better provider answer; that requires a new
frozen matched provider run with failures retained in the denominator.
The `context-compiler-contract` and `context-compiler-scaling` gates likewise
verify the fixed authoritative-objective policy, bounded no-text receipt, and
candidate-proportional operation counts without using wall-clock time. They do
not call a provider, exercise GEPA, or establish that the selected context
improves answer quality. A quality claim requires a separately frozen matched
provider treatment that isolates this compiler policy and retains failures in
the denominator.

The Direct-finalizer causal campaign is a narrower explicit provider treatment.
Its frozen suite contains six train and eight holdout cases; four preregistered
train cases form Gate A and include both correction and preservation controls.
Each pair starts from the same canonical runtime and permits exactly one request
difference: the tools-disabled Finalizer system-prompt directive. Provider,
model, budget, context, task contract, evidence and pre-treatment identity are
otherwise matched. A separate position-balanced reviewer emits strict receipts.
Only after Gate A may a GEPA selector see the exact exercised candidate plus
redacted matched deltas and return strict `promote` or `reject`; it cannot invent
an untested phenotype. Reject stops before holdout. The durable checkpoint
reserves every provider call before dispatch and caps the campaign at 58 calls
including one bounded repair. Raw outputs and checkpoints stay outside Git;
sanitized receipts retain failures, model identities, digests and the decision.
Once Gate A has run, the sanitized writer also requires the exact candidate
profile identity; a blocked report cannot depend on its private checkpoint to
recover lineage.
Neither deterministic contracts nor a partial campaign establish quality gain,
general Agent uplift, or Fugu parity.

The authorized `0.2.23` campaign completed all four Gate A pairs. The
Adversarial candidate passed one deterministic case versus two for the parent
and regressed the `train-test-failure` preservation case. Gate A therefore
blocked with `candidate_regression` after 20 conservative call reservations.
GEPA attempts remained zero and no holdout evidence, snapshot, deployment, or
learned-profile uplift exists. The independent reviewer and efficiency signals
favored the candidate descriptively, but cannot override the frozen objective
regression.

The campaign is never part of an ordinary quality-gate profile. From a clean
exact revision, an explicitly authorized provider run uses external private
paths:

```sh
CINDX_DIRECT_FINALIZER_REPORT=/private/tmp/cindx-direct-finalizer-report.json \
CINDX_DIRECT_FINALIZER_CHECKPOINT=/private/tmp/cindx-direct-finalizer-private.json \
CINDX_DIRECT_FINALIZER_SNAPSHOT=/private/tmp/cindx-direct-finalizer-snapshot.json \
cargo run --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --features realworld-eval --bin cindx-direct-finalizer-gepa-eval
```

The configured Summarizer produces both arms, the configured Reviewer performs
the blinded pair review, and the configured Planner performs the bounded GEPA
decision. The producer and reviewer identities must differ.

### 3. Provider reasoning diagnostics

The bounded GPQA runner compares configured treatments on a frozen matched
sample. It controls case identity and scoring, but model assignment, serving
conditions, timeouts, and small sample size still limit attribution. GPQA is
useful for detecting reasoning regressions; it is not the primary Agent gate.

### 4. End-to-end Agent evaluation

A product-quality claim requires tasks that exercise tools, evidence gathering,
state, recovery, and delivery. Treatments must receive equivalent task access,
budgets, deterministic verification, and repeated matched cases. Failures stay
in the denominator.

### 5. External parity evaluation

Fugu parity requires the complete frozen protocol, safety attestation, matched
budgets, and provider-backed observations described in
[FUGU_EVALUATION.md](FUGU_EVALUATION.md). A subset or a historical score cannot
be promoted to parity evidence.

## Required Report Fields

Every retained provider-backed report must include:

- application version and exact source commit;
- capture time and frozen case or dataset provenance;
- treatment configuration and budgets;
- completion, failure, quality, latency, token, tool, and safety outcomes;
- raw-evidence digest while keeping secrets and protected benchmark content out
  of Git;
- explicit confounds and claim boundary;
- a scientific-validity decision and a separate product-uplift decision,
  including `NO-GO` when evidence is insufficient or negative.

Reports with an unknown source revision are not project evidence. Historical
reports are immutable and live under `evaluations/archive/`.

## Current Real-World Findings

The `0.2.22` V5 baseline contains 72 position-balanced observations. Oracle
Reference, Grounded Direct, Auto, and Pro passed `100.0%`, `77.8%`, `83.3%`,
and `83.3%` of quality checks, respectively, with zero safety violation.
Completion was `100.0%`, `77.8%`, `77.8%`, and `83.3%`. The matrix retained 61
completed and 11 failed runs, with no timeout, permission wait, setup failure,
provider-evidence gap, strategy-evidence gap, or safety violation.

The primary causal comparison is Grounded Direct versus iso-budget Auto. Across
all 18 matched pairs, Auto improved one quality outcome (`+5.6 pp`) while
completion was unchanged. Auto's matched-pair median latency delta was
`-3,585 ms`, its aggregate median-latency ratio was `0.906`, and its total-token
ratio was `0.900`. The quality, completion, resource, setup, and safety gates
all passed;
adaptive-direct is therefore `IMPROVED` for this frozen suite. Strategy receipts
show no executed Auto workflow subset, so workflow is `NOT_EXERCISED` rather
than neutral or improved.

Pro improved one matched quality outcome (`+5.6 pp`), completed one additional
matched run (`+5.6 pp`), and had a matched-pair median latency delta of
`-137 ms` against Grounded Direct. That result is descriptive only because Pro
uses a different native budget. All nine product coding runs failed terminal
completion; one Grounded Direct and one Auto browser run also failed. Every
failed run remains in the denominator.

No frozen Auto or Pro profile artifact was supplied. The exact-parent gate
therefore classifies learned-profile and Pro-to-Auto distillation as
`NOT_EXERCISED`. V5 provides no provider-backed evidence of GEPA learning,
transfer, self-distillation, workflow uplift, Fugu parity, or frontier Agent
performance.

## Current Product Gate

`benchmarks/agent/realworld-v5.json` freezes the current execution contract. It
preserves V4's six product cases, three repeats, cyclic Latin-square ordering,
isolated browser fixture, typed effect receipts, exact target binding, and
permission-safety checks. Its four treatments are deliberately different:

- `oracle_reference` is the former no-tools Direct answer ceiling and is not a
  product baseline;
- `grounded_direct` runs Auto's shipping policy and budget through the same
  AgentKernel, models, tools, retrieval, memory, permissions, and external
  postcondition verifier, while disabling workflow collaboration after routing;
- `auto` is the iso-budget adaptive candidate;
- `pro` is reported descriptively because its native budget is larger.

Adaptive-direct and workflow conclusions use only their actual matched route
subsets, so a mixed Auto matrix does not collapse both mechanisms into one
claim. Workflow requires `workflow_profile_exercised=true`. A result without an
observed behavior difference is at most `NEUTRAL`; a positive state additionally
requires the frozen quality improvement, completion non-regression, latency,
token, setup, and safety gates. Failed, timed-out, denied, and early
unclassifiable runs remain in the matched denominator. Learned-profile and
Pro-to-Auto distillation claims require receipts proving that the challenger was
compared with its actually executed exact stable parent; otherwise they remain
`NOT_EXERCISED`.

The V5 contract has deterministic structural coverage through
`node --test scripts/agent-realworld-contract.test.mjs scripts/agent-realworld-claims.test.mjs`.
This validates the mechanism only. The retained `0.2.11` provider-backed matrix
is historical evidence for its recorded revision. The current `0.2.22` matrix
authorizes only the mechanism states recorded above.

The exact Goal Delta gate proves that failed, denied, cancelled, unrelated, and
repeated satisfaction cannot extend a segment, while first satisfaction of a
contract obligation, first recorded grounding receipt, or verified
postcondition can. It also proves that the receipt stays bounded when the
observation is large. Target binding and desktop persistence/recovery remain
separate deterministic contracts; this exact filter does not prove them. This
is a run-control improvement. V5 measured the shipping path, but it contains no
causal ablation that attributes quality or completion to Goal Delta.

The typed denial gate proves that a permission denial is `Blocked`, never
`Satisfied`, carries no Goal Delta, survives bounded checkpoint and hot/cold
permission recovery without raw arguments, and requires visible denial evidence
plus an explicit terminal disclosure. Policy and unavailable-capability denials
share one same-epoch replan token; a subsequent denial finalizes blocked, while a
new contract epoch clears the state. These are deterministic control-plane
properties. V5 completed all denied-mutation product runs with the required
visible disclosure, but that narrow observation is not a general convergence or
intelligence claim.

`benchmarks/agent/realworld-v4.json` remains the immutable protocol used by the
historical V4 product baseline. V5 preserves its case and execution-order
controls while replacing Fast with the iso-budget Grounded Direct product
baseline and separating mechanism claims.

The V5 raw and sanitized schemas bind the exact source revision, application
version, suite and execution-plan digests, configured role models, resolved
shipping budgets, actual strategy decision/profile hashes, and every successful
model response. Provider response identifiers and system fingerprints are
domain-hashed before publication; request-body and canonical semantic-response
digests bind text, ordered tool calls, and finish reason without committing
prompts, secrets, or full outputs.
Missing provider identifiers and inconsistent streaming identities remain
explicit evidence failures rather than being counted as valid receipts.

Optional Auto or Pro `FrozenPromptProfileSnapshot` inputs must remain outside
Git. Their byte digest, canonical artifact digest, candidate identity, method,
and promotion provenance are validated and bound into the execution plan before
use. A run without such an artifact is labeled `FRESH_SEED_ONLY` and cannot
support a GEPA, transfer, self-distillation, or learned-profile claim. A frozen
artifact authorizes conclusions only about that exact artifact; it does not
establish general learning uplift or Fugu parity.

The `0.2.22` V5 collection is the latest complete provider-backed product
baseline. The `0.2.11` V5 collection, V4, V3, and the `0.1.98` V2 baseline
remain historical evidence for their source revisions, and the earlier
`0.1.82` V2 attempt remains invalid. V2 through V5 cover structured file
mutation, code edit plus tests, browser
evidence, long-horizon migration, indexed knowledge plus cross-session memory,
and denied mutation.

The feature-gated `cindx-agent-realworld-eval` binary drives the shipping Agent,
tool, permission, RAG, and memory paths. Direct is explicitly marked as a
no-tools model reference rather than product-mechanism evidence. The independent
contract rejects missing or duplicate cells, suite or output hash drift,
unmatched inputs, and unbound Git provenance. Raw outputs remain outside Git.

The runner builds the driver once, then isolates every case, treatment, and
replicate in its own process. Each sample writes a private pending checkpoint
before provider execution. The frozen suite applies a 600-second process
deadline; a timeout is retained as a failed sample, never retried into a pass.
Completed samples are merged in execution order and reused after interruption
only when Git commit, suite hash, full execution plan, profile artifacts,
selection, and replicate count still match. Browser processes are retired by
the sample's unique temporary root so one treatment cannot contaminate the
resources or state of the next.

The current provider-backed matrix is recorded in
[Agent Real-World V5 0.2.22](evaluations/CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.md)
with its [sanitized machine-readable evidence](evaluations/CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.json).
The earlier V5 collection, V4, V3, the `0.1.98` V2 baseline, invalid `0.1.82`
V2 attempt, and complete V1 baseline remain linked in the
[evaluation index](evaluations/README.md) as immutable historical evidence.
Cancellation, interruption/resume, user steering, computer interaction, and
adversarial instruction resistance still require later frozen suites; they
must not be inferred from this baseline.

## Running Checks

Deterministic profiles are documented in [QUALITY_GATES.md](QUALITY_GATES.md).
The provider baseline runner is preflight-only unless `--execute` is supplied:

```sh
node scripts/run-provider-baseline.mjs
node scripts/run-provider-baseline.mjs --execute
```

The execution form is billable and requires the private benchmark source and
configured provider. It must write raw evidence outside Git and commit only a
sanitized report.

The real-world Agent runner follows the same explicit opt-in boundary:

```sh
node scripts/run-agent-realworld.mjs \
  --raw /private/tmp/cindx-agent-realworld-v5.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V5.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V5.md

node scripts/run-agent-realworld.mjs --execute \
  --raw /private/tmp/cindx-agent-realworld-v5.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V5.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V5.md
```

Without `--execute`, it performs only Git, provider, suite, and output-boundary
preflight. A subset may be used for private calibration, but the publication
contract accepts only the complete frozen matrix. Re-running the same command
after interruption resumes validated completed cells from the private raw
checkpoint; timed-out cells remain failures. Optional `--auto-profile` and
`--pro-profile` arguments accept only private frozen snapshot artifacts and bind
their identities into the execution plan.

The offline Fugu matrix is generated by the non-shipping evaluation crate:

```sh
cargo run -p orchestrator-eval --example fugu_evaluation_lab --locked -- \
  --run-plan target/fugu-v1-run-plan.json \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md
```

Missing observations must remain `not_observed`; they are never replaced with
synthetic success.
