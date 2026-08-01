#!/usr/bin/env sh
set -eu

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

# This check intentionally excludes bundle resources and the optional LanceDB
# implementation. Shipping builds retain both through the desktop default feature.
export TAURI_CONFIG='{"bundle":{"resources":[]}}'

cargo check \
  --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --lib \
  --locked \
  --no-default-features
