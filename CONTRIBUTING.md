# Contributing to Cindx

Thank you for looking at Cindx. This repository is the source of a macOS
desktop application: a React/Tauri frontend, a set of portable Rust crates
that own the agent contracts, and a desktop composition layer that wires them
together.

## Development setup

- Node 22 (the structure and docs gates rely on it) and Rust stable.
- `cd apps/desktop && npm ci` before any desktop build or test; the desktop
  crate is a separate cargo workspace.

## Checks

Every change should keep these green:

```sh
node scripts/check-docs.mjs          # documentation baseline consistency
node scripts/check-desktop-structure.mjs   # module budgets and ratchets
scripts/check.sh                     # workspace fmt/clippy/tests
scripts/check-frontend.sh            # frontend tests and tsc/eslint
scripts/check-desktop.sh             # desktop fmt/clippy/tests
```

The structure gate carries ratchets (line budgets, allowlists, assertions) that
exist on purpose: a change that adds modules must merge or retire others, and
blocked shapes fail closed rather than by review attention.

## Pull requests

- Make the smallest cohesive change that satisfies the behavior; keep tests
  with the code they prove.
- Sync the maintained documents (`docs/CURRENT.md`, `docs/ARCHITECTURE.md`,
  `docs/DEVELOPMENT.md`) in the same change when behavior, ownership, or
  verification flow changes.
- Deterministic tests prove contracts, not intelligence uplift; do not claim
  quality improvements from provider runs in a PR.
- Never commit secrets of any kind. Provider API keys live in the macOS
  Keychain, never in the tree or in configuration files.

## Commit style

Short imperative subject line, conventional prefixes (`feat`, `fix`, `docs`,
`chore`, `retire`), body explains the why and the boundary of the change.
