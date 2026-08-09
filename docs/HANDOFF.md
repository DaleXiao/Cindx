# Current Handoff

This is the single maintained handoff for coding-agent continuation. Update it
in place; do not create versioned handoff files.

## Release Identity

Current release version: `0.2.30`

| Item | Verified value |
| --- | --- |
| Release source commit | `f2ce3b7a08585472eb678d9e783d4c50caa4378c` |
| Tag | `v0.2.30` |
| GitHub release | <https://github.com/DaleXiao/Cindx/releases/tag/v0.2.30> |
| Asset | `Cindx-0.2.30-macOS-arm64.zip` |
| Asset size | `53,023,332` bytes |
| Asset SHA-256 | `02826e60135d24eff06b3b727ed17a219525623ad0441651ab7ded53c318b254` |
| Installed bundle | `/Applications/Cindx.app`, version/build `0.2.30` |
| Installed signature | `codesign --verify --deep --strict` passed |

The release is the packaged application requested for GitHub. Documentation
cleanup after the tag does not change the binary; do not rebuild merely to
include Markdown changes.

## Repository State

- Remote: `https://github.com/DaleXiao/Cindx.git`
- Integration branch: `origin/main`
- The local continuation branch may have a different name. Compare it to
  `origin/main`; do not infer divergence from the branch name.
- Product source at the release is `f2ce3b7`. Subsequent current-main changes
  should be read from Git rather than copied into this file as a second log.
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
  bounded collaboration capacity; it does not blindly activate all models.
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

The `0.2.30` release contains two scoped code changes after `0.2.29`:

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

Make strategy-event persistence and terminal lifecycle state one atomic,
observable boundary shared by Direct and Workflow evaluation arms, without
changing production route selection or UX.

Success criteria are deliberately narrow:

1. Both treatments persist the selected decision before execution can advance.
2. Terminal success, failure, cancellation, and interruption each bind to that
   decision receipt.
3. Recovery cannot leave a reserved provider action without a durable strategy
   or terminal explanation.
4. Existing Fast/Auto/Pro behavior, permissions, budgets, and serving profiles
   remain unchanged.
5. Deterministic lifecycle and replay tests pass before a new provider protocol
   is proposed.

Only after that goal should a new, small, frozen matched Direct-vs-Workflow
protocol be designed. Do not add prompt genes, roles, evaluators, or broader
benchmarks first.

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
