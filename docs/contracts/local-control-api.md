# Local Control API Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT
Owner: Foundation and Protocol
Applies to: `FND-007`, `FND-008`, `BRN-004`, `BRN-005`, `BRN-007`

This contract defines the first local `jarvis` client to `jarvisd` boundary. It
is normative for Milestones 1 and 2 even while its compatibility lifecycle is
`DRAFT`. The generated OpenAPI document and serialization fixtures must match
this contract before either milestone exits.

## Scope

The v0.1 local control surface supports:

- daemon discovery, liveness, readiness, and authenticated status;
- creation, inspection, cancellation, and event streaming for native runs;
- deterministic API/version negotiation and safe error reporting;
- reconnect and replay for persisted public run events.

It does not define remote administration, browser authentication, MCP, external
runtime transport, provider-compatible endpoints, or general connector APIs.

## Local Transport and Discovery

`jarvisd` binds an operating-system-assigned TCP port on IPv4 loopback
`127.0.0.1` by default. It must not fall back to a wildcard or non-loopback
address. Additional transports or remote binds require a later accepted
contract and explicit configuration.

The daemon atomically publishes an owner-readable discovery file in the active
profile runtime directory after binding and before reporting readiness:

```json
{
  "schema_version": 1,
  "instance_id": "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09",
  "pid": 14240,
  "base_url": "http://127.0.0.1:43127",
  "api_major": 1,
  "started_at": "2026-09-20T12:34:56Z"
}
```

Rules:

- `base_url` must use `http`, numeric loopback host `127.0.0.1`, and the bound
  port. Clients reject userinfo, path, query, fragment, non-loopback hosts, and
  unsupported schemes in this file.
- The runtime directory and discovery file use owner-only permissions or the
  closest enforceable platform ACL. Failure to establish those permissions is
  fatal.
- The file contains no credential, secret reference, database path, or provider
  configuration.
- Publication uses write, flush, and atomic replace. Shutdown removes only a
  file whose `instance_id` still belongs to that daemon.
- Clients validate both process/instance state and authenticated status. A PID
  alone is never proof that the discovered process is JARVIS.
- A stale or malformed file produces an actionable typed diagnostic. Clients do
  not scan ports or silently connect to another profile.

## Local Client Authentication

Loopback is not an authentication boundary. Every `/api/v1` request requires an
opaque local client credential:

```http
Authorization: Bearer <opaque-client-secret>
Jarvis-API-Version: 1
Jarvis-Client-Version: 0.1.0
```

The initial owner credential is generated with at least 256 bits of operating-
system CSPRNG entropy during local profile onboarding. The plaintext credential
is stored in the OS credential store where available; any fallback must be
encrypted or protected by owner-only permissions and documented by platform.
The daemon stores a one-way verifier, client ID, creation time, scopes, and
revocation state, not a recoverable plaintext copy in ordinary configuration.

Milestone 1 supports offline same-owner enrollment performed by an explicit CLI
onboarding command. There is no unauthenticated HTTP enrollment endpoint. Later
device enrollment requires a separate accepted challenge/consent contract.

Authentication rules:

- Compare credential verifiers in constant time.
- Reject credentials from another profile, revoked clients, and unknown scopes.
- Never place credentials in URLs, discovery files, logs, errors, diagnostics,
  shell arguments, or process titles.
- Requests with an `Origin` header are rejected in v0.1. No CORS response headers
  are emitted. Native clients normally send no `Origin`.
- Validate `Host` against the active numeric loopback authority. Reject forwarded
  host/proto headers because no proxy is trusted in local mode.
- A request with **no** `Host`, or with **more than one**, is rejected. Binding is
  not an address filter: a loopback socket accepts a connection addressed to
  `localhost`, to another `127.0.0.0/8` address, or to a DNS name resolving to
  loopback, so the address is checked rather than inferred from the bind. Two
  `Host` headers are refused because a proxy and an origin can legitimately
  disagree about which is authoritative, and picking one by convention is the
  basis of request smuggling.
- Authentication failure returns the same safe response for unknown, malformed,
  and revoked credentials.

## Version Negotiation

Authenticated requests send `Jarvis-API-Version: 1`. The daemon returns:

```http
Jarvis-API-Version: 1
Jarvis-Server-Version: 0.1.0
Jarvis-Request-ID: 0195f4ed-624a-77c2-b3c9-f695867e9ec0
```

Missing or unsupported API versions fail with `api.version_unsupported` and a
safe supported-major list. Patch/minor product-version differences may work only
when API major and advertised capabilities are compatible. An older binary must
not modify config or data whose minimum writer version it does not support.

## Public Endpoints

```text
GET  /health/live
GET  /health/ready
GET  /api/v1/system/status
POST /api/v1/runs
GET  /api/v1/runs/{run_id}
GET  /api/v1/runs/{run_id}/events
POST /api/v1/runs/{run_id}/cancel
```

Unknown routes return the common error envelope. All `/api/v1` responses use
`application/json` except the event stream. Request bodies are bounded to 64 KiB
in v0.1, and the run input text is bounded to 32 KiB after UTF-8 decoding.
Implementations may lower a limit only through an advertised capability or
configured policy and must return a typed limit error.

### Liveness and Readiness

`GET /health/live` and `GET /health/ready` are unauthenticated so a local service
manager can probe them. They expose no version, paths, profile, dependency names,
or error detail and never enable CORS.

Liveness returns `200` while the process event loop can serve requests:

```json
{"status":"live"}
```

Readiness returns `200` only after configuration validation, migration checks,
repository initialization, recovery, and worker startup complete:

```json
{"status":"ready"}
```

During startup, drain, failed migration, or unavailable required storage it
returns `503` with `{"status":"not_ready"}`. Detailed causes are available only
through authenticated status/doctor paths.

### Authenticated Status

`GET /api/v1/system/status` returns bounded, non-secret operational state:

```json
{
  "instance_id": "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09",
  "server_version": "0.1.0",
  "api_major": 1,
  "state": "ready",
  "profile": "default",
  "storage": {"kind":"sqlite","status":"ready"},
  "capabilities": ["runs.create","runs.read","runs.cancel","runs.events"]
}
```

The response never includes filesystem paths, secret references, connection
strings, environment values, raw internal errors, or another client's data.

## Run Resources

IDs are lowercase canonical UUIDv7 strings. Timestamps are UTC RFC 3339 with a
`Z` suffix. Unknown JSON fields are rejected on commands.

Create a run:

```http
POST /api/v1/runs
Content-Type: application/json
Idempotency-Key: 0195f4f0-18dc-729b-bb34-07e8c7627f21
```

```json
{
  "conversation_id": null,
  "input": {"type":"text","text":"hello"},
  "runtime": "jarvis-native",
  "model_policy": {"policy_id":"scripted-test","version":1}
}
```

`Idempotency-Key` is required and scoped to authenticated principal, resolved
workspace/profile, client credential, operation, and API major. Its digest is
retained at least as long as the created run and reconciliation window. Reuse
with the same canonical request returns the original resource; reuse with
different input returns `idempotency.conflict`.

The server resolves and authorizes the referenced model policy in the active
workspace. A request may add typed stricter overrides in a later schema, but it
cannot supply provider evidence or weaken workspace policy.

Successful creation returns `202 Accepted`, a `Location` header, and:

```json
{
  "run_id": "0195f4f0-4c13-7bf4-89fb-f067adac13ee",
  "conversation_id": "0195f4f0-4c13-7bf4-89fb-f067adac13ef",
  "state": "received",
  "created_at": "2026-09-20T12:35:10Z",
  "links": {
    "self": "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee",
    "events": "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee/events"
  }
}
```

`GET /api/v1/runs/{run_id}` returns the authenticated client's accessible run,
including state, public result/error summary, timestamps, and monotonically
increasing resource `version`. It does not expose prompts assembled from hidden
policy, secret values, provider internals, or hidden chain-of-thought. A run from
another scope is indistinguishable from a missing run.

Initial states are `received`, `context_building`, `model_running`, and
`responding`. Terminal states are `completed`, `failed`, and `cancelled`.
Persist the transition before publishing an event that claims it occurred.

The error summary is the `error_code` field, and it is populated from the durable column rather
than derived at read time. Two rules make it trustworthy:

- **A failure carries its code on the transition.** `RunWrite` requires a `Failed` target to name
  its `TerminalOutcome`, and the controller builds that outcome from the same `ControllerError` it
  returns, so the stored code and the response cannot disagree about why the run failed. The code
  is a namespaced identifier (`run.no_model_served`) and the transition's `reason` is a separate
  operator label, so rewording a log label cannot change a client-visible code.
- **The column is the current outcome, not a history.** Every transition assigns it, so a run that
  failed and then succeeded stops reporting the earlier failure. A `completed` or `cancelled` run
  carries no `error_code` at all, and a cancellation is deliberately **not** a failure: the
  contract maps the three terminal states one-to-one, and `run.cancelled` is not a failure code.

A recovered run's code comes from the same `RecoveryAction` that builds its recovery event
payload, so the row and the event agree.

### Cancellation

`POST /api/v1/runs/{run_id}/cancel` requires an `Idempotency-Key` and accepts:

```json
{"reason":"user_requested"}
```

It records cancellation intent before signalling workers. Success returns `202`
with the current run state. Repeated cancellation is idempotent. If the run is
already terminal, return `200` and its unchanged terminal state. Cancellation
does not report `cancelled` until bounded cleanup reaches a durable terminal
transition; a cleanup timeout becomes a truthful typed failure or recovery state.

The `reason` is **bounded and required**: it is refused when empty, longer than `MAX_CANCEL_REASON_BYTES`
(512), or containing a NUL byte, with `request.invalid`. An omitted *body* is not an absent reason —
the endpoint defaults it to `user_requested`, because a cancel is a command a client sends and the
contract has always required the field.

**Idempotency is over the reason's content, not its presence.** The recorded digest folds the
reason's bytes, so two cancels of two runs whose reasons have the same *length* are two different
requests. A digest that folded only the length would report the second as a replay of the first,
and a replayed cancel is a no-op by design — so the second run would never be signalled while its
caller was told the cancel was accepted.

The reason travels to the terminal event: the scope a cancel signals carries it, because the scope
is what the controller already holds and a reason stored beside the signal could disagree with it.
The event payload for a cancellation is **not yet emitted**, and that is a named gap — the reason is
caller-supplied text, so putting it in a public event needs escaping, and hand-rolling that would be
the ad-hoc string manipulation the architecture forbids. A **failed** run's terminal event does carry
its code, because every value there comes from a closed set.

## Run Event Stream

`GET /api/v1/runs/{run_id}/events` requires
`Accept: text/event-stream`. Events use standard SSE framing:

```text
id: 0195f4f1-0475-7613-a92c-edf01183e909
event: run.output_text.delta
data: {"contract_version":"0.1.0","event_id":"0195f4f1-0475-7613-a92c-edf01183e909","run_id":"0195f4f0-4c13-7bf4-89fb-f067adac13ee","sequence":4,"occurred_at":"2026-09-20T12:35:11Z","payload":{"delta":"Hello"}}

```

Rules:

- `sequence` starts at 1 per run and increases by exactly one for each persisted
  public event. Event IDs are globally unique.
- The server persists an event before making it visible on the stream.
- Initial connection replays retained events from sequence 1, then follows live
  events. `Last-Event-ID` resumes strictly after that event.
- A missing, foreign, malformed, or no-longer-retained replay position returns
  HTTP `409` with `stream.replay_unavailable`; it never silently skips a gap.
- Keepalives are SSE comments and do not consume sequence numbers.
- Exactly one terminal event is persisted: `run.completed`, `run.failed`, or
  `run.cancelled`. The server closes after delivering the terminal event.
- A **failed** terminal event's payload carries `{"code":…,"retryable":false}`, so a client that
  only follows the stream learns why the run stopped. Its code is the same one the run's own row
  reports, so a streaming client and a polling client cannot be told different reasons. A
  `completed` event carries no payload, and a `cancelled` one does not yet — see *Cancellation*.
- A client disconnect never cancels a durable run. Cancellation uses the command
  endpoint.
- Per-client buffers are bounded. A slow consumer is disconnected; it can replay
  from its last delivered event while retention permits.
- Output deltas contain valid UTF-8 boundaries. Stored/public event payloads are
  bounded and never contain hidden chain-of-thought or secrets.

Minimum event types for the first slice are `run.received`,
`run.context_building`, `run.model_started`, `run.output_text.delta`,
`run.usage`, and the three terminal variants. Clients ignore unknown additive
event types but never ignore a sequence gap or unknown terminal state.

A **report is not a state change**, and `run.usage` is the case that makes the distinction concrete:
it is a public event that leaves the run exactly where it was. The store therefore has an append that
is not a transition, and the run's `version` is deliberately **not** advanced by it — a version is the
optimistic-concurrency token a *transition* states it expects, so bumping it for a report would make
two workers' transitions refuse each other for a write neither of them made.

The usage event carries `call_id` and the counters the provider **reported**, each omitted when it
reported none: the contract's own "unknown is not zero" applies to a durable statement, since writing
`0` for an unreported count would publish a measurement nobody made. A provider that reported nothing
produces **no** usage event at all, rather than an empty one a client switching on the type would
have to interpret.

## Common Error Envelope

Errors use an appropriate HTTP status and this shape:

```json
{
  "error": {
    "code": "request.invalid",
    "message": "The request is invalid.",
    "request_id": "0195f4ed-624a-77c2-b3c9-f695867e9ec0",
    "retryable": false,
    "details": {}
  }
}
```

`code`, status, and retryability are stable contract fields. `message` is safe
for users and may improve without a major version. `details` is schema-defined
per code and must not contain raw parser, SQL, provider, secret-store, or
filesystem errors.

Minimum codes:

| HTTP | Code | Retryable |
| --- | --- | --- |
| 400 | `request.invalid` | no |
| 400 | `api.host_not_allowed` | no |
| 401 | `auth.invalid` | no |
| 403 | `auth.scope_denied` | no |
| 403 | `api.origin_not_allowed` | no |
| 403 | `api.forwarded_header_not_allowed` | no |
| 404 | `resource.not_found` | no |
| 409 | `idempotency.conflict` | no |
| 409 | `stream.replay_unavailable` | no |
| 413 | `request.too_large` | no |
| 415 | `request.media_type_unsupported` | no |
| 422 | `request.semantic_invalid` | no |
| 426 | `api.version_unsupported` | no |
| 429 | `request.rate_limited` | yes, after declared delay |
| 503 | `service.not_ready` | yes |
| 500 | `internal.failure` | conditionally |

Malformed authentication must be rejected before body parsing or resource
lookup where the HTTP stack permits. Internal failures return a request ID and
generic message while preserving structured diagnostics in redacted local logs.

Every refusal on this surface uses this envelope, including the ones that are not
produced by a route handler:

- an **unknown route** returns `404` with `resource.not_found`. The HTTP framework's
  own fallback is a bare `404` with an empty body, which would give a client a
  status it can see and nothing it can parse;
- a **body over the limit** returns `413` with `request.too_large` as
  `application/json`. A limiter's own plain-text `413` would satisfy "the body is
  bounded" while breaking both this rule and the JSON requirement above;
- a **`Host` refusal** returns `400` with `api.host_not_allowed`, a **browser
  `Origin`** or a **forwarding header** returns `403`, and all three are JSON.

The refusal names neither the rejected value nor the expected one where the
rejected value is caller-supplied, because echoing it would reflect untrusted text.

Layer order is part of this contract where it changes which error a caller sees.
The request-body limit is applied **outside** version negotiation and
authentication — that is, it is checked first — so an oversized body is reported as
`request.too_large` regardless of the credentials the caller presented. Ordering it
after authentication would make the same oversized request report
`api.version_unsupported` or `auth.credential_rejected` instead, which is true but
answers the wrong question.

## Crash and Recovery Semantics

- Acknowledged mutation state and idempotency records are committed atomically.
- Run state is persisted before its corresponding public event is committed in
  the same transaction or an equivalent transactional outbox operation.
- On restart, terminal runs remain terminal. Nonterminal runs are recovered to
  an explicit resumable or failed state; they are never inferred complete from
  partial text.
- Readiness stays false until migrations, integrity preconditions, and recovery
  classification complete.
- The daemon drains new mutations before shutdown while bounded in-flight work
  reaches a persisted state.

## Required Contract Tests

Before `FND-007`, `FND-008`, or `BRN-007` is complete, test:

1. discovery-file atomicity, permissions, stale PID/instance, and hostile URL;
2. missing, malformed, wrong-profile, and revoked authentication;
3. hostile `Origin`, `Host`, and forwarding headers;
4. API major mismatch and safe version reporting;
5. body/input limits, unknown fields, invalid UTF-8, and safe error redaction;
6. create-run idempotency success and conflict;
7. ordered replay, reconnect, sequence gap, slow consumer, and one terminal event;
8. disconnect without cancellation and cancellation races;
9. abrupt daemon termination before and after every durable transition;
10. denial of cross-client/profile run lookup;
11. liveness/readiness behavior during startup, migration failure, drain, and
    storage failure;
12. golden JSON/SSE fixtures and generated OpenAPI drift.

These tests provide evidence for `ACC-002`, `ACC-003`, `ACC-010`, and `ACC-012`.

### Implemented evidence (Milestone 1)

Test 2 (authentication) and test 3 (`Origin`, `Host`, and forwarding headers) are
implemented in `jarvis-infrastructure`:

- the `http` module's own tests cover the indistinguishable authentication
  response, the version-negotiation failure, and the rejections below;
- `tests/daemon_serving.rs` repeats the `Host` rejections over a **real socket**,
  where a real client chooses the header and a proxy would rewrite it. A router
  test alone would not prove the control holds on the wire, because a loopback
  socket accepts a connection addressed to any name that resolves to loopback.

The `Host` check compares against the authority the daemon **actually bound**,
carried in `ApiState::bound_authority` rather than hardcoded, because the port is
ephemeral and operating-system assigned. A constant would reject the daemon's own
address, and accepting any loopback host would admit `localhost`, a different
`127.0.0.0/8` address, or a foreign port. IPv6 authorities are compared in the
bracketed form a client sends in `Host`.

Refusals name neither the expected nor the received authority: the expected value
is the daemon's own address and the received value is attacker-supplied text, so
echoing either would reflect untrusted input back to the caller.

Test 5 (body and input limits) is implemented as well, and it found a defect in the
process: the `413` was correct but its body was the limiter's own plain text rather
than the envelope, so the status was right and the contract was still broken.

### Implemented evidence (Milestone 2, run resources)

The run resource surface exists: `POST /api/v1/runs`, `GET /api/v1/runs/{run_id}`,
`POST /api/v1/runs/{run_id}/cancel`, and `GET /api/v1/runs/{run_id}/events` are
served by `jarvis_infrastructure::http::runs` over
`jarvis_application::run_service`, and `jarvis-protocol::run` carries the wire types
so the daemon and the client share one serialization definition. `jarvis ask` and
`jarvis runs show|events|cancel` are the client half, and the path was exercised
against a real daemon on a clean profile: a question produced an answer and exit `0`.

Four rules this contract states are now enforced by construction rather than by
intention:

- **The client-visible state set is the contract's, not the controller's.**
  `wire_state` is a total, deliberately coarser projection of the twelve domain
  states onto the seven this contract exposes, and it lives at the API boundary
  because `agent-runtime.md` forbids domain state names doubling as wire strings. The
  three terminal states map one-to-one, so a client never sees a finished run
  described by a non-terminal state.
- **Authentication is a property of the handler.** `AuthenticatedClient` is a
  `FromRequestParts` extractor, so a route that lacks a credential cannot run and
  cannot be reached by forgetting a middleware call. All four run routes are asserted
  to refuse an unauthenticated caller.
- **Scope is resolved server-side** from the authenticated client. A caller cannot
  address another workspace's run by naming it, and a foreign run is
  indistinguishable from a missing one — including a malformed identifier, which
  answers `404 resource.not_found` rather than something that would reveal which
  identifiers exist.
- **Idempotency commits with the mutation.** `create_run_idempotent` writes the run,
  its opening `run.received` event, and the key record in **one** transaction, because
  this contract requires "acknowledged mutation state and idempotency records are
  committed atomically". A separate claim-then-create leaves exactly the window that
  sentence closes, and the first implementation had it: a replayed create reported a
  conflict instead of returning the original run. A reused key with different input is
  now `409 idempotency.conflict`, and a repeated key with identical input returns the
  original run.

Three further facts are worth recording because each was a defect first:

- **`Received -> Cancelled` is a legal edge.** A run cancelled before its first step
  previously had no terminal exit, so a client polling it waited forever for an answer
  that had been abandoned. This contract's cancellation section requires a durable
  terminal transition, and the state diagram was missing the edge that allows it.
- **A run's opening event is created with the run.** A client that connects before the
  run does any work must still be able to replay `run.received` at sequence 1, so the
  event is written in the same transaction as the row rather than appended afterwards.
- **An unavailable replay position is `409`, never a silent restart.** `Last-Event-ID`
  is resolved against the retained events, so a position this daemon does not have
  produces `stream.replay_unavailable` instead of delivering a gap as if it were
  complete.

The `runtime` a create request names is now **recorded** on the run, into
`agent_runs.runtime_id`/`runtime_version`, and read back by the same port. The field was
required and the unsupported-runtime refusal was real, but the validated value was then
discarded and both columns stayed `NULL` on every row — a check that reads as coverage
while the durable record it exists for was absent, and one that left
`agent-runtime.md`'s resume step ("validate runtime identity/version") with nothing to
validate against. The recorded version is this build's own, not the caller's: a client
cannot claim a runtime version it is not running.

**Not exposed on the wire, deliberately.** `RunView` (and therefore the create response
above) carries no `runtime` field. The run resource's documented shape is a closed set
and `Unknown JSON fields are rejected on commands`, so adding one is a contract change
with a versioning decision attached, not a free addition — and a client currently has no
use for the value, since it cannot request a runtime other than the native one. The
consumer that does need it is a resume, which reads the row server-side. Recorded here
so the next person does not read "the runtime is stored" as "a client can see it".

**Not implemented, and deliberately not claimed:** the event stream delivers the
retained public events and closes rather than following the run live. A client
therefore reconnects with `Last-Event-ID` until it receives a terminal event, which is
what `jarvis ask` does. Holding the connection open needs a streaming response body,
and this crate has neither a stream crate nor `axum`'s `sse` feature in its reviewed
dependency set; adding either is a dependency change the research gate requires
evidence for, and hand-writing a `Stream` would be an unreviewed async state machine
on the security-relevant path. Also outstanding: contract test 12's golden JSON/SSE
fixtures and generated OpenAPI drift, `Idempotency-Key` scoping per principal and
credential rather than per client, and contract test 8's disconnect case.

### Implemented evidence (Milestone 2, restart recovery)

Contract test 9's **abrupt-termination** half is implemented. The classification rule
lives in the domain (`jarvis_domain::run::recovery`) and the pass lives in the
application layer (`jarvis_application::recovery::reconcile`), so the decision about an
interrupted run has exactly one definition and the client can ask the same question
without opening the database.

- **The read is deliberately unscoped.** `RunRepository::incomplete_runs` returns every
  workspace's non-terminal runs with each run's workspace attached, because recovery is
a startup concern of the whole profile. Scoping it to one workspace would leave every
  other workspace's interrupted runs non-terminal with no symptom.
- **The predicate is the stored state**, not `completed_at`. A run whose completion
  instant was somehow absent is still found, and a state the domain cannot interpret is
  reported as an error rather than skipped — skipping would leave the run non-terminal
  with nobody aware, which is the failure the read exists to prevent.
- **Nothing is resumed.** Both classifications settle the run at `Failed`. Resuming
  means re-running a model call, and nothing knows what the interrupted call produced;
  the contract's "never inferred complete from partial text" rule and the
  cannot-resume case are the same case. A parked run is still *classified* separately
  (`was_resumable`, and `parked_in` in the payload) so the distinction survives for an
  operator even though the outcome does not differ yet.
- **The version read is the version written.** The transition carries the version from
  the read, so a run that completed between the read and the write is refused
  (`storage.transition_refused`) rather than having a real outcome replaced by a
  failure. This is what makes the pass safe to run against a live database.
- **Each run is written independently.** One failure is recorded in the report and the
  pass continues, so a single unsettleable run cannot leave the rest non-terminal. The
  report distinguishes a completed pass from a partial one, so a caller cannot read
  "nothing to do" out of a pass that could not read.
- **`TransitionActor::Supervisor`** records that the daemon ended the run, not the run.
  Recording it as the controller would attribute a decision to the run that the run
  never made.

The startup order is asserted, not merely intended: `start` migrates, then reconciles,
**then** publishes discovery and only then marks readiness, which is what this
contract's "readiness stays false until … recovery classification completes"
sentence requires. `tests/daemon_serving.rs` proves it end-to-end on a durable
database file across three real daemon starts — the first leaves a run mid-flight, the
second must settle it and report the count, and the third must find nothing because the
second already closed it. That test was confirmed to **fail** when the pass is made to
find nothing, so it falsifies the feature rather than describing it.

**Not claimed:** the run is not actually resumed or re-driven, and no dependency a
parked run was waiting on is re-evaluated. Nothing performs recovery of a run whose
process died mid-model-call beyond settling it as failed.
