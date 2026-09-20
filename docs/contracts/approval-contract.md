# Approval Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Purpose

Approval records prove that an authenticated principal authorized one exact
action or a narrowly defined standing rule. They are not free-form chat messages.

## Approval Request

```json
{
  "approval_id": "019...",
  "workspace_id": "019...",
  "requesting_principal_id": "019...",
  "run_id": "019...",
  "tool_call_id": "019...",
  "tool_id": "email.send@1",
  "action_fingerprint": "sha256:...",
  "risk": "high",
  "effects": ["external_communication", "write"],
  "summary": "Send one email to peter@example.com",
  "preview": {},
  "allowed_channels": ["cli", "desktop"],
  "expires_at": "2026-09-20T12:10:00Z",
  "state": "pending"
}
```

Preview is schema-defined, bounded, redacted, and sufficient for informed
consent. Hidden attachment/body/target changes are prohibited.

## Action Fingerprint

Compute over a versioned object containing:

```text
fingerprint format version
principal and workspace
tool canonical ID, source identity, schema fingerprint
normalized arguments
material content/artifact hashes
target connector account/resources
effects and constraints
logical idempotency key
```

Use a researched deterministic JSON canonicalization such as RFC 8785 and
SHA-256, encoded with an explicit algorithm prefix. The implementation must
round-trip test across Rust and any client that previews/verifies fingerprints.

## States

```text
PENDING
APPROVED
REJECTED
EXPIRED
CANCELLED
CONSUMED
INVALIDATED
```

Terminal decisions are immutable. A one-shot approval is consumed atomically
when the exact tool call is reserved. Reuse or changed fingerprint fails.

## Decision

```json
{
  "decision": "approve",
  "expected_version": 3,
  "action_fingerprint": "sha256:...",
  "comment": null
}
```

The server derives deciding principal, device/session, authentication assurance,
and time. A client cannot assert them in the body.

## Approval API

Authenticated product clients use:

```text
GET  /api/v1/approvals
GET  /api/v1/approvals/{approval_id}
POST /api/v1/approvals/{approval_id}/decide
POST /api/v1/approvals/{approval_id}/cancel
POST /api/v1/approval-grants/{grant_id}/revoke
```

### List and Detail

List filters are schema-defined: state, risk, effect, requesting run/tool, and
created/expiry time. Results use opaque workspace/principal/query-bound cursors,
stable ordering, and a bounded page size. The server exposes only approvals the
authenticated principal may inspect or decide. A foreign-workspace record is
indistinguishable from a missing record.

Detail returns the immutable action fingerprint inputs needed for informed
review as a bounded, schema-defined, redacted preview plus request/version/state,
requesting identity, tool source/schema identity, effects/risk, allowed channels,
expiry, prior decision metadata, and related run. It never returns hidden tool
arguments, credentials, or content excluded from the fingerprint.

### Decide

`decide` requires `Idempotency-Key` and the decision body above. Allowed values
are `approve` and `reject`. The server:

1. authenticates the current session/device/service client;
2. resolves workspace and decision authority server-side;
3. checks current channel and assurance against the approval request/policy;
4. expires stale requests using the authoritative clock;
5. compares `expected_version` and exact `action_fingerprint`;
6. persists the immutable decision and outbox/resume signal atomically;
7. returns the approval state, not a claim that the side effect completed.

Same-key/same-request retry returns the original decision. Same key with changed
decision, fingerprint, or comment is `idempotency.conflict`. Concurrent opposite
decisions allow one optimistic transition; the loser receives
`approval.version_conflict` and the safe current state.

An approved one-shot action is still revalidated at reservation/invocation and
consumed atomically with the exact tool-call reservation. Approval never grants
general tool access.

### Cancel and Revoke

The requesting principal or authorized policy/operator can cancel a pending or
approved-but-unconsumed request using `expected_version`, reason code, and
`Idempotency-Key`. Consumed/expired/rejected/cancelled requests are immutable;
idempotent repeats return current state.

Standing-grant revocation is a separate command bound to grant ID/version,
workspace, revoking principal/assurance, reason, and idempotency key. Revocation
commits before future policy checks and invalidates queued approvals/actions that
depend solely on that grant. It cannot erase historical audit.

### Expiry, Events, and Resume

Expiry is evaluated on every read/decision/reservation and by a durable expiry
worker. Exactly one state transition/outbox event wins. Approval activity events
are safe summaries; sensitive preview content remains referenced under policy.

Waiting runs/workflows resume from the durable decision event. A client
disconnect, duplicate event, or daemon restart cannot consume twice. Decision
responses may be `200`/`202` according to generated OpenAPI, but execution result
is queried through the related run/tool resource.

### Stable Errors

```text
approval.not_found
approval.scope_denied
approval.channel_not_allowed
approval.assurance_insufficient
approval.expired
approval.version_conflict
approval.fingerprint_mismatch
approval.already_consumed
approval.state_conflict
approval.grant_not_found
approval.grant_revoked
```

Errors use the common envelope and disclose no foreign approval, hidden target,
or sensitive preview. Authentication happens before lookup/body parsing where
the HTTP stack permits.

### CLI and Other Channels

The CLI is a thin API client with `approvals list`, `approvals show`,
`approvals approve`, `approvals reject`, `approvals cancel`, and grant-revoke
commands. Desktop/mobile render the same server preview/decision contract. Voice
uses the same endpoint/application command only after the voice call/session has
the policy-required assurance; transcribed channel text is never trusted
decision metadata.

## Standing Grants

"Always allow" creates a separate policy/grant proposal; it does not mutate a
one-shot approval. Standing grants define exact tool/source, resource/account,
argument constraints, effects/risk ceiling, channel/environment, expiry, budget,
and revocation. Critical actions may prohibit standing grants entirely.

## Voice Approval

Voice is permitted only when policy names it and the call has sufficient current
identity assurance. Critical actions default to step-up in CLI/desktop/mobile.
Transcribed "yes" alone is not universal approval.

## Invalidations

Pending approvals invalidate on:

- expiry, run/tool cancellation, or workspace/principal revocation;
- changed tool source/schema/effects;
- changed normalized arguments/material content;
- connector account or target resource change;
- policy version requiring reevaluation;
- runtime/plugin replacement;
- security kill switch.

## Audit and Privacy

Record request/decision/consumption identities, assurance, channel, policy,
fingerprint, safe preview hash/summary, timestamps, and outcome. Avoid storing
full sensitive content in the approval table; reference protected artifacts.

## Tests

- concurrent approve/reject;
- stale expected version;
- changed recipient/body/attachment/account;
- expiry and daemon restart;
- consume exactly once under duplicate call submission;
- wrong principal/workspace/channel/assurance;
- grant revocation and policy update;
- preview redaction and fingerprint cross-language vectors.
- list pagination/filter/cursor scope and foreign-workspace non-disclosure;
- API/CLI idempotency, optimistic decision race, cancel, grant revoke, and
  durable resume after disconnect/restart;
- server-derived channel/device/assurance and forged body metadata rejection;
- generated OpenAPI/client/golden fixture drift.