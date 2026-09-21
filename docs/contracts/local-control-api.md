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
