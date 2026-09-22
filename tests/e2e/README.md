# End-to-End Tests

This directory owns packaged, cross-process acceptance tests: a real client against a
real `jarvisd`, asserting durable state rather than only status codes.

## Running

Build the workspace first, then point the harness at the binary directory:

```bash
cargo build --workspace
node tests/e2e/disconnect-journey.mjs target/debug
```

The harnesses are **dependency-free** — Node's standard library only — so they run on
every supported target with no install step, and on any Node version this repository
already requires for `scripts/`. They take the directory containing `jarvisd` and
`jarvis` as their first argument, so the same file runs against `target/debug`,
`target/release`, or a staged install.

Each harness:

- uses a fresh empty `--profile` under the system temp directory, so no ambient state
  can make it pass and a stale database cannot make it fail;
- resolves the daemon's discovery file and credential from that profile rather than
  hard-coding them;
- asserts durable state read back over the API, not just responses;
- removes its profile on the way out.

## `disconnect-journey.mjs`

Proves Milestone 2's exit gate — *"killing a client does not corrupt the run;
cancellation behavior is explicit"* — and the first vertical slice's acceptance
criterion *"disconnect/cancel semantics match the contract"*. Both were recorded as
outstanding three times before this harness existed.

It deliberately does **not** use the CLI as its client. `jarvis ask` follows a run to
its terminal, so a client that *disappears* is not something it can express. The harness
speaks the local control API directly with Node's own HTTP client, which is what makes
"the client was killed" a real condition rather than an assertion about a process the
test controls.

Four sections:

1. **Disconnect does not cancel.** A client destroys its socket mid-stream. The run must
   remain reachable and must *not* be cancelled — the contract's explicit prohibition —
   then complete on its own with exactly one terminal event and contiguous event
   sequences from 1.
2. **Cancellation is explicit and settles.** A cancel is accepted (`202`, or `200` if the
   run already finished), a repeated cancel is idempotent, and the run reaches a durable
   terminal state. The terminal is *not* asserted to be `cancelled`: the scripted
   provider finishes in about twenty-five milliseconds, so the cancel legitimately
   races it. The assertion with teeth is that no run is ever left non-terminal.
3. **Cancelling a terminal run is a no-op.** `200`, not a fault.
4. **A terminal run is stable across reads.**

### What this harness found

It is worth recording, because each was invisible to unit tests:

- A cancel's status was derived from a *separate* pre-read of the run, so a run that
  finished between the two reads was reported `202` as though cleanup were in flight.
- `ContextBuilding`, `Planning`, and `Observing` had no `Cancelled` edge, so a run
  cancelled in one of those states could never leave it.
- The entry cancellation check hard-coded `Received` as the transition origin, so a
  cancel arriving after the run advanced was refused as an illegal edge and the refusal
  was swallowed by the detached task.
- The storage layer refused a contended transition with `database is locked`
  (`SQLITE_BUSY`) and reported it as a generic storage fault. A deferred transaction's
  read→write lock upgrade fails immediately and **bypasses the busy handler** — SQLite
  documents this as deadlock avoidance — so `busy_timeout` provably did not cover it and
  the run was left permanently non-terminal. Fixed with `BEGIN IMMEDIATE`; see
  `crates/jarvis-infrastructure/src/storage/repositories_tests.rs`, whose
  `concurrent_transitions_on_one_run_never_fail_with_a_storage_fault` reproduces it
  against a **file** database. An in-memory fixture cannot: it is pinned to a single
  connection, so two writers can never contend.

The deterministic half of the cancellation contract — that a cancel arriving *during*
delivery must win — is covered in-process by `run_controller`'s
`a_cancel_arriving_during_delivery_ends_the_run_cancelled`, which cancels from inside the
provider's own stream so there is no race to lose.
