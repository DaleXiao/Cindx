# Cindx Dynamic Collaboration V1 0.2.27

## Decision

`VALID_TARGETED_EVIDENCE`, `NO_GO_NOT_EXERCISED`.

The frozen eight-cell matrix completed once on source commit
`08b746907661481f5077833a9e2e324e929a9567`. Both Grounded Direct controls and
all four Auto/Pro product cells completed with full quality, full external
effects, and zero safety violations. Every Auto and Pro cell nevertheless
executed Direct. No cell materialized or completed a receipt-bound Workflow,
so the preregistered collaboration gate was not exercised and no incremental
value claim is admitted.

The generic real-world analyzer labels the machine-readable evidence
`VALID_BASELINE`; that label means the matrix and receipts are structurally
usable. It is not the targeted protocol decision above.

## Frozen Evidence

| Field | Value |
| --- | --- |
| Protocol | [Dynamic Collaboration V1 frozen protocol](CINDX_DYNAMIC_COLLABORATION_V1_PROTOCOL_0.2.27_2026-08-09.md) |
| Suite | `cindx-conductor-ownership-holdout-v1@1` |
| Suite SHA-256 | `820594e695101612b7d1c05443a015986c35a1b6704371168ebbd1b89858c00b` |
| Git commit | `08b746907661481f5077833a9e2e324e929a9567` |
| Matrix | 2 cases x 4 treatments x 1 replicate |
| Retained cells | 8 / 8 |
| Product strategy receipts | 6 / 6 |
| Provider evidence incomplete | 0 |
| Strategy evidence incomplete | 0 |
| Safety violations | 0 |
| Raw evidence SHA-256 | `ce22b8668416909466d3541631a1f4d4a9b46bbe4eeff2ff1dfdf7c81c3f4092` |

Raw prompts and provider outputs remain outside Git. The sanitized
machine-readable evidence is retained in
[`CINDX_DYNAMIC_COLLABORATION_V1_0.2.27_2026-08-09.json`](CINDX_DYNAMIC_COLLABORATION_V1_0.2.27_2026-08-09.json).

The first launcher invocation failed before any provider call because the
non-interactive process could not resolve `cargo`; it created no checkpoint.
After supplying the installed Rust toolchain path, the frozen provider matrix
was executed once without reruns, case edits, profile mutation, or discarded
cells.

## Treatment Results

| Treatment | Complete | Quality | External effect | Safety | Median latency | Total tokens | Executed Workflow |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Oracle Reference | 0 / 2 | 0 / 2 | n/a | 0 | 120,013 ms | 0 | n/a |
| Grounded Direct | 2 / 2 | 2 / 2 | 2 / 2 | 0 | 124,502 ms | 176,046 | 0 |
| Auto | 2 / 2 | 2 / 2 | 2 / 2 | 0 | 199,558 ms | 197,317 | 0 |
| Pro | 2 / 2 | 2 / 2 | 2 / 2 | 0 | 174,731 ms | 168,301 | 0 |

The two Oracle Reference cells reached the frozen collaboration-stage deadline
and remain failed in the denominator. They are a no-tools reference ceiling,
not the product baseline. Both Grounded Direct controls remained valid.

## Preregistered Gates

| Gate | Result | Evidence |
| --- | --- | --- |
| Complete, receipt-valid matrix; zero safety violations | PASS | 8/8 cells retained; provider and strategy evidence complete; 0 safety violations |
| Grounded Direct control | PASS | Both controls completed and passed every quality and external-effect check |
| Auto/Pro product non-regression | PASS | All four matched product cells preserved completion, quality, external effects, and safety |
| Receipt-bound Workflow execution | **FAIL** | Auto selected Direct twice; Pro selected Direct twice; no proposal or materialized-plan digest exists |
| Matched incremental quality or completion win | **FAIL** | Grounded Direct passed both cases, and Auto/Pro tied it on every quality and completion check |
| Frozen resource ceilings | PASS | Auto: `1.6028x` median latency and `1.1208x` tokens; Pro: `1.4034x` median latency and `0.9560x` tokens |

Because the Workflow-execution gate failed, the protocol classification is
`NO_GO_NOT_EXERCISED`. The absence of a matched win independently prevents
`GO_INCREMENTAL_VALUE`.

## What Changed And What Did Not

The evaluated revision removes the split-brain planning path: one Conductor
response can now carry both the route decision and a typed executable Workflow
proposal, and the runtime binds proposal and materialized-plan digests into the
strategy receipt. Deterministic tests prove validation, fallback, and receipt
integrity. This provider run shows that the configured Conductor did not choose
that path on either frozen route-blind task.

The result therefore supports keeping the structural fix but does not support
claiming better intelligence, useful multi-model collaboration, GEPA uplift,
memory uplift, broad generalization, or Fugu Ultra parity. The next experiment
must first explain why the Conductor repeatedly selects or degrades to Direct;
it must not tune these two evaluated cases or rerun this matrix as fresh
evidence.
