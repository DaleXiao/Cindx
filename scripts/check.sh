#!/usr/bin/env sh
set -eu

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

node scripts/check-docs.mjs
node scripts/check-gates-manifest.mjs
cargo test --workspace
node scripts/check-desktop-structure.mjs
