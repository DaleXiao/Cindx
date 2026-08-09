# Cindx Workflow GEPA V10 route-causal protocol 0.2.28

## Status

`PROTOCOL_ONLY`

This protocol freezes one causal question before any V10 provider execution: when
the same Pro conductor, prompt profile, task, workspace, model set, and budget
are held fixed, does a required Workflow produce externally verified product
value over a required Direct execution? GEPA may learn a routing instruction
only if that contrast contains a positive Workflow treatment.

There is no V10 provider result, candidate snapshot, profile promotion, quality
gain, efficiency gain, or frontier claim in this document.

| Field | Frozen value |
| --- | --- |
| Application version | `0.2.28` |
| Evidence schema | `cindx.workflow-gepa-product-evidence.v10` |
| Suite ID | `cindx-workflow-gepa-v7` |
| Suite file | `benchmarks/agent/workflow-gepa-v7.json` |
| Suite SHA-256 | `7f289521669a612f2577888c84c5b364b0cd076179f4776ca20b0bf7c97fcdab` |
| Cases | 8 frozen cases: 2 train, 2 validation, 4 untouched test |
| Learned field | `route_directive` only |
| Candidate population | 2 distinct route policies from at most 6 proposals |
| Maximum product runs | 33 planned, below the existing hard cap of 35 |
| Provider calls in this protocol change | 0 |

## Instrumentation correction from V9

V9 was consumed as `INVALID_INSTRUMENTATION` before one complete product
receipt existed. Its receipt projector rejected the deliberately injected
matched execution constraint because only the treatment label reached that
layer. V10 passes the expected constraint explicitly and requires the expected
constraint, persisted metadata, and actual Direct or Workflow route to agree.
Ordinary Pro still requires `native`; the fix does not relax production or
campaign evidence checks.

No V9 output or partial quality result informed this correction. The frozen
suite, case order, thresholds, budgets, candidate count, learned field, and
one-shot rule below are unchanged.

An authorized campaign must use one clean exact Git revision and three new
outside-repository paths for its report, candidate snapshot, and hash-chained
journal. A dirty tree, changed suite, reused path, incomplete journal, missing
receipt, or treatment drift fails closed.

## Separated causal boundaries

V8 correctly detected that the old genome coupled route choice and executable
workflow construction, but it could not learn from Direct-only seed runs. V10
separates the two interventions instead of adding another routing heuristic:

1. `route_directive` is the only route-policy gene. New profiles can change it
   without changing graph depth, topology, roles, tools, retries, verification,
   budgets, or finalization. Existing snapshots without this field retain their
   legacy behavior.
2. Workflow-execution genes no longer alter the route-policy fingerprint.
3. The evaluation runtime can require Direct or Workflow, but production leaves
   that field unset. The conductor still constructs the best bounded plan for
   the required arm, and a response with the wrong execution mode is rejected.
4. Each train pair starts from byte-identical materialized workspaces. Direct
   and Workflow share the same parent prompt and route fingerprints, Pro budget,
   provider configuration, task, and replicate; arm order is counterbalanced.
5. GEPA receives only complete same-run `forced_direct` and `forced_workflow`
   trajectories. Missing, duplicated, unrelated, or profile-drifted arms are
   rejected before mutation.

These constraints identify the execution-mode treatment. They do not by
themselves prove that Workflow is useful.

## Frozen campaign order

1. Run the two training cases as matched required-Direct and required-Workflow
   pairs: 4 product runs total.
2. Stop immediately with `valid_no_go_no_positive_workflow_treatment` unless at
   least one required-Workflow arm records an external quality win over its
   matched required-Direct arm. A tie, route change, additional models, or extra
   Task Graph activity is not a positive signal.
3. If a positive treatment exists, generate two route-only policies from the
   four matched trajectories. Every proposal must change exactly
   `route_directive`, preserve the workflow profile, avoid case/model/provider
   leakage, and have a distinct route fingerprint.
4. Evaluate each candidate against the seed on both training cases using native
   conductor decisions: 8 product runs. Select from training evidence only.
5. Open the two unseen validation cases only for the selected candidate: 4
   product runs. Validation cannot alter or reselect the policy.
6. If validation passes, run one Grounded Direct control and then open the four
   untouched test cases with two counterbalanced repeats: 17 product runs.

The maximum complete campaign therefore uses `4 + 8 + 4 + 1 + 16 = 33`
product runs. Product and campaign time, model-call, tool-call, token, physical
attempt, and mutation-call limits remain the existing V7 hard limits. Shipping
Fast, Auto, and Pro budgets are unchanged.

## Pre-registered gates

The train, validation, untouched-test, safety, causal-receipt, Grounded Direct,
and resource gates remain those frozen by V7. In particular:

- train requires complete external quality and safety, no losses, exact route
  receipts, plus a bounded quality win or a Pareto-safe 5% resource gain;
- validation requires complete quality, no losses, exact causal receipts,
  actual Workflow execution, both aggregate resource ratios at most `1.25`, and
  a quality win or Pareto-safe 10% resource gain;
- untouched test requires wins on at least two distinct cases, no losses,
  positive aggregate external behavior delta, no completion or safety
  regression, exact receipts, and aggregate latency and token ratios at most
  `1.05`;
- the Grounded Direct product control must pass before untouched-test evidence
  can support a candidate.

Only a complete pass may publish the external candidate snapshot. The campaign
does not install or promote that snapshot into the shipping profile.

## Interpretation and one-shot rule

The frozen clean revision may be executed once. Every completed, failed,
timed-out, rejected, or interrupted cell remains in its journal; there is no
result-driven prompt edit, case replacement, threshold change, or rerun.

- `valid_no_go_no_positive_workflow_treatment` means the frozen training tasks
  supplied no demonstrated Workflow advantage from which a route policy could
  learn. GEPA must not manufacture a collaboration preference from ties.
- `valid_no_go_training`, `valid_no_go_validation`,
  `valid_no_go_grounded_control`, and `valid_no_go_product_test` identify the
  first pre-registered gate that failed.
- `valid_candidate_product_uplift` means only that the frozen V10 campaign
  passed. It is not production promotion, broad Agent superiority, or Fugu
  Ultra parity.
- setup, identity, treatment, journal, or evidence inconsistency is invalid
  evidence rather than a no-go.

The result must be retained and reported regardless of outcome.
