#!/usr/bin/env sh
set -eu

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

cargo fmt --all -- --check
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings

# Keep routine lint feedback independent of generated frontend resources and
# the optional Lance implementation. Full tests and release builds retain the
# desktop default features and validate the shipping dependency graph.
export TAURI_CONFIG='{"bundle":{"resources":[]}}'
cargo clippy \
  --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --all-targets \
  --locked \
  --no-default-features \
  -- \
  -D warnings
