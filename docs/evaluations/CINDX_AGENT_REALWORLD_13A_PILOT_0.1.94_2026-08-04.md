# Cindx Agent Real-World 13A Calibration Pilot 0.1.94

## Evidence

- Status: **CALIBRATION_NO_GO**
- Source commit: `f3e46fa74a72be304a6162c50dfddf9b75bf93b3`
- Application version: `0.1.94`
- Frozen suite: `cindx-agent-realworld-v2@2`
  (`823693985aa6ac224094590fbcaf51836e1cccf914e638619bfe115bc347f0be`)
- Raw evidence SHA-256:
  `07f5b52a7a18110bf3f0997e5e3bcdb8f96a167ddce83c166ca88fb13eec4180`
- Provider: `alibaba_cn`
  (`https://dashscope.aliyuncs.com/compatible-mode/v1`)
- Models: default/conductor/executor `qwen3.8-max`, planner `glm-5.2`,
  reviewer `deepseek-v4-flash`, summarizer `qwen3.7-flash`
- Matrix: 2 frozen cases x Fast/Auto/Pro x 1 matched replicate = 6 runs
- Per-run wall-time limit: 600 seconds
- Failures remained in the denominator. Raw prompts, model outputs, API keys,
  and complete traces remain outside Git.

This was a bounded calibration subset, not a publication run. The independent
suite analyzer correctly rejected it as an incomplete matrix, so it does not
replace the complete `0.1.82` V1 baseline.

## Results

| Treatment | Terminal completion | Quality verification | External effect | Median latency | Max latency | Tokens | Model calls | Tool calls | Recoveries |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Fast | 1/2 | 2/2 | 2/2 | 16,303 ms | 81,562 ms | 120,397 | 17 | 15 | 8 |
| Auto | 1/2 | 2/2 | 2/2 | 36,736 ms | 231,574 ms | 174,595 | 25 | 21 | 13 |
| Pro | 1/2 | 2/2 | 2/2 | 39,320 ms | 303,025 ms | 209,884 | 28 | 27 | 17 |

All three treatments completed the coding task, passed all seven deterministic
checks, and recorded no safety violation. Relative to Fast on that single case,
Auto added 20,433 ms and 4,828 tokens; Pro added 23,017 ms and 3,457 tokens.
This one observation is diagnostic only.

All three long-horizon runs made the required file changes and passed all seven
external checks, but terminated as failures after two repair attempts because
`prompt_evidence:0:external_grounding` remained unsatisfied. The failure is
therefore retained for Fast, Auto, and Pro even though the external effect was
correct. Auto used 150,012 ms and 49,370 more tokens than Fast; Pro used
221,463 ms and 86,030 more tokens than Fast on this matched case.

## Interpretation Boundary

- One replicate across two task families cannot establish intelligence uplift,
  regression, statistical significance, or Fugu parity.
- The treatments use the configured product role models, so model-role choices
  are part of the treatment and a confound for attributing effects solely to
  orchestration.
- The subset excludes browser, RAG/memory, permission denial, file-only, Direct,
  steer, recovery, cancellation, computer use, and adversarial tasks.
- The shared long-horizon failure prevents a useful Auto-versus-Pro quality
  comparison. The observed latency, token, call, and recovery amplification is
  nevertheless valid diagnostic evidence for these six runs.
- The analyzer rejection is expected for a subset and prevents this pilot from
  being presented as a complete frozen baseline.

## Decision

Do not expand directly to the 72-cell matrix and do not claim Auto or Pro
uplift. First determine why successful file and command evidence does not satisfy
the long-horizon external-grounding contract, then rerun this exact six-cell
pilot on the repaired revision. Expansion is justified only if terminal
completion converges without safety, quality, latency, or resource regression.

