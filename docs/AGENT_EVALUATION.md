# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| [Agent Real-World Lite Goal 6](evaluations/CINDX_AGENT_REALWORLD_LITE_G6_0.1.80_2026-08-01.md) | `0.1.80` | 3 read-only workspace tasks, direct ceiling/Fast/Auto/Pro, 12 matched first-pass runs | Auto matched the direct ceiling but Fast and Pro regressed; `NO-GO` for collaboration uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| Deterministic quality gates | current source | Runtime, memory, queue/steer, permission, recovery, projection, performance contracts | Control-plane evidence only |

The real-world pilot is the current product decision baseline because it
exercises workspace tools and workflow composition. The GPQA run is the latest
matched provider diagnostic; it is older and measures difficult question
answering without tools.

Neither report used a promoted GEPA profile. Neither report establishes Fugu
Ultra parity.

The current source adds an Auto-to-Pro transfer gate after those reports. Its
deterministic tests establish evidence qualification, lineage isolation,
position-balanced comparison, dual-gate promotion, frozen provenance, and
reserved mutation input. Those are mechanism checks, not answer-quality
evidence. A future provider-backed treatment must compare the pre-transfer Pro
profile with a promoted transfer-trained Pro profile on frozen matched cases
before any uplift claim is allowed.

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

The latest `0.1.80` matrix was captured after the run-driver, result-selection,
task-graph, context, retrieval, memory, GEPA-boundary, and build-boundary work in
Goals 1-5. Auto answered all three cases completely, including the prior stale
terminal-selection case. That is a positive integration signal, but this single
replicate cannot establish causality or generalize beyond the three fixtures.

Fast remained incomplete on multi-file synthesis and contradiction resolution.
Pro omitted required evidence in all three cases and consumed `73.43x` the
direct tokens at `12.83x` its median latency. Auto consumed `55.76x` the direct
tokens at `7.83x` its median latency. No provider error or safety violation
occurred, so the quality failures are product behavior rather than missing
deliveries. Neither collaborative mode had a frozen GEPA profile.

These observations reject a broad collaboration-uplift claim. They do not prove
that collaboration has no value, but they require dynamic orchestration to beat
an equal tool-using baseline before a larger parity claim is credible.

## Next Product Gate

`benchmarks/agent/realworld-v1.json` freezes the next matched product baseline.
It covers structured file mutation, code edit plus tests, browser evidence,
long-horizon migration, indexed knowledge plus cross-session memory, and denied
mutation. Direct, Fast, Auto, and Pro each receive three matched repeats.

The feature-gated `cindx-agent-realworld-eval` binary drives the shipping Agent,
tool, permission, RAG, and memory paths. Direct is explicitly marked as a
no-tools model reference rather than product-mechanism evidence. The independent
contract rejects missing or duplicate cells, suite or output hash drift,
unmatched inputs, and unbound Git provenance. Raw outputs remain outside Git.

The runner builds the driver once, then isolates every case, treatment, and
replicate in its own process. Each sample writes a private pending checkpoint
before provider execution. The frozen suite applies a 600-second process
deadline; a timeout is retained as a failed sample, never retried into a pass.
Completed samples are merged in matrix order and reused after interruption only
when Git commit, suite hash, selection, and replicate count still match. Browser
processes are retired by the sample's unique temporary root so one treatment
cannot contaminate the resources or state of the next.

This protocol is not evidence until a complete provider-backed matrix has run
and its sanitized report is committed. Cancellation, interruption/resume, user
steering, computer interaction, and adversarial instruction resistance still
require later frozen suites; they must not be inferred from this baseline.

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

The real-world Agent runner follows the same explicit opt-in boundary:

```sh
node scripts/run-agent-realworld.mjs \
  --raw /private/tmp/cindx-agent-realworld-v1.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V1.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V1.md

node scripts/run-agent-realworld.mjs --execute \
  --raw /private/tmp/cindx-agent-realworld-v1.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V1.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V1.md
```

Without `--execute`, it performs only Git, provider, suite, and output-boundary
preflight. A subset may be used for private calibration, but the publication
contract accepts only the complete frozen matrix. Re-running the same command
after interruption resumes validated completed cells from the private raw
checkpoint; timed-out cells remain failures.

The offline Fugu matrix is generated by the non-shipping evaluation crate:

```sh
cargo run -p orchestrator-eval --example fugu_evaluation_lab --locked -- \
  --run-plan target/fugu-v1-run-plan.json \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md
```

Missing observations must remain `not_observed`; they are never replaced with
synthetic success.
