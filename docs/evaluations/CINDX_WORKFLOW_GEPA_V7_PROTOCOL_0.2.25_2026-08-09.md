# Cindx Workflow GEPA V7 frozen protocol 0.2.25

## Status

`PROTOCOL_ONLY`

This document freezes a replacement for Workflow GEPA V6. No V7 provider call
has run. There is no V7 candidate result, snapshot, promotion, quality gain,
efficiency gain, or frontier claim.

| Field | Frozen value |
| --- | --- |
| Application version | `0.2.25` |
| Campaign schema | `cindx.workflow-gepa-campaign.v7` |
| Suite ID | `cindx-workflow-gepa-v7` |
| Suite file | `benchmarks/agent/workflow-gepa-v7.json` |
| Suite SHA-256 | `7f289521669a612f2577888c84c5b364b0cd076179f4776ca20b0bf7c97fcdab` |
| Cases | 8 frozen cases: 2 train, 2 validation, 4 untouched test |
| Task classes | 4 coding and 4 research cases |
| Provider calls in this protocol change | 0 |

An authorized run must use a clean exact Git revision and new external report,
snapshot, and journal paths. A dirty tree, abbreviated commit, changed suite,
inside-repository path, reused journal, or interrupted journal fails closed.

## Why V7 replaces V6

V6 publicly told the Agent which cases were Direct and which required Workflow.
Its gates then rewarded obeying that declared route. That design could measure
route compliance, but it could not measure whether the Conductor selected a
useful route from the task and observed evidence.

V6 also used four fixed human-written mutation directions and had no durable
campaign-wide reservation before each provider action. A product run retained
the shipping Pro upper budget, so a failed campaign could consume excessive
provider work before producing evidence.

V7 changes those experiment boundaries without changing shipping Fast, Auto,
or Pro defaults:

1. The public task never names Direct, Workflow, collaboration, or a required
   model count. The Conductor chooses the execution mode.
2. Route changes are descriptive causal receipts, not quality credit.
3. Candidate search asks the mutation model to rank bottlenecks supported by
   redacted trajectories. Each proposal must change exactly one or two actual
   causal genes after normalization; there is no fixed search-direction list.
4. A private external hash-chained journal reserves every provider action
   before execution. Existing or interrupted journals cannot be replayed.
5. Each product run has a dedicated evaluation cap of 10 minutes, 20 logical
   model calls, 48 tool calls, and 80 physical attempts. Shipping budgets are
   unchanged.
6. The complete campaign is capped at two hours, 35 product runs, 700 product
   logical model calls, and 12 mutation logical calls.

## Frozen experiment

The eight public behavior contracts and host-owned deterministic postconditions
remain the V6 task content, but all prescribed-route fields and route wording
are removed. The suite validator rejects route leakage. Expected research
answers, raw provider outputs, prompts, secrets, and protected host inputs stay
outside Git.

| Split | Cases | Use |
| --- | --- | --- |
| Train | 1 coding, 1 research | Seed reflection, observation-driven population search, matched selection |
| Validation | 1 coding, 1 research | Admission only; cannot alter or select the candidate |
| Test | 2 coding, 2 research | Untouched final evidence with counterbalanced repeats |

Seed and candidate use separately materialized workspaces with equal content
fingerprints, candidate-specific project scopes, and alternating arm order.
Semantic-memory model extraction and cloud embedding are disabled only inside
the isolated campaign; deterministic local projection remains.

## Fail-closed gates

### Candidate generation and train

At most six provider proposals may produce three distinct route phenotypes.
Every retained proposal must change one or two measured genes and bind its
profile, route phenotype, parent, generation, response hash, and snapshot hash.

A train candidate is eligible only when both matched candidate cells complete,
pass every external quality and safety check, have no quality loss, and carry
the exact candidate-profile and route-semantics receipts. It must then show:

- at least one external quality win with aggregate latency and token ratios at
  most `1.25`; or
- at least a 5% latency improvement with token ratio at most `1.05`, or at
  least a 5% token improvement with latency ratio at most `1.05`.

The train-only Pareto archive ranks verified quality before resources. Merely
switching route, using more models, or exercising a Task Graph earns no credit.

### Validation

Both unseen candidate cells must complete with full quality, zero losses, zero
safety violations, and exact causal receipts. Latency and token ratios must
each be at most `1.25`. The candidate must additionally show a quality win or a
Pareto-safe 10% improvement in one resource while the other remains at most
`1.05`.

Failure yields a valid validation no-go. Control, untouched test, and snapshot
publication remain sealed.

### Untouched test and control

The eight counterbalanced test pairs must contain quality wins on at least two
distinct cases, no losses, positive aggregate external behavior delta, no
completion or safety regression, exact causal receipts on every candidate run,
and aggregate latency and token ratios each at most `1.05`. A separate Grounded
Direct product control must complete, pass full external quality, remain safe,
and prove Direct execution.

Only after every gate passes may the campaign publish an external candidate
snapshot. The campaign never installs or promotes that snapshot into the
shipping profile.

## Offline verification at freeze time

The following non-provider checks passed:

- focused Workflow GEPA tests: `16` passed;
- custom persisted-budget receipt test: `1` passed;
- suite SHA-256 matched the frozen value above;
- Rust formatting and feature-gated compilation passed.

These checks verify experiment isolation, dynamic-route measurement, bounded
provider work, causal identity, and fail-closed accounting only. They do not
establish that V7 improves Cindx Pro.

## Decision

No version bump, application build, profile promotion, or capability claim
follows from this protocol alone. Exactly one authorized provider campaign may
run from the frozen clean revision. Its result must be retained whether it is
go, no-go, invalid, interrupted, or more expensive than the seed.
