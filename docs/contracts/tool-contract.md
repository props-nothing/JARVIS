# Canonical Tool Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Definition

```json
{
  "id": "email.send@1",
  "namespace": "email",
  "name": "send",
  "version": "1.0.0",
  "description": "Send an email using an authorized connector account.",
  "input_schema": {"$schema": "https://json-schema.org/draft/2020-12/schema"},
  "output_schema": {"$schema": "https://json-schema.org/draft/2020-12/schema"},
  "effects": ["external_communication", "write"],
  "risk": "high",
  "required_scopes": ["email.send"],
  "default_approval": "ask",
  "idempotency": "caller_keyed",
  "timeout_ms": 30000,
  "source": {
    "kind": "connector",
    "id": "google.gmail",
    "version": "1.0.0",
    "schema_fingerprint": "sha256:..."
  }
}
```

Tool descriptions are untrusted model guidance. Effects, risk, scopes, source,
and approval defaults come from trusted reviewed configuration.

## Effects

Base effects:

```text
read_only
write
external_communication
destructive
code_execution
financial
privileged
physical
```

A tool can have multiple effects. Unknown effects fail closed. Risk is `low`,
`moderate`, `high`, or `critical`; policy derives final decision from more than
this label.

## Call Intent

```json
{
  "call_id": "019...",
  "run_id": "019...",
  "tool_id": "email.send@1",
  "arguments": {},
  "idempotency_key": "019...",
  "reason_summary": "Send the follow-up the user reviewed.",
  "source_context_refs": []
}
```

Principal/workspace/grants are trusted context, not accepted from model
arguments. `reason_summary` is display/audit context and never authorization.

## Call States

```text
REQUESTED
VALIDATED
DENIED
WAITING_APPROVAL
APPROVED
RESERVED
EXECUTING
RECONCILING
SUCCEEDED
FAILED
CANCELLED
EXPIRED
```

Transitions persist expected prior version. `EXECUTING` is recorded before an
external effect. An unknown outcome uses `RECONCILING`, not automatic retry.

## Execution Grant

The executor receives a short-lived grant bound to:

- call/tool/source/schema identity;
- principal/workspace/account/resource scope;
- normalized argument fingerprint;
- allowed effects and constraints;
- deadline and one attempt/logical operation;
- approval ID/policy decision where applicable.

The grant is not reusable with changed arguments or another adapter instance.

## Result

```json
{
  "call_id": "019...",
  "status": "succeeded",
  "content": [
    {"type": "json", "value": {}}
  ],
  "artifacts": [],
  "provider_reference": "safe-opaque-id",
  "effect_summary": "Email sent to one recipient.",
  "retryability": "not_retryable",
  "completed_at": "2026-09-20T12:00:00Z"
}
```

Results are validated, size-bounded, sensitivity-labelled, and treated as
untrusted when returned to a model or UI.

## Error Classes

```text
tool.not_found
tool.unavailable
tool.schema_invalid
tool.permission_denied
tool.approval_required
tool.approval_rejected
tool.approval_expired
tool.conflict
tool.rate_limited
tool.timeout
tool.cancelled
tool.provider_auth
tool.provider_error
tool.output_invalid
tool.outcome_ambiguous
tool.limit_exceeded
```

Provider errors map to these while protected diagnostics retain a bounded safe
source reference.

## MCP Mapping

MCP tool names/descriptions/schemas map to canonical definitions with source
identity. MCP annotations cannot lower effects/risk. Structured content is
validated if declared; unknown content blocks are bounded and preserved safely
or marked unsupported.

MCP list results affect discoverability only. Calls still resolve current source
and policy by canonical ID/fingerprint.

## Compatibility

- Breaking schema/effect/source changes require a new major tool ID.
- Additive optional output fields can be compatible.
- Tightening policy does not require a new tool ID but invalidates relevant
  standing grants/approvals.
- Approvals always bind the actual schema fingerprint and normalized arguments.