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