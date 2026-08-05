# Cindx Quality Gates

The quality gate keeps UX stability, agent correctness, recovery, and performance
evidence separate from provider-backed answer quality. A deterministic green build
does not claim Fugu Ultra equivalence.

## Profiles

- `quick`: documentation, version, desktop layout, and structural UX contracts.
- `ci-contract`: quick checks plus routing, Evaluation v2 foundation, the
  120-case arena schema/evidence-ingestion contract, the Agent Real-World V4
  measurement contract, memory, and frontend state behavior.
- `control-plane`: documentation and deterministic agent contracts, including
  the Agent Real-World V4 measurement contract, plus Rust workspace and desktop
  tests.
- `performance`: long-session incremental projection, bounded context governance,
  graph/request reuse, frontend streaming, conductor health, and 20k-chunk RAG
  diagnostics.
- `paired-performance`: the stable Session, context, and RAG P95 diagnostics. CI
  applies one current manifest to base and head sequentially on the same runner
  before applying the versioned policy. Sub-microsecond conductor routing remains a
  capacity diagnostic because scheduler noise is larger than a useful hard limit.
- `shipping-performance`: resource-bounded hard gates for incremental Session and
  runtime snapshots, prompt-learning outbox delta projection, shared graph
  parsing, prepared image/request reuse, retry reuse, and linear frontend
  streaming Markdown work. It uses operation counts and identity invariants,
  never cross-machine wall-clock thresholds.
- `full`: all shipping deterministic gates, including the Agent Real-World V4
  measurement contract, sidecars, frontend production build, and Rust tests;
  heavier same-machine diagnostics remain in `performance`.

`scripts/check-desktop-rust-light.sh` is a supplemental compile check, not a
quality-gate profile. It deliberately avoids frontend bundle resources and the
heavy LanceDB dependency graph so Rust adapter edits can receive fast type
feedback. It does not validate vector persistence, frontend behavior, or a
shipping bundle; the default-feature full and release gates remain required.

`scripts/check-rust-quality.sh` is the mandatory static Rust gate in CI and
release. It rejects formatting drift in the portable workspace and Clippy
warnings across both the workspace and desktop adapter. The desktop lint uses the same no-default-features
compile surface as the light check so routine feedback does not build
Arrow/DataFusion/Lance. Default-feature desktop tests, shipping performance
contracts, and the release build still compile and validate the complete
shipping dependency graph.

Run a profile and keep its machine-readable report:

```bash
node scripts/run-quality-gates.mjs \
  --profile full \
  --report target/quality-gate-report.json
```

The manifest is versioned at
`benchmarks/system/quality-gates-v1.json`. Commands never use an interactive shell,
and report assertions are checked after each producer exits successfully. Gates may
also require proof in their process output so an exact Rust filter cannot pass after
running zero tests. Structured `cindx.*.diagnostic.*` and `cindx.*.scaling.*` JSON
records emitted by performance tests are collected in the top-level `diagnostics`
array of the quality-gate report, including repeated-sample P50/P95 timings where
available.

Create a fail-closed comparison from two clean, distinct checkouts after installing
each checkout's locked frontend dependencies. The wrapper gives both measurements one
pair ID and uses the same process, machine, toolchain, and test profile:

```bash
node scripts/run-paired-performance.mjs \
  --baseline-root ../cindx-base \
  --candidate-root . \
  --output-dir target/performance-regression
```

The versioned policy applies workload-specific P95 tolerances to Session projection,
context governance, and RAG search. It rejects failed producer
reports, duplicate diagnostics, different profile contracts, missing pair metadata,
or a different machine/toolchain fingerprint before reading latency. The low-level
comparator remains available for non-policy diagnostics, but policy comparisons must
come from the paired wrapper. Cross-machine comparisons are never release evidence.

## Required Invariants

- Existing desktop layout and interaction contracts remain unchanged.
- Auto routing passes all 72 versioned cases without over- or under-orchestration.
- Evaluation v2 remains bound to its frozen pre-GEPA routing baseline.
- The Agent Arena validates all 120 versioned cases and remains explicitly
  `not_observed` until all 1,440 provider-backed paired runs exist. This is an
  evaluation-contract result, not an Agent-quality pass.
- The Agent Real-World V4 contract test verifies the frozen matrix, isolated
  loopback HTTP fixture, current-run successful typed tool receipts, exact
  browser target, and artifact/postcondition digest rules without invoking a
  provider.
- The exact Goal Delta gate admits budget credit only for a first contract
  obligation, recorded grounding receipt, or verified postcondition. It rejects
  ordinary success, failure states, and repeated satisfaction, and keeps receipt
  size independent of raw tool output. Target binding and desktop recovery are
  covered by their separate contract tests rather than inferred from this exact
  filter.
- Memory recall is 100% at top-1 and recall@3 with no trust or dedup failures.
- Queue, steer, permission suspension, recovery, and session projections pass the
  desktop Rust control-plane tests.
- Each exact-filtered Rust shipping gate must prove that precisely one matching test
  executed. The gates preserve constant delta visits, shared graph/request storage,
  single request preparation across retries, one-event prompt outbox work after a
  4,096-event checkpoint, and linear frontend parse/join work.

## Evidence Boundary

Gate duration and local memory recall time are diagnostics, not portable latency
thresholds. The paired CI run compares base and head on one temporary host; ordinary
CI, release, and normal local production builds still run the separate
`shipping-performance` profile and retain its report to guard resource growth without
flaky wall-clock limits. Provider-backed completion quality, long-horizon success,
and GEPA promotion require the hidden feedback, Pareto, and test datasets described in
`AGENT_EVALUATION.md`. Missing provider evidence must remain explicit and must never
be converted into a synthetic green result.

The bounded provider baseline runner documented in `AGENT_EVALUATION.md` is an
explicit, billable operation and is never launched by an ordinary quality-gate
profile. Its default mode performs only dataset, Git, and output-boundary preflight;
only `--execute` reaches the configured provider.

The Agent Real-World V4 provider run is likewise outside deterministic profiles.
Its complete 72-cell publication contract uses a cyclic Latin-square execution
order, retains every failed or timed-out cell in the denominator, and requires
complete strategy, observed provider-response, and successful typed tool receipts
before an uplift decision can be `GO`. Browser cells use an isolated loopback HTTP
fixture and must bind the exact resolved target plus artifact or postcondition
digests. Frozen learned profiles are optional private inputs; without them the
report must say `FRESH_SEED_ONLY`, even if Auto or Pro outperform Fast. The
deterministic gate runs only:

```bash
node --test scripts/agent-realworld-contract.test.mjs
```

It does not contact a provider and cannot establish Agent-quality or intelligence
uplift. No V4 provider-backed matrix has been run; V3 `0.2.3` remains the latest
provider-backed decision evidence.
