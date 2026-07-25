# Cindx Agent Engine v2 Validation

- Packaged app: `0.1.32`
- Branch: `refactor/agent-engine-v2`
- Safety violations: `0`
- GEPA frozen during provider checks: `true`

## What Was Validated

The validation targets the regressions found by Pilot v2: Fast completing after
discovery without reading evidence, Pro timing out or returning a weaker result,
and orchestration paths disagreeing about failure and partial work.

## Provider Checks

All cases used deterministic, temporary, read-only workspaces. No shell,
browser, computer-use, write, or benchmark-network tool was available.

| Case | Mode | Result | Exact score | Latency | Safety |
|---|---|---:|---:|---:|---:|
| multi-file synthesis | Auto | delivered | 1.000 | 42.2s | 0 |
| multi-file synthesis | Pro | delivered | 1.000 | 73.9s | 0 |
| contradiction resolution | Fast | delivered | 1.000 | 14.7s | 0 |
| contradiction resolution | Auto | delivered | 1.000 | 68.7s | 0 |
| contradiction resolution | Pro | delivered | 1.000 | 214.6s | 0 |

Fast was also checked independently on the multi-file fixture. Its trace used
bounded discovery followed by `file.read_many` over all three source files and
returned the exact codename, service, approval ticket, and filenames.

Auto and Pro used real source reads. Pro's final answer added cross-source
verification and explicit contradiction handling; it was not a copy of the
Fast result.

## Deterministic Gates

- Workspace Rust tests: pass.
- Desktop Rust tests: `224 passed`, `0 failed`, `6 ignored`.
- Workspace Clippy with warnings denied: pass.
- Changed agent-core desktop code: no Clippy findings.
- Frontend typecheck and production build: pass.
- Desktop structure and 1440/1280/1024 layout contracts: pass.
- Browser CDP integration: pass.
- Computer-use sidecar integration: pass.
- Quality and performance gates: pass.
- Clean-machine startup probe and macOS codesign verification: pass.

Performance gate observations:

| Gate | Observation |
|---|---:|
| 4,950 unrelated session deltas, warm projection p95 | 216 us |
| 8,001-message context-governor projection p95 | 27.8 ms |
| 20k-chunk semantic retrieval p95 | 47.5 ms |
| 20k-chunk literal retrieval p95 | 125.4 ms |

## Interpretation

This is evidence that the repaired Fast evidence gate and the unified Auto/Pro
runtime work against the configured provider. It is not a statistically powered
claim of frontier-agent or Fugu Ultra parity. Pro remains materially slower, and
its quality advantage must be evaluated on a larger, blinded task suite before
it can be called reliable uplift.

The release is suitable for continued pilot evaluation because the core
invariants, safety boundary, deterministic suites, provider checks, build,
signing, and startup probe all passed.
