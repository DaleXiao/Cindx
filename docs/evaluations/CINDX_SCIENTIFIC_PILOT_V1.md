# Cindx Scientific Evaluation Card: Pilot v1

**Captured:** 2026-07-23  
**Source commit:** `15b936bc69c6d24088e38ef1cf15b386a5e2ffb3`  
**Cindx desktop package:** `0.1.25`  
**Status:** internal mechanism gate passed; external Fugu parity not measured

## Executive result

Cindx completed its first provider-backed, paired, reproducible hidden-test gate.
Across 30 runtime-randomized cases and three matched repeats, the configured
executor failed all 90 runs when workspace evidence tools were unavailable and
passed all 90 runs when the same harness exposed only its existing read-only
evidence tools.

| Treatment | Verified | Rate | Mean latency | p50 | p95 | Max | Safety violations |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| No evaluation tools | 0 / 90 | 0% | 9.011 s | 8.320 s | 15.495 s | 27.369 s | 0 |
| Read-only evidence sandbox | 90 / 90 | 100% | 3.712 s | 3.587 s | 4.491 s | 10.220 s | 0 |

The candidate won all 90 matched comparisons. Its two-sided 95% Wilson lower
confidence bound is `0.9591`, above the frozen `0.50` promotion threshold. All
seven frozen gates passed and the evaluator recommended a 10% canary.

This is strong causal evidence for one narrow claim: **Cindx's bounded,
read-only tool-grounding path reliably retrieves unpredictable workspace
evidence that the same model cannot know without the tool.** It is not a broad
coding-quality score and is not a Fugu Ultra parity result.

## Question and hypothesis

The experiment asks whether the Cindx harness, rather than model prior
knowledge, causes successful recovery of required workspace evidence.

- Null hypothesis: exposing the bounded read-only evidence path does not
  improve deterministic task success over the same worker without tools.
- Alternative hypothesis: the evidence path improves success without adding a
  critical safety violation or category regression.

The task stores one newly generated verification code in a temporary file. The
code is not present in the prompt and is different for every case, preventing
memorization or benchmark leakage.

## Frozen protocol

- Suite: `core-agent-quality` v2, hidden `test` split.
- Cases: 30 unique runtime-random facts.
- Categories: 10 coding, 10 research, 10 tool-use.
- Repeats: three matched seeds per case.
- Treatments: `pre-gepa-no-evaluation-tools` and
  `gepa-read-only-sandbox`.
- Total: 90 paired comparisons and 180 workflow runs.
- Worker: configured executor model `glm-5.2` for both treatments.
- Concurrency: four workers.
- Verifier: exact, case-sensitive deterministic containment check.
- Candidate fingerprint:
  `12942bcf98394fc1c9a11e1cffc353a44cace758674813be7f540bec25a2ba7e`.
- Dataset fingerprint:
  `b0143ef9dd8432e6767630f738deefbed6303265735dbab5691902bcd2592ad1`.

Despite its inherited identifier, the candidate in this gate is a static seed
genome with read-only evidence enabled. The main experiment therefore isolates
tool grounding; it does not measure a learned GEPA population against its
parent.

## Safety boundary

Every case ran in a newly created temporary workspace containing only its
random evidence file. The source repository was not used as the task
workspace. The runner exposed no write, process execution, network, sensitive,
destructive, privilege, computer-use, or shell tool.

The candidate budget was bounded to one workflow step, one model, at most three
model turns, at most four read-only tool calls, and at most 2,048 output tokens
per step. Provider networking was the only network path. Temporary workspaces
were removed after the run.

Before the full gate, two prerequisite checks passed:

1. Offline registry inspection and a real temporary-file read proved that only
   read-only tools were exposed.
2. A one-case provider-backed paired smoke proved that the deterministic
   verifier rejected the no-tool answer and accepted a `file.read`-grounded
   answer.

No run recorded a safety violation. This statement is scoped to the enforced
evaluation policy; it is not an adversarial operating-system sandbox audit.

## Promotion statistics

| Measure | Result |
| --- | ---: |
| Paired comparisons | 90 |
| Candidate wins / ties / losses | 90 / 0 / 0 |
| Pairwise score | 1.000 |
| Wilson lower bound | 0.9591 |
| Absolute success gain | +1.000 |
| Average score gain | +1.000 |
| Maximum category regression | 0.000 |
| Recommended canary | 10% |

Every category moved from 0% to 100%. The candidate's observed mean latency was
58.8% lower than the baseline. Latency is descriptive: calls were concurrent
and used a live provider, so this pilot does not claim an isolated serving
speedup.

## GEPA mechanism smoke

A separate provider-backed smoke supplied the configured conductor model
`qwen3.7-max` with a synthetic failure packet whose parent genome had disabled
the required evidence tool. The reflected mutation enabled a non-disabled tool
policy, restored a positive tool-call budget, and passed genome validation.

That result demonstrates that the reflection-and-repair path is executable. It
does **not** establish population-level improvement, Pareto-front quality,
holdout generalization, or convergence. Those require a separate longitudinal
GEPA study.

## Fugu Ultra status

The frozen Fugu v1 plan contains 759 protocol-equivalent runs across 11 public
benchmarks and three tracks. No external benchmark adapter was executed in this
pilot:

| Track | Observed / required | Ready |
| --- | ---: | --- |
| Quality-first parity | 0 / 264 | No |
| Iso-budget parity | 0 / 264 | No |
| Causal ablations | 0 / 231 | No |

Accordingly, `ready_for_scientific_comparison=false`. The pilot must not be
quoted as a Fugu parity score, a frontier-model score, or a broad coding-agent
ranking.

## Telemetry gaps and limitations

- All 180 score records reported `total_tokens=0`; provider usage telemetry was
  not propagated into this evaluation path. Token efficiency and cost are
  unmeasured.
- The 30 cases use one task family: exact retrieval of a hidden file value.
  Category labels vary, but the underlying causal mechanism is the same.
- Both treatments use one configured executor model. Cross-model robustness is
  unmeasured.
- The no-tool baseline is intentionally impossible, so the extreme effect size
  is expected and should not be generalized to normal coding tasks.
- Raw model text and complete tool traces are not retained in the v2 score set;
  auditability currently relies on deterministic verifier records, run
  identity, fingerprints, and the runner implementation.
- Provider version and sampling parameters are not independently fingerprinted.
- No prompt-injection, symlink-escape, long-horizon, memory, browser,
  computer-use, or recovery stress suite was part of this pilot.

## Reproduction and independent verification

The full provider-backed gate was run with:

```bash
CINDX_EVAL_CONCURRENCY=4 cargo test \
  tests::provider_backed_hidden_gate_compares_pre_gepa_and_read_only_sandbox \
  -- --ignored --exact --nocapture
```

The frozen evaluator then independently parsed both score sets, rebuilt the
promotion report, and required promotion eligibility. The rebuilt JSON was
byte-identical to the runner's report.

Evidence is stored under
`docs/evaluations/evidence/2026-07-23-pilot-v1/` with these SHA-256 digests:

| Artifact | SHA-256 |
| --- | --- |
| `provider-test.json` | `b0143ef9dd8432e6767630f738deefbed6303265735dbab5691902bcd2592ad1` |
| `provider-baseline-scores.json` | `edc28e711030f55830b95bc4a41a1faa6a2fd6795b2afbf19194b09564e6853b` |
| `provider-candidate-scores.json` | `eb941198464704dd9b3ca9dfd7d78ad2d45e0cfb167f7947e447d9c4b3469cb9` |
| `evaluation-v2-provider-promotion.json` | `095a981ec698aa403c5e5e87c8e5bd17af7cbc4cc63c4fd9af9b283e449bf611` |

## Decision

The internal read-only evidence treatment is eligible for a bounded 10% canary
under the frozen v2 gate. Broad quality, GEPA learning, and Fugu Ultra parity
remain unmeasured. The next scientific stage should fix token and raw-trace
telemetry, then add protocol-equivalent public benchmark adapters and matched
router/GEPA/memory/recovery ablations.
