# ADR-0004: Canonical Tool Policy with MCP at the Edge

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

Native tools, connectors, MCP servers, runtime tools, and voice clients can all
cause the same external effects. Letting each route implement permission and
approval independently creates bypasses. MCP evolves and includes untrusted
remote metadata, so it is unsuitable as the internal authority model.

## Decision

JARVIS defines a canonical tool/effect/policy/approval/execution contract. Every
tool source maps into it and every invocation passes through it. MCP is used as
client/host and scoped server interoperability at adapter boundaries. Discovery
never grants execution permission.

## Consequences

### Positive

- One auditable enforcement point.
- Native and external capabilities behave consistently.
- MCP can evolve without migrating internal domain state.

### Negative

- Translation can lose provider/MCP-specific nuance unless extensions are
  modelled explicitly.
- All execution paths must resist shortcuts for trusted callers.

## Alternatives Considered

- MCP as the internal tool model: rejected due to protocol churn and missing
  JARVIS-specific authority/idempotency semantics.
- Separate permission systems per source: rejected as unsafe and untestable.

## Verification

- Route-equivalence tests prove native/MCP/runtime/voice calls reach one facade.
- Invocation-time ownership and authorization tests cover stale discovery.
- MCP conformance tests run without granting broad JARVIS access.