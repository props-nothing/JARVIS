# ADR-0008: Database-Backed Native Workflows Before Temporal

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

JARVIS needs durable timers, approvals, retries, and restart recovery early.
Temporal is mature but adds a service, operational model, deterministic workflow
constraints, and language SDK considerations before local product correctness is
known.

## Decision

V1 uses a small database-backed workflow/scheduler engine with persisted state,
leases, outbox/inbox, idempotent activities, and crash-point tests. Ports allow a
future Temporal backend. Temporal adoption requires measured needs and a new ADR.

## Consequences

### Positive

- Local install remains self-contained.
- JARVIS learns its required semantics before selecting external orchestration.
- Simple schedules/approvals do not require a cluster/service.

### Negative

- The team owns correctness for a constrained workflow engine.
- Advanced distributed replay/versioning is intentionally deferred.

## Alternatives Considered

- Temporal from day one: rejected for v1 operational/install burden.
- In-memory jobs/cron: rejected because work would not survive failure.

## Verification

- Crash before/after every transition, duplicate delivery, lease race, timer
  misfire, approval restart, and idempotent side-effect tests.
- Load/complexity metrics are recorded to trigger a Temporal evaluation.