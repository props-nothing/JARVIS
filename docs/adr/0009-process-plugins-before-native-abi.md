# ADR-0009: Process Protocols Before Native Dynamic Plugin ABI

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

Plugins need language flexibility, independent upgrades, permission isolation,
and crash containment. Rust dynamic-library ABI and in-process third-party code
make compatibility and security difficult across platforms.

## Decision

Plugins use MCP, versioned runtime protocol, HTTP/webhook, stdio, or future WASI
components. Native dynamic libraries are not the primary extension mechanism.
Install manifests are validated and permission grants are separate from install.

## Consequences

### Positive

- Process crashes and dependency conflicts are contained.
- Extensions can use any language and version independently.
- Protocol compatibility is testable and explicit.

### Negative

- Serialization and process overhead.
- Sandboxing remains platform work; process separation alone is not a sandbox.

## Alternatives Considered

- Rust `cdylib` plugin ABI: rejected for stability and trusted-code expansion.
- Source-only compiled plugins: rejected for install complexity and supply-chain
  authority inside the daemon.

## Verification

- No public native plugin loading API in v1.
- Plugin crash/hang/upgrade/uninstall and permission tests.
- Manifest provenance and protocol compatibility checks precede launch.