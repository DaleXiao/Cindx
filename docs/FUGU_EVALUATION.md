# Cindx Fugu v1 Evaluation

This evaluation freezes the public Fugu Ultra benchmark contract from
Technical Report v1. It is designed to answer three different questions
without mixing their evidence:

1. Does Cindx match the published Fugu Ultra benchmark protocols?
2. Does adaptive orchestration beat the configured worker models and a fixed ensemble?
3. Which Cindx subsystem causes a measured gain or regression?

The frozen source is [Fugu: A Model Family for Agentic Intelligence, v1](https://arxiv.org/html/2606.21228v1), Table 1 and Appendix A. The v1 LiveCodeBench score is 92.0. Later values on the product page are not silently substituted.

## Safety boundary

`benchmarks/fugu/fugu-v1.json` is deliberately plan-only:

- It cannot run shell commands, models, downloaded code, or benchmark adapters.
- It never writes to the source workspace and does not inspect user files.
- Every real run must use an ephemeral sandbox with the source workspace mounted read-only.
- Networking is disabled unless a separately reviewed runner has an explicit destination allowlist.
- Destructive actions, privilege escalation, writes outside the sandbox, protected-path access, and symlink escape invalidate an observation.
- Generated run plans set `execution_enabled=false` and contain no executable command.

The evaluator accepts imported JSONL evidence only after its protocol fingerprint, matrix key, budget, raw-result hash, provenance, and safety attestation pass validation. It does not trust a high score from an unsafe or untraceable run.

## Frozen matrix

The parity layer contains the 11 public benchmarks reported in Fugu Technical Report v1:

- SWE-Bench Pro
- TerminalBench 2.1
- LiveCodeBench v6
- LiveCodeBench Pro
- Humanity's Last Exam
- CharXiv
- GPQA-Diamond
- SciCode
- tau3 Banking
- Long Context Reasoning
- MRCRv2

Each treatment uses three matched seeds. The quality-first and iso-budget tracks compare:

- each configured worker role independently
- a fixed, non-adaptive ensemble
- Cindx Fast
- Cindx Auto
- Cindx Pro

The causal track pairs Cindx Pro with ablations for learned routing, GEPA, durable memory, verification, four-channel retrieval, and recovery. The hindsight oracle is derived from the best single-worker result for each benchmark and seed; it is never submitted as a model run.

The manifest also records Fugu's AutoResearch, Rubik, blindfold chess, trading, Kana, and CAD probes. They are optional because some data is private or the run cost is exceptionally high. They cannot make the public parity matrix green.

## Evidence lifecycle

Run the offline contract smoke test and emit a non-executable matrix:

```bash
cargo run -p orchestrator --example fugu_evaluation_lab --locked -- \
  --run-plan target/fugu-v1-run-plan.json \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md
```

This produces 759 planned runs and a blocked report with unmeasured scores. That is expected: missing evidence is visible and never replaced by synthetic values.

After separately approved adapters have produced provider-backed observations in isolated sandboxes, validate the complete matrix:

```bash
cargo run -p orchestrator --example fugu_evaluation_lab --locked -- \
  --observations /path/to/fugu-v1-observations.jsonl \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md \
  --require-ready
```

`--require-ready` fails unless every frozen run exists, all parity observations declare Fugu v1 protocol equivalence, all evidence is provider-backed and non-synthetic, all safety attestations pass, and every iso-budget fingerprint matches.

## Observation contract

Each JSONL record uses `cindx.fugu-evaluation-observation.v1` and includes:

- exact suite version and SHA-256 protocol fingerprint
- track, benchmark, treatment, seed, and candidate identity
- native benchmark score and completion state
- wall time, TTFT, model calls, tool calls, tokens, recoveries, and safety violations
- adapter, dataset, harness commit, and raw-result SHA-256 provenance
- an explicit sandbox and network safety attestation

Failed runs remain failed observations with no fabricated score. Raw benchmark artifacts stay outside Git and are referenced only by digest.

## Reading the report

The Evaluation Card reports:

- macro-normalized score by treatment
- best global single worker
- Cindx orchestration gain over that worker
- hindsight-oracle regret
- matched win/tie/loss and Wilson lower confidence bound
- p50/p95 latency, calls, tokens, recovery, and safety telemetry
- Cindx Pro deltas against the frozen Fugu Ultra v1 historical anchors
- paired causal-ablation deltas

A historical delta is descriptive until the full protocol-equivalent track is ready. This prevents a landing-page number, different split, different judge, incomplete matrix, or unsafe run from becoming a parity claim.
