# Cindx Conductor ownership targeted evaluation 0.2.27

## Decision

`VALID_TARGETED_EVIDENCE`, `KEEP_HARNESS_FIX`.

The independent holdout supports keeping commit
`affe091821deaeb4ca14e3b263cbea2ebe956408`. A complete `file.read_many`
operation can now provide the same typed exact-readback evidence as a complete
single-file read. The previously observed product failure closed without a
route rule, quality loss, safety violation, or product-budget change.

This is not evidence of general intelligence uplift, Workflow benefit, GEPA
learning, Fugu parity, or lower provider cost. Every classifiable Auto and Pro
run selected Direct, and the two-case holdout has one replicate.

## Frozen design

| Field | Value |
| --- | --- |
| Old product parent | `11ceca569c31aed0143cfd7ec42eb5bd090c3840` |
| Old train evidence revision | `be492184d7988902eb4cca9edb3fde7f66892107` |
| Current control revision | `f55a4c0bc380dc8f930153b30b3545ed427b6201` |
| Candidate revision | `affe091821deaeb4ca14e3b263cbea2ebe956408` |
| Train suite | `cindx-conductor-ownership-train-v1@1` |
| Train SHA-256 | `8cbe295604c125ad2b57aed7d43a3157fffbf83ce9b39ea383f092691b98d11d` |
| Holdout suite | `cindx-conductor-ownership-holdout-v1@1` |
| Holdout SHA-256 | `820594e695101612b7d1c05443a015986c35a1b6704371168ebbd1b89858c00b` |
| Matrix per revision and split | 2 cases x 4 treatments x 1 replicate |
| Treatments | Oracle Reference, Grounded Direct, Auto, Pro |

The task text did not name Direct, Workflow, collaboration, or a desired model
count. Old and current train were each executed once. Train evidence was used
only to locate one shared harness failure. The control and candidate then ran
once on the untouched holdout. No cell was rerun, discarded, or added after
results were visible.

The control and candidate holdout used identical case inputs, immutable
fixtures, treatment order, treatment positions, configured models, and resolved
budgets in all eight cells. Their execution-plan hashes differ because the
plan receipt binds the exact source revision; the ordered cell projections are
identical.

## Train diagnosis

| Revision | Structural decision | Product completion | Provider receipt gaps | Strategy receipt gaps |
| --- | --- | ---: | ---: | ---: |
| Old parent plus neutral evaluator | `INVALID_BASELINE` | 5 / 6 | 1 | 1 |
| Current Conductor control | `INVALID_BASELINE` | 2 / 6 | 4 | 4 |

The train split did not establish that either execution-plan architecture was
better. All classifiable Auto and Pro runs selected Direct. The common failure
occurred after successful workspace mutation and externally correct output:
the task contract still reported `workspace_verification` unsatisfied because
the model verified several files with `file.read_many`, while only
`file.read` could issue `WorkspaceExactReadbackV1` evidence.

Context compilation was active, but this matrix did not isolate context-policy
quality. No seeded memory artifact, learned prompt profile, Workflow execution,
or Task Graph collaboration was exercised. Those systems therefore cannot be
credited or blamed for the observed completion gap.

## Minimal correction

The retained change adds exact-readback authority to `file.read_many` only when
all of these conditions hold:

- the invocation and result identities match and the top-level result succeeds;
- every requested offset is zero;
- the typed batch result is complete and not cancelled;
- requested, read, failed, truncated, and item counts agree exactly;
- every item preserves request order and path, starts at byte zero, succeeds,
  is not truncated, and returns the complete file bytes.

Partial, offset, cancelled, mixed-success, list, search, and arbitrary shell
results remain unable to satisfy exact workspace readback.

## Independent holdout

| Arm | Structural decision | Product completion | Product quality | External effect | Receipt gaps | Safety violations |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Control `f55a4c0` | `INVALID_BASELINE` | 5 / 6 | 6 / 6 | 6 / 6 | 1 provider, 1 strategy | 0 |
| Candidate `affe091` | `VALID_BASELINE` | 6 / 6 | 6 / 6 | 6 / 6 | 0 | 0 |

On `holdout-capacity-retention`, control Auto produced the correct external
artifact but failed terminal completion after two repair attempts. Candidate
Auto completed with the same Direct route and full external quality. The other
five product cells preserved completion and quality. Oracle Reference is a
no-tools ceiling rather than a product baseline; its provider deadline failures
remain in the reports and do not enter product mechanism eligibility.

Within the candidate holdout, Auto completed both pairs with no quality or
completion loss against Grounded Direct. Its analyzer result is `NEUTRAL`
because there was no quality win. Median latency ratio was `0.7590` and total
token ratio was `1.0486`, but one replicate per case is insufficient to
attribute either resource difference to this change. Pro is descriptive due to
its larger native budget.

Workflow, learned-profile, and distillation states are `NOT_EXERCISED`. The
result therefore closes a reliability defect in the harness; it does not prove
better Conductor route selection or multi-model collaboration.

## Verification

- `cargo test -p tools --lib`: 122 passed.
- `cargo test -p agent-runtime postcondition_receipt --lib`: 16 passed.
- desktop `conductor_ownership_suite` with `realworld-eval`: 2 passed.
- `node --test scripts/agent-realworld-contract.test.mjs`: 27 passed.
- `cargo fmt --all -- --check`: passed.

The desktop test emitted the existing oversized `__eh_frame` linker warning;
it did not fail the suite. No application release build or installation was
performed because this targeted evidence does not change the application
version or constitute a release.

## Evidence files

Raw prompts, model outputs, provider credentials, workspaces, and temporary
databases remain outside Git. The committed files below are generated by the
existing sanitizing analyzer.

| Evidence | File SHA-256 | Raw evidence SHA-256 |
| --- | --- | --- |
| [Old train](CINDX_CONDUCTOR_OWNERSHIP_V1_OLD_TRAIN_0.2.27_2026-08-09.json) | `9aed187385ec80446bffe8fd2adec00478a0a1fecf4e5b8e98d34537c502b43b` | `bb62c65513910c9da2dbd5bf795229448487f5d86197c45fa251dc3cde9e0bd9` |
| [Current train](CINDX_CONDUCTOR_OWNERSHIP_V1_CURRENT_TRAIN_0.2.27_2026-08-09.json) | `9dbe98478cea5c51ecf27aac60f439725938dfc09a4385f2d6aababfc20963c3` | `68eb4ec84a02445df55b6e5f0baa7ace4a3935a991824339e35f4cd53ec02e0b` |
| [Control holdout](CINDX_CONDUCTOR_OWNERSHIP_V1_CONTROL_HOLDOUT_0.2.27_2026-08-09.json) | `0361d7d5f698bcb9b35216fcd97f6f1cb9ce716c111987efefce58370ea53bbb` | `e58c8d864f46683535efb7907af2beb64e7172b031c6ac847e233cfc088cf7b4` |
| [Candidate holdout](CINDX_CONDUCTOR_OWNERSHIP_V1_CANDIDATE_HOLDOUT_0.2.27_2026-08-09.json) | `5c3317f5785d091994001a8d2352a77ed49c2dc8999f19143686739c8e83cc26` | `4b343dbcf07dac979c43cb4fbc1c7409a7cee549c906ecfc03b21609537acd52` |

## Boundary and next decision

Keep the batch-readback correction. Do not tune it further from this holdout.
The next intelligence experiment must separately exercise route contrast and
Workflow on fresh tasks; this result supplies no permission to promote a GEPA
profile or claim progress toward Fugu Ultra.
