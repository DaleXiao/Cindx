# Cindx Fugu v1 Evaluation Protocol

Status: frozen comparison protocol, **not** a parity result.

This evaluation freezes the public Fugu Ultra benchmark contract from
Technical Report v1. It is designed to answer three different questions
without mixing their evidence:

1. Does Cindx match the published Fugu Ultra benchmark protocols?
2. Does adaptive orchestration beat the configured worker models and a fixed ensemble?
3. Which Cindx subsystem causes a measured gain or regression?

The first provider-backed internal mechanism report is archived as
[`Cindx Scientific Evaluation Card: Pilot v1`](evaluations/archive/2026-07/CINDX_SCIENTIFIC_PILOT_V1.md).
It validates bounded read-only evidence grounding but deliberately does not
claim completion of the external Fugu matrix described below.

The first provider-backed external-effect pilot is archived as
[`Cindx Fugu External Effect Pilot V1`](evaluations/archive/2026-07/CINDX_FUGU_EXTERNAL_EFFECT_PILOT_V1.md).
It applies fixed GPQA-Diamond and MRCR v2 samples to the configured direct,
Auto, and Pro paths, preserves failures in the denominator, and publishes only
sanitized hashes, scores, latency, and routing evidence.

The latest provider-backed product baseline is
[`Cindx Agent Real-World V2 0.1.98`](evaluations/CINDX_AGENT_REALWORLD_V2_0.1.98_2026-08-04.md).
It is a complete `VALID_BASELINE` with zero setup failures and zero safety
violations, but it is not protocol-equivalent to the external matrix below.
Auto and Pro gained quality over Fast only with materially lower completion and
higher latency, so the Agent-intelligence uplift decision remains `NO-GO`.
The raw schema did not capture learned profile or GEPA identities, preventing
any causal claim about GEPA, transfer, or self-distillation.

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
cargo run -p orchestrator-eval --example fugu_evaluation_lab --locked -- \
  --run-plan target/fugu-v1-run-plan.json \
  --report target/fugu-v1-report.json \
  --card target/fugu-v1-evaluation-card.md
```

This produces 759 planned runs and a blocked report with unmeasured scores. That is expected: missing evidence is visible and never replaced by synthetic values.

After separately approved adapters have produced provider-backed observations in isolated sandboxes, validate the complete matrix:

```bash
cargo run -p orchestrator-eval --example fugu_evaluation_lab --locked -- \
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
