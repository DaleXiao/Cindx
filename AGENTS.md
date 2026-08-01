# Cindx Collaboration Baseline

Read these before changing product behavior or architecture:

1. `docs/CURRENT.md`
2. `docs/ARCHITECTURE.md`
3. `docs/AGENT_EVALUATION.md`
4. The concern-specific document linked from `docs/README.md`

Source code is the implementation authority. Current documents are the shared
map; archived evaluations are evidence for their recorded revision only.

## Documentation Contract

Update documentation in the same change when code alters a documented contract:

- Product behavior, modes, or known limits -> `docs/CURRENT.md`
- Module ownership, dependencies, or run flow -> `docs/ARCHITECTURE.md`
- Tools, permissions, MCP, skills, or lifecycle -> `docs/TOOL_HARNESS_SPEC.md`
- Benchmarks, protocols, or measured results -> `docs/AGENT_EVALUATION.md` and
  `docs/evaluations/`
- Build, test, performance, or release behavior -> the matching operational doc

Do not add phase reports, handoff notes, or roadmap documents that duplicate
current status. Do not rewrite historical reports to match new code.

## Evidence Contract

Do not claim intelligence uplift, GEPA improvement, or Fugu parity from
deterministic tests. Provider-backed reports require an exact source revision,
version, frozen protocol, budgets, failures in the denominator, evidence digest,
confounds, and an explicit decision. Raw prompts, secrets, protected benchmark
content, and full provider outputs stay outside Git.

## Completion

Run `node scripts/check-docs.mjs` for every change. Run the concern-specific
checks in `docs/QUALITY_GATES.md` before claiming a behavior or performance
result. A file split without narrower ownership and dependency direction is not
an architecture improvement.
