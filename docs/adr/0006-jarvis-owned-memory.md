# ADR-0006: JARVIS Owns Canonical Memory

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

Providers and agent runtimes offer sessions, checkpoints, and memory stores, but
they differ in semantics, portability, provenance, retention, and access. Using
one as canonical memory would lock user knowledge to a runtime and make
cross-workspace policy inconsistent.

## Decision

JARVIS owns typed durable memory, entity resolution, provenance, correction,
forget/export, retrieval policy, and context selection. Runtime/provider memory
may exist as a checkpoint/cache and is referenced separately.

## Consequences

### Positive

- User memory survives model/runtime changes.
- One privacy, workspace, provenance, and correction policy.
- Multiple runtimes receive consistent bounded context.

### Negative

- Requires a real memory lifecycle and evaluation program.
- Provider-native continuation may duplicate some working state.

## Alternatives Considered

- Chat history as memory: rejected because it lacks typed claims and correction.
- Runtime/provider store as canonical: rejected due to lock-in and policy gaps.
- Vector-only memory: rejected because similarity is not truth or authorization.

## Verification

- Restart, provenance, correction, expiry, conflict, export/delete, and
  cross-workspace tests.
- Runtime replacement does not change canonical retrieved preference behavior.