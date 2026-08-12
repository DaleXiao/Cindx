# Current Product Baseline

Current application version: `0.2.34`

Last code-fact review: `2026-08-12`

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
- Project instruction files: `AGENTS.md` discovered from the workspace root up
  to the Git root, plus `.cindx/instructions/*.md` files, are loaded under
  bounded byte caps and injected into every run as untrusted project guidance
  with a durable provenance receipt. Files over the byte caps are recorded as
  omitted rather than silently dropped. The feature is enabled by default and
  can be disabled through the app support configuration file.

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
are recorded as services, not Actors.

Settings presents the persisted model allocation as configuration slots rather
than permanent Agent roles. The compatibility model is preferred by Fast and
remains an execution fallback; Primary and Reasoning are execution-eligible
profiles; Verifier is reserved for the independent verification lane; Utility
is limited to support work; and Conductor is a planning-service override.
Changing the compatibility model does not silently overwrite those profiles;
copying it to every profile is an explicit action. The legacy configuration and
wire keys remain unchanged for saved-provider compatibility, and Actor and
Stage attribution is still selected at each call site.

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

The source also defines bounded offline collaboration learning. The initial
matched Direct/Workflow gate decides whether one read-only Specialist is
useful. A later candidate keeps that topology fixed and changes exactly one of
two context allocations, independent verification, or one same-lane repair;
stopping is derived from the required-lane result. Under the opt-in
`realworld-eval` feature, a successor-only matched runner can persist the
assigned policy before any Owner or worker call and record a digest and byte
count for the exact encoded worker request before dispatch. The trusted
projection joins all turns to the actual lane result and externally verified
outcome. A separate
offline feature reconstructs bounded Pair/Censor records into an append-only
evidence set, while a private immutable-file journal recovers interrupted
capture as `IncompleteInstrumentation` without replaying a provider call.

These components are dormant outside evaluation and are not connected to the
invalid frozen V12 run. The tracked
`benchmarks/agent/collaboration-successor-v1.json` suite and companion protocol
manifest now freeze three matched pairs / six runs: one 5,000-bps Workflow
baseline, one sole candidate that changes only the context allocation to
7,500 bps, and one training-ineligible holdout, all under the existing
conservative Workflow evaluation run budget. Non-positive baseline evidence,
any censor, safety or preservation failure, resource regression, or no uplift
freezes the collaboration type.

Goal 3E adds a feature-gated, once-authorized successor path without changing
the tracked protocol. The authorization binary is provider-free and binds the
canonical preflight receipt, current clean source authority, redacted
provider/model configuration, fixed matrix and budgets, new external output
root, and exact execute-binary digest into a private 15-minute receipt. The
execute binary revalidates those bindings, consumes that receipt and output
root once, durably reserves the campaign, cell, and arm before model work, and
can finish only as ready for independent review, frozen, or censored. Recovery
never resumes or retries a started physical run.

The frozen Goal 3E instance was authorized and consumed once on source
`12a3ea2` / version `0.2.32`. Its first baseline Direct arm was durably
reserved, but the product run ended before selection with zero selected
decisions. Its terminal lineage correctly recorded the explicit pre-decision
`not_selected` state; no treatment or Owner execution occurred. The
selected-only external-outcome projector misclassified that legal state as a
malformed receipt before producing a valid observation. The journal therefore
closed `CENSORED` with zero observed runs; Workflow, candidate training, and
holdout never ran. This is `INVALID_EVIDENCE`, not a no-uplift or capability
result, and the protocol must not be retried. V12 and production routing,
prompt serving, permissions, safety, recovery, and history remain unchanged.
Current source now classifies that legal pre-decision state explicitly and has
a real terminal-producer-to-outcome-projector regression test. The repair only
corrects instrumentation classification; it does not recover the consumed run
or establish any intelligence or performance result.

A separate default-off Delivery Verification authority remains isolated under
`realworld-eval`. Its provider-free control plane binds a clean source HEAD/tree
and version, tracked protocol/suite and aggregate case authorities, redacted
provider and Executor/Reviewer configuration, exact execute-binary full-file
digest/size and SHA-256 CodeDirectory identity, and new external receipt and
output paths. Authorization must revalidate that exact binding before granting
a short-lived private one-shot capability. Execution compares the frozen
CodeDirectory with the kernel identity of its running process, then atomically
creates one output-authority marker beside the output root before provider work.
Every authorization path for that authority shares the marker, so moving or
deleting the output directory cannot make a consumed campaign reusable.

The frozen Delivery Verification v1 instance was authorized and consumed once
on source `5373e65`. It closed `CENSORED` / `INVALID-INSTRUMENTATION` during the
first calibration Owner call: the reservation recorded the canonical semantic
request digest, the uncommitted provider result metadata supplied a
domain-separated wire-payload digest, and the journal incorrectly required
those distinct authorities to be equal. The journal charged one logical and one physical reservation but accepted
zero terminal model-call receipts, zero case receipts, and zero valid pairs;
holdout never opened. The unbound response artifact is excluded from evidence,
so the run establishes no answer-quality, uplift, regression, usage, latency,
or cost result. This one-shot protocol is consumed and must not be rerun.
Production finalization, routing, prompt evolution, GEPA, the installed App, and
the published release remain unchanged.

The instrumentation repair was frozen into v2 without recovering or rerunning
v1. V2 was then authorized and consumed exactly once. All eight calibration
pairs completed in 16 calls: seven were both-pass and one was both-fail, with no
treatment-only win or control-only loss. The sole mismatch was a case-contract
error between the visible `controlling_revision` field and the hidden exact
oracle's `revision` field. The fixed calibration gate therefore closed
`terminal_futility`; all 24 holdout cases were durably skipped. This is neither
uplift nor regression evidence, and v2 must not be rerun.

Delivery Verification v3 froze a new seeded-defect recovery and preservation
component test, not a natural-draft uplift experiment. It was authorized and
consumed exactly once. The first calibration Reviewer call was durably reserved
and made one provider attempt. The provider wrapper returned an empty content
payload, but the v3 journal rejected its otherwise bound response artifact
because its byte count was zero. No terminal call receipt or case receipt was
accepted, calibration did not complete, holdout never opened, and the journal
froze `CENSORED`. This is an instrumentation failure with zero valid matched
pairs, not answer-quality, uplift, regression, latency, usage, or cost evidence.
V3 must not be rerun.

Delivery Verification v4 is the provider-free successor authority. It references
the exact same tracked v3 suite bytes and preserves all 32 cases, order, hidden
oracle, seeded candidates, model inputs, output contracts, budgets, decision
thresholds, and no-retry policy. The suite still contains 24 frozen seeded
defects and eight clean preservation sentinels split 8/24 between calibration
and holdout. Control is the exact seed; treatment permits one Reviewer decision,
at most one Executor repair, and one Reviewer recheck.

The unchanged calibration gate requires all eight cases, exactly six control
failures, at least four treatment-only wins with one in each defect stratum, and
no control-only loss or structural/treatment-execution failure. Holdout requires
all 24 cases, exactly 18 control failures, at least 13 treatment-only wins with
four in each defect stratum, and no clean-sentinel loss or structural/treatment-
execution failure for `seeded_repair_effective`.

V4 changes only the protocol/control-plane namespace and the defective response-
artifact instrumentation. The journal now accepts and hashes an exact zero-byte
artifact instead of treating its length as missing. Completed empty Reviewer
content is then retained as `invalid_verifier_response`, an intention-to-treat
treatment failure for that fixed case, without retry or replacement; a response
that is not complete and tool-free remains structural. Semantic-request and
immutable prepared-wire digests/sizes are still reserved before dispatch, and
terminal request metadata must match the reserved wire authority.

All nine v1-v3 preflight, authorization, and execute entrypoints reject their
consumed authorities before reading live state. V4 was frozen from clean merged
source `275868e4dd84692f15998a1cb95afa99df264267`, authorized, and consumed
exactly once. Its first calibration Reviewer call retained the exact zero-byte
artifact as a 0600 file with SHA-256
`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`,
so the v3 instrumentation defect did not recur. The provider-bound receipt
classified that call `invalid_output` and non-retryable because the response was
not a complete tool-free answer. Case 1 therefore closed
`structural_failure`, and the campaign closed `inconclusive` before any matched
pair or calibration decision existed. Accounting is one logical call, one
physical attempt, one terminal call, and zero retries; the other 31 cases never
started. V4 is consumed and must not be rerun or interpreted as repair
effectiveness, ineffectiveness, preservation regression, or provider-quality
evidence.

See [EVALUATION.md](EVALUATION.md) for the retained numbers and interpretation.

## Known Structural Limits

- Project instruction files are enabled by default and can currently only be
  toggled or extended through `project_instructions.json` in the app support
  directory; a Settings UI is not wired yet.
- `apps/desktop/src-tauri/src/lib.rs` is still a large composition root with
  many sibling modules and broad imports. Portable crates now own substantial
  contracts, but desktop orchestration remains the primary coupling hotspot.
- The frontend has been split into components and style sheets, but `App.tsx`,
  `tauri.ts`, settings, inspector, and thread styling remain large change
  surfaces.
- Evaluation binaries still compile through the desktop adapter when the
  `realworld-eval` feature is enabled. `orchestrator-eval` is non-shipping, but
  the provider campaign surface is not fully isolated from desktop code.
- Delivery Verification's no-clobber consumption marker is a local filesystem
  authority. It fails closed across output-root relocation and normal crashes,
  but it is not an external anti-rollback service against an actor able to
  delete or restore every private control-plane file under the same user ID.
- Current evidence does not establish general Auto/Pro superiority, successful
  GEPA self-improvement, or Fugu Ultra parity.
- The installed `0.2.34` validation build and published `v0.2.30` archive are
  Apple Silicon (`arm64`) and locally ad-hoc-signed. A normal-user distribution
  still needs the appropriate Apple signing and notarization path.
