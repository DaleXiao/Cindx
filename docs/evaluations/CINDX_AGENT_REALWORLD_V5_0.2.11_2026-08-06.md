# Cindx Agent Real-World Baseline 0.2.11

## Evidence

- Status: **VALID_BASELINE**
- Application version: `0.2.11`
- Git commit: `3765d23042dcafaeb721accc0460f149e1ea5ade`
- Frozen suite: `cindx-agent-realworld-v5@5` (`195c854fdb5cd3416e1191eb2c54ce0ab9ddd4988afd8561749bcf9ff643b31b`)
- Matrix: 6 cases × 4 treatments × 3 replicates
- Execution plan: `4ce53173082ddb3c0acbe309c5bc3960eba0dc7a30f4a4af177b37d6836d56de`
- Raw evidence SHA-256: `60249acaf5ae989f134a698709d94eb5a8daa09344ef6dfd18c5472901d417b9`
- Missing or structurally unverifiable cells: 0
- Provider-evidence incomplete runs: 0
- Strategy-evidence incomplete runs: 0
- Non-completed runs retained in the denominator: 8

## Treatment Contract

- `oracle_reference` is a no-tools reference ceiling and is not a product baseline.
- `grounded_direct` runs the shipping AgentKernel with Auto's budget, model routing, tools, retrieval, memory, permissions, and external postcondition verifier, but clamps collaboration to direct execution. Independent worker verification is therefore normalized to self-check.
- `auto` is the iso-budget adaptive candidate. `pro` remains descriptive because its native budget differs.

| Treatment | Complete | Quality | External effect | Safety | Median latency | Tokens | Model calls | Tool calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| oracle_reference | 100.0% | 100.0% | n/a | 0 | 12921 ms | 16971 | 18 | 0 |
| grounded_direct | 83.3% | 100.0% | 100.0% | 0 | 56470 ms | 924040 | 130 | 93 |
| auto | 83.3% | 100.0% | 100.0% | 0 | 44952 ms | 940757 | 131 | 105 |
| pro | 88.9% | 100.0% | 100.0% | 0 | 50730 ms | 876783 | 127 | 97 |

## Mechanism Claims

| Dimension | State | Evidence complete | Pairs / denominator | Quality delta runs | Completion delta runs | Reason |
| --- | --- | --- | ---: | ---: | ---: | --- |
| adaptive_direct | NEUTRAL | yes | 18 / 18 | +0 | +0 | quality_improvement_threshold_not_met |
| workflow | NOT_EXERCISED | no | 0 / 0 | +0 | +0 | workflow_not_exercised |
| learned_profile | NOT_EXERCISED | no | 0 / 0 | +0 | +0 | exact_stable_parent_not_exercised |
| distillation | NOT_EXERCISED | no | 0 / 0 | +0 | +0 | exact_stable_parent_not_exercised |

## Paired Against Grounded Direct

| Treatment | Pairs | Quality delta | Completion delta | Median latency delta |
| --- | ---: | ---: | ---: | ---: |
| auto | 18 | +0.0 pp | +0.0 pp | -5989 ms |
| pro | 18 | +0.0 pp | +5.6 pp | -4722 ms |

## Interpretation Boundary

Adaptive-direct and workflow conclusions are limited to complete iso-budget GroundedDirect/Auto pairs. Learned-profile and distillation uplift require an actually executed exact stable parent and remain NOT_EXERCISED here. Pro is descriptive because its native budget differs.
Failures, timeouts, and denials remain in the denominator. Deterministic checks validate the mechanism only; intelligence conclusions require this provider-backed paired evidence.
Raw prompts and model outputs remain outside Git.
