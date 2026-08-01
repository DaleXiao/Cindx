# Evaluation Evidence

This directory contains current decision evidence. Reports describe only the
source revision and application version recorded inside them.

## Current Product Decision

- [Cindx Agent Real-World Lite Goal 6 0.1.80](CINDX_AGENT_REALWORLD_LITE_G6_0.1.80_2026-08-01.md)
  is the current provider-backed Agent diagnostic on source commit `e40960c`.
  Auto matched the direct answer-quality ceiling, but Fast and Pro regressed and
  collaboration cost remained high. Its decision is `NO-GO` for a collaboration
  or intelligence-uplift claim.
- [Sanitized machine-readable result](CINDX_AGENT_REALWORLD_LITE_G6_0.1.80_2026-08-01.json)

## Latest Matched Provider Diagnostic

- [Cindx Provider-backed Matched Baseline 0.1.78](CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md)
  compares Direct, Auto, and Pro on 12 frozen GPQA-Diamond questions.
- [Sanitized machine-readable result](CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.json)

This GPQA report predates the current application version and is a reasoning
diagnostic, not the current Agent decision baseline.

## Historical Evidence

Reproducible older reports, including the earlier same-day `0.1.80` Agent pilot,
are under [archive](archive/README.md). They remain
available for regression history but must not be quoted as current behavior.

Reports with unknown source revisions, superseded duplicate pilots, unfinished
handoff notes, and implementation-stage summaries without provider evidence
were removed. Git history remains the source for those deleted artifacts.

See [../AGENT_EVALUATION.md](../AGENT_EVALUATION.md) for evidence levels,
required report fields, and the next product gate.
