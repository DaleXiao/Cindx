# Cindx Agent Core V3 Evaluation

Date: 2026-07-25

This report separates verified behavior from research claims. A passing local
contract is not evidence of model-quality parity, and a small provider-backed
sample is not a benchmark result.

## Scope

Agent Core V3 changes seven production boundaries:

1. Session permissions are matched to session, action, risk, scope, and grant
   time. Destructive capabilities are never reused as session-wide grants.
2. The production agent loop accepts an injected model provider and has direct
   tests for permission, cancellation, recovery, and lifecycle behavior.
3. Task graph execution owns dependencies, terminal delivery, partial recovery,
   checkpoints, resume, and steer/queue control instead of treating those as
   unrelated UI states.
4. Context projection is incremental and evidence selection is budgeted before
   provider requests.
5. Durable memory generation, deduplication, supersession, trust filtering, and
   recall are separated from workspace knowledge indexing.
6. GEPA mutations preserve executable harness genes and promotion remains
   blocked without paired execution and holdout evidence.
7. Frontend state and Tauri transport were split into controllers and explicit
   transport types. Production Tauri failures are no longer converted into
   successful local-only state changes.

## Provider-backed GPQA diagnostic

The diagnostic uses the same three frozen GPQA-Diamond cases (Biology,
Chemistry, and Physics) before and after the routing and terminal-delivery
changes.

| Candidate | Correct | Total latency | Biology | Chemistry | Physics |
| --- | ---: | ---: | ---: | ---: | ---: |
| pre-final Auto | 2/3 | 256383 ms | 35601 ms, pass | 150028 ms, failed | 70754 ms, pass |
| Agent Core V3 Auto | 3/3 | 172480 ms | 41292 ms, pass | 103441 ms, pass | 27747 ms, pass |

The failed pre-final Chemistry run selected `plan_execute_review` only because
the prompt was long. It exhausted the planner/executor path and ended with
unresolved dependencies. V3 routes a low-complexity, self-contained,
tool-free question directly even when its text is long. The replacement run
used the `single` policy and returned the correct answer.

On this exact three-case sample, correctness increased from 66.7% to 100% and
total latency decreased by 32.7%. This is a routing regression diagnostic, not
a statistically useful GPQA score. No release or Fugu claim may extrapolate
from three cases.

## Deterministic and provider mechanism evidence

The current integration tree passed:

- desktop Rust library: 235 passed, 6 provider tests ignored by default;
- orchestrator: 147 passed;
- agent runtime: 118 passed, 1 performance diagnostic ignored by default;
- memory: 23 passed;
- RAG: 23 passed, 1 performance diagnostic ignored by default;
- frontend: 17 passed;
- production frontend build, desktop layout gate, desktop structure gate, and
  CI-contract quality profile;
- provider-backed paired ablation: read-only workspace evidence changed the
  answer from unverifiable to exact, with a recorded `file.read` result;
- provider-backed GEPA reflection: a disabled tool gene was repaired and the
  resulting candidate completed the evidence task.

The two provider tests establish executable mechanism behavior. They do not
establish that a GEPA candidate is safe to promote. Promotion still requires
the frozen feedback/Pareto/hidden-test split, repeated paired executions,
holdout replay, confidence bounds, and staged rollout evidence.

## Performance and memory

Same-machine performance comparison against the pre-final integration baseline:

| Metric | Baseline p95 | V3 p95 | Result |
| --- | ---: | ---: | --- |
| warm session projection | 167 us | 128 us | pass |
| context projection | 5664 us | 3931 us | pass |
| semantic RAG | 52493 us | 46575 us | pass |
| literal RAG | 125352 us | 111819 us | pass |

The memory suite reported 10/10 top-1, 10/10 recall-at-3, zero trust
violations, zero deduplication failures, zero supersession failures, and 68 us
average local recall. These are deterministic product-memory cases, not a
claim about open-domain memory quality.

## Fugu Ultra boundary

The checked-in Fugu v1 contract requires 759 matched, provider-backed,
protocol-equivalent runs:

- quality-first parity: 0/264 observed;
- iso-budget parity: 0/264 observed;
- causal ablation: 0/231 observed.

Therefore Fugu parity is not ready and no numerical parity claim is supported.
V3 implements product mechanisms that can be evaluated against Fugu-style
orchestration: role-specific workers, task-graph execution, bounded recovery,
dynamic routing, executable harness mutation, and evidence-based selection.
Those mechanisms are not weights-level training and are not equivalent to
Fugu Ultra until the complete external matrix is run.

## Remaining architecture debt

This change is a material split, not a declaration that the repository is
fully modular:

- `App.tsx` is reduced from 5223 to about 2826 lines and from about 90 local
  state hooks to about 24, but it remains a large composition root.
- `tauri.ts` is reduced by moving explicit types and controllers out, but it
  remains about 2610 lines and should be split by command domain.
- Settings and SessionThread still contain broad UI responsibilities.
- Several legacy desktop Rust modules still import through the desktop prelude
  and must move to explicit crate-level dependencies in later bounded changes.
- The full provider-backed arena has 0/1440 observations and cannot support a
  broad quality claim.

These items remain visible so future work cannot report a cosmetic file split
as complete architecture remediation.

## Reproduction

```bash
node scripts/run-quality-gates.mjs --profile ci-contract \
  --report target/quality-gate-ci-contract-agent-v3.json
node scripts/run-quality-gates.mjs --profile performance \
  --report target/quality-gate-performance-agent-v3.json
cargo test --workspace --locked
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --locked
npm --prefix apps/desktop test
npm --prefix apps/desktop run build
```

Provider-backed tests require the user's configured provider and explicit
network access, so they remain ignored in credential-free CI.
