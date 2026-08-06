# Current Architecture

This is the maintained architecture map for the current Cindx source tree. Source code is the
implementation authority; this document records ownership and data flow so
multiple agents do not infer different systems from historical reports.

## System Boundary

```text
React UI
  -> typed Tauri commands and event subscriptions
Tauri desktop adapter
  -> persistence, provider and tool side effects
Application layer
  -> run/reprepare driver, typed run lifecycle, session projections
Agent runtime + harness
  -> one interactive loop, run control, budgets, context, cancellation
Orchestrator
  -> conductor decision, workflow/task graph, verification, prompt evolution
Domain adapters
  -> storage, memory, RAG, graph, MCP, skills, model provider, tools
External systems
  -> configured model APIs, filesystem, shell, browser/computer sidecars, MCP
```

The model never receives direct operating-system authority. Tool execution
passes through the local registry, permission broker, cancellation contract,
and event trail.

## Interactive Run Flow

The production entry point is `run_agent_task` in
`apps/desktop/src-tauri/src/agent_commands/task.rs`.

```text
run_agent_task
  -> acquire per-session AgentRunControl lease
  -> validate provider, workspace, attachments, and session
  -> persist task start + user message
  -> create AgentLoopState from bounded session history
  -> prepare_agent_execution
       -> project bounded context
       -> plan_agent_run
            Fast: direct decision
            Auto/Pro: conductor -> validated AgentRunDecision
            Auto workflow candidate: portable value-of-computation admission
       -> recall memory + retrieve workspace evidence
       -> select skills and tools
       -> optional bounded workflow/task graph
       -> append grounded workflow handoff
  -> execute_agent_run
       -> execute one prepared loop epoch
            -> model request
            -> admitted tool batch
            -> permission suspension/resume when needed
            -> commit observation; make canonical outcome durable/recoverable
            -> task-contract Goal Delta / typed denial admission and contract checks
       -> reprepare after a committed steer, or finish at a typed control boundary
       -> terminal result or typed interruption
  -> completion transaction
       -> events, messages, artifacts, lifecycle, learning evidence, cleanup
       -> background memory refresh / prompt evaluation
```

Steer, queue, cancellation, permission continuation, retry, and recovery reuse
the same run-control and lifecycle vocabulary. They are not independent loops.

`agent-core` defines `cindx.agent-run-identity.v1`: the immutable
`logical_agent_run_id` identifies one user task and `agent_run_id` identifies
one physical execution attempt. Initial execution sets them equal; steer keeps
both and advances its epoch; continuation keeps the logical ID, records the
physical `source_agent_run_id`, and creates a new physical ID. The memoized
legacy projection follows source-attempt chains only inside the same
task/project/session and rejects conflicts, missing links, cycles, and
cross-scope edges. This projection is derived from canonical events rather than
stored as a second trajectory blob.

## Crate Ownership

| Crate | Owns | Does not own |
| --- | --- | --- |
| `agent-core` | Shared ids, logical-run/physical-attempt lineage, messages, events, permission capability policy, tool and model contracts | Persistence or side effects |
| `agent-runtime` | `AgentKernel`, typed prepared task/checkpoint state, loop state, bounded cognitive projection and adaptive cursor, task-contract Goal Delta and denial/replan policy, run control, budgets, context governance, grounding scope/tool policy, model transport retry/progress policy, tool admission, terminal semantics | Provider HTTP, permission UI, actual tool execution |
| `agent-harness` | Active-run registry and exclusive-key leases over `AgentRunControl` | Agent policy or workflow planning |
| `orchestrator` | Typed run decisions, workflow/task graph, role assignment, verification, frontier selection, recovery policy, prompt-genome evaluation | Tool side effects, Tauri state, provider wire protocol |
| `agent-memory` | Durable memory extraction, trust labels, deduplication, supersession, lexical/semantic recall | Workspace file indexing |
| `agent-rag` | Workspace chunking, embeddings, file-backed index, optional LanceDB implementation, semantic search | Graph relationships or session memory |
| `agent-graph` | Graph extraction, provenance, persistence, direct expansion and graph-guided retrieval inputs | Vector storage |
| `agent-storage` | SQLite event/state contracts, indexed logical-run event scope, and implementation | Agent decisions or logical permission scope |
| `model-provider` | OpenAI-compatible request/response, streaming, embeddings, image-provider wire behavior | Routing or local tools |
| `tools` | Built-in tool specifications, validation, local/delegated execution contracts, workspace exact-patch publication, bounded query cursors, managed process sessions, and model observations | Permission decisions or UI |
| `agent-mcp` | MCP transports, catalog cache, and tool adaptation | Permission bypass or agent policy |
| `agent-skills` | Skill discovery, trust, selection, and loading | Privileged script execution |
| `agent-application` | Run/reprepare driver, typed run lifecycle, application projections, and session-level contracts | Provider construction, Tauri state, persistence, or tool side effects |
| `orchestrator-eval` | Non-shipping benchmark and evaluation harnesses | Product runtime behavior |

The root workspace excludes `orchestrator-eval` from default members so the
research harness does not enter ordinary product builds.

Within `model-provider`, immutable prepared streaming payloads own encoded
request bytes and their canonical request digest. Transport execution remains
separate from provider-identity and semantic-response receipt construction.

The desktop default feature set includes `lancedb-store`; production, CI, and
release builds therefore retain the complete vector-store implementation. The
desktop crate also exposes a no-default-features compile surface for fast Rust
type checks without Arrow/DataFusion/Lance or frontend bundle resources. On
that surface the stable RAG storage API remains type-compatible but fails
closed with an explicit error. It is not a runtime fallback and is not a
shipping or product-quality gate. CI and release use this surface for the
desktop warning-free Clippy contract; their default-feature tests and bundle
build continue to validate the shipping vector store.

## Desktop Adapter Ownership

`apps/desktop/src-tauri` is the composition root. It owns integration that must
touch Tauri or product state:

- Tauri commands and event emission.
- Provider configuration and calls through `model-provider`.
- Tool registry construction and side effects through `tools`.
- One stable AppState `ProcessManager`, shared across registry rebuilds, owns
  managed child lifecycle and performs bounded shutdown before application exit.
- Permission prompts, indexed scoped session-grant lookup, and continuation. The portable capability-match and session-reuse policy remains in `agent-core`.
- SQLite-backed project/session projections and runtime snapshots.
- Browser/computer sidecar process integration.
- Execution adapters that translate product state into the portable
  application driver, runtime, and orchestrator contracts.
- Background memory refresh, session-title refinement, and prompt evaluation.

Workspace patch planning, hashing, target locking, race rechecks, atomic
publication, and receipts stay in `tools`. The desktop adapter does not own a
second file-mutation implementation; it persists the effect contract and uses
the recorded after-SHA to verify an interrupted direct or deferred `file.patch`.

This concentration is a known structural limit. New portable policy must not be
added to the desktop prelude merely because the composition root can access all
state.

The feature-gated Agent Real-World driver is also split by responsibility: its
parent module owns the frozen suite, execution plan, case materialization, and
raw report; its `treatments` child owns versioned treatment and schema identity;
its `execution` child owns one-cell oracle-reference or product execution; and
its `runtime` child owns product-run continuation and event metric projection.
Its `http_fixture` child owns the ephemeral loopback server and request receipt
for each browser cell; the resolved URL is part of the case contract rather than
an ambient browser dependency. Its `tool_receipts` child projects typed attempts
only from the current logical Agent run, preserves every projected terminal or
unfinished attempt in the denominator, and digests workspace-bound artifact evidence.
Verification accepts a tool obligation only after successful
completion. Exact browser-target matching and artifact or postcondition digests
bind observed effects to the frozen case. This is evaluation wiring and does not
define shipping policy or establish provider-backed intelligence improvement.

V5's Grounded Direct arm enters the same Auto planning path, then applies one
typed evaluation constraint after the conductor decision. That constraint
preserves the selected model, tool class, retrieval, memory, vision, risk, and
task classification while collapsing workflow parallelism and independent
worker verification to a valid direct decision. Native product runs carry no
constraint metadata, so the seam is unreachable from ordinary product ingress.

Prompt grounding classification, evidence-tool pinning, run-context objective
selection, and model-stream retry/progress policy are portable
`agent-runtime` responsibilities. The desktop loop supplies catalog and product
state, then executes the resulting provider and tool side effects.

`agent-runtime::task_contract::cognitive_state` owns the bounded model-facing
projection of the current prepared epoch. It derives one protected transient
overlay from `PreparedTaskState`, `AgentTaskContract`, and its outcome ledger;
if that advisory overlay alone prevents a hard context invariant, the kernel
reprojects once without it rather than discarding required trust or evidence.
The projection is never another mutable or durable fact store. The adjacent
adaptive cursor owns only bounded typed-observation hashes and no-gain counters,
and cold task-state restoration intentionally recreates it empty for the
restored steer epoch. The desktop adapter transports the complete `ToolResult`
into the runtime transition and executes the resulting continuation or terminal
disposition. It does not derive cognitive facts or maintain a parallel loop
state.

The foreground execution path has three explicit responsibilities. The Actor
owns model/tool iteration and its turn budget. Verifier authority belongs to the
typed post-commit tool-observation transition, not to model prose. A tools-disabled
Finalizer owns only terminal-reserve delivery and does not advance Actor turns.
The desktop precomputes a current-epoch grounded fallback before dispatch; an
empty, malformed, tool-calling, or unavailable Finalizer either returns that
byte-identical candidate after receipt revalidation or fails closed without
starting another Actor turn.
Asynchronous managed-process polls remain ordinary typed observations: their
distance from the initiating action prevents them from minting a causally trusted
workspace-quality receipt. Command words such as `test`, `check`, or `build` do
not change that boundary.
`agent_finalizer_runtime` owns fallback and receipt policy; its
`terminal_runtime` child owns the desktop-only instruction persistence, provider
dispatch, stream reset, and terminal handoff, keeping the Actor loop independent
of Finalizer integration details.

`task_contract/goal_delta` compares bounded contract state before and after a
committed tool observation. It admits only first satisfaction of an active
obligation, first target-bound grounding, or verification of a workspace or
interaction postcondition. The desktop adapter records that typed receipt only
after the matching runtime transition commits and the canonical tool outcome is
durable or recoverable. Raw tool input/output, ordinary success, replay, and
failure status cannot mint budget credit. Existing provider/tool activity
timestamps and generic checkpoints remain liveness or diagnostic signals rather
than a second semantic-progress authority; only Goal Delta credit extends a run
segment.

`task_contract/postcondition_receipt` binds a verified postcondition to its
steer/contract epochs, typed surface, action and observation sequences, verifier
source, and digests. Receipts are bounded, retain no raw arguments or output, and
are accepted only when bounded contract evidence still contains the matching
action and observation. Workspace target digests are scoped to the logical run
and contract epoch; persisted legacy witnesses remain recoverable but cannot
mint typed authority. Checkpoint decode validates bindings, receipts, epochs, and
evidence references before restore. Desktop completion quality derives its
compatibility flag from this receipt; an unbound boolean cannot claim
verification.

`task_contract/denial` owns the other side of that transition. It records only
bounded typed denial facts tied to the active prepared-contract epoch, projects a
terminal denial as `Blocked` rather than `Satisfied`, and owns the single
same-epoch replan token for policy or capability denial. User permission denial
goes directly to blocked finalization, while a new prepared-contract epoch clears
the prior objective's denial state. `AgentKernel` binds denial evidence to the
tool observation and completion receipt. Desktop adapters classify trusted
permission and dispatch outcomes and suppress a denied invocation before the
permission broker. Permission resolution, its canonical denied tool outcome,
and the transcript observation share one SQLite transaction. Hot or rebuilt
cold task state is ready before a separate transaction claims the recovery
checkpoint and records the resumed lifecycle; startup replay reconstructs a
committed denial if an older checkpoint predates it. Within the desktop adapter,
`agent_recovery_service` composes claim and startup reconciliation,
`agent_permission_recovery` owns the failed-handoff transaction, and
`agent_recovery_status` owns typed lifecycle and cancellation projection.
Desktop code does not own another loop, retry budget, or semantic replan policy.

Within run control, `control_steer_commit` owns durable steer-batch commit and
the bounded base segment opened for an actually applied objective. It does not
admit Goal Delta credit; that remains isolated in `control_goal_delta`.

`agent_terminal_commit_runtime` is the durable terminal serialization boundary.
It hashes task, session, physical run, and steer epoch into one terminal identity
and uses an immediate SQLite transaction for both success and failure. Replay
returns the existing terminal state; malformed, nonterminal, duplicate, or
partially written identities fail and roll back. The in-memory terminal lease
still arbitrates live steer/cancel races, while SQLite provides restart-safe
exactly-once persistence.

The validated conductor decision also supplies execution intent. The desktop
composition root converts its task class, tool requirement, and vision flag into
a focused catalog exposure plan and prompt-epoch completion obligations. Normal
execution and permission recovery call the same planner. Tool success from an
older steer epoch cannot satisfy the current prompt, while replay of the same
persisted epoch retains already recorded success. Browser observation is a
separate evidence domain from general web retrieval and screen observation.

## Decision and Workflow Relationship

`AgentRunDecision` is the validated boundary between planning and execution. It
contains the task class, direct/workflow mode, primary model, tool requirement,
risk, retrieval channels, memory policy, verification policy, parallelism,
branch quorum, estimated steps, expected uplift, confidence, and stop policy.

`AgentRouteRequirements` is the smaller authoritative input boundary. It is
re-derived for every prepared steer epoch from completion intent, image
generation, and the active user image input. The decision harness verifies that
the conductor did not lower its tool or vision floor and that the selected
configured model declares the required capabilities. Fast retains its default
model choice and fails clearly when it is incompatible; Auto and Pro select a
compatible configured fallback before making conductor calls.

Completion intent classifies compound imperative steps outside quoted or fenced
material without treating dots in workspace filenames or URLs as sentence
boundaries. Source clauses resolve their own target or an explicit relation to
named workspace inputs, so a local output field named `sources` cannot invent
external grounding and an unrelated workspace path cannot suppress an external
source request. Target anchors are restricted to active evidence domains before
the contract is installed. `effect_instruction_segments` owns the one-pass
literal-aware lexical boundaries; `completion_intent` retains semantic authority
for whether those segments require effects.

- Fast constructs a direct decision without a conductor call.
- Auto and Pro ask configured conductor candidates for this schema.
- After validation, Auto workflow candidates pass the portable orchestrator
  value-of-computation policy. The policy uses the conductor's semantic
  independent-contribution contract rather than re-routing through prompt
  keywords, and combines predicted benefit, confidence, exact-shape matched
  evidence, compute units, and serial-interaction risk. Rejection collapses only
  workflow coordination fields; model, tools, vision, risk, retrieval, and
  memory remain intact. Candidate and selected tiers, verdict, value, cost, and
  evidence support are recorded in route metadata.
- A direct decision enters the interactive loop without collaboration.
- A workflow decision creates a bounded adaptive workflow. The task graph owns
  dependency order and runnable/resumable/degraded/exhausted states.
- `first_verified` requires an observed verified verdict before normal early
  commit. Terminal reserve may still return a usable best-known fallback, but
  the selection assessment keeps unmet verification as degradation. A prompt
  genome using Minimal verification is raised to Evidence whenever the
  execution contract requires verification.
- The conductor is told the actual worker capability boundary. Isolated workers
  cannot be assigned permission-gated browser, computer, shell, or mutation work;
  those effects remain in the foreground executor.
- Independent workflow contributions are defined by non-overlapping task and
  evidence lineage, not by model identity. The conductor may reuse the selected
  direct-baseline model across different branches or choose another configured
  model when role capability or supported historical evidence justifies it.
- Workflow output is a grounded handoff to the interactive loop; it does not
  bypass the final tool, permission, persistence, or terminal contracts.
- Desktop conductor calls apply a 45-second no-progress boundary even when no
  alternate model exists. Alternate-model recovery remains a desktop transport
  concern; value admission and route calibration remain portable orchestrator
  policy.

## Context, Memory, and Retrieval Relationship

These are separate inputs and must remain distinguishable in trace metadata:

- **Conversation context** is a bounded projection of canonical session
  messages for the current objective.
- **Cognitive state** is a bounded transient projection of the current prepared
  epoch's typed task contract, evidence references, and adaptive disposition. It
  does not replace canonical conversation history or persist a second truth.
- **Durable memory** is cross-turn or cross-session evidence produced from
  eligible prior runs and recalled with trust controls.
- **Workspace knowledge** comes from indexed files and graph relations.
- **Workflow evidence** comes from current-run workers and must carry grounding
  provenance before entering the main loop.

Memory recall and workspace retrieval can run concurrently. Semantic search,
file search, and direct graph lookup are independent first-stage channels;
graph walk expands from selected seeds. Fusion must not erase source identity.
The conductor admits these blocking foreground operations only when missing
project evidence is expected to change the answer. Before prompt injection,
verified current-turn requirements suppress conflicting historical
requirements. Completed self-contained direct text-only runs still refresh
deterministic memory state but do not spend a second model call on semantic
curation.

## Prompt Evolution Relationship

Prompt evolution observes completed or replayed evidence after the foreground
run. Candidate genomes are evaluated outside the active loop and promoted only
through the configured evidence gates. A promoted immutable profile may
configure a future conductor/workflow run.

The Pro evolution path can learn from qualified Auto outcomes without coupling
the two foreground runtimes:

1. The completion transaction reconstructs a teacher case only from a
   completed, usage-complete Auto workflow with a finalized checkpoint and no
   denied permission or safety violation. The score must be backed by a
   completed provider receipt from a dedicated reviewer request outside the
   workflow. A distinct evaluator model is preferred when available; if the
   configured model is reused, its separate request and model identities remain
   explicit evidence and the participant request itself cannot act as reviewer.
2. The teacher profile, source run, final output, provider/model identity,
   system prompt, policy, budget, tool contract, source/workspace evidence,
   evaluator receipt, checkpoint, and learning receipt are fingerprinted. Its
   final steer epoch is pinned, and bounded redacted workflow evidence is
   retained for background evaluation.
3. Current and challenger Pro profiles run the same objective. An evaluator
   that did not participate in either workflow performs a position-balanced
   comparison against the archived Auto result.
4. Auto-transfer observations are stored separately from same-effort GEPA
   observations. They have their own dataset digest and promotion gate. The
   mirrored pair is projected only when both records, project scope, and Auto
   source lineage agree.
5. The portable reflection selector accepts only complete strict transfer pairs
   from the active train cohort, ranks their redacted Pro/Auto trajectories by
   actionable information and strategy contrast, and keeps diverse pairs within
   the existing six-trajectory mutation budget. The mutation event fingerprints
   the exact selected set. Promotion requires both gates, and each gate blocks
   candidate-only failure or holdout quality, latency, and token non-inferiority
   regression before the frozen Pro snapshot can pin the active Auto source and
   both evidence sets.
6. Canary allocation never exceeds 50 percent. Promotion atomically installs a
   valid frozen snapshot; missing or regressed lineage rolls back to the prior
   stable profile.

An isolated Pro failure curriculum complements these positive observations.
The desktop projection derives typed, hash-only timeout, denial, and no-progress
receipts from canonical terminal or recovery events in the current steer epoch.
The portable orchestrator validates their project, run, profile, policy, epoch,
and source bindings and converts them only to negative `FailureSeed`
reflections. A mutation may consume them only when a successful scientific
train observation for the same profile supplies the comparison anchor. At most
two diverse failure seeds share the existing six-reflection ceiling with
ordinary and Auto-transfer evidence. They never enter teacher, Goal Delta,
canary-success, promotion, or distillation lineages.

This path adds no model call to the foreground user request. Missing independent
reviewers, incomplete evidence, changed Auto lineage, disagreement, or an
insufficient train/holdout cohort fails closed and leaves the current stable Pro
profile unchanged.

Auto completion records a durable project-scoped Pro intent. The background
worker dispatches it idempotently and compensates an interrupted dispatch. The
queue coalesces only requests with the same project and
effort, and its wall-clock, token, and physical-attempt budget persists across
checkpoints and request-scoped action events. Missing, malformed, interrupted,
or regressed recovery accounting fails closed.
Repeated objectives prefer a qualified teacher from the current stable Auto
profile over stale profiles. Reflective mutation output is rejected if its
custom directive copies case-specific trajectory or feedback content, including
opaque run/model identifiers; repair output is checked by the same boundary.

A promoted Pro profile can teach a later Auto challenger only through the
separate Pro-to-Auto distillation track:

1. The teacher must be the current project-scoped frozen Pro champion. Its
   attestation binds both ordinary Pro evidence and the Auto-to-Pro transfer
   evidence that qualified it; legacy or incomplete snapshots cannot teach.
2. Distillation derives a bounded Auto-compatible child from the stable Auto
   parent and the complete one- or two-gene structural delta between the
   champion and the exact stable Pro profile it defeated. The dispatch boundary
   first replays both frozen Pro gates. Directive text, larger bundles,
   unadapted Pro topology, and excess Pro resources fail closed; the child
   lineage binds the defeated Pro fingerprint and never copies a Pro genome
   wholesale.
3. The teacher, Auto parent, child, source datasets, cohorts, receipts, and
   flattened ancestor lineage are fingerprinted. Teacher cases and equivalent
   objectives are excluded from both the fresh train and holdout splits, and a
   missing typed source manifest fails closed.
4. Pro-to-Auto observations use their own explicit evolution method, dataset,
   cohort, and matched-attempt lineage. Ordinary Auto evidence and Auto-to-Pro
   evidence cannot satisfy this gate or overwrite the stable Auto parent's
   provenance.
5. The background outbox is durable and idempotent, but does not scan or launch
   distillation while a foreground Agent run is active. Campaign work remains
   bounded, cancellable, project-scoped, and isolated from ordinary Auto
   evolution requests.
6. A passing matched train/holdout gate starts a 10 percent canary under an
   immutable lease for candidate, stable, cohort, and evidence fingerprints.
   Each transition through 25 and 50 percent to stable promotion consumes fresh
   outcomes from both sides. Failed, denied, malformed, and censored assignments
   count negatively; completion, quality, latency, and token comparisons use
   contemporaneous rates or averages and include task-class checks. Drift or
   regression rolls back to the frozen stable Auto profile and quarantines that
   immutable candidate. Exact evidence replay is idempotent, while a conflicting
   payload for the same identity invalidates the pair. Only the final stage can
   install a new frozen Auto snapshot.

These contracts make self-distillation controlled and replayable. Deterministic
tests establish eligibility, isolation, reachability, and rollback behavior;
they do not establish provider-backed intelligence improvement.

Auto-transfer and Pro-distillation share a small event-projected outbox read
model. It stores a 256-intent FIFO work window and a canonical event cursor;
latest pending work is coalesced only by project and learning track, and each
track is selected by canonical sequence with deterministic identity tie-breaks.
The portable orchestrator domain owns the typed Auto/Pro intents, normalized
identity and legacy aliases, fail-closed queue aggregate, overflow policy, and
dispatch-recovery decision. Desktop adapters map events into those types, use an
indexed tail lookup for normal wakes, replay the phase-16 stream after a bad
snapshot or non-contiguous delta, and perform SQLite/CAS, Tauri, worker, and
provider effects. An overflow flag keeps the persisted window bounded; when
dispatched markers free capacity, full replay refills the next window from
canonical events, and replacing a retained project while overflowed also forces
replay before FIFO membership changes. Legacy Pro markers must match the pending
intent's normalized run context before removal, preventing a marker for a
superseded rollout from deleting newer coalesced work. CAS publication binds the
observed revision and full payload, with one reload before conflict failure, so
concurrent workers cannot publish stale discovery state. Dispatch markers are
project-bound and remove envelopes through the same projection; no canonical
intent or audit event is deleted.

The desktop boundary keeps live canary outcome projection in
`prompt_canary_outcome_projection` and bounded canary observation, quarantine,
and rollback helpers in `prompt_canary_runtime`; the evolution read model and
rollout reconciler compose those narrower responsibilities.

The evolution read model scopes learned genomes, observations, offline
datasets, failure curricula, and rollout state by project. Records without a durable scope are
rebuilt from canonical events before they can participate in selection. Its
projection version is explicit, so a semantic projection change forces a
canonical replay instead of trusting a structurally compatible stale cache.
Rollout changes are event-first, and cache publication uses the same
revision-and-payload CAS rule as the outbox. Scope projection selects only the
requested project's records instead of cloning the full model, and cold teacher
reconstruction indexes source-run events once. A compact per-identity payload
fingerprint ledger preserves conflict tombstones after full genome payloads leave
the hot window, so incremental projection and cold replay exclude the same
identities from selection. Hot-state compaction is reference-safe and soft: it may
retain more than the target limit rather than evict active cohort, attempt,
stable/canary/frozen/quarantine, or matched-observation lineage. Canonical events
remain the complete audit and deterministic recovery source.
`prompt_evolution_hot_state` owns scope selection, duplicate-genome conflict
handling, and reference-safe soft retention; `prompt_evolution_read_model`
depends on that module and owns event projection plus CAS publication, not the
reverse.

Prompt evolution is not the conductor, task graph, or run loop. It cannot alter
active permissions, transcripts, tool observations, or budgets.
It currently learns only workflow fields exercised by the matched evolution
harness. Route, retrieval, and memory stay in the per-run decision layer until
that layer has its own frozen matched causal evaluation.

## Persistence and Recovery

- SQLite under the Cindx application data directory is authoritative durable
  state.
- User messages, lifecycle transitions, tool events, permission decisions,
  traces, and completion data are persisted as events/projections.
- Events index both immutable logical-run identity and physical-attempt
  identity. Permission rows deliberately index only the physical attempt, so a
  continuation cannot inherit an allow-once decision or effect authority merely
  because it belongs to the same logical task.
- Suspended and recoverable runs preserve canonical transcript state, typed
  prepared-task and obligation checkpoints, and typed recovery state, reason,
  and identity. The desktop adapter keeps the existing storage wire through an
  explicit legacy-metadata projection rather than a second runtime truth.
- Checkpoint v2 persists only bounded fingerprints, epochs, intent, obligations,
  and counters. Cold recovery supplies the separately reconstructed effective
  objective; the runtime validates prompt, transcript, prepared state, and
  recovery identity before accepting it. The v1 decoder remains explicit.
- A started `file.patch` without a terminal result is recovered as applied only
  when its persisted verifier exactly matches the current canonical workspace
  file SHA-256. A mismatch or oversized/non-file target remains an unknown
  outcome and is never repeated automatically.
- Startup aborts if persistent state is unavailable. It never reports a
  successful in-memory substitute.

## Dependency Direction

Portable crates depend inward on contracts, not on Tauri:

```text
agent-core
  <- model-provider, agent-storage, agent-memory, agent-mcp, agent-skills
model-provider + agent-core
  <- agent-runtime, tools
agent-runtime
  <- agent-harness
agent-rag
  <- agent-graph
agent-core
  <- orchestrator
all product crates
  <- desktop composition root
orchestrator + agent-core
  <- orchestrator-eval (non-shipping)
```

Crate separation is necessary but not sufficient. A file split that still
relies on one global prelude or bidirectional ownership is not considered an
architecture improvement.
