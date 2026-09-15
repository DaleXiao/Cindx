# Cindx

Cindx is a macOS desktop agent runtime. Models provide reasoning; the local
application owns execution, permissions, persistence, retrieval, memory, and
the audit trail.

## What Ships

- Projects, sessions, streamed chat, attachments, artifacts, previews, search,
  schedules, queue/steer controls, and trace inspection.
- Fast, Default, High, and Extra High execution modes on one shared agent kernel.
- Permission-gated file, shell, process, web, browser, computer, image, MCP,
  and skill tools.
- Durable SQLite state, recoverable run identities, cancellation, retries, and
  permission suspension/resume.
- Workspace retrieval across semantic, file, graph-direct, and graph-walk
  channels, plus a separate durable memory system.

Fast, Default, High, and Extra High are effort tiers planned deterministically by
the run preparation: a fixed single-model plan per tier with no planning model
call. Fast is a quick direct answer; Default, High, and Extra High add a bounded
delivery judge and progressively larger budgets. All modes use the same
permission, tool, persistence, and completion paths.

The checked-in evidence does **not** prove that Default, High, or Extra High
generally outperform Fast, or that any prompt-evolution or multi-model
collaboration improves production quality. Deterministic gates prove contracts,
not quality uplift; provider-backed claims require the explicit evaluation
protocol described in [Development](docs/DEVELOPMENT.md).

## Repository Map

```text
apps/desktop/             React UI and Tauri composition root
crates/agent-core/        Shared transport-free contracts
crates/agent-runtime/     Kernel, run control, context, task, and tool loop
crates/agent-application/ Run/reprepare driver
crates/agent-harness/     Active-run and exclusive-work registries
crates/agent-memory/      Durable memory production, retention, and recall
crates/agent-rag/         Workspace indexing and vector retrieval
crates/agent-graph/       Graph extraction and graph-guided retrieval
crates/agent-storage/     SQLite persistence
crates/model-provider/    Provider transport
crates/tools/             Tool contracts and portable implementations
crates/agent-mcp/         MCP transport and catalog adapter
crates/agent-skills/      Skill discovery, trust, and loading
benchmarks/               Versioned deterministic contracts
scripts/                  Checks, builds, and release helpers
```

## Maintained Documents

- [Current product facts](docs/CURRENT.md)
- [Architecture and ownership](docs/ARCHITECTURE.md)
- [Development, verification, and release](docs/DEVELOPMENT.md)

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

The current release identity (version, tag, asset name/size/SHA-256) is
published on the release page itself. Do not commit application archives to
the Git tree; publish them as GitHub Release assets.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

See [SECURITY.md](SECURITY.md). Please report vulnerabilities through GitHub
private vulnerability reporting rather than public issues.

## License

[Apache-2.0](LICENSE)
