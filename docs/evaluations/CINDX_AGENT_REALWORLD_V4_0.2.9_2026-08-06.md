# Cindx Agent Real-World Baseline 0.2.9

## Evidence

- Application version: `0.2.9`
- Status: **VALID_BASELINE**
- Capture time: `2026-08-05T18:35:43.940Z`
- Git commit: `7905405551f3decd38746c45218790cb9a04be37`
- Frozen suite: `cindx-agent-realworld-v4@4` (`055da6fb9bc32af305be522f4722ddf02f554229dbf368f20776f7ad29fbb8e9`)
- Suite scope: Frozen external-effect tasks for measuring the shipping Cindx agent loop, tool harness, retrieval, memory, permissions, latency, resource use, strategy receipts, and provider response identities under a position-balanced execution protocol.
- Raw evidence SHA-256: `10ec0af3c2591fd082fac527b596243a63e742691b8ff080467be6a5cfdabc94`
- Matrix: 6 cases × 4 treatments × 3 replicates
- Execution order: cyclic_latin_square_v1 (plan `2107b3d40c2e5a746c2310478f4ce32e1c660584892b1945fac2d57355a58094`)
- Missing or structurally unverifiable cells: 0
- Provider-evidence incomplete runs: 2
- Strategy-evidence incomplete runs: 0
- Non-completed runs retained in the denominator: 8
- Provider identity: hashed_provider_id (`783601d2d3338a708fb061cc3df29e8907834c5f836496d0c741c6afc512b59a`)
- Provider endpoint: https://dashscope.aliyuncs.com/compatible-mode/v1

## Treatment Configuration and Budget

- Direct is a single-model, no-tools reference with fixture evidence supplied inline; it is not a shipping product treatment.
- Fast, Auto, and Pro execute the shipping AgentKernel, tool, retrieval, memory, and permission paths with their native effort policies.
- Every cell has the same outer process deadline of 600 seconds. Native effort budgets remain different, so this is a matched shipping-treatment baseline, not an iso-budget comparison.

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
| direct | 100.0% | 100.0% | n/a | 0 | 13419 ms | 40522 ms | 18170 | 18 | 0 |
| fast | 77.8% | 83.3% | 100.0% | 0 | 26232 ms | 61423 ms | 1010520 | 116 | 119 |
| auto | 83.3% | 100.0% | 100.0% | 0 | 53172 ms | 181046 ms | 909944 | 127 | 93 |
| pro | 94.4% | 100.0% | 100.0% | 0 | 50986 ms | 107239 ms | 812409 | 116 | 87 |

## Failure and Permission Outcomes

| Treatment | Completed | Failed | Timed out | Waiting for permission | Setup failures | Permission requests | Denied |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 18 | 0 | 0 | 0 | 0 | 0 | 0 |
| fast | 14 | 4 | 0 | 0 | 0 | 35 | 3 |
| auto | 15 | 3 | 0 | 0 | 0 | 42 | 3 |
| pro | 17 | 1 | 0 | 0 | 0 | 38 | 3 |

Total: 64 completed, 8 non-completed, 0 timed out, 0 waiting for permission, and 0 setup failures.

## Category Quality

| Category | Direct | Fast | Auto | Pro |
| --- | ---: | ---: | ---: | ---: |
| file | 100.0% | 100.0% | 100.0% | 100.0% |
| coding | 100.0% | 100.0% | 100.0% | 100.0% |
| browser | 100.0% | 100.0% | 100.0% | 100.0% |
| long_horizon | 100.0% | 100.0% | 100.0% | 100.0% |
| rag_memory | 100.0% | 0.0% | 100.0% | 100.0% |
| permission_safety | 100.0% | 100.0% | 100.0% | 100.0% |

## Paired Against Fast

| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |
| --- | ---: | ---: | ---: | ---: |
| auto | 18 | +16.7 pp | +5.6 pp | +26488 ms |
| pro | 18 | +16.7 pp | +16.7 pp | +21986 ms |

## Decisions

- Baseline validity: **VALID_BASELINE**
- Broad orchestration uplift: **NO-GO**
- Uplift rationale: At least one preregistered promotion_v1 gate failed; failures remain in the denominator and no uplift is authorized.
- Learned-profile evidence: **FRESH-SEED-ONLY**
- Required improvement: 1/18

| Candidate | Quality delta runs | Completion delta runs | Median latency ratio / max | Total token ratio / max | Eligible |
| --- | ---: | ---: | ---: | ---: | --- |
| auto | 3 | 1 | 2.026989935956084 / 3 | 0.9004710446106955 / 4 | yes |
| pro | 3 | 3 | 1.9436566026227509 / 6 | 0.8039514309464434 / 8 | yes |

## Confounds

- Cells ran serially in a fixed cyclic Latin-square order against one provider and one configured role-model set; the order balances positions but provider and browser conditions can still vary over time.
- The suite contains one fixture per category with three repeats, so category estimates are narrow.
- Direct receives inline fixture evidence and no tools, so it is a reference ceiling rather than an equal product treatment.
- The 600-second process deadline is matched, but Fast, Auto, and Pro retain different shipping budgets; timed-out cells can under-report token, call, and resource totals.
- No frozen learned artifact was supplied. These results are fresh-seed quality evidence and cannot be attributed to GEPA, transfer, or self-distillation.

## Interpretation Boundary

This matrix evaluates fresh-seed product quality only. It provides no evidence of GEPA learning, distillation, learned-profile uplift, or Fugu parity.
Raw prompts and model outputs remain outside Git; this report contains hashes, deterministic verifier results, and aggregate runtime measurements only.
