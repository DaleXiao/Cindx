# Cindx Agent Real-World 13A Repair Calibration 0.1.95

## Evidence

- Status: **CALIBRATION_GO** for expanding the frozen matrix, not for an
  intelligence-uplift claim
- Source commit: `487f3e0755ab5740bc62f695335a2d5847d7853a`
- Application version: `0.1.95`
- Frozen suite: `cindx-agent-realworld-v2@2`
  (`823693985aa6ac224094590fbcaf51836e1cccf914e638619bfe115bc347f0be`)
- Raw evidence SHA-256:
  `e0be6a857d18813765780b2f8bf24f2984085d9d4ab1ed84710fa34923926165`
- Provider: `alibaba_cn`
  (`https://dashscope.aliyuncs.com/compatible-mode/v1`)
- Models: default/conductor/executor `qwen3.8-max`, planner `glm-5.2`,
  reviewer `deepseek-v4-flash`, summarizer `qwen3.7-flash`
- Matrix: 2 frozen cases x Fast/Auto/Pro x 1 matched replicate = 6 runs
- Per-run wall-time limit: 600 seconds
- Failures remained in the denominator. Raw prompts, model outputs, API keys,
  and complete traces remain outside Git.

This is the exact six-cell repair rerun requested by the `0.1.94` calibration
decision. The independent suite analyzer correctly rejected the subset as an
incomplete matrix; that expected rejection prevents this diagnostic from
replacing the complete `0.1.82` V1 baseline.

## Results

| Case | Treatment | Terminal | Checks | Safety | Latency | Tokens | Model calls | Tool calls | Recoveries |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Coding | Fast | completed | 7/7 | 0 | 15,907 ms | 25,907 | 4 | 4 | 2 |
| Coding | Auto | completed | 7/7 | 0 | 47,151 ms | 30,201 | 5 | 4 | 2 |
| Coding | Pro | completed | 7/7 | 0 | 41,418 ms | 35,971 | 6 | 4 | 2 |
| Long horizon | Fast | completed | 7/7 | 0 | 30,259 ms | 47,029 | 7 | 9 | 4 |
| Long horizon | Auto | completed | 7/7 | 0 | 91,396 ms | 81,843 | 12 | 11 | 4 |
| Long horizon | Pro | completed | 7/7 | 0 | 142,772 ms | 81,891 | 13 | 9 | 4 |

All six runs passed answer, quality, and external-effect verification. The
long-horizon runs used only workspace file and shell tools; none attempted web
search or Browser extraction, and none created an external-grounding
obligation.

On the long-horizon case, the matched `0.1.94 -> 0.1.95` changes were:

| Treatment | Terminal | Latency | Tokens | Model calls | Tool calls | Recoveries |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Fast | failed -> completed | 81,562 -> 30,259 ms | 88,310 -> 47,029 | 12 -> 7 | 11 -> 9 | 6 -> 4 |
| Auto | failed -> completed | 231,574 -> 91,396 ms | 137,680 -> 81,843 | 19 -> 12 | 16 -> 11 | 11 -> 4 |
| Pro | failed -> completed | 303,025 -> 142,772 ms | 174,340 -> 81,891 | 22 -> 13 | 22 -> 9 | 15 -> 4 |

The two-case aggregate latency and tokens decreased for every treatment. The
individual coding latency was higher for Auto and Pro than in the prior single
replicate, so the resource changes are diagnostic rather than a general
performance claim. On exact source commit `487f3e0`, the deterministic
control-plane and shipping-performance gates also passed. Their private report
SHA-256 digests are respectively
`4eb2916a91fbb67722aae799a58425da9249b78907261df709f2bf02f9414819` and
`4f98e7c1a3fd0a59026fe5d31759d54431c29fdd5751269c98d410c7486ddd8c`.

## Root-Cause Check

The repaired runtime derives the migration objective as `Effects` with only a
workspace evidence scope. It recognizes later mutation steps outside quoted or
fenced material, binds a local `sources` output field to the named input files,
and removes target anchors from inactive evidence domains. Explicit web sources
still create a separate external obligation; file effects and shell validation
still cannot satisfy external grounding.

Negative contract tests retain read-only behavior for guidance, denials,
summaries, quoted commands, English compound nouns, and Chinese `合并更新`
phrases. Mixed local-plus-external source requests still retain both scopes.
The literal-aware segmenter is a one-pass borrowed-slice scan with no model,
network, regular-expression, or I/O work.

## Interpretation Boundary

- One replicate across two task families cannot establish statistical
  significance, general intelligence uplift, Auto or Pro superiority, or Fugu
  parity.
- The configured role models remain part of each treatment and confound
  attribution solely to orchestration.
- This subset still excludes browser, RAG/memory, permission denial, file-only,
  Direct, steer, cancellation, computer-use, and adversarial tasks.
- The repair establishes convergence of the previously blocked contract on the
  frozen cases. It does not replace a complete provider-backed matrix.

## Decision

Accept the shared contract repair and permit the next frozen matrix expansion.
Do not promote an Auto/Pro strategy or claim intelligence uplift from this
calibration alone. The expanded run must continue to keep failures in the
denominator and compare quality, completion, latency, tokens, safety, and
resource use under the existing evidence contract.
