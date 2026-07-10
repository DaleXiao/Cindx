# Post-MVP Spec v0.2

## Product Direction

The MVP proved the local runtime boundary: the desktop app owns workspace
selection, permissions, event logs, tools, model calls, RAG, and browser
observations. The next phases turn that runtime into a durable research agent:
better retrieval, richer observations, graph memory, computer use, and adaptive
model routing.

## Phase 9: Cloud Embeddings and LanceDB-Ready RAG

Goal:

- Replace local hash embeddings with provider-backed embeddings.
- Keep the existing local file adapter as the offline fallback.
- Add a LanceDB-ready schema so the storage backend can be swapped without
  changing the desktop command surface.

Scope:

- Add embedding request/response types to `model-provider`.
- Add OpenAI-compatible `/embeddings` client support.
- Add external-embedding indexing entry points to `agent-rag`.
- Add a record shape with stable fields for LanceDB and future hybrid search.
- Store embedding provider/model/dimension metadata with every indexed chunk.

Exit criteria:

- RAG can index chunks using caller-provided embedding vectors.
- Embedding request JSON and response parsing are covered by tests.
- RAG chunks can be projected into LanceDB-ready records.

Implementation snapshot:

- Implemented with `model-provider` embeddings support, `agent-rag`
  `RagEmbedder`, desktop embedding-model config, and JSONL export for
  LanceDB-ready records.

## Phase 10: Graph Extraction

Goal:

- Extract entities, claims, files, tools, decisions, and relations from local
  context into a graph-ready store.

Scope:

- Add `agent-graph` crate with node and edge contracts.
- Add extraction prompt packaging for cloud models.
- Add deterministic local extractors for file path, symbol, task, and tool
  references.
- Store provenance for every node and edge.

Exit criteria:

- A workspace question can produce graph entities with source provenance.
- The graph store supports upsert and neighbor lookup.

Implementation snapshot:

- Implemented in `agent-graph` with deterministic extraction, prompt
  packaging, TSV graph persistence, upsert, neighbor lookup, and label lookup.

## Phase 11: Graph + RAG Walk

Goal:

- Combine vector search with graph expansion before answer generation.

Scope:

- Start with vector search seeds.
- Expand over related files, entities, decisions, and tool results.
- Rank graph-neighbor chunks using score, recency, and provenance strength.
- Produce an inspectable retrieval trace.

Exit criteria:

- A query returns vector seeds, graph neighbors, and final selected sources.
- The UI shows why each source was included.

Implementation snapshot:

- Desktop RAG search and answer now use graph+RAG traces when the graph store
  exists, and source cards expose inclusion reasons.

## Phase 12: Browser Use v1

Goal:

- Move from URL fetch/capture to an inspectable browser controller.

Scope:

- Add a Playwright sidecar process with an explicit JSON command protocol.
- Support open, click, type, scroll, extract text, and screenshot.
- Record browser observations as event-log artifacts.
- Gate navigation and form entry with network/sensitive permissions.

Exit criteria:

- The agent can inspect and operate a webpage through visible audited actions.

Implementation snapshot:

- Implemented browser action tools for click, type, and scroll with
  `CINDX_BROWSER_SIDECAR` JSON protocol and local artifact fallback.

## Phase 13: Computer Use v1

Goal:

- Operate local desktop UI with explicit screenshots, actions, and approvals.

Scope:

- Add a computer-use tool protocol: screenshot, click, type, key, scroll.
- Introduce sensitive-region and destructive-action risk classes.
- Store screenshots as local artifacts with redaction hooks.
- Add a UI review queue for proposed computer actions.

Exit criteria:

- The agent can perform a simple local app workflow with user-approved actions
  and a persistent visual audit trail.

Implementation snapshot:

- Implemented computer-use tools for screenshot, click, type, key, and scroll
  with sidecar/native/fallback screenshot artifacts, redaction manifests, and
  sensitive/destructive approvals.

## Phase 14: Learned Model Router

Goal:

- Use historical traces to choose model, policy, retrieval mode, and verifier.

Scope:

- Add routing telemetry: task class, selected policy, model, latency, outcome,
  cost proxy, tool count, retrieval count, and user overrides.
- Add a rule-based router first.
- Add offline evaluation datasets from local event logs.
- Add a learned router only after enough labeled traces exist.

Exit criteria:

- The router can explain why it chose a model/policy.
- Users can override routing decisions.
- Evaluation reports compare router choices against baseline policies.

Implementation snapshot:

- Implemented rule-based and learned-table routers with routing telemetry
  contracts, explanations, baseline evaluation reports, and desktop
  `auto_router` mode.

## Phase 15: Context Manager v0

Goal:

- Reduce context loss by turning the local event log into a compact restore
  pack that can be reloaded into a future agent session.

Scope:

- Add `agent-memory` as a deterministic checkpoint builder.
- Summarize current goal, completed steps, pending approvals, decisions, file
  changes, commands, tool results, retrievals, artifacts, errors, and next
  actions from events.
- Persist the restore pack in the active workspace.
- Add desktop commands and UI controls for viewing and compacting context.

Exit criteria:

- The desktop app can generate and preview a restore pack without a local LLM.
- Manual compaction records an audit event and writes a local checkpoint file.

Implementation snapshot:

- Implemented `agent-memory`, Tauri `get_context_state` and `compact_context`,
  `.cindx/context-checkpoint.md` persistence, Settings compaction, and
  right-sidebar restore pack preview.

## Phase 16: Real Agent Loop v0

Goal:

- Turn the desktop from adjacent model/tool panels into a real audited agent
  loop where the model can request tools and the local runtime executes them.

Scope:

- Add a pure `agent-runtime` crate for model request packaging, tool-call
  normalization, and observation handling.
- Extend `model-provider` with OpenAI-compatible `tools` request support and
  non-streaming `tool_calls` parsing.
- Add desktop commands to start agent tasks, expose state, and resume after
  permissions.
- Route the default chat composer through the agent loop.

Exit criteria:

- Read-only tools can execute inside the loop.
- Risky tools pause for permission and can resume after approval or denial.
- Agent loop events are visible in the timeline and available to context
  checkpoints.

Implementation snapshot:

- Implemented `agent-runtime`, provider tool-call support, Phase 16 Tauri
  commands, composer integration, agent approval UI, and event-log persistence.

## Phase 17: Agent Transcript and Control v0

Goal:

- Make the real agent loop recoverable and controllable across permission
  pauses by preserving the canonical assistant/tool transcript.

Scope:

- Persist assistant messages that contain raw OpenAI-compatible `tool_calls`.
- Persist tool observations as `tool` role messages with `tool_call_id`.
- Resume permission continuations from event-log transcript messages instead of
  reconstructing user observation summaries.
- Add active-run isolation so old pending approvals do not leak into new runs.
- Add desktop and UI controls for cooperative cancel and retry.

Exit criteria:

- A permission-approved tool result resumes with an assistant `tool_calls`
  message followed by a matching `tool` message.
- Agent state exposes status, turn count, transcript message count, retry
  availability, and cancel availability.
- Cancelled, failed, or completed agent tasks can be retried from the latest
  user prompt.

Implementation snapshot:

- Implemented provider request encoding for assistant `tool_calls`, runtime
  transcript resume helpers, desktop event-log transcript reconstruction,
  active-run status derivation, cancel/retry commands, UI controls, and tests.

## Phase 18: Agent Trace Observability v0

Goal:

- Make agent runs debuggable by turning the event log into a structured
  trajectory with turns, steps, raw metadata, latency, and export.

Scope:

- Derive trace turns and trace steps from the active agent run in the event
  log.
- Normalize model, tool, permission, message, status, retrieval, and error
  events into a common trace step shape.
- Expose trace state through a desktop command.
- Export the active trace to workspace-local JSONL for debugging and future
  evaluation.
- Add a Trace Session view and right-sidebar step inspector.

Exit criteria:

- The UI can show agent turn groups, step status, latency, tool/model IDs,
  previews, artifacts, and raw metadata.
- JSONL export writes `.cindx/agent-trace.jsonl`.
- Tests cover trace grouping and export.

Implementation snapshot:

- Added `AgentTraceState`, `get_agent_trace_state`,
  `export_agent_trace_jsonl`, event-log trace aggregation, JSONL export, Trace
  Session UI, Trace Inspector, and desktop tests.

## Phase 19: Real Sidecars and Native Control v0

Goal:

- Move browser/computer use from passive request artifacts toward configured
  local sidecar execution with visible health state.

Scope:

- Add bundled browser and computer sidecar scripts.
- Add sidecar config persistence and automatic environment setup for tool
  execution.
- Add health checks for configured sidecar paths.
- Expose sidecar state and config editing in Settings.
- Keep the sidecar shims replaceable by a later Playwright or Swift helper.

Exit criteria:

- Browser/computer tools can discover sidecars without manual shell env setup.
- Settings shows browser/computer sidecar path and health output.
- Tool tests verify configured sidecar execution.

Implementation snapshot:

- Added Node/macOS sidecar shims under `scripts/sidecars`, desktop
  `get_sidecar_state` and `save_sidecar_config`, automatic
  `CINDX_BROWSER_SIDECAR` / `CINDX_COMPUTER_SIDECAR` setup,
  Settings UI, and tests.

## Phase 20: Projects and Sessions v0

Goal:

- Turn the desktop-first shell into a multi-project, multi-session workspace
  instead of a single static demo surface.

Scope:

- Persist projects and sessions in workspace-local config.
- Expose project/session state plus create/select commands through Tauri.
- Drive the left sidebar from real persisted projects and sessions.
- Switch the active workspace when a project or session is selected.
- Stamp agent run events with the active project/session metadata for trace
  and output attribution.

Exit criteria:

- The left sidebar can create and select persisted projects and sessions.
- Agent state and trace state expose the project/session for the active run.
- Workspace selection and project selection stay synchronized.

Implementation snapshot:

- Added `.cindx/projects.conf`, `get_project_session_state`,
  `create_project`, `create_session`, `select_project`, and `select_session`.
- Replaced static sidebar demo lists with persisted project/session state and
  compact creation controls.
- Added active project/session metadata to agent run events, state, and trace.
- Added Phase 20 desktop tests for default state and agent context attribution.

## Phase 21: Cindx Product Rename

Goal:

- Establish Cindx as the single product identity across the desktop app,
  runtime, documentation, and local namespaces.

Scope:

- Rename the macOS product and window title to Cindx.
- Rename desktop npm and Rust packages to `cindx-desktop`.
- Move runtime environment variables to the `CINDX_*` namespace.
- Move workspace-local application data from the legacy hidden directory to
  `.cindx`.
- Update prompts, default projects, tests, documentation, and browser preview
  fixtures.

Exit criteria:

- Source scans contain no old product-name or namespace references.
- The Rust workspace, frontend, and desktop tests pass.
- Tauri produces `Cindx.app` with identifier `app.cindx.desktop`.

## Non-Goals

- No local LLM runtime requirement.
- No invisible automation.
- No unaudited access to files, network, browser, or desktop UI.
- No graph or router output without provenance.
