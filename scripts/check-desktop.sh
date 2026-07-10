#!/usr/bin/env sh
set -eu

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

node scripts/check-desktop-structure.mjs
node scripts/check-desktop-layout.mjs

if [ ! -d "apps/desktop/node_modules" ]; then
  echo "apps/desktop/node_modules is missing. Run npm install in apps/desktop first." >&2
  exit 1
fi

(cd apps/desktop && npm run build)
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
