# Current Product Baseline

Current application version: `0.2.19`

Last code-fact review: `2026-08-06`

This document describes the current source tree. Evaluation reports describe
only the revision recorded in each report.

## Execution Modes

- **Fast** bypasses the conductor and runs one configured model through the
  shared interactive agent loop.
- **Auto** asks the configured conductor for a typed `AgentRunDecision`, with a
  maximum requested parallelism of two. The decision may remain direct or
  select a bounded workflow. Every candidate then passes Causal Router v2,
  which freezes a pre-decision task/capability/budget fingerprint and compares
  the candidate with its direct or grounded-direct counterfactual using explicit
  independent demand, capability provenance, evidence, model/coordination cost,
  critical-path latency, and uncertainty. Only evidence matching both that
  context and candidate action may adjust predicted benefit; legacy same-action
  evidence can veto but cannot manufacture positive value. A rejected candidate
  becomes direct or grounded-direct without another conductor call while
  preserving its selected model, tools, vision, risk, retrieval, and memory.
- **Pro** uses the same decision contract with a maximum requested parallelism
  of three and a larger workflow budget. It passes the same Router v2 contract
  with Pro's cost/latency policy; a low-value Pro workflow also downshifts
  without a repair call.
- Runtime-derived tool and image-input requirements remain hard postconditions
  on the decision and selected model. Effect authority is independent and
  tri-state: explicit no-change language forbids effects, an explicit effect
  requires them, and otherwise the existing permission-gated effect path remains
  allowed. Neither calibration nor a degraded fallback can weaken these
  constraints.
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
   decision; Auto and Pro request a conductor decision. All three paths receive
   a bounded `cindx.causal-route.v2` receipt after schema, capability, and final
   execution-constraint validation. Its pre-decision identity binds digests of
   the actual objective, recent context, prompt profile, requirements, model
   pool, and budget; action identity binds the complete executable route policy.
   The receipt records candidate, selected and counterfactual actions,
   conservative value components, capability provenance, reason, and operation
   counts without model chain-of-thought. Full receipts
   live only on the decision event; run context carries stable join keys and a
   digest. Both paths fail clearly when the selected configured model cannot
   satisfy required tools or vision.
   Compound execution instructions retain filenames and URLs while detecting
   later mutation steps. Local report fields such as `sources` inherit explicit
   workspace provenance instead of creating a web obligation; an objective
   that explicitly combines workspace and web evidence still keeps both scopes.
6. Durable memory recall and workspace retrieval are prepared without mutating
   canonical conversation history. Independent retrieval channels may execute
   in parallel; graph walk expands from selected seeds.
7. A workflow decision can run a bounded task graph and inject its grounded
   handoff into the interactive loop. Contributions must represent different
   work after case, punctuation, and whitespace normalization, while preserving
   word order so directionally different assignments remain distinct. Model
   reuse or diversity is selected dynamically from capability fit and supported
   evidence. `ReadOnlyEvidence` exposes only substantive statically read-only
   tools; `ReadOnlyExploration` may additionally expose bounded discovery tools;
   `None` exposes no tools. Analysis and verification follow their validated
   policy, while synthesis is always tool-free. Tool effects remain exclusive to the
   foreground executor and its normal permission path. Only successful,
   substantive, current-epoch receipts from the current collaboration enter a
   downstream step, and their original request must satisfy the active target
   anchor. A verification step becomes `Passed` only from a typed receipt that
   reviews every input and cites the corresponding admitted evidence; prose-only
   verification is `Inconclusive`. A direct decision skips collaboration.
8. `agent-application` owns the only run/reprepare driver. Each prepared epoch
   uses `AgentKernel` for Actor model turns, admitted tool batches, typed
   Verifier observations, contract checks, and terminal delivery. Tool exposure is focused by the
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
   native success. A tools-disabled Finalizer owns terminal-reserve delivery; it
   does not advance Actor turns or consume Actor tool or step budget. Empty,
   invalid, unavailable, or tool-calling Finalizer output falls back to the exact
   precomputed grounded candidate for the active epoch, or fails closed without
   re-entering the Actor. A later unverified synthesis cannot replace a stronger
   verified result. The stream completion event belongs to the same request
   stream that delivered the selected text.
10. The completion transaction persists the result, artifacts, lifecycle state,
   learning evidence, and cleanup under one physical-run and steer-epoch terminal
   identity. Success and failure replay the existing terminal event; a new
   transaction must create exactly one matching terminal event or roll back.
   Semantic memory refresh and prompt evolution are background work and run only
   after a newly inserted successful terminal.

Within an active epoch, `PreparedTaskState` is the typed source for the effective
objective, steer/contract epochs, and completion intent; `AgentTaskContract`
owns the resulting obligations and evidence. Runtime checkpoints use the
versioned `cindx.agent.task-state.v2` wire, while an explicit v1 decoder retains
existing sessions. The v2 checkpoint stores objective fingerprints and typed
intent, not raw objectives or target anchors. Permission and restart recovery
rebuild runtime-only anchors from the effective objective and reject mismatched
lineage instead of silently applying state to another task.

Every Actor and Finalizer request receives at most one protected, transient
`cindx.agent.cognitive-state.v1` overlay. `agent-runtime` derives it from the
prepared task identity and authoritative task-contract outcome ledger. The
bounded projection contains the current steer/contract epochs, typed focus and
permitted action alternatives, bounded typed verification targets, pending or
blocked obligations, postconditions, and evidence sequence references; it
contains no transcript text, raw tool input/output, or model reasoning. It is
advisory request context, not persisted task state or a second source of
completion, permission, or budget authority. If including this overlay would
violate a hard context invariant, the kernel reprojects once without only the
cognitive overlay; required trust policy, grounding evidence, and the current
request never yield to advisory state.

The adaptive loop cursor observes only trusted `cindx.tool-observation.v2`
results. Repeated exact actions with identical complete typed outcomes receive
one bounded replan before terminal delivery of the strongest available result;
a new steer, changed prepared objective, Goal Delta, changed outcome, or
incomplete evidence resets or advances the cursor instead of counting false
no-progress. The cursor stores only bounded hashes and counters. It is reset on
cold task-state recovery, while the persisted prepared task and task contract
remain authoritative and regenerate the cognitive overlay.

Provider and tool activity still drives the existing liveness watchdog, while
generic checkpoints remain diagnostic. Neither extends a run segment: only an
accepted Goal Delta increments budget credit. This keeps slow valid I/O from
being mistaken for semantic progress and prevents busy but irrelevant tool
loops from unlocking the maximum Auto or Pro budget.

## Workspace File Tools

- `file.read` keeps its raw text behavior and emits a v2 structured result with
  a page SHA-256. A complete first page also carries the full-file SHA-256;
  `include_sha256` can request it for a bounded file up to 8 MiB. `file.read_many`
  still accepts legacy string paths and raw sections, while also supporting
  per-file offsets, status, hashes, and continuations. Any child failure is an
  explicit partial failure rather than a complete batch success.
- `file.list` and `file.search` provide stable, bounded, snapshot-bound pages.
  Listing supports entry-name globs; search supports literal or regular-expression
  matching, case control, path globs, context, coverage, and byte-offset resume.
  Snapshot or option changes invalidate a cursor instead of silently mixing pages.
- `file.patch` is the exact-edit path for an existing UTF-8 file up to 8 MiB. It
  requires the complete base SHA-256 plus either an exact byte range and expected
  text or one unique anchor. The tool uses a target lock, final identity/hash
  recheck, atomic same-directory publication, permission preservation, a typed
  before/after/diff receipt, and a best-effort immutable output snapshot. Its
  session permission is path-bound. `file.write` remains the compatible complete
  overwrite path. Interrupted patch recovery confirms success only when the
  canonical workspace file still has the recorded after-SHA; otherwise it fails
  closed without repeating the write.

## Managed Process Tools

- `process.start` reserves an opaque, owner-bound process handle without running
  the command before its start receipt can be persisted. The first
  `process.poll` or approved `process.input` activates it; `shell.run` remains the
  compatible foreground fallback.
- Poll returns bounded stdout/stderr cursor pages and explicit pending, running,
  terminal, exit code, signal, timeout, cancellation, CPU/output-limit,
  completeness, and truncation facts. Nonzero child exit is a typed child
  outcome rather than a failed transport. `process.poll` declares idempotent
  effect semantics. A successful typed but incomplete poll can continue only by
  repeating the exact scope, tool, and input; this exempts that invocation from
  repeated-action classification but still consumes the ordinary tool-call
  budget and grants no new permission or effect authority.
- Active sessions are limited to two per physical run owner and four for the
  application. Wall, CPU, combined output, poll, and input budgets are hard
  bounded. Cancel, steer, run completion, session removal, explicit terminate,
  process limits, and confirmed app exit stop and reap the process group; old
  handles are not reattached after an app restart.
- Start authorization remains bound to the exact command and cwd. Interactive
  input always requires a payload-bound one-shot approval. Process observations
  do not grant workspace verification merely because a command name contains
  `test`, `check`, or `build`.

## Data, Memory, and Retrieval

- SQLite is the durable product state. Startup fails closed when the persistent
  store cannot be opened; the application does not silently continue with an
  in-memory substitute.
- Durable memory is produced from completed or explicitly eligible run
  evidence. The v7 ledger stores a bounded, typed utility history keyed by
  project, session, logical run, steer epoch, and memory identity. Ordinary
  co-occurrence, answer overlap, verified outcomes, corrections, and
  counterexamples remain `Unknown`; only a matched evaluation receipt can mark
  memory `Helpful` or `Harmful`.
- Production completion records an immutable recall-to-terminal attribution
  source without synchronously rewriting the ledger. Replay validates the
  exact recall and terminal events, their digests, and logical-run lineage
  before atomically applying the bounded batch. Recovery attempts may join one
  logical run, while their physical attempt identities remain auditable.
- Recall combines lexical and semantic evidence with trust, typed matched
  utility, deduplication, supersession, current-request conflict suppression,
  and session-diversity controls. Legacy lexical use counters remain readable
  but no longer affect ranking. Semantic curation is reserved for workflow,
  workspace-evidence, or durable-effect runs; direct self-contained text runs
  use deterministic projection.
- Workspace knowledge is separate from memory. The current retrieval adapter
  supports file indexing, provider embeddings, local file persistence, and a
  production-enabled LanceDB store.
- Requested retrieval can combine semantic search, file search, graph-direct
  lookup, and graph walk. Results retain source provenance.
- A passing deterministic memory benchmark proves the frozen recall contract;
  it does not prove that every live agent run requests and uses the right
  memory. The separate 18-cell memory-on/off contract proves only measurement
  integrity until an exact-revision provider run is published.

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
frozen learned artifact was supplied to the current matrix, so learned-profile
and distillation are `NOT_EXERCISED` and no outcome can be attributed to GEPA,
transfer, or self-distillation. Deterministic tests prove the transfer boundary,
evidence isolation, promotion gates, snapshot lineage, canonical event projection,
project-isolated scheduling, mutation anti-memorization boundary, paired
high-information reflection selection, and absolute holdout non-regression
gates, but there is still no provider-backed matched evidence that an identified
evolved profile improves external product quality.

A targeted [Memory-effect V2 provider matrix](evaluations/CINDX_AGENT_MEMORY_EFFECT_V2_0.2.19_2026-08-06.md)
on source commit `cd647030092971c84b7fc4dc6274285bec12b528` retained all
18 cells and classified all 9 matched pairs as evaluable. Memory-on passed 9/9
cells; memory-off passed the 3/3 irrelevant-memory controls but none of the 6
memory-required cells. Both required cases therefore improved in all 6 pairs,
while all recalled-decoy controls passed in both arms, with zero safety
violations, evidence errors, confounds, or setup failures. This is
provider-backed causal evidence for durable-memory utility under the frozen
matched direct harness only. It does not evaluate native Auto routing, workflow
collaboration, GEPA, or distillation and does not change the broader V5
orchestration conclusion.

The preceding [Memory-effect V1 provider matrix](evaluations/CINDX_AGENT_MEMORY_EFFECT_V1_0.2.19_2026-08-06.md)
on source commit `1abdd6849e4a99c7b30f7f7aa2123efbbdb0d51a` retained all
18 cells, but one required memory-off cell invoked `skill.search` outside the
frozen evaluation allowlist. The fail-closed decision is therefore
`INVALID_EVIDENCE`, with 8/9 pairs evaluable. Five evaluable required pairs
descriptively favored memory-on and all three recalled-decoy controls passed,
but the invalid pair prevents a causal memory-uplift claim. The run does not
evaluate native Auto routing, workflow collaboration, GEPA, or distillation and
does not change the broader V5 orchestration conclusion.

The current source defines Agent Real-World V5 as the active execution contract.
It renames the no-tools Direct ceiling to `oracle_reference` and introduces an
iso-budget `grounded_direct` product baseline. Grounded Direct uses Auto's
shipping AgentKernel, model routing, tools, retrieval, memory, permissions, and
external postcondition verifier, but constrains the routed decision to direct
execution; internal independent-worker verification is therefore normalized to
self-check. Auto remains the adaptive candidate and Pro remains descriptive
because its native budget differs. Mixed Auto routing is evaluated as separate
matched adaptive-direct and workflow subsets. Workflow uplift requires an
observed workflow profile; learned-profile and distillation uplift require the
actually executed exact stable parent. No observed mechanism difference can be
reported only as `NEUTRAL`, while missing evidence is `NOT_EXERCISED` or
`INVALID_EVIDENCE`. Failed, timed-out, denied, and unclassified early runs remain
in their matched denominator. Deterministic tests verify this measurement and
claim machinery only; they do not establish intelligence uplift.

V5 preserves V4's six frozen cases, three repeats, isolated browser fixture,
typed current-logical-run tool receipts, exact target and artifact binding, and
visible permission-denial requirement. The completed V5 `0.2.11` matrix is the
current provider-backed baseline. V4 `0.2.9` remains historical evidence for
its exact revision.

## Current Evidence Boundary

The latest provider-backed Agent baseline is
[Cindx Agent Real-World V5 0.2.11](evaluations/CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.md),
captured on source commit `3765d23042dcafaeb721accc0460f149e1ea5ade`.
It retained all 72 position-balanced cells across Oracle Reference, Grounded
Direct, Auto, and Pro, with complete provider and strategy evidence, zero setup
failures, zero timeouts, and zero safety violations. The publication contract
marks it `VALID_BASELINE`.

- All four treatments scored `100.0%` quality. Completion was `100.0%`,
  `83.3%`, `83.3%`, and `88.9%`, respectively.
- Against iso-budget Grounded Direct, Auto had zero quality and completion
  delta, `5,989 ms` lower median latency, and `1.8%` more total tokens. Because
  no matched pair improved quality, adaptive-direct is `NEUTRAL`.
- No Auto run exercised workflow collaboration, so workflow is
  `NOT_EXERCISED`; it is not silently merged into the adaptive-direct result.
- Pro completed one more matched run and had `4,722 ms` lower median latency
  than Grounded Direct, but is descriptive because its native budget differs.
- Six Grounded Direct and Auto browser runs failed terminal completion
  symmetrically. Pro retained one browser failure and one long-horizon failure;
  all eight failures remain in the denominator.
- All product treatments completed all denied-mutation runs with the required
  visible permission-denied explanation.
- No frozen learned profile was executed, so learned-profile and Pro-to-Auto
  distillation are `NOT_EXERCISED`.

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
- Auto preserves the Grounded Direct quality and completion baseline while
  reducing median latency, but it shows no matched quality uplift.
- The current source now has a deterministic, observable Auto
  value-of-computation admission mechanism and bounded single-conductor
  no-progress handling, plus typed denial and bounded same-epoch replan
  contracts. V5 measured adaptive-direct but did not exercise workflow or
  isolate those controls' causal contribution.
- Pro's completion signal is descriptive rather than causal; browser terminal
  completion remains the clearest measured weakness.
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
