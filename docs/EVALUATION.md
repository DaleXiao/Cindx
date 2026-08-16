# Evaluation and Claim Boundary

This is the maintained decision ledger. Detailed historical reports were
removed from the current tree because they duplicated status, included
superseded protocols, and were repeatedly read as current product facts. Their
exact contents remain available at Git commit `f2ce3b7` and earlier revisions.

## Evidence Levels

1. **Contract tests** prove schemas, invariants, replay, and fail-closed behavior.
2. **Deterministic suites** prove frozen cases and measurement plumbing without
   contacting a provider.
3. **Provider diagnostics** measure model behavior on a narrow frozen task set.
4. **Product evaluation** requires complete end-to-end runs, external effects,
   receipts, retained failures, and an explicit matched decision.
5. **Frontier/parity claims** require independent external comparison under a
   frozen, position-balanced, resource-accounted protocol.

Passing a lower level does not imply a higher-level result.

## Current Decision Ledger

| Evidence | Revision/version | Result and admitted claim |
| --- | --- | --- |
| Agent Real-World V5 | `a0000fa`, `0.2.22` | `VALID_BASELINE`. In the iso-budget adaptive-direct subset Auto gained one matched quality pass, preserved completion, reduced median paired latency by `3,585 ms`, and used about `10.0%` fewer total tokens. Workflow, learned profile, and distillation were not exercised; Pro is descriptive. |
| Memory-effect V2 | `cd64703`, `0.2.19` | `IMPROVED` for the frozen direct memory-on/off harness: all `9/9` pairs were evaluable, all `6/6` required-memory pairs improved, and all three decoy controls passed in both arms. This is not a general Agent-quality claim. |
| Dynamic Collaboration V1 | `bafbbd5`, `0.2.27` | `VALID_TARGETED_EVIDENCE`, `NO_GO_NOT_EXERCISED`. Every Auto/Pro run remained Direct. Auto used `1.6028x` median latency and `1.1208x` tokens versus Grounded Direct; Pro produced no matched quality or completion win. |
| Conductor ownership | `ae6dfcb`, `0.2.27` | `VALID_TARGETED_EVIDENCE`, `KEEP_HARNESS_FIX`. Complete batch readback closed the observed terminal-verification defect. Every classifiable route remained Direct, so no workflow or learning uplift was shown. |
| Direct-finalizer calibration | `2647daa`, `0.2.23` | `VALID_TARGETED_EVIDENCE`, `NO_GO_FOR_PROMOTION`. The candidate regressed a preservation case; GEPA, holdout, snapshot, deployment, and transfer did not run. |
| Workflow GEPA V5 | `86f7dd6`, `0.2.25` | `VALID_TARGETED_EVIDENCE`, `NO_GO_VALIDATION`. The learned profile changed training route and exercised Workflow, but unseen validation produced zero wins and two ties at `1.4337x` latency and `1.2966x` tokens. Nothing was promoted. |
| Workflow GEPA V7 | `bc37fa9`, `0.2.26` | `VALID_TARGETED_EVIDENCE`, `NO_GO_TRAINING`. Three candidates completed six training pairs with full task quality, but all pairs tied, all routes remained Direct, and no candidate passed the Pareto resource gate. |
| Workflow GEPA V12 | `ff8c238`, recorded in `0.2.30` | `INVALID_EVIDENCE`. The Direct arm retained route evidence but failed terminal completion; the Workflow arm stopped before strategy-event persistence. There is no matched pair, GO/NO-GO, candidate, snapshot, promotion, or capability conclusion. |
| Collaboration successor V1 / Goal 3E | `12a3ea2`, `0.2.32` | `INVALID_EVIDENCE`, `CENSORED`. The one-shot capability was consumed and the first baseline Direct arm was reserved, but the product run ended before selection with zero selected decisions. The terminal producer correctly persisted explicit `not_selected`; no treatment or Owner execution occurred. The selected-only projector misclassified that legal state as a malformed receipt, so zero runs were admitted, no matched pair exists, and Workflow, candidate, and holdout did not run. The protocol cannot be retried and supports no uplift, cost, latency, or capability conclusion. |
| Delivery Verification v3 | frozen v3 authority, `0.2.34` | `INVALID_EVIDENCE`, `CENSORED`. Its first calibration Reviewer call made one provider attempt and returned empty content. Journal validation rejected the bound zero-byte response artifact before accepting a terminal call receipt, so no case completed, holdout never opened, and no matched pair or scientific conclusion exists. The protocol is consumed and cannot be retried. |
| Delivery Verification v4 | `275868e`, `0.2.34` | `INVALID_EVIDENCE`, `INCONCLUSIVE`. The zero-byte artifact fix retained the first Reviewer response and its exact receipt, but the call was non-retryable `invalid_output` because it was not a complete tool-free answer. Case 1 became structural, the other 31 cases never started, and zero matched pairs exist. The one-shot authority is consumed and cannot be retried. |
| GPQA matched diagnostic | `0.1.78` | On 12 frozen GPQA-Diamond questions, Direct scored `10/12`; Auto and Pro each scored `8/12`. This predates current code and is a reasoning diagnostic, not a current product baseline. |
| Fugu pilot v1 | `556d952`, `0.2.42` | Harness-link verification over the frozen 12-case GPQA-Diamond matrix: 36 runs started, 35 completed with scored answers (cindx_auto 8/12, cindx_pro 9/12, cindx_fast 11/12); one direct run exhausted the 300-second treatment deadline, and the fail-closed projection marked the cindx_fast cell ineligible, leaving the pilot not ready. The raw report scored 66.7% / 75.0% / 91.7% descriptively only. GPQA-Diamond had been retired as an evaluation direction long before this pilot and appears here only as the minimum-infrastructure harness-link vehicle; this entry reinstates nothing. This is pipeline verification plus a resource-boundary finding, not a parity, uplift, or model-quality claim, and the attempt is consumed; rerunning to rescue the ineligible cell is prohibited. |
| DashScope thinking-default diagnostic | `f0511e2`, `0.2.36` | Level 3 provider diagnostic. The configured `qwen3.8-max`, `deepseek-v4-flash-0731`, and `glm-5.2-fast-preview` models spend the bounded output budget on provider-side reasoning by default and return empty content; explicit `enable_thinking: false` restores bounded responses. This matches the empty-content failures observed in the consumed Delivery Verification v3/v4 Reviewer calls and motivated the shipped request-builder and probe fix. Credential rotation during measurement also produced 401 responses that an earlier harness mis-scored as model output; transport/credential failures must stay classified apart from model answers. No quality or uplift claim. |
| Verifier-repair narrow diagnostic | `8d6ce95`, `0.2.36` | Level 3 only. On a private 18-case frozen arithmetic suite, a deterministic fast generator produced clean output in 23/40 attempts; for the 17 deterministic defective samples, one independent fast-model repair round fixed 6/6 multi-case structural defects and only 3/11 single-case spec-ambiguity defects. Artifacts were not retained and the cases are not tracked, so this motivates the shipped bounded verification-repair round and the direct-judge contract but proves no product behavior change or uplift. |

Earlier Real-World versions, memory V1, GPQA/Core pilots, Fugu pilots, and
Workflow GEPA V2-V4/V9-V11 remain historical diagnostic material only. Their
failures are not silently converted into positive evidence.

## Current Product Gate

The evidence currently supports these statements:

- The direct/adaptive-direct path can preserve or narrowly improve the frozen
  V5 subset under its recorded revision.
- Durable memory helped the frozen matched direct tasks under its recorded
  revision and did not fail the decoy controls.
- Permission, run identity, recovery, task graph, context, memory, prompt
  serving, and evaluation boundaries have deterministic contract coverage.
- Current deterministic contracts require one typed strategy receipt before
  treatment execution and a matching terminal receipt before route evidence is
  accepted.
- The promotion system has rejected candidates that fail preservation,
  validation, lineage, or resource gates.

It does **not** support these statements:

- Auto or Pro generally outperform Fast.
- Multi-model Workflow currently improves product quality.
- GEPA has produced and deployed a better production profile.
- Auto-to-Pro transfer or distillation improves current Pro behavior.
- Cindx matches or approaches Fugu Ultra on an independent external benchmark.
- Deterministic green tests prove frontier intelligence.

## Fugu Comparison

The versioned contract is `benchmarks/fugu/fugu-v1.json`, with portable schemas
in `crates/orchestrator-eval`. It measures routing, diversity, evidence quality,
external effects, verification, efficiency, and failure handling. It is a
comparison harness, not parity evidence.

A parity claim requires independent tasks and evaluators, a frozen direct
control, equivalent provider and tool access, position balancing, complete
resource accounting, retained failures, safety qualification, and confidence
intervals. No current retained run satisfies that bar.

A bounded pilot authority is frozen but not executed. The suite is
`benchmarks/fugu/fugu-pilot-v1.json` and the protocol manifest is
`benchmarks/fugu/fugu-pilot-protocol-v1.json`, both validated by the
provider-free `fugu-pilot-contract` gate. The pilot reuses the pinned 12-case
GPQA-Diamond sample and the cindx_fast / cindx_auto / cindx_pro single-replicate
matrix, and it verifies only the case-to-observation-to-scoring pipeline. Its
purpose field is `harness_link_verification`; it forbids parity, uplift, and
promotion claims and carries `execution_authorized=false`. A provider-backed
pilot run is a separate explicit authorization step and must not revise the
frozen case authority, prompt-profile digests, run order, or budgets to fit a
result. `provider_backed_fugu_external_effect_pilot` remains the provider-backed
harness surface and is still `#[ignore]`-gated.

The pilot was then authorized and consumed once on source `556d952`. All 36
runs started; 35 completed with scored answers. The single failure was one
`direct_default` Chemistry run that exhausted the 300-second treatment
deadline before producing a parseable answer, so the fail-closed projection
marked the `cindx_fast` cell ineligible and the pilot not ready. The raw
report and its execution record remain private outside the tracked tree. No
parity, uplift, or model-quality claim is admitted; the pilot verifies the
case-to-observation-to-scoring link and surfaces a real resource boundary.
GPQA-Diamond had been retired as an evaluation direction before this pilot:
it was chosen only because it already carried pinned, license-clean case
provenance and a no-tool exact-match judge, and the pilot does not reinstate
it. Any successor authority must bind genuine agentic benchmarks. This
attempt is consumed, and a rerun to rescue the ineligible cell is
prohibited.

## GEPA and Self-Improvement

The product contains causal assignment, train/holdout gates, stable/canary
serving, rollback, transfer, and distillation contracts. Mechanism coverage is
not outcome evidence. A candidate may be admitted only when:

- the exact parent and treatment are frozen;
- candidate assignment and delivery receipts are complete;
- quality does not regress on preservation, train, or holdout tasks;
- safety, failure, and resource gates pass;
- the serving snapshot is bound to the same lineage;
- an independent test remains sealed until final admission.

No recent workflow candidate passed those gates. Production seed behavior was
not changed by V5, V7, or V12.

## Delivery Verification Experiment

The consumed v4 authority was frozen as a successor over the exact same v3
seeded-defect recovery and preservation suite under the default-off
`realworld-eval` feature. It does not compare treatment with naturally generated
Owner drafts. The 32-case suite freezes 24 defective candidates across
unsupported-claim, omitted-obligation, and contradiction strata plus eight clean
preservation sentinels. Calibration contains six defects and two sentinels;
model-hidden holdout contains 18 defects and six sentinels. V4 preserves the
case bytes, order, hidden oracle, seeded candidates, model inputs, output
contracts, budgets, decision thresholds, and no-retry policy from v3.

Control is the exact frozen seeded candidate. Treatment sends that same seed to
a model-distinct Reviewer. A `passed` verdict preserves it exactly;
`needs_revision` permits one Executor repair and one Reviewer recheck. There is
no initial drafting call, additional repair loop, transport retry, or case
replacement. The model-visible output contract specifies property names, JSON
types, requiredness, and the additional-properties rule. Exact semantic values
remain in the evaluator-only oracle.

Durably recorded provider timeout/unavailability and completed calls whose
Reviewer verdict JSON is invalid count as intention-to-treat treatment failures
for their fixed cases; they are never retried or replaced. V4 additionally
records the exact response artifact digest and byte count when content is empty,
then classifies completed empty Reviewer content as
`invalid_verifier_response` without a retry. A response that is not complete
and tool-free is structural, as are internal time-budget, binding, or authority
failures; either closes the campaign inconclusive.

An incomplete calibration or any structural/treatment-execution failure closes
`inconclusive` before the decision gate. Among eight complete, structurally
eligible cases, calibration opens holdout only with exactly six control
failures, at least four treatment-only wins, at least one win in each defect
stratum, and no control-only loss; an eligible calibration that misses those
thresholds closes `terminal_futility`.
The holdout result is
`seeded_repair_effective` only when all 24 cases complete with exactly 18
control failures, at least 13 treatment-only wins, at least four wins in each
defect stratum, zero control-only loss, and no structural/treatment-execution
failure. Thirteen successes among the 18 frozen defects has the protocol's
one-sided reference-null tail
`p=0.048126`. Any clean-sentinel loss is `preservation_regression`; an otherwise
valid sub-threshold result is `not_effective`. These labels describe only this
finite seeded component test.

V4 retains tool-free non-streaming requests, zero transport retries, one or three
calls per case, at most 96 logical calls / physical attempts, 64,000 tokens per
case, 2,048,000 campaign tokens, and a six-hour campaign ceiling. Because v4
references the same immutable suite, its retained experimental authorities are:

- v4 protocol manifest SHA-256
  `2f56a1dc2425cc7a930e45eb05a3c109f93a156ec54ddf999fc3b3e493016c5c`;
- suite SHA-256
  `5bd95ed736641f0120ceab038a01249ee246b4394e403fa5386ebe0e5de88bc7`;
- case-order and hidden-oracle aggregates
  `e2073c98fd878e5e50fda7d076b743b132dee705e40ea6ce26de8d7b31bb8bdc`
  and `1993e95ea7227d9ce65cd1c920d2f7698545341910b6f50d50070094220ec449`;
- seeded-candidate, model-input, and output-contract aggregates
  `55c5d576ffe2fd06d504c186e4227998b9695b28ab58c546c566e61ce1b954df`,
  `6c0b7692fa283c056ef5cabe93c3fd116fbffde3ec33a84b10e76539d75d53ed`,
  and `991d14b5f0d9b6c5e1269f8ad1c8001688769eba4040125eaf4f1b0eae24eb92`;
- budget aggregate
  `eb3ae14a6e0cd452b106f8c5bd85ad2575e204b0aee08ce1ce23b4a7ef007e11`.

The provider-free v4 preflight was the boundary between tracked design and its
executable frozen instance. It bound a clean source HEAD/tree and version;
all protocol, suite, case, seed, model-input, output-contract, budget, and oracle
authorities; redacted provider/model authority; the exact execute full-file
digest/size and verified SHA-256 CodeDirectory; and new canonical external
paths. Its receipt recorded zero provider calls,
`online_runner_frozen=true`, and `execution_authorized=false`; the later
short-lived authorization was separately bound and consumed exactly once. The
retained terminal result is described below.

### Consumed v1 result

The frozen v1 instance was authorized and consumed exactly once at source
`5373e654bd4ab0e5334a62ff199157f6b4daaf47`. Its retained public authorities
are:

- preflight receipt digest
  `47d9407465877994fe6994ed549456e334d011eb385dc5604e7dcd3947cf438e`;
- exact execute-binary digest
  `b6917770e39257dab4907e4202069b948c1a488da44075a958cdf961459722e8`;
- one-shot authorization digest
  `bca1c994e5c41eaee8c9c936c598315a8caf24dcaa316e07000bff091d375242`;
- terminal journal digest
  `67192fb14ad824d2967f20c54020060a498abacc7c6ee99a84b5c1e75a136070`;
- terminal receipt digest
  `df95c6a6913773d42de021271c1cbf499672042c510d728f551f74e3c8ccc414`;
- terminal evidence digest
  `ba2c9a6e6582c33a6eb542c5f4320c5debda66319a061ed9ffd97cb4927ebb00`.

The journal closed `CENSORED` / `INVALID-INSTRUMENTATION` during the first
calibration Owner call. The request reservation bound a canonical semantic
request digest, while the uncommitted provider result metadata supplied the
actual HTTP request-body digest under the separate
`cindx.model-provider.request-payload.v1` domain. Terminal validation
incorrectly required those two distinct digests to be equal. The provider-free
tests missed this because their fixture copied one synthetic digest into both
fields instead of crossing the real preparation boundary.

The journal charged one logical call, one physical attempt, and one 4,096-token
output reservation, but accepted zero terminal model-call receipts and no
usage, latency, provider identity, or case receipt. A response artifact was
written before terminal validation but is not bound by an accepted receipt and
is therefore excluded. No calibration decision or holdout call occurred, and
there are zero valid matched pairs. This result supports neither uplift nor
no-evidence, regression, answer quality, latency, usage, or cost claims. The
frozen v1 protocol is consumed and will not be rerun. Production finalization,
Workflow, Settings, serving, learning, promotion, GEPA, the installed App, and
the published release remain unchanged.

### Consumed v2 result

The frozen v2 instance was authorized and consumed exactly once at app version
`0.2.34`, source
`275d79b883192bf4148f13123e0d0de788aa346d`, and tree
`94cd9f3893cb752025fac6d3da9aef51821ba9ca`, with source-commit binding
`010885f207c6a68948b4192d7c748709b9b92dd83e9c09d6167c66cbadb4dc3b`.
Its retained public authorities are:

- manifest, suite, ordered-cases, hidden-oracle, and budget digests
  `3b1b758330403410486c28650f012b278859f680b55de23bc7aad0e09f6c4982`,
  `b9672f7075d896e3c48673607c622604c0f8d6fb2b6df4499281b0741124fbcd`,
  `8d4ab335142e655dd53d93511bf6b4da0bb559fb99ca87cb66aef0c47b872913`,
  `af47cdf41554956882e5fc42bd432a5c8dabff49573f2ce8f7181878e69d2b2d`,
  and `4b72b261ce347f7ec0ab80328f6ed57050a04260bf81c6f0930d9c08f5c710fe`;
- preflight receipt
  `caa4712d8faba84ce7532e830380f6ab4b8b20ad19f74b9c0d8c17c4189fab2b`;
- execute name/size, full-file digest, and CodeDirectory identity
  `cindx-delivery-verification-v2-execute` / `4,491,600` bytes,
  `eb405f2ae611a6b3eac42e2d75036feab44b51f369be697923cb42f9d0788663`
  and `f7e0f0b163c6647fc0490ac86d7502efe4ceacad9cf48e20a98024d72ecc0cbb`;
- output-authority, one-shot authorization, and consumed-marker/tombstone
  digests
  `c6c95cd8314fd6dd53dcea4526f2c9a21a8a3d7662401d6c7f446a5523fc8a77`,
  `c129a4d2a3dda43b350a2054d9887ae772353ceadd5b3068018fe43c3b945230`
  and `9d1f0fd8c134638881475e1acc6236e55f0474ac6f58f03c4d1f43bf73bf6b0e`;
- terminal journal
  `e7e4044f7c387059d146c1982dd8b7a1ead5705a6ba20cff563c16475efd71be`;
- calibration decision, terminal evidence, and terminal receipt digests
  `5e2376ba8f4903bdd7f8456754057776bcd8349113edf31721409c9ace829a18`,
  `cd67cef9e9e63e277e462c331c0acf261487078dac4e9eee627ae326c20c37a9`,
  and `45d6d953318dfb7248b820751ac5411ffd160b423cb9ebe39ba368e473f0d355`.

All eight calibration pairs completed. Seven were both-pass and calibration
ordinal 3 was both-fail; there were zero treatment-only wins and zero
control-only losses. The sole mismatch was in the frozen case definition: the
model-visible objective led to `controlling_revision`, while the hidden exact
JSON required the key `revision` (with revision 8). Both arms therefore failed
the same exact oracle even though the retained output followed the visible
contract. The frozen gate correctly recorded `terminal_futility`; all 24
holdout cases were skipped.

The journal retained 16 logical calls, 16 physical attempts, 16 terminal call
receipts, 49,152 reserved output tokens, and exact observed usage of 9,468
prompt + 8,865 completion = 18,333 total tokens. Those are execution-accounting
facts, not quality evidence. The case-definition mismatch, seven both-pass
pairs, and one both-fail pair establish neither treatment uplift nor regression.
V2 is consumed and must not be rerun or reinterpreted.

### Consumed v3 result

The v3 authority retained the tracked manifest SHA-256
`8da322b9336ec69d9f37bf12ce8d7698af55572d8d4173eddc45e6c2080cc8e8`
and the suite and aggregate authorities listed above. It was authorized and
consumed exactly once. Its first calibration Reviewer call was durably reserved,
and exactly one physical provider attempt returned an observed response whose
content was zero bytes.

The v3 terminal path constructed a response-artifact receipt with the digest of
the exact empty bytes and a byte count of zero. Journal validation treated that
valid zero length as an inconsistent artifact before writing the artifact or
committing the terminal call receipt. The execute path therefore froze the
campaign `CENSORED`. No case receipt, calibration decision, holdout call, or
valid matched pair exists. The attempt supports no answer-quality, uplift,
regression, latency, usage, cost, or provider-capability conclusion. V3 is
consumed and must not be rerun or reinterpreted.

### Consumed v4 result

V4 was frozen and executed once from clean merged source
`275868e4dd84692f15998a1cb95afa99df264267` with tree
`75f304e160c0b7bab37f5158b383fade970af271`. Its retained public authorities
are:

- preflight receipt digest
  `f0264540eeefde1c66e7933c521e60b129d235359d236253156d68df74a84635`;
- execute full-file SHA-256
  `0c22cf3df421b1d57e156ea38f69357562205abaebc3d2fc1b97a04211e43870`,
  size `4,574,288`, and CodeDirectory SHA-256
  `393179158e792eeaab8e9ab328f76823edcafc3c19fab5e2bdcdecbf05f2c1c2`;
- one-shot authorization digest
  `bbdf2bbb7e1e2c189f0eaa4ee0cc800806742a8ba430dfa71a8a4d6fd4f956f9`;
- consumed-tombstone digest
  `43befa6d8368d6f16925d9c193cd051374e712be6311ae33820f42cf07e12723`;
- terminal journal digest
  `272d3102c6118c9b1cc4d136c64d285a5629725543920a2f1922dae22e05d616`;
- terminal receipt digest
  `60a4b58c86cf9a556f5d5ea6ce4157f3e184c99f33a819c487a56bae4b4d7d6c`.

The first calibration case executed only `verifier_initial`. Its request
reserved semantic SHA-256
`63b740e67ec3ff9b23c0b92c2fe23ed810e7df743036d9c3f4cd550ac11457d6`
over 4,664 bytes and immutable wire SHA-256
`42748ae4890dc82a9608231e0f121257518531b9c476174f3f594b75200a017c`
over 3,389 bytes; the terminal request digest matched that wire authority. The
provider returned an observed receipt whose artifact was exactly zero bytes.
V4 correctly wrote the 0600 artifact with SHA-256
`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`
and committed the call receipt, so the v3 instrumentation failure was fixed.

The call nevertheless closed `invalid_output`, non-retryable, with reason
`delivery provider response is not a complete tool-free answer`. The journal
does not retain the underlying finish-reason subtype, so no more specific cause
is asserted. Exact provider usage was 840 prompt, 2,049 completion, and 2,889
total tokens over 19,755 ms; the reported completion count is one above the
frozen 2,048 output reservation and is retained as an anomaly, not silently
normalized. Case 1 closed `structural_failure`; its optional repair and recheck
were `not_required`. The campaign then closed `inconclusive` with one logical
call, one physical attempt, one terminal call, and zero retries. The other 31
cases remained unstarted, calibration and holdout decisions are null, and no
matched pair exists. The partial calibration counts are one started, one
terminal, zero complete, and one structural failure; control failures,
treatment-only wins, control-only losses, treatment-execution failures, wins in
each defect stratum, and preservation losses are all zero. Holdout was never
started rather than evaluated as a zero-result cohort. This is not
`seeded_repair_effective`, `not_effective`,
`preservation_regression`, or `terminal_futility`, and it supports no model-
quality conclusion. V4 is consumed and must not be rerun. GEPA and production
serving remain out of scope.

## Required Next Evidence

Do not rerun Workflow GEPA V12. The current source now links atomic strategy
selection and terminal transitions in one observable lifecycle for both
treatment arms, and the evaluation projector rejects missing, duplicate, or
mismatched lifecycle receipts. This is deterministic
instrumentation evidence only; it does not repair the frozen V12 attempt.

The production graph is now deterministically constrained to one Specialist,
an optional planned Independent Verifier, and a non-model Owner handoff. Matched
evaluation projection rejects a Direct arm with workflow-worker exposure, a
Workflow arm without its planned treatment, or any reintroduced anchor/reviewer
competition. This is structure and observability evidence, not quality uplift.

Goal 3A adds a provider-free shadow outcome contract shared by Direct and
Workflow. It derives bounded reward only from externally verified behavior
after lifecycle, actual treatment exposure, preservation, safety, and resource
receipts validate. Valid failures remain zero-score evidence; missing or
tampered provenance is censored. The receipt does not write production learning
evidence or authorize routing, prompt, memory, canary, or promotion changes.
This closes a reward-plumbing prerequisite only; no provider run or matched
uplift result was produced.

The Goal 3B/3C contracts add provider-free structured policy, attribution,
aggregation, offline admission, canonical replay, and crash-safe journal
boundaries. Under the successor-only `realworld-eval` path, policy assignment
is joined to the actual pre-dispatch request payload identity and size, Specialist/Verifier
attempts, derived stopping, and the Goal 3A outcome receipt. Candidate lineage
changes one bounded axis; valid failures remain in the denominator, while
incomplete attribution is retained as a censor instead of being silently
dropped. The external journal stores private immutable records, rejects
missing/tampered/forked chains, and recovers pending capture as
`IncompleteInstrumentation` without rerunning a provider. Train and sealed
holdout evidence can reach review readiness, but only an independently bound
review can authorize an offline narrow-validation record. Nothing is served or
promoted.

All Goal 3C evidence is deterministic and provider-free. V12 remains invalid
and was not rerun; no collaboration uplift or provider-cost result was
produced.

Goal 3D now freezes one successor in
`benchmarks/agent/collaboration-successor-protocol-v1.json`, bound by SHA-256 to
the new `collaboration-successor-v1.json` suite. The fixed matrix is three
matched pairs / six runs in this order:

1. a Direct-first training pair against the 5,000-bps, fail-fast Workflow
   baseline;
2. a Workflow-first training pair for the only candidate, which keeps the same
   topology, verification, and repair policy and changes only context to
   7,500 bps;
3. a Direct-first, training-ineligible holdout pair for that candidate.

The manifest permits one candidate and requires a valid positive baseline
before its pair, then a passing candidate training result before holdout. It
reuses the existing conservative Workflow evaluation run budget: at most ten
minutes, 20 logical model calls, 48 tool calls, 20 Agent turns, 80 physical
model attempts, and `83,886,080` accounted tokens per run. The six-run campaign
aggregate is capped at one hour, 120 model calls, 288 tool calls, 120 turns,
480 physical attempts, and `503,316,480` accounted tokens. Any censor,
non-positive baseline, safety or preservation failure, resource regression, or
missing final uplift terminates and freezes the collaboration type; a started
physical run cannot be retried by this protocol.

The tracked manifest explicitly sets `execution_authorized=false`. The
preflight binary validates a clean source tree, tracked protocol and case
digests, complete redacted provider/model bindings, case prestates, and new
private output paths, then writes a private receipt with
`provider_calls_performed=0`. It remains unable to authorize execution.

Goal 3E implements the missing once-authorized control plane without changing
that authority. A separate provider-free binary can mint one private 15-minute
authorization bound to the canonical preflight, exact execute binary, current
clean source and provider/model configuration, frozen cells and budgets, and a
new one-shot output root. The execute binary must consume that capability and
durably reserve the campaign, each cell, and each arm before model work. Any
expiry, drift, tamper, reuse, interruption, censor, non-positive baseline,
candidate failure, resource failure, or absent holdout uplift stops terminally;
recovery cannot retry a started physical run.

The exact Goal 3E instance was then authorized and executed once. Its sanitized
authority is:

- source `12a3ea2105029d9df30781b1d2bf0cc832855a29`, tree
  `22151a52f4906043b040ab6667f5e99a8b567b50`, version `0.2.32`;
- manifest `befe6ee366f5d91cfac024b8d79493a616776cfbf2990cd071f16912a2ae61df`
  and suite `12c6ac24d3bad5d99421f53bc7c42c2ee303941e55a31a95ff7af45ff4e44fc1`;
- preflight receipt `82d3cd899ffe793b3df5ef091fdc1379b10c3c009d097e6f6ee191dd14a56f87`,
  execute runner `90b37b722166fc2dc8b63f4bfa06a6ab5275b25c3b4438e7b13e35d50d1b4117`,
  and authorization `34a87b2a137d5a3e7b3d51e844c2f3f888b036f02a11fbc8930e558844800f53`;
- redacted provider identity `28db4f61122b0583b64308a476cc00923d81921719d5de4da270ed4daee1a043`,
  configuration `ad6971a91692069b7b0ba93cebac182695a4ff218afca5e654a8e1ad3272f9ee`,
  capture pool `73611876a040101a100423235ed40437bf45fcc70f62aa17ec4aa3ae30e8c734`,
  and ordered pool `d02e5f8be5b3cb4c28c13f18eba2c60ebadc0973a0134dcd670368707a22c404`;
- run, outcome, and campaign budget receipts
  `aacf624fb32fd3cb5c397f5548bb8b3f665e38799cc597eaa47894c7c329da31`,
  `59bf01edde1db9ed4219be9a1c687b9a10439e947ac19f4eedc4ae6f500e7f57`,
  and `fd9e4c89936743a2575940a81c1a720d72538fccca5bc9d3abed3f97956869f2`.

The terminal journal closed at revision 4 with tombstone digest
`7add0265028b0baf918bc3f314398cb8bf4eab63ae2ea00223ad5e2ace6b001e`,
journal digest
`04c838031c9bc43be82aec4e97954b26fb501ecf53f138de7afa8a6e87db95c7`
and terminal digest
`b6e66f754fa20980b7e5d61f493af846895b687995ccd65fcfab929f0c5dc003`.
It charged one run's maximum reservation but admitted zero observed runs and
zero observed calls, attempts, tokens, or duration. The baseline Direct arm was
reserved; its Workflow arm and both later cells remained unstarted. The
product run terminated before selection with zero selected decisions. Its
terminal producer correctly persisted the explicit pre-decision `not_selected`
state; the treatment was never selected and Owner did not execute. The
selected-only outcome projector misclassified that legal state as a malformed
receipt and failed closed before a valid Pair/Censor observation could be
formed. This is instrumentation evidence only: Goal 3E is consumed,
`CENSORED`, and must not be rerun. Current source now classifies that legal
pre-decision state explicitly and covers the real terminal-producer-to-outcome-
projector seam; this corrects the diagnostic but cannot turn the consumed run
into evidence. No Goal 3F or provider run is authorized. V12 remains unchanged
and invalid.

## Running Evaluation

Deterministic contracts are included in the profiles documented in
[DEVELOPMENT.md](DEVELOPMENT.md). Frozen inputs live under:

```text
benchmarks/agent/
benchmarks/fugu/
benchmarks/system/
```

Provider binaries are feature-gated in the desktop crate. They must not be run
without explicit authorization, a clean pinned revision, a preflight-only pass,
and a bounded output directory outside the tracked tree. Sanitized summaries
must record version, commit, provider, cases, treatment, budgets, failures,
receipts, evidence digest, confounds, and decision.

Historical report retrieval example:

```sh
git show f2ce3b7:docs/evaluations/README.md
```
