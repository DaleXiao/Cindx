# Evaluation Evidence

This directory contains current decision evidence. Reports describe only the
source revision and application version recorded inside them.

## Current Product Decision

- [Cindx Agent Real-World Lite 0.1.80](CINDX_AGENT_REALWORLD_LITE_0.1.80_2026-08-01.md)
  is the current tool-using Agent baseline. Its decision is `NO-GO` for an
  intelligence-uplift claim.

## Latest Matched Provider Diagnostic

- [Cindx Provider-backed Matched Baseline 0.1.78](CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md)
  compares Direct, Auto, and Pro on 12 frozen GPQA-Diamond questions.
- [Sanitized machine-readable result](CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.json)

This GPQA report predates the current application version and is a reasoning
diagnostic, not the current Agent decision baseline.

## Historical Evidence

Reproducible older reports are under [archive](archive/README.md). They remain
available for regression history but must not be quoted as current behavior.

Reports with unknown source revisions, superseded duplicate pilots, unfinished
handoff notes, and implementation-stage summaries without provider evidence
were removed. Git history remains the source for those deleted artifacts.

See [../AGENT_EVALUATION.md](../AGENT_EVALUATION.md) for evidence levels,
required report fields, and the next product gate.
