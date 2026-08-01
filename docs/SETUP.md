# Local Setup

These instructions describe repository prerequisites, not one developer's
machine state.

## Prerequisites

- macOS with Apple Command Line Tools
- Node.js 22 and npm
- Rust stable with the `aarch64-apple-darwin` target
- Git and SQLite
- `protobuf` for native Rust dependencies

The repository currently uses Tauri 2, React, TypeScript, Vite, and a Rust
backend.

## Rust Toolchain

One supported Homebrew setup is:

```sh
brew install rustup protobuf
$(brew --prefix rustup)/bin/rustup toolchain install stable --profile minimal
$(brew --prefix rustup)/bin/rustup default stable
$(brew --prefix rustup)/bin/rustup target add aarch64-apple-darwin
```

Homebrew's `rustup` formula is keg-only. If `cargo` is not visible, add:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

## Dependencies

Install the locked frontend dependencies:

```sh
cd apps/desktop
npm ci
```

If registry resolution stalls on IPv6, use the repository helper from the root:

```sh
scripts/install-desktop-deps-ipv4.sh
```

If a restricted environment cannot reach the Cargo registry, fetch desktop
dependencies from a normal terminal:

```sh
scripts/fetch-desktop-rust-deps.sh
```

## Verification

From the repository root:

```sh
node scripts/check-docs.mjs
scripts/check.sh
scripts/check-frontend.sh
scripts/check-desktop.sh
```

`scripts/check.sh` runs the Rust workspace and desktop structure checks.
Frontend and Tauri checks require their locked dependencies to be installed.
See [QUALITY_GATES.md](QUALITY_GATES.md) for larger profiles.

## Development

Start the desktop development process through the repository wrapper:

```sh
npm --prefix apps/desktop run tauri -- dev
```

Local packaging uses:

```sh
npm --prefix apps/desktop run build:app
```

The wrapper owns version carry rules and startup probes. Do not bypass it when
producing an application intended for installation.

## Application Data

Installed builds store durable state under:

```text
~/Library/Application Support/Cindx
```

The SQLite database and `startup.log` live there. Set `CINDX_DATA_DIR` only for
isolated development or CI probes.

Cindx requires persistent state. If SQLite cannot be opened safely, startup
aborts and records the failure; it does not continue with an in-memory store.
