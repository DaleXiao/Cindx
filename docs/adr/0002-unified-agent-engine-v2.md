# ADR 0002: Unified Agent Engine v2

## Status

Accepted for Cindx `0.1.32`.

## Context

Cindx previously had overlapping execution paths for direct responses, adaptive
collaboration, prompt evaluation, and external-effect evaluation. Each path
could interpret budgets, failures, partial work, and completion differently.
That made Pro brittle: extra workers increased latency without guaranteeing
better evidence or a deliverable result.

## Decision

All interactive and evaluation modes use the same typed execution primitives:

- `AgentKernel` owns task state and the shared turn budget.
- `TaskContract` declares required evidence and completion obligations.
- `WorkerPolicy` and `WorkerRuntime` define bounded worker behavior.
- `TypedFailure` separates retryable transport/model failures from contract,
  policy, cancellation, and deadline failures.
- `WorkflowHandoff` is the only protected boundary between orchestration and
  the interactive agent loop.
- `WorkflowStepStatus` distinguishes completed, degraded, cancelled, and failed
  work; failed output is never injected as evidence.
- The anytime frontier selects only contract-valid candidates. Auto and Pro can
  stop after a valid quorum and improvement window instead of waiting for every
  worker.
- Pro starts from an independently generated anchor and must demonstrate
  verified uplift. A repair pass is bounded and cannot disguise a failed
  collaboration as success.
- Prompt evolution uses train and holdout evidence plus a promotion gate;
  runtime traffic cannot directly promote a prompt.

Fast, Auto, Pro, and evaluation therefore differ by policy and budget, not by
separate agent loops.

## Architectural Boundaries

The desktop layer is an adapter. Core state, contracts, budgets, parallelism,
and typed failure semantics live in `crates/agent-runtime`. Workflow planning,
uplift policy, validation, prompt promotion, and handoff live in
`crates/orchestrator`. Tool safety and batch file evidence live in
`crates/tools`.

Large desktop coordination modules were split by responsibility: setup,
conductor, frontier waves, recovery, reconciliation, quality, uplift,
observability, tool turns, model turns, completion, and command handlers.
The structure checker protects these boundaries from accidental re-merging.

## Consequences

- Collaboration can fail or degrade honestly without losing valid partial work.
- Discovery-only tool traces cannot satisfy a substantive file-evidence task.
- Multi-file evidence can be collected in one bounded `file.read_many` call.
- Auto and Pro can cancel work that is no longer useful.
- Tests and provider evaluation exercise the same runtime used by the app.
- More model calls remain more expensive; uplift is verified, not assumed.

This ADR does not claim Fugu Ultra parity. It establishes the execution
invariants required to measure and improve toward that target without adding
another independent orchestration path.
