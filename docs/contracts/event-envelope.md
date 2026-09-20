# Event Envelope Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Envelope

```json
{
  "event_id": "019...",
  "event_type": "tool.call.completed",
  "schema_version": "1.0.0",
  "occurred_at": "2026-09-20T12:00:00Z",
  "recorded_at": "2026-09-20T12:00:00.010Z",
  "producer": {"name": "jarvis-tooling", "version": "0.1.0"},
  "scope": {
    "workspace_id": "019...",
    "principal_id": "019..."
  },
  "correlation_id": "019...",
  "causation_id": "019...",
  "aggregate": {
    "type": "tool_call",
    "id": "019...",
    "version": 5
  },
  "deduplication_key": null,
  "sensitivity": "confidential",
  "payload": {},
  "trace": {"traceparent": "..."}
}
```

## Semantics

- `event_id` identifies this immutable event.
- `event_type` is stable, namespaced, and past-tense.
- `occurred_at` is domain occurrence; `recorded_at` is durable ingestion time.
- `causation_id` points to the direct causing command/event when known.
- `correlation_id` groups a business/request flow.
- Aggregate version supports ordering/conflict detection for one entity.
- External delivery IDs belong in a scoped deduplication structure.

## Payloads

Each `(event_type, schema major)` has a generated JSON Schema. Payloads contain
IDs/references and necessary facts, not entire mutable domain objects. Sensitive
large content uses artifact references.

## Ordering

No global ordering is promised. Events for one aggregate are ordered by
aggregate version when emitted from canonical state. Delivery can duplicate or
arrive out of order; consumers use version and event ID.

## Delivery

The internal durable bus is at least once. Handler completion is recorded with
event ID, handler ID/version, attempt, outcome, and timestamps. Failed events
retry according to handler policy and can enter a dead letter queue.

Replay requires explicit authorization and records a new handling attempt, not a
new domain event. Re-emitting corrected facts uses a new event with causation.

## Compatibility

- Additive optional payload fields are minor-version compatible.
- Consumers ignore unknown fields but fail safely on unknown security-critical
  enum/effect values.
- Breaking payload meaning uses a new major schema version.
- Event type names are never repurposed.
- Consumer support ranges are observable before producer upgrades.

## External Events

Provider webhook events are not copied directly into this envelope. The connector
first verifies/deduplicates, then emits a normalized JARVIS event with provider
source reference and observed/received timestamps. Raw payload retention follows
policy.

## Required Tests

- serialize/deserialize golden fixtures;
- unknown additive fields;
- duplicate and out-of-order delivery;
- aggregate version gap/conflict;
- payload schema mismatch;
- dead letter and authorized replay;
- cross-workspace routing rejection;
- sensitive payload/artifact redaction.