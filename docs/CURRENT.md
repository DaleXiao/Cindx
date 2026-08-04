# Current Product Baseline

Current application version: `0.2.0`

Last code-fact review: `2026-08-04`

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
   use the same planning path. A committed steer returns through the same driver
   before another epoch can begin.
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

The learned genome continues to govern the evaluated workflow surface:
topology, branch shape, roles, verification, tool exposure, retry/recovery,
stopping, context policy, and bounded step budgets. Per-request route,
retrieval, and memory decisions remain owned by `AgentRunDecision`; the current
GEPA campaign does not execute that decision layer, so those fields are not
presented as learned strategy without a matched causal evaluation.

Transfer observations are appended as one mirrored pair and enter the canonical
evolution read model only when both sides are scientific, project-scoped, and
bound to the same source-attested Auto run and profile. Legacy transfer records
without this lineage fail closed. Projection version 3 rebuilds older caches
from canonical events instead of silently preserving the previous projection
that omitted transfer events. A completed Auto run schedules its project-scoped
Pro learning intent durably; the background worker dispatches and idempotently
replays that intent after interruption. Background requests are
coalesced by project and effort; activity in one project cannot replace another
project's pending campaign. Campaign wall time, tokens, and physical provider
attempts survive request-scoped action events as well as checkpoints and are
never replenished by malformed or interrupted recovery state. When repeated
objectives have multiple qualified Auto teachers, the
current stable Auto profile is selected before stale profiles, then
independently measured quality and resource use break ties.

The current tree also contains a separate controlled Pro-to-Auto distillation
track. Only the active frozen Pro champion can be a teacher, and its attestation
must replay both its ordinary Pro gate and Auto-to-Pro source lineage. The
runtime derives an Auto-bounded child from the stable Auto parent rather than
copying Pro budgets, topology, directives, or genome wholesale. Teacher and
ancestor cases or equivalent objectives cannot enter the fresh train/holdout
cohort; missing manifests, stale teachers, mixed lineage, treatment failures,
timeouts, or permission denials fail closed.

Distillation dispatch is asynchronous, project-scoped, idempotent, bounded, and
suppressed while a foreground Agent run is active. A real matched gate can
start a 10 percent Auto canary; fresh live evidence is required for the 25 and
50 percent stages and final frozen promotion. Safety, quality, latency, token,
gate, or lineage regression rolls back to the prior stable Auto profile. The
deterministic suite verifies these mechanism contracts only. No current
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

The latest provider-backed raw schema did not capture learned profile or GEPA
identities. Deterministic tests prove the transfer boundary, evidence isolation,
promotion gates, snapshot lineage, canonical event projection, project-isolated
scheduling, mutation anti-memorization boundary, paired high-information
reflection selection, and absolute holdout non-regression gates, but the current
baseline cannot attribute an outcome to GEPA, transfer, or self-distillation.
There is still no provider-backed matched evidence that an identified evolved
profile improves external product quality.

## Current Evidence Boundary

The latest provider-backed Agent baseline is
[Cindx Agent Real-World V2 0.1.98](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md),
captured on source commit `4fc736cdfd0b8eb85ffee0a9ef5dfaea4b05e469`.
It retained all 72 matched cells across Direct, Fast, Auto, and Pro, with zero
setup failures and zero safety violations. The publication contract marks it
`VALID_BASELINE`; the independent broad orchestration-uplift decision is
`NO-GO`.

- Direct, Fast, Auto, and Pro scored `100.0%`, `77.8%`, `88.9%`, and `83.3%`
  quality, with `100.0%`, `88.9%`, `66.7%`, and `61.1%` completion.
- Relative to Fast, Auto gained `11.1` quality points, lost `22.2` completion
  points, and added `24.663 s` paired median latency.
- Relative to Fast, Pro gained `5.6` quality points, lost `27.8` completion
  points, and added `41.396 s` paired median latency.
- Fast failed the RAG/memory answer check in all three repeats; Auto and Pro
  passed all three after successful setup.
- Browser evidence remained the dominant failure: Fast passed `2/3`, Auto
  `1/3`, and Pro `0/3`; Pro reached the 600-second process deadline three times.
- All product treatments safely passed the denied-mutation verifier, but Auto
  and Pro did not converge to a successful terminal runtime state after denial.

The earlier [13A repair calibration](evaluations/CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md)
remains the exact pre-matrix repair evidence. The invalid `0.1.82` V2 attempt
and complete V1 matrix remain historical reports for their own revisions.

The latest matched GPQA diagnostic is version `0.1.78`: Direct scored `10/12`,
while Auto and Pro each scored `8/12` under the budget. The sample is too small
for broad conclusions, but it does not demonstrate orchestration uplift.

Therefore the current claim is:

- The control plane, local memory contract, permission boundary, and
  deterministic quality gates have substantial automated coverage.
- Auto shows a real RAG/memory quality signal in this small matrix, but its
  overall completion and latency regress materially against Fast.
- The current source now has a deterministic, observable Auto
  value-of-computation admission mechanism and bounded single-conductor
  no-progress handling. These contracts have not yet been measured in a fresh
  provider-backed matched run, so they are not evidence of an intelligence or
  product-quality uplift.
- Pro does not provide a broad product advantage over Fast or Auto in the
  current matrix; its browser completion is the clearest measured weakness.
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
