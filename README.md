# Cindx

Cindx is a macOS desktop agent runtime. Models provide reasoning; the local
application owns execution, permissions, persistence, retrieval, memory, and
the audit trail.

## What Ships

- Projects, sessions, streamed chat, attachments, artifacts, previews, search,
  schedules, queue/steer controls, and trace inspection.
- Fast, Auto, and Pro execution modes on one shared agent kernel.
- Permission-gated file, shell, process, web, browser, computer, image, MCP,
  and skill tools.
- Durable SQLite state, recoverable run identities, cancellation, retries, and
  permission suspension/resume.
- Workspace retrieval across semantic, file, graph-direct, and graph-walk
  channels, plus a separate durable memory system.
- Background, evidence-gated prompt evolution for future runs.

Fast is the direct single-model path. Auto and Pro ask a configured Conductor
for a typed direct-or-workflow decision; Pro has a larger bounded collaboration
budget. All modes use the same permission, tool, persistence, and completion
paths.

The checked-in evidence does **not** prove that Auto or Pro generally outperform
Fast, that GEPA currently improves production quality, or that Cindx matches
Fugu Ultra. The exact boundary is documented in
[Evaluation](docs/EVALUATION.md).

## Repository Map

```text
apps/desktop/             React UI and Tauri composition root
crates/agent-core/        Shared transport-free contracts
crates/agent-runtime/     Kernel, run control, context, task, and tool loop
crates/agent-application/ Run/reprepare driver
crates/agent-harness/     Active-run and exclusive-work registries
crates/orchestrator/      Conductor decisions, workflows, task graph, evolution
crates/agent-memory/      Durable memory production, retention, and recall
crates/agent-rag/         Workspace indexing and vector retrieval
crates/agent-graph/       Graph extraction and graph-guided retrieval
crates/agent-storage/     SQLite persistence
crates/model-provider/    Provider transport
crates/tools/             Tool contracts and portable implementations
crates/agent-mcp/         MCP transport and catalog adapter
crates/agent-skills/      Skill discovery, trust, and loading
crates/orchestrator-eval/ Non-shipping evaluation harness
benchmarks/               Versioned deterministic contracts
scripts/                  Checks, builds, and release helpers
```

## Maintained Documents

- [Current product facts](docs/CURRENT.md)
- [Architecture and ownership](docs/ARCHITECTURE.md)
- [Development, verification, and release](docs/DEVELOPMENT.md)
- [Evaluation results and claim boundary](docs/EVALUATION.md)
- [Current coding-agent handoff](docs/HANDOFF.md)

These are the only maintained project documents. Historical reports and old
release notes remain available in Git history; they are not current product
documentation.

## Development

```sh
cd apps/desktop
npm ci
cd ../..

node scripts/check-docs.mjs
scripts/check.sh
scripts/check-frontend.sh
scripts/check-desktop.sh
```

See [Development](docs/DEVELOPMENT.md) before running a provider evaluation or
production build. Provider-backed evaluations are explicit billable operations
and are never implied by a green deterministic test run.

## Releases

- [GitHub Releases](https://github.com/DaleXiao/Cindx/releases)
- [Cindx v0.2.30](https://github.com/DaleXiao/Cindx/releases/tag/v0.2.30)

The current published asset is `Cindx-0.2.30-macOS-arm64.zip`.
