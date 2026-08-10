# Current Product Baseline

Current application version: `0.2.30`

Last code-fact review: `2026-08-10`

This document describes the current source tree. It is not a quality claim.

## Product Surface

The desktop app currently includes:

- Projects and sessions with rename, delete confirmation, search, status, and
  persistent history.
- Streamed chat with Markdown, code, Mermaid, mind maps, links, attachments,
  queue/steer input, cancellation, retry, and artifact output.
- A right output pane with file lists and previews, plus a collapsible debug
  drawer for trace, context, and artifacts.
- Schedules, model/provider settings, tools, MCP, skills, permissions, knowledge,
  personalization, appearance, and runtime diagnostics.
- Browser and computer sidecars, image generation, speech input, and managed
  local processes when configured and permitted.

## Execution Modes

- **Fast** bypasses the Conductor and selects one configured model for direct
  execution.
- **Auto** asks the Conductor for one typed direct-or-workflow candidate under
  its bounded planning budget.
- **Pro** uses the same decision contract with a larger bounded planning and
  execution budget.

Auto and Pro do not automatically run every configured model. The Conductor
selects the route, model roles, retrieval needs, decomposition, and verification
requirements in one validated execution plan. A production Workflow contains
exactly one bounded Specialist, optionally one Independent Verifier, and a
deterministic handoff to the foreground Owner. It does not run competing
anchors, reviewer tournaments, repair syntheses, or a model-authored final
synthesis. An Independent Verifier must use a different configured model from
the Specialist; a second prompt to the same model is not treated as
independence. Invalid candidates receive bounded repair; if planning still
fails, execution falls back to the shared foreground Owner without
manufacturing a workflow.

All modes ultimately use the same kernel, run-control, tool-permission,
persistence, and terminal-commit paths. Their planning budgets differ; their
effect authority does not.

Current Agent model events also carry an additive typed attribution projection:
the acting subject is Owner, Specialist, or Independent Verifier; the stage is
plan, evidence, act, verify, or finalize; and the model profile is Primary,
Reasoning, Verifier, or Utility. The Conductor and background learning utilities
are recorded as services, not Actors. Existing role, stage, model,
configuration, and UI fields remain unchanged, so this projection does not
change routing or model calls.

## Run Lifecycle

1. The Tauri adapter validates session, provider, workspace, attachments, and
   current-time context.
2. The user message and task-start state are committed to SQLite.
3. A logical run identity and a physical attempt identity are created. Retry
   and recovery retain logical lineage while receiving a new physical attempt.
4. Run control applies cancellation, steer, turn, stage, and deadline budgets;
   `agent-harness` prevents duplicate active work for the same key.
5. The effective objective and bounded history are compiled into context.
6. Fast creates a direct plan; Auto and Pro validate a Conductor plan. Required
   tools, image input, effects, capability, and budget remain hard constraints.
   The router event and selected decision are committed in one SQLite
   transaction behind the active preparation epoch before treatment execution
   can advance.
7. Durable memory and requested workspace retrieval are prepared separately.
   Semantic search, file search, graph-direct lookup, and graph walk may run in
   parallel and retain source provenance.
8. A validated workflow may execute one read-only Specialist and, when planned,
   one tool-free Independent Verifier. The runtime materializes their checked
   checkpoint as a deterministic handoff; the foreground Owner independently
   decides and performs any effects and final delivery.
9. `agent-application` drives prepared epochs through `AgentKernel`, including
   model turns, tool batches, typed observations, permission suspension, steer,
   recovery, and completion checks.
10. Terminal delivery and lifecycle state are committed once for the active
    physical attempt and steer epoch. Success, failure, and cancellation bind
    the same strategy receipt; pre-decision termination records an explicit
    `not_selected` explanation. Pause remains recoverable and carries the
    receipt without becoming a terminal event. Replay returns the existing
    terminal event instead of creating a second completion.

## Tools and Permissions

Cindx exposes file, patch, shell, managed process, web, browser, computer,
image, MCP, skill, and selected local utility tools through typed contracts.
Tool visibility does not grant authority.

- Side effects pass through the desktop permission path.
- A session grant is matched against task, session, action, risk, scope, and
  capability metadata. `shell.run` and `process.start` also require a command
  capability key.
- Allow-once, session approval, denial, cancellation, and policy/capability
  rejection remain distinct durable outcomes.
- A denied or blocked obligation is not marked complete and must be disclosed
  in terminal output.
- Browser and computer control require healthy sidecars and the relevant macOS
  privacy permissions.

## State, Retrieval, and Memory

- SQLite is the durable product store. Startup fails closed if persistent state
  cannot be opened; there is no silent in-memory substitute.
- Workspace knowledge and project memory are separate systems. Knowledge comes
  from indexed files and graph/vector adapters. Memory comes from eligible run
  evidence and user requirements.
- Memory recall combines lexical and semantic evidence with trust, utility,
  deduplication, supersession, conflict suppression, and session diversity.
- Memory is not append-only. Capacity retention protects pins and verified user
  requirements, then useful observed records; harmful, inactive, superseded,
  expired, and unobserved records are evicted first.
- Semantic memory curation is a bounded background queue. Failed or rejected
  jobs use deterministic projection instead of silently dropping memory.
- Recall-to-terminal attribution is applied from durable events. Ordinary
  answer overlap is not enough to label a memory helpful or harmful; matched
  evaluation evidence is required for that causal label.

## Prompt Evolution

Prompt evolution is outside the foreground loop. Background workers consume
redacted completed evidence, evaluate candidate profiles, and can publish a
stable/canary deployment for future Auto or Pro runs only after typed train,
holdout, safety, and lineage gates pass.

Fast remains seed-only. A profile cannot broaden tool authority, permissions,
context limits, or run budgets, and cannot alter an in-flight run. Missing,
stale, invalid, or inconsistent deployment state fails closed to the seed
profile.

The mechanism is wired, but current checked-in provider evidence does not show
that a learned workflow or finalizer profile improves production quality. No
profile produced by the recent workflow experiments was promoted.

## Current Evidence Boundary

The latest complete product-level provider baseline is Agent Real-World V5 on
version `0.2.22`. It supports a narrow adaptive-direct improvement claim only.
It did not exercise Workflow, learned profiles, or distillation, and Pro was not
an iso-budget causal comparison.

The latest route-causal attempt, V12 on source `ff8c238`, is
`INVALID_EVIDENCE`: the Direct arm retained route evidence but failed terminal
completion, while the Workflow arm stopped before its strategy event was
persisted. It produced no matched pair and no GO/NO-GO result. Production Fast,
Auto, Pro, and profile serving were unchanged by that attempt.

The current source repairs that observability prerequisite and narrows the
production Workflow graph with deterministic contracts. It does not
retroactively validate V12, prove an intelligence or provider-cost gain, or
authorize another provider run.

The current evaluation projection can also derive one bounded, typed
externally verified outcome for either Direct or Workflow from validated
strategy and terminal lineage, actual Actor exposure, external postconditions,
preservation checks, and complete resource receipts. This is shadow evidence
only: it is not production `LearningEvidenceV1`, does not enter routing, prompt
evolution, memory, canary, or serving, and does not establish an intelligence
gain.

See [EVALUATION.md](EVALUATION.md) for the retained numbers and interpretation.

## Known Structural Limits

- `apps/desktop/src-tauri/src/lib.rs` is still a large composition root with
  many sibling modules and broad imports. Portable crates now own substantial
  contracts, but desktop orchestration remains the primary coupling hotspot.
- The frontend has been split into components and style sheets, but `App.tsx`,
  `tauri.ts`, settings, inspector, and thread styling remain large change
  surfaces.
- Evaluation binaries still compile through the desktop adapter when the
  `realworld-eval` feature is enabled. `orchestrator-eval` is non-shipping, but
  the provider campaign surface is not fully isolated from desktop code.
- Current evidence does not establish general Auto/Pro superiority, successful
  GEPA self-improvement, or Fugu Ultra parity.
- The published `v0.2.30` archive is Apple Silicon (`arm64`) and locally
  ad-hoc-signed. A normal-user distribution still needs the appropriate Apple
  signing and notarization path.
