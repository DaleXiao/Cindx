# Cindx Workflow GEPA V11 route-causal protocol 0.2.28

## Status

`PROTOCOL_ONLY`

V11 freezes one causal question before provider execution: with the same Pro
conductor candidate, workflow proposal, route profile, task, workspace, model
set, and budget, does executing the proposal as a Workflow produce externally
verified value over projecting that same proposal onto Direct execution? GEPA
may mutate only route guidance, and only after a positive Workflow treatment.

There is no V11 provider result, snapshot, promotion, quality gain, efficiency
gain, or frontier claim in this protocol.

| Field | Frozen value |
| --- | --- |
| Application version | `0.2.28` |
| Evidence schema | `cindx.workflow-gepa-product-evidence.v11` |
| Suite ID | `cindx-workflow-gepa-v7` |
| Suite SHA-256 | `7f289521669a612f2577888c84c5b364b0cd076179f4776ca20b0bf7c97fcdab` |
| Cases | 8 frozen cases: 2 train, 2 validation, 4 untouched test |
| Learned field | `route_directive` only |
| Candidate population | 2 distinct route policies from at most 6 proposals |
| Maximum product runs | 33 planned, below the hard cap of 35 |

## Correction from V10

V10 failed closed because Direct and Workflow were separate conductor requests.
That mixed provider instruction-following and plan variation into the execution
treatment. V11 removes that confound:

1. The first arm in each pair generates one workflow-capable conductor plan;
   the second arm reuses that exact in-memory plan anchor and does not ask the
   conductor to plan again. Arm order remains counterbalanced, so the one
   planning cost is position-balanced across Direct and Workflow.
2. The Workflow arm executes that plan unchanged.
3. The Direct arm is projected by the runtime onto the single-agent path after
   planning; model, task class, tools, retrieval, memory, vision, risk, and
   estimated work remain inherited from the conductor candidate.
4. Each pair must contain identical conductor-candidate and workflow-proposal
   SHA-256 receipts. Any difference makes the pair invalid rather than a win,
   loss, or tie.
5. The persisted execution constraint and actual route must still agree, and
   both execution plans must identify `runtime_constraint` authority.
6. The private plan anchor is excluded from persisted event context and public
   evidence; only its exact candidate/proposal identities enter the sanitized
   pair receipt.

Production Fast, Auto, and Pro do not install an evaluation constraint, so this
change does not alter their routing or budgets.

## Frozen campaign

1. Execute the two training cases as matched Direct and Workflow pairs: four
   product runs.
2. Stop with `valid_no_go_no_positive_workflow_treatment` unless Workflow wins
   at least one externally verified quality comparison.
3. If a positive treatment exists, let GEPA propose two route-only policies.
   A proposal must change only `route_directive`, preserve the workflow profile,
   avoid case/model/provider leakage, and have a distinct route fingerprint.
4. Select candidates on the two training cases only: eight product runs.
5. Admit the selected policy on two unseen validation cases: four product runs.
6. Only after validation, run one Grounded Direct control and four untouched
   test cases with two counterbalanced repeats: seventeen product runs.

Maximum complete usage is `4 + 8 + 4 + 1 + 16 = 33` product runs. Existing
V7 limits for duration, model calls, tool calls, tokens, physical attempts, and
mutation calls remain unchanged.

## Pre-registered gates

- Training requires complete external checks, zero safety violations, no
  losses, exact matched-plan receipts, and either a quality win or a Pareto-safe
  5% resource gain.
- Validation requires complete quality, no losses, actual Workflow execution,
  exact causal receipts, aggregate latency and token ratios at most `1.25`, and
  either a quality win or a Pareto-safe 10% resource gain.
- Untouched test requires wins on at least two distinct cases, no losses,
  positive behavior delta, no completion or safety regression, exact receipts,
  and aggregate latency and token ratios at most `1.05`.
- The Grounded Direct control must pass before test evidence can support a
  candidate.

Only a complete pass may publish an external candidate snapshot. The campaign
does not install or promote it into the shipping profile.

## Interpretation

The clean frozen revision may be executed once. Failed, timed-out, rejected, or
interrupted cells remain in the external hash-chained journal. There is no
result-driven prompt edit, case replacement, threshold change, or rerun.

- `valid_no_go_no_positive_workflow_treatment` means the training pairs supplied
  no demonstrated Workflow advantage from which GEPA could learn.
- Later `valid_no_go_*` statuses identify the first pre-registered gate that
  failed.
- `valid_candidate_product_uplift` means only that this frozen campaign passed;
  it is not production promotion, broad superiority, or Fugu Ultra parity.
- Missing identity, shared-plan, treatment, journal, or provider evidence is
  invalid evidence, not a no-go.

The result must be retained and reported regardless of outcome.
