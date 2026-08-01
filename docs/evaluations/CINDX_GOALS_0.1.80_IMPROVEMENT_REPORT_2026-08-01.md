# Cindx 0.1.80 Goal Improvement Report

## Verdict

This release completes five bounded engineering goals without changing the
visible product workflow. It improves runtime ownership, collaboration
calibration, GEPA evidence isolation, dependency cost, and production security
boundaries. The integrated release gate passed and the application was built
and installed locally.

No provider-backed quality evaluation was run for this release. Therefore this
report does not claim an intelligence uplift, GEPA quality uplift, or Fugu Ultra
parity.

## Goal Results

### 1. Owned agent harness state

Commit: `308b2d8c5bea0c3f1eca728e711d111bcef524b5`

- Moved run ownership and suspended-run lifecycle state into the
  `agent-harness` boundary instead of leaving it distributed across desktop
  globals.
- Gave the session output cache an owned store and narrowed project/session
  lifecycle coupling.
- Added structure and integration-boundary checks so the ownership split cannot
  silently collapse back into desktop glue.

### 2. Evidence-calibrated collaboration

Commit: `c012cab8a9f7754fb8eea18b3be45e2be09b67e1`

- Made collaboration decisions compare their expected value with direct-model
  evidence instead of treating extra branches as automatically beneficial.
- Added topology-learning and workflow-revision evidence to routing decisions.
- Preserved the direct result as the quality anchor and added tests for the
  revised selection behavior.

This is an engineering prerequisite for better collaboration, not proof that
Auto or Pro now outperform Direct/Fast.

### 3. GEPA search and holdout isolation

Commit: `3cbb1205321a504b441431839ab011eefda122b3`

- Separated prompt search observations, fitness calculation, Pareto selection,
  and search operations into owned modules.
- Prevented holdout evidence from entering the search path that proposes prompt
  candidates.
- Retained promotion as an evidence-gated step rather than reporting generated
  prompt variants as learned improvements.

### 4. Dependency and build-cost reduction

Commit: `673374ba0b41e17de588ca66843a227e0bc783d9`

- Consolidated duplicate desktop transport dependencies and removed 350 lockfile
  and manifest lines net from the affected change.
- Added dependency-policy checks to CI and the desktop verification gate.
- Kept the shipping transport on the supported dependency path.

The final clean release still took 17 minutes 5 seconds. Lance/DataFusion and
release LTO remain material build-cost sources, so build-cost work is improved
but not complete.

### 5. Production security and release integrity

- The real desktop tool-permission branch now has focused production-path tests
  for no grant, exact session reuse, and cross-session rejection.
- Permissionless isolated-worker reads now require all three conditions:
  read-only risk, read-only declared effect, and no permission request.
- The patched `event-listener` production dependency is enforced at version
  `5.4.2` or newer. The normal macOS build graph uses `quick-xml 0.41.0`.
- CI and release workflows now fail on moderate-or-higher npm audit findings.
- Vite, esbuild, and PostCSS were updated or pinned to audited versions; npm
  reports zero known vulnerabilities for the resolved frontend graph.

## Verification

The single final release gate passed:

- 5 release-version tests.
- Browser and computer sidecar integration checks.
- 141 frontend tests.
- Full locked Rust workspace test suite.
- 522 desktop Rust tests: 515 passed, 7 explicit provider/network diagnostics
  ignored, 0 failed.
- Focused tool-registry and production permission-path regression tests.
- Frontend production build and Apple Silicon Tauri release build.
- Shipping performance checks for session projection, runtime snapshot,
  graph-cache reuse, image preparation, transport preparation, and large
  Markdown handling.
- Bundle signing verification and local installation verification.

Installed bundle: `/Applications/Cindx.app`, version `0.1.80`.

Distribution archive: `Cindx-0.1.80-macOS-arm64.zip`.

SHA-256:
`4493c7aa6e2c35d7627e7f9eabfbbc3a70ab46603c43c4dbe8f0927cc83dc183`.

## Residual Risks

- Provider-backed matched evaluation was intentionally not run, so no product
  intelligence claim is supported by this release.
- The normal release build still compiles Lance-related testing/benchmark
  dependencies. Their reachability and removal need a separate measured goal.
- The Rust dependency graph still reports the unmaintained `paste` advisory via
  DataFusion; no patched upstream version is currently available in the locked
  graph.
- The debug test linker still emits the existing macOS `__eh_frame section too
  large` warning.
- Vite still reports lazy diagram chunks above 500 kB. They need interaction and
  startup profiling before dependency removal or chunk changes.

These residuals are recorded rather than converted into unsupported passes.
