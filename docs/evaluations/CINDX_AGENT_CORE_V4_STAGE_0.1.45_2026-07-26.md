# Cindx Agent Core V4 Stage Report 0.1.45

Date: 2026-07-26

This is a stage report, not a claim that the Agent Core V4 program is complete.
It records the implementation that is being released for review, the evidence
available at this checkpoint, and the unresolved work that remains paused.

## Implemented in this stage

1. **One execution contract.** Direct, collaborative, and task-graph paths now
   share run budgets, cancellation state, result-frontier handling, and final
   delivery guidance instead of maintaining incompatible local rules.
2. **Deadline-aware final delivery.** The runtime reserves time for a final
   answer, lets task-graph delivery own a contract-valid synthesis, and avoids
   a redundant second model call when the workflow has already produced one.
3. **Recoverable interruption.** Streaming worker output and bounded partial
   results survive cooperative cancellation, so a deadline or quorum decision
   does not automatically erase useful completed work.
4. **Task-graph recovery.** The graph records partial outcomes, preserves the
   best available frontier, and can return an authoritative terminal result
   instead of exposing unresolved dependency state as the user answer.
5. **Context and memory controls.** Context evidence is bounded and deduplicated;
   memory extraction and recall distinguish identity, durable requirements,
   evidence, and untrusted instructions. Retrieval benchmarks remain explicit
   product tests rather than open-domain quality claims.
6. **Prompt-learning gate.** Prompt mutations are evaluated as paired candidates
   and require holdout/Pareto evidence before promotion. A generated mutation is
   not treated as an improvement merely because it was produced.
7. **Provider and permission boundaries.** Provider wire parsing was split from
   the provider facade. Session permission reuse is constrained by task, action,
   risk, and grant time; destructive capabilities are not silently generalized.

## GPQA evidence at this checkpoint

The frozen 12-question GPQA-Diamond diagnostic at commit `17eba6c` reported:

| Treatment | Completed | Budgeted score | p50 latency | p95 latency |
| --- | ---: | ---: | ---: | ---: |
| direct default | 10/12 | 83.33% | 27.5s | 120.0s |
| Cindx Auto | 10/12 | 83.33% | 50.2s | 205.4s |
| Cindx Pro | 6/12 | 50.00% | 136.4s | 206.9s |

This result does **not** demonstrate orchestration uplift. Direct and Auto tied,
while Pro completed fewer cases.

Later targeted diagnostics used one previously failing Chemistry case to test
the recovery mechanism:

- At commit `7afb9f0`, Pro recovered a correct answer at the 300s treatment
  boundary after direct and Auto timed out. This demonstrates that cooperative
  partial-result recovery can work; `n=1` cannot establish a quality gain.
- At commit `0b6d111`, all three treatments failed on the same case under a
  different provider run. Pro retained token telemetry but timed out. This
  demonstrates material provider variance and an unresolved Pro latency tail.

No post-`0b6d111` full 12-question matched evaluation was completed before this
stage was paused. Therefore version 0.1.45 must not be described as GPQA-better,
Fugu-Ultra-equivalent, or frontier-agent parity.

## Verified improvements and limits

- Targeted unit and integration tests cover shared execution, cancellation,
  cooperative collection, task-graph delivery, prompt evaluation, permission
  matching, memory recall, and retrieval fusion.
- Earlier same-machine diagnostics measured lower context and retrieval latency,
  but the release gate is rerun for this exact commit before packaging.
- The evaluation schema now records planning, workflow, per-step, and terminal
  timing without committing prompts, answers, or credentials.

## Release verification

The exact staged source was checked before packaging:

- workspace Rust tests passed, including 133 active Agent Runtime tests, 152
  Orchestrator tests, 26 Memory tests, 23 active RAG tests, and 33 Tools tests;
- desktop Rust tests passed: 243 passed, 6 explicitly ignored provider-backed
  tests, 0 failed;
- frontend behavior tests passed: 17/17;
- browser sidecar integration passed 12 real CDP actions;
- production TypeScript/Vite build and desktop structure/layout gates passed;
- root workspace and desktop `clippy --all-targets -- -D warnings` passed;
- the local release build completed, its clean-data startup probe created and
  opened the SQLite state, and both the built and installed app passed strict
  code-signature verification.

The installed bundle reports `CFBundleShortVersionString=0.1.45` and
`CFBundleVersion=0.1.45`. The Apple Silicon archive SHA-256 is
`cd7bedb784a860d475426c761b756527c7e481864008f99ca18f5c220764bcbd`.

Repository-wide `cargo fmt --check` is not a passing gate at this checkpoint:
it reports formatting drift in baseline files outside this stage, including
Agent Application, MCP, and Skills modules. Those unrelated files were not
mechanically reformatted during release; compilation, tests, clippy, packaging,
and startup verification are all passing.

## Known unresolved work

1. The Pro worker pool is capped at three distinct models and currently orders
   Planner, Executor, Reviewer before Summarizer. With four configured role
   models, the dedicated Summarizer is excluded and final synthesis can fall
   back to the Planner. This is a confirmed role-budget defect and is not fixed
   in 0.1.45 because implementation was explicitly paused for review.
2. The final full matched GPQA rerun is still required after the role-budget
   correction. Repeated windows are needed before attributing changes to the
   harness rather than provider variance.
3. Fugu parity remains unmeasured. Role collaboration, task graphs, learned
   routing, and prompt evolution are mechanisms to evaluate, not evidence of
   equivalent model intelligence.
4. Large desktop composition modules and broad prelude imports remain. The
   current split is material but not complete architecture remediation.
5. LiveCodeBench and SciCode remain blocked until a trusted disposable execution
   sandbox exists. Generated benchmark code must not run on the host.

## Review decision

Release 0.1.45 as an inspectable intermediate checkpoint. Do not continue model
role changes or broad benchmark expansion until the architecture and GPQA
findings in this report are reviewed.
