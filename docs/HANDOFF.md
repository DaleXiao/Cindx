# Current Handoff

This is the single maintained handoff for coding-agent continuation. Update it
in place; do not create versioned handoff files.

## Release Identity

Current release version: `0.2.40`

| Item | Verified value |
| --- | --- |
| Local build source commit | `73cfc4a8917ef45e4e2784162375d388a37e9c16` |
| Tag | Not tagged; latest published tag remains `v0.2.30` |
| GitHub release | Not published; latest published release remains <https://github.com/DaleXiao/Cindx/releases/tag/v0.2.30> |
| Local asset | `dist/Cindx-0.2.35-macOS-arm64.zip` |
| Local asset size | `53,834,452` bytes |
| Local asset SHA-256 | `228c365241855ac96f2c3abb3a80d1c45b02e64d18098332d8095cb84b4f208f` |
| Installed bundle | `/Applications/Cindx.app`, version/build `0.2.35` |
| Installed signature | `codesign --verify --deep --strict` passed |

This is an ad-hoc-signed local validation build, not a tagged, notarized, or
GitHub-published release. Its embedded source revision remains the clean commit
above; the subsequent version/evidence commit does not change the binary.

## Repository State

- Remote: `https://github.com/DaleXiao/Cindx.git`
- Integration branch: `origin/main`
- The local continuation branch may have a different name. Compare it to
  `origin/main`; do not infer divergence from the branch name.
- The installed `0.2.34` local build embeds `e0f8b8b`; the latest published
  `v0.2.30` release embeds `f2ce3b7`. Read later source state from Git rather
  than copying it into this file as a second log.
- The current document set is intentionally limited to `README.md`, `AGENTS.md`,
  and the five files under `docs/`.

Start with:

```sh
git status --short --branch
git fetch --prune origin
git rev-list --left-right --count HEAD...origin/main
git merge --ff-only origin/main  # only when behind with no divergence
node scripts/check-docs.mjs
```

Never use `reset --hard` or force-push to synchronize this checkout.

## Current Agent-Core Position

- Fast is a direct single-model route.
- Auto and Pro use one Conductor-owned typed execution plan. Pro has larger
  bounded planning capacity; it does not blindly activate all models.
- A production Workflow runs exactly one read-only Specialist, optionally one
  model-distinct Independent Verifier, then a deterministic checkpoint handoff
  to the Owner. It has no competing anchor, reviewer tournament, or model
  synthesis layer. A `needs_revision` verdict opens at most one bounded repair
  round before a single recheck.
- One shared kernel owns model/tool turns, permission suspension, steer,
  recovery, and terminal commit.
- Workspace retrieval, durable memory, task graph, and prompt evolution are
  implemented and contract-tested.
- Prompt evolution runs in the background and cannot modify an in-flight run or
  broaden tool/permission authority.
- No recent workflow candidate was promoted. Current evidence does not prove
  general Auto/Pro superiority or Fugu Ultra parity.

The exact flow and ownership are in [ARCHITECTURE.md](ARCHITECTURE.md); do not
reconstruct them from old commit messages or historical reports.

## Last Completed Work

Goal 1 of the post-`0.2.30` Agent architecture work is complete in the current
source:

1. Current Agent model events have explicit Actor, Stage, model-profile,
   service, trust, and effect-authority attribution while retaining every
   legacy role/configuration field.
2. Router and selected-decision events commit atomically behind a preparation
   epoch checkpoint, before treatment execution can advance.
3. Success, failure, cancellation, pause/recovery, and startup reconciliation
   bind the same strategy receipt or an explicit pre-decision `not_selected`
   explanation.
4. Evaluation projection rejects missing, duplicate, tampered, or unlinked
   strategy/terminal receipts.

This work did not change production route selection, model configuration,
budgets, permissions, tool authority, UI behavior, or provider call counts. No
provider evaluation was run, and no intelligence improvement is claimed.

Goal 2 is also complete in the current source:

1. Direct remains the foreground Owner path; Workflow is constrained to one
   Specialist, an optional planned Independent Verifier, and the same Owner.
2. The compatibility synthesis node is completed deterministically from the
   durable checkpoint and consumes no model attempt.
3. Direct-anchor competition, post-team quality/reviewer competition, model
   synthesis, uplift repair, and secondary Conductor replanning were removed
   from the production Workflow path. Bounded retry remains within the same
   logical Specialist or Verifier lane.
4. Permission-gated effects and final delivery remain Owner-only. Missing or
   failed required verification falls through to the direct Owner path.
5. Matched evaluation now validates actual Actor exposure instead of trusting
   Direct/Workflow labels alone, while retaining total call and token accounting.

These are deterministic graph, safety, and attribution guarantees. No provider
evaluation was run, so they do not establish quality, latency, token, or
intelligence improvement.

Goal 3A is complete in the current source:

1. `agent-application` owns one bounded externally verified outcome receipt
   shared by Direct and Workflow.
2. The receipt binds strategy and terminal lifecycle, semantic execution plan,
   actual Actor exposure, external postconditions, preservation, and resource
   accounting before deriving reward.
3. The same integer rule records positive, partial, and negative outcomes;
   valid safety or preservation failures remain zero-score evidence, while
   missing or tampered provenance is censored.
4. Evaluation reuses this portable contract without writing
   `LearningEvidenceV1` or changing production routing, prompts, memory,
   permissions, provider calls, or serving.

No provider evaluation was run, so this is deterministic reward-plumbing
evidence, not an intelligence or collaboration-uplift result.

The Goal 3B contract foundation is complete in the current source:

1. Collaboration candidates use a bounded structured policy and retain the
   Goal 2 Owner, Specialist, Verifier, authority, and serial-graph limits.
2. Policy assignment is accepted only when a trusted event projection binds
   the semantic plan, exact context receipt, per-lane attempts and repair,
   derived stop reason, and Goal 3A externally verified outcome.
3. Candidate lineage changes one bounded axis. The in-memory evidence contract
   is content-addressed, deduplicated, train/holdout separated, and retains both
   valid zero-score outcomes and explicit censor records.
4. Missing attribution, safety or preservation failure, resource regression,
   holdout failure, budget exhaustion, or no causal uplift freezes the evidence
   set. Readiness still requires an independent review receipt before an
   offline narrow-validation record can be approved.
5. The contract has no production routing, prompt, memory, permission, canary,
   snapshot, or serving consumer.

Goal 3C adds the provider-free runtime and recovery adapter around that
foundation:

1. A successor-only `realworld-eval` entry commits the matched policy assignment
   before model work, applies the assigned worker context budget and fail-fast
   bound, and records only the SHA-256 and size of the exact encoded request
   before dispatch. The frozen V12 entry continues to pass no learning policy.
2. The trusted projector aggregates all worker turns and rejects missing,
   duplicated, reordered, or mismatched run/plan/step/model/context receipts.
3. Canonical offline genesis and Pair/Censor entries replay through the public
   constructors into the same sealed evidence contract.
4. The private external journal uses immutable entries and an atomic manifest;
   one valid orphan may be adopted, while missing, tampered, forked, or pending
   state fails closed or becomes `IncompleteInstrumentation`. It has no provider
   retry callback.
5. The capture bridge derives the comparison binding from both actual runs and
   accepts only a pre-frozen source/cohort authority; it remains evaluation-only.
   The invalid V12 protocol was not changed or rerun.

No provider evaluation was run, so these are deterministic collection and
recovery guarantees, not evidence that collaboration improves intelligence or
performance.

Goal 3D freezes the final narrow successor protocol without authorizing it:

1. The tracked `collaboration-successor-v1.json` suite contains three new,
   route-blind cases for exactly three matched pairs / six runs: baseline train,
   candidate train, and sealed holdout.
2. The tracked `collaboration-successor-protocol-v1.json` manifest binds the
   suite and complete case contracts, arm ordering, the existing conservative
   Workflow evaluation run budget, its six-run aggregate, and terminal stop
   rules.
3. There is exactly one candidate. It preserves the Owner + one read-only
   Specialist topology, planned verification, and fail-fast repair, changing
   only the Specialist context allocation from 5,000 to 7,500 bps.
4. A preflight-only binary binds a clean source HEAD/tree, redacted provider and
   full configured-model catalog, materialized prestates, budgets, cohort, and
   new external output paths into a private receipt. It performs zero provider
   calls and its tracked authority keeps `execution_authorized=false`.
5. At the Goal 3D checkpoint no private online authorization had been generated
   and provider-action reservation/execution was not wired. The invalid V12
   attempt and production serving remained unchanged.

No provider evaluation was run for Goal 3D. The frozen protocol is not a causal
result and establishes no quality, latency, token, or intelligence uplift.

Goal 3E implements the bounded execution control plane around that frozen
authority:

1. A provider-free authorization binary accepts only the explicit frozen
   protocol command and can mint one private 15-minute receipt bound to the
   canonical preflight, current clean source and provider/model configuration,
   exact execute binary, fixed matrix and budgets, and new output root.
2. The execute binary revalidates and consumes those bindings once, persists
   the campaign before provider-capable setup, and durably reserves each cell
   and arm before its model action.
3. Baseline admission precedes the sole candidate, candidate admission precedes
   holdout, and the controller ends only ready for independent review, frozen,
   or censored. Recovery never resumes or retries a started physical run.
4. The provider-free execution contract contains 17 deterministic tests. The
   collaboration-learning gate now contains 18 tests. These counts prove
   contracts, not uplift.

Goal 3E was authorized and consumed once on `12a3ea2` / `0.2.32`. The private
journal closed `CENSORED` after reserving the baseline Direct arm: the
product run ended before selection with zero selected decisions. The terminal
producer correctly persisted explicit pre-decision `not_selected`; no treatment
or Owner execution occurred. The selected-only external-outcome projector
misclassified that legal state as a malformed receipt, leaving zero valid
observed runs. Workflow, candidate training, and holdout did not run. This is
`INVALID_EVIDENCE`, not a collaboration or capability result; the frozen Goal
3E protocol must not be retried. V12, the tracked successor manifest/suite, and
production serving remain unchanged.
Current source now distinguishes the legal pre-decision state before selected-
outcome projection and covers the real terminal-producer-to-projector seam.
This corrects classification only; it does not recover or reinterpret the run.

The current Settings and runtime model-slot alignment passed the focused
frontend and routing contracts, the full deterministic profile (`57/57`
gates), and the preserved Goal 3 contracts. The single formal `0.2.34` arm64
build from `e0f8b8b` then passed strict signature, clean-start, and fail-closed
persistent-state probes before installation. No provider evaluation was run,
so this validates configuration semantics and compatibility rather than an
intelligence uplift.

Delivery Verification remains default-off and separate from production serving
and GEPA. V1, v2, and v3 are consumed authorities. Their nine public preflight,
authorization, and execute entrypoints are permanently fail-closed before
arguments, environment, configuration, paths, or live-state access.

Delivery Verification v1 was then authorized and consumed once on source
`5373e65`. It closed `CENSORED` / `INVALID-INSTRUMENTATION` during the first
calibration Owner call. The reservation stored a canonical semantic-request
digest, while uncommitted provider result metadata supplied a separately
domain-separated wire-payload digest; journal terminal validation incorrectly
required them to be equal.
The journal charged one logical and one physical reservation but accepted zero
terminal model-call receipts and no case receipt. Calibration never completed,
holdout never opened, and no matched pair exists. An unbound response artifact
is excluded from evidence. This is not an uplift, no-evidence, regression,
quality, latency, usage, or cost result, and v1 must not be rerun. The production
finalizer, Workflow, Settings, prompt evolution, GEPA, routing, permissions,
serving, installed App, and published release remain unchanged.

Delivery Verification v2 was subsequently authorized and consumed once at
source `275d79b883192bf4148f13123e0d0de788aa346d` / version `0.2.34`.
All eight calibration pairs completed in 16 calls: seven both-pass and ordinal
3 both-fail, with no treatment-only win or control-only loss. The unique failure
was a frozen case-definition mismatch between the visible
`controlling_revision` key and the hidden exact oracle's `revision` key. The
fixed gate closed `terminal_futility`, and all 24 holdout cases were durably
skipped. This is neither uplift nor regression evidence. V2 must not be rerun.

Delivery Verification v3 was then authorized and consumed once. The first
calibration Reviewer call was durably reserved and made exactly one provider
attempt. The provider wrapper returned empty content, but journal validation
rejected the otherwise bound response artifact solely because its byte count was
zero. It froze `CENSORED` before accepting a terminal call receipt or case
receipt. Calibration did not complete, holdout never opened, and no matched pair
exists. This is invalid instrumentation with no scientific conclusion; v3 must
not be rerun.

V4 is the provider-free successor authority over the exact same tracked v3
seeded-defect recovery and preservation suite, not a natural-draft uplift
experiment:

1. The unchanged 32-case suite freezes 24 seeded defects and eight clean
   preservation sentinels, split 8 calibration / 24 holdout. Each of four
   strata has two calibration and six holdout cases. Case bytes, order, hidden
   oracle, seeds, model inputs, output contracts, budgets, decision thresholds,
   and no-retry policy are identical to v3.
2. Control is the exact frozen seed. Treatment starts with a model-distinct
   Reviewer; `passed` preserves the seed, while `needs_revision` permits one
   Executor repair and one Reviewer recheck. There is no initial drafting call,
   second repair, retry, or replacement.
3. Model-visible output contracts expose property names, JSON types,
   requiredness, and additional-property policy. Exact oracle values remain
   evaluator-only.
4. An incomplete calibration or any structural/treatment-execution failure
   closes `inconclusive` before the decision gate. Among eight complete,
   structurally eligible cases, exactly six control failures, at least four
   treatment-only wins, at least one win per defect stratum, and no loss opens
   holdout; an eligible threshold miss closes `terminal_futility`.
5. Holdout requires all 24 cases, exactly 18 control failures, at least 13
   treatment-only wins, at least four wins per defect stratum, and no loss for
   `seeded_repair_effective`; structural/treatment-execution failure is
   ineligible. A clean-sentinel loss is
   `preservation_regression`; an otherwise valid sub-threshold result is
   `not_effective`.
6. Requests are tool-free and non-streaming, with zero transport retries, one or
   three calls per case, 96 total calls/attempts, 64,000 tokens per case,
   2,048,000 campaign tokens, and six hours.
   Durable provider timeout/unavailability or invalid Reviewer verdict JSON is
   an intention-to-treat treatment failure; internal time-budget, binding, or
   authority failure is structural and closes inconclusive.
7. Provider-free preflight binds clean source HEAD/tree/version; protocol,
   suite, case, order, seed, model-input, output-contract, budget, and hidden
   oracle authorities; redacted provider/model authority; exact execute
   full-file digest/size and CodeDirectory; and new canonical external paths.
   It records zero provider calls and `execution_authorized=false`.
8. Authorization remains a separate explicit one-shot step. Execute must match
   the frozen runner's full-file and kernel-backed CodeDirectory identities and
   atomically consume the shared no-clobber output marker before provider work.
9. Journal v4 reserves campaign/case state and then stage, role, configured
   model, semantic request digest/size, immutable prepared wire digest/size, and
   output budget before every dispatch. Terminal metadata must match the
   reserved wire authority. Exact zero-byte response artifacts retain their
   digest and zero length; completed empty Reviewer content becomes
   `invalid_verifier_response` without retry. Started calls never resume or
   retry.
10. Case receipts retain seed/control/treatment digests and outcomes, verifier
    decisions/finding counts, repair activation, recheck telemetry, treatment
    disposition, failure stage/code, outcome reason, and exact accounting.

V4 was subsequently frozen from clean merged source
`275868e4dd84692f15998a1cb95afa99df264267` / tree
`75f304e160c0b7bab37f5158b383fade970af271`, authorized, and consumed once.
The zero-byte instrumentation fix worked: the first Reviewer response was
committed as a 0600 empty artifact with exact provider identity, usage, latency,
and request bindings. The call was nevertheless `invalid_output`, non-retryable,
because it was not a complete tool-free answer. Case 1 closed
`structural_failure`, the campaign closed `inconclusive`, and 31 cases never
started. Accounting is one logical call, one physical attempt, one terminal
call, and zero retries. There are zero matched pairs and no calibration or
holdout decision. V4 must not be rerun, and this result is not repair
effectiveness, ineffectiveness, preservation regression, or model-quality
evidence.

The `0.2.30` release itself contains two scoped code changes after `0.2.29`:

1. `ff8c238` separated route/task-graph evidence from direct-finalizer evidence
   in the V12 causal evaluation path.
2. `b5500cf` recorded the one authorized V12 attempt as
   `INVALID_EVIDENCE` without changing production serving.

This documentation cleanup removes obsolete reports and duplicate explanations
from the current tree. Full historical documents remain available at
`f2ce3b7` and earlier commits.

Workspace undo/redo is wired into the file tools and the session surface:
`file.write` and `file.patch` capture pre-change bytes under
`.cindx/undo-history` on a best-effort basis (matching the output-history
contract), and a per-session undo registry stored as a CAS-protected read
model supports last-in-first-out undo/redo with content-hash conflict guards.
Composer renders Undo/Redo controls when a session has recorded changes. The
9-test `workspace-undo-contract` gate joins `quick`, `ci-contract`,
`control-plane`, and `full`. No provider evaluation was run; route selection,
permission authority, budgets, learning consumers, and serving are unchanged.

Project instruction files are wired into run preparation: bounded discovery of
`AGENTS.md` from the workspace root up to the Git root plus
`.cindx/instructions/*.md`, a protected `ProjectInstructions` context source,
untrusted-guidance boundary text, and a provenance receipt in the preparation
commit event. The 17-test `project-instructions-contract` gate joins `quick`,
`ci-contract`, `control-plane`, and `full`. The feature is enabled by default;
until a Settings UI exists it is toggled only through
`project_instructions.json` in the app support directory. No provider
evaluation was run; route selection, permission authority, budgets, learning
consumers, and serving are unchanged.

Custom commands are wired end to end: markdown command files are discovered
from the workspace `.cindx/commands/` and global `~/.cindx/commands/`
directories (project names override global names, capped at 24 files and
8 KB each), listed through the `get_custom_commands` command, and rendered in
a Composer toolbar menu. Selecting a command expands its template into the
draft (`$ARGUMENTS` substitutes the current draft text) and applies an
optional effort override. The 9-test `custom-commands-contract` gate joins
`quick`, `ci-contract`, `control-plane`, and `full`. No provider evaluation
was run; route selection, permission authority, budgets, learning consumers,
and serving are unchanged.

The chat request builder and the credential verification probe now set
`enable_thinking: false` for model families whose provider builds enable
reasoning by default (`qwen`, `qwq`, `glm`, `kimi`, `deepseek`). This keeps
bounded output budgets spent on the answer instead of provider-side reasoning
traces — the failure class behind the empty-content responses observed in the
consumed Delivery Verification v3 and v4 Reviewer calls. Five deterministic
contract tests cover the wire-format and probe behavior. Other model families
keep provider defaults, and there is no runtime or per-effort toggle yet.
Consumed one-shot protocols were not rerun or reinterpreted; no provider
evaluation was run, and no intelligence, quality, or latency uplift is claimed.

The workflow checkpoint now contracts one bounded verification-repair round:
`begin_verification_repair` accepts a completed verification step whose
`needs_revision` receipt is valid, returns its audited steps to a pending state
under one additional granted model turn, keeps the revision receipt readable
for repair prompting, and caps the verification step at one repair round before
its single recheck. Two deterministic contract tests cover the transition and
its guards. The adaptive frontier driver now schedules the round: after each
wave reconciliation it opens the repair transition, requeues the audited and
verification candidates through `AnytimeController::requeue_for_repair`, keeps
the revision receipt readable, and injects the unresolved findings into the
repaired worker prompt before the single recheck. A desktop contract test
pins the exactly-once requeue; a second revision verdict still exhausts the
budget and falls through to the Owner.

Auto and Pro direct execution now contract a delivery judge:
`orchestrator::direct_judge` defines the typed single-line receipt
(`CINDX_DIRECT_JUDGE`, `pass`/`revise` with findings), the model-distinct
reviewer selection, the eligibility gate (never Fast), the judge prompt, the
repair directive, and the one-repair/one-recheck cap, while
`direct_judge_runtime` plans and resolves the judge round on desktop. Nine
deterministic tests pin the contract and the run-context parsing.

The first absence-evidence build regressed one bounded-context contract
test: an unconditional `grounding_absent:false` marker grew every grounding
capsule and pushed an at-the-edge context budget over the governance limit.
The marker is now emitted only when absence is recorded, keeping capsules
byte-stable in the common case.

The direct judge now receives bounded execution facts alongside the
objective and candidate answer: successful workspace mutation count, whether
mutations carry post-mutation verification evidence, the workspace
verification policy, and whether grounding evidence was recorded. Judge and
recheck prompts both carry the facts; a deterministic test pins the summary
shape.

Smoke testing also reproduced a grounding death spiral: tasks requesting a
workspace file that does not exist registered a workspace_grounding
obligation that no successful tool call could ever satisfy, because evidence
recording only counted succeeded calls. The kernel now records anchor-matched
failed attempts as absent-target receipts that satisfy the obligation and are
surfaced to the model context with a grounding_absent marker; two new
deterministic tests pin the anchor-mandatory, epoch-checked, no-free-pass
behavior. Smoke testing found that completed
deliveries bypass the terminal finalizer (zero finalizer events in 312
historical completions), so the gate now sits on the shared completion
chokepoint (`finalize_agent_completion`), covering both direct completion and
terminal-finalizer routes. Eligible direct Auto/Pro deliveries run one judge
call, at most one repair round with one recheck, and every fail-open path
records a `direct_judge_disposition` on the completion context. Live smoke
testing exercised judge pass, verified-revise repair, and the ungrounded
fail-open path; repaired answers rebind the original visible evidence
sequences (mirroring the finalizer fallback) so grounding-restricted tasks
can accept a repair.

Chat request preparation also accepts an optional `generation_temperature`
metadata override that is clamped into the provider range and carried on both
streaming and non-streaming wire payloads; without the key the provider default
is unchanged. The executor loop now produces it per effort: Fast and Auto pin
`0` for deterministic sampling through the shared `AgentLoopState` metadata
hook, Pro keeps provider defaults, and the field survives loop reprepare.
Collaboration worker, Conductor, and utility calls still use provider
defaults. No provider evaluation was run for either change; routing, budgets,
permissions, learning consumers, and serving semantics are otherwise
unchanged.

Frontend component logic extraction continues: the Composer textarea
auto-sizing computation and the attachment batch validation rules now live in
the pure `composerSizingModel.ts` and `attachmentLimitsModel.ts` modules with
11 node tests, and `Composer.tsx` / `attachmentIpc.ts` consume them. Behavior
is unchanged; this reduces untested logic inside components without touching
the gate-pinned Composer assertions.

## Blocking Fact

V12 did not produce a causal result:

- Direct retained complete route evidence but failed terminal completion after
  nine model calls and `54,579` tokens.
- Workflow stopped before `Agent run decision selected`/strategy-event
  persistence.
- Therefore there is no matched pair, GO/NO-GO, candidate, snapshot, promotion,
  or intelligence conclusion.

Do not authorize or run V12 again. The instrumentation defect is the result.

## Next High-Value Goal

Two approved directions from the execution-grounded verification program
remain, each deliberately scoped to its own session:

1. Bounded read-only parallel exploration. The owner-execution graph pins
   exactly one Specialist (owner_execution_graph validation plus contract
   tests pin it); widening to at most two Analysis specialists for
   read-only exploration tasks requires a deliberate invariant change:
   a graph-shape variant, conductor contract selection for read-only
   task classes, anytime candidate fan-out under the existing branch
   budget, and verifier audit of both branches. Do not loosen the
   single-Specialist invariant for mutation-bearing graphs.
2. Real reward into prompt evolution. The judge disposition is recorded on
   the completion context (`direct_judge_disposition`); the next step is to
   project it plus post-mutation verification success into outcome evidence
   and use it as prompt-evolution fitness, keeping the shadow-only evidence
   boundary until an independent review admits a record.

Do not start either change in the middle of an unrelated session; each needs
its own contract tests and documentation pass.



The bounded Goal 3 instrumentation repair is complete: `selected`, explicit
pre-decision `not_selected`, absent, and malformed states are distinct; legal
`not_selected` remains a censor; and the real terminal producer-to-projector
seam is covered. The deterministic gates and one formal App build are complete;
stop this line of work. Do not create Goal 3F, run a provider, or revise Goal 3E
cases, candidate, budgets, ordering, holdout, evaluator, or receipts to rescue
the consumed run.

Do not rerun Delivery Verification v1, v2, v3, or v4. The v4 preflight digest is
`f0264540eeefde1c66e7933c521e60b129d235359d236253156d68df74a84635`,
its consumed authorization digest is
`bbdf2bbb7e1e2c189f0eaa4ee0cc800806742a8ba430dfa71a8a4d6fd4f956f9`,
and its terminal journal digest is
`272d3102c6118c9b1cc4d136c64d285a5629725543920a2f1922dae22e05d616`.
The advance one-shot authorization is exhausted; there is no remaining online
authority. A future Delivery Verification attempt must be a separately frozen
successor and must not revise v4 cases, order, oracle, seeds, model inputs,
output contracts, budgets, thresholds, or no-retry behavior to rescue this
result. Do not weaken the complete tool-free response contract or normalize the
provider's reported 2,049 completion tokens to fit the 2,048 reservation without
a separately reviewed protocol change. Do not change production serving or
GEPA, and do not create an App build, version tag, or GitHub release for this
evidence-only update.

## Structural Risks

- The desktop composition root still declares and imports many sibling modules.
  Refactor by moving complete ownership with tests, not by adding more files.
- Provider evaluation campaigns are feature-gated but still compile through the
  desktop adapter; keep them out of shipping authority.
- Large frontend surfaces remain in `App.tsx`, `tauri.ts`, Settings, Inspector,
  and thread styles. Avoid unrelated UX changes while working on the core.
- Full builds can create many gigabytes of reproducible Rust output. Run one
  formal build only when a release is required.

## Verification and Release

Use [DEVELOPMENT.md](DEVELOPMENT.md) for the exact gates. At minimum for a code
change:

```sh
node scripts/check-docs.mjs
node scripts/check-desktop-structure.mjs
git diff --check
```

Choose the quality profile that covers the changed owner. Provider runs require
separate authorization. A deterministic green result is not intelligence
evidence.

Before a future release, verify version sources, tag target, GitHub asset digest,
installed bundle version, and strict code signature. Use one build and remove
only reproducible `target`, `node_modules`, `dist`, and temporary evaluation
outputs after delivery.
