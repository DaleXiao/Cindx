# Cindx Workflow GEPA V4 invalid evaluation attempt 0.2.25

## Decision

`INVALID_EVALUATOR`. This attempt is not capability, regression, admission, or
promotion evidence.

Both full-product training tasks completed and the campaign generated one
candidate snapshot. Before either arm of the first validation pair could run,
the matched-workspace gate rejected two byte-identical fixtures because it
compared path-bound workspace revision hashes. Seed and candidate intentionally
use different isolated temporary roots, so the canonical paths made those
revision hashes unequal even when their contents matched.

No validation arm, untouched test arm, learned-profile route comparison, Task
Graph comparison, or Grounded Direct control ran. No campaign report was
created, the generated snapshot was not admitted or promoted, and no production
profile changed.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.25` |
| Source commit | `f4553200c5eb7278a9e52b303e07b3b620fc1298` |
| Suite | `cindx-workflow-gepa-v4`, version `4` |
| Suite SHA-256 | `d80d4dd25c15355cc7285a03ca4dbff2ac807912d1ecf7d4d6061e1d51b9db51` |
| Provider attempt | Authorized, provider-backed |
| Completed stages | 2 training tasks; candidate snapshot generation |
| Validation and test cells | 0 |
| Campaign report | None |
| External candidate snapshot SHA-256 | `916e7f70b0a78048fde054b6a91425ea3ed75efd9dfa277d3a3155967da3b1b7` |
| Production promotion | None |

## Corrective boundary

The production workspace revision remains path-bound because it identifies a
specific workspace. The matched evaluator now uses a separate bounded content
fingerprint when comparing isolated seed and candidate roots. A deterministic
preflight materializes every validation and test case before any provider work;
tests prove that equal contents under different roots match and that a content
change does not. A new clean-revision provider run is required before V4 can
support any quality conclusion.
