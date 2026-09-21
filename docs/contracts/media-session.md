# Provider-Neutral Realtime Media Session Contract

Status: PROPOSED
Lifecycle: DRAFT
Contract version: 0.1.0
Owner: Media Platform

**Not normative until [ADR-0011](../adr/0011-realtime-media-session-boundary.md) is
`ACCEPTED`.** It is published now so the boundary is reviewable before any
transport is adopted, and so no adapter can define it by accident.

## Purpose

This contract owns canonical media session state independently of any realtime
transport, room implementation, or SIP leg. Transport events, participant
attributes, track publications, and data messages are untrusted observations. The
JARVIS media session controller validates, authorizes, deduplicates, orders, and
reconciles them before canonical state or workflow events change.

The voice call contract ([voice-call.md](voice-call.md)) remains the authority for
a telephone call. A bridged call is a participant inside a media session; it does
not gain a second lifecycle.

## Session Record

```json
{
  "session_id": "019...",
  "workspace_id": "019...",
  "purpose": "interactive",
  "state": "active",
  "version": 7,
  "transport_binding": {
    "provider_id": "media.example",
    "connection_id": "019...",
    "provider_session_ref": "opaque"
  },
  "identity": {
    "acting_principal_id": "019...",
    "assurance": "device_bound"
  },
  "grants": [
    {
      "participant_ref": "019...",
      "publish": ["microphone"],
      "subscribe": ["audio", "video", "screen"],
      "expires_at": "2026-09-21T12:30:00Z"
    }
  ],
  "capture": {
    "microphone": "active",
    "camera": "off",
    "screen": "off",
    "screen_audio": "unsupported"
  },
  "recording": "disabled",
  "related_run_id": null,
  "consent_policy_ref": "policy:...@3",
  "idempotency_key_ref": "digest:...",
  "created_at": "2026-09-21T12:00:00Z",
  "ended_at": null,
  "outcome": null
}
```

`workspace_id` and `acting_principal_id` resolve server-side from a JARVIS
credential. A transport-issued token, participant identity, participant
attribute, display name, or room metadata field is **never** an input to that
resolution.

## Lifecycle States

```text
CREATED
ADMISSION_PENDING
ADMITTED
SIGNALLING
MEDIA_NEGOTIATING
ACTIVE
DEGRADED
SUSPENDED
ENDING
COMPLETED
FAILED
CANCELLED
```

`COMPLETED`, `FAILED`, and `CANCELLED` are terminal.

Initial transition rules:

- `ADMISSION_PENDING -> ADMITTED` only after the JARVIS credential validates and
  grants are issued; a rejected admission terminates without creating a session;
- `ADMITTED -> SIGNALLING -> MEDIA_NEGOTIATING` are distinct, because a successful
  control/signalling phase does **not** imply that media can flow;
- `MEDIA_NEGOTIATING -> ACTIVE` only when the media phase is established. A
  participant that signalled but whose media phase failed is not `ACTIVE`;
- `ACTIVE <-> DEGRADED` records transport-reported network quality; degradation is
  an observation, not an inferred timeout;
- `ACTIVE <-> SUSPENDED` covers reconnection where the adapter supports it; where
  it does not, a disconnect proceeds toward a terminal state with a recorded
  reason;
- `ENDING -> COMPLETED` requires every track grant to be released and every
  recording/export side effect to reach a recorded outcome.

Provider callbacks and transport events can arrive late, duplicated, or out of
order. Monotonic transition rules and provider status reconciliation apply
exactly as they do for calls.

## Participant and Track Records

A participant record carries a scoped external reference, its role, its joined and
left observations, and its current connection quality. A track record carries the
kind (`audio`, `video`, `screen`, `screen_audio`, `data`), its role, the
publication state, and the grant that authorized it.

Rules:

- `participant_ref` is a JARVIS-scoped reference. It is not a transport identity,
  a display name, or an account identifier.
- A track's existence is not authorization. Authorization is the grant recorded
  alongside it, re-evaluated at publication.
- A transport-reported mute, subscription restriction, or permission flag is
  defense in depth and is recorded as an observation. No JARVIS decision may
  assume the transport enforced it.
- An agent participant is represented identically to a human participant and holds
  a scoped grant rather than ambient authority.

## Data Plane

Data surfaces are normalized into bounded, typed records. They are policy input.

- **Guaranteed, message-based** (text streams, byte streams, request/response
  calls): chunked and topic- or method-routed, with concurrent streams handled in
  the order the sender opened them. All are size bounded by JARVIS policy in
  addition to any transport limit, and all carry the sender's scoped reference.
- **Lossy, continuous** (data tracks): frame delivery is best-effort. A dropped
  frame is neither an error nor a reconciliation gap.
- **State synchronization** (participant attributes, metadata): provider-writable
  and broadcast. Never an authorization input, workspace selector, or secret
  store.

Request/response calls require every one of the following:

1. a fixed, JARVIS-defined method set, not extensible by a peer;
2. schema-validated arguments at the receiving boundary;
3. the same policy, approval, and audit path as any other proposed effect;
4. a bounded timeout and a bounded payload, enforced by JARVIS rather than
   inherited from the transport;
5. an error surface that returns a typed failure and never returns authority,
   credentials, or another participant's data to the caller.

## Capture

| Capture | Default | Requirement |
| --- | --- | --- |
| Microphone | off | Explicit per-session opt-in; visible while active |
| Camera | off | Explicit per-session opt-in; visible while active |
| Screen | off | Explicit per-session opt-in; visible while active |
| Screen or tab audio | off | Explicit opt-in; unavailable states reported, never implied |

Revocation stops publication immediately, is recorded on the session, and takes
effect without ending the session. A capture grant does not imply a perception
grant, and a perception grant does not imply authorization to act.

## Recording, Ingress, and Egress

Recording a session, exporting it, and ingesting an external source into it are
separate side effects, each with its own grant:

- disabled by default; enabled only by explicit policy for a named session;
- a recording records its own consent basis, retention class, and terminal
  outcome;
- external ingestion resolves its target session from configuration, never from a
  caller-supplied identifier;
- an unbounded or orphaned recording is a defect, and a session cannot reach
  `COMPLETED` while one is unresolved.

## Webhook and Event Ingress

- Verify the transport's signature over the **raw request bytes**, using the
  transport's current documented mechanism, before parsing.
- Persist the delivery identifier and session mapping before acknowledging.
- Delivery is not guaranteed and retries occur, so **deduplicate by delivery
  identifier and treat ordering as advisory**. Some transports sequence delivery
  so that a newer event follows an older one being delivered or abandoned; that is
  a helpful property, not a guarantee to rely on for correctness.
- Treat every payload field as untrusted. Never let a payload-supplied
  workspace/user identifier select tenant data.
- Reconcile against current session state; a payload is a claim about what
  happened, not proof that canonical state should become it.

## Persistence and Reconciliation

- Persist state before publishing an event that claims the transition happened.
- Every state change carries an expected prior version and an actor.
- Media payloads, frames, and transcripts are retained only under an explicit
  retention policy; the default is not to retain them.
- Terminal state requires every grant released, every recording resolved, and
  every external side effect to have a recorded outcome.

## Verification

- A transport-issued participant attribute cannot alter workspace or grant.
- A track published without a matching grant is refused and recorded.
- A request/response call to an unregistered method fails with a typed error and
  no side effect.
- A duplicated webhook delivery produces one state change.
- An out-of-order or late event cannot move the session backwards.
- A signalled-but-media-failed participant never appears as `ACTIVE`.
- Removing the transport adapter leaves canonical session records readable through
  the CLI.
