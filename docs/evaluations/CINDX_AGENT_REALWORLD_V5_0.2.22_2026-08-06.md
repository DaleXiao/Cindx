# Cindx Agent Real-World Baseline 0.2.22

## Evidence

- Status: **VALID_BASELINE**
- Application version: `0.2.22`
- Git commit: `a0000fa55907b90adf6684b39aa3b9d5cc679f42`
- Frozen suite: `cindx-agent-realworld-v5@5` (`195c854fdb5cd3416e1191eb2c54ce0ab9ddd4988afd8561749bcf9ff643b31b`)
- Matrix: 6 cases × 4 treatments × 3 replicates
- Execution plan: `418fc6d28a6c6029bb4f3c7cd6c354b90fac968516be85b74565cccd2485f65c`
- Raw evidence SHA-256: `8c39922d7e8d1298562e3553e8598ca9822a02f89e8d6abf724d02d3b872b521`
- Missing or structurally unverifiable cells: 0
- Provider-evidence incomplete runs: 0
- Strategy-evidence incomplete runs: 0
- Non-completed runs retained in the denominator: 11

## Treatment Contract

- `oracle_reference` is a no-tools reference ceiling and is not a product baseline.
- `grounded_direct` runs the shipping AgentKernel with Auto's budget, model routing, tools, retrieval, memory, permissions, and external postcondition verifier, but clamps collaboration to direct execution. Independent worker verification is therefore normalized to self-check.
- `auto` is the iso-budget adaptive candidate. `pro` remains descriptive because its native budget differs.

| Treatment | Complete | Quality | External effect | Safety | Median latency | Tokens | Model calls | Tool calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| oracle_reference | 100.0% | 100.0% | n/a | 0 | 17381 ms | 18692 | 18 | 0 |
| grounded_direct | 77.8% | 77.8% | 77.8% | 0 | 71322 ms | 1129066 | 134 | 108 |
| auto | 77.8% | 83.3% | 83.3% | 0 | 64585 ms | 1016265 | 122 | 88 |
| pro | 83.3% | 83.3% | 83.3% | 0 | 60165 ms | 1083760 | 129 | 102 |

## Mechanism Claims

| Dimension | State | Evidence complete | Pairs / denominator | Quality delta runs | Completion delta runs | Reason |
| --- | --- | --- | ---: | ---: | ---: | --- |
| adaptive_direct | IMPROVED | yes | 18 / 18 | +1 | +0 | claim_contract_passed |
| workflow | NOT_EXERCISED | no | 0 / 0 | +0 | +0 | workflow_not_exercised |
| learned_profile | NOT_EXERCISED | no | 0 / 0 | +0 | +0 | exact_stable_parent_not_exercised |
| distillation | NOT_EXERCISED | no | 0 / 0 | +0 | +0 | exact_stable_parent_not_exercised |

## Paired Against Grounded Direct

| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |
| --- | ---: | ---: | ---: | ---: |
| auto | 18 | +5.6 pp | +0.0 pp | -3585 ms |
| pro | 18 | +5.6 pp | +5.6 pp | -137 ms |

## Interpretation Boundary

Adaptive-direct and workflow conclusions are limited to complete iso-budget GroundedDirect/Auto pairs. Learned-profile and distillation uplift require an actually executed exact stable parent and remain NOT_EXERCISED here. Pro is descriptive because its native budget differs.
Failures, timeouts, and denials remain in the denominator. Deterministic checks validate the mechanism only; intelligence conclusions require this provider-backed paired evidence.
Raw prompts and model outputs remain outside Git.
