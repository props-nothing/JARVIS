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
## `policy-surface.mjs`

Proves that the model data policy selector is reachable from a **real** `jarvisd`. The
handler tests build their own `ApiState` with `.with_policies(...)`, so they prove the
handlers work *when the state carries a service* — and cannot prove the daemon composition
passes one. A daemon that never called `.with_policies` passes every handler test while
answering `service.not_ready` to every real client, which is precisely the failure mode a
composition root needs an end-to-end test for.

Eleven sections:

1. **The routes exist and are authenticated.** Each path is distinguished from the router's
   fallback (`resource.not_found`), because "the route is absent" and "the route answered
   something else" both produce a non-200. A bad credential must be refused with `401`:
   binding to loopback is not authorization, since another local user can reach the port.
2. **A store with no policy row is `model.policy_not_found`, not `service.not_ready`.** The
   contract keeps "no store" (a readiness fact a client retries) and "no policy in force"
   (a decision a client acts on) distinct. This is the assertion that proves the composition,
   because the handler short-circuits to `not_ready` *before* reading anything — and it also
   asserts the body is not the not-ready envelope, so a client reading only the body cannot
   see one state rendered as the other.
3. **The route probe with no policy reports the missing policy, not a refusal.** It must not
   be `model.policy_unsatisfied`: nothing was offered, so "your policy refused every model"
   would name a decision that was never made. The two codes are one word apart, which is why
   the `RouteSelectionFailure` enum keeps them apart and why this is asserted over the wire.
4. **Two identical probes answer byte-identically.** A per-request clock or a second store
   read inside the response would make two probes of one daemon disagree.
5–11. **The write path.** A `PUT` creates version 1 and reports the version *it* chose; a read
   agrees; the reply and the read carry every rule the submission did; an unrestricted
   allow-list stays **absent** rather than becoming an empty array; a looser submission is
   accepted as version 2 while the stricter locality survives; a stale precondition is a
   `409` that leaves the stored policy at version 2; a write with no `Idempotency-Key` is
   refused before it writes; an unsupported rule value is refused and advances nothing; and
   the daemon still reports liveness.

### What this harness found

- Removing `.with_policies(...)` from `daemon.rs` turns every policy request into
  `service.not_ready` while the whole handler-test suite stays green. The harness fails on five
  checks, which is what makes it the test that covers the composition rather than the handler.
- **A `PUT` reply that dropped `allow_fallback`.** The reply rendered the rules through the
  six-field *statement* shape, which omits `allow_fallback`, `allowed_providers`, and
  `allowed_models` — so the daemon's own description of what it had stored omitted a rule, and a
  client doing read-modify-write would resubmit a body that reset it. The same omission made
  `GET` un-round-trippable. Both responses now carry the full nine-field shape. This is why the
  section asserts the reply and the read agree on **every** rule rather than only on `locality`:
  a partial comparison is what let the omission through the first time.

The merge direction — that a submission can only narrow what is in force — is covered in
`jarvis_application::policy_service`'s own tests, which drive the write and the evaluation through
one service so the stored policy is shown to reach selection. Replacing the merge with the raw
submission fails three of them plus the HTTP test, and the failure body names the widening.

The wire spellings are covered in `jarvis_infrastructure::http::policy`'s own tests, which
enumerate **every** variant of every value family. That location is deliberate: a handler test
can only reach the variants a fixture produces, and the defect found in the previous round —
`rejected_view` rendering `RejectionReason`'s operator prose `"locality violated"` where the
contract specifies the code `locality_violated` — survived because the one rejection code the
handler test reached was produced by the domain, so the test agreed with the defect.