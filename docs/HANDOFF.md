# Current Handoff

This is the single maintained handoff for coding-agent continuation. Update it
in place; do not create versioned handoff files.

## Release Identity

Current release version: `0.2.32`

| Item | Verified value |
| --- | --- |
| Local build source commit | `ef2cac38f5b4455e13f3e57ac3d48a8e5b8275e6` |
| Tag | Not tagged; latest published tag remains `v0.2.30` |
| GitHub release | Not published; latest published release remains <https://github.com/DaleXiao/Cindx/releases/tag/v0.2.30> |
| Local asset | `dist/Cindx-0.2.31-macOS-arm64.zip` |
| Local asset size | `53,780,372` bytes |
| Local asset SHA-256 | `26fc553805615221c87483ddca67b13ac0c71d6e661e6b095fa53e501fffa337` |
| Installed bundle | `/Applications/Cindx.app`, version/build `0.2.31` |
| Installed signature | `codesign --verify --deep --strict` passed |

This is an ad-hoc-signed local validation build, not a tagged, notarized, or
GitHub-published release. Its embedded source revision remains the clean commit
above; the subsequent version/evidence commit does not change the binary.

## Repository State

- Remote: `https://github.com/DaleXiao/Cindx.git`
- Integration branch: `origin/main`
- The local continuation branch may have a different name. Compare it to
  `origin/main`; do not infer divergence from the branch name.
- The installed `0.2.31` local build embeds `ef2cac3`; the latest published
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
  synthesis layer.
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
external-outcome projector rejected a malformed persisted strategy receipt,
leaving zero valid observed runs. Workflow, candidate training, and holdout did
not run. This is `INVALID_EVIDENCE`, not a collaboration or capability result;
the frozen Goal 3E protocol must not be retried. V12, the tracked successor
manifest/suite, and production serving remain unchanged.

The Goal 3C revision passed the full deterministic profile (`55/55` gates),
the offline application contract (`5/5`), and the desktop successor adapter
contract (`12/12`). The formal `0.2.31` arm64 build then passed strict signature,
clean-start, and fail-closed persistent-state probes before installation.

The `0.2.30` release itself contains two scoped code changes after `0.2.29`:

1. `ff8c238` separated route/task-graph evidence from direct-finalizer evidence
   in the V12 causal evaluation path.
2. `b5500cf` recorded the one authorized V12 attempt as
   `INVALID_EVIDENCE` without changing production serving.

This documentation cleanup removes obsolete reports and duplicate explanations
from the current tree. Full historical documents remain available at
`f2ce3b7` and earlier commits.

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

Do not rerun Goal 3E. First diagnose and repair the persisted strategy-receipt
instrumentation that failed the external-outcome projection, with deterministic
coverage for the exact malformed lineage. Only after that fix is merged may a
new, separately named successor protocol be frozen and considered for another
explicitly authorized provider attempt. Do not revise Goal 3E cases, candidate,
budgets, ordering, holdout, evaluator, or receipts to rescue the consumed run.

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
