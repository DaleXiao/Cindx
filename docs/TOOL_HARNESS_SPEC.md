# Cindx Tool Kernel and Agent Harness

Status: current contract. See [ARCHITECTURE.md](ARCHITECTURE.md) for the complete
run flow and crate relationships.

## Goals

- One tool protocol for built-ins, MCP servers, and skill-assisted workflows.
- JSON Schema arguments remain structured from the model to the executor.
- Every execution route shares permission, cancellation, trace, and artifact handling.
- Agent startup never waits for an unavailable MCP server.
- Skills add scoped instructions and resources without bypassing tool permissions.

## Harness ownership

The harness has one lifecycle owner per concern:

- `agent-runtime` owns `AgentLoopState`, `AgentRunControl`, run budgets,
  cancellation, no-progress detection, repeated-action detection, and typed turn
  budget exhaustion. `PreparedTaskState` owns the prepared objective and epoch
  facts; `AgentTaskContract` owns obligations, evidence, and the bounded Goal
  Delta derived for that prepared state.
- `agent-application` owns the single run/reprepare driver and the typed run
  lifecycle vocabulary. A steer may request a fresh prepared epoch, but it
  cannot create a second production loop.
- `orchestrator` owns the validated run-decision schema and portable workflow
  semantics: task-graph state, role assignment contracts, verification,
  frontier selection, recovery policy, and prompt evaluation.
- The desktop adapter owns side effects: provider calls, permission prompts, tool
  execution, event persistence, Tauri commands, and the execution adapters that
  drive conductor and worker model calls. It implements the application driver
  boundary; it does not define a second run-control policy, run/reprepare loop,
  or task graph.
- `queue_service` and `session_projection` own queue reduction and the versioned
  per-session read model. Interactive commands use compact receipts and indexed
  session deltas instead of rebuilding complete application state.
- `agent-core` owns capability matching and the fail-closed rules for reusable
  session permissions. Desktop `permission_service` owns only indexed
  session-grant and pending-request lookup.
- `tools::ProcessManager` owns bounded managed-process identity, lifecycle,
  streams, and cleanup. The desktop keeps one stable AppState instance across
  tool-registry rebuilds and shuts it down before application exit; registry
  entries are only adapters to that owner.
  `collaboration_service` owns Fugu worker request/result envelopes, continuation
  budgets, and the reserved final-answer turn; neither service performs provider
  calls or tool side effects.

All views derive `idle`, `running`, `waiting_for_permission`, `paused`,
`completed`, `failed`, and `cancelled` from the same typed lifecycle reducer.
Reaching a run or turn budget is recoverable control flow: the runtime preserves
the transcript and the desktop exposes a continuation instead of recording an
ordinary agent failure.

Run identity has two deliberately different scopes. The versioned
`logical_agent_run_id` remains stable across steer, permission recovery, retry,
and continuation so routing, memory, and evaluation see one canonical
experience. The existing `agent_run_id` is the physical attempt and changes on
continuation; `source_agent_run_id` records the exact physical recovery source.
Permission lookup, allow-once decisions, tool effect witnesses, exact replay,
idempotency, and runtime snapshots continue to bind the physical attempt.
Sharing a logical ID never grants cross-attempt authority. Legacy source chains
are projected only inside one task/project/session and fail closed on missing,
cyclic, conflicting, or cross-scope lineage.

Run activity and objective progress are separate contracts. Provider bytes,
tool start/finish, streaming text, and status updates keep the existing
liveness watchdog current, while generic checkpoints remain diagnostic. Neither
kind of signal extends a run segment. Budget extension requires a de-duplicated
Goal Delta from `AgentTaskContract`: first satisfaction of an active obligation,
first target-bound grounding, or a verified workspace/interaction postcondition.
The desktop may record it only after the matching runtime transition commits and
the canonical tool outcome is durable or recoverable. Ordinary success,
parameter or output variation, exact-call replay, and failed, denied, or
cancelled calls do not unlock more model, tool, or turn budget.
Denied observations additionally enter the active `AgentTaskContract` as bounded
typed facts. A user permission denial goes directly to blocked finalization. A
runtime-policy or unavailable-capability denial can request one same-epoch
replan; another denial consumes no new segment and becomes blocked finalization.
The exact denied invocation, and a denied capability when applicable, is
intercepted before the permission broker. Read-only work may continue after a
finalization constraint, but another effect cannot bypass a user denial.
After a durable steer commit actually applies user guidance, run control may
open one fresh base segment for the new objective under the existing lineage
caps. This objective transition is not Goal Delta credit; failed, deleted, and
no-op steers open no segment.

## Fugu-style collaboration and GEPA

Cindx's Fugu-style collaboration is not one standalone component. The
orchestrator supplies the validated decision, workflow, task graph, frontier,
and verification semantics; the desktop adapter performs provider-backed
conductor and worker stages; `agent-runtime` governs cancellation and budgets.
Together they can plan bounded parallel branches, authorize dependency outputs,
reserve terminal delivery, recover a failed worker, and resume from a
checkpoint.

Branch independence is a semantic contract over different assignments and
evidence lineage. It is not inferred from different model names. The conductor
preserves the selected direct model as the baseline and introduces another model
only when its configured role or supported prior predicts a useful contribution.

GEPA is an optimizer outside the active loop. It consumes redacted completed or
replay trajectories, reflects on paired outcomes, and proposes a versioned prompt
genome. Auto-to-Pro reflection requires both sides of a strict source-attested
train pair; deterministic information and diversity selection keeps the exact
redacted set within six trajectories and records its schema and digest. Promotion
requires independent train/holdout evidence and blocks candidate-only failure or
absolute quality, latency, and token non-inferiority regression against the stable
profile. A selected champion may configure a future Fugu run, but GEPA cannot
mutate an in-flight transcript, permission decision, tool result, run budget, or
workflow checkpoint. Route, retrieval, and memory remain per-run decisions until
their own execution path is included in the matched evaluation protocol.

Pro may use canonical timeout, denial, and no-progress facts as an isolated
negative failure curriculum. Each receipt is hash-only, bound to the current
project/run/profile/policy/steer epoch, and contains no prompt, tool arguments,
outputs, provider error text, or secrets. Failure seeds require a successful
scientific train anchor, are limited to two within the existing six-reflection
budget, and cannot become a teacher, positive evidence, Goal Delta, canary
success, promotion evidence, or Pro-to-Auto input. A denial can only reinforce
respect for the current permission boundary or authorized recovery.

## Performance invariants

- Session interaction paths never scan every event for the shared agent task.
- A warm session projection reads only events newer than its stored revision.
- Queue edit, delete, and steer commands return mutation receipts, not full chat
  history.
- Session permission reuse queries the task/session/run index rather than scanning
  the global permission audit.
- Cancellation remains observable during provider streams and sidecar execution.
- Long-session regression tests verify that unrelated session growth does not
  increase projection work.
- Goal Delta identity contains only bounded obligation ids, prompt scope, and
  verified postcondition ids. It never copies or hashes raw tool output, so
  large successful reads cannot add another output-sized allocation merely to
  request budget credit.
- Workspace file observations are bounded. `file.read_many` accepts at most
  eight 32 KiB pages; listing and search have hard discovery/path/byte limits.
  Their cursors bind options and a deterministic snapshot and reject changed
  result sets. Search resumes dense context-free pages from a byte offset rather
  than rescanning the already-consumed prefix.
- Managed processes admit at most two active sessions per physical owner and
  four per application. Wall time is capped at 1,800 seconds, aggregate captured
  output at 8 MiB, CPU at 900 seconds and never above wall time, poll pages at
  64 KiB with a two-second wait, input calls at 64 KiB and cumulative input at
  1 MiB. Cancellation, steer-epoch invalidation, run completion, session
  removal, limits, explicit termination, and application exit terminate and
  reap the process group.

## Tool contract

`ToolSpec` carries a stable wire name, namespace, source, exposure policy, risk,
input JSON Schema, and optional output schema. `ToolResult` can return text,
images, resources, structured output, artifacts, a typed failure, and an optional
model-facing `cindx.tool-observation.v2` projection. The full result remains the
authority for persistence, trace, UI, and artifacts; the projection is only the
bounded evidence passed back to the model.

The v2 projection keeps the tool name, status, summary, completeness, stable
facts, failure code, artifact references, and an optional next action ahead of
the evidence body. `output=` remains the final field so an empty evidence body
cannot satisfy the existing grounding contract. Results without a valid v2
projection use the prior observation format unchanged. The projection is stored
with `ToolCallFinished` and restored during exact-call recovery, and its schema
version participates in the prompt-learning tool-contract digest.

After a successful v2 observation explicitly reports `evidence_complete=false`,
run control may record an exact continuation only when the registered tool
effect is `read_only` or `idempotent`. The lease is bound to the current epoch,
execution scope, tool name, and byte-identical input. It only excludes that exact
continuation from repeated-action and cycle classification; every invocation
still consumes the existing tool-call budget and passes the existing permission,
admission, and cancellation boundaries. Complete, failed, legacy/untyped, or
non-idempotent observations do not create the lease, and steer clears it. The
lease itself grants no Goal Delta, verification authority, permission reuse, or
additional effect authority.

The narrow `tools::tool_contract_v2` adapter owns the hand-written input/output
schemas and v2 projections used by `file.read`, `shell.run`, and
`browser.extract_text`. `file_query_contract_v3` owns search options, cursor,
coverage, and result projection; focused `file_list`, `file_batch`, and
`file_patch` modules own their typed contracts. Execution remains in each tool
module, and portable permission reuse policy remains in `agent-core`.

The workspace file plane preserves legacy raw output and input where documented.
`file.read` v2 adds page and optional bounded full-file hashes;
`file.read_many` exposes per-item partial status and continuation. List and
search pages are stable, bounded, and snapshot-bound. `file.patch` requires a
full base hash plus an exact range/expected text or unique anchor, obtains Write
permission bound to the target path, locks and rechecks file identity/content,
then publishes a permission-preserving same-directory replacement. Its typed
receipt contains before/after/diff hashes and bounded facts. A successful patch
also attempts an immutable `.cindx/output-history` snapshot; snapshot failure is
reported explicitly but cannot reverse or misreport the already-published patch.

The managed process plane adds `process.start`, `process.poll`, `process.input`,
and `process.terminate` without replacing compatible `shell.run`. Start first
creates an owner-bound opaque pending reservation; only a later poll or approved
input activates the already-authorized command, after the start receipt can be
durable. Poll returns bounded cursor pages plus typed pending/running/terminal,
exit, signal, timeout, cancellation, CPU/output-limit, completeness, and
truncation facts. Handles bind the task, project, session, physical run,
collaboration, steer, and contract epochs, and are never recovered from an OS
PID after restart. Input uses a bounded writer queue and a payload-bound one-shot
permission. A parent-death watchdog and the AppState manager prevent ordinary
cancel or application exit from leaving a managed child group behind.

`process.poll` is registered as idempotent so an incomplete typed cursor page can
request the exact same bounded poll again without being mistaken for an action
cycle. This classification does not make process activation a new permission
grant, does not authorize `process.input`, and does not relax owner, epoch,
resource, poll-count, or tool-call limits.

The runtime preserves model arguments as JSON. Legacy `key=value` input remains
accepted only by built-in tools for existing sessions and the manual tool runner.
Successful results still retain their full persistence and UI contract, but an
interactive, collaboration-worker, or evaluation-worker tool observation can
extend its run segment only through the post-commit
`cindx.agent.goal-delta.v1` receipt.

Postcondition verification additionally emits a bounded
`cindx.postcondition-verification-receipt.v1` receipt. It binds the active
steer/contract epochs, postcondition kind, action and observation sequences,
verifier source, and digests without copying raw arguments or output. Completion
quality revalidates that receipt against bounded contract evidence; the legacy
boolean metadata is derived compatibility state and carries no authority alone.

Verifier authority is a closed tool capability, not a tool name, risk label, or
model-provided string. `file.read` signs exact readback only after its structured
result proves an offset-zero, complete, untruncated read of the requested path.
`shell.run` signs a workspace-wide quality check only after a structured zero
exit from a conservative check/test/build/lint command with no shell indirection
or compound syntax. Listing, searching, partial reads, `echo`, dynamic shell,
third-party read-only tools, and every asynchronous `process.poll` receipt carry
no authority by default. A command string containing `test`, `check`, or `build`
cannot create verification; only the trusted synchronous postcondition adapter
may sign it. Multi-target
mutations require cumulative exact coverage of every target, unless one trusted
workspace-wide quality check covers the set.

Persisted workspace target witnesses contain only logical-run and contract-epoch
scoped digests. A witness from another run, another contract epoch, legacy v1,
or an untyped recovered action fails closed. The full task checkpoint is capped
at 1 MiB before decode and validates every retained binding and receipt against
its bounded evidence before state restoration.

## Execution roles and final delivery

- Actor calls own model/tool iteration and consume the ordinary run and Actor
  stage budgets while preserving the terminal reserve.
- Verifier authority is the typed post-commit observation transition. Model text
  cannot mint a verification receipt.
- Finalizer requests expose zero tools, use only the Finalizer stage reserve, and
  do not consume an Actor turn or tool/step budget.
- Before a Finalizer call, the runtime freezes a grounded current-epoch fallback.
  Empty, malformed, unavailable, or tool-calling output returns that exact
  candidate after receipt revalidation; without one, delivery fails closed and
  never retries through the Actor.
- Success and failure terminalization share one durable identity per task,
  session, physical run, and steer epoch. Replay cannot append a second terminal.

## Exposure

Small catalogs are sent to the model directly. When the automatic tool pool is
larger than the context-aware threshold, Cindx keeps approval-gated tools inline,
ranks the remaining tools against the user request, and exposes:

- `tool.search`
- `tool.inspect`
- `tool.invoke`

`tool.invoke` delegates the target tool's permission request before execution.
Meta tools cannot invoke themselves.

For validated conductor decisions, catalog ranking additionally uses task class,
read/effect intent, and active evidence domains. Explicitly required tools stay
inline; unrelated browser or computer control namespaces can be deferred without
removing `tool.search`/`tool.inspect`/`tool.invoke`. Required evidence tools are
pinned after ranking. Permission continuation recomputes this exact plan from the
persisted run context instead of falling back to a broader catalog.

Before model execution, a separate typed route contract pins the minimum tool
class from the active completion intent. Image generation raises that floor to
effects, and a current user image pins vision. The conductor response, Fast
direct route, and degraded fallback all pass the same capability validation;
declaring a weaker route or selecting a configured model without the required
tool/vision capability is an explicit preparation error rather than a silent
downgrade.

Auto value-of-computation calibration cannot weaken that route contract. When
coordination is not worth its predicted cost, only workflow parallelism,
quorum, and independent-verification fields are collapsed; the chosen model,
tool class, vision requirement, risk, retrieval channels, and memory policy are
preserved. Trace metadata distinguishes the workflow candidate from the final
direct or grounded-direct route and records the bounded admission inputs.

The provider-backed V5 evaluation can apply the same collapse as an explicit
Grounded Direct constraint after Auto routing. It preserves the product model,
tool, retrieval, memory, permission, and external postcondition paths while
removing collaboration. The constraint is written only by the feature-gated
evaluation driver; normal product ingress remains native.

Every conductor provider request has a 45-second no-progress boundary. A
single configured conductor therefore degrades through the existing typed
fallback instead of consuming the whole run deadline without bytes; once a
response has made progress, the existing response/recovery limits still apply.

The completion-intent parser preserves path and URL tokens while recognizing
later mutation steps outside quoted or fenced material. A source clause must
resolve to its own workspace target or explicitly refer back to named workspace
inputs before it is treated as local; an unrelated path cannot suppress an
external source request. Explicit web provenance still creates a separate
external evidence obligation, and file effects or post-run verification never
stand in for that evidence domain.

## Permission continuation

Each session owns its suspended `AgentLoopState`. An approval resolution executes
or denies the original invocation, appends the result to that state, and resumes
the same loop. Persisted transcript reconstruction is a crash-recovery fallback,
not the normal approval path.

Cold recovery preserves the conductor task class, tool requirement, vision flag,
prepared objective fingerprint, and independent steer/contract epochs. It
validates the original prompt and durable transcript separately from the
effective objective, then rebuilds non-persisted target anchors. A successful
tool call from an older steer epoch does not satisfy a newly steered prompt.
Successful permission outcomes also persist a bounded, versioned effect witness
bound to the original tool and input fingerprint. Workspace targets are stored
only as run-scoped pseudonymous path tokens, so cold replay can reconstruct a
matching mutation/verification or interaction postcondition without persisting
raw tool input or output in the witness. Older outcomes without this metadata
remain readable and conservatively use the prior fingerprint-only fallback.
The `file.patch` witness records its planned after-SHA. Hot or cold recovery uses
the matching persisted started/finished contract, accepts only an in-workspace
canonical regular file no larger than the patch limit, and reports the effect as
applied only on an exact SHA match; every other state fails closed without a
second mutation.

Hot and cold permission recovery reconstruct the same
`cindx.agent.action-denial.v1` fact. Its persisted state contains the contract
epoch, evidence sequence, tool, SHA-256 input fingerprint, stable kind/code,
scope, and recovery only; raw tool arguments, denial text, and tool output are
not copied into the denial record. A blocked obligation remains visible in the
outcome ledger with denial evidence and can produce only an honestly disclosed
`constraint_observed` completion. It cannot become satisfied, verified, a Goal
Delta, a fresh run segment, or learning-quality evidence.

A denied permission resolution, `PermissionResolved`, `ToolCallFinished`, and
its tool transcript message commit together. The hot or cold task state is ready
before recovery is claimed; the claim and resumed lifecycle event then share a
second transaction. A failed claim rolls back, a failed driver handoff releases
the claim to `Paused`, and startup can replay the canonical denial over an older
task-state checkpoint. No crash window turns the denial into success or Goal
Delta credit.

Identical tool arguments may fail twice. A third identical attempt is blocked so
the model must use its remaining bounded alternative or report the stable
blocker. A new prepared objective epoch clears this denial/replan state; a retry
or continuation of the same objective does not.

`process.start` uses the same exact command-and-cwd capability boundary as
`shell.run`; destructive or dynamic commands remain one-shot. Every
`process.input` call is separately approved and binds its handle, payload digest,
and EOF intent, so interactive input can never inherit a session grant.

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
