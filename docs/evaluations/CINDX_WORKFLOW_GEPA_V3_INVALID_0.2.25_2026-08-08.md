# Cindx Workflow GEPA V3 invalid evaluation attempt 0.2.25

## Decision

`INVALID_TASK_SPEC`. This attempt is not capability, regression, or promotion
evidence.

The first seed task completed and passed. The second seed task returned a
string-valued port while the host verifier required a number. That requirement
was absent from both the task objective and the Agent-visible check. Hidden
examples are valid; hidden behavior requirements are not. The run therefore
stopped at seed task 2 of 8 before mutation, validation, learned-profile route,
Task Graph comparison, Grounded Direct control, report creation, or snapshot
creation.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.25` |
| Source commit | `2b5f5351eba42da18d99d1cbd2fda74129b6ac0d` |
| Suite | `cindx-workflow-gepa-v3`, version `3` |
| Suite SHA-256 | `bfb7527f7af504a1ed7540576dbc480e2066eb16d6fe631a9c13254a38bde0cb` |
| Provider attempt | Authorized, provider-backed |
| Completed cells | 1 passed seed; 1 invalidly rejected seed |
| Report or snapshot | None |

## Root causes found after the stop

V3 had three independent scientific defects:

1. All four coding tasks hid part of their behavior contract while exposing
   only smoke checks to the Agent.
2. Candidate selection used holdout observations, so that holdout was not an
   untouched final test.
3. Candidate evidence came mainly from a read-only prompt workflow and model
   reviewer, not matched executions of the complete product Agent and tools.
   External host-verifier failures also did not enter the learning evidence, so
   an internally `completed` but externally failed run could teach the wrong
   lesson.

## Replacement boundary

V4 discloses complete behavior contracts while retaining unseen concrete test
inputs, feeds external deterministic outcomes into reflection, admits a single
candidate on validation only, and reserves a separate two-repeat matched
product test. Seed and candidate use isolated identical workspaces, execution
order is counterbalanced, and candidate receipts must prove the exact learned
profile influenced route semantics and at least one Task Graph run. V4 remains
unverified until a separately authorized provider campaign completes.
