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
| `agent-core` | Shared ids, messages, events, permissions, tool and model contracts | Persistence or side effects |
| `agent-runtime` | `AgentKernel`, loop state, run control, budgets, context governance, tool admission, terminal semantics | Provider HTTP, permission UI, actual tool execution |
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
shipping or product-quality gate.

## Desktop Adapter Ownership

`apps/desktop/src-tauri` is the composition root. It owns integration that must
touch Tauri or product state:

- Tauri commands and event emission.
- Provider configuration and calls through `model-provider`.
- Tool registry construction and side effects through `tools`.
- Permission prompts, scoped session-grant lookup, and continuation.
- SQLite-backed project/session projections and runtime snapshots.
- Browser/computer sidecar process integration.
- Execution adapters that translate product state into the portable
  application driver, runtime, and orchestrator contracts.
- Background memory refresh, session-title refinement, and prompt evaluation.

This concentration is a known structural limit. New portable policy must not be
added to the desktop prelude merely because the composition root can access all
state.

## Decision and Workflow Relationship

`AgentRunDecision` is the validated boundary between planning and execution. It
contains the task class, direct/workflow mode, primary model, tool requirement,
risk, retrieval channels, memory policy, verification policy, parallelism,
branch quorum, estimated steps, expected uplift, confidence, and stop policy.

- Fast constructs a direct decision without a conductor call.
- Auto and Pro ask configured conductor candidates for this schema.
- A direct decision enters the interactive loop without collaboration.
- A workflow decision creates a bounded adaptive workflow. The task graph owns
  dependency order and runnable/resumable/degraded/exhausted states.
- Independent workflow contributions are defined by non-overlapping task and
  evidence lineage, not by model identity. The conductor may reuse the selected
  direct-baseline model across different branches or choose another configured
  model when role capability or supported historical evidence justifies it.
- Workflow output is a grounded handoff to the interactive loop; it does not
  bypass the final tool, permission, persistence, or terminal contracts.

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
   completed, independently scored, usage-complete Auto workflow with a
   finalized checkpoint and no denied permission or safety violation.
2. The teacher profile, source run, final output, and dataset are fingerprinted;
   bounded redacted workflow evidence is retained for background evaluation.
3. Current and challenger Pro profiles run the same objective. An evaluator
   that did not participate in either workflow performs a position-balanced
   comparison against the archived Auto result.
4. Auto-transfer observations are stored separately from same-effort GEPA
   observations. They have their own dataset digest and promotion gate.
5. Pro mutation reserves reflection capacity for both ordinary Pro evidence and
   Auto-transfer evidence. Promotion requires both gates, and the frozen Pro
   snapshot pins the active Auto source and both evidence sets.

This path adds no model call to the foreground user request. Missing independent
reviewers, incomplete evidence, changed Auto lineage, disagreement, or an
insufficient train/holdout cohort fails closed and leaves the current stable Pro
profile unchanged.

The evolution read model scopes learned genomes, observations, offline
datasets, and rollout state by project. Records without a durable scope are
rebuilt from canonical events before they can participate in selection.

Prompt evolution is not the conductor, task graph, or run loop. It cannot alter
active permissions, transcripts, tool observations, or budgets.

## Persistence and Recovery

- SQLite under the Cindx application data directory is authoritative durable
  state.
- User messages, lifecycle transitions, tool events, permission decisions,
  traces, and completion data are persisted as events/projections.
- Suspended and recoverable runs preserve canonical transcript state and typed
  run-control metadata.
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
