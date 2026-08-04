# Cindx Agent Real-World Baseline 0.1.98

## Evidence

- Application version: `0.1.98`
- Status: **VALID_BASELINE**
- Capture time: `2026-08-04T10:07:54.525Z`
- Git commit: `4fc736cdfd0b8eb85ffee0a9ef5dfaea4b05e469`
- Frozen suite: `cindx-agent-realworld-v2@2` (`823693985aa6ac224094590fbcaf51836e1cccf914e638619bfe115bc347f0be`)
- Suite scope: Frozen external-effect tasks for measuring the shipping Cindx agent loop, tool harness, retrieval, memory, permissions, latency, and resource use. V2 recognizes file.read_many as equivalent read evidence for the coding task; task objectives and all other success conditions are unchanged.
- Raw evidence SHA-256: `ce924292361cde92afba0e88829b0120684a8a4329c69de996375f5bfa1f26cb`
- Matrix: 6 cases × 4 treatments × 3 replicates
- Missing or structurally unverifiable cells: 0
- Non-completed runs retained in the denominator: 15
- Provider: alibaba_cn (https://dashscope.aliyuncs.com/compatible-mode/v1)

## Treatment Configuration and Budget

- Direct is a single-model, no-tools reference with fixture evidence supplied inline; it is not a shipping product treatment.
- Fast, Auto, and Pro execute the shipping AgentKernel, tool, retrieval, memory, and permission paths with their native effort policies.
- Every cell has the same outer process deadline of 600 seconds. Native effort budgets remain different, so this is a matched shipping-treatment baseline, not an iso-budget comparison.

The native limits below are taken from `RunBudget::for_effort` at evaluated
commit `4fc736cdfd0b8eb85ffee0a9ef5dfaea4b05e469`. They are per Agent control;
the evaluation driver's 600-second process deadline remains the cell-level cap.

| Treatment | Native control duration | Maximum model calls | Maximum tool calls | Evaluation cell deadline |
| --- | ---: | ---: | ---: | ---: |
| direct | one logical request under the Fast control | 1 observed logical call | 0 | 600 s |
| fast | 300 s | 12 | 24 | 600 s |
| auto | 2700 s | 72 | 144 | 600 s |
| pro | 14400 s | 384 | 768 | 600 s |

| Model role | Configured model |
| --- | --- |
| conductor | qwen3.8-max |
| default | qwen3.8-max |
| embedding | qwen3.7-text-embedding |
| executor | qwen3.8-max |
| planner | glm-5.2 |
| reviewer | deepseek-v4-flash |
| summarizer | qwen3.7-flash |

## Treatment Results

| Treatment | Complete | Quality | External effect | Safety violations | Median latency | P95 latency | Tokens | Model calls | Tool calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 100.0% | 100.0% | n/a | 0 | 15062 ms | 44246 ms | 18742 | 18 | 0 |
| fast | 88.9% | 77.8% | 94.4% | 0 | 31576 ms | 503612 ms | 1514908 | 165 | 182 |
| auto | 66.7% | 88.9% | 88.9% | 0 | 58266 ms | 600000 ms | 1019780 | 141 | 123 |
| pro | 61.1% | 83.3% | 83.3% | 0 | 66013 ms | 600000 ms | 594905 | 95 | 64 |

## Failure and Permission Outcomes

| Treatment | Completed | Failed | Timed out | Waiting for permission | Setup failures | Permission requests | Denied |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 18 | 0 | 0 | 0 | 0 | 0 | 0 |
| fast | 16 | 1 | 0 | 1 | 0 | 68 | 3 |
| auto | 12 | 5 | 1 | 0 | 0 | 44 | 6 |
| pro | 11 | 4 | 3 | 0 | 0 | 28 | 6 |

Total: 57 completed, 15 non-completed, 4 timed out, 1 waiting for permission, and 0 setup failures.

## Category Quality

| Category | Direct | Fast | Auto | Pro |
| --- | ---: | ---: | ---: | ---: |
| file | 100.0% | 100.0% | 100.0% | 100.0% |
| coding | 100.0% | 100.0% | 100.0% | 100.0% |
| browser | 100.0% | 66.7% | 33.3% | 0.0% |
| long_horizon | 100.0% | 100.0% | 100.0% | 100.0% |
| rag_memory | 100.0% | 0.0% | 100.0% | 100.0% |
| permission_safety | 100.0% | 100.0% | 100.0% | 100.0% |

## Paired Against Fast

| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |
| --- | ---: | ---: | ---: | ---: |
| auto | 18 | +11.1 pp | -22.2 pp | +24663 ms |
| pro | 18 | +5.6 pp | -27.8 pp | +41396 ms |

## Decisions

- Baseline validity: **VALID_BASELINE**
- Broad orchestration uplift: **NO-GO**
- Uplift rationale: This descriptive V2 baseline has no preregistered promotion threshold and cannot authorize a broad orchestration-uplift claim.

## Confounds

- Cells ran serially in one frozen order against one provider and one configured role-model set; provider and browser conditions can vary over time.
- The suite contains one fixture per category with three repeats, so category estimates are narrow.
- Direct receives inline fixture evidence and no tools, so it is a reference ceiling rather than an equal product treatment.
- The 600-second process deadline is matched, but Fast, Auto, and Pro retain different shipping budgets; timed-out cells can under-report token, call, and resource totals.
- Raw v1 evidence does not capture learned profile or GEPA identities, so results cannot be attributed to GEPA, transfer, or self-distillation.

## Interpretation Boundary

This is a matched product baseline, not evidence of Fugu Ultra parity or causal intelligence uplift.
Raw prompts and model outputs remain outside Git; this report contains hashes, deterministic verifier results, and aggregate runtime measurements only.
