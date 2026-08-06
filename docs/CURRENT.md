# Current Product Baseline

Current application version: `0.2.11`

Last code-fact review: `2026-08-06`

This document describes the current source tree. Evaluation reports describe
only the revision recorded in each report.

## Execution Modes

- **Fast** bypasses the conductor and runs one configured model through the
  shared interactive agent loop.
- **Auto** asks the configured conductor for a typed `AgentRunDecision`, with a
  maximum requested parallelism of two. The decision may remain direct or
  select a bounded workflow. A workflow candidate then passes a runtime
  value-of-computation check over confidence-weighted predicted benefit,
  same-shape matched evidence, independent contribution/synthesis/verification
  cost, and serial-interaction risk. A rejected candidate becomes direct or
  grounded-direct without another conductor call while preserving its selected
  model, tools, vision, risk, retrieval, and memory. Runtime-derived tool and
  image-input requirements remain hard postconditions on both the decision and
  selected model; neither the conductor nor calibration can downgrade them.
- **Pro** uses the same decision contract with a maximum requested parallelism
  of three and a larger workflow budget.
- An invalid conductor response receives bounded repair. Exhausted conductor
  attempts produce an explicit capability-compatible degraded fallback rather
  than an unvalidated workflow. Strong matched direct-anchor evidence
  calibrates an otherwise admissible workflow to direct execution without
  spending repair or alternate-conductor calls. Every conductor request has a
  45-second no-progress boundary, including a configuration with only one
  conductor model; response progress retains the existing bounded recovery
  behavior.

All three modes ultimately use the same `AgentKernel`, run-control contract,
tool permission path, persistence path, and completion transaction. They differ
in planning and collaboration policy, not in separate product loops.

Every user-visible Agent task now has a versioned
`cindx.agent-run-identity.v1` identity. `logical_agent_run_id` is immutable for
the task, while the existing `agent_run_id` remains the physical attempt ID.
They are equal on the first attempt; steer changes only `steer_epoch`, and a
continuation or retry preserves the logical ID while creating a fresh physical
ID. `source_agent_run_id` names the exact physical recovery source. Routing,
memory, and evaluation aggregate canonical evidence by logical run, but
permissions, effect replay, idempotency, runtime snapshots, and the active UI
remain physical-attempt scoped. Legacy continuation chains are resolved only
within one task/project/session; ambiguous, cyclic, or cross-scope lineage is
not merged.

## Agent Run

A new run currently follows this sequence:

1. The Tauri command validates the session, provider configuration, workspace,
   attachments, and current-time context.
2. The desktop adapter appends the user message and task-start event to SQLite.
3. `AgentRunControl` establishes cancellation, steer, turn, stage, and deadline
   budgets; `agent-harness` prevents duplicate active work for the same key.
4. Session history is bounded and projected for the current objective.
5. The active objective, completion intent, current image input, and image
   generation requirement form typed route requirements. Fast chooses a direct
   decision; Auto and Pro request a conductor decision. Auto alone applies the
   value-of-computation admission policy after schema and capability validation.
   Both paths fail clearly when the selected configured model cannot satisfy
   required tools or vision.
   Compound execution instructions retain filenames and URLs while detecting
   later mutation steps. Local report fields such as `sources` inherit explicit
   workspace provenance instead of creating a web obligation; an objective
   that explicitly combines workspace and web evidence still keeps both scopes.
6. Durable memory recall and workspace retrieval are prepared without mutating
   canonical conversation history. Independent retrieval channels may execute
   in parallel; graph walk expands from selected seeds.
7. A workflow decision can run a bounded task graph and inject its grounded
   handoff into the interactive loop. Contributions must represent different
   work, while model reuse or diversity is selected dynamically from capability
   fit and supported evidence. A direct decision skips collaboration.
8. `agent-application` owns the only run/reprepare driver. Each prepared epoch
   uses `AgentKernel` for model turns, admitted tool batches, observations,
   contract checks, and terminal delivery. Tool exposure is focused by the
   validated conductor decision, and prompt-scoped capability and evidence
   obligations are bound to that epoch. Normal execution and permission recovery
   use the same planning path. After the matching runtime transition commits and
   its canonical tool outcome is durable or recoverable, `AgentTaskContract` may
   emit a bounded Goal Delta for a newly satisfied obligation, target-bound
   grounding receipt, or verified postcondition. Ordinary success, changed
   arguments or output, replay, and failed, denied, or cancelled calls do not
   receive progress credit or extend the run segment. A trusted denial is stored
   as bounded epoch, tool, input-fingerprint, kind, code, scope, and recovery
   facts rather than raw arguments or output. User permission denial enters
   blocked finalization; policy or capability denial receives at most one
   same-epoch replan before blocked finalization. The same denied action is
   intercepted before another permission request. A blocked obligation is never
   marked satisfied, and terminal delivery requires both visible denial evidence
   and an explicit blocker disclosure; it remains substantive rather than
   grounded or verified quality. Permission resolution and its canonical denial
   observation commit together; recovery state is prepared before an atomic
   claim/resume transition, failed handoff returns to `Paused`, and startup
   replay repairs an older checkpoint from that canonical observation. A
   committed steer returns
   through the same driver before another epoch can begin. When that durable
   commit actually applies user guidance, run control atomically opens one
   fresh base segment for the new objective without resetting lineage counters
   or resource limits. Deleted or otherwise no-op steers open no segment, and a
   steer is not a Goal Delta.
9. `first_verified` can stop early only for a genuinely verified deliverable.
   At terminal reserve the strongest usable result may still be returned, but
   its failed verification obligations remain explicit degradation rather than
   native success. A later unverified synthesis cannot replace a stronger
   verified result. The stream completion event belongs to the same request
   stream that delivered the selected text.
10. The completion transaction persists the result, artifacts, lifecycle state,
   learning evidence, and cleanup. Semantic memory refresh and prompt evolution
   are background work.

Within an active epoch, `PreparedTaskState` is the typed source for the effective
objective, steer/contract epochs, and completion intent; `AgentTaskContract`
owns the resulting obligations and evidence. Runtime checkpoints use the
versioned `cindx.agent.task-state.v2` wire, while an explicit v1 decoder retains
existing sessions. The v2 checkpoint stores objective fingerprints and typed
intent, not raw objectives or target anchors. Permission and restart recovery
rebuild runtime-only anchors from the effective objective and reject mismatched
lineage instead of silently applying state to another task.
Provider and tool activity still drives the existing liveness watchdog, while
generic checkpoints remain diagnostic. Neither extends a run segment: only an
accepted Goal Delta increments budget credit. This keeps slow valid I/O from
being mistaken for semantic progress and prevents busy but irrelevant tool
loops from unlocking the maximum Auto or Pro budget.

## Data, Memory, and Retrieval

- SQLite is the durable product state. Startup fails closed when the persistent
  store cannot be opened; the application does not silently continue with an
  in-memory substitute.
- Durable memory is produced from completed or explicitly eligible run
  evidence. Recall combines lexical and semantic evidence with trust,
  deduplication, supersession, current-request conflict suppression, and
  session-diversity controls. Semantic curation is reserved for workflow,
  workspace-evidence, or durable-effect runs; direct self-contained text runs
  use deterministic projection.
- Workspace knowledge is separate from memory. The current retrieval adapter
  supports file indexing, provider embeddings, local file persistence, and a
  production-enabled LanceDB store.
- Requested retrieval can combine semantic search, file search, graph-direct
  lookup, and graph walk. Results retain source provenance.
- A passing deterministic memory benchmark proves the frozen recall contract;
  it does not prove that every live agent run requests and uses the right
  memory.

## Prompt Evolution

Prompt evolution is outside the active agent loop. It consumes redacted,
completed evidence, evaluates candidates against paired and holdout gates, and
can promote a frozen profile for future runs. It cannot mutate an in-flight
transcript, tool result, permission, or budget.

Pro also has an Auto-to-Pro transfer track. A completed Auto workflow becomes a
teacher case only when its terminal evidence is positive, independently scored,
usage-complete, bound to the final steer epoch, free of denied permissions and
safety violations, and reconstructable from a finalized workflow checkpoint.
The independent score must carry a completed provider request receipt from a
dedicated reviewer request that did not author or execute the workflow;
synthetic or legacy scores cannot become teachers. A different evaluator model
is preferred when one is available, while same-model review remains explicitly
represented as a separate request rather than model diversity. The receipt
hashes the exact bounded artifact reviewed by that provider call, and the
archived teacher output is reconstructed from a matching persisted model result
or finalized checkpoint rather than a later unreviewed user-facing answer. The
evaluator request identity is checked against the runtime coordinator,
checkpoint steps, repairs, synthesis, and terminal artifact author, not only the
declared plan. The archived case contains
redacted bounded outputs and fingerprints for provider/model identity, system
prompt, policy, budget, tool contract, source/workspace evidence, evaluator
receipt, finalized checkpoint, and learning receipt. Its source steer epoch is
part of the transfer lineage and dataset digest, so evidence from an earlier
user objective cannot be reused after steering.

In background evaluation, current and challenger Pro profiles execute the same
objective and are compared with the archived Auto result by an independent,
position-balanced reviewer. Transfer evidence has a separate dataset identity
and cannot count as ordinary same-effort GEPA evidence. A Pro profile can be
promoted only when both the ordinary paired/holdout gate and the Auto-transfer
paired/holdout gate pass. Its frozen snapshot pins both evidence sets and the
active Auto profile fingerprint. Pro mutation now admits an Auto-transfer
reflection only as a complete, strict, source-attested train pair from the
active cohort. It supplies both the Pro execution and its matched Auto teacher
execution, ranks pairs deterministically by actionable regret, measured
contrast, runtime failures, and strategy-shape difference, then preserves task
and strategy diversity. Ordinary and transfer evidence still share the same
six-trajectory input ceiling, so richer learning evidence does not expand the
mutation context budget. Mutation events pin the selector schema and the digest
of the exact redacted trajectory set.

Pro mutation may also receive a bounded failure curriculum derived only from
canonical current-epoch timeout, denial, or no-progress outcomes. These records
contain typed failure codes and fingerprints, never prompts, tool arguments,
outputs, provider errors, or secret-bearing text. They are negative-only
`FailureSeed` reflections and enter mutation only beside a successful scientific
train observation for the same profile. The success anchor, at most two diverse
failure seeds, ordinary reflection, and Auto-transfer reflection still share the
same six-packet ceiling. Failure seeds cannot become positive learning evidence,
Auto teachers, Goal Delta, canary success, promotion evidence, or Pro-to-Auto
teachers. Permission failures can teach stopping or recovery within existing
authority, never permission bypass, budget expansion, or repeated side effects.

The learned genome continues to govern the evaluated workflow surface:
topology, branch shape, roles, verification, tool exposure, retry/recovery,
stopping, context policy, and bounded step budgets. Per-request route,
retrieval, and memory decisions remain owned by `AgentRunDecision`; the current
GEPA campaign does not execute that decision layer, so those fields are not
presented as learned strategy without a matched causal evaluation.

Transfer observations are appended as one mirrored pair and enter the canonical
evolution read model only when both sides are scientific, project-scoped, and
bound to the same source-attested Auto run and profile. Legacy transfer records
without this lineage fail closed. A projection-version change rebuilds older
caches from canonical events instead of silently preserving stale semantics. A
completed Auto run schedules its project-scoped
Pro learning intent durably; the background worker dispatches and idempotently
replays that intent after interruption. Background requests are
coalesced by project and effort; activity in one project cannot replace another
project's pending campaign. Campaign wall time, tokens, and physical provider
attempts survive request-scoped action events as well as checkpoints and are
never replenished by malformed or interrupted recovery state. When repeated
objectives have multiple qualified Auto teachers, the
current stable Auto profile is selected before stale profiles, then
independently measured quality and resource use break ties.

Auto-transfer and Pro-distillation discovery use one durable outbox projection.
Its versioned cursor uses an indexed tail lookup and reads only new canonical
events during an ordinary wake; a non-contiguous sequence, corrupt payload hash,
or invalid snapshot triggers deterministic full replay. The persisted FIFO work
window contains at most 256 undispatched intents, coalesces only within the same
project and learning track, and dispatches each track by canonical event sequence
rather than project-name order. Overflow remains in canonical events: after a
window drains, replay refills the next window instead of discarding or blocking
the backlog; replacing a retained project while overflowed forces replay before
dispatch order can change. Exact replay is idempotent, while an identity that
changes payload, project, or learning track fails closed. The orchestrator owns
normalized Auto/Pro intent identity, typed payload validation, queue transitions,
and interruption recovery. A legacy Pro marker is accepted only when its
normalized run context still identifies the pending intent, so an older rollout
cannot remove newer coalesced work. Desktop code only maps canonical events and
performs SQLite/CAS, worker, and provider side effects. Snapshot publication uses
compare-and-swap, retries one observed conflict, and stops on a second conflict
so a stale worker cannot overwrite newer state.

The current tree also contains a separate controlled Pro-to-Auto distillation
track. Only the active frozen Pro champion can be a teacher, and its attestation
must replay both its ordinary Pro gate and Auto-to-Pro source lineage at the
dispatch boundary. The runtime compares that champion with the exact stable Pro
profile it defeated and will transfer only a complete, attributable one- or
two-gene structural delta that already fits the Auto contract. It rejects
directive wording, unadapted Pro topology, excess Pro resources, and larger
bundles instead of selecting an untested subset. The child lineage binds the
teacher, defeated Pro fingerprint, and stable Auto parent rather than copying a
Pro genome wholesale. Teacher and ancestor cases or equivalent objectives
cannot enter the fresh train/holdout cohort; missing manifests, stale teachers,
mixed lineage, treatment failures, timeouts, or permission denials fail closed.

Distillation dispatch is asynchronous, project-scoped, idempotent, bounded, and
suppressed while a foreground Agent run is active. A real matched gate can
start a 10 percent Auto canary. Its immutable lease binds both profile
fingerprints, the matched cohort, and the paired evidence. Each 10 to 25, 25 to
50, and 50 to stable transition requires fresh candidate and stable outcomes;
failed, denied, malformed, and censored assigned runs remain negative canary
outcomes rather than disappearing from the completion denominator. Completion,
format, quality, latency, and token comparisons use rates or averages and check
each candidate task class against contemporaneous stable traffic. Evidence or
checkpoint drift, safety failure, or any regression rolls back to the prior
stable Auto profile, and a rolled-back immutable candidate remains quarantined
for that stable lineage. Exact evidence replay is idempotent; the same evidence
identity with a different payload invalidates the pair instead of replacing it.
The deterministic suite verifies these mechanism contracts only. No current
provider-backed result demonstrates that the distilled Auto child is more
intelligent than the stable Auto baseline.

Canary traffic is capped at 50 percent. A challenger can become the stable
profile only by first producing a valid immutable frozen snapshot; missing or
regressed ordinary/transfer lineage rolls the canary back and preserves the
previous stable profile. Both promotion tracks compare mirrored holdout lanes
against their stable profile. A candidate-only failure, absolute quality loss
beyond `0.01`, latency regression beyond `5%`, or token regression beyond `2%`
is a hard blocker even when the pairwise reviewer prefers the candidate.

Reflective mutations may learn a general strategy from redacted trajectories,
but a deterministic validator rejects candidate directives that contain case,
run, candidate, or participant-model identifiers, or copy substantial spans
from prompts, outputs, tool traces, verifier details, or actionable feedback.
Rejected and repaired mutations pass through the same validator.

Learned genomes, datasets, observations, and rollout state are isolated by
project. A reflective mutation produced from one project's evidence cannot
enter another project's candidate population, evaluation, or rollout.

Rollout transitions append their canonical event before any read-model cache can
observe the change. Evolution snapshots are published only when both the
previous revision and payload still match; a competing writer causes one
bounded reload and then fails closed. Conflicting genome payloads under the same
project, effort, and identity leave a compact fingerprint tombstone and remain
excluded from selection and rollout after their full payload leaves the hot
window. The hot projection applies deterministic, reference-safe
soft limits: active attempts, cohorts, stable/canary/frozen/quarantined profiles,
and their matched evidence are never evicted, so unusually large active evidence
may exceed a soft limit. Canonical events remain append-only and are the recovery
and audit authority; deterministic tests do not turn this cache contract into a
claim of provider-backed intelligence or strict constant memory.

The latest provider-backed raw schema binds the actual strategy/profile identity
and provider response receipts for runs that reach evidence collection. No
frozen learned artifact was supplied to the current matrix, so it remains
`FRESH-SEED-ONLY` and cannot attribute an outcome to GEPA, transfer, or
self-distillation. Deterministic tests prove the transfer boundary, evidence
isolation, promotion gates, snapshot lineage, canonical event projection,
project-isolated scheduling, mutation anti-memorization boundary, paired
high-information reflection selection, and absolute holdout non-regression
gates, but there is still no provider-backed matched evidence that an identified
evolved profile improves external product quality.

The current source defines Agent Real-World V4 as the active execution contract.
Browser cases receive a per-cell loopback HTTP fixture instead of a
`file://` target. Tool obligations can be satisfied only by successful typed
receipts from the current logical run; projected failed, denied, cancelled, and
unfinished calls remain visible in the tool-call denominator and cannot count as
tool success. A process-level timeout remains a failed matrix cell even when it
cannot emit a final tool receipt. Browser evidence must bind the exact resolved
target and an artifact or postcondition digest. The denied-mutation product case
also requires a user-visible permission-denied explanation; an unchanged file
with an empty Agent answer cannot pass quality. Deterministic tests verify this
measurement machinery only. The completed V4 `0.2.9` provider-backed matrix is
a `VALID_BASELINE`, but the collector classifies continuation tool events in two
Fast runs as outside the current logical run. The shared receipt gate and broad
orchestration-uplift decision are therefore `NO-GO`. Current source resolves
physical continuation attempts through the versioned logical-run identity and
a fail-closed legacy lineage projection. That is a deterministic attribution
fix, not a reinterpretation of the recorded V4 matrix; a newly frozen
provider-backed run is still required before making a quality or
orchestration-uplift claim.

## Current Evidence Boundary

The latest provider-backed Agent baseline is
[Cindx Agent Real-World V4 0.2.9](evaluations/CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.md),
captured on source commit `7905405551f3decd38746c45218790cb9a04be37`.
It retained all 72 position-balanced cells across Direct, Fast, Auto, and Pro,
with zero setup failures and zero safety violations. The publication contract
marks it `VALID_BASELINE`; the independent broad orchestration-uplift decision
is `NO-GO`.

- Direct, Fast, Auto, and Pro scored `100.0%`, `83.3%`, `100.0%`, and `100.0%`
  quality, with `100.0%`, `77.8%`, `83.3%`, and `94.4%` completion.
- Relative to Fast, Auto and Pro each gained three quality-pass runs and
  completed one and three additional runs, respectively. Their median-latency
  ratios were `2.03` and `1.94`, while total-token ratios were `0.90` and
  `0.80`; all were within their preregistered limits.
- Fast failed the RAG/memory answer check in all three repeats; Auto and Pro
  passed all three after successful setup.
- Browser quality passed in every treatment, but terminal completion remained
  weak: Fast completed `0/3`, Auto `1/3`, and Pro `2/3`.
- All product treatments completed all denied-mutation runs with the required
  visible permission-denied explanation.
- In one failed Fast long-horizon run and one completed Fast RAG run, the
  collector classified continuation tool events as outside the current logical
  Agent run. Those evidence errors make the shared receipt gate fail closed;
  neither cell was rerun.

The `0.1.98` V2 baseline, earlier
[13A repair calibration](evaluations/CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md),
invalid `0.1.82` V2 attempt, and complete V1 matrix remain historical reports
for their own revisions.

The latest matched GPQA diagnostic is version `0.1.78`: Direct scored `10/12`,
while Auto and Pro each scored `8/12` under the budget. The sample is too small
for broad conclusions, but it does not demonstrate orchestration uplift.

Therefore the current claim is:

- The control plane, local memory contract, permission boundary, and
  deterministic quality gates have substantial automated coverage.
- Auto and Pro show a real fresh-seed RAG/memory and completion signal in this
  small matrix, but the shared receipt gate prevents a broad uplift claim.
- The current source now has a deterministic, observable Auto
  value-of-computation admission mechanism and bounded single-conductor
  no-progress handling, plus typed denial and bounded same-epoch replan
  contracts. V4 measured their combined shipping path, but did not isolate
  their causal contribution.
- Pro is individually eligible against Fast in V4, but the matrix-wide receipt
  failure prevents promotion; browser completion remains the clearest measured
  weakness.
- No current provider-backed result identifies an evolved profile or proves a
  GEPA, transfer, or self-distillation gain.
- Cindx has not demonstrated Fugu Ultra parity or frontier Agent performance.

## Known Structural Limits

- The Tauri crate remains a large composition root. Portable contracts exist in
  dedicated crates, but provider calls, permission UI, tool side effects,
  persistence coordination, and several workflow adapters still meet in the
  desktop integration layer.
- `agent-rag` and parts of the desktop adapter remain large modules. Structure
  checks prevent some regressions but do not prove ideal boundaries.
- `agent-application` owns the portable run/reprepare and lifecycle contracts,
  but several use cases and all product side-effect adapters still live in the
  desktop composition root.
- Provider-backed real-world coverage now includes file mutation, code editing,
  browser evidence, long-horizon work, RAG/memory, and denied mutation.
  Cancellation, interruption/resume, steering, broader computer interaction,
  and adversarial instruction resistance still need matched repeated suites.

These are current constraints, not roadmap promises. A later change may remove
them only with code and verification evidence in the same revision.
