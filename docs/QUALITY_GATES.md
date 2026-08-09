# Cindx Quality Gates

The quality gate keeps UX stability, agent correctness, recovery, and performance
evidence separate from provider-backed answer quality. A deterministic green build
does not claim Fugu Ultra equivalence.

## Profiles

- `quick`: documentation, version, desktop layout, and structural UX contracts.
- `ci-contract`: quick checks plus routing, the typed collaboration execution
  contract, the Causal Router v2 contract and scaling bound, the authoritative
  Context Compiler contract and operation-count bound, Evaluation v2
  foundation, the
  120-case arena schema/evidence-ingestion contract, the Agent Real-World V5
  measurement contract, the Direct-finalizer causal/evolution contracts, the 18-cell
  memory-effect evidence contract, the exact managed-process session contract,
  memory attribution, and frontend state behavior.
- `control-plane`: documentation and deterministic agent contracts, including
  typed collaboration execution, the Agent Real-World V5 measurement, and
  Direct-finalizer causal/evolution, memory-effect, authoritative Context Compiler, and
  managed-process session contracts,
  plus Rust workspace and desktop tests.
- `performance`: long-session incremental projection, bounded context governance,
  authoritative Context Compiler operation counts, graph/request reuse, frontend
  streaming, conductor health, and 20k-chunk RAG diagnostics.
- `paired-performance`: the stable Session, context, and RAG P95 diagnostics. CI
  applies one current manifest to base and head sequentially on the same runner
  before applying the versioned policy. Sub-microsecond conductor routing remains a
  capacity diagnostic because scheduler noise is larger than a useful hard limit.
- `shipping-performance`: resource-bounded hard gates for incremental Session and
  runtime snapshots, prompt-learning outbox delta projection, shared graph
  parsing, prepared image/request reuse, retry reuse, and linear frontend
  streaming Markdown work, plus the bounded Agent cognitive-loop projection and
  adaptive cursor, the Causal Router v2 two-action/indexed-evidence receipt, and
  the Context Compiler's candidate-proportional operation bound.
  It uses operation counts and identity invariants and never cross-machine wall-clock thresholds.
- `full`: all shipping deterministic gates, including the Agent Real-World V5
  measurement, Direct-finalizer causal/evolution, and managed-process session contracts,
  sidecars, frontend production build, and Rust tests; heavier same-machine
  diagnostics remain in `performance`.

`scripts/check-desktop-rust-light.sh` is a supplemental compile check, not a
quality-gate profile. It deliberately avoids frontend bundle resources and the
heavy LanceDB dependency graph so Rust adapter edits can receive fast type
feedback. It does not validate vector persistence, frontend behavior, or a
shipping bundle; the default-feature full and release gates remain required.
The structure gate also enforces that `agent-runtime` consumes transport-free
model contracts from `agent-core` and cannot regain a direct dependency on the
HTTP/WebSocket `model-provider` crate.

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
  within 4 KiB with a saturated no-gain signal and all eight semantic-action
  identities populated. This is a projection and control-state scaling
  contract, not provider-backed evidence of intelligence, answer quality, or
  long-horizon task success.
- The exact `agent-collaboration-contract` gate requires one
  `cindx.agent-collaboration-contract.v1` marker. It proves distinct evidence and
  exploration catalogs, static read-only effect filtering, current
  collaboration/epoch evidence projection, typed verifier receipts, and
  target-bound collaboration grounding. It also proves that only an explicit
  `AcceptTeam` uplift decision admits workflow guidance; every unproven decision
  selects the Direct control path without applying frontier text. The combined 4,096-token fixture keeps
  three independent grounding domains, trust policy, and bounded cognitive
  overlay dispatchable together. These are deterministic execution-plane
  contracts, not provider-backed evidence of collaboration quality or
  intelligence uplift.
- The route-to-workflow proposal checks additionally require the route decision
  parser, desktop context boundary, and real-world strategy receipt tests to
  reject missing, stale, semantically disconnected, or digest-mismatched
  proposals. They prove that one Conductor response can materialize the same
  executable graph, reject self-dependencies, record the actual materialization
  source, and bind completion evidence; independent verification must reach
  synthesis. They do not prove that Workflow improves a provider-backed result;
  that claim remains gated by the frozen comparison in `AGENT_EVALUATION.md`.
- The exact `causal-router-v2-contract` gate requires one
  `cindx.causal-router-v2-contract.v1` marker. It proves a stable pre-decision
  fingerprint, explicit workflow/direct counterfactual, deterministic policy,
  capability and effect-authority checks, Conductor-owned typed demand, and
  exact-context plus route-shape matched-evidence semantics. Complete failed
  team comparisons remain negative evidence. In current production this policy
  is a read-only shadow: `cindx.execution-plan.v2` binds the real executable
  action to a typed authority receipt, rejects compatibility-value authority,
  and retains V1 only for historical replay. The paired
  `causal-router-v2-scaling` gate requires one
  `cindx.causal-router-v2-scaling.v1` marker and proves identical selection when
  the exact record is after the prompt's top eight in both 32- and 2,048-row
  inputs. Selection performs at most one exact and one route-shape key
  lookup, scans no history rows after index construction, evaluates at most two
  actions, and keeps its receipt within 4 KiB. These are routing-control
  contracts, not provider-backed quality or intelligence evidence.
- The exact `context-compiler-contract` gate requires one
  `cindx.context-compiler-contract.v1` marker. It proves that optional context is
  ranked against the prepared effective objective while the current request,
  protected sources, and complete tool rounds retain hard-invariant authority;
  its `cindx.context-compiler-receipt.v1` projection contains no raw source or
  objective text and stays within 4 KiB. The exact
  `context-compiler-scaling` gate requires one
  `cindx.context-compiler-scaling.v1` marker and bounds candidate scans, term
  indexing, and membership checks using deterministic operation counts rather
  than wall-clock duration. These are request-compilation control and resource
  contracts, not provider-backed answer-quality, GEPA, or intelligence evidence.
- The exact prompt-profile shipping, live-assignment attribution,
  distillation-control, and recovery gates
  require their versioned markers and prove that only a background worker can
  reconcile/publish a compact stable/canary deployment; foreground assignment
  is project-isolated, logical-run-stable, typed on fallback, bound to exact
  genome/deployment/lease lineage, route-isolated, and unable to broaden tool
  authority or runtime limits. Recovery also rejects same-revision history ABA
  through a persistent content binding and cleans binding-only orphans. The
  matching selection-scaling gate compares small and heavily populated unrelated state
  using deterministic lookup/scan/write counts, not wall-clock latency. These
  gates establish causal wiring, recovery, and resource bounds; they do not
  establish provider-backed GEPA, distillation, Auto, or Pro quality uplift.
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
- The Direct-finalizer causal contracts prove that the default phenotype is
  byte-compatible, a non-default phenotype changes only the tools-disabled
  Finalizer prompt, assignment/request/non-fallback delivery receipts remain
  exact, the reviewer and GEPA schemas fail closed, and provider-call accounting
  is durable before execution with a hard 58-call cap. The frozen suite contract
  checks six train, eight holdout and preregistered preservation controls. These
  tests do not contact a provider or establish intelligence uplift.
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

The first authorized Workflow GEPA V4 attempt is `INVALID_EVALUATOR`: both
training tasks completed, but a path-bound revision comparison rejected the
separate isolated roots before validation. Current source uses a bounded
path-independent content fingerprint and preflights every matched case before
provider work. The second authorized run is `INVALID_TASK_SPEC`: its candidate
reached both validation pairs, but the failed research case required lower-case
identifiers and an exact phrase that the public task did not disclose. Its raw
route and resource receipts are diagnostic only; the quality, tie, and
validation decision are not capability evidence. The untouched test matrix and
Grounded Direct control were not run. The Workflow GEPA V5 product campaign
replaced V4 and its first run produced valid targeted no-go evidence. The current
Workflow GEPA V6 protocol is a separate explicit, billable run. Its deterministic
contract checks are part of the Rust suite, but they do not call a provider and
cannot prove learned-profile uplift. A valid V6 run must start from a clean exact
revision and external report/snapshot paths. It uses two public training tasks
to generate three distinct route phenotypes, executes two matched full-product
train pairs for each, and selects only a candidate that clears train quality,
safety, causal-profile, public-route, Workflow-exercise, and measured-gain gates.
The train-only instance Pareto archive prioritizes quality and verified success;
latency and tokens are tie-breakers only. No candidate means `valid_no_go_training`
and validation remains sealed. A selected snapshot stays inside the private
campaign directory until validation, Grounded Direct control, and untouched
test all pass; no failed candidate is published to the external snapshot path.

V6 no longer counts a route-contract change as product improvement. Train
admission requires either an externally verified quality win while both latency
and token ratios are at most `1.25`, or at least a 5% improvement in one resource
while the other ratio remains at most `1.05`.

The selected candidate must next clear two validation-only matched product pairs
before four untouched tasks run with two counterbalanced repeats. Every candidate
cell must carry the exact profile and route-phenotype receipts, all declared route
contracts must pass, and validation plus final test must prove Task Graph use.
Validation requires complete candidate quality, zero losses, both resource
ratios at most `1.25`, and either a quality win or a Pareto-safe 10% resource
gain.
The final gate requires complete quality acceptance for every candidate cell,
quality wins on at least two unseen tasks, zero quality losses, no completion or
safety regression, and aggregate latency and token ratios no greater than
`1.05`. A Grounded Direct product control must also pass. During the isolated
campaign, semantic-memory model extraction and cloud embedding are disabled;
deterministic local memory projection remains so background provider work cannot
contaminate matched resource measurements.
V2 and the first V4 attempt are `INVALID_EVALUATOR`; V3 and the second V4
attempt are `INVALID_TASK_SPEC`. None is a baseline or evidence of capability
change. The V4 candidate is not reusable as promotion evidence, and untouched
test evidence must not be used to select its successor. The first authorized V5
provider run is `VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`: its selected
profile changed a public training route, but unseen validation had no quality
win and materially regressed resources. The fail-closed gate withheld control,
test, and snapshot publication. Any successor campaign requires a new frozen
protocol and separate explicit authorization. V6 is that frozen successor
protocol, but no V6 provider run has occurred.

The current implementation retains the frozen V7 product suite and uses the V11
route-causal evidence schema. Before mutation, each training pair shares one
workflow-capable conductor candidate and proposal. Workflow executes the anchor;
Direct is projected after planning. Exact candidate and proposal hashes, task,
workspace prestate, prompt profile, route profile, provider configuration, and
budget must match while recording the explicit runtime treatment. Incomplete
pairs, plan drift, treatment drift, or profile drift fail closed. The campaign
stops before mutation unless Workflow wins at
least one externally verified matched quality outcome.

Only after that positive treatment signal may GEPA propose two distinct route
policies. A retained candidate changes exactly `route_directive`; the workflow
execution, tool, safety, finalizer, and budget phenotype remains identical. The
existing train, unseen-validation, Grounded Direct, untouched-test, and resource
gates retain their order. The complete path uses at most 33 product runs under
the existing hard cap of 35.

The deterministic gates can be exercised without provider access:

```bash
cargo test -p orchestrator --lib
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --features realworld-eval --lib
```

Passing them proves schema, matched-treatment, profile-identity, route-only
intervention, and fail-closed behavior; it does not prove answer-quality uplift
and does not authorize a provider run.

The separately authorized V11 one-shot campaign uses a clean exact revision and
three fresh outside-Git paths. The resulting journal and report must be retained
even when the command exits with a preregistered no-go:

```sh
run_dir="$(mktemp -d /private/tmp/cindx-workflow-gepa-v11.XXXXXX)"
CINDX_WORKFLOW_GEPA_REPORT="$run_dir/report.json" \
CINDX_WORKFLOW_GEPA_SNAPSHOT="$run_dir/snapshot.json" \
CINDX_WORKFLOW_GEPA_JOURNAL="$run_dir/journal.json" \
cargo run --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --no-default-features --features realworld-eval \
  --bin cindx-workflow-gepa-eval
```

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
