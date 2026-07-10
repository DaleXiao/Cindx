#!/usr/bin/env sh
set -eu

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

cargo fetch --manifest-path apps/desktop/src-tauri/Cargo.toml
