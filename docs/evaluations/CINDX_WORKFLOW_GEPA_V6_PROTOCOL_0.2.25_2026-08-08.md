# Cindx Workflow GEPA V6 frozen protocol 0.2.25

## Status

`PROTOCOL_ONLY`

This document freezes the successor to the first Workflow GEPA V5 provider
campaign. No V6 provider call has run. There is no V6 candidate result,
snapshot, promotion, quality gain, efficiency gain, or frontier claim.

| Field | Frozen value |
| --- | --- |
| Application version | `0.2.25` |
| Campaign schema | `cindx.workflow-gepa-campaign.v6` |
| Suite ID | `cindx-workflow-gepa-v6` |
| Suite file | `benchmarks/agent/workflow-gepa-v6.json` |
| Suite SHA-256 | `55a0cd7b25a9db65a4c2181448823eb170b3125068b52126ea65be02d70fef01` |
| Cases | 8 new cases: 2 train, 2 validation, 4 untouched test |
| Route strata | 4 public Direct and 4 public Workflow contracts |
| Task classes | 4 coding and 4 research cases |
| Provider calls in this protocol change | 0 |

An authorized run must use a clean exact Git revision supplied through the
campaign provenance contract. A dirty tree, abbreviated commit, changed suite,
inside-repository report path, or inside-repository snapshot path fails before
provider work.

## Why V6 exists

The first V5 run is valid targeted evidence and a validation no-go. Its selected
candidate causally changed a training route and exercised the Task Graph, but it
did not improve either unseen validation case. It used `1.4337x` seed latency
and `1.2966x` seed tokens there. V5 also allowed a public route-contract
improvement to satisfy the generic train-side gain predicate even when product
quality did not improve.

V6 changes the admission contract rather than reinterpreting V5:

1. Route contrast remains mandatory causal evidence but is not product gain.
2. Every V5 task is replaced. Observed V5 validation outcomes cannot select a
   V6 candidate.
3. External postconditions retain partial behavior scores for diagnosis, while
   every admitted candidate cell must still pass full quality and safety.
4. Train and validation add explicit resource ceilings.
5. The production execution contract narrows the outer workflow budget to a
   task-specific estimate while preserving the structural minimum needed for
   required contributions, verification, and synthesis.

## Frozen experiment

The Agent receives the complete behavior contract and whether the case requires
Direct or Workflow execution. Hidden host inputs test behaviorally equivalent
implementations rather than prescribed source spelling. Every case has four
independent external command postconditions; research cases also validate their
exact structured result. Expected research answers are not copied into public
checks.

| Split | Cases | Use |
| --- | --- | --- |
| Train | 1 coding Direct, 1 research Workflow | Seed reflection, candidate generation, matched candidate selection |
| Validation | 1 coding Direct, 1 research Workflow | Admission only; cannot alter or select the candidate |
| Test | 2 coding and 2 research, balanced Direct/Workflow | Untouched final evidence, two counterbalanced repeats per case |

Seed and candidate use separately materialized workspaces with equal content
fingerprints, candidate-specific project scopes, and alternating arm order.
Semantic-memory model extraction and cloud embedding are disabled only within
the isolated campaign; deterministic local projection remains. Raw prompts,
provider outputs, secrets, and protected host inputs stay outside Git.

## Fail-closed gates

### Train

A candidate is eligible only when both train cells:

- complete and pass every quality and safety check;
- carry the exact candidate profile and route-semantics receipts;
- pass both public route contracts;
- exercise the Workflow profile at least once; and
- have no quality loss.

It must then show one of these measured gains:

- at least one externally verified quality win with both aggregate latency and
  token ratios at most `1.25`; or
- at least a 5% latency improvement with token ratio at most `1.05`, or at least
  a 5% token improvement with latency ratio at most `1.05`.

The train-only Pareto archive ranks verified quality before resources. Validation
and test observations cannot participate in selection.

### Validation

Both unseen cells must complete with full quality, zero losses, zero safety
violations, exact profile and route receipts, both public route contracts, and
actual Workflow execution. Latency and token ratios must each be at most `1.25`.
The candidate must additionally show either a quality win or a Pareto-safe 10%
improvement in one resource while the other ratio remains at most `1.05`.

Failure yields `valid_no_go_validation`; control, test, and snapshot publication
remain sealed.

### Untouched test and control

The eight counterbalanced test pairs must contain quality wins on at least two
distinct cases, no losses, positive aggregate external behavior delta, no
completion or safety regression, exact causal receipts, satisfied public route
contracts, actual Workflow execution, and latency and token ratios each at most
`1.05`. A separate Grounded Direct product control must also complete, pass full
external quality, remain safe, and prove Direct execution.

Only after every gate passes may the campaign publish an external candidate
snapshot. The campaign does not install or promote that snapshot into the
shipping profile.

## Offline verification at freeze time

The following non-provider checks passed:

- `cargo test -p orchestrator`: `302` passed;
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features realworld-eval workflow_gepa --lib`: `11` passed;
- suite JSON parsed successfully and matched the frozen SHA-256 above.

These results verify structure, isolation, evidence accounting, and fail-closed
gates only. They do not establish that V6 improves Cindx Pro.

## Decision

No release, version bump, application build, profile promotion, or capability
claim follows from this protocol alone. A V6 provider run requires separate
explicit authorization. Its result must be retained whether it is go, no-go,
invalid, interrupted, or more expensive than the seed.
