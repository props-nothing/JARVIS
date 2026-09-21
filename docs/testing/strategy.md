# Testing Strategy

Status: PROPOSED

## Principles

1. Test the smallest owner of the behavior first.
2. Use deterministic clocks, IDs, model streams, and external effects.
3. Use captured real payloads for wire assumptions.
4. Use gated live tests for provider-owned behavior.
5. Test restart/cancellation/duplication as normal paths, not afterthoughts.
6. Test packaged artifacts for installation and upgrade claims.
7. Never use production credentials or users in automated tests.

## Test Layers

### Domain Unit Tests

Pure, fast tests for transitions, policy, risk, fingerprints, budgets, ranking,
schedule semantics, and validation. No Tokio/network/database unless required by
the type under test.

### Application Tests

Use in-memory/fake ports to test use cases, transaction intent, cancellation,
event publication, retries, approvals, and run/workflow orchestration.

The scripted model provider emits arbitrary event sequences, malformed tool
arguments, refusals, usage timing, and errors deterministically.

### Repository Contract Tests

The same semantic suite runs against SQLite and PostgreSQL adapters:

- CRUD through domain methods;
- optimistic transitions/conflicts;
- transactions and outbox/inbox atomicity;
- workspace isolation;
- idempotency and leases;
- migration from supported versions;
- backup/restore where backend-owned.

### Adapter Contract Tests

Use local fake HTTP/WebSocket/MCP/runtime servers and captured official payloads
to prove parsing, streaming, auth placement, errors, pagination, retries,
webhook signatures, cancellation, and normalization.

Fixtures record source URL, provider/API version, capture date, redactions, and
the invariant they prove. Hand-authored happy fixtures are insufficient.

### Gated Live Tests

Live tests use dedicated sandbox/test accounts and explicit environment flags.
They:

- skip with a precise reason when unavailable;
- have cost, time, request, and resource limits;
- avoid real communication/phone calls unless the named test explicitly enables
  a dedicated destination;
- clean up remote resources and report leftovers;
- record provider IDs safely for diagnostics;
- never run from untrusted forks with secrets.

### End-to-End Tests

Exercise public interfaces through a real daemon and real SQLite, scripted local
providers/runtimes/tools, and packaged binaries where relevant. E2E scenarios do
not reach into private tables to create normal product state.

### Installer and Upgrade Tests

Native clean VM/runner tests cover install, onboarding, service lifecycle,
status/doctor, update from supported prior artifacts, interrupted update,
rollback, uninstall, and data retention/purge choices.

### Security and Fuzz Tests

- authorization matrix and workspace isolation;
- prompt-injection scenarios;
- path/URL/webhook/token parsers;
- JSON/protocol/event stream fuzzing;
- malicious runtime/MCP/plugin processes;
- secret canary scans;
- installer/update tampering;
- resource exhaustion and budget enforcement.

### Performance and Soak

Measure startup, request overhead, first event/token, queue depth, database
latency, memory retrieval, event lag, scheduler lateness, tool/model/voice
latency, CPU/memory/disk, and cost. Soak connector/runtime reconnect and workflow
recovery. Performance tests define hardware/profile and avoid universal claims.

## Test Ownership Matrix

| Behavior | Primary test | Secondary proof |
| --- | --- | --- |
| Policy/approval decision | Domain unit | Cross-route E2E |
| Run/workflow transition | Application + repository | Crash E2E |
| Provider JSON/SSE | Adapter contract fixture | Gated live |
| OAuth/webhook | Fake server/fixture | Dedicated test account |
| MCP negotiation | Protocol/conformance | Inspector/live server |
| Runtime lifecycle | Fixture child process | Real adapter smoke |
| Workspace isolation | Repository/API integration | Security review |
| Installer/update | Native packaged E2E | Manual release signoff |
| Voice wire/call | Captured compatibility fixture | Dedicated phone test |

## Fault Injection

Add controlled failpoints around:

- before/after transaction commit;
- before/after outbox publish;
- before external request, after request write, after provider acceptance, before
  outcome persistence;
- approval creation/decision/consumption;
- workflow lease claim/renew/complete;
- runtime event persist/ack;
- migration and update activation;
- webhook inbox persist/ack.

Use failpoints in test builds only and identify them by stable names.

## Time and Concurrency

- Inject clocks; never use sleeps to prove timer behavior.
- Use bounded deterministic schedulers/fakes where possible.
- Run concurrency races repeatedly and with model checking/property tests for
  critical transition code where practical.
- Assert no task/process/socket/temporary directory leaks after cancellation.
- Use timeouts around every integration test so CI cannot hang indefinitely.

## Snapshots

Snapshots are appropriate for stable public JSON/events, CLI help, diagnostics
shape, and generated contracts. They are not a substitute for semantic
assertions. Redact nondeterministic IDs/times explicitly and review diffs.

## Planned CI Lanes

`FND-010` implements the first five lanes. The `[x]` markers below mean the lane
exists in `.github/workflows`; they do not mean every behavior it will eventually
cover is present. `docs/operations/ci-gates.md` is the operator description of
what currently runs and what remains unproven.

```text
docs            [x] links, structure, generated docs
rust-fast       [x] fmt, clippy, unit
rust-workspace  [x] all features/targets where practical (host target per lane)
sqlite          [x] migrations/repository/integration
postgres        [ ] migrations/repository/integration
protocol        [ ] schema golden, runtime, MCP conformance
security        [ ] audit, deny, secret canary, fuzz smoke
ui              [ ] lint, type, unit, build
e2e-local       [x] real daemon with fakes
install-<os>    [x] packaged clean-machine journey (binaries, not yet a package)
live-<provider> [ ] manual/scheduled protected test account
release         [ ] signatures, SBOM, provenance, upgrade journey
```

Two lanes are native rather than host-agnostic. `rust-workspace` runs per tier-1
target on a matching runner (`aarch64-unknown-linux-gnu`
on `ubuntu-24.04-arm`, `aarch64-apple-darwin` on `macos-14`, and
`x86_64-apple-darwin` on `macos-15-intel`), because bundled SQLite needs each
target's own C toolchain and the workspace does not cross-compile from one host.
`install-<os>` currently stages the release binaries into an empty profile
directory; it does not yet install a package, which is `FND-011`/`FND-012`.

A lane marked `[x]` that never executes on a real runner is not proven. The
`Native targets` matrix is where the `#[cfg(unix)]` permission assertions finally
run; on the Windows authoring host they are typechecked only.

## Coverage and Completion

Line coverage is diagnostic, not the goal. Completion requires acceptance and
failure-path evidence for the behavior. High-risk boundaries require branch/
state transition coverage and mutation/property/fuzz testing where it catches
meaningful faults.

Every skipped required test is reported as residual risk, not success.