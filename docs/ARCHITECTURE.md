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
  -> single-model execution plan
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

- **Actor:** historical vocabulary of Owner, Specialist, or Independent
  Verifier; single-model effort-tier runs act exclusively as Owner.
- **Stage:** plan, evidence, act, verify, or finalize.
- **Model profile:** Primary, Reasoning, Verifier, or Utility.
- **Service:** Background learning utilities are services and are never recorded
  as Actors. Effort-tier planning is deterministic and makes no model call, so
  it is not a stage participant at all.

The Owner alone owns permission-gated effects and final user delivery. The
retired workflow lanes used Specialists for bounded internal plans or evidence
and Independent Verifiers for effect-free artifact evaluation; those lanes no
longer execute. Utility-profile model calls support bounded background
preparation but do not participate in any production decision or own final
delivery.
Background prompt mutation is recorded as a learning utility service, not a
production Specialist.

The provider configuration keeps its legacy storage keys, but their current
product semantics are explicit:

- `model` is the Fast-preferred compatibility execution model.
- `executor_model` is the Primary profile.
- `planner_model` is the Reasoning profile.
- `reviewer_model` is the Verifier profile.
- `summarizer_model` is the Utility profile.
- `conductor_model` is a legacy persisted slot; the effort-tier planner makes
  no planning model call, so it no longer selects anything.

These slots do not assign permanent Actors to models. A concrete model may be
configured in more than one slot, but each call is admitted by the slot needed
for that lane: Primary and Reasoning models may execute the direct or Specialist
path, a Verifier model may enter only the verification lane, and a model
configured only as Utility cannot enter production routing or workflow
execution. The compatibility model can be copied to all four profiles only
through an explicit Settings action.

These fields are an event-local sidecar on current Agent model request events.
Started and finished events reuse the same explicit attribution selected at the
call site. Existing `role`, `stage`, persisted model keys, summaries, and event
counts are preserved for compatibility; legacy events without the schema remain
legacy and are not reclassified from display strings.

## Crate Ownership

| Crate | Current owner responsibility |
| --- | --- |
| `agent-core` | Transport-free IDs, messages, events, permissions, tool/model contracts, and shared schemas |
| `agent-eval` | Deterministic portable evaluation skeleton: case schema, postcondition checks, scripted-provider runner, per-run resource receipts, matched position-balanced arms, and suite reports; shares the product effort-tier scheduling authority; no production consumer, and the deterministic harness makes no provider network calls |
| `agent-runtime` | Kernel, run control, context governor, task contract, adaptive cursor, system-prompt composition, model-turn and tool-runtime semantics |
| `agent-application` | The run/reprepare driver, strategy/terminal lifecycle, the portable effort-tier scheduling authority (`EffortRunPlan`/planner, extracted from the desktop crate so the eval harness shares it), and the portable externally verified outcome contract |
| `agent-harness` | Active-run and exclusive-work registries; no model policy |
| `agent-memory` | Memory records, retention, recall, utility attribution, and deterministic curation contracts |
| `agent-rag` | Workspace indexing, file adapter, semantic retrieval, and vector-store integration |
| `agent-graph` | Graph extraction, direct graph retrieval, and graph walk |
| `agent-storage` | SQLite schema and durable event/state repositories |
| `model-provider` | HTTP/WebSocket provider transport, streaming adapters, and request-generation semantics (thinking defaults and generation-temperature overrides) |
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
- Foreground orchestration glue and memory workers.

This layer is not yet a thin adapter. `src/lib.rs` is a module index; the
Tauri builder and command registration live in `app_bootstrap.rs`, and several
workflows still cross desktop services by shared composition state. Future
refactors should move a complete owner and its tests behind a narrow
interface; merely creating more sibling files would not reduce coupling.

## Interactive Run Flow

### 1. Admission and identity

The task command validates provider, session, workspace, attachments, and model
capabilities, then persists the user message and start state. Logical run ID,
physical attempt ID, and steer epoch define replay and recovery boundaries.

`agent-harness` admits only one active owner for the same run/work key.
`AgentRunControl` applies cancellation, deadlines, stage budgets, and steer, and
owns the one physical model-resource ledger: the Owner turn loop and every
auxiliary lane (Plan drafting, Subagent children, rolling Summary) reserve and
settle physical attempts through it (`ControlledModelAttempt` /
`controlled_aux_model_call`), so `max_total_tokens` and physical-attempt
telemetry account for the whole run instead of undercounting the aux lanes.

### 2. Context and execution plan

The desktop adapter projects bounded history and an authoritative effective
objective. `agent-runtime` compiles invariant sources, complete tool rounds,
optional evidence, and a bounded cognitive overlay. Bounded project
instruction files (`AGENTS.md` between the workspace root and the Git root,
plus `.cindx/instructions/*.md`) join the prepared context as a protected,
untrusted guidance source carrying a provenance receipt.

Planning is deterministic and effort-tier based; it makes no model call. The
desktop effort planner (`agent_effort_planner`) builds one `EffortRunPlan` per
preparation: the effort label, the tier-selected primary model
(`effort_tier_model` — the tier's pinned default or the executor-role
fallback), fixed single-model scheduling facts, and the knowledge decision
(fast: no memory recall or workspace retrieval; default: relevant memory
recall keyed on the bounded run prompt plus workspace retrieval; high/xhigh:
comprehensive memory recall plus a wider workspace retrieval cap). Preparation
applies the prompt-derived route requirements onto the plan fail-closed (tool
requirement lift, effect authority, image-input vision) and validates the
tier-selected model's capabilities against the configured candidate pool. The
plan's facts are written key-for-key into the run context (including a
compatibility `run_decision` projection and the plan digest as
`execution_plan_semantic_sha256`), so the loop and contract read the same keys
they read before the conductor was removed.

The selected plan also produces a strategy receipt bound to task, session,
physical run, steer epoch, and plan digest. Run control serializes a
preparation checkpoint without entering execution, while one immediate SQLite
transaction writes the typed "Agent run decision selected" event. A
cancellation or steer that wins first prevents that stale decision from being
committed.

### 2a. Plan-then-confirm gate (High/Xhigh only)

When the Composer's plan toggle is on for a High/Xhigh run, the task command
runs a plan gate after the durable start commit and before preparation
(`agent_plan_mode_runtime`). The gate drafts a plan with a bounded read-only
tool loop that reuses the subagent whitelist (`file.read`, `file.list`,
`file.search`, `file.glob`, `web.search`, `web.fetch`) and the registry's
permissionless-read guard, and
charges every drafting call through `begin_stage_model_call(_,
RunStageClass::Worker)` plus a physical-attempt reservation on the unified
resource ledger, so plan drafting counts against the run budget like an Owner
call. The proposal (bounded markdown plus its
content-bound digest) and the later user decision are persisted as ordinary
run events; the gate then parks the run with the existing nonterminal pause
plus recovery-envelope mechanism (`waiting_for_plan_confirmation`), so
recovery re-presents the same pending confirmation. Resolving the gate reuses
the paused-run continuation path; the execution itself is unchanged. A
user-approved plan is injected during preparation from durable events via the
project-instructions pipeline pattern — a protected internal system message
with a provenance digest receipt and untrusted-guidance boundary text — and is
re-derived identically on re-preparation and recovery. The deterministic
`EffortRunPlan` remains the sole scheduling authority: plan mode writes only
`confirmed_plan_*` provenance keys and never touches tool, effect, model, or
budget scheduling keys.

Model request generation follows the effort policy: chat requests and the
credential probe disable provider-side thinking for model families whose
builds enable it by default, and the executor loop pins deterministic sampling
(`generation_temperature` `0`) for Fast and Default while High and Extra High
keep provider defaults. `agent-core` owns the metadata key; `model-provider` owns
the wire emission; desktop owns the per-effort value.

### 3. Retrieval and memory

Workspace retrieval and durable memory are independent inputs:

- `agent-rag` searches indexed file chunks and vector storage.
- `agent-graph` provides direct relation lookup and bounded graph walk.
- `agent-memory` ranks project/session memories under trust, utility,
  supersession, conflict, retention, and diversity constraints.

The desktop adapter coordinates provider embeddings, background queues, and
state persistence. Retrieved data retains source provenance and does not mutate
canonical chat history.

### 4. No workflow lane

Multi-model workflow collaboration has been physically removed. There is no
owner-execution graph materializer, no workflow checkpoint, no read-only
Specialist/Verifier widening, no verification-repair wave, and no conductor
planning model call. Every run is a single-model Owner execution planned
deterministically by effort tier; the preparation-time route requirements lift
and capability validation keep tool, effect, and vision constraints enforced,
and the foreground Owner owns all effects and final delivery. A residual
workflow decision fails closed rather than executing.

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

Write subagents (`task` with `allow_patches`) plug into the same permission
path without holding any authority of their own. A write subagent's patch call
is persisted as a pending permission request under the parent run's identity
(task, session, physical run, objectives), but with two deliberate marks:
`session_reusable=false` and a subagent-origin marker. The first makes the
request fail the session-capability policy in both directions — a session
grant the parent already holds for the same path never satisfies a
subagent-originated request, and an `allow_for_session` decision is rejected —
so every patch a write subagent attempts is approved per call by the user.
Rationale: a session grant encodes trust in the parent's current trajectory;
the child is a separately prompted model context whose patch targets the
parent did not necessarily foresee, so widening must be re-authorized at each
call. The subagent gate never consults the session-capability store at all,
so grant non-propagation is structural rather than emergent.

The approval handshake also differs deliberately from the run-level
suspend/resume mechanism. A subagent loop runs on a scoped thread inside the
parent's tool batch, and its internal message history is not part of the
durable runtime snapshot, so suspending the run would abandon the child and
replay the delegation on resume — re-asking the same approval and
double-executing an already-applied patch. Instead the subagent thread parks
on the durable permission row (polling with cancellation), the run stays
active, and the projected pending approval flips the run status to
waiting_for_permission as usual. The `resolve_agent_permission` command routes
subagent-marked requests to an in-place branch that validates and persists
only the resolution rows: it registers no run control (the live run already
owns the session slot) and resumes no loop. The parked subagent thread then
executes an approved patch through the ordinary
`execute_agent_tool_invocation_for_objective_epoch` path, so durable
started/finished events, tool-budget accounting, undo projection, and
workspace-cache invalidation are identical to a parent-run effect. A denial
records the same denied tool-finished event shape and returns to the child as
an ordinary denied observation; a cancellation before approval executes
nothing.

Grounding obligations accept negative evidence: when a tool attempt fails
against an input that matches the obligation's bound target anchors, the
failure is recorded as an absent-target receipt and satisfies the obligation,
stopping repair loops that could never succeed on a missing target. Anchor
matching stays mandatory, so failures against unrelated inputs grant nothing;
the absent receipt is surfaced to the model context so the answer can state
the absence instead of fabricating content.

### 6. Completion and recovery

Terminal selection cannot convert an unmet obligation into verified success.
The Finalizer role is retired with the serial collaboration architecture:
single-model sessions deliver through the actor, and a forced terminal commit
runs one toolless wrap-up turn of the same session model (actor system prompt,
no Summarizer role, no evolved directive) when no grounded material is
available yet; an unusable wrap-up falls back to the exact eligible grounded
candidate rather than restarting the actor. Empty or unusable wrap-up output
with no verified fallback gets one bounded retry of the gate. When no
deliverable control candidate exists but the run holds visible tool evidence,
the latest substantive visible assistant text (at least 160 chars, internal
drafts excluded) is admitted as a last-resort grounded fallback so a provider
flake cannot fail a run that already did visible work; short progress notes
and evidence-free runs stay closed. Forced terminal commits skip the wrap-up
model call entirely when such grounded material already exists and deliver it
directly. The direct-finalizer phenotype policy module remains dormant pending
physical removal with a prompt-genome schema migration.

Every tier above Fast passes an optional bounded judge gate at the
shared completion chokepoint (both direct completion and terminal-finalizer
routes): when the execution contract requires verification, the configured
Reviewer model is distinct from the executor, and the candidate is not a
fallback, one tool-free Reviewer call audits the
candidate against the objective and returns a typed single-line receipt. A `revise` verdict permits at most one Executor
repair round over the recorded findings and one recheck; a repaired answer is
re-grounded against the task contract before replacing the candidate. Judge
unavailability, inconclusive receipts, empty or ungrounded repairs, and
fallback candidates retain the original answer and record a
`direct_judge_disposition` instead of blocking delivery. The persisted
`direct_judge_fail_closed` provider setting (default off) diverts the two
judged quality failures — an ungrounded repair or a recheck still requiring
revision — to the existing failure terminal for runs with at least one
successful workspace mutation, recording a
`direct_judge_fail_closed_blocked` disposition; infrastructure failures stay
fail-open. Fast execution is
never judged.

The completion transaction persists terminal event, result, artifacts,
lifecycle, learning evidence, the observed-use measurement for recalled
memories, and cleanup under one attempt/epoch identity.
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

Delivery Verification was a separate `realworld-eval` authority; its desktop
runtime adapter, the `realworld-eval` cargo feature, and the
`cindx-agent-realworld-eval` binary were physically removed in the phase-3
effort-tier rebuild. V1, v2, and v3 are consumed lineages: all nine of their
preflight, authorization, and execute entrypoints return a consumed-protocol
error before reading arguments,
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

The portable collaboration-learning contracts and the `collaboration_learning_*`
modules in `agent-application` were removed in the phase-3 effort-tier rebuild
along with the `realworld-eval` adapter that produced them; multi-model
collaboration is permanently retired, so there is no collaboration-learning
policy, exercise receipt, offline-import feature, or train/holdout evidence
contract in the tree. The recall-to-terminal attribution that survives is
unrelated to collaboration learning.

The desktop `realworld-eval` adapter was the only runtime producer; it was
removed in the phase-3 effort-tier rebuild and the paragraph below is retained
as a historical record. Its explicit
successor-only entry installed a matched-arm policy, committed Direct assignment
with the durable strategy decision or Workflow assignment with the materialized
plan, applied the assigned context budget and fail-fast attempt bound, and
committed only the SHA-256 identity and size of the exact encoded request before
dispatch. Raw request contents were never added to the learning receipt. The
adapter retained the run event slice outside the frozen report schema and derived
the comparison binding from the actual pair plus a pre-frozen source/cohort
authority.

The successor evaluation control plane — the
`benchmarks/agent/collaboration-successor-protocol-v1.json` authority, its
desktop preflight, and the Goal 3E authorize/execute binaries — was removed in
the phase-3 effort-tier rebuild; the protocol file is absent from the tree and
the one-shot Goal 3E authority was consumed and closed `CENSORED` before any
valid observation (see the [EVALUATION.md](EVALUATION.md) ledger). It cannot be
rerun. The surviving outcome projector still distinguishes selected, explicit
pre-decision not-selected, absent, and malformed receipt states before outcome
construction, so a legal not-selected terminal remains a censor and can never
become an outcome.

## Persistence and Background Work

SQLite stores projects, sessions, events, permissions, checkpoints, schedules,
memory, and inert measurement projections. Persistent-state failure aborts
startup. Recovery adopts only one valid successor; missing, tampered, or forked
state fails closed, and an unfinished capture becomes a censor rather than a
provider retry.

Background services are bounded and must not block the healthy foreground path:

- Semantic memory curation and vector refresh.
- Schedule dispatch and sidecar health work.

The continuous prompt-evolution loop (prompt evidence projection,
mutation/evaluation, rollout, canary, transfer, and distillation) and the
offline collaboration-learning harness were retired and physically removed in
the phase-3 effort-tier rebuild; they are not background services any longer.

Canonical events remain authoritative. Derived serving snapshots and caches can
be rebuilt; a cache publication failure cannot rewrite the scientific outcome.

One shadow measurement file also lives beside the app data root outside
SQLite: `run-telemetry.journal.jsonl` (0600, capacity-capped, atomically
rewritten). The desktop loop appends one `cindx.agent.run-telemetry.v1`
receipt at each physical run's durable terminal commit (success, failure,
preparation failure, or cancellation) behind the existing exactly-once
terminal identity. It has no production reader or consumer; only tests load
it back.

## Prompt Evolution Boundary

Prompt evolution is retired. The campaign runtime, mutation, pairwise
evaluation, learning outbox, canary/rollout, and distillation machinery have
been removed from the desktop crate and the (now-deleted) `orchestrator` crate.
The prompt-genome / prompt-profile serving machinery is removed as well; a run
carries no prompt profile at all.

The shadow direct-judge outcome journal remains as inert measurement plumbing
with no production consumer; nothing can publish or promote a learned profile.

## Dependency Rules

- Portable crates must not import Tauri or desktop state.
- `agent-runtime` must remain provider-transport independent.
- Tools declare effects; the desktop permission path grants execution.
- Memory and workspace knowledge remain separate stores and provenance domains.
- UI projections may cache derived state but cannot become durable truth.
- Evaluation code cannot promote a profile without production admission gates.

These rules are checked by `scripts/check-desktop-structure.mjs`, Rust tests,
and the quality profiles described in [DEVELOPMENT.md](DEVELOPMENT.md).
