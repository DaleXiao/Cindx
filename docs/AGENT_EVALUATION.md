# Agent Evaluation Baseline

This document separates deterministic engineering checks, provider reasoning
diagnostics, and end-to-end Agent quality. A green build is not an intelligence
claim, and a small GPQA sample is not a real-world Agent benchmark.

## Current Decision Evidence

| Evidence | Version | Scope | Current conclusion |
| --- | --- | --- | --- |
| Agent Real-World V5 contract | current source | 6 frozen product cases; Oracle Reference/Grounded Direct/Auto/Pro; 72 position-balanced cells | Deterministic contract only until the complete provider-backed matrix is retained; separates adaptive-direct, workflow, learned-profile, and distillation claims and fails closed on missing exact-parent or receipt evidence |
| [Agent Real-World V4](evaluations/CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.md) | `0.2.9` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 position-balanced provider-backed runs with current-run tool, strategy, and provider receipts | `VALID_BASELINE`, zero setup failures, and zero safety violations; Auto/Pro pass their paired quality, completion, latency, and token gates, but the collector classifies continuation tool events in two Fast runs as outside the current logical run, so the receipt gate fails closed and broad orchestration uplift remains `NO-GO`; no learned artifact was supplied |
| [Agent Real-World V3](evaluations/CINDX_AGENT_REALWORLD_V3_0.2.3_2026-08-04.md) | `0.2.3` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 position-balanced provider-backed runs with strategy and provider receipts | `VALID_BASELINE`, zero setup failures, and zero safety violations; receipt evidence fails closed on five browser timeouts, Auto/Pro lose completion despite quality gains, and broad orchestration uplift remains `NO-GO`; no learned artifact was supplied |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md) | `0.1.98` | 6 frozen product cases; Direct/Fast/Auto/Pro; 72 provider-backed runs | `VALID_BASELINE` with zero setup failures and zero safety violations; Auto/Pro trade quality gains for materially lower completion and higher latency, so broad orchestration uplift remains `NO-GO` |
| [13A repair calibration](evaluations/CINDX_AGENT_REALWORLD_13A_REPAIR_0.1.95_2026-08-04.md) | `0.1.95` | Exact repair rerun: 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | All 6 completed with 7/7 checks and no safety violation; false external grounding is closed on the frozen case; `CALIBRATION_GO` for matrix expansion only |
| [13A calibration pilot](evaluations/CINDX_AGENT_REALWORLD_13A_PILOT_0.1.94_2026-08-04.md) | `0.1.94` | 2 frozen tool tasks; Fast/Auto/Pro; 6 provider-backed runs | Coding passed in all modes; all long-horizon effects verified but terminal completion failed on the shared external-grounding contract; `CALIBRATION_NO_GO` for matrix expansion |
| [Agent Real-World V2](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.82_2026-08-02.md) | `0.1.82` | Prior 72-cell V2 attempt | Four infrastructure-failed RAG/memory cells make it `INVALID_BASELINE`; retained for diagnosis only |
| [Agent Real-World V1](evaluations/CINDX_AGENT_REALWORLD_V1_0.1.82_2026-08-02.md) | `0.1.82` | 6 tool, state, memory, and safety tasks; Direct/Fast/Auto/Pro; 72 matched runs | Fast, Auto, and Pro each scored `72.2%`; Auto and Pro added no quality over Fast and had lower completion plus higher latency; `NO-GO` for orchestration uplift |
| [Matched provider baseline](evaluations/CINDX_PROVIDER_BASELINE_0.1.78_2026-07-31.md) | `0.1.78` | 12 frozen GPQA-Diamond questions, 36 matched treatments | Direct `10/12`; Auto and Pro `8/12`; no orchestration uplift shown |
| Goal Delta control contract | current source | Deterministic obligation, grounding, postcondition, repeated-satisfaction, failure, and bounded-receipt cases | Ordinary tool success no longer earns budget credit; control-plane evidence only, with no provider-backed quality claim |
| Typed denial and bounded replan contract | current source | Deterministic permission/policy/capability denial, epoch isolation, persistence, evidence visibility, terminal disclosure, and no-credit cases | Denial can converge to an honest blocked result without becoming success; no provider-backed convergence or intelligence-uplift claim |
| Typed Pro failure curriculum contract | current source | Deterministic canonical timeout/denial/no-progress projection, redaction, scope/epoch binding, successful anchor, diversity, replay, and shared six-packet cap | Failed runs can inform challenger generation without becoming positive evidence or teachers; no provider-backed intelligence-uplift claim |
| Deterministic quality gates | current source | Runtime, memory, queue/steer, permission, recovery, projection, performance contracts | Control-plane evidence only |

The `0.2.9` V4 matrix is the latest complete provider-backed product baseline.
All 72 cells were retained, with no setup failure and no safety violation. Its
`VALID_BASELINE` status means the matrix is structurally usable; it is not a
capability `GO`. Relative to Fast, Auto and Pro each gained three quality-pass
runs and completed one and three additional runs, respectively. Their median
latency and total-token ratios remained within the separate preregistered
ceilings. In two Fast runs, the collector nevertheless classified continuation
tool events as outside the current logical Agent run. That evidence error makes
the shared receipt gate fail closed, so broad orchestration uplift remains
`NO-GO`.

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

The current Pro mutation path also has an isolated negative failure curriculum.
Its exact contract proves that only typed current-epoch timeout, denial, and
no-progress facts survive redaction; a successful scientific observation must
anchor their use; selection remains diverse, deterministic, and within the
existing six-packet context limit; and the same failures remain ineligible for
positive, teacher, canary-success, promotion, or distillation roles. This proves
the learning boundary, not that model-generated challengers improve answer
quality.

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

The `0.2.9` V4 baseline contains 72 position-balanced observations. Direct,
Fast, Auto, and Pro passed `100.0%`, `83.3%`, `100.0%`, and `100.0%` of quality
checks. Completion was `100.0%`, `77.8%`, `83.3%`, and `94.4%`, respectively.
The matrix retained 64 completed and eight failed runs, with no timeout,
permission wait, setup failure, or safety violation.

The remaining signal is category-specific. Auto and Pro passed all three
RAG/memory checks where Fast passed none, while all treatments passed every
file, coding, browser, long-horizon, and permission-safety quality check. Browser
terminal completion was still weak: Fast completed `0/3`, Auto `1/3`, and Pro
`2/3`. All product treatments completed all three denied-mutation runs with the
required visible explanation.

Relative to Fast, Auto and Pro each add three quality-pass runs and complete one
and three additional runs. Their median latency ratios are `2.03` and `1.94`;
token ratios are `0.90` and `0.80`, within the separate preregistered ceilings.
Both candidates are individually eligible. The shared promotion gate still
fails because the evidence collector identifies continuation tool events in two
Fast runs as outside the current logical Agent run. One affected long-horizon run
failed and one affected RAG run completed; both remain in the denominator and
were not rerun.

No frozen Auto or Pro profile artifact was supplied. Strategy receipts identify
the actual fresh-seed decisions and profiles, but V4 provides no evidence of
GEPA learning, transfer, self-distillation, learned-profile uplift, or Fugu
parity. The next measured target is narrow: eliminate physical-attempt versus
logical-run lineage-attribution ambiguity without weakening the fail-closed
receipt contract. Current source now implements that deterministic identity and
projection boundary, including a scoped legacy-chain decoder and physical-only
permission/effect scope. This does not change the recorded V4 result. The next
evidence step is a newly frozen provider-backed matrix, not tuning or replaying
these 72 cells.

## Current Product Gate

`benchmarks/agent/realworld-v5.json` freezes the current execution contract. It
preserves V4's six product cases, three repeats, cyclic Latin-square ordering,
isolated browser fixture, typed effect receipts, exact target binding, and
permission-safety checks. Its four treatments are deliberately different:

- `oracle_reference` is the former no-tools Direct answer ceiling and is not a
  product baseline;
- `grounded_direct` runs Auto's shipping policy and budget through the same
  AgentKernel, models, tools, retrieval, memory, permissions, and external
  postcondition verifier, while disabling workflow collaboration after routing;
- `auto` is the iso-budget adaptive candidate;
- `pro` is reported descriptively because its native budget is larger.

Adaptive-direct and workflow conclusions use only their actual matched route
subsets, so a mixed Auto matrix does not collapse both mechanisms into one
claim. Workflow requires `workflow_profile_exercised=true`. A result without an
observed behavior difference is at most `NEUTRAL`; a positive state additionally
requires the frozen quality improvement, completion non-regression, latency,
token, setup, and safety gates. Failed, timed-out, denied, and early
unclassifiable runs remain in the matched denominator. Learned-profile and
Pro-to-Auto distillation claims require receipts proving that the challenger was
compared with its actually executed exact stable parent; otherwise they remain
`NOT_EXERCISED`.

The V5 contract has deterministic structural coverage through
`node --test scripts/agent-realworld-contract.test.mjs scripts/agent-realworld-claims.test.mjs`.
This validates the mechanism only. Until a complete provider-backed V5 matrix
is retained, V4 `0.2.9` remains the latest measured product baseline and no new
intelligence claim is authorized.

The exact Goal Delta gate proves that failed, denied, cancelled, unrelated, and
repeated satisfaction cannot extend a segment, while first satisfaction of a
contract obligation, first recorded grounding receipt, or verified
postcondition can. It also proves that the receipt stays bounded when the
observation is large. Target binding and desktop persistence/recovery remain
separate deterministic contracts; this exact filter does not prove them. This
is a run-control improvement. V4 measured the combined shipping path, but it
contains no causal ablation that attributes quality or completion to Goal Delta.

The typed denial gate proves that a permission denial is `Blocked`, never
`Satisfied`, carries no Goal Delta, survives bounded checkpoint and hot/cold
permission recovery without raw arguments, and requires visible denial evidence
plus an explicit terminal disclosure. Policy and unavailable-capability denials
share one same-epoch replan token; a subsequent denial finalizes blocked, while a
new contract epoch clears the state. These are deterministic control-plane
properties. V4 completed all denied-mutation product runs with the required
visible disclosure, but that narrow observation does not override the shared
receipt `NO-GO`.

`benchmarks/agent/realworld-v4.json` remains the immutable protocol used by the
latest measured V4 product baseline. Its 72-cell plan balances treatment position across
case blocks while preserving a globally stable execution index for interruption
and resume. V4 preregistered non-regression and resource ceilings against Fast:
both Auto and Pro had to preserve quality and completion, stay within their
separate latency and token ratios, and at least one had to add one passing or
completed run. Setup failures, safety violations, or incomplete
strategy/provider receipts made promotion fail closed.

The V4 raw and sanitized schemas bind the exact source revision, application
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

The `0.2.9` V4 collection is the latest complete provider-backed product
baseline. V3 and the `0.1.98` V2 baseline remain historical evidence for their
source revisions, and the earlier `0.1.82` V2 attempt remains invalid. V2
through V4 cover structured file mutation, code edit plus tests, browser
evidence, long-horizon migration, indexed knowledge plus cross-session memory,
and denied mutation.

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
[Agent Real-World V4 0.2.9](evaluations/CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.md)
with its [sanitized machine-readable evidence](evaluations/CINDX_AGENT_REALWORLD_V4_0.2.9_2026-08-06.json).
V3, the `0.1.98` V2 baseline, invalid `0.1.82` V2 attempt, and complete V1
baseline remain linked in the [evaluation index](evaluations/README.md) as
immutable historical evidence.
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
  --raw /private/tmp/cindx-agent-realworld-v5.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V5.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V5.md

node scripts/run-agent-realworld.mjs --execute \
  --raw /private/tmp/cindx-agent-realworld-v5.raw.json \
  --sanitized docs/evaluations/CINDX_AGENT_REALWORLD_V5.json \
  --markdown docs/evaluations/CINDX_AGENT_REALWORLD_V5.md
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
