# Common Contract Conventions

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Encoding

- UTF-8 JSON for HTTP, WebSocket, JSON-RPC, events, and persisted envelopes.
- JSON field names use `snake_case` in JARVIS-owned contracts.
- JSON Schema dialect is 2020-12 unless a constrained external protocol requires
  another dialect at its adapter.
- Binary/large content uses artifact references, not unbounded base64 in normal
  messages.
- Numbers that can exceed interoperable JSON integer precision are decimal
  strings or bounded explicitly.

## IDs

Canonical IDs are typed strings. The intended generator is UUIDv7, pending
dependency verification. Consumers treat IDs as opaque and case-sensitive.

External IDs include provider/account scope:

```json
{
  "provider": "github",
  "account_id": "01...",
  "external_id": "123456"
}
```

An external ID alone is never globally unique or trusted authorization context.

## Time

- Absolute timestamps are RFC 3339 UTC strings with `Z`.
- Durations and deadlines use explicit names such as `timeout_ms`.
- Recurrence stores timezone/IANA zone plus schedule, not only a UTC offset.
- Expiry comparisons use an injected authoritative application clock.

## Versions

Contracts use semantic version strings. Handshakes offer supported ranges or
ordered exact versions. Persisted records include a schema version when their
payload can evolve independently from the table.

Version selection is explicit; clients do not infer capability from application
version alone.

## Enums and Unknown Values

Servers reject unknown command enum values unless the contract explicitly
allows extension strings. Read-side clients preserve or safely render unknown
future values instead of mapping them to a misleading default.

Rust adapters should prefer an `Unknown(String)` representation at evolving
provider boundaries. Stable domain state admits only understood values.

## Request Context

Trusted context is server-derived and not accepted as ordinary body fields:

```text
request_id
correlation_id
trace context
principal_id and assurance
device/service client/session
workspace_id
deadline/cancellation
policy/grant snapshot
channel/origin
```

Public APIs may accept a requested workspace selector, but the server resolves
and authorizes it before constructing trusted context.

`request_id` is minted server-side, once per authenticated request, by the
authentication middleware. It is recorded on the request extensions, returned to
the client in the `jarvis-request-id` response header, and used as the
`RequestContext`'s own identifier, so an error envelope, the response header,
and the daemon's structured diagnostics all name the same request. A
caller-supplied `jarvis-request-id` request header is ignored rather than
echoed: a correlation identifier a caller can choose is one a caller can use to
aim an operator's search, or to collide with another request's.

## Error Envelope

```json
{
  "error": {
    "code": "tool.permission_denied",
    "message": "This capability is not allowed in the active workspace.",
    "request_id": "019...",
    "retryable": false,
    "retry_after_ms": null,
    "details": {}
  }
}
```

Requirements:

- `code` is stable and namespaced.
- `message` is safe for the requesting principal.
- `request_id` is populated on every refusal a handler can produce, not only on
  internal failures, and matches the `jarvis-request-id` response header. The
  example above is a permission denial for that reason: the field is what lets a
  client quote one request, and a refusal is the case a client most often needs
  to report.
- `details` is schema-defined per code and contains no secret/internal stack.
- Retryability describes this operation, not the error class in every context.
- Internal/source error and provider IDs are stored in protected diagnostics.

## Idempotency

An idempotency key is scoped by authenticated principal, resolved workspace,
client credential when multiple clients can act as that principal, operation,
and API major. The server stores a key digest, request fingerprint, and terminal
or ambiguous result. Reusing a key with different material input is a conflict.

Idempotency expiration is longer than the maximum provider retry/reconciliation
window for the operation. It does not imply an external provider performed the
effect exactly once; ambiguous outcomes are explicit.

## Pagination

Opaque cursor response:

```json
{
  "items": [],
  "next_cursor": null,
  "has_more": false
}
```

Cursors bind workspace, filter, order, page size, and backend snapshot semantics.
They are signed/encrypted or stored server-side so callers cannot widen scope.

## Content and Artifacts

Content blocks can include text, JSON, artifact reference, image/audio reference,
or redacted/omitted markers. Every block has media type and optional sensitivity
and provenance metadata. Content is bounded by contract and storage policy.

Artifact references are opaque JARVIS IDs. External URLs are adapter metadata and
never returned as durable credentials.

## Sensitive Fields

Generated schemas should mark sensitive fields using a JARVIS extension such as
`x-jarvis-sensitive: true`. Logging, tracing, diagnostics, and debug rendering
consume this metadata in addition to credential-pattern redaction.

## Compatibility Behavior

- Readers ignore unknown additive object fields unless strict validation is a
  security requirement for commands.
- Writers do not emit fields unavailable in the negotiated version.
- Unknown event types are retained/acknowledged according to stream contract and
  surfaced as compatibility warnings.
- Security-critical unknowns fail closed.
- Every boundary has golden old/new fixtures and downgrade tests.