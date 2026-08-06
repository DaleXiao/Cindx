# Cindx memory-effect evaluation

- Decision: **IMPROVED**
- Execution source: Cindx `0.2.19`, commit `cd647030092971c84b7fc4dc6274285bec12b528`
- Frozen suite: `cindx-agent-memory-effect-v2` version 2
- Frozen protocol: 18 cells / 9 matched pairs (6 required-memory, 3 irrelevant-memory control)
- Suite / plan SHA-256: `86891892daa8893d3911723a7cf50fa2f56aeb240831c2caa5135811051b3e82` / `8dc7ced6e05a1da1eb35988cc9fc5b750e355aaf6c238f3aba4b7c7decd5a8f4`
- Provider binding / configuration SHA-256: `885d79470ee787231b81cf39d4da8c4ce4c72341fa38a54a92ca6cb7a56455f5` / `f12ab6dd194d8628f44dc33a1499ded5013781e48f5d4c8f8ca35f7ec085bae7`
- Resolved budget: `{"max_duration_ms":2700000,"model_call_timeout_ms":300000,"tool_call_timeout_ms":900000,"initial_model_calls":18,"max_model_calls":72,"initial_tool_calls":36,"max_tool_calls":144,"no_progress_timeout_ms":120000,"max_total_tokens":301989888,"max_physical_model_attempts":288,"terminal_token_reserve":16777216,"terminal_physical_model_attempt_reserve":16}`
- Completion: memory-on 9/9; memory-off 9/9
- Quality passed: memory-on 9/9; memory-off 3/9
- Recall / selected: memory-on 9/9; memory-off 0/0
- Pair states: {"EVALUABLE":9}
- Positive improvements/regressions: 6/0
- Negative-control regressions: 0
- Evidence-invalid cells / confounded pairs / required not-exercised pairs: 0/0/0
- Raw evidence SHA-256: `6bca4496b4f7cbc1c96cc11f0bc2183caee74d0ada39ce3f795e4fa03b526a19`
- Sanitized evidence digest: `68fcb153b9649c55d8822f04e22395d8f34587b167a6a73c73dde27f65538284`

Failures, invalid evidence, confounds, and non-exercised routes remain in the frozen denominator. This result is limited to durable-memory utility under the frozen matched direct harness. It does not establish native Auto-router, GEPA, distillation, or frontier-Agent uplift.
