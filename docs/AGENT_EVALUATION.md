# Cindx Agent Evaluation

Cindx evaluates the harness at three separate levels. A green build is not treated as proof of agent quality.

## 1. Deterministic Routing Gate

Run:

```bash
cargo run -p orchestrator --example evaluation_lab
```

The gate covers direct answers, coding, browser work, grounded retrieval, ordinary research, latency-sensitive work, and bounded two- and three-worker collaboration. It fails on the wrong policy, retrieval mode, or primary model role and reports over- or under-orchestration separately.

## 2. Operational Trace Report

`evaluate_routing_telemetry` aggregates completed local traces without sending data off-device. It reports:

- run outcomes and success rate
- average latency and token cost proxy
- average tool and retrieval calls
- policy and model distribution

Operational success means the run completed, not that its answer was correct. It must not be used as the only learned-router reward.

## 3. Quality Rubric

Representative tasks should be scored from 0 to 5 on correctness, evidence, completion, and safety. `QualityRubricScore` passes only when every dimension is at least 3 and the total is at least 15 of 20.

Use a human reviewer or an explicitly requested judge-model run. Cindx must not spend cloud tokens on hidden background evaluation.

## Release Gate

Before release:

1. Run the deterministic routing gate.
2. Run the Rust workspace and desktop tests.
3. Compare operational traces by Fast, Auto, and Pro.
4. Score a stable representative task set with the quality rubric.
5. Investigate regressions in quality, latency, token use, or over-orchestration before changing learned-router thresholds.
