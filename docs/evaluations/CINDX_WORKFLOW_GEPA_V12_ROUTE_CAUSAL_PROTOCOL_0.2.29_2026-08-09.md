# Cindx Workflow GEPA V12 route-causal protocol 0.2.29

## Status

`INVALID_EVIDENCE`

V12 preserves V11's causal question: with the same Pro conductor candidate,
workflow proposal, route profile, task, workspace, model set, and budget, does
executing the proposal as a Workflow produce externally verified value over
projecting that exact proposal onto Direct execution? GEPA may mutate only
route guidance, and only after a positive matched Workflow treatment.

V11's one authorized run was `INVALID_EVIDENCE` because its first failed
product arm had complete route-related observations but the generic strategy
projection also required a direct-finalizer terminal-completion receipt. V12
changes only that evidence ownership boundary:

1. `StrategyReceipt` contains route and task-graph identity only.
2. Direct-finalizer assignment, request, delivery, and projection errors are
   recorded independently on the raw product run.
3. Route evidence still fails closed on missing or inconsistent strategy,
   execution-plan, provider, tool, verification, budget, shared-plan, or
   treatment receipts.
4. Failed, paused, timed-out, denied, or interrupted product runs remain in the
   matched denominator and receive zero externally verified behavior score.
5. The route campaign makes no direct-finalizer phenotype or uplift claim from
   the independent diagnostic.

| Field | Frozen value |
| --- | --- |
| Application version | `0.2.29` |
| Evidence schema | `cindx.workflow-gepa-product-evidence.v12` |
| Suite ID | `cindx-workflow-gepa-v7` |
| Suite SHA-256 | `7f289521669a612f2577888c84c5b364b0cd076179f4776ca20b0bf7c97fcdab` |
| Cases | 8 frozen cases: 2 train, 2 validation, 4 untouched test |
| Learned field | `route_directive` only |
| Candidate population | 2 distinct route policies from at most 6 proposals |
| Maximum product runs | 33 planned, below the hard cap of 35 |
| Executed source commit | `ff8c238d4edd7131c4ac0409c89535712c1fe56b` |
| Journal SHA-256 | `9cb476f4af4bb3bc3b9b971f29632f6ed683fb4b3c39f75c9bc7102e184660ed` |
| Reserved / completed product runs | `2 / 1` |
| Completed product usage | `9` model calls; `54,579` tokens |
| Journal state | `running` with the Workflow arm pending; replay is forbidden |

## Observed execution result

The single authorized V12 execution reached both arms of the first frozen case,
`coding-reconcile-records`. The Direct arm completed its journal action with a
`failed` product terminal, nine model calls, 54,579 tokens, nine external check
receipts, and complete route/task-graph evidence. This confirms that V12 fixed
V11's evidence-ownership failure: a failed terminal no longer erases an
otherwise projectable route receipt.

The counterbalanced Workflow arm then failed before an `Agent run decision
selected` event was persisted. Its raw run therefore had no strategy receipt,
execution-plan receipt, or model receipt, and failed closed with:

`agent strategy receipt is missing`

The durable journal contains the completed Direct action and the reserved
Workflow action, but the current failure path does not persist the pre-strategy
product error that caused the Workflow run to stop. There is no report,
snapshot, matched pair, candidate, mutation, validation, test, promotion, or
GO/NO-GO result. Raw provider output was not inspected or committed. V12 must
not be replayed.

## Frozen campaign and gates

The execution order, counterbalancing, shared private plan anchor, train-only
candidate selection, unseen validation, Grounded Direct control, two-repeat
untouched test, resource ceilings, safety gates, and durable external journal
are identical to V11. Maximum complete usage remains
`4 + 8 + 4 + 1 + 16 = 33` product runs.

The campaign stops before mutation with a valid no-go if Workflow has no
externally verified training win over Direct. Later validation and test gates
retain the preregistered quality, completion, safety, latency, token, causal
profile, and route-semantics thresholds recorded by V11. Only a complete pass
may publish an external candidate snapshot; it is not installed into the
shipping profile.

## Interpretation

The clean frozen revision was executed once. The completed Direct action and
pending Workflow reservation remain in the external hash-chained journal. There
was no result-driven prompt edit, case replacement, threshold change, or rerun.

- A valid no-go is evidence that the frozen collaboration/learning path did not
  improve this suite; it is not evaluator failure.
- A complete gate pass is targeted product evidence only, not broad superiority
  or Fugu Ultra parity.
- Missing route, shared-plan, treatment, provider, journal, or external
  verification identity remains invalid evidence.

Production Fast, Auto, and Pro routing, budgets, prompt serving, UX, and safety
policy are unchanged by this invalid evaluation attempt. A successor requires a
bounded product-run failure receipt that survives errors before strategy-event
persistence; this result does not justify another provider campaign by itself.
