# Cindx memory-effect evaluation

- Decision: **INVALID_EVIDENCE**
- Execution source: Cindx `0.2.19`, commit `1abdd6849e4a99c7b30f7f7aa2123efbbdb0d51a`
- Frozen protocol: 18 cells / 9 matched pairs (6 required-memory, 3 irrelevant-memory control)
- Suite / plan SHA-256: `ea469ae361dc8bb96e007ed6e37a12ecddd36056cde472457af9838d005fd2df` / `b5796a628633eb214747d08bd118102c988519b2b39a6987b856da7ef6be833c`
- Provider binding / configuration SHA-256: `885d79470ee787231b81cf39d4da8c4ce4c72341fa38a54a92ca6cb7a56455f5` / `f12ab6dd194d8628f44dc33a1499ded5013781e48f5d4c8f8ca35f7ec085bae7`
- Resolved budget: `{"max_duration_ms":2700000,"model_call_timeout_ms":300000,"tool_call_timeout_ms":900000,"initial_model_calls":18,"max_model_calls":72,"initial_tool_calls":36,"max_tool_calls":144,"no_progress_timeout_ms":120000,"max_total_tokens":301989888,"max_physical_model_attempts":288,"terminal_token_reserve":16777216,"terminal_physical_model_attempt_reserve":16}`
- Completion: memory-on 9/9; memory-off 9/9
- Quality passed: memory-on 9/9; memory-off 3/9
- Recall / selected: memory-on 9/9; memory-off 0/0
- Pair states: {"EVALUABLE":8,"INVALID_EVIDENCE":1}
- Positive improvements/regressions: 5/0
- Negative-control regressions: 0
- Evidence-invalid cells / confounded pairs / required not-exercised pairs: 1/0/0
- Raw evidence SHA-256: `72cc2a2ca5bc45768798a05a1db8eac5d1a0af792047138f328b24133fed550e`
- Sanitized evidence digest: `7a49d6deb31d9e11df28157aef397ea272fde09f31b006281e67cdd370738d00`

Failures, invalid evidence, confounds, and non-exercised routes remain in the frozen denominator. Because the matrix contains invalid evidence, its descriptive deltas do not establish causal durable-memory utility or any product uplift. It does not establish native Auto-router, GEPA, distillation, or frontier-Agent uplift.
