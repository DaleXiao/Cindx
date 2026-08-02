# Cindx Agent Real-World Evaluation 0.1.82

## Evidence

- Status: **INVALID_BASELINE**
- Git commit: `6b2ee39baba3aa8b9812d997a0863c8752ebdcb3`
- Frozen suite: `cindx-agent-realworld-v2@2` (`823693985aa6ac224094590fbcaf51836e1cccf914e638619bfe115bc347f0be`)
- Raw evidence SHA-256: `9c5904c14f6fdaa0fa6dfca7f904aadd03d26a63a758c2bb086ea8c748da2d2c`
- Matrix: 6 cases × 4 treatments × 3 replicates
- Incomplete or unverified runs: 4
- Provider: alibaba_cn (https://dashscope.aliyuncs.com/compatible-mode/v1)

## Treatment Results

| Treatment | Complete | Quality | External effect | Safety violations | Median latency | P95 latency | Tokens | Model calls | Tool calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| direct | 100.0% | 88.9% | n/a | 0 | 12472 ms | 25090 ms | 16384 | 18 | 0 |
| fast | 77.8% | 72.2% | 72.2% | 1 | 35075 ms | 259642 ms | 1365456 | 163 | 148 |
| auto | 55.6% | 77.8% | 87.5% | 0 | 46418 ms | 388223 ms | 1401572 | 184 | 144 |
| pro | 50.0% | 66.7% | 75.0% | 0 | 60980 ms | 556145 ms | 1841418 | 224 | 196 |

## Category Quality

| Category | Direct | Fast | Auto | Pro |
| --- | ---: | ---: | ---: | ---: |
| file | 100.0% | 100.0% | 100.0% | 66.7% |
| coding | 100.0% | 66.7% | 100.0% | 100.0% |
| browser | 100.0% | 0.0% | 33.3% | 0.0% |
| long_horizon | 100.0% | 100.0% | 100.0% | 100.0% |
| rag_memory | 100.0% | 100.0% | 33.3% | 33.3% |
| permission_safety | 33.3% | 66.7% | 100.0% | 100.0% |

## Interpretation Boundary

Infrastructure failures make this matrix invalid for capability promotion or treatment comparison.
Raw prompts and model outputs remain outside Git; this report contains hashes, deterministic verifier results, and aggregate runtime measurements only.
