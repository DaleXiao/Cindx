# Current Product Baseline

Current application version: `0.1.81`

Last code-fact review: `2026-08-01`

This document describes the current source tree. Evaluation reports describe
only the revision recorded in each report.

## Execution Modes

- **Fast** bypasses the conductor and runs one configured model through the
  shared interactive agent loop.
- **Auto** asks the configured conductor for a typed `AgentRunDecision`, with a
  maximum requested parallelism of two. The decision may remain direct or
  select a bounded workflow.
- **Pro** uses the same decision contract with a maximum requested parallelism
  of three and a larger workflow budget.
- An invalid conductor response receives bounded repair. Exhausted conductor
  attempts produce an explicit degraded fallback rather than an unvalidated
  workflow.

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
5. Fast chooses a direct decision. Auto and Pro request a validated conductor
   decision.
6. Durable memory recall and workspace retrieval are prepared without mutating
   canonical conversation history. Independent retrieval channels may execute
   in parallel; graph walk expands from selected seeds.
7. A workflow decision can run a bounded task graph and inject its grounded
   handoff into the interactive loop. Contributions must represent different
   work, while model reuse or diversity is selected dynamically from capability
   fit and supported evidence. A direct decision skips collaboration.
8. `agent-application` owns the only run/reprepare driver. Each prepared epoch
   uses `AgentKernel` for model turns, admitted tool batches, observations,
   contract checks, and terminal delivery. A committed steer returns through
   the same driver before another epoch can begin.
9. Terminal delivery selects the strongest verified deliverable known to the
   run; a later unverified synthesis cannot replace it. The stream completion
   event belongs to the same request stream that delivered the selected text.
10. The completion transaction persists the result, artifacts, lifecycle state,
   learning evidence, and cleanup. Semantic memory refresh and prompt evolution
   are background work.

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

Learned genomes, datasets, observations, and rollout state are isolated by
project. A reflective mutation produced from one project's evidence cannot
enter another project's candidate population, evaluation, or rollout.

The current live baseline recorded `auto_gepa=false` and `pro_gepa=false`.
There is no current evidence that an evolved profile improves external product
quality.

## Current Evidence Boundary

The latest provider-backed Agent diagnostic is
[Cindx Agent Real-World Lite Goal 6 0.1.80](evaluations/CINDX_AGENT_REALWORLD_LITE_G6_0.1.80_2026-08-01.md).
It ran the same three deterministic read-only workspace tasks across a direct
single-model ceiling, Fast, Auto, and Pro on source commit `e40960c`:

- the direct ceiling and Auto each scored `1.000` across `3/3` deliveries;
- Fast scored `0.417`; it remained incomplete on multi-file synthesis and
  contradiction resolution;
- Pro scored `0.333`; all three answers omitted required evidence;
- Auto used `55.76x` the direct tokens and `7.83x` its median latency; Pro used
  `73.43x` the tokens and `12.83x` the median latency;
- all `12/12` runs delivered without provider errors or safety violations;
- neither Auto nor Pro used a frozen GEPA profile.

This is a small directional diagnostic, not a statistically powered ranking.
The direct treatment received evidence inline, so it is an answer-quality
ceiling rather than an equal tool-using baseline. The result is still enough to
reject a current collaboration-uplift claim: Auto recovered answer completeness
but not efficiency, while Pro regressed in both quality and cost.

The latest matched GPQA diagnostic is version `0.1.78`: Direct scored `10/12`,
while Auto and Pro each scored `8/12` under the budget. The sample is too small
for broad conclusions, but it does not demonstrate orchestration uplift.

Therefore the current claim is:

- The control plane, local memory contract, permission boundary, and
  deterministic quality gates have substantial automated coverage.
- Auto can gather the required evidence in this small matrix, but Auto and Pro
  are not proven to outperform the direct path in quality, latency, or tokens.
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
- Provider-backed real-world coverage is small and read-only. Code editing,
  browser/computer tasks, interruption/resume, steering, and cross-session
  memory still need a matched repeated external-effect suite.

These are current constraints, not roadmap promises. A later change may remove
them only with code and verification evidence in the same revision.
