# Security Policy

## Supported versions

Only the latest published release is supported with security fixes.

## Reporting a vulnerability

Please report vulnerabilities through **GitHub private vulnerability
reporting** (the Security tab of this repository) rather than public issues,
and include a reproduction and the affected version.

## Scope

Cindx is a local desktop application. The following are in scope:

- Anything that exfiltrates the provider API key or other secrets stored in
  the macOS Keychain.
- Escapes from the sandbox profile or the permission gate (file, shell,
  process, web, browser, computer, MCP, or skill tools executing without the
  recorded permission decision).
- Persistence or state corruption that silently alters the audit trail.
- Prompt-injection paths that turn retrieved workspace content into tool
  execution beyond the permission gate.

Out of scope: issues in bundled dependencies (report upstream), theoretical
issues without a reproduction path in the shipping configuration, and the
feature-gated evaluation tooling when it is not enabled.
