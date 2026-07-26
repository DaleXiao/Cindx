# Cindx Agent Engine 0.1.47 Review

## Release candidate

- Evaluated engine commit: `5b1dcd69ea94289ba2721470260c4ca6283cf673`
- Raw evidence SHA-256: `1627cb2f3aa9187d73a84f1ec3f8026297e1ba26551fca20621714e4dca82bce`
- Frozen GPQA source revision: `56686c06f5e19865c153de0fdb11be3890014df7`
- Frozen GPQA file SHA-256: `41d1213cd7a4998605a26c2798500652572007161b3a92817ba46b35befcd305`
- Provider-backed run: 12 matched GPQA-Diamond questions, 36 treatments, 4,476.02 seconds.
- No Agent Engine behavior was changed after inspecting the provider-backed results. The only post-run code change lets the sanitizer accept the already-written `raw.v3` evidence schema.

## What the run establishes

| Treatment | Completed | Correct under budget | p50 | p95 | Tokens observed |
| --- | ---: | ---: | ---: | ---: | ---: |
| Fast / direct | 8/12 | 8/12 (66.67%) | 38.6s | 180.0s | 21,068 |
| Auto | 10/12 | 9/12 (75.00%) | 120.0s | 180.0s | 127,179 |
| Pro | 9/12 | 8/12 (66.67%) | 160.1s | 300.0s | 192,082 |

Auto delivered two more answers than Fast and gained one matched correct answer. Pro delivered one more answer than Fast but did not gain a net correct answer. Every exact McNemar comparison has `p=1.0`; with 12 questions, none of these differences is statistically persuasive.

The run does not establish Fugu-Ultra parity. It does not establish that Pro is better than Auto or Fast. It does show that the shared engine and Auto fail-soft path can preserve an answer when the direct call fails, but the evidence is one question and needs replication.

## Failure review

- Fast had four provider cancellations at the 180-second call limit. All eight delivered Fast answers were correct.
- Auto had two cancelled terminal paths. One completed Chemistry answer was wrong, leaving 9/10 completed-answer accuracy.
- Pro had three 300-second deadline failures. Diagnostics show analysis or synthesis consuming the stage budget before a terminal answer could be started or completed.
- Auto and Pro made the same wrong Chemistry decision once. Multiple roles did not provide independent error correction on that case.
- Pro's p50 improved from 212.3 seconds in 0.1.43 to 160.1 seconds, while its score and completion count stayed at 8/12 and 9/12. Its p95 still reached the 300-second cap.
- Auto moved from 8/12 to 9/12 and from 9/12 to 10/12 completed versus 0.1.43, but p50 grew from 44.8 seconds to 120.0 seconds and observed tokens grew from 46,171 to 127,179. The older run is not concurrent, so provider conditions confound that comparison.

## GEPA boundary

All 24 orchestrated runs used the built-in seed prompt genome. No run used a frozen, held-out-evaluated GEPA snapshot. The release now has provenance-bound snapshot and promotion machinery, but this GPQA result is not evidence that GEPA improved the product. A future GEPA claim requires a promoted snapshot frozen before the test sample, a disjoint training set, repeated matched holdout runs, and a rollback threshold.

## Engineering gates

- Root Rust workspace tests passed, including 168 orchestrator tests, 134 active runtime tests, memory, RAG, storage, provider, and tool suites.
- Desktop Rust tests passed: 252 passed, 6 ignored.
- Frontend tests passed: 17/17. The production frontend build passed.
- Root and desktop Clippy passed with warnings denied.
- The full quality gate passed browser and computer sidecar smoke tests, 72 routing cases, memory retrieval checks, structure limits, version checks, performance policy, tests, and frontend build.
- Current warm p95 diagnostics versus the 0.1.39 baseline: session projection 132us vs 187us; context 4,378us vs 27,045us; semantic RAG 19,811us vs 47,169us; literal RAG 75,093us vs 113,542us.
- Shell execution now has a dedicated module, a cleared allowlisted environment, non-login `zsh -fc`, canonical working-directory scope, and destructive-command session-grant rejection.
- Permission reuse now performs an indexed capability query and shell grants are tied to the exact canonical working directory.
- Product and evaluation paths now share the same durable Agent Engine, anytime controller, task-graph contracts, and bidirectional candidate comparison.

## Release decision

Release 0.1.47 as an engineering and safety improvement, not as a frontier-intelligence claim. The next intelligence gate should target one issue at a time:

1. Reserve terminal-answer time before launching optional branches, and require a direct-answer checkpoint that can be returned without another model call.
2. Measure branch contribution and stop roles that produce correlated answers without independent evidence.
3. Promote a GEPA snapshot only from disjoint training evidence, then compare the frozen snapshot against the seed on repeated matched holdouts.
4. Repeat the frozen GPQA sample in multiple serving windows before expanding the benchmark or claiming uplift.

The sanitized machine-readable report is `CINDX_AGENT_ENGINE_0.1.47_GPQA_MATCHED_2026-07-26.json`; the generated human-readable report is `CINDX_AGENT_ENGINE_0.1.47_GPQA_MATCHED_2026-07-26.md`.
