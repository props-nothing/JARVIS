# ADR-0002: One Durable Daemon Serves Thin Clients

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

CLI, desktop, browser, mobile, API, and voice must observe the same sessions,
memory, policy, tasks, approvals, and events. Embedding a core in each interface
would duplicate state and create conflicting authorities.

## Decision

`jarvisd` is the long-running authority. `jarvis`, desktop, web, mobile, voice,
and external clients use versioned authenticated APIs. V1 local communication is
loopback HTTP/WebSocket/SSE with a permission-restricted discovery file and
local credential. Remote mode is explicit and uses separate TLS/auth policy.

## Consequences

### Positive

- One state/policy owner and consistent multi-interface behavior.
- Background work survives UI/CLI exit.
- API and daemon can be tested independently from interfaces.

### Negative

- Requires service lifecycle, discovery, compatibility, and repair logic.
- Loopback APIs require real authentication, Host/Origin controls, and limits.

## Alternatives Considered

- In-process library per UI: rejected due to duplicated authority/state.
- Native socket only: may be added later, but loopback gives the first clients
  one cross-platform protocol and easier browser/desktop parity.

## Verification

- CLI and desktop contain no business policy/database/provider credentials.
- Killing a client does not stop a durable run.
- Local-origin and unauthenticated-process attack tests fail closed.