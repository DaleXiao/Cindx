# Cindx Documentation

This directory separates current product facts from historical evidence. Code
is the implementation authority; the documents below are the maintained map of
that implementation.

## Current Baseline

Read these in order:

1. [CURRENT.md](CURRENT.md) - shipped behavior, current version, known limits,
   and the evidence boundary.
2. [ARCHITECTURE.md](ARCHITECTURE.md) - runtime flow, crate relationships, and
   ownership boundaries.
3. [TOOL_HARNESS_SPEC.md](TOOL_HARNESS_SPEC.md) - tool, permission, lifecycle,
   MCP, and skill contracts.
4. [AGENT_EVALUATION.md](AGENT_EVALUATION.md) - what is measured, what the
   current evidence says, and what cannot be claimed.
5. [QUALITY_GATES.md](QUALITY_GATES.md) - deterministic, performance, and
   release checks.

Operational documents:

- [SETUP.md](SETUP.md)
- [BROWSER_CONTROL.md](BROWSER_CONTROL.md)
- [RELEASING.md](RELEASING.md)

Research protocol:

- [FUGU_EVALUATION.md](FUGU_EVALUATION.md) describes the frozen comparison
  protocol. It is not evidence of parity.

Architecture decision records under [adr](adr/README.md) preserve why durable
choices were made. They are historical decisions, not a replacement for the
current architecture document.

## Evaluation Evidence

[evaluations/README.md](evaluations/README.md) identifies the current decision
baseline and the latest matched provider diagnostic. Older reproducible reports
are under `evaluations/archive/` and apply only to the commit and version named
inside each report.

An evaluation report without a source revision, protocol boundary, and raw
evidence digest is not retained as project evidence.

## Update Rules

Every change that alters one of these contracts must update the corresponding
current document in the same change:

| Change | Required documentation |
| --- | --- |
| Product behavior, supported mode, or known limitation | `CURRENT.md` |
| Module ownership, dependency direction, or run flow | `ARCHITECTURE.md` |
| Tool, permission, MCP, skill, or lifecycle contract | `TOOL_HARNESS_SPEC.md` |
| Evaluation protocol, benchmark, or result | `AGENT_EVALUATION.md` and `evaluations/` |
| Build, test, performance, or release gate | `QUALITY_GATES.md`, `SETUP.md`, or `RELEASING.md` |

Do not edit an old report to make it resemble current behavior. Add a new
versioned report and update the current evaluation index. Do not add phase
reports, handoff notes, or roadmaps that restate implementation status without
an owner and verification boundary.

Run the documentation check before handing work to another agent:

```sh
node scripts/check-docs.mjs
```
