#!/usr/bin/env sh
set -eu

cd "$(dirname "$0")/../apps/desktop"

NODE_BIN="/opt/homebrew/bin/node"
NPM_CLI="/opt/homebrew/lib/node_modules/npm/bin/npm-cli.js"

if [ ! -x "$NODE_BIN" ]; then
  echo "Node was not found at $NODE_BIN" >&2
  exit 1
fi

if [ ! -f "$NPM_CLI" ]; then
  echo "npm CLI was not found at $NPM_CLI" >&2
  exit 1
fi

exec "$NODE_BIN" --dns-result-order=ipv4first "$NPM_CLI" install
