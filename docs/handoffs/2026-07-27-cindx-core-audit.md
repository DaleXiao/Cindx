# Cindx core audit handoff - 2026-07-27

## Purpose

This branch is a source checkpoint for the unfinished Cindx core-audit goal. It is intended to let a new Codex task continue from the exact implementation state without depending on the very long originating conversation.

This is **not a release-ready or behaviorally verified revision**. The user asked to stop implementation, preserve the latest source, publish a handoff branch, and continue in a new task. Rust compilation, automated tests, packaging, installation, and runtime QA have intentionally not been run for this checkpoint.

## Goal that remains active

Complete the remaining source work from the latest Cindx code audit:

- recover agent runtime state and session output caches across app restarts;
- govern reusable browser-session lifecycle safely;
- split oversized modules without changing behavior;
- perform static regression review;
- preserve existing UX, functionality, performance, and agent intelligence.

## Non-negotiable product constraints

- Do not regress existing UX, features, behavior, performance, or agent intelligence.
- Prefer fail-closed state recovery over silently reconstructing invalid durable state.
- Preserve user-authored worktree changes and unrelated files.
- Treat source inspection as weaker evidence than compilation, tests, and runtime verification.
- Do not call this work complete until every requirement has direct evidence.

## Source work present in this checkpoint

### Runtime recovery and output continuity

- `apps/desktop/src-tauri/src/agent_runtime_snapshot.rs`
  - versioned, atomic runtime snapshots;
  - session, project, source-run, prompt-fingerprint, and event-revision binding;
  - terminal snapshot cleanup and recovery handoff support.
- `apps/desktop/src-tauri/src/session_output_cache.rs`
  - revision-aware output projection cache;
  - delta refresh and full rebuild paths;
  - bounded multi-session retention.
- Recovery, session deletion, context, read-model, and bootstrap paths were connected to the new durable state.

### Browser-session lifecycle

- `scripts/sidecars/browser-sidecar.js`
  - atomic session-state writes;
  - per-session action locks with stale-owner recovery;
  - browser and watchdog PID ownership verification;
  - renewable inactivity leases and watchdog cleanup;
  - idempotent close behavior;
  - bounded, oldest-first expired sibling cleanup.
- `scripts/test-browser-sidecar.mjs` contains expanded lifecycle scenarios, but it has not been executed in this checkpoint.

### Tool-effect and recovery semantics

- Tool effect metadata is delegated through nested `tool.invoke` calls.
- Deferred file-write recovery unwraps nested target arguments and verifies expected file content.
- File, metadata, web-search, image-generation, and stream-capture implementations were split out of the former tools giant module.

### Prompt evolution and GEPA evidence integrity

- Promotion checks include blind forward/reverse pairwise evaluation, evaluator independence, exact dataset cohorts, paired train and replay evidence, case/task-class coverage, Wilson confidence, and generalization/safety/format blockers.
- Promoted rollout records now fail closed when their frozen evidence-bound prompt snapshot is missing or invalid.
- Stable prompt selection prefers the validated frozen genome bytes rather than a mutable profile with the same identifier.
- Source tests for the frozen-lineage behavior were added but have not been run.

### Module decomposition

- Model-provider responsibilities were split into request building, image-provider handling, streaming wire handling, and streaming response processing.
- Tool responsibilities were split into focused files rather than remaining in `crates/tools/src/lib.rs`.
- Desktop React giants were decomposed into workspace chrome, session navigation, artifacts, tool-chain, markdown, minimap, settings panels, and focused controllers.
- Structure-budget guards were expanded in `scripts/check-desktop-structure.mjs`, but the guard script itself has not been executed.

## Static evidence collected before handoff

The following checks passed on the checkpoint worktree:

- `git diff --check`
- no Git conflict markers under `apps`, `crates`, `scripts`, or `.github`
- `node --check scripts/check-desktop-structure.mjs`
- `node --check scripts/sidecars/browser-sidecar.js`
- `node --check scripts/test-browser-sidecar.mjs`
- `node --check scripts/build-local-app.mjs`
- parse-only inspection of all 44 TypeScript/TSX sources under `apps/desktop/src`
- static resolution of semicolon-form Rust module declarations across 191 Rust source files

These checks prove only basic source integrity. They do not prove Rust type correctness, cross-crate contracts, runtime behavior, performance, or UX fidelity.

## Deliberately not run

- Cargo or Rust compilation
- Rust, TypeScript, browser-sidecar, integration, or end-to-end tests
- `scripts/check-desktop-structure.mjs`
- npm/Tauri builds
- app packaging, signing, installation, or launch
- runtime performance or memory measurements
- visual regression checks

## Highest-priority continuation work

1. **Finish browser fail-closed review.** `sessionState()` currently converts any state read/parse failure into `{}`. Callers such as `saveActiveTab()` can then write a partial state object and lose PID, profile, launch-token, and session ownership metadata. Decide and implement an explicit missing-versus-corrupt state contract before claiming lifecycle safety.
2. **Audit snapshot event-revision semantics.** Confirm with direct tests whether allowing `snapshot.event_revision <= latest_event_revision` can ever restore stale runtime state after a later durable transition that did not emit a replacement snapshot.
3. **Compile before repairing speculative errors.** Run focused Rust checks first, then fix only evidenced compiler failures. The current checkpoint has not been type-checked.
4. **Run focused behavior tests.** Prioritize runtime snapshot recovery, output-cache restart continuity, nested tool-effect recovery, promoted frozen-prompt lineage, and browser lifecycle/close/expiry behavior.
5. **Run architecture guards and frontend checks.** Confirm module budgets, imports, TypeScript contracts, and existing UX behavior after the decompositions.
6. **Perform end-to-end restart QA.** Start work, restart the app during model/tool states, verify recovery without duplicated effects, and confirm session outputs remain visible.
7. **Only then package or merge.** This handoff branch should not be treated as a release candidate until the focused evidence above is green.

## Worktree boundary

Five visual-QA image deletions existed in the originating worktree and were intentionally excluded from this checkpoint commit:

- `docs/visual-qa/session-compact-1024.jpg`
- `docs/visual-qa/session-compact-inspector.jpg`
- `docs/visual-qa/session-default.jpg`
- `docs/visual-qa/settings-runtime.jpg`
- `docs/visual-qa/trace-wide.jpg`

Do not restore, delete, or commit those files without first confirming the user's intent. They are unrelated to this source checkpoint.

## Suggested new-task bootstrap

Start from the handoff branch, read this document, inspect the live worktree and Git status, and treat current source plus fresh command output as authoritative. Continue with the browser corrupt-state contract first, then compile and run the focused verification matrix. Preserve the five visual-QA deletions as unrelated user changes if they are still present locally.
