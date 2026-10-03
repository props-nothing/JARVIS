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

It also proves the run's **retry policy is durable**, added by `BRN-050`. The policy was
implemented and unreachable: `RunBudget::with_retry` had no caller outside the tests, so every
run created through the daemon recorded `max_attempts: 1` and no transient failure could ever be
retried. Three checks, and the first is read from **storage** rather than a response, because the
controller reads this policy when a later attempt fails — long after the create response is gone:

- a run created with no stated policy records the daemon's **default** (three attempts with a
  backoff), not one attempt;
- a **caller-stated** policy is the one recorded, which the check above cannot establish — a
  daemon that ignored the field entirely would still pass it;
- a policy **outside the daemon's bounds** is refused with `request.semantic_invalid` rather than
  clamped, because a clamped value is one the caller cannot detect and would size its own
  reconciliation window against.

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

## `tool-recovery-journey.mjs`

Proves the **composition root** half of `TLS-006`'s startup recovery: that a real `jarvisd`
settles tool-call reservations a previous shutdown stranded. The unit tests build their own
repository and their own ports, so they prove the pass works when called — they cannot prove the
daemon calls it, with the ledger repository wired to the same pool, at the right point in startup.

It seeds three rows **directly into the profile's database**, because no executor exists to make a
tool call through the API — one that is `reserved` (claimed, never dispatched), one that is
`executing` (dispatched, outcome unknown), and one that is already terminal — then restarts the
daemon and asserts:

- the `reserved` row is settled `cancelled` with the `tool.cancelled` outcome a terminal row
  requires, and no dispatch is recorded on it;
- the `executing` row is moved to `reconciling` and keeps its dispatch fact;
- the terminal row is left exactly as it was;
- the daemon **logs** the recovery report with both counts, and a second restart changes nothing
  and reports nothing.

### What this harness found

- **The conversion predicate reached two of the six states the classification answers for.** The
  pass pages `awaiting_conversion`, whose predicate read `state = 'executing'` — but
  `classify_interrupted` answers `SafeToRetry` for five pre-dispatch states. So a stranded
  **pre-dispatch** reservation was never settled: its dead holder kept the unique-index entry and
  every later retry of that key was answered `InFlight`, i.e. *wait on a process that is gone* —
  the exact harm the pass exists to remove. Restoring the narrow predicate fails three checks here,
  the first naming the cancelled row.
- **A terminal target with no outcome.** `apply` records `None` for a terminal transition handed
  `None`, and a terminal row with a `NULL` outcome is refused as corruption by the reader — so the
  pass's cancelled write must carry `tool.cancelled`, or the row it wrote is unreadable on the next
  start.
- **The report was computed and dropped.** The daemon logged the run pass's summary and discarded
  the tool-call one. Disabling the new log line fails the report check here and nothing else.

## `approval-journey.mjs`

Proves that the approval surface is reachable from a **real** `jarvisd`. The handler tests build their
own `ApiState` with `.with_approvals(...)`, so they prove the handlers work when the state carries a
service — they cannot prove the daemon composition attaches one. A daemon that never called
`.with_approvals` would pass every handler test while answering `503 service.not_ready` to every real
client.

It seeds a pending approval **directly into the profile's database**, because no executor exists to
create one over the API — a request comes from a policy `Ask` decision on a tool call. The seeded rows
use the caller's own workspace and principal, read from the run the create path wrote, so the scope is
the one the daemon resolves rather than an invented value.

Nineteen checks, including the CLI's paths: the composed listing answers `200` with the approval, its
preview, its channels and state, and reports `has_more:false`; a bounded page is spent on a row the
caller may decide; an unsupported filter is refused rather than ignored; a decision applies and records
the **server-derived** principal and channel; a repeat answers `applied:false`; the audit trail has
exactly one row; a fingerprint mismatch is refused with the contract's code and changes nothing;
`approvals show` and `approvals list` print the daemon's own bodies; a refused CLI decision prints the
daemon's own code; and the decision survives a restart.

**Two rows are seeded, not one: a live approval and a second already past its deadline.** That makes the
listing's row count the read-path expiry assertion — a surface that still offered the lapsed row would
report two — and lets the detail read assert the recorded `expired` state directly. The lapsed row is
checked in **storage** as well as on the wire, with one audit row, because hiding a lapsed row from the
response would satisfy the response check alone while leaving the prompt to reappear in every later
listing. Ordering is `expires_at ASC`, so the lapsed row sorts *first* in a `limit=1` page — which is
also what proves the sweep **re-reads** rather than filtering: a filtered page would have come back
short.

The faithful-render check also asserts the contract's "tool source/**schema identity**": the view carries
`tool_source_kind`, `tool_source_owner`, `tool_source_version`, and `schema_fingerprint` beside the
capability, which is what lets a client detect the `ACC-024` case where the tool behind a name was
replaced. The fixture gives the schema a **different** digest from the action fingerprint, so a
projection that swapped the two would fail rather than pass by coincidence.

### What this harness found

- **Removing `.with_approvals(...)` from `daemon.rs` fails six of the ten checks** with
  `503 service.not_ready`, while the entire handler suite stays green. This is the same
  composition-root gap `BRN-014` fixed for the policy surface, and the reason a composition needs an
  end-to-end test rather than another handler test.
- **The fixture's deadline must be far in the future.** The handler reads the real clock, so a
  near deadline made the first version of the HTTP handler tests exercise the expiry path instead of
  the decision path — a test that would start failing when the calendar moved. The read-path expiry
  checks invert this deliberately: their row carries a deadline in the **past**, which is how the
  lapse is reachable without a test-only clock knob.
- **The lapse has to be recorded, so the journey asserts storage.** Bypassing the listing sweep fails
  with `exactly one approval is live (the second is lapsed), got 2`; bypassing the detail sweep fails
  with `left: Pending, right: Expired`. A response-only assertion would be satisfied by a surface that
  hid the row while leaving it `pending`, which is the state that makes a dead prompt reappear.

## `tool-grant-cli-journey.mjs`

Proves the `jarvis grants` command drives a **real** `jarvisd`. Every other harness here speaks HTTP
directly, because it is testing a *surface*; this one runs the shipped `jarvis` binary, because the thing
under test **is the client** — whether an operator can drive tool authorization from the product. `TLS-015`
recorded that as absent ("no CLI commands for the surface yet"), and a harness that spoke HTTP would prove
the routes work — which the other journeys already do — while leaving the command unexercised.

Eight checks: `grants list` reaches the composed service rather than answering `service.not_ready`;
`grants create` writes a grant and prints the stored row; `grants replace` **succeeds** and advances the
version; a stale version is a conflict carrying `tool.grant_version_conflict`; `grants revoke` withdraws the
row and keeps it for audit; `grants deny add` stores a refusal the listing shows with its reason;
`grants deny remove` takes it away; and a run completes while a refusal is in force, so a refusal is an
observation rather than a run fault.

The CLI's own unit tests assert the *request builders* — the query string, each verb's body shape, the
inverted `--workspace` flag. They cannot prove the daemon accepts those bytes, and this harness is what does.

### What this harness found

- **`PATCH` was unreachable, and no test of any kind had invoked `replace_tool_grant`.** The route reads
  `expected_version` from the body and hands the body to a shared parse typed as `WriteToolGrantRequest` —
  which carries `#[serde(deny_unknown_fields)]` and does not model the field. So the one body the route
  *required* was the one body its own parse refused, and every replace answered `400 request.invalid`. It
  survived because the route was asserted to **exist** (a compile-time guarantee) and each code it can emit
  was asserted to be in the contract table, and both were true of a handler that could never succeed. This is
  `TLS-018`'s lesson one layer out: a route's existence is not its reachability, and a command's is not
  either.
- **A widening refusal was reported as `jarvis.internal`.** The service carried the *repository's* field name
  (`grant_scope`) into `Widens { code }`, and a bare `grant_scope` is in no owned namespace, so the envelope
  constructor replaced it — the four `tool.grant_*` widening codes the contract documents could never be
  sent, and an operator who asked for too much risk was told the daemon had an internal error. The code-table
  scan reads **owned string literals**, and this value arrived as a `String` from another crate's `what`
  field, so it was structurally invisible to the guard. `GrantWidening` is now a typed enum whose codes are
  literals in the service.
- **The refusal format is a code and an advice line, not the daemon's envelope.** A non-2xx from the
  transport arrives as `ClientError::Rejected { status, code }` rather than as a body, so the CLI prints
  `error: <code>` and discards `message`/`request_id`. The first version of this harness parsed JSON out of
  stderr and saw `null`; reading the line is what the client actually emits.

## `provider-smoke.mjs`

Proves Milestone 2's exit gate — *"a gated real-provider smoke test streams a response"* — and it is
the **only** harness that reaches a real model. Every other test here and in the workspace runs
against the scripted provider or a fake server, so none of them shows that JARVIS can talk to a real
OpenAI-compatible endpoint, translate its stream, and record the answer.

It starts a daemon configured for a **real endpoint**, creates a run through the real control API, and
asserts:

- the daemon is wired to the **configured** provider rather than the scripted fallback, which is the
  composition check and the reason the run is not enough on its own;
- the run reaches `completed`, so the adapter's transport, framing, translation, and the daemon's
  composition all worked;
- the run's durable events carry the **model's own text**, which is what makes this a proof about the
  provider — the scripted provider's fixed acknowledgement cannot satisfy it;
- `jarvis ask` prints that answer and exits 0, which exercises the client's own SSE reader against a
  real chunked stream, where a framing defect shows up as a truncated answer.

### Why it is gated, and how the gate behaves

It targets **Ollama on loopback** by default, because that is the endpoint this build can reach: the
adapter refuses a non-loopback host since the workspace has no TLS implementation, so a cloud endpoint
is not testable here at all. Ollama also needs no credential, which makes the gate "is a local server
listening" rather than "is a secret present" — and a secret must never be needed to run a test.

When no server answers, the harness prints its skip **and says nothing was proved**. That distinction
is the point: a skip that printed a pass would make a CI log from a machine without a provider look
identical to one that had proved the real path, which is the single failure a gated test must not
have.

Host, port, base path, and model all come from the environment (`JARVIS_SMOKE_*`), so pointing it at
another compatible server is a configuration rather than a code change.

### What this harness forced

Two additions and one defect, and none was visible from the contract or from a fixture:

- **A base path.** Ollama serves `/v1/chat/completions`, not `/chat/completions`, as do vLLM, LM
  Studio, and most gateways — so the adapter was posting to a path that 404s on a real server while
  **every fixture passed**, because a fixture's server is built to the adapter's own assumption.
- **A model-name mapping.** A JARVIS model id is a lowercase dotted slug and cannot contain the `:` in
  Ollama's `glm-5.3-flash:cloud`, so the adapter could not name an Ollama cloud model at all. This is
  a real namespace difference, not an Ollama quirk: providers also use `/` and `@`.
- **A chunked-transfer defect in the CLI's own client.** Making the event stream live made hyper
  emit `transfer-encoding: chunked`, and the hand-written client read the framing as if it were the
  message. It *appeared* to work because each frame fit in one chunk; a larger frame splits across
  chunks, puts a size line inside a `data:` line, and `jarvis ask` prints a truncated answer and
  exits 0. See `BRN-003` for why the socket-level test, not the decoder's unit tests, was what caught
  it.
## `memory-live.mjs`

The gated **live memory journey**, run against a real model (Ollama on loopback, including its `:cloud`
models) through the real `jarvisd` and `jarvis` binaries:

```bash
cargo build -p jarvisd -p jarvis-cli
node tests/e2e/memory-live.mjs target/debug
```

It asks a question the model cannot know the answer to (a favourite slide-deck colour) and shows, in order:
the model does **not** know it with nothing remembered (the control that makes the rest mean something);
`jarvis memory remember` stores it and `jarvis memory search` finds it by words; the real model then answers
with it; an unrelated question is unaffected; after a real daemon restart the model still answers with it;
and after `jarvis memory forget` it no longer does. The assertions are about what JARVIS owns (the fact reached
the prompt, and stopped reaching it), never about a model obeying an instruction in general.

Gated like `provider-smoke.mjs`: with no server on the Ollama port it prints `skip` and exits 0, and the summary
says nothing was proved. `tests/memory_recall.rs` is the deterministic counterpart that runs everywhere.

### What this harness found

Its first run failed the restart step with `jarvis.transport_failed`, and the cause was the harness, not the
product: on Windows `kill()` ends the daemon abruptly, so its `discovery.json` stays behind, and the script read
that stale file before the new daemon had published its own. The harness now removes the stale file when it
restarts, and says why. The client's behaviour on a stale file — reporting the daemon unreachable — was correct.
## `mcp-tool-offer.mjs`

Deterministic (no model, no network): a real `jarvisd` composes the repository's MCP fixture server as a child
process and a local fake OpenAI-compatible server records the request the daemon sends. It asserts the model is
**offered the MCP tool** with its description and its own argument schema, and that the native tool is still
offered beside it. This is the check that fails if the daemon stops passing composed MCP tools to the pipeline's
catalog — verified by making it do exactly that.

```bash
cargo build -p jarvisd && cargo build -p jarvis-infrastructure --example mcp_fixture_server
node tests/e2e/mcp-tool-offer.mjs target/debug
```

### What this harness found

It exists because of a live run: the daemon composed MCP servers and could route to them, yet built the tool
catalog from native tools alone, so no model was ever told an MCP tool existed. No unit test could see it — each
layer was correct, and the defect was in the composition root. A first capture also showed the model was offered
only a name and an empty object schema, so a real model's arguments failed the tool's schema.

## `mcp-live.mjs`

The gated live counterpart, against a real model (Ollama, including `:cloud` models): the daemon composes the
fixture server, an operator grants the tool through `jarvis grants create`, the model proposes a call, the
pipeline **withholds** it and raises an approval, and `jarvis approvals approve` records the decision. Then the
run **continues**: the tool runs in the MCP child and the model answers with the sentence only that child could
produce — a real assertion, verified against `deepseek-v4.1-flash:cloud`. A model that declines to call the tool is
a skip, never a failure: whether a model chooses a tool is its behaviour, not JARVIS's.

## `activity-feed.mjs`

Deterministic (no model, no network): the real daemon, CLI and MCP fixture child against a fake model. Opens
`GET /api/v1/activity` and checks that a feed with no position **starts from now** (an earlier run is not replayed); that
two runs started together, one **parked on an approval**, appear in one stream with strictly increasing cursors and the
parked run's `run.approval_requested`; that approving surfaces the rest of that run in the same stream; that streamed
output text is absent unless `?deltas=true`; that a reconnect with `Last-Event-ID` delivers **exactly the frames that
were missed**; that `?after=0` replays retained history; and that an unauthenticated request is refused and a malformed
cursor is `400`. Making the cursor comparison inclusive fails five checks; dropping the delta filter fails one.

```bash
node tests/e2e/activity-feed.mjs target/debug
```

## `kill-switch.mjs`

Deterministic (no model, no network): the real daemon, CLI and MCP fixture child against a fake model, on one profile.
One run is **parked on an approval** (the fixture tool asks at the default level) and another is **mid-way through a
slow model call**. `runs list` shows both and only the parked one says `awaiting_approval`; `runs stop-all` reports one
run signalled and one parked run cancelled; both end `cancelled`, the approval the parked run waited on is withdrawn,
nothing is listed as active, repeating the command stops nothing, and a new run is still accepted afterwards (a stop,
not a latch). Making `stop_all` signal nothing fails three journey checks and a unit test.

```bash
node tests/e2e/kill-switch.mjs target/debug
```

## `files-journey.mjs`

Deterministic (no model, no network): a real `jarvisd` and CLI over temporary directories declared as
`[[tools.files.roots]]`, with a recording fake model that proposes scripted file-tool calls. At `balanced`: a read runs
with no prompt and its text reaches the model; a write raises a prompt whose preview shows the **path and the start of
the content**, nothing is written while it is open, and `approve --remember` creates the file and lets the next write run
without asking; a write into a read-only root and a `../secret.txt` read are refused, and a sentinel file beside the
root never reaches the model. At `autonomous` a write runs with no prompt, and a profile with no roots is never offered
a file tool. Removing the writable check fails two journey checks and a unit test.

```bash
node tests/e2e/files-journey.mjs target/debug
```

## `autonomy-journey.mjs`

Deterministic (no model, no network): the real daemon, CLI and MCP fixture child against a recording fake model, on
three fresh profiles — and **no grant is ever written**. `balanced` (the default): the first call reaches a prompt
rather than a refusal, `approve --remember` runs it, a later call with other arguments runs **without asking**,
`approvals standing` lists the permission and `approvals cancel` revokes it, after which the tool asks again.
`autonomous`: the same reversible write runs with no prompt at all. `ask`: the original behaviour, where the model
is told `tool.permission_denied`.

```bash
node tests/e2e/autonomy-journey.mjs target/debug
```

### What this harness found

Revoking a standing permission **made every later policy read fail**: the approval reader reconstructed a
cancellation as one step from `pending`, so an `approved -> cancelled` row read as corrupt (`storage.row_corrupted`)
and the next run failed. The state machine also refused the revocation edge outright although the service and the
contract both described it. Both are fixed and covered; the first only a real daemon with a real database found.
Making the standing flag always false in the pipeline fails the journey at the second run.

## `approval-resume.mjs`

Deterministic (no model, no network): a real `jarvisd`, the real CLI, and the MCP fixture child process, with a
fake OpenAI-compatible server standing in for the model. Its first request proposes a call to the fixture's
`read_file` tool and its second answers; **every request is recorded**, so the assertions are on what the daemon
actually sent the model.

Five scenarios on one daemon (the fifth restarts it): **approve** (the run stays open on a pending approval with the tool not run; the CLI
decision continues the *same run*; the model's second request carries the tool's own output; the event stream shows
the park, the resumed execution and the completion in order; the one-shot approval is `consumed`), **the same action
asks again** (a second run raises a new approval instead of reusing the spent one), **reject** (the run still
completes, the model is told `tool.approval_rejected`, and the tool never ran), and **cancel while parked** (a run
with no task to signal is cancelled, and its prompt is withdrawn rather than left pending), and **restart** (a run parked
before the daemon is killed and restarted keeps its pending approval, is not failed by recovery, and continues to
completion when the approval is decided on the new daemon; falsified by breaking the run-recovery exclusion).

```bash
cargo build -p jarvisd -p jarvis-cli && cargo build -p jarvis-infrastructure --example mcp_fixture_server
node tests/e2e/approval-resume.mjs target/debug     # JARVIS_E2E_DEBUG=1 prints the daemon log on failure
```

### What this harness found

Before it existed, a call that needed approval **never parked the run at all**: the controller moved through
`Observing` and then asked for a transition from a state the run was no longer in, the store refused it, and the
detached task swallowed the error — so the run read as running for ever beside a pending approval. A controller
test that reads the *stored* state found that; this harness proves the fix on a real daemon. Dropping the HTTP
handler's hook that continues the run fails it with `timed out waiting for the run to complete after approval`,
while every handler and controller test stays green — the composition-root gap this directory exists for. It also
established that the public run state is deliberately coarse (a parked run reads `model_running`), so the
journey identifies a parked run by its pending approval and an unanswered model rather than by a state name.