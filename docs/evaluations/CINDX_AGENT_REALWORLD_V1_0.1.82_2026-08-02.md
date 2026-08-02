# Cindx Agent Real-World Baseline 0.1.82

## Evidence

- Status: **VALID_BASELINE**
- Git commit: `4d43e770d1192da68c221368df83275cdb2b4963`
- Frozen suite: `cindx-agent-realworld-v1@1` (`f5f032d58b1e32d41ce0c0fda5db4efe7e5aa61ae5fd865a64e5da21615d53a2`)
- Raw evidence SHA-256: `ade682db5f77cf3e4d63450fecc8915668c23dffe3c21a24bae9264b2e27280f`
- Matrix: 6 cases × 4 treatments × 3 replicates
- Provider: alibaba_cn (https://dashscope.aliyuncs.com/compatible-mode/v1)

## Treatment Results

| Treatment | Complete | Quality | External effect | Safety violations | Median latency | P95 latency | Tokens | Model calls | Tool calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 100.0% | 88.9% | n/a | 0 | 12528 ms | 20317 ms | 16014 | 18 | 0 |
| fast | 94.4% | 72.2% | 72.2% | 0 | 31345 ms | 297800 ms | 1508603 | 153 | 138 |
| auto | 83.3% | 72.2% | 72.2% | 0 | 46996 ms | 600000 ms | 1229443 | 136 | 97 |
| pro | 88.9% | 72.2% | 72.2% | 0 | 51035 ms | 598520 ms | 1765259 | 184 | 146 |

## Category Quality

| Category | Direct | Fast | Auto | Pro |
| --- | ---: | ---: | ---: | ---: |
| file | 100.0% | 100.0% | 100.0% | 100.0% |
| coding | 100.0% | 33.3% | 0.0% | 33.3% |
| browser | 100.0% | 0.0% | 33.3% | 0.0% |
| long_horizon | 100.0% | 100.0% | 100.0% | 100.0% |
| rag_memory | 100.0% | 100.0% | 100.0% | 100.0% |
| permission_safety | 33.3% | 100.0% | 100.0% | 100.0% |

## Interpretation Boundary

This is a matched product baseline, not evidence of Fugu Ultra parity or causal intelligence uplift.
Raw prompts and model outputs remain outside Git; this report contains hashes, deterministic verifier results, and aggregate runtime measurements only.
