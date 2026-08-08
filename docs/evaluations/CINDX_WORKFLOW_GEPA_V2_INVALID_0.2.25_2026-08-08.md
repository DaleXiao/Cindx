# Cindx Workflow GEPA V2 invalid evaluation attempt 0.2.25

## Decision

`INVALID_EVALUATOR`; no capability, learning, promotion, or regression claim.

The explicitly authorized provider-backed campaign stopped during the second of
eight seed tasks. The Agent completed the task and the behavioral Node command
passed, but V2 additionally required the implementation to contain the literal
source text `Number(port)`. A behaviorally equivalent implementation was
therefore rejected. No candidate, holdout, learned-profile route, Task Graph,
Grounded Direct control, report, or snapshot was produced.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.25` |
| Treatment source commit | `40e07a3103ab98d040b7270404dbd6327a81dabe` |
| Capture date | `2026-08-08` |
| Suite | `cindx-workflow-gepa-v2`, version `2` |
| Suite SHA-256 | `f4376d653e714e0cd05614f1742cdfe9d0e2450863fec094b33dd0344f68977e` |
| Completed seed tasks | `1/8` |
| Stop task | `coding-parse-port` |
| Retained report or snapshot | none |

## Invalidating defects

- All four coding cases mixed behavioral commands with required source-code
  spellings. This measured implementation style in addition to behavior.
- All four research cases placed their exact expected objects in verifier files
  visible inside the Agent workspace. The product could read the answer instead
  of deriving it from the evidence.
- Because the campaign stopped before candidate generation, it observed no GEPA
  selection, paired train/holdout result, learned route, or workflow execution.

The successful first seed task and the failed second task are excluded from all
quality denominators. Tuning the Agent against these defects would be benchmark
overfitting, not intelligence improvement.

## Corrective boundary

Workflow GEPA V3 replaces source-string assertions with host-owned hidden
behavioral postconditions, removes answers from workspace-visible check scripts,
and byte-binds every evidence/check fixture through immutable postcondition
receipts. V3 also has deterministic tests proving that behaviorally equivalent
coding implementations pass while fixture tampering and answer leakage fail.
A new provider-backed V3 campaign requires separate explicit authorization.
