# Setup

## Current Machine Status

Checked on the target Mac:

- macOS: 27.0
- CPU: Apple M4, arm64
- Memory: 16 GB
- Node: installed
- npm: installed
- Python: installed
- Git: installed
- SQLite: installed
- Apple Command Line Tools: installed
- Rust: installed through Homebrew `rustup` and stable toolchain

## Rust Toolchain

Rust was installed with:

```sh
brew install rustup
$(brew --prefix rustup)/bin/rustup toolchain install stable --profile minimal
$(brew --prefix rustup)/bin/rustup default stable
```

Then make sure Rust is visible in the shell:

```sh
rustc --version
cargo --version
```

Because the Homebrew `rustup` formula is keg-only, fresh shells may need this
PATH entry:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

Project check command:

```sh
scripts/check.sh
```

This checks the Rust kernel workspace and performs an offline desktop structure
check. It does not download npm or Cargo dependencies.

Desktop check command after npm dependencies are installed:

```sh
scripts/check-frontend.sh
scripts/check-desktop.sh
```

If registry DNS resolution prefers IPv6 or stalls, install desktop dependencies
with the project IPv4-first helper:

```sh
scripts/install-desktop-deps-ipv4.sh
```

If Cargo cannot resolve `index.crates.io` from inside Codex, fetch the desktop
Rust dependencies from a normal terminal:

```sh
scripts/fetch-desktop-rust-deps.sh
```

## Tauri Desktop Gate

After Rust is installed, scaffold or add the Tauri desktop app under
`apps/desktop`.

Planned stack:

```text
Tauri v2
React
TypeScript
Vite
Rust backend commands
```

Full Xcode is not required for the first desktop MVP. Apple Command Line Tools
are sufficient for initial local development.

## Verified

The initial Rust workspace compiles and tests with the project check script.

Known note: the first install attempt used the old `rustup-init` Homebrew
formula name. The correct formula is `rustup`.

## Installed Application Data

Release builds do not use the source checkout for runtime state. On macOS,
Cindx stores its database and configuration under:

```text
~/Library/Application Support/Cindx
```

Startup diagnostics are appended to `startup.log` in the same directory. Set
`CINDX_DATA_DIR` only for isolated development or CI probes.
