# Cindx Workflow GEPA V4 invalid task-spec attempt 0.2.25

## Decision

`INVALID_TASK_SPEC`. This attempt is not capability, regression, admission, or
promotion evidence.

The second authorized provider-backed V4 campaign completed both training
tasks and both matched validation pairs on the frozen source revision. The raw
evaluator emitted `valid_no_go_validation`, and its execution receipts remain
useful diagnostics. A post-run protocol audit found that the failed research
case was not a valid quality contract: the public task required only string and
number types, while the hidden verifier additionally required lower-case
identifiers and an exact phrase that the public task never specified.

Because both seed and candidate can satisfy the disclosed task while failing
that hidden normalization contract, the recorded quality failure, tie count,
and validation decision cannot support a candidate-quality conclusion. The
candidate snapshot was not promoted, the untouched test set and Grounded Direct
control were not run, and no production profile changed.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.25` |
| Source commit | `ee408db59112757ef356fefd25bda9eb3a42dc47` |
| Suite | `cindx-workflow-gepa-v4`, version `4` |
| Suite SHA-256 | `d80d4dd25c15355cc7285a03ca4dbff2ac807912d1ecf7d4d6061e1d51b9db51` |
| Training dataset SHA-256 | `1c363825330f8554e74e0d2cd5715e43608a56de05ad1ac1935ee6041d510f5a` |
| Provider | `alibaba_cn` |
| Candidate | `learned-pro-v4-24bad25edc99d43a`, generation `1` |
| Candidate profile SHA-256 | `d55daf91888763c0afd058a84f3a028a81d52d8899f98f85b9ccb32c972b8bb8` |
| Candidate route profile SHA-256 | `19228c073bf08f3aa3ecfac48bebb26554629ee6b3866c674fd90126977f6fdc` |
| Retained JSON SHA-256 | `c2a5c739354f8168c4eee115f117641eb7ebba2f9bd5c5804588c1bb5217c9c3` |
| External candidate snapshot SHA-256 | `a48a9d1f1adb1a0bfbd5c61cd81f83b891a134581e6a3fafc5bd1caf03179c46` |
| Production promotion | None |

## Training result

Both public training tasks completed and passed their deterministic quality
contracts with zero safety violations:

| Case | Quality | Behavior | Latency | Tokens |
| --- | --- | ---: | ---: | ---: |
| `coding-calculate-total` | pass | `1.0` | `61,618 ms` | `68,612` |
| `research-authoritative-threshold` | pass | `1.0` | `94,523 ms` | `64,589` |

The external reflection produced one unrepaired generation-1 candidate. The
candidate was selected without validation or test evidence, and the final test
set remained untouched.

## Diagnostic validation result

| Case | Seed quality | Candidate quality | Outcome | Seed latency | Candidate latency | Seed tokens | Candidate tokens |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: |
| `coding-parse-port` | pass | pass | tie | `60,115 ms` | `73,578 ms` | `68,247` | `78,726` |
| `research-measurement-choice` | fail | fail | tie | `58,778 ms` | `96,901 ms` | `57,715` | `56,784` |

Raw evaluator diagnostics:

- `0` candidate wins, `0` losses, and `2` ties across two task classes.
- Both arms completed `2/2`; each passed quality on only `1/2` cases.
- Candidate behavior delta was `0.0` and candidate safety violations were `0`.
- Candidate/seed latency ratio was `1.4339`, above the `1.05` bound.
- Candidate/seed token ratio was `1.0758`, above the `1.05` bound.
- Exact candidate profile and route semantics were observed on `2/2` candidate
  runs, but learned workflow-profile execution was observed on `0/2`.

The coding case has a valid quality contract. The research case does not,
because its hidden exact-value contract was not publicly derivable. Consequently
the aggregate win/loss/tie and efficiency ratios are retained only as
diagnostics for these executions, not as admission or regression evidence. The
absent workflow-profile execution also means this attempt did not demonstrate a
learned change to multi-step collaboration.

## Boundaries and follow-up

The untouched four-task test matrix and Grounded Direct control were correctly
not run after validation failed. Their empty fields are fail-closed behavior,
not failed test or control results. Raw prompts, model outputs, credentials, and
the candidate snapshot stay outside Git; the retained JSON contains bounded
receipts and hashes only.

The evaluator also emitted repeated semantic-memory shutdown warnings while
isolated runs were closing. Every validation arm reached a completed terminal
state, but the warnings should be diagnosed before another provider campaign.

Before a separately authorized follow-up campaign, a new suite version must
make output normalization and semantic fields explicit without disclosing the
answer, then prove that every hidden expected value is uniquely derivable from
the public contract. The campaign must also select among multiple candidates on
training evidence instead of accepting one reflective mutation without a
train-side comparison. Reusing this inadmissible candidate or inspecting the
untouched test set for candidate selection would invalidate the next comparison.

Machine-readable evidence:
[CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.json](CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.json).
