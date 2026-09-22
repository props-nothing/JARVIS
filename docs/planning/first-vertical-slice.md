# First Runnable Vertical Slice

Status: ACCEPTED
Readiness: READY; begin with `FND-000`
Target TODOs: `FND-000` through `FND-014`, then `BRN-001`, `BRN-002`, `BRN-004`,
`BRN-005`, and `BRN-007`

## Goal

From one source checkout on Windows, macOS, or Linux, start `jarvisd`, connect
with `jarvis`, send a prompt to a deterministic scripted provider, stream a
response, persist the session/run in SQLite, restart the daemon, and inspect the
same completed run.

This slice deliberately excludes paid providers, MCP, memory extraction,
external runtimes, desktop UI, connectors, and voice.

## Hypothesis

A small daemon/client path built on domain/application ports and real SQLite can
prove the process, storage, streaming, cancellation, and documentation boundaries
without committing to any external provider SDK.

The cheapest falsifier is an end-to-end test that starts a real daemon with a
temporary profile, streams scripted output through the public client, kills/
restarts the daemon, and reads the persisted run.

## Initial Workspace

Create only these crates/apps unless implementation evidence requires a split:

```text
Cargo.toml
rust-toolchain.toml
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

`jarvis-runtime` and `jarvis-tooling` can be introduced in their owning slices;
do not create empty future crates.

## Ordered Work

1. Research exact Foundation versions, licenses, target support, and behavior;
  pass the evidence manifest gate.
2. Workspace/toolchain/lints and CI on three operating systems.
3. Typed IDs, clock, cancellation, request context, and domain errors.
4. Platform profile paths and owner-only directory/runtime-file behavior.
5. Layered versioned config with atomic writes and redaction.
6. Tracing/logging with a seeded secret-canary test.
7. SQLite migration runner and run/session/message schema subset.
8. `jarvisd` composition, single-instance lock, loopback bind, local enrollment,
   liveness/readiness, and graceful drain.
9. `jarvis status` and `jarvis doctor` minimum useful checks.
10. Per-user service lifecycle and clean-machine native CI smoke tests.
11. Native artifacts, test signing, SBOM/provenance, install/update/rollback,
  portable mode, and uninstall journeys.
12. Reviewable support bundle and previewed/confirmed repair operations.
13. Complete the Milestone 1 exit gate before opening Brain implementation.
14. Normalized model request/event port and deterministic scripted provider.
15. Native minimal run state machine: received, context, model, responding,
    completed/failed/cancelled.
16. POST run/chat and SSE activity endpoint plus thin CLI renderer.
17. Real-daemon E2E, abrupt restart, disconnect, cancellation, and corruption
    tests.

## Scripted Provider Scenarios

- text in arbitrary chunks including split Unicode boundaries;
- delayed first event under injected clock/control;
- usage before/after text;
- explicit refusal;
- retryable and terminal error;
- stream ends without terminal;
- duplicate/out-of-order event;
- cancellation race;
- oversized output.

Only valid normalized behavior reaches the run state machine.

## Minimal API

Wire behavior, authentication, discovery, errors, cancellation, and replay are
governed by the
[local control API contract](../contracts/local-control-api.md).

```text
GET  /health/live
GET  /health/ready
POST /api/v1/runs
GET  /api/v1/runs/{id}
GET  /api/v1/runs/{id}/events
POST /api/v1/runs/{id}/cancel
```

The CLI provides:

```text
jarvis status
jarvis doctor
jarvis ask "hello"
jarvis runs show <id>
```

## Explicit Non-Goals

- No tool call stub presented as implemented.
- No provider API key or external network dependency.
- No PostgreSQL implementation until SQLite repository semantics are stable.
- No web/desktop UI.
- No Brain implementation begins before the background service, installer,
  diagnostics, support, and repair work completes Milestone 1.
- No hidden chain-of-thought or fake planning UI.

## Progress

Steps 1 through 13 (Milestone 1) are complete apart from the owner-gated
`OWN-001` through `OWN-005`; see `TODO.md`. In this slice: step 14 (the port and
scripted provider, `BRN-002`) is done, step 15's **state machine** (`BRN-005`) is
done as the transition layer, step 16's **repositories** (`BRN-004`) are done
against SQLite, `BRN-006`'s **context budgeter** is done as candidate selection
and the manifest, and step 15's **run controller** plus the repositories' in-memory
test doubles (`BRN-008`, the pieces of that TODO which exist) now drive the states.

`BRN-005` was taken before `BRN-004` because the state machine is pure domain logic
with no dependency on a repository, while `BRN-004` needs one; the IDs are stable
and were not renumbered.

`jarvis_application::run_controller` joins the four pieces: `execute` advances the
state machine, reads the transcript, performs one model turn through
`ModelProvider`, records the attempt, and persists every transition with its public
event in the same write. A plain question now reaches `Completed` with five activity
events, so the run loop the earlier slices described exists as code rather than as
disconnected parts. Driving the real path is what found three defects that no unit
test had — a provider serving no model left the run stuck in `AwaitingModel`, a
refused stream left its model-call row open, and the request builder was a method
that read no port — and the `BRN-008` evidence records them.

What this is **not** yet: a working product. The controller performs exactly one
model turn and **refuses a tool intent** with a typed, terminal
`run.tools_not_implemented` rather than fabricating an observation, because the tool
fabric is Milestone 3; there is no turn/token/cost/time budget, no retry or fallback,
no `Waiting` entry because nothing suspends and resumes yet, and no timeout or
daemon-restart behavior. No endpoint or CLI path calls the controller, no event is
published to a client, no controller state is mapped onto the client-visible set,
and nothing retrieves the context candidates `BRN-006` budgets — so the pieces that
exist are joined to one another by the controller but not yet to a caller. Step 16 is
only partly done — `agent_steps` has a table but no adapter. Step 17 (`BRN-007`,
HTTP/SSE and CLI surfaces) and the E2E step are not started, and `BRN-003` remains
gated on its `REQUIRED` evidence note.

## Acceptance

The slice is complete when:

- focused unit/repository tests pass;
- real-daemon E2E passes on Windows and Unix-like CI;
- SSE emits ordered IDs and one terminal event;
- restart retains completed state and truthfully recovers incomplete state;
- disconnect/cancel semantics match the contract;
- secret-canary scan passes;
- `cargo fmt`, Clippy with warnings denied, and workspace tests pass;
- docs, TODO evidence, and generated contract snapshots are current.