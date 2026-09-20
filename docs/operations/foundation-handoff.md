# Foundation Implementation Handoff

Status: ACCEPTED
Owner: Foundation
Applies to: Milestone 1 and the first runnable vertical slice

This is the implementation handoff for the first coding agent. It does not
replace [the backlog](../../TODO.md), architecture, contracts, or acceptance
scenarios. It fixes their execution order and required evidence so an agent can
start building without making implicit platform or security decisions.

## Definition of Ready

Foundation implementation may begin when all of the following are true:

- Milestone 0 exit checks pass and its TODO evidence is current.
- Accepted ADRs 0001 through 0010 remain applicable.
- The [local control API contract](../contracts/local-control-api.md) is the
  daemon/client boundary for the first slice.
- The project owner understands the public-release blockers in
  [release readiness](release-readiness.md). They do not block local engineering
  or CI test-signing.
- Proposed external crates, protocols, and OS facilities have current evidence
  under the [integration research policy](../research/integration-research-policy.md).
- No unrelated provider, connector, MCP, desktop, external runtime, or voice
  implementation is included in the Foundation slice.

## Research Before Dependency Selection

Before editing `Cargo.toml` or pinning the toolchain, create or refresh evidence
for the exact proposed versions and supported targets of:

- Rust/Cargo and target support;
- Tokio process, signal, cancellation, and runtime behavior;
- Axum/Tower request limits, loopback binding, middleware, and graceful drain;
- Serde/JSON behavior used by public or persisted contracts;
- SQLx and SQLite migration, locking, backup, integrity, and feature behavior;
- platform path, permission/ACL, credential-store, lock, and service facilities;
- tracing and any OpenTelemetry components selected for the first slice;
- CLI/config crates whose parsing or serialization becomes a user contract.

Boundary-affecting dependencies require full integration evidence. Routine
implementation-only crates use the shorter dependency-evidence record defined
by the research policy. No dependency is selected solely because another
project uses it.

## Build Order

Complete one row and its focused check before opening the next behavior slice.

| Order | TODO | Result | Cheapest discriminating check |
| --- | --- | --- | --- |
| 0 | `FND-000` | Exact Foundation versions, licenses, supported targets, and official evidence | Evidence manifest becomes ready and rejects stale/mismatched metadata |
| 1 | `FND-001` | Workspace, toolchain, lints, dependency policy | Empty/minimal workspace passes format, Clippy, test on native CI |
| 2 | `FND-002` | Typed IDs, clock, cancellation, request context, errors | Serialization and cancellation race tests |
| 3 | `FND-003` | Profile paths, ACLs, runtime/discovery paths | Table-driven tests on Windows, macOS, Linux |
| 4 | `FND-004` | Versioned layered configuration | Atomic-write, precedence, unknown-version, redaction tests |
| 5 | `FND-005` | Structured local observability | Seeded secret-canary scan over every output sink |
| 6 | `FND-006` | SQLite migrations and recovery primitives | Fresh/upgrade/failure/backup/restore repository tests |
| 7 | `FND-007` | `jarvisd` lifecycle and authenticated local API | Real daemon test for discovery, auth, readiness, drain, restart |
| 8 | `FND-008` | `jarvis status`, `config`, `logs`, `service`, `doctor` | Fault-injection CLI scenario with actionable diagnostics |
| 9 | `FND-009` | Per-user service lifecycle | Native install/start/restart/remove tests without elevation |
| 10 | `FND-010` | Clean-machine CI journeys | Packaged binary smoke test in clean user homes |
| 11 | `FND-011` | Artifacts, test signing, SBOM, provenance | Consumer-side verification rejects modified bytes/metadata |
| 12 | `FND-012` | Install/update/rollback/portable/uninstall | Signed lower-version fixture upgrade and failed activation rollback |
| 13 | `FND-013` | Reviewable diagnostics/support bundle | Secret-canary scan and user-preview/export acceptance |
| 14 | `FND-014` | Previewed repair plans with rollback | Fault-injection repair scenarios verify postconditions and preserve recoverable state |

The first runnable Brain slice then follows
[its dedicated plan](../planning/first-vertical-slice.md). Do not add empty
future crates to make the target tree look complete.

## Initial Repository Shape

The initial workspace contains only ownership boundaries exercised by the slice:

```text
apps/
  jarvisd/
  jarvis-cli/
crates/
  jarvis-domain/
  jarvis-application/
  jarvis-protocol/
  jarvis-infrastructure/
  jarvis-observability/
tests/
  e2e/
```

Dependency direction is enforced mechanically:

```text
apps -> application -> domain
adapters/infrastructure -> domain ports
domain -> standard library and narrowly justified domain-only crates
```

HTTP handlers, CLI rendering, SQLx repositories, and platform service code do
not own domain decisions. `main.rs` composes dependencies and lifecycle only.

## Evidence Per TODO

When a TODO is checked, its evidence names:

- requirement, ADR, and contract sections implemented;
- focused test command and result;
- native operating-system lanes run or explicitly skipped;
- failure, cancellation, restart, migration, and redaction cases covered;
- generated schema/fixture changes;
- integration evidence note and exact dependency versions, when applicable;
- operator documentation and known unsupported behavior.

Do not use "tests pass" without the command and behavior. Do not mark an item
complete from a type, trait, route stub, canned response, or test that mocks the
boundary the item exists to prove.

## Stop and Escalate Conditions

Stop the current slice and record `BLOCKED` when:

- the only path weakens an accepted ADR or trust boundary;
- a platform cannot enforce required local permissions or authentication;
- official sources disagree on behavior that affects a contract;
- a license is incompatible or not verified;
- a migration cannot fail without risking existing data;
- a release step needs an owner-controlled secret, legal decision, domain, or
  external account that has not been provided.

Continue with another ready, dependency-safe item only when the blocked item is
not on its critical path.

## Milestone Exit Record

Milestone 1 exits with a dated report containing:

- exact commit/artifact identifiers and target matrix;
- every `FND-*` evidence entry;
- `ACC-001` through `ACC-005`, `ACC-070`, and `ACC-072` results as applicable;
- checksums, signature-verification output, SBOM, and provenance references;
- skipped lanes, owner-controlled public-release blockers, and residual risks;
- restore/rollback proof from packaged artifacts, not source binaries.
