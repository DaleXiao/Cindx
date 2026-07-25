#!/usr/bin/env sh
set -eu

node scripts/check-desktop-structure.mjs

if [ ! -d "apps/desktop/node_modules" ]; then
  echo "apps/desktop/node_modules is missing. Run scripts/install-desktop-deps-ipv4.sh first." >&2
  exit 1
fi

(cd apps/desktop && npm test && npm run build)
