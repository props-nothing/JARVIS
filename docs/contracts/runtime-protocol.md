# JARVIS Runtime Protocol

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Purpose

This protocol lets out-of-process agent runtimes start, resume, stream, accept
input, and cancel work without becoming part of JARVIS's trusted core.

## Transport

Initial transports:

- newline-delimited JSON-RPC 2.0 over child stdin/stdout;
- JSON-RPC 2.0 messages over authenticated WebSocket for remote runtimes.

Stdio stdout carries protocol messages only. Logs use stderr. Messages have a
configured maximum size and the process has bounded output capture.

## Initialize

JARVIS initiates:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "initialize",
  "params": {
    "protocol_versions": ["0.1.0"],
    "host": {"name": "jarvisd", "version": "0.1.0"},
    "capabilities": {
      "runtime_ack": true,
      "artifacts": true,
      "scoped_mcp": true
    },
    "limits": {
      "max_message_bytes": 1048576,
      "max_in_flight_events": 64
    }
  }
}
```

Runtime response:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "result": {
    "protocol_version": "0.1.0",
    "runtime": {
      "id": "example.runtime",
      "name": "Example Runtime",
      "version": "1.2.3"
    },
    "capabilities": {
      "resume": true,
      "interrupt_input": true,
      "tool_intents": true,
      "artifacts": true,
      "modalities": ["text"]
    }
  }
}
```

No start method is valid before successful initialization. Unsupported versions
return a typed incompatibility error and the process is not given run secrets or
tool access.

## Start

Method: `runtime/start`

```json
{
  "run_id": "019...",
  "attempt_id": "019...",
  "objective": {"type": "text", "text": "..."},
  "context_manifest": {"id": "019...", "items": []},
  "runtime_config": {},
  "capabilities": {
    "mcp_endpoint": "http://127.0.0.1:.../mcp/runtime/...",
    "mcp_token_ref": "one-time-delivery-handle"
  },
  "limits": {
    "deadline": "2026-09-20T12:00:00Z",
    "max_tool_calls": 20,
    "max_artifact_bytes": 10485760
  }
}
```

The actual secret/token delivery mechanism must not require logging or command
arguments and is finalized with the supervisor implementation.

Response acknowledges acceptance and returns runtime-owned run/checkpoint
identity. Acceptance is not completion.

## Runtime Events

Notification method: `runtime/event`

```json
{
  "jsonrpc": "2.0",
  "method": "runtime/event",
  "params": {
    "run_id": "019...",
    "runtime_run_id": "opaque-runtime-id",
    "event_id": "019...",
    "sequence": 7,
    "type": "tool.intent",
    "occurred_at": "2026-09-20T12:00:00Z",
    "payload": {
      "call_id": "runtime-call-4",
      "tool_name": "email.search",
      "arguments": {"query": "from:Peter"}
    }
  }
}
```

Required event classes:

```text
runtime.started
activity.status
model.usage
output.text.delta
tool.intent
checkpoint.created
artifact.offered
input.required
runtime.warning
runtime.completed
runtime.failed
runtime.cancelled
```

One runtime run emits monotonically increasing sequence numbers and exactly one
terminal event. Event IDs are stable across replay. JARVIS persists accepted
events before acknowledging `through_sequence` with `runtime/ack` when negotiated.

Gaps, duplicate sequence with different event ID/content, events after terminal,
and wrong run identity are protocol violations.

## Tool Intent Result/Input

JARVIS executes tool intents through canonical policy. It responds through
`runtime/input` with the runtime `call_id` and one of:

- normalized successful result;
- approval pending plus resumable wait reference;
- rejected/denied result;
- timeout/cancel/error result.

A runtime cannot mark its own tool intent executed. Direct scoped MCP mode still
records calls through JARVIS and may return a call reference for reconciliation.

## Resume

Method: `runtime/resume`

Includes JARVIS run/attempt, runtime run/checkpoint ID, last acknowledged
sequence, reconciled tool call outcomes, new user/approval input, refreshed
limits, and fresh scoped capabilities.

The runtime may replay unacknowledged events with identical IDs/content. JARVIS
deduplicates them. A checkpoint from a different runtime/version/workspace/run is
rejected.

## Cancel and Shutdown

- `runtime/cancel` requests cancellation with reason/deadline.
- Runtime emits `runtime.cancelled` after cleanup when possible.
- Supervisor escalates from cancellation to process termination after deadline.
- `shutdown` stops accepting starts and drains bounded current work.
- Process exit before terminal becomes typed runtime failure or resumable
  suspension according to checkpoint capability.

## Artifacts

Runtime offers metadata and a transport-specific temporary handle. JARVIS checks
declared size/media/hash, imports under workspace policy, computes its own hash,
and returns a canonical artifact ID. Runtime paths/URLs are never accepted as
durable user-facing artifact IDs.

## Errors

Stable classes include:

```text
protocol.incompatible
protocol.violation
runtime.unavailable
runtime.start_timeout
runtime.crashed
runtime.quarantined
runtime.limit_exceeded
runtime.checkpoint_invalid
runtime.cancelled
runtime.internal
```

Runtime error text is untrusted, bounded, and redacted before display/logging.

## Conformance

A reference fixture runtime must test initialize/version mismatch, start, event
ordering/replay, ack/backpressure, tool intent, approval wait/resume, checkpoint,
artifact import, cancel, crash, hang, malformed/oversized message, duplicate
terminal, stderr flooding, and graceful shutdown.