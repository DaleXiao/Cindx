# Cindx Workflow GEPA V4 validation 0.2.25

## Decision

`VALID_TARGETED_EVIDENCE`; `NO_GO_FOR_PROMOTION`.

The second authorized provider-backed V4 campaign completed both training
tasks and both matched validation pairs on the frozen source revision. The
candidate passed the causal route-receipt checks, but it produced no quality
win, did not exercise its learned workflow profile, failed one of two quality
contracts together with the seed, and exceeded both validation efficiency
bounds. The fail-closed validation gate therefore stopped the campaign before
the untouched test set and Grounded Direct control.

This is valid negative evidence for this candidate and frozen validation
cohort. It is not evidence of a general Pro regression, Agent-quality uplift,
or Fugu Ultra parity. The candidate snapshot was not promoted and no production
profile changed.

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

## Validation result

| Case | Seed quality | Candidate quality | Outcome | Seed latency | Candidate latency | Seed tokens | Candidate tokens |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: |
| `coding-parse-port` | pass | pass | tie | `60,115 ms` | `73,578 ms` | `68,247` | `78,726` |
| `research-measurement-choice` | fail | fail | tie | `58,778 ms` | `96,901 ms` | `57,715` | `56,784` |

Aggregate validation evidence:

- `0` candidate wins, `0` losses, and `2` ties across two task classes.
- Both arms completed `2/2`; each passed quality on only `1/2` cases.
- Candidate behavior delta was `0.0` and candidate safety violations were `0`.
- Candidate/seed latency ratio was `1.4339`, above the `1.05` bound.
- Candidate/seed token ratio was `1.0758`, above the `1.05` bound.
- Exact candidate profile and route semantics were observed on `2/2` candidate
  runs, but learned workflow-profile execution was observed on `0/2`.

The result rejects the candidate for three independent reasons: incomplete
candidate quality, no demonstrated quality gain, and efficiency regressions.
The absent workflow-profile execution additionally means this campaign did not
demonstrate a learned change to multi-step collaboration.

## Boundaries and follow-up

The untouched four-task test matrix and Grounded Direct control were correctly
not run after validation failed. Their empty fields are fail-closed behavior,
not failed test or control results. Raw prompts, model outputs, credentials, and
the candidate snapshot stay outside Git; the retained JSON contains bounded
receipts and hashes only.

The evaluator also emitted repeated semantic-memory shutdown warnings while
isolated runs were closing. Every scored validation arm still reached a
completed terminal state, so the warnings do not invalidate this report, but
they should be diagnosed before another provider campaign.

Before a separately authorized follow-up campaign, engineering work must target
the measured failures rather than loosen the gate: ensure learned workflow
decisions are actually exercised, repair the shared research-quality miss, and
reduce collaboration latency and token overhead. Reusing this rejected
candidate or inspecting the untouched test set for candidate selection would
invalidate the next comparison.

Machine-readable evidence:
[CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.json](CINDX_WORKFLOW_GEPA_V4_0.2.25_2026-08-08.json).
