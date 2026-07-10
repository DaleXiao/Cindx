# Cindx Tool Kernel and Agent Harness

## Goals

- One tool protocol for built-ins, MCP servers, and skill-assisted workflows.
- JSON Schema arguments remain structured from the model to the executor.
- Every execution route shares permission, cancellation, trace, and artifact handling.
- Agent startup never waits for an unavailable MCP server.
- Skills add scoped instructions and resources without bypassing tool permissions.

## Tool contract

`ToolSpec` carries a stable wire name, namespace, source, exposure policy, risk,
input JSON Schema, and optional output schema. `ToolResult` can return text,
images, resources, structured output, artifacts, and a typed failure.

The runtime preserves model arguments as JSON. Legacy `key=value` input remains
accepted only by built-in tools for existing sessions and the manual tool runner.

## Exposure

Small catalogs are sent to the model directly. When the automatic tool pool is
larger than the context-aware threshold, Cindx keeps approval-gated tools inline,
ranks the remaining tools against the user request, and exposes:

- `tool.search`
- `tool.inspect`
- `tool.invoke`

`tool.invoke` delegates the target tool's permission request before execution.
Meta tools cannot invoke themselves.

## Permission continuation

Each session owns its suspended `AgentLoopState`. An approval resolution executes
or denies the original invocation, appends the result to that state, and resumes
the same loop. Persisted transcript reconstruction is a crash-recovery fallback,
not the normal approval path.

Identical tool arguments may fail twice. A third identical attempt is blocked so
the model must change its approach.

## MCP

MCP configuration and the last-known-good catalog are persisted separately.
Prompt assembly reads only the cache. Explicit refresh connects in the background
path and updates the cache without deleting the prior snapshot on failure.

Supported transports:

- stdio with a persistent child process
- Streamable HTTP with JSON or SSE responses and MCP session headers

The current client is a focused in-repo MCP protocol adapter. Its transport and
catalog boundaries are intentionally isolated so it can move to the official
Rust SDK without changing the tool kernel or settings contract.

MCP tools use stable `mcp__server__tool` names. Approval is enabled by default.
Secrets are stored in mode `0600` configuration and are never returned through
the settings read API.

## Skills

Cindx discovers `SKILL.md` packages from:

- `~/.cindx/skills`
- `<project>/.cindx/skills`
- compatibility readers for `<project>/.agents/skills` and `.claude/skills`

Compatibility skills begin disabled and untrusted. Only enabled, trusted skills
can be selected or loaded. At most two relevant skills are injected for a request;
the rest remain available through `skill.search` and `skill.load`.

Skill scripts are resources, not privileged executables. Running one still goes
through a registered process tool and the permission broker.

## Artifacts and trace

Tool lifecycle events remain `proposed`, `permission requested/resolved`,
`started`, and `finished`. Structured output is attached to event metadata. Image
content is materialized under `<project>/.cindx/artifacts` so the inspector can
render it as an agent-produced artifact.
