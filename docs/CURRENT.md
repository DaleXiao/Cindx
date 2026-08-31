# Current Product Baseline

Current application version: `0.3.11`

Last code-fact review: `2026-08-12`

This document describes the current source tree. It is not a quality claim.

## Product Surface

The desktop app currently includes:

- Projects and sessions with rename, delete confirmation, search, status, and
  persistent history.
- Streamed chat with Markdown, code, Mermaid, mind maps, links, attachments,
  queue/steer input, cancellation, retry, and artifact output. A turn's tool and
  event activity is collected into a single "Agent actions" disclosure shown once,
  instead of one before and one after the answer. The live run status (Thinking,
  Executing plan, Running ...) trails that disclosure header while the run is
  active with the same shimmer as the thinking indicator, and is no longer shown
  as a separate status block. The streamed answer is
  suppressed once the run reaches a terminal status (completed/failed/cancelled),
  so a lingering stream no longer duplicates the committed answer. Adjacent
  assistant messages with identical content are also collapsed during state merge,
  so a duplicated final answer is never rendered twice. The streaming article is
  likewise hidden while the run's live assistant message is already rendered, so
  an in-progress answer is never shown twice.
- A right output pane with file lists and previews, plus a collapsible debug
  drawer for trace, context, and artifacts.
- Schedules, model/provider settings, tools, MCP, skills, permissions, knowledge,
  personalization, appearance, and runtime diagnostics.
- Browser and computer sidecars, image generation, speech input, and managed
  local processes when configured and permitted.
- A `todo.write` tool gives the run a flat, persisted working-memory list (borrowed
  from opencode/deepseek-harness), and the loop never executes a tool-call batch that
  the provider truncated at its output limit (borrowed from pi), re-asking instead.
- Before the hard repeated-action stop, the loop injects an advisory reminder
  into the model context when the same tool with identical canonical arguments
  repeats consecutively (thresholds 3 and 5): the notice never vetoes or
  rewrites the call, its argument preview is capped at 500 characters, and a
  different call resets it. The reminder is produced by the loop-observer
  registry below, so its advisory behavior is unchanged. Context compaction and
  truncation cuts never split an assistant tool-call round from its tool
  results; a cut that would land inside a round falls back to the nearest
  balanced point.
- The loop carries an advisory observer registry (`agent_runtime::loop_observers`)
  that hooks two kernel points — after each tool observation and before each
  model turn — and proposes interventions the kernel applies to loop state. A
  panicking observer is caught, logged once, and isolated; it can never abort
  the loop or disturb the other observers. The standard observers are: the
  migrated repetition advisory notice (behavior unchanged); a stuck-target
  guard that quarantines a host after three consecutive failed `web.search` /
  `web.fetch` calls (a success clears that host's count), after which calls to
  the quarantined host are blocked before execution with a denied
  `stuck target blocked: <host>` observation; a finalization guard that forces
  the closing turn once the remaining model-call budget reaches the terminal
  reserve (removing all tools and appending a final-answer instruction); and a
  doom-loop guard that, when the same tool with identical arguments repeats to
  its threshold (4), pauses the run behind an explicit user confirmation
  ("Agent appears stuck on <tool>; continue?") that never auto-continues —
  allowing resumes the suspended run exactly once after resetting the streak,
  denying terminates the run as cancelled. The quarantine set, forced-final
  flag, and pending confirmation are runtime-only and are not persisted.
- A tool-call batch whose calls are all permissionless read-only tools (the
  registry `permissionless_read_tool` predicate shared with the subagent and
  plan-mode whitelists) is admitted for parallel execution without the
  conservative serial fallback: a pending continuation lease is honored inline,
  the serial path's checkpoint-driven budget extension may cover the batch, and
  an exhausted budget or a tripped repeated-action guard stops the run with the
  same reason the serial path would record. A batch mixing in any
  permission-requiring or non-read-only tool keeps the serial fallback; budget
  accounting, steer-epoch validation, and tool telemetry pairing are unchanged.
- Long runs keep a rolling summary injected into the trusted runtime context so the
  model retains the objective across compaction. Only transcripts already pressing on
  the context window (≥40% used) pay a model-generated summary
  (Goal/Constraints/Progress/Decisions/Next Steps), cached by transcript fingerprint;
  everything else uses the cheap deterministic extractive
  Goal/Progress/Latest-position summary, so routine runs never block at startup.
  Context token usage (compaction trigger, budget allocation, attempt
  reservation) is counted with a real `cl100k_base` BPE tokenizer held as a
  lazy singleton, not a character heuristic; the heuristic remains only as a
  fallback if tokenizer initialization fails.
- Subagent delegation (borrowed from opencode/pi/deepseek-harness): a `task` tool
  delegates to a child run seeded with a bounded prefix of the parent's balanced
  completed rounds (at most 32 messages, in-flight unpaired tool-call rounds
  excluded; an empty parent history keeps the former delegation-prompt-only
  shape) ahead of the subagent contract and the delegated task, and its answer
  returns to the parent as an internal instruction. The child runs a bounded read-only tool loop (not a single
  completion): each step it may call whitelisted read-only discovery tools
  (`file.read`, `file.list`, `file.search`, `file.glob`, `web.search`,
  `web.fetch`), whose observations are appended to its own isolated message
  history, until it answers without a tool
  call or exhausts `SUBAGENT_MAX_STEPS`. The whitelist is enforced both when the
  tool surface is built and again per call, and the registry additionally refuses
  any non-read-only or permission-requiring tool, so an effectful or out-of-policy
  call becomes a denied observation rather than an execution. Every child model
  call is charged to the parent run's bounded Worker stage budget, so subagents
  share the run's budget and cannot run unbounded; the child is asked to cite
  file findings as `path:line`. The read-only tool policy and step budget live in
  `agent_runtime::subagent`. Multiple delegations in one batch run concurrently
  (results rejoined in call order) and honour the parent run's cancellation, so
  stopping a run aborts its subagents. While subagents run, the Agent actions
  header shows a per-subagent panel (one row each, orb while running, check when
  done) plus a "Running subagents done/total" status; the panel collapses when
  every delegation completes.
  A delegation may also set `allow_patches: true`: when the parent run's own
  prompt effect authority permits workspace effects, the child becomes a write
  subagent whose surface adds exactly `file.patch` and `file.patch_batch` (never
  shell, process, computer, browser, or `file.write`). A write subagent holds no
  permission capability of its own: each patch call is persisted as a pending
  permission request in the parent run's name (marked `session_reusable=false`,
  so session grants never apply and the UI offers only allow-once or deny), the
  parent run keeps executing while the subagent thread parks on the decision,
  and the ordinary approval command resolves the request in place. An approved
  patch executes through the same tool path as a parent-run call (durable
  started/finished events, shared tool budget, undo projection, workspace cache
  invalidation); a denial returns as the child's denied observation; a
  cancellation before approval executes nothing. When the run's objective
  forbids effects, a requested write delegation is refused without a model call.
  The subagent panel marks write subagents with a `write` badge, and the
  approval card names the delegating subagent.
- The main window stays hidden until fonts and initial state are ready plus a short
  timer, then reveals. The wait uses a timer (not requestAnimationFrame, which does
  not fire while the window is hidden), so launch can never stall with no UI, and
  the WebView gets a beat to paint before the window shows.
- Workspace file changes made by `file.write`, `file.patch`, and
  `file.patch_batch` preserve their prior content best-effort for recovery. A
  session can undo and redo its most recent file change through Composer
  controls, guarded by content-hash conflict checks; a `file.patch_batch` call
  is one undo entry, so undoing it restores every file in the batch together
  and any externally edited member blocks the whole group restore. Shell,
  browser, computer, and process effects remain irreversible.
- Project instruction files: `AGENTS.md` discovered from the workspace root up
  to the Git root, plus `.cindx/instructions/*.md` files, are loaded under
  bounded byte caps and injected into every run as untrusted project guidance
  with a durable provenance receipt. Files over the byte caps are recorded as
  omitted rather than silently dropped. The feature is enabled by default and
  can be disabled through the app support configuration file.
- Custom commands: markdown templates discovered from the workspace
  `.cindx/commands/` directory and the global `~/.cindx/commands/` directory
  (project names override global names). They appear in a Composer menu;
  selecting one expands its template into the draft, substituting `$ARGUMENTS`
  with the current draft text and applying an optional effort override.

## Execution Modes

The three effort tiers form a compute ladder over one kernel and one
quality spine; they differ by budget, iteration depth, and verification
strength, not by permission authority:

- **Fast** (quick direct answer): runs one direct model call with a small
  budget and no delivery judge.
- **Auto** (verified answer, default): execution is adaptive direct work
  followed by the independent delivery judge with one bounded repair round
  when eligible.
- **Pro** (deep mission): the same single-model shape with a much larger
  budget for deep, multi-iteration work, plus the most capable prompt
  genome.

Planning is deterministic for every tier: an effort-tier planner builds the
run plan (effort label, tier-selected model, single-model scheduling facts,
and the knowledge decision) with no planning model call. Prompt-derived route
requirements still lift tool/effect/vision constraints fail-closed during
preparation.

Plan mode (plan-then-confirm) is an opt-in interaction feature, not a planning
system. Settings exposes it as a default-off toggle ("Plan first (high/xhigh)"
in Settings → Models, persisted as the provider `plan_first_enabled` setting)
that applies only to High and Extra High effort; Fast and Default never honor
it, and the Composer no longer shows a Plan button. When enabled, the run
drafts a plan before any preparation or execution: a bounded read-only loop
(the same `file.read`/`file.list`/`file.search`/`file.glob`/`web.search`/
`web.fetch` whitelist and per-call enforcement as subagent delegation) whose
model calls are charged to the run's Worker stage budget. The drafted plan — an ordered step list plus
the involved files, as bounded plain markdown — is persisted with the run
events and the run pauses for an explicit user decision: approve and execute,
discard and execute, or cancel the run. Approving injects the plan into the
execution context through the project-instructions pipeline pattern (a
protected internal source with a provenance digest receipt and
untrusted-guidance boundary text); the plan is context content only and
carries no scheduling authority. Discarding runs the ordinary path with the
drafting budget still charged; cancelling terminates the run. The wait is the
existing nonterminal pause with a durable recovery envelope, so an app restart
re-presents the same pending confirmation, and resolving resumes the run
through the ordinary paused-run continuation path. A plan-drafting failure
(provider error, empty plan, or an exhausted Worker stage budget) is recorded
and the run proceeds without a plan.

The confirmation card additionally auto-approves after 30 idle seconds, but
only under the strict approval policy: there every effect still prompts
individually, so the timer merely unblocks the run. Under session/all
policies the card never counts down and the backend rejects timeout
resolutions, keeping the plan gate an explicit user decision. Any sign of
reading (hover, press, focus, scroll), a hidden window, or an unfocused app
pauses the countdown; a failed resolution restarts the full window instead of
retrying. Every resolution records its source (`local-user` or `auto-timeout`)
on the `plan_resolved` event so an audit can tell a timeout approval from an
explicit click.

Each tier can pin a configured default model (`fast_model`, `auto_model`,
`pro_model` in the provider configuration). The Settings Models panel exposes
the three tier pins plus the legacy compatibility fallback slot; the legacy
per-stage role slots (Primary/Reasoning/Verifier/Utility and the planning
override) are no longer surfaced there. They remain persisted provider fields;
the executor-role slot is the fallback when a tier is unpinned. A pinned model
anchors the tier's primary model directly. When a tier is unpinned, the provider catalog's
tier default applies instead — for the DashScope provider that is a flash-class
model for Fast, a plus-class model for Auto, and a max-class model for Pro —
and providers without catalog tier defaults keep the legacy role-slot behavior.

Auto and Pro do not automatically run every configured model; one model
executes the effort plan.
**Multi-model workflow collaboration is retired and physically removed.**
Natural workflow routing never triggered in production, every historical forced
collaboration campaign closed no-go, invalid, or censored, and the owner
decision is permanent retirement, not evidence-gated reconsideration. The
workflow execution chain, the owner-execution graph validator (including the
former read-only two-Specialist widening), the conductor planning runtime,
and the GEPA campaign/evolution worker machinery have all been removed from
the tree. A residual workflow decision fails closed. Conductor, Collaboration,
Workflow, and GEPA semantics no longer appear in the product UI. Remaining
`Conductor*`/`collaboration_*` Rust identifiers are legacy internal naming for
the execution contract, prompt profiles, and the bounded stage-call plumbing
the delivery judge still uses — there is no separate conductor model and no
collaboration lane.

All modes ultimately use the same kernel, run-control, tool-permission,
persistence, and terminal-commit paths. Their budgets differ; their effect
authority does not. There is no multi-model workflow lane. Memory recall and
workspace retrieval scale by effort tier: Fast answers without either; Auto
(`default`) recalls durable project memory with the `relevant` policy (keyed
on the bounded run prompt) and retrieves up to 8 workspace results; `high`
and `xhigh` recall with the `comprehensive` policy and retrieve up to 12 and
16 workspace results.

The Finalizer role is retired: single-model sessions deliver through the
actor, and a forced stop runs at most one toolless wrap-up turn of the same
model (no Summarizer role, no evolved directive). Delivery stays resilient to
wrap-up flakes: an empty or unusable wrap-up response is retried once when no
verified fallback exists, and when the run already holds visible tool evidence
the latest substantive visible assistant text (at least 160 chars) is delivered
as the grounded last-resort answer instead of failing the run. Short progress
notes and evidence-free runs are never delivered this way, and forced stops
with grounded material skip the wrap-up call entirely. Plain text-only actor
answers never needed a second call.

Auto and Pro direct execution carry a contracted delivery judge at the
shared completion point: a model-distinct Reviewer audits the final answer
together with bounded execution facts (mutation count, mutation-verification
state, verification policy, grounding evidence) and returns one typed
single-line receipt (`pass` or `revise` with findings). A `revise` verdict permits at most one repair round
and one recheck; Fast runs and collaboration workflow products are never
judged. Judge unavailability, inconclusive receipts, empty or ungrounded
repairs, and fallback candidates keep the original answer and record a
`direct_judge_disposition` instead of blocking delivery. An opt-in
`direct_judge_fail_closed` provider setting (default off, exposed as a
Settings toggle) narrows two of those outcomes for runs that recorded at
least one successful workspace mutation: when the judge required revision and
the repair could not be grounded, or the recheck still requires revision, the
run commits the ordinary failure terminal with the judge findings instead of
delivering the unverified candidate, and records a
`direct_judge_fail_closed_blocked` disposition. Judge unavailability and
inconclusive receipts stay fail-open on every configuration — they are
infrastructure failures, not quality evidence — and mutation-free runs are
unaffected. At finalization the
disposition plus the task contract's mutation and post-mutation verification
facts are projected into a typed shadow outcome receipt
(`cindx.agent.direct-judge-outcome.v1`) and appended to a private
capped fitness-signal journal; the channel is shadow-only, consumes no
provider call, and is not admitted to routing, promotion, memory, canary, or
serving. An independent review receipt binds the exact journal-window digest
and an explicit decision; its admission maps the window into a conservative,
permanently production- and promotion-ineligible fitness shape. The prompt
evolution consumption path has been removed, so the admitted window is inert
measurement data with no production consumer. Grounding obligations also accept
anchor-matched failed tool attempts
as absent-target evidence, so a task whose requested workspace file does not
 exist fails closed on content but no longer spins through unsatisfiable
 grounding repairs. The effect any-tool obligation (`conductor_effect` /
 `prompt_effect`) sources its satisfiable alternatives from the full tool
 catalog rather than the inline exposure list, so a deferred-but-available
 effect tool such as `image.generate` satisfies the obligation on success and
 cannot trigger an unsatisfiable repair loop.

Current Agent model events also carry an additive typed attribution projection:
the acting subject is Owner, Specialist, or Independent Verifier; the stage is
plan, evidence, act, verify, or finalize; and the model profile is Primary,
Reasoning, Verifier, or Utility. Background learning
utilities are recorded as services, not Actors; effort-tier planning is
deterministic and produces no model event.

Settings presents the persisted model allocation as configuration slots rather
than permanent Agent roles. The compatibility model is preferred by Fast and
remains an execution fallback; Primary and Reasoning are execution-eligible
profiles; Verifier is reserved for the independent verification lane; Utility
is limited to support work; and the planning slot is a planning-service
override.
Changing the compatibility model does not silently overwrite those profiles;
copying it to every profile is an explicit action. The legacy configuration and
wire keys remain unchanged for saved-provider compatibility, and Actor and
Stage attribution is still selected at each call site.

Chat completion requests and the credential verification probe explicitly set
`enable_thinking: false` for model families whose provider builds enable
reasoning by default (`qwen`, `qwq`, `glm`, `kimi`, `deepseek`). This keeps
bounded output budgets spent on the answer instead of provider-side reasoning
traces, the failure class behind the empty-content responses observed in the
consumed Delivery Verification v3 and v4 Reviewer calls. Other model families
keep their provider defaults. There is no runtime or per-effort toggle yet.

## Run Lifecycle

1. The Tauri adapter validates session, provider, workspace, attachments, and
   current-time context.
2. The user message and task-start state are committed to SQLite.
3. A logical run identity and a physical attempt identity are created. Retry
   and recovery retain logical lineage while receiving a new physical attempt.
4. Run control applies cancellation, steer, turn, stage, and deadline budgets;
   `agent-harness` prevents duplicate active work for the same key.
5. The effective objective and bounded history are compiled into context.
6. The effort-tier planner deterministically builds the run plan (no model
   call) and preparation applies the prompt-derived route requirements onto it.
   Required
   tools, image input, effects, capability, and budget remain hard constraints.
   The selected decision is committed in one SQLite
   transaction behind the active preparation epoch before treatment execution
   can advance.
7. Durable memory and workspace retrieval follow the plan's knowledge decision
   and are prepared separately.
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
- Shell and managed-process command classification keeps Execute/Destructive
  risk but tightens what Execute admits. Commands whose executable is outside
  the known-executable list run at Execute risk yet are approved one-shot,
  never auto-grant under session/all policies, and never derive a prefix
  grant. Interpreter executions with a positional script file
  (`bash scripts/build.sh`) run at Execute risk but always prompt and allow
  only an exact-command session grant, never a prefix. Output piped into an
  interpreter (`cat x | sh`) is Destructive; `||` remains the OR operator,
  and module executions (`python3 -m http.server`) keep their historical
  auto-grant eligibility. Guardian auto-approval skips any request the
  approval policy itself would still prompt for.
- Managed processes (`process.start`) honor the session sandbox mode exactly
  like `shell.run`: non-full modes wrap the supervised launcher in
  `sandbox-exec`, so a read-only session also confines long-running processes.
- A session grant is matched against task, session, action, risk, scope, and
  capability metadata. `shell.run` and `process.start` also require a command
  capability key. In addition to the exact-command reuse, a `shell.run`
  approval may grant a command prefix for the session ("Allow prefix"): the
  grant records a `command_prefix` derived from the approved command, and later
  `shell.run` calls whose shell tokens begin with that exact token sequence
  reuse it without a new prompt; exact-command grants and every other session
  grant keep their exact-match behavior. Prefix reuse is fail-closed: commands
  with shell metacharacters never tokenize, and obviously destructive commands
  (`sudo`, `rm -rf`, `chmod 777`, `mkfs`, `dd`, writes to disk devices,
  download-piped-to-shell, and similar) always prompt even when a prefix
  matches, and a dangerous command can never be granted as a prefix.
- Optional OS-level shell confinement (default off): each session carries a
  sandbox mode — `full` (default, no wrapper, argv byte-identical to the
  unconfined execution), `workspace-write`, or `read-only` — stored as
  replayable session events (`sandbox_event = mode_changed`), so the effective
  mode is a fold over the session's event log, survives restarts by replay,
  and never crosses sessions. When the effective mode is not `full`,
  `shell.run` wraps its command in macOS Seatbelt via
  `/usr/bin/sandbox-exec -p <profile> -- /bin/zsh -c <cmd>` with a
  deterministic SBPL profile (`(version 1)(allow default)(deny file-write*)`
  plus a `/dev/null` write allowance; `workspace-write` additionally allows
  writes under the workspace root and `/tmp`; paths are SBPL-escaped). The
  profile and argv derivation are pure functions in `agent-core`
  (`seatbelt_profile_args`, `confined_argv`); a mode without an available
  sandbox-exec fails the command instead of silently running unconfined. The
  mode is selected per session from the composer's model/effort menu ("Shell
  sandbox") and read fresh before every shell execution.
- An opt-in `guardian_auto_approval` provider setting (default off) runs a
  model-distinct Reviewer check on a permission request that would otherwise
  pause for user approval: the reviewer sees the objective, tool, arguments,
  risk level, and a bounded recent-context excerpt under a 20-second cap and
  must answer one strict single-line JSON verdict; only an explicit `allow`
  auto-approves (as allow-once, resolved by `guardian-auto-approval`), while a
  `deny`, timeout, malformed answer, or unavailable reviewer falls back to the
  ordinary user prompt without failing the task, and Destructive-risk requests
  are never auto-approved. Every review records a shadow
  `guardian_disposition` on the run's permission events for observability.
- The Settings permissions panel offers a three-tier approval policy for agent
  permission requests (default Strict): Strict keeps the current behavior and
  prompts for every request; Approve in session auto-approves every
  non-Destructive request for the duration of its session; Approve all
  auto-approves every non-Destructive request without prompting. Destructive-risk
  requests are never auto-approved under any policy and always pause for manual
  confirmation. Every policy auto-approval records a `PermissionResolved` event
  with an `auto_session` or `auto_all` decision for audit, and the policy is
  persisted as the provider `approval_policy` setting (a missing or unknown
  persisted value falls back to Strict).
- Allow-once, session approval, denial, cancellation, and policy/capability
  rejection remain distinct durable outcomes.
- A denied or blocked obligation is not marked complete and must be disclosed
  in terminal output.
- Browser and computer control require healthy sidecars and the relevant macOS
  privacy permissions.
- `file.patch_batch` applies up to 16 `file.patch` operations (same anchor or
  byte-range selector plus base SHA-256 per file) to distinct workspace files
  as one atomic batch: every target is validated in memory before any write,
  a validation failure writes nothing and reports each item's failure reason,
  and publication follows the input order through the same locked atomic
  replace (a mid-batch publish race rolls the applied prefix back
  best-effort). The batch is bounded to 32 MiB of combined base and patched
  content, shares `file.patch`'s Write risk, and its permission scope is the
  sorted set of target paths, so a session grant is reused only for the
  identical path set. It is not on the read-only subagent or plan-mode
  whitelist; a write subagent (`task` with `allow_patches`) may call it under
  the per-call approval contract described above.
- `file.glob` is a pure read-only tool that matches workspace files against a
  relative glob pattern (for example `src/**/*.rs`) and returns a sorted list
  bounded to 200 results with a truncated flag; hidden entries, symlinks, and
  local credential files are skipped. `web.fetch` retrieves the body of one
  HTTP/HTTPS URL through curl (at most five redirects, an 8 MiB byte bound, and
  a clamped timeout) and wraps the body in an explicit untrusted-provenance
  boundary; it shares the `web.search` Network permission and risk level. Both
  tools are on the subagent and plan-mode read-only whitelist.
- MCP servers already configured for other tools can be imported via
  "Import installed" in Settings → MCP. Cindx reads the standard config
  locations (Claude Desktop, Claude Code, Cursor, opencode `.jsonc`/`.json`,
  a workspace `.mcp.json`), strips JSONC comments, and converts both
  string+args and array-form `command` entries (stdio) plus `url` entries
   (http/sse). Imported servers are added as enabled with approval required;
   existing servers and `enabled:false` entries are never overwritten/imported.
   Each MCP tool is exposed under a stable `mcp__<server>__<tool>` wire name; when
   two distinct tools slug to the same name, the later one gets a numeric suffix
   instead of being silently dropped, and the raw tool name is always used for the
   remote call.

## State, Retrieval, and Memory

- SQLite is the durable product store. Startup fails closed if persistent state
  cannot be opened; there is no silent in-memory substitute.
- Workspace knowledge and project memory are separate systems. Knowledge comes
  from indexed files and graph/vector adapters. Memory comes from eligible run
  evidence and user requirements.
- Workspace knowledge re-indexing is incremental: files whose content hash is
  unchanged keep their previous chunks and embeddings (only their
  modification-time freshness markers are refreshed), only new or
  content-changed files are re-chunked and embedded, and deleted files' chunks
  are dropped. The merged index is content-equivalent to a full rebuild of the
  same workspace, and the atomic generation publish (file snapshot, LanceDB
  export and database, graph store) is unchanged; a changed embedding profile
  discards the reuse base so the whole workspace is re-embedded.
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
- A successfully completed run measures observed use of the memories it
  recalled: the terminal commit records a `memory_use` event against the
  delivered answer and increments each used record's `observed_use_count`.
  Failed, cancelled, and steered-away runs record nothing.
- Each physical agent run also appends one shadow-only performance telemetry
  receipt (`cindx.agent.run-telemetry.v1`) to a private 0600 capped journal
  beside the app data root at its durable terminal commit. The receipt records
  the run/session/effort labels, wall time, model and tool call counts with
  cumulative provider wait and tool execution time, context compaction and
  rolling-summary counts, workspace retrieval duration and channel hits,
  provider-observed prompt/completion tokens with usage-source provenance,
  the terminal path (direct, terminal finalizer, fallback, or none), and the
  stop reason. The channel is measurement plumbing only: production code never
  reads the journal, and no routing, prompt, memory, permission, or serving
  path consumes it.

## Prompt Evolution

Prompt evolution is retired and physically removed. The background workers that
consumed redacted completed evidence, evaluated candidate profiles, and
published stable/canary deployments have been removed from the tree, and the
prompt-genome / prompt-profile serving machinery is gone with them: a run
carries no prompt profile at all.

The Settings evolution panel and its toggle are removed from the UI, and the
`set_prompt_evolution_enabled` command is retired with them. The background
evolution machinery (campaign runtime, mutation, pairwise evaluation, learning
outbox, canary/rollout, and distillation), the genome schema, and the
seed-profile serving path have all been physically removed from the tree; only
the shadow judge-outcome measurement plumbing remains (an inert journal with no
production consumer). Current checked-in provider evidence does not show that a
learned workflow or finalizer profile improves production quality, and no
profile from the historical experiments was promoted.

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
stopping is derived from the required-lane result.

**Removed in the phase-3 effort-tier rebuild.** The `realworld-eval` runtime
adapter, the successor matched runner, the prompt-genome / prompt-profile
machinery, and the GEPA campaign/evolution workers have been physically removed
from the desktop crate and the (now-deleted) `orchestrator` crate; the
`realworld-eval` cargo feature and the `cindx-agent-realworld-eval` binary no
longer exist. The paragraphs
below are retained as a historical record of the consumed evaluation protocols;
no production or shipping code references them any longer. The trusted
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

- Undo and redo cover only `file.write`, `file.patch`, and `file.patch_batch`
  changes whose undo snapshot was captured. A change whose snapshot capture
  failed is disclosed as not undoable, and undo is blocked when the target
  file changed outside the recorded run.
- Project instruction files are enabled by default and can currently only be
  toggled or extended through `project_instructions.json` in the app support
  directory; a Settings UI is not wired yet.
- `apps/desktop/src-tauri/src/lib.rs` is now a module index (around 230 lines);
  the Tauri builder and command registration live in `app_bootstrap.rs`.
  Portable crates own substantial contracts, but desktop orchestration
  remains the primary coupling hotspot.
- The frontend has been split into components and style sheets. The browser
  preview fallback state now lives in its own module beside `tauri.ts`, and
  Composer textarea sizing plus attachment batch limits are extracted into
  pure, node-tested models (`composerSizingModel`, `attachmentLimitsModel`).
  `App.tsx` is now a composition and layout layer: the session runtime store
  (agent state, trace, optimistic overlays, queue drain), project/session
  lifecycle commands, run and queued-message commands, and the runtime sync
  effects (polling, prefetch, subscriptions) live in `src/controllers/` as
  `useSessionRuntimeController`, `useProjectSessionController`,
  `useAgentRunController`, and `useSessionRuntimeSync`. Agent-state IPC
  responses (`NativeAgentState` / `NativeAgentStateDelta`) are
  runtime-validated at the boundary (`tauriAgentStateContract`), and the
  optimistic queue reconciliation invariant lives in the node-tested
  `sessionRuntimeModel` (`reconcileOptimisticQueuedMessages`). `tauri.ts`,
  settings, inspector, and thread styling remain large change surfaces.
- The `realworld-eval` feature and the `orchestrator`/`orchestrator-eval`
  crates are physically removed; there is no provider evaluation surface in the
  product build.
- Delivery Verification's no-clobber consumption marker is a local filesystem
  authority. It fails closed across output-root relocation and normal crashes,
  but it is not an external anti-rollback service against an actor able to
  delete or restore every private control-plane file under the same user ID.
- Current evidence does not establish general Auto/Pro superiority, successful
  prompt-evolution self-improvement, or external-benchmark parity.
- The per-request `generation_temperature` override is produced by the
  executor loop: Fast and Auto pin deterministic sampling (`0`), Pro keeps
  provider defaults.

- The installed `0.2.77` build and the published `v0.2.77` archive are Apple
  Silicon (`arm64`) and locally ad-hoc-signed. A normal-user distribution still
  needs the appropriate Apple signing and notarization path.
