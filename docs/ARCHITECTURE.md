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

The provider configuration keeps its legacy storage keys, but their current
product semantics are explicit:

- `model` is the Fast-preferred compatibility execution model.
- `executor_model` is the Primary profile.
- `planner_model` is the Reasoning profile.
- `reviewer_model` is the Verifier profile.
- `summarizer_model` is the Utility profile.
- `conductor_model` is a planning-service override, not an Actor profile.

These slots do not assign permanent Actors to models. A concrete model may be
configured in more than one slot, but each call is admitted by the slot needed
for that lane: Primary and Reasoning models may execute the direct or Specialist
path, a Verifier model may enter only the verification lane, and a model
configured only as Utility cannot enter production routing, workflow execution,
or Conductor fallback. The compatibility model can be copied to all four
profiles only through an explicit Settings action; it does not overwrite the
planning-service override.

These fields are an event-local sidecar on current Agent model request events.
Started and finished events reuse the same explicit attribution selected at the
call site. Existing `role`, `stage`, persisted model keys, summaries, and event
counts are preserved for compatibility; legacy events without the schema remain
legacy and are not reclassified from display strings.

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

Delivery Verification remains a separate `realworld-eval` authority. V1, v2,
and v3 are consumed lineages: all nine of their preflight, authorization, and
execute entrypoints return a consumed-protocol error before reading arguments,
environment, configuration, paths, or live control-plane state. V1 exposed the
semantic/wire digest instrumentation defect. V2 fixed that defect, completed all
eight calibration pairs once, and closed `terminal_futility`. V3 made one
provider attempt for its first calibration Reviewer call, but its journal
rejected a bound zero-byte response artifact before committing a terminal call
receipt and froze `CENSORED`. None can be resumed, relocated, or rerun.

V4 is a distinct protocol and control-plane authority over the exact same tracked
v3 seeded-defect recovery and preservation suite. Its 32 cases, order, hidden
oracle, seeded candidates, model inputs, output contracts, budgets, decision
thresholds, and no-retry rule are unchanged. The suite contains 24 seeded
defects and eight clean preservation sentinels, with 8 calibration / 24 holdout
cases and equal representation of unsupported claims, omitted obligations,
contradictions, and preservation. Every case owns a frozen seed, a model-visible
objective, ordered obligations/evidence, and an output contract that exposes
property names, JSON types, requiredness, and the additional-properties rule.
Exact semantic values remain in the hidden oracle and never enter a model
request. The model-visible request envelope deliberately retains its v3
compatibility schema so the successor does not change prepared semantic or wire
payloads for reasons unrelated to the instrumentation fix.

The control arm is the exact frozen seed. Treatment starts with a model-distinct
Reviewer over that same seed. `passed` returns the unchanged seed; only
`needs_revision` activates one Executor repair followed by one Reviewer
recheck. There is no initial Executor drafting call, second repair, retry, or
case replacement. A preservation seed passes treatment only when it remains
unchanged; an unnecessary repair is therefore a control-only loss even if the
Reviewer accepts it. This topology measures joint defect detection and repair
plus clean-input preservation. It does not estimate uplift over naturally
generated drafts.

A durably terminal provider timeout/unavailability or completed call whose
Reviewer verdict JSON is invalid remains an intention-to-treat treatment failure
for that fixed case, with no retry or replacement. V4 records an exact response
artifact even when it contains zero bytes, including its digest and zero length.
A complete tool-free response with empty content projects as
`invalid_verifier_response`; a response that fails the complete tool-free
contract closes `invalid_output` and remains structural. Internal time-budget,
request-binding, or authority failure likewise closes the campaign
inconclusive.

V4 preflight is provider-free. It binds the clean source HEAD/tree and app
version; protocol, suite, per-case and aggregate case digests; case order;
seeded-candidate, model-input, output-contract, budget, and hidden-oracle
aggregates; redacted provider/model authority; exact execute name, full-file
digest, size, and SHA-256 CodeDirectory identity; and canonical external output
authority. A valid receipt records zero provider calls,
`online_runner_frozen=true`, and `execution_authorized=false`. Authorization is
a separate provider-free step that must revalidate the canonical preflight,
current source/provider/model/credential binding, exact runner identities, new
output root, and short validity window.

Before provider construction, execute compares the frozen CodeDirectory with
macOS's kernel-backed identity for the running process and atomically creates a
no-clobber marker in the output root's parent. All authorization paths for that
canonical output authority share the marker; the output-root tombstone must
byte-match it, so relocating or deleting the output directory does not reopen
consumption. The v4 journal persists campaign and case reservation before work.
Each call then reserves stage/role/model, semantic request digest/size,
immutable prepared wire-payload digest/size, and output budget before dispatch;
the provider receives exactly the prepared non-streaming bytes, and terminal
request metadata must match the reserved wire digest.

Call receipts retain provider-failure classification, retryability, status,
latency, hashed provider identities, exact usage, and response-artifact binding.
Case receipts retain seed/control/treatment authorities, arm outcomes, initial
verdict and finding counts, repair activation, recheck verdict/counts, treatment
disposition, failure stage/code, and outcome reason. Interrupted started calls
are terminal and never resumed or retried. V4's single consumed attempt retained
one zero-byte `invalid_output` artifact, closed its first case
`structural_failure`, and terminated the campaign `inconclusive` with no matched
pair. Its canonical output authority cannot execute again. There is no Delivery
serving, learning, promotion, production-finalizer, or GEPA dependency.

The consumed marker is an intentionally local authority, not an external
anti-rollback ledger. Normal crashes, concurrent consumers, alternate
authorization paths, and output-root relocation fail closed. An actor with the
same user identity who can delete or restore all private control-plane files is
outside that local-filesystem threat boundary.

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
