# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| [13A calibration pilot](evaluations/CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md) | `0.1.94` | 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | Coding passed in all modes; all long-horizon effects verified but terminal completion failed on the shared external-grounding contract; `CALIBRATION_NO_GO` for matrix expansion |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md) | `0.1.82` | Same 72-cell product matrix with corrected read-evidence equivalence | Latest collection attempt; four infrastructure-failed RAG/memory cells make it `INVALID_BASELINE`; retained for diagnosis only |
| [Agent Real-World V1](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md) | `0.1.82` | 6 tool, state, memory, and safety tasks; Direct/Fast/Auto/Pro; 72 matched runs | Fast, Auto, and Pro each scored `72.2%`; Auto and Pro added no quality over Fast and had lower completion plus higher latency; `NO-GO` for orchestration uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| Deterministic quality gates | current source | Runtime, memory, queue/steer, permission, recovery, projection, performance contracts | Control-plane evidence only |

The `0.1.94` 13A pilot is the latest provider-backed diagnostic, but its six-run
subset is intentionally too small to become a baseline. It found that all three
product treatments produced the correct long-horizon external effect while the
shared external-grounding contract still forced terminal failure and repair
amplification. The decision is to stop before a full matrix and investigate
that common contract boundary.

The real-world V2 collection is the latest full-matrix attempt, but it is not a baseline:
four cells failed during memory setup, before product behavior could be
verified. The real-world V1 matrix therefore remains the latest complete
product decision baseline because it exercises shipping Agent, tool,
permission, RAG, and memory paths. Direct is a
no-tools answer ceiling with task evidence supplied inline, not an equal
product treatment. The GPQA run is older and measures difficult question
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

The latest V2 collection executed all 72 cells once and retained every result.
Four Auto/Pro RAG-memory cells ended in infrastructure failure, so the contract
marks the whole collection `INVALID_BASELINE`; no rerun was used to replace
those failures. Descriptively, Direct, Fast, Auto, and Pro passed `88.9%`,
`72.2%`, `77.8%`, and `66.7%` of quality checks, but those rates are not valid
for promotion or treatment comparison because the matrix is incomplete. The
collection also exposed one Fast safety-verifier failure: the protected file
was not changed, but the run never produced the required denied-permission
evidence. Browser runs again dominated tail latency and failure. These are
diagnostic findings, not an intelligence-uplift result.

The latest complete `0.1.82` V1 matrix contains 72 matched runs: six frozen cases, four treatments,
and three repeats. Fast, Auto, and Pro each passed `72.2%` of the complete
matrix. Relative to Fast, Auto added `0.0` percentage points of quality, lost
`11.1` points of completion, and added `14.3 s` paired median latency. Pro added
`0.0` points of quality, lost `5.6` points of completion, and added `18.0 s`.
Pro also consumed `1,765,259` tokens and recorded `1,296` recovery events,
without a quality advantage over Fast.

File mutation, long-horizon migration, RAG/memory recall, and denied mutation
all passed for every product treatment. Browser evidence was the dominant real
failure: Fast passed `0/3`, Auto `1/3`, and Pro `0/3`; the path produced two
600-second timeouts and three unresolved permission waits. Coding scores also
expose a frozen-suite instrumentation defect: seven runs passed the file and
test checks but were failed solely because `file.read_many` was not accepted as
equivalent read evidence. The current verifier now accepts that narrow semantic
equivalence without treating search or write as read evidence. The published
scores remain unchanged; only a new matched provider-backed collection can
measure the effect of the corrected verifier and runtime changes.

No run violated the denied-mutation safety check. That is positive evidence for
this exact permission path, not proof of complete runtime security. The matrix
rejects a current orchestration-uplift claim and gives the next goals concrete
targets: browser stop/permission convergence, evidence-aware tool completion,
and collaboration that must beat Fast under matched verification and latency.

## Current Product Gate

`benchmarks/agent/realworld-v2.json` freezes the current matched product
protocol. V2 changes only the coding verifier's read-evidence equivalence by
accepting `file.read_many`; objectives, fixtures, treatments, replicates, and
all other success conditions remain unchanged from V1. The published V1 result
remains the current measured baseline until a complete V2 matrix is collected.
The retained V2 attempt is explicitly invalid and does not replace it.
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

The latest invalid V2 attempt is recorded in
[Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md)
with its [sanitized machine-readable evidence](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.json).
The latest complete provider-backed matrix is recorded in
[Agent Real-World V1](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md)
with its [sanitized machine-readable evidence](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.json).
Cancellation, interruption/resume, user steering, computer interaction, and
adversarial instruction resistance still require later frozen suites; they
must not be inferred from this baseline.

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
  --raw /private/tmp/cindx-agent-realworld-v2.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V2.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V2.md

node scripts/run-agent-realworld.mjs --execute \
  --raw /private/tmp/cindx-agent-realworld-v2.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V2.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V2.md
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
