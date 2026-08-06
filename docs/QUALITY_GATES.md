# Cindx Quality Gates

The quality gate keeps UX stability, agent correctness, recovery, and performance
evidence separate from provider-backed answer quality. A deterministic green build
does not claim Fugu Ultra equivalence.

## Profiles

- `quick`: documentation, version, desktop layout, and structural UX contracts.
- `ci-contract`: quick checks plus routing, the typed collaboration execution
  contract, the Causal Router v2 contract and scaling bound, Evaluation v2 foundation, the
  120-case arena schema/evidence-ingestion contract, the Agent Real-World V5
  measurement contract, the 18-cell memory-effect evidence contract, the exact
  managed-process session contract, memory attribution, and frontend state
  behavior.
- `control-plane`: documentation and deterministic agent contracts, including
  typed collaboration execution, the Agent Real-World V5 measurement, and
  memory-effect and managed-process session contracts,
  plus Rust workspace and desktop tests.
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
  streaming Markdown work, plus the bounded Agent cognitive-loop projection and
  adaptive cursor and the Causal Router v2 two-action/indexed-evidence receipt.
  It uses operation counts and identity invariants and never cross-machine wall-clock thresholds.
- `full`: all shipping deterministic gates, including the Agent Real-World V5
  measurement and managed-process session contracts, sidecars, frontend
  production build, and Rust tests; heavier same-machine diagnostics remain in
  `performance`.

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
- The Agent Real-World V5 contract tests verify the frozen four-arm matrix,
  iso-budget Grounded Direct baseline, separated mechanism claims, exact-parent
  fail-closed boundary, failed-cell denominator, isolated loopback HTTP fixture,
  current-run successful typed tool receipts, exact browser target, and
  artifact/postcondition digest rules without invoking a provider.
- The exact Agent run-lineage gate proves the versioned logical/physical
  identity contract, three-attempt legacy continuation projection, stable steer
  identity, and fail-closed cycle/cross-scope handling. Storage and desktop
  tests separately prove the indexed logical event scope and physical-only
  permission boundary. A feature-enabled exact receipt gate separately proves
  that legacy continuation attempts remain inside one logical real-world
  evaluation denominator.
- The exact Goal Delta gate admits budget credit only for a first contract
  obligation, recorded grounding receipt, or verified postcondition. It rejects
  ordinary success, failure states, and repeated satisfaction, and keeps receipt
  size independent of raw tool output. Target binding and desktop recovery are
  covered by their separate contract tests rather than inferred from this exact
  filter.
- The deterministic `agent-cognitive-loop-scaling` contract runs
  `task_contract::cognitive_state::tests::compact_cognitive_state_and_adaptive_loop_scaling_contract`
  and requires the `cindx.agent-cognitive-loop-scaling.v1` marker. Its 2,048
  synthetic obligations prove the cognitive JSON remains within 8 KiB, action
  alternatives and truncation remain bounded, and the adaptive cursor remains
  within 512 bytes with a saturated no-gain counter. This is a projection and
  control-state scaling contract, not provider-backed evidence of intelligence,
  answer quality, or long-horizon task success.
- The exact `agent-collaboration-contract` gate requires one
  `cindx.agent-collaboration-contract.v1` marker. It proves distinct evidence and
  exploration catalogs, static read-only effect filtering, current
  collaboration/epoch evidence projection, typed verifier receipts, and
  target-bound collaboration grounding. The combined 4,096-token fixture keeps
  three independent grounding domains, trust policy, and bounded cognitive
  overlay dispatchable together. These are deterministic execution-plane
  contracts, not provider-backed evidence of collaboration quality or
  intelligence uplift.
- The exact `causal-router-v2-contract` gate requires one
  `cindx.causal-router-v2-contract.v1` marker. It proves a stable pre-decision
  fingerprint, explicit workflow/direct counterfactual, deterministic policy,
  capability and effect-authority checks, and exact context-and-action evidence
  semantics. The paired `causal-router-v2-scaling` gate requires one
  `cindx.causal-router-v2-scaling.v1` marker and proves identical selection when
  the exact record is after the prompt's top eight in both 32- and 2,048-row
  inputs. Selection performs at most one exact and one explicit-legacy key
  lookup, scans no history rows after index construction, evaluates at most two
  actions, and keeps its receipt within 4 KiB. These are routing-control
  contracts, not provider-backed quality or intelligence evidence.
- Exact workspace file-plane tests cover stale, ambiguous, racing, locked, and
  failed-publication patches; permission and receipt preservation; degraded
  output-history capture; read hashes and batch partial/offset behavior;
  list/search resource bounds, snapshot pagination, cursor invalidation, UTF-8
  truncation, and coverage; and desktop after-SHA recovery. These are mechanism
  and control checks, not provider-backed Agent-quality evidence.
- The exact managed-process gate proves pending-before-activation, typed terminal
  and nonzero outcomes, bounded polling/input, explicit CPU/output truncation,
  owner/concurrency isolation, cancel-and-reap behavior, credential isolation,
  one-shot input permission, and absence of postcondition verifier authority.
  These are process-control mechanisms, not provider-backed intelligence uplift.
- The execution-role contracts keep Actor and Finalizer stage usage independent,
  expose no tools or Actor turn to Finalizer delivery, bind postcondition quality
  to a typed action/observation receipt, return only a revalidated byte-identical
  grounded fallback, and commit one durable success or failure terminal across
  replay. These deterministic checks establish control semantics, not
  provider-backed answer-quality uplift.
- The typed denial gate keeps permission, policy, capability, and repeated-action
  refusals distinct from ordinary failure and success. It proves one bounded
  same-epoch replan, exact-call suppression before permission, epoch isolation,
  atomic permission-denial persistence, claim rollback and release, startup
  replay over an older checkpoint, hot-recovery preservation, no Goal Delta,
  `Blocked` ledger projection, and visible honest terminal disclosure. This is
  a deterministic control contract, not provider-backed post-denial quality
  evidence.
- The exact failure-curriculum gates project only canonical current-epoch Pro
  timeout, denial, and no-progress facts into bounded hash-only negative seeds.
  It proves redaction, scope binding, deterministic replay and diversity, a
  successful scientific anchor, the shared six-packet ceiling, and exclusion
  from positive, teacher, canary-success, promotion, and distillation roles. It
  does not prove that a generated challenger is more intelligent.
- Memory recall is 100% at top-1 and recall@3 with no trust or dedup failures.
- The two memory-attribution exact gates prove that recovery attempts join by
  one logical run, retain physical-attempt audit identities, and cannot replay
  a partial batch. Portable memory tests separately prove bounded full-key
  replacement, unknown-by-default utility, authority-sensitive expiry, and
  matched-receipt-only helpful/harmful classification.
- The deterministic memory-effect gate validates the frozen 18-cell
  memory-on/off protocol, complete provider-receipt coverage, raw/output
  digests, failed-cell denominator, regression precedence, and irrelevant
  control. It does not call a provider or establish live memory benefit.
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

The Agent Real-World V5 provider run is likewise outside deterministic profiles.
Its complete 72-cell publication contract uses a cyclic Latin-square execution
order across Oracle Reference, Grounded Direct, Auto, and Pro. It retains every
failed, timed-out, or denied cell in the denominator and requires complete
strategy, observed provider-response, and successful typed tool receipts before
any scoped mechanism claim can improve. Browser cells use an isolated loopback
HTTP fixture and must bind the exact resolved target plus artifact or
postcondition digests. Frozen learned profiles are optional private inputs;
without an actually executed exact stable parent, learned-profile and
distillation claims remain `NOT_EXERCISED`. The deterministic gate runs only:

```bash
node --test scripts/agent-realworld-contract.test.mjs scripts/agent-realworld-claims.test.mjs
```

It does not contact a provider and cannot establish Agent-quality or intelligence
uplift. The historical V4 `0.2.9` provider-backed report is a `VALID_BASELINE`, but
the collector classifies continuation tool events in two Fast runs as outside
the current logical run. Its preregistered receipt gate and broad
orchestration-uplift decision are therefore `NO-GO`. Current source repairs the
deterministic identity/projection boundary, but the historical matrix remains
unchanged and a newly frozen provider run is required for any revised decision.

The memory-effect provider matrix is a separate explicit, billable run. Its
deterministic analyzer is exercised with:

```bash
node --test scripts/agent-memory-effect-contract.test.mjs
```

Only a clean exact-revision `--execute` run, followed by publication of
sanitized evidence, may classify memory as `IMPROVED`, `NEUTRAL`, `REGRESSED`,
`NOT_EXERCISED`, or `INVALID_EVIDENCE`. The current suite is
`benchmarks/agent/memory-effect-v2.json`; V1 remains frozen and invalid because
one cell used a permissionless read-only skill tool outside its allowlist. V2
keeps the same cases and matched design but adds that observed `skill.search`
tool to the frozen read-only allowlist. Both matrices fix the same compatible
model and direct/read-only route before applying
memory-on/off, deny mutation requests, and require the negative-control decoy to
be recalled rather than trivially absent. They are not Auto-router comparisons.
Raw provider output remains outside Git.
