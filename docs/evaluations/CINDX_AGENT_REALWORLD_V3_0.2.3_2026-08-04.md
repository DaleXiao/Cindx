# Cindx Agent Real-World Baseline 0.2.3

## Evidence

- Application version: `0.2.3`
- Status: **VALID_BASELINE**
- Capture time: `2026-08-04T23:05:17.637Z`
- Git commit: `af5025f46137096c34bd5dd4f70a89657713fde4`
- Frozen suite: `cindx-agent-realworld-v3@3` (`5f3df3ba3a5b250d7254e300d9dd9ff81d0527d1870bbe29df878b5ac0ad30bb`)
- Suite scope: Frozen external-effect tasks for measuring the shipping Cindx agent loop, tool harness, retrieval, memory, permissions, latency, resource use, strategy receipts, and provider response identities under a position-balanced execution protocol.
- Raw evidence SHA-256: `9784dae7d5eb8df2b2226a2a82e4ef2ba13bc6d467d08cf3a6b876c5020c23c8`
- Matrix: 6 cases × 4 treatments × 3 replicates
- Execution order: cyclic_latin_square_v1 (plan `4753b2b827752e79e4dd1eab6d03d3e804bc5c19a6ec07d165b1dae737c5977b`)
- Missing or structurally unverifiable cells: 0
- Provider-evidence incomplete runs: 5
- Strategy-evidence incomplete runs: 5
- Non-completed runs retained in the denominator: 16
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
| direct | 100.0% | 100.0% | n/a | 0 | 11905 ms | 38627 ms | 18167 | 18 | 0 |
| fast | 77.8% | 72.2% | 88.9% | 0 | 23195 ms | 600000 ms | 1161554 | 136 | 164 |
| auto | 66.7% | 83.3% | 83.3% | 0 | 60336 ms | 600000 ms | 1104531 | 152 | 125 |
| pro | 66.7% | 83.3% | 83.3% | 0 | 61854 ms | 600000 ms | 637473 | 101 | 64 |

## Failure and Permission Outcomes

| Treatment | Completed | Failed | Timed out | Waiting for permission | Setup failures | Permission requests | Denied |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 18 | 0 | 0 | 0 | 0 | 0 | 0 |
| fast | 14 | 2 | 1 | 1 | 0 | 55 | 3 |
| auto | 12 | 5 | 1 | 0 | 0 | 52 | 7 |
| pro | 12 | 3 | 3 | 0 | 0 | 27 | 7 |

Total: 56 completed, 16 non-completed, 5 timed out, 1 waiting for permission, and 0 setup failures.

## Category Quality

| Category | Direct | Fast | Auto | Pro |
| --- | ---: | ---: | ---: | ---: |
| file | 100.0% | 66.7% | 100.0% | 100.0% |
| coding | 100.0% | 100.0% | 100.0% | 100.0% |
| browser | 100.0% | 66.7% | 0.0% | 0.0% |
| long_horizon | 100.0% | 100.0% | 100.0% | 100.0% |
| rag_memory | 100.0% | 0.0% | 100.0% | 100.0% |
| permission_safety | 100.0% | 100.0% | 100.0% | 100.0% |

## Paired Against Fast

| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |
| --- | ---: | ---: | ---: | ---: |
| auto | 18 | +11.1 pp | -11.1 pp | +28163 ms |
| pro | 18 | +11.1 pp | -11.1 pp | +35744 ms |

## Decisions

- Baseline validity: **VALID_BASELINE**
- Broad orchestration uplift: **NO-GO**
- Uplift rationale: At least one preregistered promotion_v1 gate failed; failures remain in the denominator and no uplift is authorized.
- Learned-profile evidence: **FRESH-SEED-ONLY**
- Required improvement: 1/18

| Candidate | Quality delta runs | Completion delta runs | Median latency ratio / max | Total token ratio / max | Eligible |
| --- | ---: | ---: | ---: | ---: | --- |
| auto | 2 | -2 | 2.601250269454624 / 3 | 0.9509080077206914 / 4 | no |
| pro | 2 | -2 | 2.6666954084932097 / 6 | 0.548810472866522 / 8 | no |

## Confounds

- Cells ran serially in a fixed cyclic Latin-square order against one provider and one configured role-model set; the order balances positions but provider and browser conditions can still vary over time.
- The suite contains one fixture per category with three repeats, so category estimates are narrow.
- Direct receives inline fixture evidence and no tools, so it is a reference ceiling rather than an equal product treatment.
- The 600-second process deadline is matched, but Fast, Auto, and Pro retain different shipping budgets; timed-out cells can under-report token, call, and resource totals.
- No frozen learned artifact was supplied. These results are fresh-seed quality evidence and cannot be attributed to GEPA, transfer, or self-distillation.

## Interpretation Boundary

This matrix evaluates fresh-seed product quality only. It provides no evidence of GEPA learning, distillation, learned-profile uplift, or Fugu parity.
Raw prompts and model outputs remain outside Git; this report contains hashes, deterministic verifier results, and aggregate runtime measurements only.
