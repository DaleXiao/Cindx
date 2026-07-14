# Roadmap

## Phase 0: Spec and Boundaries

- Write MVP spec.
- Create Rust workspace skeleton.
- Define crate responsibilities.
- Confirm local toolchain gates.

Exit criteria:

- Repository has docs and module skeletons.
- Missing prerequisites are explicit.

## Phase 1: Compilable Kernel Skeleton

- Install Rust.
- Compile empty workspace.
- Add tests for core domain types.
- Add SQLite dependency decision.

Exit criteria:

- `cargo test` passes for all initial crates.

## Phase 2: Desktop Shell

- Scaffold Tauri app.
- Add frontend task/chat layout.
- Connect frontend to a Rust command.
- Add settings placeholder.

Exit criteria:

- Desktop app launches locally.
- `scripts/check-desktop.sh` passes.

## Phase 3: Event Log and Permissions

- Implement SQLite event store.
- Implement permission request lifecycle.
- Render approval UI.
- Persist decisions.

Exit criteria:

- User can approve/deny a mock tool call and see persisted audit records.

Status:

- Implemented SQLite-backed events and permission audits in `agent-storage`.
- Added desktop commands for Phase 3 state, mock permission requests, and
  permission resolution.
- Wired the React permission panel to persisted audit records.
- Verification target: `scripts/check.sh`, `scripts/check-frontend.sh`,
  `scripts/check-desktop.sh`, and Tauri `cargo test`.

## Phase 4: Cloud Model Provider

- Implement OpenAI-compatible provider.
- Add API key config.
- Add model role config.
- Add request/response event logging.

Exit criteria:

- User can send a prompt and receive a streamed answer through the kernel.

Status:

- Implemented an OpenAI-compatible `chat/completions` provider using the system
  `curl` binary and streaming response parsing.
- Added local provider config persistence for base URL, API key, default model,
  and role-specific models.
- Added desktop commands for reading/saving provider config and sending a model
  prompt through the Rust kernel.
- Added Tauri stream events so the UI can render assistant deltas while the
  request is running.
- Model request start, finish, user messages, assistant messages, and errors are
  recorded in the SQLite event log.

## Phase 5: File and Shell Tools

- Implement read/list/search files.
- Implement write file.
- Implement shell run.
- Require permission for write and execute.
- Record tool results.

Exit criteria:

- Agent can inspect a workspace, propose a change, request approval, and apply
  it.

Status:

- Implemented real workspace-scoped `file.read`, `file.list`, `file.search`,
  `file.write`, and `shell.run` tools in the `tools` crate.
- Write and shell tools create permission requests before execution.
- Added desktop commands for reading tool state, running tools, and resolving
  tool permissions.
- Tool proposals, starts, finishes, denied calls, outputs, and errors are
  recorded in the SQLite event log.
- Added a desktop tool runner UI with pending approval and recent result views.

## Phase 6: Orchestration v0

- Implement `single`.
- Implement `plan_execute_review`.
- Implement `best_of_n` contract.
- Store orchestration traces.

Exit criteria:

- User can choose orchestration mode per task.

Status:

- Added stable policy parsing and step prompt packaging to the `orchestrator`
  crate.
- Added desktop commands to read orchestration state and run `single`,
  `plan_execute_review`, or `best_of_n`.
- Each orchestration step uses the configured role-specific model and records
  start, finish, role, model, latency, and output in the SQLite event log.
- Added a desktop workflow runner with policy selection and trace output.

## Phase 7: RAG v0

- Add LanceDB sidecar contract.
- Add file indexing.
- Add cloud embeddings.
- Add semantic search tool.
- Show source provenance.

Exit criteria:

- Agent answers workspace questions using indexed local chunks.

Status:

- Added `agent-rag` with a LanceDB-style adapter contract backed by a local
  persisted index for the MVP.
- Implemented workspace file indexing with chunk-level line provenance and
  local hybrid semantic/lexical scoring.
- Added grounded answer prompt packaging that cites `[path:start-end]` source
  ranges.
- Added desktop commands for RAG state, indexing, searching, and answering with
  the configured cloud model.
- Added a desktop RAG panel with index stats, source cards, and grounded answer
  output.

## Phase 8: Browser Tool v0

- Add Playwright sidecar contract.
- Open URL.
- Capture screenshot.
- Extract page text.
- Record browser observations.

Exit criteria:

- Agent can open and inspect a webpage with visible audit events.

Status:

- Added network-gated `web.search`, `browser.open`, `browser.extract_text`, and
  `browser.capture` tools.
- Added a Playwright CLI sidecar contract for screenshot capture with local
  HTML/text snapshot fallback when Playwright is not installed.
- Browser observations write tool proposal, permission, execution, output, URL,
  capture kind, and artifact paths into the SQLite event log.
- Added desktop Phase 8 commands for state, browser tool execution, and
  permission resolution.
- Added a Browser panel with web search, open, text extraction, capture,
  approval, and observation output.

## Phase 9: Cloud Embeddings and LanceDB-Ready RAG

- Add embedding request/response contracts.
- Add OpenAI-compatible embeddings client support.
- Add external-embedding RAG indexing.
- Add LanceDB-ready record schema.
- Preserve local hash embedding fallback.

Exit criteria:

- RAG can index chunks using provider-supplied vectors and expose records ready
  for a LanceDB backend.

Status:

- Added embedding request/response contracts and parsing to `model-provider`.
- Added OpenAI-compatible `/embeddings` request construction and provider
  method.
- Added external embedding indexing to `agent-rag` through a `RagEmbedder`
  trait.
- Added embedding provenance fields to every RAG chunk: provider, model, and
  dimensions.
- Added LanceDB-ready record projection and JSONL export while keeping the
  existing local file adapter as fallback.
- Added desktop provider config for the embedding model and provider-backed RAG
  indexing when cloud credentials are configured.

## Phase 10: Graph Extraction

- Add graph node and edge contracts.
- Add deterministic local extractors.
- Add model-packaged extraction prompts.
- Store provenance for graph facts.

Exit criteria:

- Workspace content can produce graph entities and relations with source
  provenance.

Status:

- Added `agent-graph` with graph node, edge, provenance, extraction, and store
  contracts.
- Added deterministic extraction for files, tools, symbols, decisions, claims,
  and tasks from indexed RAG chunks.
- Added graph extraction prompt packaging for cloud-model extraction passes.
- Added a TSV-backed graph store with upsert, load, save, neighbor lookup, and
  label lookup.
- RAG indexing now writes `.cindx/graph.tsv` alongside the RAG index.

## Phase 11: Graph + RAG Walk

- Seed retrieval from vector search.
- Expand through graph neighbors.
- Rank combined vector and graph sources.
- Show retrieval trace in the UI.

Exit criteria:

- A question can retrieve vector seeds, graph neighbors, and final cited source
  chunks.

Status:

- Added graph-guided expansion over vector seed files and related graph nodes.
- Added inspectable graph RAG traces with `vector_seed`, `graph_neighbor`, and
  selected source sets.
- Desktop RAG search and answer now use graph+RAG traces when a graph store is
  present.
- Source cards now show why each source was selected.

## Phase 12: Browser Use v1

- Add Playwright JSON sidecar.
- Support open, click, type, scroll, screenshot, and text extraction.
- Gate browser actions through permissions.
- Record browser artifacts.

Exit criteria:

- Agent can operate a webpage with visible action and observation audit events.

Status:

- Added audited browser action tools: `browser.click`, `browser.type`, and
  `browser.scroll`.
- Added `CINDX_BROWSER_SIDECAR` JSON command protocol with local request
  artifact fallback under `.cindx/browser-actions`.
- Existing browser open, extract, search, and capture tools remain
  permission-gated and write observations to the event log.
- Desktop Browser panel exposes search, open, extract, capture, click, type,
  and scroll actions with approval and observation views.

## Phase 13: Computer Use v1

- Add screenshot/action protocol.
- Support click, type, key, and scroll.
- Add redaction and sensitive-region hooks.
- Add approval UI for proposed computer actions.

Exit criteria:

- Agent can complete a simple local app workflow with user-approved actions and
  visual audit artifacts.

Status:

- Added audited computer-use tools: `computer.screenshot`, `computer.click`,
  `computer.type`, `computer.key`, and `computer.scroll`.
- Added `CINDX_COMPUTER_SIDECAR` JSON command protocol with local request
  artifact fallback under `.cindx/computer-actions`.
- `computer.screenshot` can use a sidecar, macOS `screencapture`, or a
  placeholder artifact, and always writes a redaction manifest.
- Computer actions use sensitive or destructive permission risks and flow
  through the existing desktop tool approval queue.

## Phase 14: Learned Model Router

- Add routing telemetry.
- Add rule-based router.
- Build offline evaluation traces.
- Add learned router only after enough labeled data exists.

Exit criteria:

- Router can explain model and policy choices, and users can override them.

Status:

- Added task classification, model candidates, routing context, routing
  decisions, routing telemetry, and baseline evaluation reports to
  `orchestrator`.
- Added a rule-based router that chooses policy, model, retrieval mode, and
  verifier with a human-readable explanation.
- Added a learned table router trained from successful routing telemetry, with
  rule-based fallback when no local evidence exists.
- Desktop orchestration exposes `auto_router`; explicit policies remain user
  overrides.
- Orchestration events record requested policy, selected policy, router model,
  retrieval mode, task class, and router explanation for future evaluation.

## Phase 15: Context Manager v0

- Add deterministic session checkpoint generation from the local event log.
- Persist a restore context pack under the active workspace.
- Expose manual context compaction in Settings.
- Preview agent restore context in the artifact sidebar.

Exit criteria:

- User can compact the current session and inspect a local restore pack without
  requiring a local model.

Status:

- Added `agent-memory` with event-log checkpoint extraction and markdown
  restore pack generation.
- Added desktop `get_context_state` and `compact_context` commands.
- Context checkpoints are written to `.cindx/context-checkpoint.md` in
  the active workspace.
- The desktop Settings view exposes context stats and manual compaction; the
  right artifact sidebar previews the restore pack.

## Phase 16: Real Agent Loop v0

- Add a runtime loop that packages prompts, tools, model responses, and tool
  observations.
- Extend the OpenAI-compatible provider with non-streaming tool-call support.
- Let the chat composer start an audited agent task rather than only a model
  chat turn.
- Pause on risky tool calls and resume after user approval.

Exit criteria:

- A user prompt can cause the model to request a local tool, the desktop app can
  execute read-only tools immediately, and risky tools can wait for review then
  continue after approval.

Status:

- Added `agent-runtime` with prompt packaging, tool-call normalization,
  observation messages, and loop state tests.
- `model-provider` now emits OpenAI-compatible `tools`, parses `tool_calls`, and
  supports non-streaming tool-call requests.
- Added desktop `run_agent_task`, `get_agent_state`, and
  `resolve_agent_permission` commands with Phase 16 event logging.
- The composer now runs the agent loop; Settings and the right sidebar include
  agent permission review state.

## Phase 17: Agent Transcript and Control v0

- Store assistant tool-call messages and tool observation messages in the event
  log as canonical transcript entries.
- Resume the agent loop from stored transcript messages rather than rebuilt
  observation summaries.
- Isolate the active run from older events and stale pending approvals.
- Add cooperative cancel and retry commands plus composer/right-sidebar UI
  controls.

Exit criteria:

- Permission continuations preserve the assistant `tool_calls` to `tool` role
  chain expected by OpenAI-compatible chat APIs.
- Agent state reports active status, turn count, transcript message count,
  retry eligibility, and cancel eligibility.
- A cancelled, failed, or completed task can be retried from the last prompt.

Status:

- `model-provider` now re-encodes assistant messages with raw `tool_calls` and
  matching `tool` messages.
- `agent-runtime` can resume from canonical messages and append tool
  observations with `tool_call_id`.
- Desktop state rebuilds active-run transcript from events, filters stale
  approvals, and exposes `cancel_agent_task` / `retry_agent_task`.
- The UI shows agent status, turn count, transcript size, and cancel/retry
  controls.

## Phase 18: Agent Trace Observability v0

- Derive structured trace turns and steps from the active agent event log.
- Preserve raw event metadata while also normalizing step kind, status, latency,
  model, tool, request, permission, input preview, output preview, and artifact.
- Add a Trace Session view for turn-by-turn inspection.
- Add a right-sidebar Trace Inspector for selected step details.
- Export the active trace as workspace-local JSONL.

Exit criteria:

- Agent runs are inspectable as grouped turns and individual steps.
- A selected step exposes raw metadata, identifiers, previews, latency, and
  artifact paths.
- `.cindx/agent-trace.jsonl` can be generated from the current active run.

Status:

- Added desktop `get_agent_trace_state` and `export_agent_trace_jsonl`.
- Added event-log trace aggregation and JSONL export helpers.
- Added React trace types, Trace Session UI, Trace Inspector, and export
  control.
- Added tests for trace grouping and export.

## Phase 19: Real Sidecars and Native Control v0

- Bundle local browser/computer sidecar shims in the repository.
- Load sidecar config at desktop startup and automatically configure tool
  environment variables.
- Expose sidecar health and paths in Settings.
- Keep sidecars replaceable so Playwright and Swift/native helpers can be
  dropped in later.

Exit criteria:

- Browser/computer tools can run a configured sidecar without manual env setup.
- Settings reports sidecar path, env key, health, and auto-configure state.
- Tests cover configured sidecar execution and default sidecar health.

Status:

- Added `scripts/sidecars/browser-sidecar.js` and
  `scripts/sidecars/computer-sidecar.js`.
- Added desktop `get_sidecar_state` and `save_sidecar_config`.
- Desktop startup now applies `CINDX_BROWSER_SIDECAR` and
  `CINDX_COMPUTER_SIDECAR` from persisted config.
- Settings includes sidecar path editing, health output, and auto-configure.

## Phase 20: Projects and Sessions v0

- Persist desktop project/session state in `.cindx/projects.conf`.
- Expose project/session create and select commands through the Tauri bridge.
- Replace static left-sidebar demo rows with real project/session rows.
- Keep active project selection synchronized with the active workspace root.
- Add active project/session metadata to agent run events, state, and trace.

Exit criteria:

- The left sidebar can create/select projects and sessions.
- Selecting a project or session updates the active workspace.
- Agent output and trace state can identify the active project/session.

Status:

- Added `ProjectSessionConfig`, project/session state views, and Tauri
  commands for get/create/select.
- Added React bridge types and left-sidebar controls.
- Added project/session chips to agent output and trace/topbar context.
- Added tests for default project/session state and agent context attribution.

## Phase 21: Cindx Product Rename

- Rename all user-facing product branding to Cindx.
- Rename the Tauri product, window title, bundle identifier, npm package, and
  Rust desktop package.
- Rename runtime environment variables to `CINDX_*` and workspace-local data
  to `.cindx`.
- Update default project identity, prompts, fixtures, tests, and docs.

Exit criteria:

- No old product identity remains in source-controlled files.
- All checks pass and the macOS bundle is generated as `Cindx.app`.

## Phase 22: Browser Control v2

- Replace the browser shim with a real reusable Chromium controller.
- Use CDP for browser sessions, tabs, target ids, and event telemetry.
- Use Playwright for semantic locators, frames, auto-waiting, downloads, and
  screenshots.
- Return browser traces and output files through the existing tool artifact
  contract.
- Enforce permission review, cancellation, timeouts, workspace path
  containment, and trace redaction.

Exit criteria:

- One browser session survives separate tool invocations and supports multiple
  tabs and frames.
- Semantic click/type actions, download capture, screenshot capture, and text
  extraction pass against a real browser fixture.
- The bundled app contains the browser controller and pinned Playwright client.
- Computer Use remains an independent native/accessibility channel.

Status:

- Added the `cindx.browser-control.v2` request, response, and trace protocols.
- Added a dedicated Chromium profile connected through CDP and controlled by
  Playwright locators and waits.
- Added `browser.tabs` and `browser.select_tab` beside the existing browser
  tools.
- Added structured traces, artifacts, cooperative cancellation, hard timeouts,
  and workspace-constrained paths.
- Added a real Chromium integration fixture to local checks, builds, and CI.
