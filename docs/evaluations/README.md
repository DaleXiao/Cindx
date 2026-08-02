# Evaluation Evidence

This directory contains current decision evidence. Reports describe only the
source revision and application version recorded inside them.

## Current Product Decision

- [Cindx Agent Real-World V2 0.1.82](CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md)
  is the latest provider-backed collection attempt on source commit `6b2ee39`.
  Four RAG/memory cells ended in infrastructure failure, so its decision is
  `INVALID_BASELINE`. It is retained as failure evidence and cannot be used for
  capability promotion or treatment comparison.
- [Sanitized V2 machine-readable result](CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.json)
- [Cindx Agent Real-World V1 0.1.82](CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md)
  remains the latest complete provider-backed Agent baseline on source commit
  `4d43e77`.
  It contains 72 matched runs across file, coding, browser, long-horizon,
  RAG/memory, and permission-safety tasks. Fast, Auto, and Pro each passed
  `72.2%`; Auto and Pro did not improve quality over Fast and regressed in
  completion and latency. The decision is `NO-GO` for an orchestration or
  frontier-intelligence uplift claim.
- [Sanitized V1 machine-readable result](CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.json)

The earlier `0.1.80` lite diagnostic remains historical evidence for its own
revision and is no longer the current product decision.

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
required report fields, and interpretation limits.
