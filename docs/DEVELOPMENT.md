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

## Fast Checks

Run documentation and static structure checks after every relevant change:

```sh
node scripts/check-docs.mjs
node scripts/check-desktop-structure.mjs
node scripts/check-desktop-layout.mjs
git diff --check
```

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

## Quality Profiles

The source of truth is `benchmarks/system/quality-gates-v1.json`.

| Profile | Purpose |
| --- | --- |
| `quick` | Documentation, version, layout, and structural contracts |
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
`agent-terminal-lifecycle-contract` gates. They also include the
`agent-execution-graph-contract`, which projects typed model attribution to
prove that matched Direct exposes no workflow worker while matched Workflow
completes exactly one read-only Specialist and its optional planned Independent
Verifier. When present, the actual Verifier attribution must use a different
configured model from the Specialist. The gate retains the existing total
model-call boundary. Together these checks cover preparation and start
linearization, selected-decision recovery, success/failure/cancellation terminal
linkage, exactly-once replay, and evaluation fail-closed behavior without
contacting a provider.

The lifecycle filters include the preparation failure race against cancellation
and steer, including exactly-once terminal persistence for the winning epoch.

The same profiles include `agent-outcome-evidence-contract`. This provider-free
gate checks exact lifecycle, treatment-exposure, and resource bindings;
integer positive, partial, and negative scoring from external postconditions;
zero-score retention for valid safety or preservation failures; tamper
censoring; and isolation from production learning consumers.

They also include `agent-collaboration-learning-contract`. This provider-free
gate checks the bounded policy schema, trusted assignment-to-exercise binding,
actual context and same-lane repair attribution, matched Direct/Workflow
identity, append-only train/holdout evidence, freeze conditions, and the
independent-review requirement. Its approved state remains offline-only and is
not a prompt, routing, memory, canary, or serving admission.

`agent-collaboration-learning-offline-contract` checks bounded canonical import,
hash-chain replay, physical-run deduplication, and committed-context aggregation
with the optional agent-application feature. The adjacent
`agent-collaboration-learning-offline-adapter-contract` enables only
`realworld-eval` and checks explicit matched policy installation, actual context
budgeting, materialized assignment, private journal recovery, orphan handling,
tamper/fork rejection, old-V12 isolation, actual-pair capture identity, and
censor-without-retry behavior. These gates do not
contact a provider or prove intelligence uplift.

`agent-collaboration-successor-protocol-contract` also enables only
`realworld-eval`. Its nine deterministic tests validate the tracked successor
suite and protocol manifest, three-pair / six-run matrix, exact case and budget
digests, sole 5,000-to-7,500-bps context candidate, redacted full model-catalog
binding, external-path isolation, pair projection, and the absence of an online
executor. The gate is in `ci-contract`, `control-plane`, and `full`, but not
`quick`; it performs no provider call and proves no uplift.

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

The Goal 3D successor is currently preflight-only. After the source is committed
and clean, a provider-free preflight may be run with two new private paths
outside the repository:

```sh
CINDX_COLLABORATION_SUCCESSOR_OUTPUT_ROOT=/private/path/new-output-root \
CINDX_COLLABORATION_SUCCESSOR_PREFLIGHT_RECEIPT=/private/path/new-preflight.json \
cargo run --locked \
  --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --no-default-features --features realworld-eval \
  --bin cindx-collaboration-successor-preflight
```

The command accepts no execution flag, constructs no provider transport, and
writes `provider_calls_performed=0` and `execution_authorized=false`. It validates
that configured credentials exist only to bind the redacted provider and full
model catalog; secrets and model names are not written. This receipt is not a
private online authorization. That authorization and the corresponding
provider-action reservation/execution wiring do not exist yet and require a
separate user decision before any billable call.

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

`startup.log` in that directory records startup probes and persistent-store
failure. If SQLite cannot be opened, Cindx logs
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

The current release is
[v0.2.30](https://github.com/DaleXiao/Cindx/releases/tag/v0.2.30), with
`Cindx-0.2.30-macOS-arm64.zip`. Do not commit application archives to the Git
tree; publish them as GitHub Release assets.

The locally installed `0.2.31` Goal 3C validation build is not a GitHub release;
its exact source, archive digest, and signature evidence are recorded in
[HANDOFF.md](HANDOFF.md).

Developer ID signing and notarization require the matching Apple credentials in
the release environment. An ad-hoc-signed local archive is suitable for local
testing but is not a notarized distribution.

## Documentation Maintenance

The allowed project document set is enforced by `scripts/check-docs.mjs`.
Update an existing document instead of adding a phase report, roadmap, duplicate
handoff, release note, ADR, or per-run provider report. Git history preserves
the removed records.
