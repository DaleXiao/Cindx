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
            -> observations and contract checks
       -> reprepare after a committed steer, or finish at a typed control boundary
       -> terminal result or typed interruption
  -> completion transaction
       -> events, messages, artifacts, lifecycle, learning evidence, cleanup
       -> background memory refresh / prompt evaluation
```

Steer, queue, cancellation, permission continuation, retry, and recovery reuse
the same run-control and lifecycle vocabulary. They are not independent loops.

## Crate Ownership

| Crate | Owns | Does not own |
| --- | --- | --- |
| `agent-core` | Shared ids, messages, events, permission capability policy, tool and model contracts | Persistence or side effects |
| `agent-runtime` | `AgentKernel`, typed prepared task/checkpoint state, loop state, run control, budgets, context governance, grounding scope/tool policy, model transport retry/progress policy, tool admission, terminal semantics | Provider HTTP, permission UI, actual tool execution |
| `agent-harness` | Active-run registry and exclusive-key leases over `AgentRunControl` | Agent policy or workflow planning |
| `orchestrator` | Typed run decisions, workflow/task graph, role assignment, verification, frontier selection, recovery policy, prompt-genome evaluation | Tool side effects, Tauri state, provider wire protocol |
| `agent-memory` | Durable memory extraction, trust labels, deduplication, supersession, lexical/semantic recall | Workspace file indexing |
| `agent-rag` | Workspace chunking, embeddings, file-backed index, optional LanceDB implementation, semantic search | Graph relationships or session memory |
| `agent-graph` | Graph extraction, provenance, persistence, direct expansion and graph-guided retrieval inputs | Vector storage |
| `agent-storage` | SQLite event/state contracts and implementation | Agent decisions |
| `model-provider` | OpenAI-compatible request/response, streaming, embeddings, image-provider wire behavior | Routing or local tools |
| `tools` | Built-in tool specifications, validation, local/delegated execution contracts | Permission decisions or UI |
| `agent-mcp` | MCP transports, catalog cache, and tool adaptation | Permission bypass or agent policy |
| `agent-skills` | Skill discovery, trust, selection, and loading | Privileged script execution |
| `agent-application` | Run/reprepare driver, typed run lifecycle, application projections, and session-level contracts | Provider construction, Tauri state, persistence, or tool side effects |
| `orchestrator-eval` | Non-shipping benchmark and evaluation harnesses | Product runtime behavior |

The root workspace excludes `orchestrator-eval` from default members so the
research harness does not enter ordinary product builds.

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
- Permission prompts, indexed scoped session-grant lookup, and continuation. The portable capability-match and session-reuse policy remains in `agent-core`.
- SQLite-backed project/session projections and runtime snapshots.
- Browser/computer sidecar process integration.
- Execution adapters that translate product state into the portable
  application driver, runtime, and orchestrator contracts.
- Background memory refresh, session-title refinement, and prompt evaluation.

This concentration is a known structural limit. New portable policy must not be
added to the desktop prelude merely because the composition root can access all
state.

Prompt grounding classification, evidence-tool pinning, run-context objective
selection, and model-stream retry/progress policy are portable
`agent-runtime` responsibilities. The desktop loop supplies catalog and product
state, then executes the resulting provider and tool side effects.

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
   parent and only the learned delta relative to the Pro seed. It never copies
   the Pro genome, custom directive, budget, or parallel topology into Auto.
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
6. A passing matched train/holdout gate starts a 10 percent canary. Fresh live
   evidence advances it through 25 and 50 percent; safety, quality, latency,
   token, lineage, or gate regression rolls back to the frozen stable Auto
   profile. Only the final stage can install a new frozen Auto snapshot.

These contracts make self-distillation controlled and replayable. Deterministic
tests establish eligibility, isolation, reachability, and rollback behavior;
they do not establish provider-backed intelligence improvement.

The evolution read model scopes learned genomes, observations, offline
datasets, and rollout state by project. Records without a durable scope are
rebuilt from canonical events before they can participate in selection. Its
projection version is explicit, so a semantic projection change forces a
canonical replay instead of trusting a structurally compatible stale cache.

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
- Suspended and recoverable runs preserve canonical transcript state, typed
  prepared-task and obligation checkpoints, and typed recovery state, reason,
  and identity. The desktop adapter keeps the existing storage wire through an
  explicit legacy-metadata projection rather than a second runtime truth.
- Checkpoint v2 persists only bounded fingerprints, epochs, intent, obligations,
  and counters. Cold recovery supplies the separately reconstructed effective
  objective; the runtime validates prompt, transcript, prepared state, and
  recovery identity before accepting it. The v1 decoder remains explicit.
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
