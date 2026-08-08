# Cindx Workflow GEPA V5 provider evaluation 0.2.25

## Decision

`VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`.

The first authorized provider-backed V5 campaign ran once from clean source,
generated three distinct learned route profiles from the same two public
training observations, selected one candidate from training evidence only, and
then stopped at the unseen validation gate. The selected candidate completed
and passed both validation tasks with zero safety violations, but it tied the
seed on quality, used `1.4337x` the latency and `1.2966x` the tokens, and won no
case. The Grounded Direct control and untouched test set therefore remained
sealed.

The exact learned profile and route phenotype were exercised in both candidate
validation runs. The result proves that the profile can causally alter product
routing, but it does not prove a quality improvement, useful collaboration, a
promotion candidate, or Fugu Ultra parity. No candidate snapshot was published
and no production profile changed.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.25` |
| Source commit | `86f7dd61b0ee42d265b656425957b1a4550a4700` |
| Suite | `cindx-workflow-gepa-v5` |
| Suite SHA-256 | `2cbda20664a36c74e9ad4abe480002a77c72f8aa31db3768b1b53f360c5867f8` |
| Training dataset SHA-256 | `b4bd2de2bf801a1a73b89f241fe58651004505bc304809670eeae5f5291c6711` |
| Reflection evidence SHA-256 | `98d86703bd154e41adfd10599690ee3800347bf0a0cef8665724055e08186eb3` |
| Provider | `alibaba_cn` |
| Selected candidate | `learned-pro-v5-a7b7fc9e57ae9c1b`, generation `1` |
| Candidate profile SHA-256 | `ad89b3f8064ee61c8b8b9fda22cdd4fc95d56d7b9663823e83c6eabbf59f20a4` |
| Candidate route profile SHA-256 | `cccc760af4bd1509adafefee0a62dc4bcac0329973270d8e74da90b74eb19be9` |
| Retained JSON SHA-256 | `8af764dbea12ac4269ff82fabdb26e7a3bd8520947dc42f2f5c8d58d8eacb73e` |
| Published candidate snapshot | None |
| Production promotion | None |

Provider endpoint and configured model identities are retained only as hashes
in the machine-readable report. Raw prompts, model outputs, credentials, and
private train candidate snapshots remain outside Git.

## Training search

All three candidates completed both matched training pairs with full quality,
zero losses, and zero safety violations. Only candidate 1 changed the public
Workflow route contract, so it was the sole train-eligible candidate:

| Candidate | Route result | Quality | Latency ratio | Token ratio | Decision |
| --- | --- | --- | ---: | ---: | --- |
| `a7b7fc9e...` | Candidate `2/2`, seed `1/2`; Workflow exercised | two ties | `4.3557` | `2.1075` | selected |
| `7d93b0ea...` | Candidate `1/2`, seed `1/2`; no route contrast | two ties | `0.7018` | `0.9531` | ineligible |
| `fd8046b9...` | Candidate `1/2`, seed `1/2`; no route contrast | two ties | `0.7750` | `1.1498` | ineligible |

On `research-authoritative-threshold`, the selected candidate changed the seed
route from `direct` to `workflow` and satisfied the declared route contract.
That run still tied the seed on quality while taking `605,574 ms` and `166,694`
tokens. This is causal route evidence, not useful-intelligence evidence.

## Unseen validation

| Case | Outcome | Seed latency | Candidate latency | Seed tokens | Candidate tokens |
| --- | --- | ---: | ---: | ---: | ---: |
| `coding-parse-port` | tie, both quality pass | `55,969 ms` | `65,415 ms` | `57,966` | `102,096` |
| `research-measurement-choice` | tie, both quality pass | `51,400 ms` | `88,523 ms` | `56,597` | `46,451` |

Aggregate validation facts:

- candidate wins `0`, losses `0`, ties `2`;
- both arms completed and passed quality on `2/2` cases;
- candidate behavior delta `0.0` and safety violations `0`;
- candidate/seed latency ratio `1.4337`;
- candidate/seed token ratio `1.2966`;
- exact candidate profile and route semantics observed on `2/2` candidate runs;
- learned Workflow/Task Graph execution observed on `0/2` validation runs.

The validation tasks intentionally leave route choice to the learned profile.
The candidate chose `direct` for both, so it did not demonstrate generalized
collaboration. A post-run task-spec audit confirmed that the public objectives
and specifications disclose every value, type, normalization, and fixed-string
requirement used by the validators. This result is therefore a valid no-go,
not an invalid evaluator or hidden-contract failure.

## Boundaries and next decision

The fail-closed protocol correctly withheld the external snapshot, Grounded
Direct control, and all eight untouched test executions. Their absent fields
are unrun evidence, not failures. Because validation did not pass, this campaign
does not justify a version bump, application build, release, or production
profile change.

The next engineering decision should address the observed selection failure:
the train gate admitted a route-contract improvement even when it preserved
quality at extreme cost, while the learned route did not generalize to either
validation task. A new campaign must use a new frozen protocol and separate
authorization; this V5 run must not be repeated or used to tune against its
sealed test set.

Machine-readable evidence:
[CINDX_WORKFLOW_GEPA_V5_0.2.25_2026-08-08.json](CINDX_WORKFLOW_GEPA_V5_0.2.25_2026-08-08.json).
