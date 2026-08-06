# Evaluation Evidence

This directory contains current decision evidence. Reports describe only the
source revision and application version recorded inside them.

## Current Execution Contract

Agent Real-World V5 is the current 72-cell execution contract. It preserves the
frozen cases and typed evidence boundaries while separating the no-tools Oracle
Reference from an iso-budget Grounded Direct product baseline. Auto's actual
adaptive-direct and workflow subsets are evaluated separately; learned-profile
and distillation claims require the executed exact stable parent. Its
deterministic contract test does not contact a provider and is not intelligence
evidence by itself.

## Current Product Decision

- [Cindx Agent Real-World V5 0.2.11](CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.md)
  is the current 72-cell provider-backed baseline on source commit
  `3765d23042dcafaeb721accc0460f149e1ea5ade`. All cells were retained, with
  complete provider and strategy evidence, zero setup failures, zero timeouts,
  and zero safety violations, so baseline validity is `VALID_BASELINE`. Auto
  preserved Grounded Direct quality and completion, reduced median latency by
  `5,989 ms`, and used `1.8%` more total tokens. With no matched quality
  improvement, adaptive-direct is `NEUTRAL`. Workflow, learned-profile, and
  distillation were not exercised. Pro's higher completion is descriptive
  because its native budget differs.
- [Sanitized V5 0.2.11 machine-readable result](CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.json)

- [Cindx Agent Real-World V4 0.2.9](CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.md)
  is the previous 72-cell provider-backed baseline on source commit
  `7905405551f3decd38746c45218790cb9a04be37`. All cells were retained, with
  zero setup failures and zero safety violations, so baseline validity is
  `VALID_BASELINE`. Auto and Pro preserved quality and completion against Fast,
  improved three quality runs, and completed one and three additional runs,
  respectively, within their resource ceilings. The collector nevertheless
  classified continuation tool events in two Fast runs as outside the current
  logical Agent run, so provider-evidence completeness fails closed and broad
  orchestration uplift remains `NO-GO`.
  No frozen learned artifact was supplied; the learned-profile status is
  `FRESH-SEED-ONLY` and the result cannot be attributed to GEPA, transfer, or
  self-distillation.
- [Sanitized V4 0.2.9 machine-readable result](CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.json)

- [Cindx Agent Real-World V3 0.2.3](CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.md)
  is the previous 72-cell provider-backed baseline on source commit
  `af5025f46137096c34bd5dd4f70a89657713fde4`. The cyclic Latin-square matrix
  retained every failure, with zero setup failures and zero safety violations,
  so scientific validity is `VALID_BASELINE`. Auto and Pro each gained two
  quality-pass runs over Fast but lost two completed runs; five browser
  timeouts also failed the preregistered receipt gate. Broad orchestration
  uplift is `NO-GO`, and the absence of a frozen learned artifact makes the
  learned-profile status `FRESH-SEED-ONLY`.
- [Sanitized V3 0.2.3 machine-readable result](CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.json)

- [Cindx Agent Real-World V2 0.1.98](CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md)
  is the previous 72-cell provider-backed baseline on source commit
  `4fc736cdfd0b8eb85ffee0a9ef5dfaea4b05e469`.
  All cells were retained, with zero setup failures and zero safety violations,
  so the scientific-validity decision is `VALID_BASELINE`. Auto and Pro gained
  quality over Fast only with materially lower completion and higher latency;
  the separate broad orchestration-uplift decision is `NO-GO`.
- [Sanitized V2 0.1.98 machine-readable result](CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.json)

- [Cindx Agent Real-World 13A Repair Calibration 0.1.95](CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md)
  is the exact six-run provider-backed rerun on source commit `487f3e0`.
  All six runs completed with all checks passing and no safety violation; the
  three long-horizon runs no longer created a false external-grounding
  obligation. Its decision is `CALIBRATION_GO` for frozen matrix expansion,
  not an intelligence-uplift claim or a replacement baseline.

- [Cindx Agent Real-World 13A Calibration Pilot 0.1.94](CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md)
  is a six-run provider-backed calibration subset on source commit `f3e46fa`.
  All treatments produced the required long-horizon external effect, but all
  failed terminal completion because the external-grounding contract remained
  unsatisfied. Its decision is `CALIBRATION_NO_GO`; it is not a baseline and
  does not replace the complete V1 matrix.

- [Cindx Agent Real-World V2 0.1.82](CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md)
  is a prior provider-backed collection attempt on source commit `6b2ee39`.
  Four RAG/memory cells ended in infrastructure failure, so its decision is
  `INVALID_BASELINE`. It is retained as failure evidence and cannot be used for
  capability promotion or treatment comparison.
- [Sanitized V2 machine-readable result](CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.json)
- [Cindx Agent Real-World V1 0.1.82](CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md)
  is the prior complete provider-backed Agent baseline on source commit
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
