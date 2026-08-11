# Current Architecture

This document records ownership and dependency direction in the current tree.
It deliberately calls out remaining coupling instead of treating file count as
modularity.

## System Boundary

```text
React UI
  -> typed Tauri adapter
  -> desktop commands and composition root
  -> application driver
  -> execution plan + optional workflow
  -> agent kernel
  -> model observations and permission-gated tools
  -> SQLite events, state, artifacts, memory, and retrieval indexes
```

Cloud providers reason and stream model output. The local app owns run identity,
context assembly, tool exposure, permission decisions, effects, persistence,
recovery, retrieval, memory, and the delivered result.

## Runtime Ontology and Trace Attribution

Current Agent execution uses separate dimensions instead of treating legacy
role names as one ontology:

- **Actor:** Owner, Specialist, or Independent Verifier.
- **Stage:** plan, evidence, act, verify, or finalize.
- **Model profile:** Primary, Reasoning, Verifier, or Utility.
- **Service:** Conductor planning and background learning utilities are
  services and are never recorded as Actors.

The Owner alone owns permission-gated effects and final user delivery.
Specialists contribute bounded internal plans or evidence. Independent
Verifiers evaluate an artifact without effect authority. Utility-profile model
calls support bounded background preparation but do not participate in the
production Workflow decision or own final delivery.
Background prompt mutation is recorded as a learning utility service, not a
production Specialist.

These fields are an event-local sidecar on current Agent model request events.
Started and finished events reuse the same explicit attribution selected at the
call site. Existing `role`, `stage`, model configuration slots, summaries, and
event counts are preserved for compatibility; legacy events without the schema
remain legacy and are not reclassified from display strings.

## Crate Ownership

| Crate | Current owner responsibility |
| --- | --- |
| `agent-core` | Transport-free IDs, messages, events, permissions, tool/model contracts, and shared schemas |
| `agent-runtime` | Kernel, run control, context governor, task contract, adaptive cursor, model-turn and tool-runtime semantics |
| `agent-application` | The run/reprepare driver, strategy/terminal lifecycle, and portable externally verified outcome contract |
| `agent-harness` | Active-run and exclusive-work registries; no model policy |
| `orchestrator` | Conductor execution contracts, run decisions, workflows, task graph, verification, routing evidence, and prompt-evolution policy |
| `orchestrator-eval` | Non-default evaluation and Fugu comparison contracts |
| `agent-memory` | Memory records, retention, recall, utility attribution, and deterministic curation contracts |
| `agent-rag` | Workspace indexing, file adapter, semantic retrieval, and vector-store integration |
| `agent-graph` | Graph extraction, direct graph retrieval, and graph walk |
| `agent-storage` | SQLite schema and durable event/state repositories |
| `model-provider` | HTTP/WebSocket provider transport and streaming adapters |
| `tools` | Tool schemas and portable tool implementations |
| `agent-mcp` | MCP transport, catalog, and invocation adapter |
| `agent-skills` | Skill discovery, trust, loading, and built-in skill assets |

`agent-runtime` consumes model contracts from `agent-core`; it must not depend
on the provider transport crate. The structure gate enforces that boundary.

## Desktop Composition Root

`apps/desktop/src-tauri` owns integration that cannot be portable without
changing product behavior:

- Tauri commands and UI event emission.
- Provider configuration and concrete transport construction.
- SQLite-backed application state and projections.
- Permission UI, platform commands, sidecars, and effect execution.
- Session/project lifecycle, attachments, outputs, schedules, voice, and native
  open/reveal operations.
- Foreground orchestration glue, workflow checkpoints, memory workers, and
  prompt-evolution workers.

This layer is not yet a thin adapter. `src/lib.rs` declares and imports a large
set of sibling modules, and several workflows still cross desktop services by
shared composition state. Future refactors should move a complete owner and its
tests behind a narrow interface; merely creating more sibling files would not
reduce coupling.

## Interactive Run Flow

### 1. Admission and identity

The task command validates provider, session, workspace, attachments, and model
capabilities, then persists the user message and start state. Logical run ID,
physical attempt ID, and steer epoch define replay and recovery boundaries.

`agent-harness` admits only one active owner for the same run/work key.
`AgentRunControl` applies cancellation, deadlines, stage budgets, and steer.

### 2. Context and execution plan

The desktop adapter projects bounded history and an authoritative effective
objective. `agent-runtime` compiles invariant sources, complete tool rounds,
optional evidence, and a bounded cognitive overlay.

Fast creates a fixed direct candidate. Auto and Pro request one typed Conductor
candidate. `orchestrator` validates route, task class, model capabilities,
effects, retrieval, graph, and verification under a bounded execution contract.
A Workflow candidate must include its executable branch graph in the same
decision response; a second planner does not silently replace it.

The persisted execution plan binds candidate, final action, authority, hard
constraints, context and profile fingerprints, and optional workflow identity.
A compatibility router may produce shadow evidence, but it does not override a
valid Conductor action.

The selected plan also produces a strategy receipt bound to task, session,
physical run, steer epoch, and semantic plan digest. Run control serializes a
preparation checkpoint without entering execution, while one immediate SQLite
transaction writes both the router event and selected decision. A cancellation
or steer that wins first prevents that stale decision from being committed.

### 3. Retrieval and memory

Workspace retrieval and durable memory are independent inputs:

- `agent-rag` searches indexed file chunks and vector storage.
- `agent-graph` provides direct relation lookup and bounded graph walk.
- `agent-memory` ranks project/session memories under trust, utility,
  supersession, conflict, retention, and diversity constraints.

The desktop adapter coordinates provider embeddings, background queues, and
state persistence. Retrieved data retains source provenance and does not mutate
canonical chat history.

### 4. Optional workflow and task graph

`orchestrator` materializes a fixed owner-execution graph only for a validated
Workflow decision: one dependency-free Analysis or Evidence Specialist,
optionally one tool-free and model-distinct Independent Verifier, and one final
compatibility sink. The Specialist receives only its admitted read-only catalog.
Side effects remain with the foreground Owner.

The final sink is not a model Actor. The runtime completes it deterministically
from the checkpoint after its dependency succeeds, preserving step identity,
input and output digests, evidence lineage, resume identity, and typed
verification receipts. The resulting packet is untrusted internal guidance;
the Owner independently reconciles it and owns final delivery. Missing,
malformed, failed, or required-but-unsatisfied verification rejects the handoff
and falls through to the direct Owner path. Checkpoint loading separates an
executable resume from an untrusted Owner-only handoff: retired models and a
legacy final sink that already consumed a model attempt are never scheduled,
while completed outputs remain available as partial context. The production
graph has no direct anchor competition, reviewer tournament, model synthesis,
uplift repair, or second Conductor planner.

### 5. Kernel and effects

`agent-application` is the outer driver. `AgentKernel` owns the prepared epoch:

1. select the next model/tool action,
2. execute admitted model turns or tool batches,
3. record typed observations,
4. update task and progress contracts,
5. suspend for permission or steer when required,
6. reprepare or select terminal delivery.

Tool exposure and permission are separate. The desktop tool runtime constructs
the exact permission request, checks a matching session capability or asks the
user, executes the effect, and commits the canonical outcome. Effects use the
physical attempt identity for replay and idempotency.

### 6. Completion and recovery

Terminal selection cannot convert an unmet obligation into verified success. A
tools-disabled finalizer may produce the visible response without consuming an
actor turn; an invalid finalizer falls back to the exact eligible grounded
candidate rather than restarting the actor.

The completion transaction persists terminal event, result, artifacts,
lifecycle, learning evidence, and cleanup under one attempt/epoch identity.
Its terminal identity keeps the existing exactly-once key and additionally
validates the matching strategy receipt before a new terminal write. Post-start
preparation failures arbitrate terminal persistence with cancellation and steer
under the same run-control lock; a winning steer replays preparation.
Each new physical attempt keeps session admission and run control registered
until its durable start transaction commits. Initial start/message writes and
continuation recovery-claim/start/replay writes are atomic; pending steers stay
queued for the resumed attempt instead of invalidating its start.
Startup recovery uses durable events and checkpoints and restores the matching
receipt; a run that ended before selection records `not_selected`. Pause and
permission wait are intentionally nonterminal. Permission recovery and retry
preserve logical lineage while keeping physical effects auditable.

`agent-application` also owns
`cindx.agent.externally-verified-outcome.v1`. Evaluation adapters may derive it
only from validated strategy and terminal identity, actual Actor exposure,
external postcondition and preservation receipts, and complete resource
accounting. Missing or tampered provenance is censored; valid unsafe or
preservation-breaking outcomes remain zero-score evidence. The receipt is
shadow-only and has no production routing, prompt, memory, permission, or
serving consumer.

The same crate owns the portable collaboration-learning contracts, separate
from production prompt evolution. A structured policy keeps the Goal 2 graph
and Owner authority fixed while bounding context, verification, and same-lane
repair choices. Its companion exercise receipt is projected from lifecycle
events rather than supplied by the candidate: it binds policy assignment,
semantic plan, exact pre-dispatch request receipts, per-lane attempts, derived
stop reason, and the externally verified outcome digest. The optional
`collaboration-learning-offline` feature adds canonical bounded import and
hash-chain replay into the same train/holdout evidence contract. Review may
authorize only a narrow offline validation record; there is no conversion to
production learning evidence, prompt genomes, snapshots, routing, canary, or
serving.

The desktop `realworld-eval` adapter is the only runtime producer. Its explicit
successor-only entry installs a matched-arm policy, commits Direct assignment
with the durable strategy decision or Workflow assignment with the materialized
plan, applies the assigned context budget and fail-fast attempt bound, and
commits only the SHA-256 identity and size of the exact encoded request before
dispatch. Raw request contents are never added to the learning receipt. The
adapter retains the run event slice outside the frozen report schema and derives
the comparison binding from the actual pair plus a pre-frozen source/cohort
authority.

The successor evaluation control plane has one tracked protocol authority:
`benchmarks/agent/collaboration-successor-protocol-v1.json`, bound to the
tracked three-case suite by digest. The manifest fixes three pairs / six runs,
the 5,000-to-7,500-bps context-only candidate, the existing conservative
per-run budget, the six-run campaign aggregate, and terminal freeze rules. The
desktop preflight validates that authority, materializes the exact cases, and
binds a clean Git HEAD/tree plus redacted provider and complete model-catalog
digests into a new private receipt outside the repository. It performs zero
provider calls and cannot authorize execution. Goal 3E adds two separate,
feature-gated control-plane binaries around that receipt. Authorization is a
provider-free, 15-minute, private one-shot capability bound to the current
preflight, clean source, provider/model configuration, exact execute binary,
fixed cells and budgets, and a new external output root. Execution revalidates
and atomically consumes it, then persists campaign, cell, and arm reservations
before the corresponding provider action. The private lifecycle journal is the
recovery authority; any ambiguous, interrupted, tampered, expired, or reused
state terminates frozen/censored and cannot resume a started physical run.

The fixed controller admits the baseline before the candidate and the
candidate before holdout, then exposes only ready-for-independent-review,
frozen, or censored terminal state. It has no production serving or promotion
consumer. The Goal 3E instance consumed its one-shot authorization, reserved
the first baseline Direct arm, and closed `CENSORED` before a valid observation
after the product run terminated before selection with zero selected decisions.
The terminal producer correctly persisted the explicit pre-decision
`not_selected` state, with no treatment or Owner execution, but the
selected-only external-outcome projector misclassified that legal state as a
malformed receipt. This confirms the lifecycle stopped fail-closed; it is not a
quality result and does not permit recovery or retry of that protocol.
The current projector now distinguishes selected, explicit pre-decision
not-selected, absent, and malformed receipt states before outcome construction;
a legal not-selected terminal remains a censor and can never become an outcome.

## Persistence and Background Work

SQLite stores projects, sessions, events, permissions, checkpoints, schedules,
memory, prompt evolution, and projections. Persistent-state failure aborts
startup. Offline collaboration evidence deliberately does not add a production
SQLite table: an explicitly selected evaluation harness uses private 0600,
content-addressed genesis and entry files plus an atomic manifest. Recovery
adopts only one valid successor; missing, tampered, or forked state fails
closed, and an unfinished capture becomes a censor rather than a provider
retry.

Background services are bounded and must not block the healthy foreground path:

- Semantic memory curation and vector refresh.
- Prompt evidence projection, mutation/evaluation, rollout, canary, transfer,
  and distillation work.
- Schedule dispatch and sidecar health work.

Canonical events remain authoritative. Derived serving snapshots and caches can
be rebuilt; a cache publication failure cannot rewrite the scientific outcome.

## Prompt Evolution Boundary

Prompt evolution observes completed, redacted evidence. `orchestrator` owns
candidate schemas, comparison, promotion gates, and rollout policy; desktop
workers own provider calls, durable campaign state, and publication.

Goal 3B collaboration candidates are not prompt-evolution candidates. They are
shadow/offline admission records owned by `agent-application`; keeping that
dependency direction prevents the production prompt selector from consuming
them implicitly.

A deployed profile is selected once per logical run, is stable across retries,
and cannot alter permissions, tools, or budgets. Fast is seed-only. Missing or
invalid deployment state falls back to seed. Evaluation harnesses are not
serving authority.

## Dependency Rules

- Portable crates must not import Tauri or desktop state.
- `agent-runtime` must remain provider-transport independent.
- `orchestrator-eval` is excluded from default workspace members and must not
  become a shipping dependency.
- Tools declare effects; the desktop permission path grants execution.
- Memory and workspace knowledge remain separate stores and provenance domains.
- UI projections may cache derived state but cannot become durable truth.
- Evaluation code cannot promote a profile without production admission gates.

These rules are checked by `scripts/check-desktop-structure.mjs`, Rust tests,
and the quality profiles described in [DEVELOPMENT.md](DEVELOPMENT.md).
