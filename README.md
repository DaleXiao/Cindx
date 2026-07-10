# Cindx

Desktop-first local agent runtime for macOS.

The product goal is a trusted local execution environment: cloud models do
reasoning, while the local app owns tools, permissions, memory, retrieval,
browser automation, and audit logs.

## Download

The current internal test build is available for Apple Silicon Macs:

- [Cindx 0.0.4 for macOS (arm64)](releases/Cindx_0.0.4_aarch64.zip)
- [Installation and verification notes](releases/README.md)
- [Automated builds and releases](docs/RELEASING.md)

This build is ad-hoc signed but not Apple-notarized. macOS may require the
one-time Gatekeeper steps documented with the package. Intel Macs need a
separate `x86_64` build.

## Current Status

This repository has a runnable desktop MVP:

- Tauri macOS app with a Rust command bridge.
- OpenAI-compatible cloud model config and streaming chat.
- SQLite event log and permission audit trail.
- Permission-gated file, shell, web, browser, and computer-use tools.
- Bundled browser/computer sidecar shims with Settings health checks and
  automatic tool environment configuration.
- Persistent desktop projects and sessions with real left-sidebar selection.
- Multi-model orchestration with manual policies and `auto_router`.
- A real agent loop that lets models request local tools and resumes after
  permission review with canonical assistant/tool transcript recovery.
- Structured agent trace observability with turn/step inspection and JSONL
  export.
- Workspace-selectable RAG with cloud embeddings, LanceDB-ready export, graph
  extraction, graph+RAG walk, and source provenance.
- Event-log context checkpoints with local restore pack preview and manual
  compaction.

Known machine gate:

- Rust is installed through Homebrew `rustup` and the stable toolchain.
- Node, npm, Python, Git, SQLite, Homebrew, and Apple Command Line Tools are
  already present on the target machine.
- Homebrew's formula for the Rust installer is `rustup`, not `rustup-init`.
- Because Homebrew `rustup` is keg-only, use `scripts/check.sh` or add the Rust
  paths from [docs/SETUP.md](docs/SETUP.md) to your shell.

## MVP

The MVP is intentionally narrow:

- Tauri desktop shell.
- Rust agent kernel.
- Cloud model provider abstraction.
- Multi-model orchestration v0.
- Permission-managed file and shell tools.
- SQLite event log.
- LanceDB-backed RAG sidecar.
- Browser tool v0.

See [docs/MVP_SPEC.md](docs/MVP_SPEC.md).
See [docs/SETUP.md](docs/SETUP.md) for local setup gates.

## Repository Layout

```text
apps/
  desktop/              Tauri desktop app.
crates/
  agent-core/           Shared domain types and contracts.
  agent-graph/          Graph extraction and graph+RAG traversal.
  agent-memory/         Event-log checkpoints and restore context packs.
  agent-mcp/            MCP transports, catalog cache, and remote tools.
  agent-rag/            Local RAG adapter, indexing, and search.
  agent-runtime/        Model-tool-observation agent loop.
  agent-skills/         Skill discovery, trust, and progressive loading.
  agent-storage/        Event log and state persistence.
  model-provider/       Cloud model provider abstraction.
  orchestrator/         Routing, learned router, and workflow planning.
  tools/                Local, web, browser, and computer tool registry.
docs/
  adr/                  Architecture decision records.
```

## Development Sequence

1. Install Rust.
2. Validate the Rust kernel workspace and desktop structure with `scripts/check.sh`.
3. Scaffold the Tauri desktop app.
4. Implement SQLite event log.
5. Implement model provider v0.
6. Implement file and shell tools behind permissions.
7. Add LanceDB sidecar and retrieval tool.
8. Add browser tool v0.
9. Add cloud embeddings, graph extraction, graph+RAG, browser/computer action
   protocols, and learned routing.
10. Add event-log context compaction and restore pack previews.
11. Add the real model-tool-observation agent loop.
12. Persist canonical agent transcripts and add cancel/retry controls.
13. Add structured agent trace observability and JSONL export.
14. Add bundled sidecar configuration and health checks for browser/computer
    control.
15. Add persistent project/session state and bind agent runs to the active
    desktop context.
16. Rename the product, desktop bundle, runtime namespace, and local data
    directory to Cindx.

Desktop checks require npm dependencies:

```sh
scripts/install-desktop-deps-ipv4.sh
scripts/check-frontend.sh
scripts/fetch-desktop-rust-deps.sh
scripts/check-desktop.sh
```
