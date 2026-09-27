# API and Protocol Surfaces

Status: PROPOSED

## Surface Separation

JARVIS exposes several protocols for different consumers. They share application
services and authorization but do not share accidental wire contracts.

| Surface | Consumer | Purpose |
| --- | --- | --- |
| JARVIS API | CLI, desktop, web, mobile, service clients | Full product operations |
| Activity stream | Interactive clients | Ordered public run/workflow events |
| Runtime protocol | External agent runtime processes | Start/resume/cancel and runtime events |
| MCP server | External AI hosts/agents | Narrow capability export |
| MCP client | External tool servers | Capability import |
| OpenAI-compatible edge | ElevenLabs and selected compatibility clients | Voice/model-style streamed turns |
| Webhook ingress | Providers | Authenticated external events/callbacks |
| Voice control callback | Voice carrier | Call completion, handoff reason, and session status after a call |

Compatibility endpoints are adapters. The internal application must not be
forced into an OpenAI, MCP, or runtime-provider data model.

## JARVIS HTTP API

Use `/api/v1` for the first public major. Candidate resources:

```text
POST /api/v1/chat
POST /api/v1/runs
GET  /api/v1/runs/{run_id}
POST /api/v1/runs/{run_id}/cancel
GET  /api/v1/runs/{run_id}/events

GET  /api/v1/conversations
GET  /api/v1/conversations/{conversation_id}/messages

GET  /api/v1/tools
GET  /api/v1/approvals
GET  /api/v1/approvals/{approval_id}
POST /api/v1/approvals/{approval_id}/decide
POST /api/v1/approvals/{approval_id}/cancel
POST /api/v1/approval-grants/{grant_id}/revoke

GET  /api/v1/memories
POST /api/v1/memories
POST /api/v1/memories/{memory_id}/correct
DELETE /api/v1/memories/{memory_id}

GET  /api/v1/connectors
GET  /api/v1/runtimes
GET  /api/v1/models
GET  /api/v1/model-data-policy
PUT  /api/v1/model-data-policy
GET  /api/v1/workflows
GET  /api/v1/events

GET  /health/live
GET  /health/ready
GET  /api/v1/diagnostics/summary
```

The Foundation and first runnable Brain subset is governed by the
[accepted local control API contract](../contracts/local-control-api.md). The
remaining resources are conceptual until their owning OpenAPI contracts are
accepted.

Model policy behavior is already governed by the
[model data policy contract](../contracts/model-data-policy.md); generated API
schemas must implement that contract without weakening precedence or evidence
requirements.

Approval list/detail/decision/cancel and standing-grant revocation behavior is
governed by the [approval contract](../contracts/approval-contract.md), including
server-derived channel assurance, concurrency, idempotency, and resume semantics.

## Request Rules

- JSON requests use explicit media types and bounded body sizes.
- Unknown fields are rejected for commands where silent typos are dangerous.
- Every request has request/correlation IDs; client-provided IDs are validated.
- Mutation requests support an `Idempotency-Key` where logical retry is valid.
- Preconditions use entity/version tokens for concurrent updates.
- Deadlines and cancellation propagate to application services.
- Workspace is resolved from session/authorized route, not trusted from body.
- Sensitive fields are marked in schemas for logging/diagnostic redaction.

## Error Envelope

Return stable machine codes with safe human detail:

```json
{
  "error": {
    "code": "approval.expired",
    "message": "This approval is no longer valid.",
    "request_id": "...",
    "retryable": false,
    "details": {}
  }
}
```

Provider stack traces, SQL errors, secret values, raw auth responses, and
untrusted payloads do not cross the API boundary. Internal diagnostics use the
request ID.

## Pagination and Filtering

Use opaque cursors for changing datasets. A cursor binds query shape, workspace,
sort, and backend/version information and has bounded lifetime. Never expose raw
SQL offsets or allow a cursor from one workspace to query another.

List endpoints have maximum page sizes and explicit stable ordering. Filters are
schema-defined rather than arbitrary query languages in v1.

## Streaming

### SSE

SSE is the default for one-way run/activity and OpenAI-compatible streams.

- each event has event type, stable event ID, run ID, sequence, and JSON payload;
- heartbeats are comments or typed keepalive events, never fake output;
- reconnect uses last event ID only when retention/replay is supported;
- one terminal event closes the logical stream;
- client disconnect does not imply durable-run cancellation;
- slow clients have bounded buffers and an explicit overflow/reconnect outcome.

### WebSocket

Use WebSocket for bidirectional interactive control where SSE plus HTTP commands
is insufficient, such as low-latency desktop/voice activity. Define a versioned
envelope, handshake, auth expiry/rekey, ping/pong, subscriptions, backpressure,
and reconnect behavior. Do not send ad hoc JSON shapes per feature.

### Implemented evidence (Milestone 2, run resources)

The first four run endpoints are served, and `jarvis-protocol::run` is the single
serialization definition the daemon and the CLI both use rather than each maintaining
its own. Three points from the rules above are now enforced by construction:

- **One terminal event, and the server closes.** A stream is rendered from the stored
  activity events, which the repository already guarantees contains exactly one
  terminal event per run, so the framing rule holds because the source does rather
  than because a writer remembers.
- **Reconnect uses a last event ID only where replay is supported.** A `Last-Event-ID`
  is resolved against the retained events, and a position this daemon does not have
  returns `409 stream.replay_unavailable` instead of starting over — because a silent
  restart would deliver a gap as if the stream were complete.
- **Client disconnect does not cancel a run.** The run is driven by a background task
  owned by `jarvis_application::run_service`, so its lifetime is not tied to the
  connection; cancellation is the explicit command endpoint.
- **The connection is now held open.** The events endpoint replays the retained events
  from sequence 1 and then pushes each new one as it is published, which is what the
  rules above assume — "heartbeats are comments", "slow clients have bounded buffers" —
  and none of them is meaningful on a connection that already closed.

**How the live half is built, and why it is not a queue.** The durable event store stays
the single source of truth: a follower's position is a **sequence number** and every read
is `load_events(from_sequence)`. The notification that wakes a follower carries **no
payload** — it is permission to read again — and that is the property that makes a bounded
buffer safe. A slow follower therefore loses *wake-ups*, not events: it reads from its
position and catches up, which is exactly the contract's "a slow consumer is disconnected;
it can replay from its last delivered event" without a special case. Delivery from the
notification would instead make the buffer a queue whose overflow is data loss.

Two consequences are worth stating because they are the failure modes:

- **The stream ends on the durable terminal state, not on a notification.** A run that
  finishes between two reads is seen in the read, so a notification that never arrives — or
  arrives after the terminal — cannot leave a client holding an open connection for a run
  that is over.
- **The body's own shape is not hand-written `poll` logic.** The response is a bounded
  channel fed by an ordinary `async` follow task, with a thin `Stream` implementation that
  delegates to the channel's `poll_recv`. A `Stream` that polled a future borrowing the
  state it must also reach would need unsafe code or a self-referential type; that hazard is
  avoided rather than solved, using the same task-plus-channel shape the model adapter
  already uses for a provider's frames.

**Keepalives are emitted while the follower waits.** The rule above — "heartbeats are
comments or typed keepalive events, never fake output" — had a producer-less frame until
now: `jarvis_protocol::run::keepalive_frame` existed with a test asserting its shape and no
caller, so the product could describe a comment it never sent. The follow loop now emits one
per interval **only while it is waiting**, which is the state the comment exists for: a run
that is thinking has nothing to send, and a connection silent for long enough is dropped by
whatever sits between the daemon and the client. Two properties make it safe rather than
merely present:

- **It is a comment, so it carries no `id:` and consumes no sequence number.** That is what
  the contract requires, and it is why a keepalive cannot be modelled as a synthetic event:
  a client resuming from `Last-Event-ID` would be sent to a position that never existed. The
  test asserts the *contiguity of the event sequences* across a stream containing comments,
  which is the same rule stated the way a client experiences it.
- **The timer starts at the follow loop, not at each wake-up**, and its first immediate tick
  is consumed. A per-iteration timer would be reset by every event, and a stream that did not
  consume the first tick would emit a comment as its **first** output — a keepalive meaning
  "nothing has happened" sent to a client that had just connected, ahead of the event telling
  it the run exists. Missed ticks are not repaid as a burst, because a delayed loop emitting
  several comments at once is traffic with no purpose.

**A stalled follower is told why its stream ended.** The rule above — "slow clients have bounded
buffers" — had a channel but no disconnect: a full channel parks the follow task, and a parked task is
indistinguishable to the client from a run with nothing to say. The connection stayed open, nothing was
sent, and the contract's "a slow consumer is disconnected" described a control that did not exist. Two
things were needed, and the split is the reason this is worth recording:

- **A bound on how long one hand-off may take**, measured on *delivery* rather than on the run. A
  follower that is keeping up never waits on a send, so the bound cannot fire for it; a follower that
  has stopped reading waits for ever. This is what makes the buffer bound *observable* rather than
  merely real.
- **A signal that travels beside the channel, not through it.** The one moment an overrun exists is the
  moment the channel is full — a follower that were reading would never trigger the bound — so a signal
  sent through it would be blocked behind the congestion it is describing. It is therefore recorded
  beside the channel and delivered by the body as its **last** item, after everything the daemon did
  manage to hand over. Delivering it last is also what makes the client's instruction actionable: it
  says "resume from the last event id you saw", and the last frame the client read is exactly that.

The signal is a real event and not a comment, which is the mirror of the keepalive's reasoning: a
comment is for a client that is waiting, and this is for a client that must act. It carries **no `id:`**
and consumes no sequence number, so a client that resumes on it is returned to the last genuine
position rather than to one that never existed. Nothing is dropped and nothing is skipped, because the
events are durable and delivery is bounded by *time* rather than by a position — a client that is
simply slow loses nothing when it reconnects, which is the property that makes "disconnect the slow
consumer" safe to do at all. `send_bounded` is where this lives; `FOLLOW_CHANNEL_DEPTH` is what makes
the bound necessary, and `DEFAULT_STREAM_OVERRUN_TIMEOUT` is what the daemon serves.

**Generated OpenAPI and golden fixtures are still outstanding.**

**The client streams, and it resumes.** The reference client (`jarvis`) follows a run on **one
connection**, printing each delta as it arrives rather than reconnecting on a timer — which matters
because a polling client shows the answer in bursts at whatever the interval was, and the interval is
not a property of the model. Two consequences of holding the connection for the run's lifetime are
worth recording, because both were absent while the endpoint replayed and closed:

- **The parser must be incremental.** A streamed body arrives in pieces that respect no framing
  boundary, so a piece can end in the middle of a `data:` line — normal for any frame larger than the
  socket buffer. A parser over a whole body would have to buffer the entire stream, which defeats the
  purpose, and one that expected whole frames would drop or truncate the frame it split. The client's
  parser therefore accumulates an incomplete frame and is asserted against **every split point** of a
  fixture frame, since a parser that happened to tolerate a split at a newline would look correct
  against a single hand-picked case.
- **A dropped connection is resumed, not abandoned.** One long-lived connection is exposed to failures
  a 50 ms poll never was — a daemon restart, a socket reset — and the failure mode would be a follow
  that had already printed half an answer and then stopped. The client keeps the last delivered event
  id and reconnects with it, which the contract makes exact ("`Last-Event-ID` resumes strictly after
  that event"), so a resume cannot duplicate output. Attempts are bounded, and a failure with no
  position to resume from is not retried at all: the retry would be the same failure.
- **An overrun is resumed as well, and it is classified rather than recognised.** The daemon's signal
  is not a failure and not an ending, so a client that treated "my stream stopped" as one uniform
  condition would either report a working run as finished or print a fault for a decision the daemon
  made on its behalf. The classification is therefore a **function over the frame** rather than an
  `if` inside the callback — because the interesting property is what a frame is *not*, and an `if`
  buried in a streaming loop can only be tested with a daemon on the other end. It is asserted over
  every event type the protocol defines, in both directions: the signal resumes, and the three real
  terminals still end the follow. That second half is not decoration — the arms are adjacent, so a
  careless edit that widened the signal's arm would swallow a terminal and leave the client following a
  run that had already finished.
- **A signalled overrun still has a floor.** Retrying an overrun is bounded like any other resume, so a
  daemon disconnecting in a loop surfaces as an error rather than as a command that never returns. The
  one case that is not retried at all is an overrun that arrived **before any event did**: there is no
  position to resume from, and no number of retries reaches an event that never arrived, so the client
  says so rather than asking again.
- **A sequence gap ends the follow, and the client is where the contract puts that obligation.** "Clients
  ignore unknown additive event types but never ignore a sequence gap" is a rule about the *reader*, and
  the reference client satisfied it by not looking: it parsed `event_id`, `event` and the payload, and
  never read `sequence` at all — so a gap could not be noticed because the value was never read. That is
  the same failure the rule names, arrived at by omission rather than by decision, and it is the worst
  version of it: the client keeps printing across the gap and delivers two halves of an output that were
  never adjacent, as a complete answer.
  - **The gap is fatal rather than resumable, and the reason is that it is unrepairable.** The client
    cannot know which events it missed or whether they are still retained, and its one recovery
    mechanism — `Last-Event-ID` — re-reads from the position it already reached, which leaves the gap
    exactly where it was. So the honest response is to stop and say so, and the check therefore
    **outranks the retry decision**: a gap consulted through the retry path would be answered as a
    dropped connection and reconnected.
  - **Two boundaries keep the check from refusing healthy streams.** A resumed stream's first frame is
    exempt, because `Last-Event-ID` resumes *strictly after* the named event — requiring contiguity
    across a reconnect would refuse the client's own recovery path. And a frame with no readable
    `sequence` is not a gap: the contract bounds what a reader concludes from a frame, and inventing a
    gap from an absent field would refuse a stream the daemon is sending correctly.
  - **The rule is checked against a real socket, not only as a function.** The watcher is pure and
    unit-tested, but a correct function nothing calls is the exact shape of the original defect — so a
    stub listener on an ephemeral loopback port serves a stream with a gap in it, and the follow is
    asserted to fail *while the stream still carries a terminal event*. That last clause is load-bearing:
    the first version of the test ended the body after the gapped frames, and the mutation disabling the
    whole check **passed**, because a stream with no terminal fails the follow anyway. A gap-ignoring
    client has to be able to succeed for the assertion to be about the gap.

## Generated Contracts

- Generate OpenAPI from the owning Rust schema or one canonical source.
- Generate TypeScript clients/types; do not hand-maintain duplicates.
- Check generated output drift in CI.
- Preserve golden serialization fixtures for public events/errors.
- Classify breaking, additive, and behavioral changes.

## Runtime Protocol

The runtime protocol is versioned independently from `/api/v1`. It supports:

- initialize and capability/version negotiation;
- runtime instance registration/health;
- start/resume/cancel/interrupt input;
- ordered runtime events with IDs and acknowledgements where required;
- tool intents and scoped tool-access configuration;
- artifacts and checkpoint references;
- usage, warning, error, and terminal outcomes;
- graceful shutdown and protocol mismatch errors.

Local transport starts with stdio or loopback WebSocket/HTTP depending on the
runtime. Remote runtimes require TLS and service identity. Message sizes and
in-flight event windows are bounded.

## MCP Surface

Mount current Streamable HTTP under an authenticated route such as `/mcp` and
support local stdio as a separate launcher mode. MCP version and extensions are
negotiated. External MCP credentials map to a service client/workspace/export
policy before any list/call operation.

MCP errors preserve protocol requirements while internal details remain hidden.
The JARVIS tool ID/source fingerprint stays in private metadata or a stable safe
extension when negotiated.

## OpenAI-Compatible Edge

Expose only the subset needed by a researched consumer:

```text
POST /v1/responses
POST /v1/chat/completions
```

The Responses endpoint is preferred for new ElevenLabs integration. Chat
Completions is a compatibility fallback. This edge:

- authenticates the external voice/service client;
- resolves a signed opaque JARVIS session token;
- maps input/tools to a JARVIS run;
- streams exact consumer-compatible SSE events;
- translates JARVIS tool decisions to supported function/system-tool calls;
- does not expose the general JARVIS model gateway or arbitrary model proxy;
- has stricter latency, output, and session limits.

Conformance tests use captured current consumer requests and verify event names,
payloads, ordering, terminal events, `[DONE]` behavior where required, errors,
disconnect, and function calls.

## Webhooks

Use provider-specific routes and raw-body verification, for example:

```text
POST /webhooks/v1/elevenlabs/{connection_id}
POST /webhooks/v1/github/{connection_id}
```

The route ID selects verification configuration but is not authorization by
itself. Unknown/disabled connections, invalid signatures, stale timestamps, and
duplicate deliveries have provider-appropriate responses and safe audit events.

## Version Compatibility

CLI and daemon negotiate API version and feature capabilities. A patch-version
mismatch should normally work; unsupported major/minimum versions fail with an
update instruction. Desktop packages and daemon upgrades must account for
rolling restart and migration order.