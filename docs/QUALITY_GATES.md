# Cindx Quality Gates

The quality gate keeps UX stability, agent correctness, recovery, and performance
evidence separate from provider-backed answer quality. A deterministic green build
does not claim Fugu Ultra equivalence.

## Profiles

- `quick`: version, desktop layout, and structural UX contracts.
- `ci-contract`: quick checks plus routing, Evaluation v2 foundation, the
  120-case arena contract, memory, and frontend state behavior.
- `control-plane`: deterministic agent contracts plus Rust workspace and desktop tests.
- `performance`: long-session incremental projection, bounded context governance,
  and 20k-chunk RAG diagnostics.
- `full`: all deterministic gates, sidecars, frontend production build, and Rust tests.

Run a profile and keep its machine-readable report:

```bash
node scripts/run-quality-gates.mjs \
  --profile full \
  --report target/quality-gate-report.json
```

The manifest is versioned at
`benchmarks/system/quality-gates-v1.json`. Commands never use an interactive shell,
and report assertions are checked after each producer exits successfully. Structured
`cindx.*.diagnostic.*` JSON records emitted by performance tests are collected in the
top-level `diagnostics` array of the quality-gate report, including repeated-sample
P50/P95 timings where available.

Compare two reports captured on the same hardware and build profile before accepting
an optimization:

```bash
node scripts/compare-performance-reports.mjs \
  --baseline target/performance-before.json \
  --candidate target/performance-after.json \
  --policy benchmarks/system/performance-policy-v1.json \
  --report target/performance-comparison.json
```

The versioned same-machine policy applies workload-specific P95 tolerances to Session
projection, context governance, and RAG search. Without `--policy`, the comparator
retains its compatible 25% relative or 1ms absolute allowance. Cross-machine
comparisons remain diagnostic only.

## Required Invariants

- Existing desktop layout and interaction contracts remain unchanged.
- Auto routing passes all 72 versioned cases without over- or under-orchestration.
- Evaluation v2 remains bound to its frozen pre-GEPA routing baseline.
- The Agent Arena validates all 120 versioned cases and remains explicitly
  unmeasured until all 1,440 provider-backed paired runs exist.
- Memory recall is 100% at top-1 and recall@3 with no trust or dedup failures.
- Queue, steer, permission suspension, recovery, and session projections pass the
  desktop Rust control-plane tests.

## Evidence Boundary

Gate duration and local memory recall time are diagnostics, not portable latency
thresholds. The performance profile records current local timings while enforcing
scaling invariants such as reading one warm Session delta regardless of unrelated
events. Provider-backed completion quality, long-horizon success, and GEPA
promotion require the hidden feedback, Pareto, and test datasets described in
`AGENT_EVALUATION.md`. Missing provider evidence must remain explicit and must never
be converted into a synthetic green result.
