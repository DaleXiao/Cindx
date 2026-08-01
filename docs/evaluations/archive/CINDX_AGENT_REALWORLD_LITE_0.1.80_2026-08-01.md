# Cindx Agent Real-World Lite Evaluation

- App: `0.1.80`
- Commit: `7cd6eb1fc3210726832f75a8c485361ed2461c8b`
- Captured: `2026-08-01`
- Decision: `NO-GO` for an Agent-intelligence uplift claim

## Question

GPQA mainly measures difficult question answering. This pilot instead asks whether the
product can inspect a workspace, use bounded tools, combine evidence, resolve a
contradiction, and return the best result produced by its workflow.

## Protocol

The first pass executed 12 live-provider runs: three deterministic temporary workspace
tasks across Direct, Fast, Auto, and Pro. Five incomplete cells received one targeted
rerun. The tasks covered:

1. exact single-file evidence retrieval;
2. synthesis across three files, including one distracting draft;
3. resolution of conflicting claims using the authoritative runtime file.

Cindx modes received only read-only workspace tools. Direct received the same evidence
inline, so Direct is an answer-quality ceiling, not a fair tool-using Agent baseline.
Models also differ by treatment. Results must therefore not be attributed solely to
orchestration.

The frozen runtime reported `auto_gepa=false` and `pro_gepa=false`. This run does not
measure an evolved GEPA profile.

## First-Pass External Effect

| Treatment | Delivered | Complete | Mean score | Median latency | Tokens | Tool calls | Safety violations |
|---|---:|---:|---:|---:|---:|---:|---:|
| Direct | 3/3 | 3/3 | 1.000 | 8.8s | 1,562 | 0 | 0 |
| Fast | 3/3 | 0/3 | 0.250 | 6.9s | 26,075 | 10 | 0 |
| Auto | 3/3 | 2/3 | 0.750 | 59.3s | 77,567 | 17 | 0 |
| Pro | 3/3 | 2/3 | 0.833 | 91.8s | 109,308 | 24 | 0 |

All modes returned a visible response, but delivery alone concealed incomplete and stale
answers. Auto and Pro completed the multi-file synthesis that Fast missed, which is a
directional benefit for complex evidence gathering. Neither mode exceeded the Direct
ceiling, and both paid a large latency and token penalty.

## Targeted Rerun

| Case | Treatment | First pass | Rerun | Classification |
|---|---|---:|---:|---|
| Single-file retrieval | Fast | 0.50 | 1.00 | Recovered variance |
| Single-file retrieval | Pro | 0.50 | 1.00 | Recovered variance |
| Multi-file synthesis | Fast | 0.00 | 0.00 | Persistent harness failure |
| Contradiction resolution | Fast | 0.25 | 0.50 | Persistent incomplete result |
| Contradiction resolution | Auto | 0.25 | 0.50 | Persistent result-selection failure |

## Causal Findings

### 1. Fast exhausts the evidence stage before reading decisive files

In both multi-file attempts Fast successfully listed the root, `release`, and `notes`,
then reached finalization without a remaining evidence-capable turn. It returned a claim
that no file-reading tools were available. This is a worker budget/finalization defect,
not a missing provider capability.

### 2. Auto can produce the right answer and still deliver the wrong one

In both contradiction attempts the reviewer produced the correct port, authoritative
file, and stale-source explanation. The run-level final output instead selected an older
unsupported response claiming that workspace access was unavailable. The result frontier
or terminal selection is discarding a stronger downstream result.

### 3. Tool state is not represented consistently to the worker

Several final responses claimed that no tools or workspace access existed after successful
`file.read`, `file.read_many`, or `file.search` calls had already returned evidence. Tool
execution itself worked; observation continuity and finalization did not.

### 4. Current collaboration is not economically or operationally dominant

Auto and Pro can rescue multi-file work, but they use roughly 3x and 4x the Fast tokens
and have much longer critical paths. The conductor is still over-routing simple retrieval
into `plan_execute_review` or `best_of_n`, while no promoted GEPA profile is active.

## Memory Gate

The deterministic `core-memory` v5 suite passed:

- top-1: `18/18`;
- recall@3: `18/18`;
- trust, deduplication, supersession, false-positive, false-persistence, verbatim-evidence,
  semantic-laundering, and security failures: `0`;
- average local recall: `185 us`; maximum: `441 us`.

This validates the frozen retrieval contract. It does not prove that live Agent responses
consistently request, receive, and use the right memory.

## Runtime Reliability Gate

`agent-runtime` completed `211` tests with `0` failures; one performance diagnostic was
explicitly ignored. The passing tests cover steering, cancellation, budgets, finalizer
reserves, context compaction, task contracts, recovery, parallel workers, and tool
admission. This is strong control-plane evidence, but the live pilot shows that correct
components can still compose into an incorrect delivered result.

## Interpretation

GPQA is useful for controlling worker-model reasoning quality, but it is insufficient as
the primary Agent benchmark. This pilot exposed defects that GPQA cannot see: evidence-turn
allocation, tool-observation continuity, workflow dependency handling, result selection,
and latency amplification.

The sample is intentionally small and only covers read-only workspaces. It cannot support
a broad ranking, a GEPA claim, or Fugu Ultra parity. It is sufficient to reject an A-grade
Agent claim for `0.1.80` and to prioritize two production defects before spending on a
larger benchmark:

1. preserve an evidence-capable turn before finalization;
2. select the strongest grounded downstream result rather than stale fallback text.

## Next Evaluation Gate

After those defects are fixed, the next suite should use a tool-enabled single-agent
baseline and equal budgets across treatments. A minimal matched matrix should include
code edit plus tests, structured file mutation, local browser tasks, interruption/resume,
user steering, and cross-session memory. Each case needs deterministic verification and
at least three matched repeats. Direct inline evidence should no longer serve as the
primary baseline.

## Evidence

Raw prompts, outputs, and tool responses remain outside Git.

| Artifact | SHA-256 |
|---|---|
| First-pass raw evidence | `f8b2d431f5ec0e434cb09f45117ce1d29f45c69f7fa173047f837e14a74e84d2` |
| Targeted rerun raw evidence | `febbbd93bbe756bd6114819b7c2cead4597469f5c3729b4d44c8ea4b19971962` |
| Memory report | `25a64b9e46cee22ea1ec888530c0e0089e1c6c35b200ad8bbcb78dc8fa6ee17d` |

The evaluation build produced `6.2 GB` of Rust artifacts after a clean workspace. That
cost is an additional engineering signal, not an Agent-quality metric.
