# Current Product Baseline

Current application version: `0.1.92`

Last code-fact review: `2026-08-03`

This document describes the current source tree. Evaluation reports describe
only the revision recorded in each report.

## Execution Modes

- **Fast** bypasses the conductor and runs one configured model through the
  shared interactive agent loop.
- **Auto** asks the configured conductor for a typed `AgentRunDecision`, with a
  maximum requested parallelism of two. The decision may remain direct or
  select a bounded workflow. Runtime-derived tool and image-input requirements
  are hard postconditions on both the decision and selected model; the
  conductor cannot downgrade them.
- **Pro** uses the same decision contract with a maximum requested parallelism
  of three and a larger workflow budget.
- An invalid conductor response receives bounded repair. Exhausted conductor
  attempts produce an explicit capability-compatible degraded fallback rather
  than an unvalidated workflow. Strong matched direct-anchor evidence
  calibrates an otherwise admissible workflow to direct execution without
  spending repair or alternate-conductor calls.

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
   decision; Auto and Pro request a conductor decision. Both paths fail clearly
   when the selected configured model cannot satisfy required tools or vision.
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
active Auto profile fingerprint. Auto transfer reflections receive reserved
capacity in Pro mutation input, so they cannot be displaced by a full ordinary
reflection batch.

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

Canary traffic is capped at 50 percent. A challenger can become the stable
profile only by first producing a valid immutable frozen snapshot; missing or
regressed ordinary/transfer lineage rolls the canary back and preserves the
previous stable profile.

Reflective mutations may learn a general strategy from redacted trajectories,
but a deterministic validator rejects candidate directives that contain case,
run, candidate, or participant-model identifiers, or copy substantial spans
from prompts, outputs, tool traces, verifier details, or actionable feedback.
Rejected and repaired mutations pass through the same validator.

Learned genomes, datasets, observations, and rollout state are isolated by
project. A reflective mutation produced from one project's evidence cannot
enter another project's candidate population, evaluation, or rollout.

The latest provider-backed baseline recorded `auto_gepa=false` and
`pro_gepa=false`; it predates the Auto-to-Pro transfer path. Deterministic tests
prove the transfer boundary, evidence isolation, promotion gates, snapshot
lineage, canonical event projection, project-isolated scheduling, mutation
anti-memorization boundary, and mutation-input allocation. There is still no
provider-backed matched evidence that an evolved profile improves external
product quality.

## Current Evidence Boundary

The latest provider-backed collection attempt is
[Cindx Agent Real-World V2 0.1.82](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md).
It executed all 72 cells on source commit `6b2ee39`, but four Auto/Pro
RAG-memory cells failed during setup. The publication contract therefore marks
it `INVALID_BASELINE`; it cannot support capability promotion, treatment
comparison, or an intelligence-uplift claim. It is retained because the setup
failures, browser tail latency, and missing Fast permission-denial evidence are
actionable product evidence.

The latest complete provider-backed Agent baseline remains
[Cindx Agent Real-World V1 0.1.82](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md).
It ran six frozen tasks across Direct, Fast, Auto, and Pro with three matched
repeats on source commit `4d43e77`, for 72 observations:

- Direct scored `88.9%`, but is a no-tools answer ceiling with evidence inline;
- Fast, Auto, and Pro each scored `72.2%` on the complete matrix;
- relative to Fast, Auto had equal quality, `11.1` points lower completion, and
  `14.3 s` higher paired median latency;
- relative to Fast, Pro had equal quality, `5.6` points lower completion, and
  `18.0 s` higher paired median latency;
- all product treatments passed file, long-horizon, RAG/memory, and denied
  mutation cases; browser evidence was the dominant failure domain;
- no denied-mutation safety violation occurred.

Seven coding failures are conservative instrumentation failures: file and test
checks passed, but the frozen suite did not accept `file.read_many` as read
evidence. The score remains published unchanged. This defect must be corrected
in a new suite version before collecting the next matrix; it must not be fixed
retroactively to improve this result.

The latest matched GPQA diagnostic is version `0.1.78`: Direct scored `10/12`,
while Auto and Pro each scored `8/12` under the budget. The sample is too small
for broad conclusions, but it does not demonstrate orchestration uplift.

Therefore the current claim is:

- The control plane, local memory contract, permission boundary, and
  deterministic quality gates have substantial automated coverage.
- Auto can gather the required evidence in this small matrix, but Auto and Pro
  are not proven to outperform the direct path in quality, latency, or tokens.
- Auto and Pro are not proven to outperform Fast; both matched Fast quality and
  regressed in completion and latency in the current matrix.
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
