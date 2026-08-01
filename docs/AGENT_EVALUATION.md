# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| [Agent Real-World Lite](evaluations/CINDX_AGENT_REALWORLD_LITE_0.1.80_2026-08-01.md) | `0.1.80` | 3 read-only workspace tasks, Direct/Fast/Auto/Pro, 12 first-pass runs plus 5 targeted reruns | `NO-GO` for Agent-intelligence uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| Deterministic quality gates | current source | Runtime, memory, queue/steer, permission, recovery, projection, performance contracts | Control-plane evidence only |

The real-world pilot is the current product decision baseline because it
exercises workspace tools and workflow composition. The GPQA run is the latest
matched provider diagnostic; it is older and measures difficult question
answering without tools.

Neither report used a promoted GEPA profile. Neither report establishes Fugu
Ultra parity.

## Evidence Levels

### 1. Contract tests

Rust, frontend, sidecar, and structure tests verify deterministic behavior such
as parsing, permission scope, cancellation, lifecycle reduction, persistence,
and graph invariants. They answer “does this contract hold?” They do not answer
“is this Agent better?”

### 2. Deterministic evaluation suites

Versioned manifests under `benchmarks/` validate routing contracts, memory
recall fixtures, evaluation schemas, resource invariants, and safety behavior.
Synthetic or fixture-based scores must remain labeled as contract results.

### 3. Provider reasoning diagnostics

The bounded GPQA runner compares configured treatments on a frozen matched
sample. It controls case identity and scoring, but model assignment, serving
conditions, timeouts, and small sample size still limit attribution. GPQA is
useful for detecting reasoning regressions; it is not the primary Agent gate.

### 4. End-to-end Agent evaluation

A product-quality claim requires tasks that exercise tools, evidence gathering,
state, recovery, and delivery. Treatments must receive equivalent task access,
budgets, deterministic verification, and repeated matched cases. Failures stay
in the denominator.

### 5. External parity evaluation

Fugu parity requires the complete frozen protocol, safety attestation, matched
budgets, and provider-backed observations described in
[FUGU_EVALUATION.md](FUGU_EVALUATION.md). A subset or a historical score cannot
be promoted to parity evidence.

## Required Report Fields

Every retained provider-backed report must include:

- application version and exact source commit;
- capture time and frozen case or dataset provenance;
- treatment configuration and budgets;
- completion, failure, quality, latency, token, tool, and safety outcomes;
- raw-evidence digest while keeping secrets and protected benchmark content out
  of Git;
- explicit confounds and claim boundary;
- a decision, including `NO-GO` when evidence is insufficient or negative.

Reports with an unknown source revision are not project evidence. Historical
reports are immutable and live under `evaluations/archive/`.

## Current Real-World Findings

The `0.1.80` pilot found two integration failures that deterministic component
tests and GPQA did not expose:

1. Fast spent its evidence budget before reading decisive files, then finalized
   with an incomplete answer.
2. Auto produced a correct downstream reviewer result but selected stale weaker
   text for terminal delivery.

It also found that Auto and Pro completed multi-file synthesis but used much
more time and tokens than the direct ceiling. These observations reject a broad
uplift claim; they do not prove that collaboration has no value.

## Next Product Gate

The next external-effect suite should use a tool-enabled single-agent baseline
and equal treatment budgets. It should include repeated matched cases for:

- exact and multi-file workspace retrieval;
- code edit plus tests;
- structured file mutation;
- browser and computer interaction;
- cancellation, interruption/resume, and user steering;
- cross-session memory production, recall, and actual use;
- long-horizon dependency recovery;
- permission denial and unsafe-instruction resistance.

Each case needs deterministic verification and at least three matched repeats
before a treatment-level conclusion. The suite should be frozen before tuning.

## Running Checks

Deterministic profiles are documented in [QUALITY_GATES.md](QUALITY_GATES.md).
The provider baseline runner is preflight-only unless `--execute` is supplied:

```sh
node scripts/run-provider-baseline.mjs
node scripts/run-provider-baseline.mjs --execute
```

The execution form is billable and requires the private benchmark source and
configured provider. It must write raw evidence outside Git and commit only a
sanitized report.

The offline Fugu matrix is generated by the non-shipping evaluation crate:

```sh
cargo run -p orchestrator-eval --example fugu_evaluation_lab --locked -- \
  --run-plan target/fugu-v1-run-plan.json \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md
```

Missing observations must remain `not_observed`; they are never replaced with
synthetic success.
