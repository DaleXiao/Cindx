# MVP Spec v0.1

## Product Thesis

Build a desktop-first macOS local agent that uses cloud models for reasoning
while local runtime code owns execution, permissions, memory, retrieval, audit
logs, and user-visible context boundaries.

The MVP is not a general chatbot. It is a trusted local agent runtime with a
chat interface.

## Primary User Outcome

A user can open the desktop app, configure a cloud model API, choose a
workspace, ask the agent to perform a local task, inspect proposed tool calls,
approve or deny risky actions, and see a persistent event trail of what
happened.

## In Scope

### Desktop App

- Tauri shell with a web frontend.
- Chat and task timeline.
- Tool-call activity panel.
- Permission prompt UI.
- Settings for provider config, workspace roots, permissions, and memory.

### Agent Kernel

- Task runtime.
- Event log.
- Tool registry.
- Permission manager.
- Cancellation and retry.
- Structured tool-call validation.
- Context packaging for cloud model calls.

### Cloud Model Layer

- OpenAI-compatible provider v0.
- Provider registry.
- Model role configuration:
  - planner
  - executor
  - reviewer
  - summarizer
- Request/response metadata:
  - provider
  - model
  - role
  - latency
  - token usage when available
  - request id when available

### Multi-Model Orchestration v0

- `single`: one model handles the interaction.
- `plan_execute_review`: planner creates a plan, executor acts, reviewer checks.
- `best_of_n`: multiple candidates are generated, then reviewed or verified.

The MVP does not include a learned Fugu-style router. The event log should still
capture enough data to train or tune routing policies later.

### Core Tools

- Read file.
- Write file.
- List directory.
- Search files.
- Run shell command.
- Web search.
- Fetch URL.
- Browser open/screenshot v0.

All local tools go through the Rust kernel and permission manager. The model
never receives raw system access.

### Memory v0

- SQLite-backed conversations.
- SQLite-backed events.
- Tool-call and permission audit records.
- Project memory entries:
  - preference
  - convention
  - path
  - decision
- Every memory entry must include provenance.

### RAG v0

- LanceDB sidecar or adapter.
- File chunking.
- Cloud embedding API.
- Semantic search tool.
- Chunk metadata:
  - path
  - file hash
  - modified time
  - byte range or line range
  - indexed time

## Out of Scope

- Local LLM runtime.
- Full computer use.
- Graph extraction.
- Graph-guided RAG traversal.
- Plugin marketplace.
- Long-running automation scheduler.
- App Store distribution.
- Learned model-routing system.

## Permission Model

The MVP uses explicit risk classes:

- `read`: local read-only context access.
- `write`: creates or modifies files.
- `execute`: runs a local process.
- `network`: sends or fetches network data.
- `sensitive`: may expose secrets, private user data, or credentials.
- `destructive`: deletes, overwrites, resets, or otherwise risks data loss.

The permission manager must be able to:

- allow
- deny
- request user approval
- remember scoped decisions
- record the decision in the event log

## Privacy Boundary

The UI must make cloud-bound context inspectable. At minimum, each model request
must record references to the source data sent to the provider:

- user message
- retrieved chunks
- file snippets
- shell output
- web content
- tool results

The MVP can store full request payloads locally behind a setting, but it must
store enough metadata to audit what categories of data were sent.

## Definition of Done

The MVP is done when a user can:

1. Launch the desktop app.
2. Configure an OpenAI-compatible cloud model API.
3. Choose a workspace.
4. Ask the agent to inspect and change files.
5. See permission prompts for risky actions.
6. Approve or deny tool calls.
7. See a persistent timeline of messages, model calls, tool calls, and results.
8. Run a `plan_execute_review` workflow.
9. Index local files into RAG.
10. Ask a question that uses semantic retrieval with source provenance.
