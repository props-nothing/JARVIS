# ADR-0003: SQLite Local and PostgreSQL Server Profiles

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

An OpenClaw-like personal installation cannot require a database server. A
multi-device/team deployment needs concurrency, operational tooling, and vector
indexing beyond a single local file. Making either backend universal harms the
other product mode.

## Decision

SQLite plus local files is the default local/portable store. PostgreSQL plus
pgvector and object storage is the server store. Both implement JARVIS-owned
repository and unit-of-work ports with semantic parity. Secrets live outside
normal database records.

## Consequences

### Positive

- Zero-infrastructure personal setup.
- Production server scale without abandoning relational canonical state.
- Storage remains replaceable behind domain-oriented ports.

### Negative

- Two migration/query implementations and parity tests.
- Some search/concurrency strategies differ by profile.

## Alternatives Considered

- PostgreSQL everywhere: rejected for installation burden.
- SQLite everywhere: rejected for team/server concurrency and operations.
- Dedicated vector database initially: rejected until measured pgvector/local
  limits justify it.

## Verification

- Shared repository contract suite runs against both backends.
- Local clean install requires no external service.
- Workspace filtering, transactions, migrations, backup, and restore have parity
  tests.