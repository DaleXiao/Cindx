# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| [Agent Real-World V3](evaluations/CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.md) | `0.2.3` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 position-balanced provider-backed runs with strategy and provider receipts | `VALID_BASELINE`, zero setup failures, and zero safety violations; receipt evidence fails closed on five browser timeouts, Auto/Pro lose completion despite quality gains, and broad orchestration uplift remains `NO-GO`; no learned artifact was supplied |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md) | `0.1.98` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 provider-backed runs | `VALID_BASELINE` with zero setup failures and zero safety violations; Auto/Pro trade quality gains for materially lower completion and higher latency, so broad orchestration uplift remains `NO-GO` |
| [13A repair calibration](evaluations/CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md) | `0.1.95` | Exact repair rerun: 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | All 6 completed with 7/7 checks and no safety violation; false external grounding is closed on the frozen case; `CALIBRATION_GO` for matrix expansion only |
| [13A calibration pilot](evaluations/CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md) | `0.1.94` | 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | Coding passed in all modes; all long-horizon effects verified but terminal completion failed on the shared external-grounding contract; `CALIBRATION_NO_GO` for matrix expansion |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md) | `0.1.82` | Prior 72-cell V2 attempt | Four infrastructure-failed RAG/memory cells make it `INVALID_BASELINE`; retained for diagnosis only |
| [Agent Real-World V1](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md) | `0.1.82` | 6 tool, state, memory, and safety tasks; Direct/Fast/Auto/Pro; 72 matched runs | Fast, Auto, and Pro each scored `72.2%`; Auto and Pro added no quality over Fast and had lower completion plus higher latency; `NO-GO` for orchestration uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| Agent Real-World V4 execution contract | current source | Prospective 72-cell matrix with isolated HTTP fixtures and typed tool/effect receipts | Deterministic measurement contract only; no V4 provider-backed observations or intelligence-uplift claim |
| Deterministic quality gates | current source | Runtime, memory, queue/steer, permission, recovery, projection, performance contracts | Control-plane evidence only |

The `0.2.3` V3 matrix is the latest complete provider-backed product baseline.
All 72 cells were retained, with no setup failure and no safety violation. Its
`VALID_BASELINE` status means the matrix is structurally usable; it is not a
capability `GO`. Relative to Fast, Auto and Pro each gained `11.1` quality
points while losing `11.1` completion points and adding `28.163 s` and
`35.744 s` paired median latency, respectively. Five browser timeouts did not
reach receipt collection, so the preregistered receipt gate also fails closed.
Broad orchestration uplift therefore remains `NO-GO`.

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
strict paired high-information mutation input. The ordinary and transfer gates
also reject candidate-only failure and absolute holdout quality, latency, or
token regression against the stable profile, even if relative reviewer reward
is positive. Those are mechanism checks, not answer-quality evidence. The
current campaign does not execute the per-run route/retrieval decision layer, so
no learned routing or retrieval claim is made. A future provider-backed
treatment must compare the pre-transfer Pro profile with a promoted
transfer-trained Pro profile on frozen matched cases before any uplift claim is
allowed.

The Pro-to-Auto path additionally replays the current Pro champion's two gates,
transfers only the complete bounded structural delta relative to the stable Pro
it defeated, and uses an immutable, dual-sided staged canary with conflict-safe
evidence replay. These are deterministic eligibility and rollback mechanisms;
they do not establish that a distilled Auto profile improves provider-backed
answer quality.

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

The `0.2.3` V3 baseline contains 72 position-balanced observations. Direct,
Fast, Auto, and Pro passed `100.0%`, `72.2%`, `83.3%`, and `83.3%` of quality
checks. Completion was `100.0%`, `77.8%`, `66.7%`, and `66.7%`, respectively.
The matrix retained 56 completed runs, 10 failed runs, five timeouts, and one
run waiting for permission. There were no setup failures and no safety
violations.

The signal remains category-specific. Auto and Pro passed all three RAG/memory
checks where Fast passed none, and all shipping treatments passed all coding
checks. Browser behavior moved in the opposite direction: Fast passed `2/3`
quality checks but completed none, while Auto and Pro passed none and completed
none. All three Pro browser runs reached the frozen 600-second deadline. Auto
and Pro also verified the denied-mutation effect safely in every repeat but did
not reach a successful terminal state after denial.

Relative to Fast, Auto and Pro each add two quality-pass runs but lose two
completed runs. Their median latency ratios are `2.60` and `2.67`; token ratios
are `0.95` and `0.55`, within the separate preregistered ceilings. Because
completion non-regression fails, neither candidate is eligible. Five browser
timeouts produced no provider or strategy receipts, which independently makes
the receipt-evidence gate fail closed. Those timeouts remain product failures
in the denominator rather than evaluation infrastructure failures.

No frozen Auto or Pro profile artifact was supplied. Strategy receipts identify
the actual fresh-seed decisions and profiles used by runs that reached evidence
collection, but V3 provides no evidence of GEPA learning, transfer,
self-distillation, learned-profile uplift, or Fugu parity. The next measured
target is therefore narrow: preserve the RAG gain while making browser and
post-denial convergence at least non-regressive against Fast.

## Current Product Gate

`benchmarks/agent/realworld-v4.json` freezes the current prospective execution
contract. It preserves the six product cases, three repeats, Direct/Fast/Auto/Pro
treatments, cyclic Latin-square execution order, non-regression thresholds, and
resource ceilings used by V3. V4 changes the measurement boundary: every browser
cell gets an isolated loopback HTTP fixture, and product tool obligations are
derived from typed receipts for the current Agent run rather than tool-start
events or tool names alone. Only successful receipts can satisfy a required
tool. Projected failed, denied, cancelled, and unfinished attempts remain in the
tool-call denominator; a process-level timeout remains a failed matrix cell even
when it cannot emit a final tool receipt. Browser verification additionally
requires the exact resolved URL and artifact or postcondition digests that bind
evidence to the frozen case.
Setup failures, safety violations, incomplete strategy/provider receipts, or
missing effect evidence make promotion fail closed.

The V4 contract has deterministic structural coverage through
`node --test scripts/agent-realworld-contract.test.mjs`, but no V4 provider-backed
matrix has been executed. It is therefore a prospective measurement contract,
not evidence that the Agent, Auto, Pro, GEPA, or self-distillation improved.

`benchmarks/agent/realworld-v3.json` freezes the protocol used by the latest
measured product baseline. Its 72-cell plan balances treatment position across
case blocks while preserving a globally stable execution index for interruption
and resume. V3 preregistered non-regression and resource ceilings against Fast:
both Auto and Pro had to preserve quality and completion, stay within their
separate latency and token ratios, and at least one had to add one passing or
completed run. Setup failures, safety violations, or incomplete
strategy/provider receipts made promotion fail closed.

The V3 raw and sanitized schemas bind the exact source revision, application
version, suite and execution-plan digests, configured role models, resolved
shipping budgets, actual strategy decision/profile hashes, and every successful
model response. Provider response identifiers and system fingerprints are
domain-hashed before publication; request-body and canonical semantic-response
digests bind text, ordered tool calls, and finish reason without committing
prompts, secrets, or full outputs.
Missing provider identifiers and inconsistent streaming identities remain
explicit evidence failures rather than being counted as valid receipts.

Optional Auto or Pro `FrozenPromptProfileSnapshot` inputs must remain outside
Git. Their byte digest, canonical artifact digest, candidate identity, method,
and promotion provenance are validated and bound into the execution plan before
use. A run without such an artifact is labeled `FRESH_SEED_ONLY` and cannot
support a GEPA, transfer, self-distillation, or learned-profile claim. A frozen
artifact authorizes conclusions only about that exact artifact; it does not
establish general learning uplift or Fugu parity.

The `0.2.3` V3 collection is the latest complete provider-backed product
baseline. The `0.1.98` V2 baseline remains historical evidence for its source
revision, and the earlier `0.1.82` V2 attempt remains invalid. V2 and V3 cover
structured file mutation, code edit plus tests, browser evidence, long-horizon
migration, indexed knowledge plus cross-session memory, and denied mutation.

The feature-gated `cindx-agent-realworld-eval` binary drives the shipping Agent,
tool, permission, RAG, and memory paths. Direct is explicitly marked as a
no-tools model reference rather than product-mechanism evidence. The independent
contract rejects missing or duplicate cells, suite or output hash drift,
unmatched inputs, and unbound Git provenance. Raw outputs remain outside Git.

The runner builds the driver once, then isolates every case, treatment, and
replicate in its own process. Each sample writes a private pending checkpoint
before provider execution. The frozen suite applies a 600-second process
deadline; a timeout is retained as a failed sample, never retried into a pass.
Completed samples are merged in execution order and reused after interruption
only when Git commit, suite hash, full execution plan, profile artifacts,
selection, and replicate count still match. Browser processes are retired by
the sample's unique temporary root so one treatment cannot contaminate the
resources or state of the next.

The current provider-backed matrix is recorded in
[Agent Real-World V3 0.2.3](evaluations/CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.md)
with its [sanitized machine-readable evidence](evaluations/CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.json).
The `0.1.98` V2 baseline, invalid `0.1.82` V2 attempt, and complete V1 baseline remain linked in the
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
  --raw /private/tmp/cindx-agent-realworld-v4.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V4.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V4.md

node scripts/run-agent-realworld.mjs --execute \
  --raw /private/tmp/cindx-agent-realworld-v4.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V4.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V4.md
```

Without `--execute`, it performs only Git, provider, suite, and output-boundary
preflight. A subset may be used for private calibration, but the publication
contract accepts only the complete frozen matrix. Re-running the same command
after interruption resumes validated completed cells from the private raw
checkpoint; timed-out cells remain failures. Optional `--auto-profile` and
`--pro-profile` arguments accept only private frozen snapshot artifacts and bind
their identities into the execution plan.

The offline Fugu matrix is generated by the non-shipping evaluation crate:

```sh
cargo run -p orchestrator-eval --example fugu_evaluation_lab --locked -- \
  --run-plan target/fugu-v1-run-plan.json \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md
```

Missing observations must remain `not_observed`; they are never replaced with
synthetic success.
