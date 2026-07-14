# Cindx Agent Evaluation

Cindx separates deterministic harness regressions from observed answer quality. A green build proves the routing contract is stable; it does not claim that a model answer is correct.

## Versioned Contract Suite

The checked-in `benchmarks/agent/core-v1.json` suite contains 72 tasks across:

- direct general and coding questions
- coding workflows
- browser and computer use
- grounded retrieval
- ordinary research and comparison
- bounded two- and three-model collaboration
- latency-sensitive complex work

Every case specifies the expected task class, Auto policy, collaboration width, retrieval mode, and primary model role. `core-v1-baseline.json` requires a 100% Auto contract pass rate with no over- or under-orchestration.

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

Use human review or an explicitly requested judge-model run. Cindx must not spend cloud tokens on hidden background evaluation.

## Quality Regression Baselines

`observed_mode_thresholds` in a baseline can require, per mode:

- minimum observed runs
- minimum completion and quality pass rates
- maximum average latency, tokens, and cost
- maximum safety violations

The checked-in CI baseline leaves these thresholds empty because CI has no provider credentials. A controlled evaluation environment can supply its own suite, baseline, and observation files with `--suite`, `--baseline`, and `--observations`.

## Release Gate

Before release:

1. Pass the 72-case offline contract suite.
2. Run the Rust workspace and desktop tests.
3. Compare representative real observations across single-model, Fast, Auto, and Pro.
4. Investigate quality, safety, latency, token, cost, or orchestration regressions before changing routing thresholds.
