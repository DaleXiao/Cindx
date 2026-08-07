# Cindx Direct-finalizer GEPA calibration 0.2.23

## Decision

`VALID_TARGETED_EVIDENCE`; `NO_GO_FOR_PROMOTION`.

The preregistered four-pair Gate A completed, retained every recorded pair, and
blocked the frozen `Adversarial` Direct-finalizer candidate for a deterministic
regression. GEPA selection, the remaining two train cases, all eight holdout
cases, snapshot creation, profile promotion, and production deployment were
not run. This report therefore demonstrates a working fail-closed learning
gate, not an Agent-quality uplift or Fugu Ultra parity.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.23` |
| Treatment source commit | `2647daafae98e99d31c101886d2836e446d83d96` |
| Capture completed | `2026-08-07T18:18:28+08:00` |
| Suite | `cindx-direct-finalizer-gepa-v1`, version `1` |
| Suite SHA-256 | `8f7fbdf3abf20c16226d13f041bc1f589b7e4b32594e7555ea5a398570442ec4` |
| Dataset SHA-256 | `bedc4e5969c1399f717f6956ee15bdc0449b87ceaeabf6b5edf02f0cb4e058d2` |
| Cohort SHA-256 | `07f6f8dd6e0e158a7cc4c271f8eead579b8ffd1624ea0735b60b5bba3eb3e00b` |
| Provider | `alibaba_cn` |
| Producer | `qwen3.7-flash` |
| Independent reviewer | `deepseek-v4-flash` |
| GEPA selector | `glm-5.2` (not invoked) |
| Parent profile | `seed-auto-v1`, SHA-256 `be58315c193ef1544b1bea4bc0cfd8c666f27f82447699cb5c5f975a67bdd68a` |
| Candidate profile | `seed-auto-v1-g1-direct-finalizer-candidate-02`, SHA-256 `4f39e353d8d4915927ebbde92c1704d59fb9adcf60ce7c310b2e82eefa4d2cbe` |
| Retained JSON SHA-256 | `8f37590504fdd6ac499e4e0e528875608a1874a5fab970370eb47a53430e2029` |

## Frozen treatment and budget

Both arms started from the same canonical runtime, task contract, frozen tool
evidence, provider, producer model, context, and output budget. The only request
difference was the tools-disabled terminal Finalizer directive selected by the
Direct-finalizer gene: the parent `Evidence` phenotype adds no directive; the
candidate used the frozen `Adversarial` directive. Each pair used two producer
calls and two position-balanced reviewer calls.

The configured context window was `1,000,000` tokens and the terminal producer
output cap was `32,768` tokens. The durable campaign budget was at most 58
provider-call reservations, including the one allowed GEPA repair. Gate A
stopped the run after 20 reservations. Sixteen provider receipts belong to the
four retained pairs; four conservative reservations came from an interrupted
attempt described below and remain charged to the budget.

## Gate A results

| Case | Parent verification | Candidate verification | Parent reviewer | Candidate reviewer | Outcome |
| --- | ---: | ---: | ---: | ---: | --- |
| `train-deployment-timeout` | pass, `1.000` | pass, `1.000` | `0.00` | `1.00` | preservation pass |
| `train-json-contract` | fail, `0.000` | fail, `0.000` | `0.00` | `0.00` | no correction |
| `train-release-verification` | fail, `0.667` | fail, `0.667` | `0.15` | `1.00` | no deterministic correction |
| `train-test-failure` | pass, `1.000` | fail, `0.750` | `1.00` | `1.00` | candidate regression |

Aggregate deterministic verification passed `2/4` parent cases and `1/4`
candidate cases. Deterministic-score totals were `2.667` and `2.417`.
Position-balanced reviewer-score totals descriptively favored the candidate
(`3.00` versus `1.15`), but the frozen gate correctly gave the objective
contract regression precedence. Both arms had zero reviewer safety violations.

The candidate used 9,904 total tokens versus 11,593 for the parent and had a
median arm latency of 5,699 ms versus 7,622 ms. Those efficiency signals do not
override the quality regression and authorize no product change.

Gate A status is `blocked`, with blocker `candidate_regression`. GEPA attempts
are `0`; there is no promotion receipt, paired holdout digest, frozen snapshot,
or deployed profile.

## Failures and confounds

- The first process completed three pairs, then ended after durably reserving
  the fourth pair's four-call budget. The checkpoint stored no partial pair.
  One bounded resume on the same clean source reused the three complete pairs
  and recomputed the whole fourth pair. The conservative call count is 20, not
  16; no interrupted output was scored or retried into a pass.
- Provider serving variance, a four-case calibration gate, and one producer /
  reviewer assignment limit generalization. This is targeted causal evidence,
  not the complete 72-cell product baseline.
- The original blocked receipt omitted the top-level candidate profile even
  though the pair directive digest and private checkpoint remained bound. The
  retained JSON normalizes only that candidate identity from the validated
  checkpoint. A same-goal fail-closed writer fix now rejects future Gate A
  receipts without candidate identity. No score, output digest, provider
  receipt, decision, or call count was changed.
- Raw prompts, model outputs, API credentials, and the private checkpoint stay
  outside Git. The retained JSON contains only bounded receipts and digests.

## Product impact

The shipping seed remains unchanged. The new Direct-finalizer gene, exact
assignment/delivery receipts, causal runner, durable budget, independent review,
and GEPA/holdout gates are available for future frozen candidates. This
particular candidate is rejected and must not enter Auto, Pro, transfer, or
self-distillation. The current product claim remains the separate V5 result;
there is still no provider-backed GEPA, learned-profile, or Fugu parity claim.

Machine-readable evidence:
[CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.json](CINDX_DIRECT_FINALIZER_GEPA_0.2.23_2026-08-07.json).
