# Cindx

Cindx is a macOS desktop agent runtime. Cloud models provide reasoning; the
local application owns tool execution, permissions, durable state, retrieval,
memory, browser/computer control, and the audit trail.

## Documentation Baseline

Start with these documents instead of inferring the product from historical
reports:

- [Current product baseline](docs/CURRENT.md)
- [Current architecture and module relationships](docs/ARCHITECTURE.md)
- [Agent evaluation policy and current evidence](docs/AGENT_EVALUATION.md)
- [Documentation index and maintenance rules](docs/README.md)

Historical evaluation reports are evidence for their recorded versions only.
They are indexed under [docs/evaluations](docs/evaluations/README.md) and do not
describe the current implementation unless a current document cites them.

## Current Product

The current desktop application provides:

- A Tauri 2 macOS application with persistent projects, sessions, messages,
  artifacts, trace inspection, queue/steer controls, and scheduled tasks.
- OpenAI-compatible model configuration and streamed model responses.
- A single interactive agent loop with cancellation, typed budgets, recovery,
  permission suspension/resume, and durable event-backed state.
- Fast mode as a direct single-model path. Auto and Pro use a conductor to
  choose a validated direct or bounded workflow decision for each run.
- Permission-gated file, shell, web, MCP, browser, computer-use, and image
  tools. Session grants are scoped by request attributes rather than acting as
  blanket approval.
- Workspace retrieval across semantic, file-search, graph-direct, and
  graph-walk channels, with source provenance.
- Durable memory production and lexical/semantic recall, kept separate from
  workspace knowledge indexing.
- Background, evidence-gated prompt evolution. No checked-in evaluation proves
  that the current GEPA profile improves product quality.

The current evidence does **not** establish that Auto or Pro outperform the
direct baseline, or that Cindx matches Fugu Ultra. See
[docs/CURRENT.md](docs/CURRENT.md) for the exact claim boundary.

## Repository Layout

```text
apps/desktop/           React frontend and Tauri integration adapter
crates/agent-core/      Shared domain contracts
crates/agent-runtime/   Model/tool loop, run control, budgets, context governor
crates/agent-harness/   Active-run and exclusive-work registries
crates/orchestrator/    Run decisions, workflows, task graph, verification, GEPA
crates/agent-memory/    Durable memory production and recall
crates/agent-rag/       Workspace indexing, file adapter, and LanceDB storage
crates/agent-graph/     Graph extraction and graph-guided retrieval
crates/agent-storage/   SQLite event and state persistence
crates/model-provider/  Provider request, response, and streaming boundary
crates/tools/           Built-in and delegated tool contracts
crates/agent-mcp/       MCP transport and catalog adapter
crates/agent-skills/    Skill discovery, trust, and loading
crates/agent-application/ Thin application-level projections
crates/orchestrator-eval/ Non-shipping evaluation harness
benchmarks/             Versioned deterministic evaluation contracts
docs/                   Current documentation and historical evidence
scripts/                Checks, local builds, sidecars, and release helpers
```

The desktop crate remains the composition root for provider calls, permission
UI, tool side effects, persistence, and Tauri commands. The Rust crates own the
portable contracts and algorithms. The precise ownership boundaries are in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Development

Prerequisites and first-time setup are documented in
[docs/SETUP.md](docs/SETUP.md).

Run the repository checks with:

```sh
scripts/check.sh
scripts/check-frontend.sh
scripts/check-desktop.sh
```

Run the versioned quality-gate profiles as described in
[docs/QUALITY_GATES.md](docs/QUALITY_GATES.md). Provider-backed evaluations are
explicit, billable operations and are never implied by an ordinary green build.

## Releases

- [GitHub releases](https://github.com/DaleXiao/Cindx/releases)
- [Installation and startup diagnostics](releases/README.md)
- [Release process](docs/RELEASING.md)

Tagged builds are Universal macOS applications. Signing and notarization depend
on the repository secrets described in the release guide.
