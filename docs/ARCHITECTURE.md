# Current Architecture

This document records ownership and dependency direction in the current tree.
It deliberately calls out remaining coupling instead of treating file count as
modularity.

## System Boundary

```text
React UI
  -> typed Tauri adapter
  -> desktop commands and composition root
  -> application driver
  -> single-model execution plan
  -> agent kernel
  -> model observations and permission-gated tools
  -> SQLite events, state, artifacts, memory, and retrieval indexes
```

Cloud providers reason and stream model output. The local app owns run identity,
context assembly, tool exposure, permission decisions, effects, persistence,
recovery, retrieval, memory, and the delivered result.

## Runtime Ontology and Trace Attribution

Current Agent execution uses separate dimensions instead of treating legacy
role names as one ontology:

- **Actor:** historical vocabulary of Owner, Specialist, or Independent
  Verifier; single-model effort-tier runs act exclusively as Owner.
- **Stage:** plan, evidence, act, verify, or finalize.
- **Model profile:** Primary, Reasoning, Verifier, or Utility.
- **Service:** Background learning utilities are services and are never recorded
  as Actors. Effort-tier planning is deterministic and makes no model call, so
  it is not a stage participant at all.

The Owner alone owns permission-gated effects and final user delivery. The
retired workflow lanes used Specialists for bounded internal plans or evidence
and Independent Verifiers for effect-free artifact evaluation; those lanes no
longer execute. Utility-profile model calls support bounded background
preparation but do not participate in any production decision or own final
delivery.
Background prompt mutation is recorded as a learning utility service, not a
production Specialist.

The provider configuration keeps its legacy storage keys, but their current
product semantics are explicit:

- `model` is the Fast-preferred compatibility execution model.
- `executor_model` is the Primary profile.
- `planner_model` is the Reasoning profile.
- `reviewer_model` is the Verifier profile.
- `summarizer_model` is the Utility profile.
- `conductor_model` is a legacy persisted slot; the effort-tier planner makes
  no planning model call, so it no longer selects anything.

These slots do not assign permanent Actors to models. A concrete model may be
configured in more than one slot, but each call is admitted by the slot needed
for that lane: Primary and Reasoning models may execute the direct or Specialist
path, a Verifier model may enter only the verification lane, and a model
configured only as Utility cannot enter production routing or workflow
execution. The compatibility model can be copied to all four profiles only
through an explicit Settings action.

These fields are an event-local sidecar on current Agent model request events.
Started and finished events reuse the same explicit attribution selected at the
call site. Existing `role`, `stage`, persisted model keys, summaries, and event
counts are preserved for compatibility; legacy events without the schema remain
legacy and are not reclassified from display strings.

## Crate Ownership

| Crate | Current owner responsibility |
| --- | --- |
| `agent-core` | Transport-free IDs, messages, events, permissions, tool/model contracts, shared schemas, and the single outbound HTTP policy owner (`http_policy`) |
| `agent-eval` | Deterministic portable evaluation skeleton: case schema, postcondition checks, scripted-provider runner, per-run resource receipts, matched position-balanced arms, and suite reports; shares the product effort-tier scheduling authority; no production consumer, and the deterministic harness makes no provider network calls |
| `agent-runtime` | Kernel, run control, context governor, task contract, adaptive cursor, system-prompt composition, model-turn and tool-runtime semantics |
| `agent-application` | The run/reprepare driver, strategy/terminal lifecycle, the portable effort-tier scheduling authority (`EffortRunPlan`/planner, extracted from the desktop crate so the eval harness shares it), the portable externally verified outcome contract, and the delivery-judge fail-closed decision (so the evaluation harness applies the same verification rule as the product) |
| `agent-harness` | Active-run and exclusive-work registries; no model policy |
| `agent-memory` | Memory records, retention, recall, utility attribution, and deterministic curation contracts |
| `agent-rag` | Workspace indexing, file adapter, semantic retrieval, and vector-store integration |
| `agent-graph` | Graph extraction, direct graph retrieval, and graph walk |
| `agent-storage` | SQLite schema and durable event/state repositories |
| `model-provider` | HTTP/WebSocket provider transport, streaming adapters, and request-generation semantics (thinking defaults and generation-temperature overrides) |
| `tools` | Tool schemas and portable tool implementations |
| `agent-mcp` | MCP transport, catalog, and invocation adapter |
| `agent-skills` | Skill discovery, trust, loading, and built-in skill assets |

`agent-runtime` consumes model contracts from `agent-core`; it must not depend
on the provider transport crate. The structure gate enforces that boundary.

## Desktop Composition Root

`apps/desktop/src-tauri` owns integration that cannot be portable without
changing product behavior:

- Tauri commands and UI event emission.
- Provider configuration and concrete transport construction.
- SQLite-backed application state and projections.
- Permission UI, platform commands, sidecars, and effect execution.
- Session/project lifecycle, attachments, outputs, schedules, voice, and native
  open/reveal operations.
- Foreground orchestration glue and memory workers.

This layer is not yet a thin adapter. `src/lib.rs` is a module index; the
Tauri builder and command registration live in `app_bootstrap.rs`, and several
workflows still cross desktop services by shared composition state. Future
refactors should move a complete owner and its tests behind a narrow
interface; merely creating more sibling files would not reduce coupling.

## Interactive Run Flow

### 1. Admission and identity

The task command validates provider, session, workspace, attachments, and model
capabilities, then persists the user message and start state. Logical run ID,
physical attempt ID, and steer epoch define replay and recovery boundaries.

`agent-harness` admits only one active owner for the same run/work key.
`AgentRunControl` applies cancellation, deadlines, stage budgets, and steer, and
owns the one physical model-resource ledger: the Owner turn loop and every
auxiliary lane (Plan drafting, Subagent children, rolling Summary) reserve and
settle physical attempts through it (`ControlledModelAttempt` /
`controlled_aux_model_call`), so `max_total_tokens` and physical-attempt
telemetry account for the whole run instead of undercounting the aux lanes.

### 2. Context and execution plan

The desktop adapter projects bounded history and an authoritative effective
objective. `agent-runtime` compiles invariant sources, complete tool rounds,
optional evidence, and a bounded cognitive overlay. Bounded project
instruction files (`AGENTS.md` between the workspace root and the Git root,
plus `.cindx/instructions/*.md`) join the prepared context as a protected,
untrusted guidance source carrying a provenance receipt.

Planning is deterministic and effort-tier based; it makes no model call. The
desktop effort planner (`agent_effort_planner`) builds one `EffortRunPlan` per
preparation: the effort label, the tier-selected primary model
(`effort_tier_model` — the tier's pinned default or the executor-role
fallback), fixed single-model scheduling facts, and the knowledge decision
(fast: no memory recall or workspace retrieval; default: relevant memory
recall keyed on the bounded run prompt plus workspace retrieval; high/xhigh:
comprehensive memory recall plus a wider workspace retrieval cap). Preparation
applies the prompt-derived route requirements onto the plan fail-closed (tool
requirement lift, effect authority, image-input vision) and validates the
tier-selected model's capabilities against the configured candidate pool. The
plan's facts are written key-for-key into the run context (including a
compatibility `run_decision` projection and the plan digest as
`execution_plan_semantic_sha256`), so the loop and contract read the same keys
they read before the conductor was removed.

The selected plan also produces a strategy receipt bound to task, session,
physical run, steer epoch, and plan digest. Run control serializes a
preparation checkpoint without entering execution, while one immediate SQLite
transaction writes the typed "Agent run decision selected" event. A
cancellation or steer that wins first prevents that stale decision from being
committed.

### 2a. Plan-then-confirm gate (High/Xhigh only)

When the Composer's plan toggle is on for a High/Xhigh run, the task command
runs a plan gate after the durable start commit and before preparation
(`agent_plan_mode_runtime`). The gate drafts a plan with a bounded read-only
tool loop that reuses the subagent whitelist (`file.read`, `file.list`,
`file.search`, `file.glob`, `web.search`, `web.fetch`) and the registry's
permissionless-read guard, and
charges every drafting call through `begin_stage_model_call(_,
RunStageClass::Worker)` plus a physical-attempt reservation on the unified
resource ledger, so plan drafting counts against the run budget like an Owner
call. The proposal (bounded markdown plus its
content-bound digest) and the later user decision are persisted as ordinary
run events; the gate then parks the run with the existing nonterminal pause
plus recovery-envelope mechanism (`waiting_for_plan_confirmation`), so
recovery re-presents the same pending confirmation. Resolving the gate reuses
the paused-run continuation path; the execution itself is unchanged. A
user-approved plan is injected during preparation from durable events via the
project-instructions pipeline pattern — a protected internal system message
with a provenance digest receipt and untrusted-guidance boundary text — and is
re-derived identically on re-preparation and recovery. The deterministic
`EffortRunPlan` remains the sole scheduling authority: plan mode writes only
`confirmed_plan_*` provenance keys and never touches tool, effect, model, or
budget scheduling keys.

Model request generation follows the effort policy: chat requests and the
credential probe disable provider-side thinking for model families whose
builds enable it by default, and the executor loop pins deterministic sampling
(`generation_temperature` `0`) for Fast and Default while High and Extra High
keep provider defaults. `agent-core` owns the metadata key; `model-provider` owns
the wire emission; desktop owns the per-effort value.

### 3. Retrieval and memory

Workspace retrieval and durable memory are independent inputs:

- `agent-rag` searches indexed file chunks and vector storage.
- `agent-graph` provides direct relation lookup and bounded graph walk.
- `agent-memory` ranks project/session memories under trust, utility,
  supersession, conflict, retention, and diversity constraints.

The desktop adapter coordinates provider embeddings, background queues, and
state persistence. `agent-rag` owns the embedding batch planner and the streaming
pass: a batch is cut on whichever binds first — the provider-safe chunk count or a
cumulative chunk-text byte budget — and each batch's vectors are assigned in place
before the next request, so the transient allocation is one batch rather than the
corpus (P1-08). The pass reports its own peak-memory telemetry
(`RagEmbeddingStreamStats`). Because batches land as they arrive, the two fallback
publishers — workspace knowledge and project memory vectors — own restoring the
deterministic local profile across every chunk before publishing a
`local-fallback` generation, so a mid-pass failure can never publish a mixed
index. Retrieved data retains source provenance and does not mutate
canonical chat history.

### 4. No workflow lane

Multi-model workflow collaboration has been physically removed. There is no
owner-execution graph materializer, no workflow checkpoint, no read-only
Specialist/Verifier widening, no verification-repair wave, and no conductor
planning model call. Every run is a single-model Owner execution planned
deterministically by effort tier; the preparation-time route requirements lift
and capability validation keep tool, effect, and vision constraints enforced,
and the foreground Owner owns all effects and final delivery. A residual
workflow decision fails closed rather than executing.

### 5. Kernel and effects

`agent-application` is the outer driver. `AgentKernel` owns the prepared epoch:

1. select the next model/tool action,
2. execute admitted model turns or tool batches,
3. record typed observations,
4. update task and progress contracts,
5. suspend for permission or steer when required,
6. reprepare or select terminal delivery.

Tool exposure and permission are separate. The desktop tool runtime constructs
the exact permission request, checks a matching session capability or asks the
user, executes the effect, and commits the canonical outcome. Effects use the
physical attempt identity for replay and idempotency.

Write subagents (`task` with `allow_patches`) plug into the same permission
path without holding any authority of their own. A write subagent's patch call
is persisted as a pending permission request under the parent run's identity
(task, session, physical run, objectives), but with two deliberate marks:
`session_reusable=false` and a subagent-origin marker. The first makes the
request fail the session-capability policy in both directions — a session
grant the parent already holds for the same path never satisfies a
subagent-originated request, and an `allow_for_session` decision is rejected —
so every patch a write subagent attempts is approved per call by the user.
Rationale: a session grant encodes trust in the parent's current trajectory;
the child is a separately prompted model context whose patch targets the
parent did not necessarily foresee, so widening must be re-authorized at each
call. The subagent gate never consults the session-capability store at all,
so grant non-propagation is structural rather than emergent.

Child model calls carry the parent run's effort tier as `reasoning_effort`
request metadata, so thinking-family providers apply the tier's thinking budget
to subagent calls instead of the previous empty-metadata default (which left
High/Xhigh reasoning unreachable for children). Each child also enforces a
per-child tool-call cap (`SUBAGENT_MAX_TOOL_CALLS`) so parallel children cannot
amplify tool calls unbounded; exhaustion stops the child, not the parent run
(P1-01 resource governance). Child tool calls additionally aggregate into the
parent run's tool-call accounting (`record_external_tool_call`), so the parent
budget and resource snapshot reflect real child usage while the parent enforces
its own limit on its own tool calls. Subagent read tools now also emit durable
started/finished events (`append_tool_proposed_event` /
`append_tool_finished_event`) so child discovery is visible in the run's durable
tool lineage, completing P1-01's tool-accounting unification.

A delegation is durable at its boundaries, not in its middle. The child's own
message history still lives only on its scoped thread — persisting it would make
the durable transcript diverge from the in-memory one the parent reasons over —
but the two things a parent needs after a restart are recorded. The appended
`subagent_result` message is persisted where it is appended, because both commit
points downstream capture their `previous_message_count` after the delegation
appended it and would otherwise skip it, so a restart used to rebuild the
transcript without the delegated answer and close the durable `task` call with a
synthetic "interrupted" observation. The message carries `internal=true`, which
the chat projection drops and the recovery and resume transcripts keep, so the
answer survives without surfacing a raw internal instruction in the thread. The
child's facts ride on its terminal progress event as a
`cindx.agent.subagent-run.v1` record (`agent_runtime::SubagentRunRecord`): the
`task` call id, the description bounded by the same
`SUBAGENT_RECORD_DESCRIPTION_MAX_CHARS` the permission path truncates to, the
write flag, the stop reason, model turns attempted, child tool calls executed,
the answer's size and digest, and the model and effort tier that served it. It
stores the answer's digest rather than its text, so the two durable pieces
corroborate each other without storing the answer twice, and it is written only
when it stays inside the bounds the child loop itself enforces — a counter that
outran its cap, or a record with no call id, fails closed and writes nothing. A
refused delegation never starts a child and so never emits a finish event; its
record rides on the refusal event instead.

The same pass separated the stop reasons the child loop had been collapsing.
`subagent_stage_stop_reason` reports `cancelled` for a stopped run, `steered` for
an objective epoch the child can no longer act on, and `stage_budget` only for a
real Worker-budget refusal, and `SubagentStopReason` gives each of the nine
outcomes its own wire label. A steered child previously returned the
stage-budget answer, so the parent model and the durable record were both told
the delegation ran out of budget when the user had actually moved the goal.

The approval handshake also differs deliberately from the run-level
suspend/resume mechanism. A subagent loop runs on a scoped thread inside the
parent's tool batch, and its internal message history is not part of the
durable runtime snapshot, so suspending the run would abandon the child and
replay the delegation on resume — re-asking the same approval and
double-executing an already-applied patch. Instead the subagent thread parks
on the durable permission row (polling with cancellation), the run stays
active, and the projected pending approval flips the run status to
waiting_for_permission as usual. The `resolve_agent_permission` command routes
subagent-marked requests to an in-place branch that validates and persists
only the resolution rows: it registers no run control (the live run already
owns the session slot) and resumes no loop. The parked subagent thread then
executes an approved patch through the ordinary
`execute_agent_tool_invocation_for_objective_epoch` path, so durable
started/finished events, tool-budget accounting, undo projection, and
workspace-cache invalidation are identical to a parent-run effect. A denial
records the same denied tool-finished event shape and returns to the child as
an ordinary denied observation; a cancellation before approval executes
nothing.

Grounding obligations accept negative evidence: when a tool attempt fails
against an input that matches the obligation's bound target anchors, the
failure is recorded as an absent-target receipt and satisfies the obligation,
stopping repair loops that could never succeed on a missing target. Anchor
matching stays mandatory, so failures against unrelated inputs grant nothing;
the absent receipt is surfaced to the model context so the answer can state
the absence instead of fabricating content.

### 5a. One owner for outbound HTTP policy

Every egress path takes its proxy posture, redirect bound, timeout, response byte
caps, User-Agent, scheme allowlist, and public-address audit decision from
`agent_core::http_policy` for its `HttpEgressProfile`, and every `curl` path
derives its shared argument fragment from `curl_policy_args`. The profiles are
`PublicFetch` (`web.fetch`), `PublicSearchFallback`, `ConfiguredSearchApi`,
`CredentialedSearchApi`, `McpHttp`, `ProviderApi`, and `SkillInstall`; the table
in that module is the single place where those seven postures are written down,
so a change to one is a reviewed change in one file with a test.

The owner lives in `agent-core` because that is the only crate every egress path
can reach: `tools` depends on `model-provider`, so a policy owned by either could
never be shared by both, and `agent-runtime` is gate-barred from the provider
transport stack. The public-address audit (loopback, RFC1918, link-local
including the cloud-metadata range, CGNAT, broadcast, multicast, reserved, and
their IPv6 and IPv4-bearing-IPv6 forms, rejected fail-closed) moved there with the
policy, and `web.fetch` still pins the audited IPs with `--resolve` so the
connection cannot drift between audit and use.

Two postures are deliberate and asymmetric. Only `PublicFetch` refuses ambient
proxies (`--noproxy '*'`), because a proxy would connect on its behalf and bypass
the IP pin that is the SSRF defense; every user-configured endpoint honors the
environment proxy, since refusing it would break provider, MCP, and search access
for users whose only egress is proxied. Only `PublicFetch` audits resolved
addresses, because the other endpoints are chosen by the user's own
configuration, which explicitly supports loopback providers and local search
endpoints. Redirects are bounded everywhere: a credentialed search endpoint
follows none at all (`-L --max-redirs 0`, HTTPS-only) so a bearer header can never
be replayed to another origin, the MCP transport treats a 3xx as an error rather
than a silent re-dial, and the remaining paths share one bound.

### 6. Completion and recovery

Terminal selection cannot convert an unmet obligation into verified success.
The Finalizer role is retired with the serial collaboration architecture:
single-model sessions deliver through the actor, and a forced terminal commit
runs one toolless wrap-up turn of the same session model (actor system prompt,
no Summarizer role, no evolved directive) when no grounded material is
available yet; an unusable wrap-up falls back to the exact eligible grounded
candidate rather than restarting the actor. Empty or unusable wrap-up output
with no verified fallback gets one bounded retry of the gate. When no
deliverable control candidate exists but the run holds visible tool evidence,
the latest substantive visible assistant text (at least 160 chars, internal
drafts excluded) is admitted as a last-resort grounded fallback so a provider
flake cannot fail a run that already did visible work; short progress notes
and evidence-free runs stay closed. Forced terminal commits skip the wrap-up
model call entirely when such grounded material already exists and deliver it
directly. The direct-finalizer phenotype policy module remains dormant pending
physical removal with a prompt-genome schema migration.

Every tier above Fast passes an optional bounded judge gate at the
shared completion chokepoint (both direct completion and terminal-finalizer
routes): when the execution contract requires verification, the configured
Reviewer model is distinct from the executor, and the candidate is not a
fallback, one tool-free Reviewer call audits the
candidate against the objective and returns a typed single-line receipt. A `revise` verdict permits at most one Executor
repair round over the recorded findings and one recheck; a repaired answer is
re-grounded against the task contract before replacing the candidate. Judge
unavailability, inconclusive receipts, empty or ungrounded repairs, and
fallback candidates retain the original answer and record a
`direct_judge_disposition` instead of blocking delivery. The persisted
`direct_judge_fail_closed` provider setting (default off) diverts the two
judged quality failures — an ungrounded repair or a recheck still requiring
revision — to the existing failure terminal for runs with at least one
successful workspace mutation, recording a
`direct_judge_fail_closed_blocked` disposition; infrastructure failures stay
fail-open. Fast execution is
never judged.

The judge's bounded execution facts are not only counts. `agent-runtime`'s
`claim_evidence` module deterministically binds the candidate's workspace
citations (`path`, `path:line`, `path:start-end`, `#Lline`) to the locations the
run actually observed: the tool dispatcher stamps each whitelisted file-tool call
onto its durable tool message (`file.read`/`file.read_many` reads,
`file.search`/`file.list`/`file.glob` subtree scopes, `file.write`/`file.patch`/
`file.patch_batch` mutations — successes and failures alike, since a failed read
is what contradicts a citation), and the judge path reads those stamps back to
classify every citation as supported, unsupported (no observed call covered that
path), or contradicted (the run's own call on that path did not succeed). The
receipt (`cindx.agent.claim-evidence.v1`) is additive judge evidence — it never
blocks delivery on its own — and it is location-level binding, not
natural-language entailment: a citation can bind to real evidence while the prose
around it is still wrong. Citation parsing is shared with the prompt-evidence
anchors (`evidence_target`), so an answer citation and a prompt anchor can never
disagree about what a path is, and a line reference written at the end of a
sentence no longer leaks into the target. An answer that cites nothing produces no
block, so its judge prompt is byte-identical to the previous one.

That same claim-evidence receipt now drives the delivery-verification contract,
which was built and tested but had no verdict producer and therefore never ran.
`agent_runtime::claim_evidence_verdict` is the deterministic producer: a
contradicted citation becomes a `Contradiction` finding, an unsupported one an
`UnsupportedClaim`, and a receipt with no findings — including an answer that
cites nothing — yields `Passed` with the empty finding list the contract
requires. `OmittedObligation` is never produced, because deciding that an answer
omitted an obligation means reading the obligations, which a location-level binder
does not do, and findings carry no obligation or evidence refs for the same
reason. `verify_delivery_against_claims` binds the subject to the exact answer
bytes and reference context, records that verdict, and closes the state: `Passed`
terminates as passed, and `NeedsRevision` terminates as `Unverified` rather than
resting in `AwaitingRepair`, since this contract attempts no repair of its own —
the judge gate owns repair. The claim receipt must have been computed over exactly
the candidate the subject binds, so a receipt for one answer can never verify
another. At the terminal commit the desktop seam records the state
(`cindx.agent.delivery-verification.v1`: status, subject digest, answer digest,
citation count, verdict digest, finding count) beside the grounded-completion
receipt and the outcome ledger. It is additive and fail-open: an unbindable
subject is recorded as `unbound` with its reason, never fails a commit, and never
changes what the user receives.

The completion transaction persists terminal event, result, artifacts,
lifecycle, learning evidence, the observed-use measurement for recalled
memories, and cleanup under one attempt/epoch identity.
Its terminal identity keeps the existing exactly-once key and additionally
validates the matching strategy receipt before a new terminal write. Post-start
preparation failures arbitrate terminal persistence with cancellation and steer
under the same run-control lock; a winning steer replays preparation.
Each new physical attempt keeps session admission and run control registered
until its durable start transaction commits. Initial start/message writes and
continuation recovery-claim/start/replay writes are atomic; pending steers stay
queued for the resumed attempt instead of invalidating its start.
Startup recovery uses durable events and checkpoints and restores the matching
receipt; a run that ended before selection records `not_selected`. Pause and
permission wait are intentionally nonterminal. Permission recovery and retry
preserve logical lineage while keeping physical effects auditable.

`agent-application` also owns
`cindx.agent.externally-verified-outcome.v1`. Evaluation adapters may derive it
only from validated strategy and terminal identity, actual Actor exposure,
external postcondition and preservation receipts, and complete resource
accounting. Missing or tampered provenance is censored; valid unsafe or
preservation-breaking outcomes remain zero-score evidence. The receipt is
shadow-only and has no production routing, prompt, memory, permission, or
serving consumer.

Delivery Verification was a separate `realworld-eval` authority; its desktop
runtime adapter, the `realworld-eval` cargo feature, and the
`cindx-agent-realworld-eval` binary were physically removed in the phase-3
effort-tier rebuild. V1, v2, and v3 are consumed lineages: all nine of their
preflight, authorization, and execute entrypoints return a consumed-protocol
error before reading arguments,
environment, configuration, paths, or live control-plane state. V1 exposed the
semantic/wire digest instrumentation defect. V2 fixed that defect, completed all
eight calibration pairs once, and closed `terminal_futility`. V3 made one
provider attempt for its first calibration Reviewer call, but its journal
rejected a bound zero-byte response artifact before committing a terminal call
receipt and froze `CENSORED`. None can be resumed, relocated, or rerun.

V4 is a distinct protocol and control-plane authority over the exact same tracked
v3 seeded-defect recovery and preservation suite. Its 32 cases, order, hidden
oracle, seeded candidates, model inputs, output contracts, budgets, decision
thresholds, and no-retry rule are unchanged. The suite contains 24 seeded
defects and eight clean preservation sentinels, with 8 calibration / 24 holdout
cases and equal representation of unsupported claims, omitted obligations,
contradictions, and preservation. Every case owns a frozen seed, a model-visible
objective, ordered obligations/evidence, and an output contract that exposes
property names, JSON types, requiredness, and the additional-properties rule.
Exact semantic values remain in the hidden oracle and never enter a model
request. The model-visible request envelope deliberately retains its v3
compatibility schema so the successor does not change prepared semantic or wire
payloads for reasons unrelated to the instrumentation fix.

The control arm is the exact frozen seed. Treatment starts with a model-distinct
Reviewer over that same seed. `passed` returns the unchanged seed; only
`needs_revision` activates one Executor repair followed by one Reviewer
recheck. There is no initial Executor drafting call, second repair, retry, or
case replacement. A preservation seed passes treatment only when it remains
unchanged; an unnecessary repair is therefore a control-only loss even if the
Reviewer accepts it. This topology measures joint defect detection and repair
plus clean-input preservation. It does not estimate uplift over naturally
generated drafts.

A durably terminal provider timeout/unavailability or completed call whose
Reviewer verdict JSON is invalid remains an intention-to-treat treatment failure
for that fixed case, with no retry or replacement. V4 records an exact response
artifact even when it contains zero bytes, including its digest and zero length.
A complete tool-free response with empty content projects as
`invalid_verifier_response`; a response that fails the complete tool-free
contract closes `invalid_output` and remains structural. Internal time-budget,
request-binding, or authority failure likewise closes the campaign
inconclusive.

V4 preflight is provider-free. It binds the clean source HEAD/tree and app
version; protocol, suite, per-case and aggregate case digests; case order;
seeded-candidate, model-input, output-contract, budget, and hidden-oracle
aggregates; redacted provider/model authority; exact execute name, full-file
digest, size, and SHA-256 CodeDirectory identity; and canonical external output
authority. A valid receipt records zero provider calls,
`online_runner_frozen=true`, and `execution_authorized=false`. Authorization is
a separate provider-free step that must revalidate the canonical preflight,
current source/provider/model/credential binding, exact runner identities, new
output root, and short validity window.

Before provider construction, execute compares the frozen CodeDirectory with
macOS's kernel-backed identity for the running process and atomically creates a
no-clobber marker in the output root's parent. All authorization paths for that
canonical output authority share the marker; the output-root tombstone must
byte-match it, so relocating or deleting the output directory does not reopen
consumption. The v4 journal persists campaign and case reservation before work.
Each call then reserves stage/role/model, semantic request digest/size,
immutable prepared wire-payload digest/size, and output budget before dispatch;
the provider receives exactly the prepared non-streaming bytes, and terminal
request metadata must match the reserved wire digest.

Call receipts retain provider-failure classification, retryability, status,
latency, hashed provider identities, exact usage, and response-artifact binding.
Case receipts retain seed/control/treatment authorities, arm outcomes, initial
verdict and finding counts, repair activation, recheck verdict/counts, treatment
disposition, failure stage/code, and outcome reason. Interrupted started calls
are terminal and never resumed or retried. V4's single consumed attempt retained
one zero-byte `invalid_output` artifact, closed its first case
`structural_failure`, and terminated the campaign `inconclusive` with no matched
pair. Its canonical output authority cannot execute again. There is no Delivery
serving, learning, promotion, production-finalizer, or GEPA dependency.

The consumed marker is an intentionally local authority, not an external
anti-rollback ledger. Normal crashes, concurrent consumers, alternate
authorization paths, and output-root relocation fail closed. An actor with the
same user identity who can delete or restore all private control-plane files is
outside that local-filesystem threat boundary.

The portable collaboration-learning contracts and the `collaboration_learning_*`
modules in `agent-application` were removed in the phase-3 effort-tier rebuild
along with the `realworld-eval` adapter that produced them; multi-model
collaboration is permanently retired, so there is no collaboration-learning
policy, exercise receipt, offline-import feature, or train/holdout evidence
contract in the tree. The recall-to-terminal attribution that survives is
unrelated to collaboration learning.

The desktop `realworld-eval` adapter was the only runtime producer; it was
removed in the phase-3 effort-tier rebuild and the paragraph below is retained
as a historical record. Its explicit
successor-only entry installed a matched-arm policy, committed Direct assignment
with the durable strategy decision or Workflow assignment with the materialized
plan, applied the assigned context budget and fail-fast attempt bound, and
committed only the SHA-256 identity and size of the exact encoded request before
dispatch. Raw request contents were never added to the learning receipt. The
adapter retained the run event slice outside the frozen report schema and derived
the comparison binding from the actual pair plus a pre-frozen source/cohort
authority.

The successor evaluation control plane — the
`benchmarks/agent/collaboration-successor-protocol-v1.json` authority, its
desktop preflight, and the Goal 3E authorize/execute binaries — was removed in
the phase-3 effort-tier rebuild; the protocol file is absent from the tree and
the one-shot Goal 3E authority was consumed and closed `CENSORED` before any
valid observation (see the [EVALUATION.md](EVALUATION.md) ledger). It cannot be
rerun. The surviving outcome projector still distinguishes selected, explicit
pre-decision not-selected, absent, and malformed receipt states before outcome
construction, so a legal not-selected terminal remains a censor and can never
become an outcome.

## Persistence and Background Work

SQLite stores projects, sessions, events, permissions, checkpoints, schedules,
memory, and inert measurement projections. Persistent-state failure aborts
startup. Recovery adopts only one valid successor; missing, tampered, or forked
state fails closed, and an unfinished capture becomes a censor rather than a
provider retry.

Background services are bounded and must not block the healthy foreground path:

- Semantic memory curation and vector refresh.
- Schedule dispatch and sidecar health work.
- Workspace `.cindx` retention (aggregate quota plus TTL sweep).

Workspace state retention is owned by the desktop crate
(`cindx_retention_runtime`), because that is the only composition root that can
enumerate every registered workspace root and reach the audited deletion
primitives. One bounded sweep runs per root shortly after startup, at most once
per launch: it measures the whole `.cindx` tree without following a symlink,
expires the regenerable recovery and capture classes past a TTL, and — only if
the aggregate quota is still exceeded — evicts the oldest remaining candidates
until the tree fits. Planning is a pure function over the measurement, and each
deletion re-checks containment and file type at the moment it happens, so a plan
built from an earlier measurement cannot delete through a path that changed
shape. Knowledge and memory-vector generations keep their own lease-guarded
generation-count GC, which this sweep never preempts: semantic state is measured
but never expired or evicted here.

The continuous prompt-evolution loop (prompt evidence projection,
mutation/evaluation, rollout, canary, transfer, and distillation) and the
offline collaboration-learning harness were retired and physically removed in
the phase-3 effort-tier rebuild; they are not background services any longer.

Canonical events remain authoritative. Derived serving snapshots and caches can
be rebuilt; a cache publication failure cannot rewrite the scientific outcome.

One shadow measurement file also lives beside the app data root outside
SQLite: `run-telemetry.journal.jsonl` (0600, capacity-capped, atomically
rewritten). The desktop loop appends one `cindx.agent.run-telemetry.v1`
receipt at each physical run's durable terminal commit (success, failure,
preparation failure, or cancellation) behind the existing exactly-once
terminal identity. It has no production reader or consumer; only tests load
it back.

## Prompt Evolution Boundary

Prompt evolution is retired. The campaign runtime, mutation, pairwise
evaluation, learning outbox, canary/rollout, and distillation machinery have
been removed from the desktop crate and the (now-deleted) `orchestrator` crate.
The prompt-genome / prompt-profile serving machinery is removed as well; a run
carries no prompt profile at all.

The shadow direct-judge outcome journal remains as inert measurement plumbing
with no production consumer; nothing can publish or promote a learned profile.

## Dependency Rules

- Portable crates must not import Tauri or desktop state.
- `agent-runtime` must remain provider-transport independent.
- Tools declare effects; the desktop permission path grants execution.
- Memory and workspace knowledge remain separate stores and provenance domains.
- UI projections may cache derived state but cannot become durable truth.
- Evaluation code cannot promote a profile without production admission gates.

These rules are checked by `scripts/check-desktop-structure.mjs`, Rust tests,
and the quality profiles described in [DEVELOPMENT.md](DEVELOPMENT.md).
