# Cindx Collaboration Contract

Source code is the implementation authority. Before changing behavior or
architecture, read:

1. `docs/CURRENT.md`
2. `docs/ARCHITECTURE.md`
3. `docs/EVALUATION.md`
4. `docs/HANDOFF.md`
5. `docs/DEVELOPMENT.md` for the required checks

## Scope

- Confirm the checkout, branch, remote, and worktree before editing.
- Preserve unrelated user work. Never use destructive Git commands to make a
  dirty tree look clean.
- Make the smallest cohesive change that satisfies the requested behavior.
- A file split is not an architecture improvement unless ownership and
  dependency direction become narrower.

## Documentation

Update the matching maintained document in the same change:

- Product behavior or known limits -> `docs/CURRENT.md`
- Ownership, dependencies, or run flow -> `docs/ARCHITECTURE.md`
- Tests, build, release, or operational behavior -> `docs/DEVELOPMENT.md`
- Benchmarks, provider results, or claims -> `docs/EVALUATION.md`
- Continuation state for another coding agent -> `docs/HANDOFF.md`

There is exactly one handoff document. Update it in place. Do not add phase
reports, roadmaps, duplicate architecture summaries, per-run evaluation prose,
or checked-in raw provider transcripts. Git history is the archive.

Run `node scripts/check-docs.mjs` for every change.

## Evidence

- Deterministic tests prove contracts, not intelligence uplift.
- Provider-backed claims require a frozen revision and cases, retained failures,
  complete receipts, budgets, confounds, and an explicit decision.
- Incomplete or invalid evidence fails closed. Do not claim GEPA improvement,
  Auto/Pro superiority, or Fugu parity without matching evidence.
- Do not rerun a one-shot provider protocol after its frozen attempt. Design a
  successor protocol only after the observed instrumentation defect is fixed.

## Completion

Use the concern-specific gates in `docs/DEVELOPMENT.md`. A release is complete
only when source, version metadata, tag, GitHub asset, installed bundle, and
signature agree. Use one formal build and clean only reproducible outputs.
