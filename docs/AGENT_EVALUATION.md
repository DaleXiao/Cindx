# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md) | `0.1.98` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 provider-backed runs | `VALID_BASELINE` with zero setup failures and zero safety violations; Auto/Pro trade quality gains for materially lower completion and higher latency, so broad orchestration uplift remains `NO-GO` |
| [13A repair calibration](evaluations/CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md) | `0.1.95` | Exact repair rerun: 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | All 6 completed with 7/7 checks and no safety violation; false external grounding is closed on the frozen case; `CALIBRATION_GO` for matrix expansion only |
| [13A calibration pilot](evaluations/CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md) | `0.1.94` | 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | Coding passed in all modes; all long-horizon effects verified but terminal completion failed on the shared external-grounding contract; `CALIBRATION_NO_GO` for matrix expansion |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md) | `0.1.82` | Prior 72-cell V2 attempt | Four infrastructure-failed RAG/memory cells make it `INVALID_BASELINE`; retained for diagnosis only |
| [Agent Real-World V1](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md) | `0.1.82` | 6 tool, state, memory, and safety tasks; Direct/Fast/Auto/Pro; 72 matched runs | Fast, Auto, and Pro each scored `72.2%`; Auto and Pro added no quality over Fast and had lower completion plus higher latency; `NO-GO` for orchestration uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| Deterministic quality gates | current source | Runtime, memory, queue/steer, permission, recovery, projection, performance contracts | Control-plane evidence only |

The `0.1.98` V2 matrix is the latest complete provider-backed product baseline.
All 72 cells were retained, with no setup failure and no safety violation. Its
`VALID_BASELINE` status means the evidence is structurally usable; it is not a
capability `GO`. Relative to Fast, Auto gained `11.1` quality points while
losing `22.2` completion points and adding `24.663 s` paired median latency.
Pro gained `5.6` quality points while losing `27.8` completion points and
adding `41.396 s`. Broad orchestration uplift therefore remains `NO-GO`.

The `0.1.95` 13A repair calibration remains the before-matrix confirmation
that false external grounding was closed on its exact six-run subset. The
older invalid V2 attempt and complete V1 matrix remain historical evidence for
their source revisions. Direct is a no-tools answer reference with fixture
evidence supplied inline, not an equal product treatment. The GPQA run is an
older difficult-question diagnostic without tools.

The V2 raw schema did not capture learned profile or GEPA identities. The
result therefore cannot be attributed to GEPA, transfer, self-distillation, or
a particular promoted profile, and it does not establish Fugu Ultra parity.

The current source contains an Auto-to-Pro transfer gate. Its deterministic
tests establish evidence qualification, lineage isolation,
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
- a scientific-validity decision and a separate product-uplift decision,
  including `NO-GO` when evidence is insufficient or negative.

Reports with an unknown source revision are not project evidence. Historical
reports are immutable and live under `evaluations/archive/`.

## Current Real-World Findings

The `0.1.98` V2 baseline contains 72 matched observations. Direct, Fast, Auto,
and Pro passed `100.0%`, `77.8%`, `88.9%`, and `83.3%` of quality checks.
Completion was `100.0%`, `88.9%`, `66.7%`, and `61.1%`, respectively. The
matrix retained 57 completed runs, 10 failed runs, four timeouts, and one run
waiting for permission. There were no setup failures and no safety violations.

The quality signal is localized rather than broad. Fast failed the RAG/memory
answer check in all three repeats after setup and execution succeeded, while
Auto and Pro passed all three. Browser evidence moved in the opposite
direction: Fast passed `2/3`, Auto `1/3`, and Pro `0/3`; all three Pro browser
runs reached the frozen 600-second process deadline. Auto and Pro also verified
the denied-mutation result safely in all repeats but failed to converge on a
successful terminal runtime state after denial. Those are product behavior
failures, not evaluation infrastructure failures.

Relative to Fast, neither adaptive treatment improves quality, completion, and
latency together. The effective 600-second process deadline is matched, but the
three shipping modes retain different native budgets, so this is not an
iso-budget causal comparison. Timed-out processes can also under-report token,
call, and resource totals. The valid scientific conclusion is therefore a
descriptive baseline; the product-uplift decision remains `NO-GO`.

No run violated the denied-mutation safety check. That is positive evidence for
this exact permission path, not proof of complete runtime security. The next
Agent goals have concrete measured targets: value-aware routing must preserve
the RAG gain without paying browser and completion regressions, permission
denial must converge to a truthful terminal state, and learned profiles must be
identified in future provider-backed evidence before any GEPA claim.

## Current Product Gate

`benchmarks/agent/realworld-v2.json` freezes the current matched product
protocol. V2 changes only the coding verifier's read-evidence equivalence by
accepting `file.read_many`; objectives, fixtures, treatments, replicates, and
all other success conditions remain unchanged from V1. The `0.1.98` collection
is the current complete V2 baseline; the earlier `0.1.82` V2 attempt remains
invalid historical evidence and does not affect the current decision.
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

The current provider-backed matrix is recorded in
[Agent Real-World V2 0.1.98](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md)
with its [sanitized machine-readable evidence](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.json).
The invalid `0.1.82` V2 attempt and complete V1 baseline remain linked in the
[evaluation index](evaluations/README.md) as immutable historical evidence.
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
