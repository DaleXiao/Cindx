# Modules

The tool and harness architecture is specified in [TOOL_HARNESS_SPEC.md](./TOOL_HARNESS_SPEC.md).
Browser session and interaction boundaries are specified in
[BROWSER_CONTROL.md](./BROWSER_CONTROL.md).

## apps/desktop

Tauri desktop application.

Responsibilities:

- Render chat and task timeline.
- Render permission prompts.
- Render tool-call and audit panels.
- Render agent trace turns, step inspector, and trace export controls.
- Render persisted project and session navigation in the left sidebar.
- Call Rust commands exposed by the agent kernel.
- Manage provider, workspace, sidecar, project/session, and memory settings.
- Adapt Tauri commands to the queue, session projection, runtime, orchestration,
  permission, and persistence boundaries.

Non-responsibilities:

- Direct file writes.
- Direct shell execution.
- Direct cloud model calls that bypass the kernel.

## crates/agent-core

Shared domain model.

Responsibilities:

- Task ids and task status.
- Event ids and event kinds.
- Message roles.
- Tool invocation/result contracts.
- Permission request/decision contracts.
- Model roles.
- Common metadata shapes.

## crates/agent-storage

Persistence boundary.

Responsibilities:

- Event append/read contracts.
- Conversation persistence contracts.
- Permission audit persistence.
- SQLite implementation.

## crates/agent-graph

Graph extraction and graph-guided retrieval boundary.

Responsibilities:

- Graph node, edge, and provenance contracts.
- Deterministic graph extraction from indexed chunks.
- Cloud-model graph extraction prompt packaging.
- File-backed graph persistence for local development.
- Graph neighbor expansion for graph+RAG retrieval traces.

## crates/agent-memory

Durable memory production, recall, and restore-pack boundary.

Responsibilities:

- Build deterministic session checkpoints from event logs.
- Preserve requirements from incomplete runs without promoting unfinished work
  to completed outcomes.
- Rank lexical and semantic recall under trust, source-diversity, deduplication,
  and observed-use constraints.
- Extract current goal, completed work, pending actions, decisions, tool
  results, retrievals, artifacts, and errors.
- Generate markdown restore packs for context recovery.
- Keep the MVP independent from local model summarization.

## crates/agent-runtime

Agent loop boundary.

Responsibilities:

- Package user prompts, available tools, assistant tool calls, and tool
  observations for model turns.
- Normalize provider tool calls into local tool invocations.
- Preserve canonical assistant/tool transcript messages for resume.
- Own run budgets, cancellation, no-progress and repeated-action guards.
- Own stage budgets, terminal reserves, partial-output retention, and the
  objective-aware context governor.
- Track loop states such as completed, tool requested, recoverable turn-budget
  exhaustion, and failed.
- Keep desktop storage, permission UI, and actual tool execution outside the
  pure runtime core.

## crates/model-provider

Cloud model abstraction.

Responsibilities:

- Provider capability model.
- Model request/response contracts.
- Streaming and non-streaming interfaces.
- Tool-call payload compatibility.
- Provider metadata.

Initial implementation target:

- OpenAI-compatible HTTP API.

## crates/orchestrator

Model and workflow coordination.

Responsibilities:

- Select orchestration policy.
- Build execution plans.
- Support `single`, `plan_execute_review`, `best_of_n`, and `auto_router`.
- Explain routing decisions and preserve override metadata.
- Train a table-based learned router from routing telemetry.
- Compare router decisions against baseline policies.
- Run the Fugu bounded multi-model workflow engine with dependency isolation,
  role-aware workers, verification, recovery, and resumable checkpoints.
- Own the durable workflow task graph, including runnable, resumable, degraded,
  exhausted, and dependency-blocked frontier semantics.
- Apply capability-aware evidence budgets so text-only Fast stays direct while
  unknown-workspace exploration can complete a bounded discovery/read chain.
- Evaluate and promote GEPA prompt genomes from completed or replayed evidence;
  never mutate an active runtime loop.

Non-responsibilities:

- Tool execution.
- Permission decisions.
- Provider-specific HTTP behavior.
- Active-run cancellation, permission state, or tool side effects.

## Desktop harness services

`apps/desktop/src-tauri/src/queue_service.rs` owns queue message contracts and
event reduction. `session_projection.rs` owns the versioned per-session read
model and incremental projection. `run_lifecycle.rs` is the shared UI-facing
lifecycle vocabulary. `permission_service.rs` owns indexed session permission
lookup and permission labels. `collaboration_service.rs` owns the desktop-facing
Fugu worker contracts, continuation budgets, evidence envelopes, and reserved
final-answer turn policy. `prompt_evaluation_runtime.rs` executes the isolated
evaluation DAG; `prompt_evaluation_feedback.rs` converts completed traces into
redacted actionable GEPA feedback without owning execution. These modules keep
Tauri command handlers thin while leaving provider calls, tool side effects, and
persistence in the desktop adapter.

## crates/tools

Local tool system.

Responsibilities:

- Tool registry.
- Tool specs.
- Permission planning for tools.
- Execution boundary for local tools.

Initial tools:

- file read
- file write
- directory list
- file search
- shell run
- web search
- URL fetch
- browser open/text/capture/action/tab tools
- computer screenshot/action tools

## Sidecars

Sidecars are small local controller processes behind explicit JSON protocols.

Current sidecars:

- Bundled Node browser controller using CDP sessions and Playwright semantics.
- Bundled Node/macOS computer shim for screenshot/click/type/key requests.

Planned sidecars:

- LanceDB RAG sidecar behind the existing RAG adapter contract.
- Swift/Rust computer-use helper behind the existing computer JSON action
  contract.

## Projects and Sessions

Projects and sessions are desktop navigation state stored outside the core
event schema.

Responsibilities:

- Persist project roots and session labels in `.cindx/projects.conf`.
- Keep the active project synchronized with the active workspace root.
- Stamp agent run metadata with active project/session ids and names.
- Feed the left sidebar with real selectable rows.

Non-responsibilities:

- Agent loop budgets or orchestration policy.
- Replaying unrelated sessions to construct the active session view.
