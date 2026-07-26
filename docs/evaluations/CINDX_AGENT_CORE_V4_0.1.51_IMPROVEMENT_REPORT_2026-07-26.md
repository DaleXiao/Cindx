# Cindx Agent Core V4 0.1.51 Improvement Report

## Verdict

This release establishes a cleaner and more defensible agent runtime, but it
does not demonstrate frontier-level intelligence or Fugu Ultra parity.

- The engineering work is real: production execution, evaluation code,
  persistence, background work, task-graph revision, semantic memory, and
  prompt evolution now have narrower ownership boundaries and stronger tests.
- The matched GPQA result is mixed: Auto is faster and keeps its 9/12 score,
  while Pro completes more cases but falls from 8/12 to 7/12 correct. Direct
  remains the strongest treatment at 10/12 in this run.
- Therefore the release may claim better runtime structure, observability, and
  some latency/delivery improvements. It may not claim orchestration uplift,
  self-evolution uplift, or Fugu Ultra equivalence.

## What Changed

### Agent execution

- Fresh, retry, and resume paths now prepare runs through one engine instead of
  rebuilding overlapping state independently.
- Fast remains a direct single-model path. Auto and Pro ask the configured
  conductor for a bounded execution decision; invalid decisions receive one
  repair attempt and then fall back safely.
- The conductor decision covers model assignment, retrieval, memory, tools,
  risk, verification, budgets, and stop conditions as one validated contract.
- Collaboration keeps a direct anchor, a best-known-result frontier, terminal
  delivery reserve, bounded repair, cancellation, and fail-soft delivery.

### Task graph and context

- The runtime can revise a failed graph step at most twice, while preserving
  completed work and rewiring only affected descendants.
- Retrieval is selected dynamically. When requested, semantic, literal-file,
  and graph seed searches run in parallel; graph walking starts only after seed
  evidence exists.
- Context assembly protects the active execution contract, current objective,
  verified evidence, and trusted memory before lower-priority workspace text.

### Memory and GEPA

- Semantic memory candidates must cite source events. Requirements can survive
  incomplete runs, but assistant outcomes require verified completion evidence.
- Memory production is queued through one bounded, deduplicating background
  worker and yields to foreground agent work.
- Prompt evolution now uses a durable request/worker lifecycle. Promotion
  requires paired execution evidence, independent repeats, holdout coverage,
  safety, and a frozen provenance-bound snapshot. Seed prompts are not reported
  as learned GEPA improvements.

### Architecture, performance, and safety

- Research-only arena, benchmark, and Fugu evaluation code moved from the
  shipping `orchestrator` crate into `orchestrator-eval`; ordinary product builds
  do not compile it.
- `persistence_runtime.rs` fell from 2,042 to 733 lines. Configuration, project
  sessions, event persistence, event security, sidecars, and runtime-value
  normalization now live in explicit production modules.
- `agent_commands/task.rs` fell from 833 to 609 lines and `tool_execution.rs`
  from 1,498 to 1,101 lines. These are improvements, not completion: the latter
  remains too large.
- Session/event hot paths use scoped indexed reads, event-kind filters, and
  incremental read models. No-op memory projections no longer rewrite state.
- Prompt evolution and semantic-memory jobs wait for foreground idle time;
  model streaming background work is preemptible and requeued.
- Session permissions now match the same session, action, risk, and optional
  scope. Destructive permissions are never session-reusable, and shell reuse is
  bound to the canonical working directory.

## Verification

The integrated release gate passed:

- 5 version-stamping tests.
- 12 browser-sidecar integration actions.
- 17 frontend unit tests.
- 489 workspace Rust tests, with 2 explicit performance/provider diagnostics
  ignored.
- 251 desktop Rust tests, with 6 explicit provider-backed tests ignored in the
  ordinary gate.
- 774 passing automated checks in total, plus desktop structure and responsive
  layout contracts.
- Production TypeScript/Vite build and Tauri arm64 release build.
- Bundle signing verification and a clean-HOME startup probe.
- Installed bundle: `/Applications/Cindx.app`, version `0.1.51`, 126 MB.
- Distribution ZIP: 49 MB, SHA-256
  `4772f70ea5608aa0761f6d1a7af176f5466325ad7920ed4c228acd0170d9ea2b`.

The only build warning was the macOS compact-unwind size warning for the debug
test binary. The optimized application completed without a source warning.
Interactive visual inspection could not run because macOS was locked; this is
recorded as missing visual evidence rather than a pass.

## Matched GPQA Result

Protocol: the same 12 frozen GPQA-Diamond case IDs used by 0.1.47, with a 180 s
model-call timeout and 300 s treatment deadline. Failures remain in the
denominator. The raw evidence stays private; committed reports contain hashes,
scores, latency, usage totals, and sanitized diagnostics.

| Treatment | 0.1.47 correct | 0.1.51 correct | 0.1.47 completed | 0.1.51 completed | p50 latency before -> after |
| --- | ---: | ---: | ---: | ---: | ---: |
| Direct | 8/12 | 10/12 | 8/12 | 10/12 | 38.6 s -> 31.8 s |
| Auto | 9/12 | 9/12 | 10/12 | 9/12 | 120.0 s -> 64.9 s |
| Pro | 8/12 | 7/12 | 9/12 | 10/12 | 160.1 s -> 123.3 s |

Current paired exact McNemar results are non-significant: Direct vs Auto
`p=1.000`, Direct vs Pro `p=0.250`, and Auto vs Pro `p=0.500`. Provider
stochasticity and the small sample prevent attributing Direct's improvement to
this code change.

Auto's median latency improved substantially and all nine delivered answers
were correct, but it lost one completion. Pro reduced median latency, reduced
timeouts from three to one, and completed one more case, but three delivered
answers were wrong. The current Pro collaboration path therefore improves
delivery without reliably improving answer selection.

## Remaining Engineering Debt

- `tests.rs` remains 9,129 lines, `tool_execution.rs` 1,101 lines, and
  `orchestrator/src/routing.rs` 1,247 lines. Further splits must move owned data
  and dependencies, not merely use `include!` or `use super::*` facades.
- The release build still has a large native dependency graph, dominated by
  Lance/DataFusion. A clean arm64 release build took 12 minutes 38 seconds and
  produced 16 GB of temporary Cargo output before cleanup.
- Frontend diagram chunks still trigger Vite's over-500 kB advisory. They are
  lazy chunks, but their load behavior needs measured startup and interaction
  profiling before further dependency removal.

## Next Evidence Gate

The next intelligence change should not add more unconditional collaboration.
It must make collaboration earn its place against the direct anchor:

1. The conductor predicts the expected marginal value and latency of each
   branch using matched historical evidence, not coarse prompt keywords.
2. Auto and Pro stop at the direct anchor unless an independent branch produces
   evidence that survives bidirectional comparison.
3. The final selector must compare factual support and answer consistency, not
   reward consensus or verbosity by themselves.
4. GEPA promotion requires a larger frozen multi-domain holdout and must improve
   both paired quality and delivery under the same budget. A lower timeout count
   alone is insufficient.
5. The identical GPQA sample is rerun first. Broader GPQA, coding, long-context,
   tool-use, and safety suites follow only after the small paired gate improves.

Until those conditions are met, Fast/Direct remains the quality reference and
Pro remains experimental rather than a claimed superior mode.
