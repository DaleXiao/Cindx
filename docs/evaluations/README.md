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

- [Cindx Agent Real-World V5 0.2.22](CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.md)
  is the current 72-cell provider-backed baseline on source commit
  `a0000fa55907b90adf6684b39aa3b9d5cc679f42`. All cells were retained, with
  complete provider and strategy evidence, zero setup failures, zero timeouts,
  and zero safety violations, so baseline validity is `VALID_BASELINE`. Auto
  improved one matched quality outcome, preserved completion, had a
  matched-pair median latency delta of `-3,585 ms`, and used about `10.0%`
  fewer total tokens, so the frozen adaptive-direct claim is `IMPROVED`.
  Workflow, learned-profile, and
  distillation were not exercised. Pro remains descriptive because its native
  budget differs.
- [Sanitized V5 0.2.22 machine-readable result](CINDX_AGENT_REALWORLD_V5_0.2.22_2026-08-06.json)

- [Cindx Agent Real-World V5 0.2.11](CINDX_AGENT_REALWORLD_V5_0.2.11_2026-08-06.md)
  is the previous V5 provider-backed baseline on source commit
  `3765d23042dcafaeb721accc0460f149e1ea5ade`. It was a `VALID_BASELINE` with
  adaptive-direct `NEUTRAL`; workflow, learned-profile, and distillation were
  not exercised.
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

## Current Targeted Causal Evidence

- [Cindx Dynamic Collaboration V1 result 0.2.27](CINDX_DYNAMIC_COLLABORATION_V1_0.2.27_2026-08-09.md)
  records the completed eight-cell route-blind matrix under the
  [frozen protocol](CINDX_DYNAMIC_COLLABORATION_V1_PROTOCOL_0.2.27_2026-08-09.md).
  It is `VALID_TARGETED_EVIDENCE`, `NO_GO_NOT_EXERCISED`: both Grounded Direct
  controls and every Auto/Pro cell completed with full quality, full external
  effects, and zero safety violations, but all four Auto/Pro cells executed
  Direct. Auto used `1.6028x` the median latency and `1.1208x` the tokens of
  Grounded Direct; Pro produced no matched quality or completion win. No
  Workflow, GEPA, memory, general intelligence, or Fugu parity claim is admitted.
- [Sanitized Dynamic Collaboration V1 machine-readable result](CINDX_DYNAMIC_COLLABORATION_V1_0.2.27_2026-08-09.json)

- [Cindx Conductor ownership targeted evaluation 0.2.27](CINDX_CONDUCTOR_OWNERSHIP_V1_0.2.27_2026-08-09.md)
  records one frozen old/current train diagnosis and one independent
  control/candidate holdout for the Conductor-owned execution plan. It is
  `VALID_TARGETED_EVIDENCE`, `KEEP_HARNESS_FIX`: a complete batch read can now
  provide closed exact-readback evidence, closing the observed Auto terminal
  failure while all six candidate product cells preserve quality, completion,
  and safety. Every classifiable Auto/Pro run remained Direct, so Workflow,
  learned-profile, GEPA, distillation, and frontier uplift remain unproven.
- [Sanitized old-train result](CINDX_CONDUCTOR_OWNERSHIP_V1_OLD_TRAIN_0.2.27_2026-08-09.json)
- [Sanitized current-train result](CINDX_CONDUCTOR_OWNERSHIP_V1_CURRENT_TRAIN_0.2.27_2026-08-09.json)
- [Sanitized control-holdout result](CINDX_CONDUCTOR_OWNERSHIP_V1_CONTROL_HOLDOUT_0.2.27_2026-08-09.json)
- [Sanitized candidate-holdout result](CINDX_CONDUCTOR_OWNERSHIP_V1_CANDIDATE_HOLDOUT_0.2.27_2026-08-09.json)

- [Cindx Workflow GEPA V2 invalid attempt 0.2.25](CINDX_WORKFLOW_GEPA_V2_INVALID_0.2.25_2026-08-08.md)
  records the authorized provider attempt on source commit
  `40e07a3103ab98d040b7270404dbd6327a81dabe`. It is
  `INVALID_EVALUATOR`: V2 rejected a behaviorally valid implementation based on
  source spelling and exposed exact research answers in workspace fixtures. The
  run stopped before candidate generation and provides no uplift or regression
  evidence.

- [Cindx Workflow GEPA V3 invalid attempt 0.2.25](CINDX_WORKFLOW_GEPA_V3_INVALID_0.2.25_2026-08-08.md)
  records the authorized provider attempt on source commit
  `2b5f5351eba42da18d99d1cbd2fda74129b6ac0d`. It is
  `INVALID_TASK_SPEC`: the second task required a numeric port without
  disclosing that behavior, and the audit also found holdout selection and
  prompt-simulation confounds. V4 replaces that protocol.

- [Cindx Workflow GEPA V4 invalid attempt 0.2.25](CINDX_WORKFLOW_GEPA_V4_INVALID_0.2.25_2026-08-08.md)
  records the authorized provider attempt on source commit
  `f4553200c5eb7278a9e52b303e07b3b620fc1298`. It is `INVALID_EVALUATOR`:
  both training tasks completed and generated a candidate snapshot, but a
  path-bound revision comparison rejected separate isolated roots before any
  validation arm ran. The snapshot was not admitted or promoted, and the
  attempt provides no uplift or regression evidence.

- [Cindx Workflow GEPA V4 invalid task-spec attempt 0.2.25](CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.md)
  records the second authorized provider attempt on clean source commit
  `ee408db59112757ef356fefd25bda9eb3a42dc47`. It is `INVALID_TASK_SPEC`:
  the candidate reached matched validation and exercised its learned route
  receipts, but the failed research case hid lower-case identifier and
  exact-phrase requirements absent from the public contract. The raw execution
  receipts are diagnostic only and support no quality or regression conclusion.
  Untouched test and control stayed sealed and no production profile changed.
  The unmodified evaluator output is
  [retained here](CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.json).

- [Cindx Workflow GEPA V5 provider evaluation 0.2.25](CINDX_WORKFLOW_GEPA_V5_0.2.25_2026-08-08.md)
  records the first authorized V5 provider run on clean source commit
  `86f7dd61b0ee42d265b656425957b1a4550a4700`. It is
  `VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`: the train-selected learned
  profile causally changed routing and exercised Workflow, but both unseen
  validation cases tied the seed on quality while aggregate latency and tokens
  regressed. Control and untouched test stayed sealed, no snapshot was
  published, and no production profile changed. The sanitized report is
  [retained here](CINDX_WORKFLOW_GEPA_V5_0.2.25_2026-08-08.json).

- [Cindx Workflow GEPA V6 frozen protocol 0.2.25](CINDX_WORKFLOW_GEPA_V6_PROTOCOL_0.2.25_2026-08-08.md)
  records the successor protocol and its eight fresh train, validation, and
  untouched-test tasks. It is `PROTOCOL_ONLY`: deterministic contract tests pass,
  but no V6 provider run, snapshot, promotion, or capability result exists.

- [Cindx Workflow GEPA V7 frozen protocol 0.2.25](CINDX_WORKFLOW_GEPA_V7_PROTOCOL_0.2.25_2026-08-09.md)
  replaces prescribed route labels and fixed mutation directions with measured
  Conductor route choice and trajectory-grounded one-or-two-gene proposals. It
  also adds a durable external action journal and strict product/campaign caps.
  The protocol document remains the preregistered contract; the separate
  `0.2.26` provider report below records its completed train-only `NO-GO` result.

- [Cindx Workflow GEPA V7 interrupted campaign 0.2.25](CINDX_WORKFLOW_GEPA_V7_INTERRUPTED_0.2.25_2026-08-09.md)
  records the one authorized run on clean source `8be0fb1`. Both training seed
  runs completed, but candidate mutation stopped on `no_progress` before a
  population existed. It is `INTERRUPTED_CAMPAIGN` and supports no capability
  conclusion, snapshot, or production-profile change. The sanitized evidence is
  [retained here](CINDX_WORKFLOW_GEPA_V7_INTERRUPTED_0.2.25_2026-08-09.json).

- [Cindx Workflow GEPA V7 provider evaluation 0.2.26](CINDX_WORKFLOW_GEPA_V7_0.2.26_2026-08-09.md)
  records the separate authorized run on clean source `bc37fa9`. Candidate
  generation completed in three calls, and all three distinct candidates
  completed both matched training pairs with full quality and zero safety
  violations. All six pairs tied, all candidate runs remained Direct, and no
  candidate met the Pareto resource gate. The result is
  `VALID_TARGETED_EVIDENCE`, `NO_GO_TRAINING`; validation, test, Grounded Direct
  control, snapshot publication, and production promotion remained sealed. The
  sanitized report is
  [retained here](CINDX_WORKFLOW_GEPA_V7_0.2.26_2026-08-09.json).

- [Cindx Workflow GEPA V9 route-causal protocol 0.2.28](CINDX_WORKFLOW_GEPA_V9_ROUTE_CAUSAL_PROTOCOL_0.2.28_2026-08-09.md)
  freezes matched required-Direct versus required-Workflow training treatments,
  a positive-Workflow-signal stop gate, and route-only GEPA before any provider
  execution. It reuses the frozen V7 task splits and product gates. It is
  `PROTOCOL_ONLY`; no V9 provider result, snapshot, promotion, or uplift claim
  exists yet.

- [Cindx Direct-finalizer GEPA calibration 0.2.23](CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.md)
  is the provider-backed four-pair Gate A run on source commit
  `2647daafae98e99d31c101886d2836e446d83d96`. It is
  `VALID_TARGETED_EVIDENCE` and `NO_GO_FOR_PROMOTION`: the frozen Adversarial
  candidate regressed deterministic verification on one preservation case.
  GEPA, holdout, snapshot creation, production deployment, and transfer did not
  run, so this is a fail-closed calibration result rather than an intelligence
  uplift claim.
- [Sanitized Direct-finalizer machine-readable result](CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.json)

- [Cindx Memory-effect V2 0.2.19](CINDX_AGENT_MEMORY_EFFECT_V2_0.2.19_2026-08-06.md)
  is the current 18-cell provider-backed matched memory-on/off matrix on source
  commit `cd647030092971c84b7fc4dc6274285bec12b528`. All 9 pairs were
  evaluable: memory improved all 6 required-memory pairs, while all 3
  recalled-decoy controls passed in both arms. The scoped decision is
  `IMPROVED` for durable-memory utility under the frozen matched direct harness
  only; it does not replace V5 or establish Auto, GEPA, distillation, or
  frontier uplift.
- [Sanitized Memory-effect V2 machine-readable result](CINDX_AGENT_MEMORY_EFFECT_V2_0.2.19_2026-08-06.json)

- [Cindx Memory-effect V1 0.2.19](CINDX_AGENT_MEMORY_EFFECT_V1_0.2.19_2026-08-06.md)
  is an 18-cell provider-backed matched memory-on/off matrix on source commit
  `1abdd6849e4a99c7b30f7f7aa2123efbbdb0d51a`. One required memory-off cell
  invoked `skill.search` outside the frozen allowlist, so its fail-closed
  decision is `INVALID_EVIDENCE` with 8/9 pairs evaluable. The five evaluable
  required pairs favored memory-on and all three controls passed, but those
  descriptive outcomes do not establish causal memory utility. The run does
  not replace V5 or establish Auto, GEPA, distillation, or frontier uplift.
- [Sanitized Memory-effect V1 machine-readable result](CINDX_AGENT_MEMORY_EFFECT_V1_0.2.19_2026-08-06.json)

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
