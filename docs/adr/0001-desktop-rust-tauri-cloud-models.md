# ADR 0001: Desktop-First Rust/Tauri Runtime With Cloud Models

## Status

Accepted for MVP.

## Context

The product is a local agent that can operate files, tools, memory, browser
automation, retrieval, and eventually computer use. The user will provide cloud
model APIs and does not plan to run local LLMs in the MVP.

## Decision

Use Rust for the local agent kernel and Tauri for the desktop app shell.

Use cloud model providers through a provider abstraction. The local runtime keeps
authority over tools, permissions, event logs, memory, and context packaging.

## Consequences

Benefits:

- Strong local execution boundary.
- Small desktop bundle compared with Electron.
- Good fit for permission and audit logic.
- Cloud models remain replaceable.

Tradeoffs:

- Rust and Tauri toolchains must be installed.
- Browser automation and RAG may still need Node/Python sidecars.
- Some macOS computer-use capabilities may require Swift or native helper code.

## Follow-Up Decisions

- SQLite crate and migration strategy.
- API key storage strategy.
- OpenAI-compatible provider request format.
- LanceDB sidecar protocol.
- Browser controller protocol.
