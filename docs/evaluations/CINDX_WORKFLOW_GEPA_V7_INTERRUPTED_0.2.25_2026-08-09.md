# Cindx Workflow GEPA V7 interrupted campaign 0.2.25

## Decision

`INTERRUPTED_CAMPAIGN` / `NO_CAPABILITY_CONCLUSION`

The one authorized V7 provider campaign on August 9, 2026 stopped during
candidate mutation search with `Run stopped during model call: no_progress`.
It was not rerun or tuned. This result neither proves nor disproves an Auto,
Pro, GEPA, collaboration, Task Graph, or frontier-intelligence improvement.

| Field | Observed value |
| --- | --- |
| Source commit | `8be0fb1070635ecaaedb344be3f043494cebd0c5` |
| Application version | `0.2.25` |
| Suite | `cindx-workflow-gepa-v7` |
| Suite SHA-256 | `7f289521669a612f2577888c84c5b364b0cd076179f4776ca20b0bf7c97fcdab` |
| Completed training seed runs | `2 / 2` |
| Completed product logical calls | `15` |
| Completed product tokens | `116,781` |
| Candidate population | Not generated |
| Validation / test / control | Not opened |
| Snapshot / production profile | Not published / unchanged |

## What happened

Both training seed actions completed and were committed to the external
hash-chained journal before candidate search began. Their retained receipts
record `7` and `8` logical model calls, `59,374` and `57,407` tokens, completed
terminal states, output hashes, and an intact receipt chain.

The journal then reserved the single mutation search with a hard upper bound of
`12` logical calls. The model-call stage stopped on the runtime's `no_progress`
boundary before a candidate population completed. Because completion never
occurred, actual mutation calls, physical attempts, and tokens are not reported
as observed usage. The `12` value is a reservation ceiling, not measured use.

The raw journal intentionally remains `running` with a pending action. Its path
cannot be reused, so an interrupted provider action cannot be silently replayed
or charged to a fresh campaign budget. No product report or candidate snapshot
was emitted by the evaluator.

## What this run establishes

- The clean source, frozen suite, provider/model hashes, campaign budget, and
  action order were bound before provider work.
- Product actions were reserved before execution and committed into a
  hash-linked receipt chain.
- The mutation-stage liveness guard stopped a non-progressing call instead of
  allowing unbounded provider work.
- The interruption sealed candidate selection, validation, untouched test,
  Grounded Direct control, snapshot publication, and production promotion.

It does not establish training quality, candidate quality, efficiency uplift,
or regression because no candidate pair exists.

## Review

The scientific outcome is a diagnostic no-result, not another optimization
round. V7 removed the route-answer leak, fixed human mutation directions, and
unbounded/replayable campaign behavior, but the first live run shows that the
candidate-generation stage does not yet reliably finish under its frozen
liveness contract. Changing that contract now and rerunning the same campaign
would turn this result into tuning evidence. Any follow-up must be a separately
frozen protocol with an independently justified mutation-stage transport and
liveness design.

The sanitized machine-readable evidence is
[CINDX_WORKFLOW_GEPA_V7_INTERRUPTED_0.2.25_2026-08-09.json](CINDX_WORKFLOW_GEPA_V7_INTERRUPTED_0.2.25_2026-08-09.json).
