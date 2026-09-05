# Development and Release

## Prerequisites

- macOS on Apple Silicon for the current local release path.
- Xcode Command Line Tools.
- Stable Rust through `rustup` with `aarch64-apple-darwin`.
- Node.js and npm.
- Optional provider credentials for explicitly authorized provider tests.
- macOS Accessibility/Screen Recording permissions for computer control.

Install frontend dependencies once:

```sh
cd apps/desktop
npm ci
cd ../..
```

Use `scripts/install-desktop-deps-ipv4.sh` only when ordinary npm resolution has
an IPv6/DNS problem.

`npm audit --audit-level=moderate` is a blocking CI and release step. Transitive
build-tool advisories are pinned patched in `apps/desktop/package.json`
`overrides` (for example `browserslist` for GHSA-c83g-rgw3-j3cx /
GHSA-73wf-gq98-2v4g); bump the override and refresh `package-lock.json` rather
than relaxing the audit level.

Managed `.cindx` artifacts (tool-result images and large structured output,
attachments, output-history snapshots, and shell stdout/stderr captures) are
written through the symlink-safe primitives in `crates/tools/src/safe_fs.rs`.
Every write validates each path component against the trusted workspace root,
refuses any symbolic-link component or leaf, forces owner-only modes through
`O_NOFOLLOW` descriptors, and publishes files by atomic temp-plus-rename, so a
hostile workspace that pre-plants `.cindx/...` as a symlink cannot redirect a
write or a recursive chmod outside the project. The online provider eval example
(`crates/agent-eval/examples/real_provider.rs`) is likewise contained: case ids
must be a single safe slug, each run uses an exclusive temp root, and the example
refuses to run without `CINDX_EVAL_AUTHORIZE=yes`. `ensure_private_dir` also runs
a bounded, once-per-directory, best-effort migration that forces pre-existing
legacy files in a managed leaf to owner-only (0600), skipping symlinks and
subdirectories, so world-readable artifacts left by older builds converge on the
next managed write without a recursive chmod of an arbitrary tree.

## Fast Checks

Run documentation and static structure checks after every relevant change:

```sh
node scripts/check-docs.mjs
node scripts/check-gates-manifest.mjs
node scripts/check-desktop-structure.mjs
node scripts/check-desktop-layout.mjs
git diff --check
```

`check-gates-manifest.mjs` parses every tracked benchmark contract and checks
the quality-gates manifest for duplicate gate ids, dangling profile
references, orphaned gates, and the four required profiles. It is the first
CI step, so a merge-corrupted JSON file fails the pipeline before any cargo or
npm work begins.

The normal repository checks are:

```sh
scripts/check.sh
scripts/check-frontend.sh
scripts/check-desktop.sh
```

`scripts/check-desktop-rust-light.sh` is a quick desktop compile surface without
the frontend bundle and heavy vector dependency graph. It is feedback, not a
shipping gate. `scripts/check-rust-quality.sh` is the mandatory formatting and
Clippy gate used by CI.

## Merge Discipline

Stacked feature branches must be rebased onto the integration branch and have
their conflicts resolved locally before merge. Resolving conflicts against
tracked machine-read manifests (notably `benchmarks/system/quality-gates-v1.json`)
through a web editor or by hand without re-running the fast checks is how a
syntactically invalid manifest reaches `main` and breaks every quality gate.
After any merge that touches `benchmarks/`, run `node scripts/check-gates-manifest.mjs`
(and `node scripts/check-docs.mjs`) before considering the merge done.

## Quality Profiles

The source of truth is `benchmarks/system/quality-gates-v1.json`.

| Profile | Purpose |
| --- | --- |
| `quick` | Documentation, version, layout, structural, workspace-undo, project-instruction, and custom-command contracts |
| `ci-contract` | Quick checks plus deterministic routing, context, memory, task, tool, and evaluation contracts |
| `control-plane` | Deterministic agent contracts plus Rust workspace and desktop tests |
| `performance` | Same-machine diagnostics for long sessions, context, RAG, streaming, and provider health |
| `paired-performance` | Base/head P95 comparison on the same machine and toolchain |
| `shipping-performance` | Resource and operation-count gates used by production builds |
| `full` | Shipping deterministic gates, sidecars, frontend build, and Rust tests |

Run a profile and retain its report outside Git unless it is an approved,
sanitized decision artifact:

```sh
node scripts/run-quality-gates.mjs \
  --profile full \
  --report target/quality-gate-report.json
```

The `ci-contract`, `control-plane`, and `full` profiles include the
`agent-strategy-lifecycle-contract`, `agent-strategy-control-contract`, and
`agent-terminal-lifecycle-contract` gates. Together these checks cover
preparation and start linearization, selected-decision recovery,
success/failure/cancellation terminal linkage, exactly-once replay, and
evaluation fail-closed behavior without contacting a provider. The former
`agent-execution-graph-contract` (which projected matched Direct/Workflow
attribution) is retired along with the workflow machinery it guarded.

The lifecycle filters include the preparation failure race against cancellation
and steer, including exactly-once terminal persistence for the winning epoch.

The same profiles include `agent-outcome-evidence-contract`. This provider-free
gate checks exact lifecycle, treatment-exposure, and resource bindings;
integer positive, partial, and negative scoring from external postconditions;
zero-score retention for valid safety or preservation failures; tamper
censoring; and isolation from production learning consumers.

`direct-judge-outcome-contract` runs ten provider-free agent-application
tests over the shadow outcome projection wired at terminal finalization and
its review-admission contract. It pins all twelve recorded
`direct_judge_disposition` values to their typed families with unknown values
failing closed, checks the monotone integer reward rule (verified pass, pass
with unverified mutations under a required verification policy, judged
failure, censored unjudged), rejects tampered or schema-drifted receipts, and
proves the fitness-signal summary window stays deduplicated, bounded,
censor-aware, and permanently promotion-ineligible. It also pins the
independent review receipt: admission requires an approving receipt whose
window digest binds the exact signal window in order, and the resulting
admission record hardcodes `production_eligible=false` and
`promotion_eligible=false`. The desktop recorder appends signals to a private
capped journal best-effort and never disturbs delivery; the prompt-evolution
admission read path (`prompt_evolution_admission_runtime`) has been physically
removed with the prompt-genome mechanism. It is included in `ci-contract`,
`control-plane`, and `full`.

`workspace-undo-contract` runs 18 provider-free desktop tests covering undo
entry projection from tool events, undo/redo of created, overwritten, and
patched files, `file.patch_batch` group projection plus whole-group undo/redo
(with any externally edited member blocking the entire group restore),
disclosure of a capture failure as a not-undoable entry, external-edit
conflict blocking, LIFO ordering, undo registry persistence, and the
failure-recovery protocol: a group restore that fails partway rolls its
already-written files back (undo and redo alike) and stays retryable, and
successful restores publish through a temporary-file rename without leaking
temp artifacts. It is
included in `quick`, `ci-contract`, `control-plane`, and `full`.

`project-instructions-contract` runs 17 provider-free desktop tests covering
bounded discovery of workspace instruction files (`AGENTS.md` from the
workspace root up to the Git root and `.cindx/instructions/*.md`), per-file
and total byte caps, receipt digests, glob validation, the untrusted-guidance
boundary text, and stale-context removal on re-preparation. It is included in
`quick`, `ci-contract`, `control-plane`, and `full`.

`custom-commands-contract` runs 9 provider-free desktop tests covering
markdown command discovery, frontmatter parsing and effort normalization,
project-over-global name precedence, and file-count and byte caps. It is
included in `quick`, `ci-contract`, `control-plane`, and `full`.

`plan-mode-contract` runs 16 provider-free desktop tests covering the
High/Xhigh-only plan-mode gate (Fast/Default never activate it), the bounded
read-only plan-drafting loop (whitelist enforcement, step bound, Worker-stage
budget charging, cancellation), the user-confirmed plan context injection with
its provenance receipt and untrusted-guidance boundary (which never writes a
scheduling key), the paused pending-confirmation projection, and the
plan-aware cancel guard. It is included in `quick`, `ci-contract`,
`control-plane`, and `full`.

The portable collaboration-learning contracts (`agent-collaboration-learning-contract`,
`agent-collaboration-learning-offline-contract`, and the retired
`agent-collaboration-learning-offline-adapter-contract`) have been removed along
with the `collaboration_learning_*` modules in `agent-application`: multi-model
collaboration is permanently retired, so the policy/projection/holdout/admission
contracts had no production consumer. Learning evidence that remains is the
terminal `learning_evidence_v1` used by memory extraction/retention and
recall-to-terminal attribution, which is unrelated to collaboration learning.

The `realworld-eval`-only collaboration-successor and desktop
delivery-verification gates were removed in the phase-3 effort-tier rebuild
along with the `realworld-eval` feature and binaries. The paragraphs below are
retained as a historical record of what those gates validated.

`agent-collaboration-successor-protocol-contract` enabled only
`realworld-eval`. Its nine deterministic tests validated the tracked successor
suite and protocol manifest, three-pair / six-run matrix, exact case and budget
digests, sole 5,000-to-7,500-bps context candidate, redacted full model-catalog
binding, external-path isolation, pair projection, and the non-authorizing
preflight boundary.

`agent-collaboration-successor-execution-contract` contained 17 provider-free
tests for the exact execute-binary authorization binding, fixed 15-minute
window, private one-shot output root, campaign/cell/arm reservations, strict
baseline-candidate-holdout ordering, terminal independent-review/freeze/censor
outcomes, accounting limits, and crash/tamper/concurrency recovery without a
provider retry.

`agent-delivery-verification-core-contract` still runs 12 portable receipt and
state tests. The removed `agent-delivery-verification-contract` enabled only
`realworld-eval` with default desktop features disabled and ran 22 exact request
and attempt projection tests. Together they bound the exact frozen seeded
candidate as the matched control, preserved its bytes, and constrained treatment
to three model stages: one initial Reviewer decision, at most one Owner repair
when activated, and one Reviewer recheck after repair. They failed closed on
model-visible output
contract, objective, evidence, request, reference, attempt-slot, or stopping
violations while keeping the exact oracle values model-hidden. The surviving
core contract is included in `ci-contract`, `control-plane`, and `full`, but not
`quick`; it does not exercise the production finalizer or GEPA or prove natural
quality uplift.

`agent-delivery-verification-protocol-contract` runs 19 additional
provider-free tests over the tracked eight-case calibration / 24-case holdout
suite, four balanced seeded-defect/preservation strata, exact case/oracle/order/
budget digests, seeded-candidate/model-input/output-contract aggregates,
matched decision table, redacted model-distinct provider binding, clean-source
authority, canonical private preflight receipt, and zero-call/non-authorizing
boundary. It is included in `ci-contract`, `control-plane`, and `full`, but not
`quick`. The v4 preflight binds the clean source, protocol, unchanged v3 suite
cases, all four aggregates, budgets, hidden-oracle aggregate, redacted model
authority, exact v4 execute name/digest/size/full SHA-256 CodeDirectory, and new
external paths,
and records `provider_calls=0`, `online_runner_frozen=true`, and
`execution_authorized=false`. The CodeDirectory is derived from a verified
private copy of the exact bytes. The separate authorization must match both
runner identities and every aggregate already frozen by preflight.

`agent-delivery-verification-execution-contract` runs 42 provider-free tests
for canonical short-lived authorization, exact preflight/source/provider/model/
credential/runner/output binding, per-case seeded candidate SHA-256/byte
binding, private one-shot consumption, campaign and physical-call reservations
before transport, complete resource accounting, and crash/tamper/concurrency
recovery without retry. It exercises the seeded control and three-stage
treatment flow, calibration-before-holdout ordering, a maximum of 96 logical
calls and 96 physical attempts, zero transport retries, and separate semantic
and immutable wire-payload authority through a real local-loopback HTTP path.
It also covers running-image/path replacement rejection through macOS's
kernel-backed CodeDirectory identity and output-root relocation after competing
authorization files through one parent-level no-clobber consumed marker. It
compiles the three permanently fail-closed v1 binaries, the three permanently
fail-closed v2 binaries, the three permanently fail-closed v3 binaries, and the
three tracked v4 binaries without invoking their entrypoints. The loopback
coverage also proves that an exact zero-byte response artifact is retained with
its digest and zero length, while completed empty Reviewer content becomes
`invalid_verifier_response` without retry. Two loopback fault-injection tests
guard the instrumentation against the failure modes observed in consumed
campaigns: an over-reservation provider usage report is retained verbatim
without silent normalization, and an output-limit (`finish_reason="length"`)
response is rejected as `invalid_output` without retry.
It is included in `ci-contract`, `control-plane`, and `full`, but not `quick`,
`performance`, or `shipping-performance`. It proves no provider outcome or
intelligence uplift.

```sh
cargo test --locked -p agent-runtime \
  delivery_verification_contract_ -- --nocapture
```

The desktop `realworld-eval` delivery-verification tests and the
`cindx-delivery-verification-*` / `cindx-agent-realworld-eval` binaries were
removed in the phase-3 effort-tier rebuild; only the portable `agent-runtime`
receipt/state contract above remains.

```sh
node scripts/run-quality-gates.mjs \
  --profile control-plane \
  --report target/quality-gate-report.json
```

For a performance change, use two clean checkouts on the same machine:

```sh
node scripts/run-paired-performance.mjs \
  --baseline-root ../cindx-base \
  --candidate-root . \
  --output-dir target/performance-regression
```

Cross-machine wall-clock numbers are diagnostics, not release evidence.
Deterministic contracts do not establish answer quality or intelligence uplift.

## Provider Evaluations

Provider-backed runs are explicit, billable, and require user authorization.
Before execution:

1. Pin the source revision, app version, provider identity, cases, treatment,
   budgets, and output location.
2. Preflight every case without contacting the provider.
3. Preserve failures and timeouts in the denominator.
4. Keep prompts, secrets, protected benchmark content, and full outputs outside
   Git.
5. Commit only a sanitized decision summary when it changes the current claim
   boundary.

Do not rerun the frozen Workflow GEPA V12 attempt. Its one-shot evidence is
invalid; a successor requires a corrected lifecycle instrument and a new frozen
protocol. See [EVALUATION.md](EVALUATION.md).

The Goal 3D preflight was provider-free. Its `cindx-collaboration-successor-preflight`
binary (and the Goal 3E authorize/execute and Delivery Verification binaries)
were removed in the phase-3 effort-tier rebuild along with the `realworld-eval`
feature; the text below is retained as a historical record. After the source was
committed and clean, it could be run with two new private paths outside the
repository; it accepted no execution flag, constructed no provider transport, and
wrote `provider_calls_performed=0` and `execution_authorized=false`, validating
that configured credentials exist only to bind the redacted provider and full
model catalog (secrets and model names were not written). This receipt was not an
online authorization.

Goal 3E added separate feature-gated `cindx-collaboration-successor-authorize`
and `cindx-collaboration-successor-execute` binaries; both were removed in the
phase-3 effort-tier rebuild and the following is a historical record. Authorization
was itself
provider-free and bound the canonical preflight, current clean source and
provider/model configuration, exact execute binary, fixed matrix and budgets,
and new output root for 15 minutes. Execution revalidated and consumed that
private capability and output root once, reserved the campaign/cell/arm before
model work, and never retried a started physical run after interruption.

The frozen Goal 3E instance was authorized and consumed once on source
`12a3ea2`. It stopped `CENSORED` after reserving the first baseline Direct arm
after the product run ended before selection with zero selected decisions. The
terminal producer correctly persisted explicit pre-decision `not_selected`; no
treatment or Owner execution occurred. The selected-only external-outcome
projector misclassified that legal state as a malformed receipt, so zero valid
runs were observed. Do not run its authorization or execute path again. Keep
the private authorization, tombstone, journal, logs, workspaces, artifacts,
model identities, and raw outputs outside Git; only the sanitized decision in
[EVALUATION.md](EVALUATION.md) is tracked. Current source now distinguishes the
legal pre-decision state before selected-outcome projection and covers the real
terminal producer-to-projector seam in `agent-strategy-lifecycle-contract`.
This is a classifier repair only: no Goal 3F or provider run is authorized, and
deterministic green gates do not establish uplift.

Delivery Verification retained separate feature-gated v1
`cindx-delivery-verification-preflight`,
`cindx-delivery-verification-authorize`, and
`cindx-delivery-verification-execute` binaries plus the separately named v2,
v3, and v4 triples; all twelve binaries required `realworld-eval` and were
removed in the phase-3 effort-tier rebuild, and the following is a historical
record. The nine old binaries were already retired and permanently rejected
their consumed protocols before reading arguments, environment, paths,
configuration, or live state. The tracked v4 implementation used
`cindx-delivery-verification-v4-preflight`,
`cindx-delivery-verification-v4-authorize`, and
`cindx-delivery-verification-v4-execute` plus
`CINDX_DELIVERY_VERIFICATION_V4_*` paths, compiled by the provider-free
contract gate.

The consumed v4 instance built the three binaries together from clean merged
source into one new private target root outside the repository. Its preflight
required its exact sibling execute binary and wrote a private receipt binding
source, runner full-file and CodeDirectory identities, provider, cases, case
order, seeded candidates, model inputs, output contracts, hidden oracle,
budget, and new output path while reporting zero provider calls and
`execution_authorized=false`. This preflight is provider-free. Verify the
receipt CodeDirectory against `CandidateCDHashFull sha256=` from the exact
copied execute binary. Do not mint another v4 preflight or run its authorization
or execute binaries again; the one-shot authority is consumed.
Private receipts, tombstones, journals, requests, responses, model identities,
secrets, and raw outputs stay outside Git. This successor does not change the
installed App, version, tag, or GitHub release.

The consumed one-shot execute first checked
the frozen CodeDirectory against the kernel identity of its running process,
then atomically created a consumed marker in the output root's parent. Its name
is derived from the canonical output-path digest, so all authorization paths
for that campaign compete for the same marker; moving or deleting the output
root cannot reopen it. This is a local-filesystem guarantee for normal crashes,
concurrency, and relocation, not an external anti-rollback service against a
same-user actor able to delete or restore every private control-plane file.

Delivery Verification protocol v1 consumed its one authorized attempt and
closed `CENSORED` / `INVALID-INSTRUMENTATION` because journal validation equated
its semantic-request digest with the separately domain-separated wire-payload
digest. Protocol v2 separately consumed its one authorized attempt and closed
`terminal_futility` after calibration. Protocol v3 consumed one attempt, made
one provider call, then froze `CENSORED` because terminal validation rejected an
otherwise bound zero-byte response artifact. Protocol v4 then consumed one
attempt whose zero-byte artifact was retained correctly, but the first call was
non-retryable `invalid_output`; case 1 closed `structural_failure` and the
campaign closed `inconclusive` with zero matched pairs. Never mint or execute
another v1, v2, v3, or v4 authority.

The v4 protocol is a new authority over the byte-identical v3 suite. Cases,
order, hidden oracle, seeds, model inputs, output contracts, budgets, thresholds,
and no-retry behavior are unchanged. The model request envelope intentionally
keeps its v3 compatibility schema so the instrumentation successor does not
change model-visible semantic or wire payloads. V4 reserves both identities from
one immutable prepared request, dispatches those exact bytes, and accepts an
exact zero-byte response artifact with its digest and zero length. Empty Reviewer
content from a complete tool-free response is retained as
`invalid_verifier_response` without retry or replacement; a response that is
not complete and tool-free remains structural. Its retained preflight was
non-authorizing, and its later explicit one-shot authorization is now exhausted.

## Browser and Computer Sidecars

Relevant checks:

```sh
node scripts/test-browser-sidecar.mjs
node scripts/test-computer-sidecar.mjs
```

Browser control uses the configured sidecar and a supported local browser. A
managed browser profile is separate from the user's ordinary profile. Computer
control fails clearly when macOS Accessibility permission is missing.

Sidecar health in Settings is diagnostic. A green deterministic sidecar test
does not prove a live page or desktop task succeeded.

## Runtime Data

Persistent state is stored under:

```text
~/Library/Application Support/Cindx
```

`startup.log` in that directory records startup probes, persistent-store
failure, and the workspace `.cindx` retention sweep (one line per workspace, only
when the sweep removed something or hit a failure). If SQLite cannot be opened,
Cindx logs
`persistent state unavailable; startup aborted` and exits instead of using an
in-memory substitute.

## Local Production Build

The local build script requires a clean Git tree, advances the patch version,
stamps all version sources including `docs/CURRENT.md` and `docs/HANDOFF.md`,
builds an Apple Silicon bundle, ad-hoc signs it, probes clean startup and
fail-closed persistence,
creates a zip, and installs `/Applications/Cindx.app` unless `--no-install` is
used.

Run all required checks first, then perform one formal build:

```sh
node scripts/build-local-app.mjs --skip-tests
```

Without `--skip-tests`, the script runs sidecars, frontend tests, Rust tests,
and `shipping-performance` before building. The complete dependency graph can
produce many gigabytes of reproducible `target` output and can be quiet while
Rust is compiling. Check the compiler process; do not start a duplicate build.
After archiving, the build fails closed if `dist/Cindx-<version>-macOS-arm64.zip`
exceeds its bundle-size budget (audit P3-01), so a dependency or asset regression
that bloats the published artifact is caught at build time instead of shipping
silently; raise the budget deliberately, never to absorb an unexplained jump.

Use `--ephemeral-target` only when disposable build storage is desired. Cleanup
must preserve source, user data, configuration, and the installed app.

## GitHub Release

Release consistency requires:

- identical version in package, lockfile, Tauri, Cargo, `CURRENT.md`, and
  `HANDOFF.md`;
- clean source revision and annotated version tag;
- passing release and shipping gates;
- a verified bundle signature;
- a GitHub asset whose name, size, and SHA-256 match the built archive;
- installed app version matching the release.

The current release identity (version, tag, asset, installed bundle) is
maintained in `docs/HANDOFF.md` (Release Identity); this section defines the
consistency contract, not the version. Do not commit application archives to the Git
tree; publish them as GitHub Release assets.

The locally installed `0.2.34` model-semantics validation build is not a
GitHub release; its exact source, archive digest, and signature evidence are
recorded in [HANDOFF.md](HANDOFF.md).

Developer ID signing and notarization require the matching Apple credentials in
the release environment. An ad-hoc-signed local archive is suitable for local
testing but is not a notarized distribution.

## Documentation Maintenance

The allowed project document set is enforced by `scripts/check-docs.mjs`.
Update an existing document instead of adding a phase report, roadmap, duplicate
handoff, release note, ADR, or per-run provider report. Git history preserves
the removed records.
