# Cindx Workflow GEPA V7 provider evaluation 0.2.26

## Decision

`VALID_TARGETED_EVIDENCE`, `NO_GO_TRAINING`.

The authorized V7 campaign ran once from clean source and completed the full
train-only search gate. It produced three structurally distinct one-or-two-gene
candidates without repair, executed both matched product training pairs for
each candidate, and rejected all three before validation. Every candidate and
seed run completed, passed all external quality checks, and recorded zero
safety violations, but all six pairs tied on quality and none produced the
minimum Pareto-safe resource gain.

All candidate training runs remained Direct. Exact candidate profile and route
semantics receipts were observed, but no pair changed the executed route and no
Workflow or Task Graph collaboration ran. Structural route diversity therefore
did not become a product behavior difference and earns no capability credit.
No candidate was selected, no snapshot was published, and no production profile
changed.

## Provenance

| Field | Value |
| --- | --- |
| Application version | `0.2.26` |
| Source commit | `bc37fa96a848eec56a58807c43af456b6b542beb` |
| Suite | `cindx-workflow-gepa-v7` |
| Suite SHA-256 | `7f289521669a612f2577888c84c5b364b0cd076179f4776ca20b0bf7c97fcdab` |
| Training dataset SHA-256 | `e60728c38c2e7f7adf10ac214988518e7e762f6d5eea6e4f17caeb811977df5e` |
| Reflection evidence SHA-256 | `ca67db08100d73b14e0b74fcdc6a36dfb52d315077214510b8cd9131b017790a` |
| Provider | `alibaba_cn` |
| Raw private report SHA-256 | `483b28f50f3c78513869c872e2115235debfaadbdce0a019e50b32c7c0112a2d` |
| Raw private journal SHA-256 | `186a1b4476c881b65d33a3fed713622ef53e3b7ccf6c58927c7189adf0cb7a0f` |
| Retained JSON SHA-256 | `1aff96c30d757f5078b4b785b2216da3d771f121e742befdf7822c68f23332de` |
| Published candidate snapshot | None |
| Production promotion | None |

Provider endpoint and model identities are retained only as hashes in the
machine-readable report. Raw prompts, provider responses, credentials, private
candidate snapshots, and campaign workspaces remain outside Git.

## Completed campaign work

The campaign completed `14/14` reserved product runs using `99` logical product
model calls and `1,391,699` product tokens. Mutation accounting reserved all
`12` bounded call slots; candidate generation completed `3` provider calls
across three physical attempts and used `34,140` mutation tokens. The two
training seed runs both completed with full quality and zero safety violations
before any candidate was generated.

| Candidate | Measured gene changes | Quality | Latency ratio | Token ratio | Executed route | Decision |
| --- | --- | --- | ---: | ---: | --- | --- |
| `2baa448b...` | `graph_depth`, `max_parallel_branches` | 0 wins, 0 losses, 2 ties | `1.6316` | `1.3419` | Direct `2/2` | ineligible |
| `3df5ce1d...` | `verification` | 0 wins, 0 losses, 2 ties | `0.9959` | `1.1268` | Direct `2/2` | ineligible |
| `7831427a...` | `context_policy` | 0 wins, 0 losses, 2 ties | `0.9820` | `1.0394` | Direct `2/2` | ineligible |

Candidate 1 regressed both latency and tokens. Candidate 2 was effectively flat
on latency and regressed tokens. Candidate 3 reduced latency by about `1.8%` but
increased tokens by about `3.9%`; this is below the frozen train gate's required
`5%` resource improvement while the other ratio remains at most `1.05`. All
three therefore received `no_train_side_improvement`.

## Sealed evidence

Because no candidate passed train-only selection:

- the two unseen validation pairs were not opened;
- the four untouched test tasks and their counterbalanced repeats were not run;
- the Grounded Direct control was not run;
- no external candidate snapshot was written;
- no production Auto or Pro profile changed.

Those absent stages are sealed, not failed or passed. The campaign ended with a
complete journal, no pending action, and status `valid_no_go_training`.

## Interpretation

This run proves that the repaired candidate-generation transport can complete
inside the full product campaign and that the fail-closed train gate rejects
valid but behaviorally ineffective candidates. It does not prove GEPA quality
uplift, Workflow collaboration uplift, Pro improvement, transfer,
self-distillation, Fugu Ultra parity, or frontier Agent performance.

The measured bottleneck is now downstream of generation: three different genome
and route-profile identities all preserved the same Direct behavior on the two
training tasks. The next protocol must learn from this completed no-go evidence
without reopening V7 validation or test data. Repeating V7 or weakening its
selection gate would be evaluation leakage, not progress.

Machine-readable evidence:
[CINDX_WORKFLOW_GEPA_V7_0.2.26_2026-08-09.json](CINDX_WORKFLOW_GEPA_V7_0.2.26_2026-08-09.json).
