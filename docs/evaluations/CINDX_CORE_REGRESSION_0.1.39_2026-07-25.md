# Cindx Core Regression Evaluation 0.1.39

- Candidate: `0.1.39`
- Base commit: `2d0cbc2`
- Evaluation date: `2026-07-25`
- Provider comparison: targeted, GEPA frozen
- Safety violations: `0`

## Decision

The two persistent Pilot v2 harness regressions from `0.1.31` are recovered in
targeted provider-backed reruns. The deterministic control plane and scaling
diagnostics are green. This is sufficient to release the kernel correction, but
it is not a statistically powered Fugu Ultra parity claim and does not replace a
new 32-cell Pilot matrix or the 1,440-observation Agent Arena.

## What Changed

| Area | Kernel change | Regression contract |
|---|---|---|
| Agent loop and harness | One typed run-control budget now governs model, tool, stage, terminal-reserve, cancellation, retry, continuation, and partial-output behavior. A usable answer at the soft turn boundary is retained; retries and new tool calls cannot consume the terminal reserve. | Turn-budget, no-progress, cancellation, steer, permission, continuation, terminal-commit, and partial-output tests. |
| Task graph | Runnable, running, resumable, degraded, exhausted, and dependency-blocked states are derived by the shared orchestrator graph. Production and prompt evaluation use the same deterministic frontier and claim semantics. | Frontier, claim, resume, degraded dependency, exhausted root, and blocked dependent tests. |
| Context engine | Supplemental context is selected by current-objective coverage and provenance under a bounded token budget. Canonical conversation history is not mutated. | 8,001-message scaling diagnostic and context provenance tests. |
| Memory | Incomplete runs may preserve user requirements but cannot create completed outcomes. Recall combines lexical and semantic evidence, diversifies sessions, preserves trust labels, and downranks repeatedly recalled but unused records. | Memory production, deduplication, trust, semantic calibration, usage attribution, and incomplete-run tests. |
| Retrieval | Four-channel retrieval remains independent. Fusion rewards query and identifier coverage plus independent channel consensus while preventing correlated graph routes from double counting. | Identifier coverage, consensus, graph-family calibration, file fallback, and four-channel tests. |
| GEPA | Reflection receives redacted actionable failures, retries, tool traces, and verifier feedback. Failed or fail-soft cancelled branches cannot become positive training evidence. Pareto selection prioritizes quality, safety, generalization, task coverage, and latency; model price is not an objective. | Paired evidence, holdout, Wilson confidence, rollback, instance Pareto, replay deduplication, and safety tests. |
| Fugu-style execution | Capability-aware topology learning chooses bounded collaboration from matching task signatures. Pro is fail-soft at branch quorum but keeps strict safety and terminal quality gates. Read-only exploration receives a discovery, expansion, and evidence round without slowing text-only Fast requests. | Real GPQA Pro and Fast multi-file workspace reruns plus deterministic topology and tool-budget tests. |
| Module boundaries | Task-graph semantics live in `crates/orchestrator`; run-control and context governance live in `crates/agent-runtime`; memory and retrieval ranking stay in their crates. Evaluation feedback was removed from the execution loop into `prompt_evaluation_feedback.rs`. | Workspace dependency direction plus full Rust tests. |

## Provider-Backed Regression Results

| Persistent 0.1.31 case | Previous result | 0.1.39 candidate | Interpretation |
|---|---:|---:|---|
| GPQA `reckEnrOPFT9Ru7tW`, Pro | no delivery, 300.0 s, score 0.0 | delivered, 287.5 s, exact 1.0, answer `A` | A correct forward branch and final synthesis survived failed/cancelled stragglers. The final answer was correct, but the internal quality gate remained false because two intermediate steps did not succeed. |
| `workspace-multi-file-synthesis`, Fast | delivered, 19.9 s, score 0.4 | delivered, 15.5 s, exact 1.0 | Fast remained single-model. A capability floor supplied three bounded evidence rounds and six read-only calls for unknown-directory exploration; text-only Fast remains one model round. |

The GPQA run used 34,568 total tokens. Its forward analysis and synthesis steps
succeeded; reverse analysis exhausted its turn budget and the verifier was
cancelled. The correct answer is therefore evidence of recovered anytime
delivery, not evidence that every Pro branch passed.

The workspace run used 11,290 total tokens and exactly six read-only calls:
three directory listings followed by reads of `release/manifest.toml`,
`release/approvals.md`, and `notes/draft.txt`. It returned `Silver Current`,
`relay-gateway`, and `SAFE-2718` with the correct source files.

Raw evidence was retained outside Git during evaluation:

- GPQA SHA-256: `89c1bfea1a164f20e79f16d1d3f8adb52869eb81bd3c854ccc25569c30897c5d`
- Workspace SHA-256: `960600552e17f2a92886a28f960e3c36b891e506b3f138081d36a68feaa6f3fb`

## Deterministic Regression

- Full quality-gate profile: `13/13 passed`, including browser/computer
  sidecars, routing, memory, workspace, desktop, frontend, release, layout, and
  architecture contracts.
- Desktop Rust: `224 passed`, `6 ignored`, `0 failed`.
- Shared Rust workspace: `372 passed`, `2 ignored`, `0 failed`.
- Frontend production build: passed (`3,856` modules transformed).

## Scaling Diagnostics

| Workload | Result |
|---|---:|
| Session projection, 1,000 initial + 4,950 unrelated events | 1 delta event read, 0.131 ms p50, 0.187 ms p95 |
| 20,000 chunks, 64 dimensions, semantic search | 46.7 ms sample, 47.2 ms p95 |
| 20,000 chunks, literal search | 112.5 ms sample, 113.5 ms p95 |
| 8,001 messages / 1.18 MB context governance | 26.5 ms p50, 27.0 ms p95, 29.1 ms max |
| Context projection | about 467,894 estimated tokens to 27,004 tokens, preserving 417 messages |

These timings are same-machine diagnostics, not portable product latency claims.

## Fugu Ultra Boundary

Cindx now implements the engineering mechanisms that can be evaluated locally:
typed durable execution state, bounded multi-model DAGs, role-specific workers,
fail-soft anytime delivery, verification, evidence isolation, contextual topology
learning, and GEPA-style prompt/harness evolution with hidden-split promotion
gates. It does not reproduce Sakana's private training data, model weights,
reinforcement-learning procedure, or unpublished production conductor. The honest
claim is improved execution-equivalent behavior, not weights-level parity.

## Remaining Evidence

- Rerun the complete 32-cell Pilot v2 matrix before changing the checked-in
  full-evaluation gate from its previous `NO-GO`.
- Populate the frozen 1,440-observation Agent Arena before making broad quality
  rankings across Fast, Auto, Pro, a single model, or Fugu Ultra.
- Keep `quality_gate_met` separate from delivery success in future Pilot reports;
  a correct degraded answer is useful but must not train GEPA as a clean win.
