# Cindx Agent Evaluation

Cindx separates deterministic harness regressions from observed answer quality. A green build proves the routing contract is stable; it does not claim that a model answer is correct.

## Fugu v1 Parity Evaluation

The frozen Fugu Ultra public-protocol matrix, safety attestation contract,
quality-first and iso-budget tracks, and causal-ablation report are documented in
[`FUGU_EVALUATION.md`](FUGU_EVALUATION.md). Its CLI validates evidence and emits
an Evaluation Card but never executes third-party benchmarks itself.

## Agent Arena

`benchmarks/agent/arena-v1.json` is the provider-backed comparison contract for
single-model, Fast, Auto, and Pro. It contains 120 versioned tasks across 12
capability families, uses the same wall-time, model-call, tool-call, and token
budget for every mode, and requires three matched seeds per case. A complete
comparison therefore contains 1,440 observations.

The checked-in gate validates the suite but deliberately reports `ready=false`
because CI has no provider credentials:

```bash
cargo run -p orchestrator --example arena_lab --locked -- \
  --report target/agent-arena-contract.json
```

Controlled evaluation supplies JSONL observations and turns on the hard gate:

```bash
cargo run -p orchestrator --example arena_lab --locked -- \
  --observations path/to/provider-arena-observations.jsonl \
  --report target/agent-arena-report.json \
  --require-ready
```

Every observation is bound to the suite version, case, seed, mode, candidate,
and exact shared-budget fingerprint. Readiness requires a complete one-to-one
matrix, provider-backed provenance, deterministic verifier evidence, zero
budget overruns, and no duplicate run keys. Missing evidence remains visible;
synthetic scores cannot make the arena green.

The checked-in 120-case Arena is a schema and evidence-analysis contract. It
does not contain an executable provider runner or the private task fixtures, so
its credential-free CI result is `not_observed`, not an Agent-quality pass.

## Provider-backed matched baseline

The narrow executable baseline reuses the existing ignored GPQA provider test
for 12 pinned GPQA-Diamond questions across Direct, Auto, and Pro. Run preflight
first; it verifies the pinned dataset SHA-256, a clean tracked worktree, the full
Git commit, and private output boundaries without making a provider call:

```bash
node scripts/run-provider-baseline.mjs \
  --gpqa /private/path/gpqa_diamond.csv \
  --raw /private/tmp/cindx-provider-baseline.raw.json \
  --sanitized docs/evaluations/CINDX_PROVIDER_BASELINE_CURRENT.json \
  --markdown docs/evaluations/CINDX_PROVIDER_BASELINE_CURRENT.md
```

After reviewing the preflight, add `--execute` to the same command. This is the
only mode that invokes the configured cloud provider. The script passes paths
as process arguments without a shell, never accepts or prints an API key, keeps
raw prompts and outputs outside the repository, and publishes only the
sanitized report after the frozen baseline contract succeeds.

This is a matched product-treatment baseline: all three treatments use the same
questions, expected-answer hashes, source commit, and evaluation limits, while
Auto and Pro intentionally use their configured role models. Model differences
therefore remain a confound when attributing any difference solely to
orchestration. The 12-question, one-repeat result is descriptive; it cannot
support a significance claim, Fugu parity, GEPA improvement, or a general claim
that Auto or Pro is superior.

## Evaluation v2 Foundation

The pre-GEPA baseline is frozen at commit `340e263c6207cb043655a870661fb2be317f95bd` in `benchmarks/agent/evaluation-v2-baseline.json`. It records SHA-256 fingerprints for the 72-case routing suite and baseline. The foundation command verifies those files before reporting any optimization readiness:

```bash
cargo run -p orchestrator --example evaluation_v2_lab -- \
  --report target/evaluation-v2-foundation.json
```

The quality baseline is deliberately `unmeasured`. Provider-backed quality cannot be inferred from the routing contract.

Evaluation v2 keeps three datasets separate:

- `feedback`: full redacted traces, verifier diagnostics, and actionable side information may be shown to the reflection model.
- `pareto`: only per-case score records may enter candidate selection; prompts, trajectories, and outputs are withheld from reflection.
- `test`: only final score records may enter the release report; test cases never participate in reflection or Pareto selection.

Hidden test datasets belong under `benchmarks/agent/hidden/`, which Git ignores. A release gate must load them explicitly and use `--require-ready`; checked-in fixtures must never stand in for the hidden test set.

Hidden score sets contain only per-case scores and are bound to one dataset SHA-256. Baseline and candidate records must match on `case_id + seed`, use deterministic verifiers, and declare provider-backed provenance. Synthetic gate-smoke data is always blocked from promotion, regardless of its apparent score. Run the paired release gate with:

```bash
cargo run -p orchestrator --example evaluation_v2_lab -- \
  --feedback benchmarks/agent/hidden/feedback.json \
  --pareto benchmarks/agent/hidden/pareto.json \
  --test benchmarks/agent/hidden/test.json \
  --baseline-scores path/to/baseline-scores.json \
  --candidate-scores path/to/candidate-scores.json \
  --promotion-report target/evaluation-v2-promotion.json \
  --require-ready --require-promotion
```

The promotion report enforces the frozen minimum case and repeat counts, absolute success gain, paired Wilson lower bound, per-category regression ceiling, and zero critical safety violations. Passing recommends the first 10% canary stage; it does not skip staged rollout.

## Versioned Contract Suite

The historical pre-GEPA baseline remains frozen against `core-v1`. The active
`benchmarks/agent/core-v2.json` suite contains the same 72 tasks, with complex
workspace coding work upgraded to the four-channel retrieval contract. It covers:

- direct general and coding questions
- coding workflows
- browser and computer use
- grounded retrieval
- ordinary research and comparison
- bounded two- and three-model collaboration
- latency-sensitive complex work

Every case specifies the expected task class, Auto policy, collaboration width,
retrieval mode, and primary model role. `core-v2-baseline.json` requires a 100%
Auto contract pass rate with no over- or under-orchestration.

Run the offline gate and emit a standalone report:

```bash
cargo run -p orchestrator --example evaluation_lab -- \
  --report target/agent-benchmark-report.json
```

The report compares four modes for every case:

- `single_model`: one frontier generalist baseline
- `fast`: one latency-first role model
- `auto`: Cindx intent and complexity routing
- `pro`: explicit three-candidate collaboration

Projected model calls, latency units, and cost units compare topology only. They are not presented as measured provider usage.

## Project Memory Gate

The checked-in `benchmarks/agent/memory-v1.json` suite verifies deterministic project-memory behavior separately from provider answer quality. It covers English and Chinese continuity queries, cross-session recall, requirement trust labels, and duplicate-source compaction. Run it with:

```bash
cargo run -p agent-memory --example memory_lab --locked -- \
  --report target/memory-evaluation.json
```

The gate requires 100% top-1 and recall@3 matches, zero trust-boundary violations, and zero deduplication failures. Reported local recall latency is measured for diagnostics but is not a machine-independent quality claim.

## Real Run Observations

Pass an explicit JSONL file to compare measured quality and efficiency:

```bash
cargo run -p orchestrator --example evaluation_lab -- \
  --observations path/to/observations.jsonl \
  --report target/agent-benchmark-report.json
```

Each line uses `cindx.agent-benchmark-observation.v1`:

```json
{"schema":"cindx.agent-benchmark-observation.v1","suite_id":"core","suite_version":1,"case_id":"general-greeting","mode":"auto","completed":true,"quality":{"correctness":5,"evidence":4,"completion":5,"safety":5},"latency_ms":1200,"prompt_tokens":100,"completion_tokens":200,"estimated_cost_microusd":80,"tool_calls":0,"retrievals":0,"evidence_citations":0,"safety_violations":0}
```

Correctness, evidence, completion, and safety use a 0–5 rubric. A run passes quality only when every dimension is at least 3 and the total is at least 15 of 20. Missing observations remain `null`; Cindx never fabricates quality, latency, token, or cost measurements.

Use human review or an explicitly requested judge-model run for release claims. Runtime Genetic Pareto evaluation is a separate, visible feature: it runs only when the user-facing setting is enabled and a request already enters adaptive Auto/Pro collaboration, publishes paired/replay evidence in Settings, and never turns Fast or lightweight single-model requests into hidden multi-model work.

Pairwise evaluation waits until all foreground agent runs have finished and the app has remained idle briefly. A new foreground request cancels an in-flight evaluation so learning cannot compete with interactive responses. One idle lease may advance a bounded batch of at most four mutation/evaluation steps; this lets offline evolution make progress without coupling work to dozens of later user turns. Each comparison is judged in both candidate orders to reduce position bias, then persisted as one atomic observation pair. Offline train/holdout assignments are recorded in a durable split manifest so adding later cases cannot move old evidence across the evaluation boundary. Token and estimated-cost telemetry remain observable, but model price and token volume do not participate in Pareto dominance; selection prioritizes quality, safety, generalization, task coverage, and latency.

The operational campaign is derived deterministically from durable dataset, reflection, pair,
replay, frontier, and rollout evidence. Its stages are `collect_dataset -> collect_feedback ->
reflect_and_mutate -> paired_train -> holdout_replay -> select_frontier -> canary -> stable`.
Mutation, paired-execution, and replay events carry the current stage, next action, and a
dataset-bound resume token through their run context. An in-flight process does not change the
token, while a dataset digest change does, so a restart resumes the same scientific campaign
without treating an interrupted request as new evidence. Rollback and canary safety states take
precedence over exploration stages.

## Runtime Harness Evolution

Genetic Pareto evolves the bounded Conductor harness, not provider model weights. A genome controls workflow depth, verification strength, context selection, branch width, tool access, retry policy, per-step attempt budget, model-turn budget, and tool-call budget. Learned mutations may change only one or two validated genes and a bounded custom directive. Invalid model-generated mutations receive one bounded schema-repair attempt using exact enum values.

Candidates cannot be promoted from plans, live traffic, or judge prose alone. Cindx runs both the stable and challenger harnesses in a read-only Agent sandbox backed by the active workspace, passes dependency outputs only through declared access lists, and records every model input, output, error, tool request, and tool result. Only `ToolRisk::ReadOnly` tools can cross the sandbox boundary; writes, process execution, browser/computer control, sensitive context, and network tools are rejected both when tools are exposed and again when they execute. Historical tasks are replayed as holdout executions. Plan-only `paired_shadow` and `replay_holdout` records remain readable for compatibility but do not count as promotion evidence.

GEPA reflection consumes only redacted `feedback` trajectories and actionable verifier diagnostics. `pareto` and `test` prompts and outputs never enter reflection. Instance-wise Pareto keeps candidates that lead on at least one repeatedly evaluated case, samples mutation parents by complementary case coverage, and merges only disjoint gene changes from a shared direct ancestor. Aggregate fitness remains a compatibility and rollout signal, not a replacement for per-instance selection.

Promotion requires valid paired and replay executions, no safety or format regression, bounded train/holdout generalization, and a replay-only Wilson lower confidence bound of at least 0.50. The confidence gate counts only direct challenger-versus-current-stable holdout comparisons, so wins against weaker exploratory profiles cannot authorize rollout. At least four direct replay comparisons are required. A qualifying challenger receives deterministic staged traffic at 10%, 25%, 50%, and 100%. Each stage requires fresh direct replay comparisons and live evidence. Safety failures, repeated completion failures, or a material reward regression automatically restore the stable profile and persist an auditable rollout event.

This reproduces Fugu-style dynamic workflow search, isolated worker execution, bounded harness mutation, and feedback-driven selection using cloud models. It is not weights-level parity with a reinforcement-trained Conductor.

## Quality Regression Baselines

`observed_mode_thresholds` in a baseline can require, per mode:

- minimum observed runs
- minimum completion and quality pass rates
- maximum average latency, tokens, and cost
- maximum safety violations

The checked-in CI baseline leaves these thresholds empty because CI has no provider credentials. A controlled evaluation environment can supply its own suite, baseline, and observation files with `--suite`, `--baseline`, and `--observations`.

## Control-Plane Reliability Gate

Provider quality and harness plumbing are evaluated separately. The local Rust
gate verifies that:

- a warm Session projection reads only its target Session delta even after
  thousands of unrelated events;
- queue mutations remain incremental and replay-safe;
- run, permission, pause, completion, failure, and cancellation use one lifecycle
  reducer;
- runtime and run-control budgets share one source;
- turn-budget exhaustion is typed recoverable control flow rather than a generic
  agent failure;
- Fugu workers reserve a tool-free final answer turn and can resume from bounded
  checkpoints.

Run the deterministic gate with:

```bash
cargo test -p agent-runtime --locked
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --locked
```

These tests prove control-flow and scaling invariants. They do not replace the
provider-backed completion, quality, latency, and safety observations above.

## Release Gate

Before release:

1. Pass the 72-case offline contract suite.
2. Run the Rust workspace and desktop tests.
3. Compare representative real observations across single-model, Fast, Auto, and Pro.
4. Investigate quality, safety, latency, token, cost, or orchestration regressions before changing routing thresholds.
