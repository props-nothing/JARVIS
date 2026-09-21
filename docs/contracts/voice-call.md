# Provider-Neutral Voice Call Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT
Owner: Voice Platform

## Purpose

This contract owns canonical call state independently from ElevenLabs, SIP,
local audio, or any future provider. Provider callbacks and realtime frames are
untrusted observations. The JARVIS call controller validates, deduplicates,
orders, and reconciles them before canonical state or workflow events change.

## Call Record

```json
{
  "call_id": "019...",
  "workspace_id": "019...",
  "direction": "outbound",
  "state": "ringing",
  "version": 5,
  "provider_binding": {
    "provider_id": "voice.example",
    "connection_id": "019...",
    "provider_call_id": "opaque"
  },
  "identity": {
    "acting_principal_id": null,
    "assurance": "guest"
  },
  "reason_ref": "artifact:...",
  "related_run_id": null,
  "related_workflow_id": "019...",
  "consent_policy_ref": "policy:...@3",
  "idempotency_key_ref": "digest:...",
  "created_at": "2026-09-20T12:00:00Z",
  "connected_at": null,
  "ended_at": null,
  "outcome": null
}
```

The configured inbound route or outbound workflow resolves `workspace_id`
server-side. Provider or caller metadata cannot select it. Provider IDs are
scoped by provider connection/account and are not authorization.

## Lifecycle States

```text
CREATED
DIAL_REQUESTED
DIALING
RINGING
CONNECTED
ACTIVE
HELD
TRANSFER_REQUESTED
TRANSFERRING
VOICEMAIL
ENDING
COMPLETED
FAILED
CANCELLED
```

`COMPLETED`, `FAILED`, and `CANCELLED` are terminal. Identity assurance,
recording/transcript policy, current turn, and transfer status are related state,
not overloaded into the lifecycle enum.

Initial transition rules:

- outbound: `CREATED -> DIAL_REQUESTED -> DIALING -> RINGING/CONNECTED`;
- inbound: verified configured ingress creates `CREATED`, then
  `RINGING/CONNECTED`; unknown routes do not create an unscoped call;
- `CONNECTED -> ACTIVE` only after provider/session binding is valid;
- `ACTIVE <-> HELD` and `ACTIVE -> TRANSFER_REQUESTED -> TRANSFERRING` are
  explicit controller commands/observations;
- voicemail detection moves a connected/ringing call to `VOICEMAIL` only when
  the configured provider evidence supports that observation;
- user/provider end requests move a nonterminal call to `ENDING` before one
  terminal outcome;
- policy denial or cancellation before provider acceptance becomes `CANCELLED`;
  provider/transport failure becomes `FAILED` with a typed class;
- no callback transitions a terminal call back to a live state.

An executable transition table is generated from the owning Rust domain model
and golden-tested against these rules. Unknown states or transitions fail closed
and enter reconciliation diagnostics rather than being coerced.

## Commands

Canonical commands include:

```text
call.start_outbound
call.accept_inbound
call.hold
call.resume
call.transfer
call.leave_voicemail
call.end
call.cancel
```

Commands carry call/workspace/principal context, expected state version,
deadline, reason, and idempotency key where they can create provider effects.
Policy and approval run before reservation. Provider invocation begins only
after the durable command/effect reservation commits.

Outbound `call.start_outbound` reuses one logical idempotency identity through
retries. A timeout after provider acceptance is `AMBIGUOUS` and reconciles by
provider status/callback; it never blindly redials. Target, purpose, voice/agent,
consent basis, time window, and material context are part of the action
fingerprint.

## Normalized Events

Each durable event has `event_id`, `call_id`, `workspace_id`, per-call `sequence`,
expected/new version, occurred/observed/recorded timestamps, type, source, source
delivery ID, correlation/causation, sensitivity, and bounded payload.

Initial event families:

```text
call.created
call.dial_requested
call.provider_accepted
call.ringing
call.connected
call.identity_assurance_changed
call.active
call.held
call.resumed
call.interruption_detected
call.turn_cancel_requested
call.transcript_finalized
call.transfer_requested
call.transfer_started
call.transfer_completed
call.transfer_failed
call.voicemail_detected
call.voicemail_message_completed
call.end_requested
call.disconnected
call.timeout
call.reconciliation_required
call.completed
call.failed
call.cancelled
```

There is exactly one canonical terminal event. Provider-specific event names and
payloads stay in protected ingress artifacts/evidence and are normalized by the
adapter.

## Callback Deduplication and Ordering

Before acknowledgement where provider deadlines permit:

1. authenticate the provider/connection and verify raw signature/timestamp;
2. resolve configured route and call binding server-side;
3. persist scoped delivery ID, raw artifact reference under retention, and
   observed provider timestamp;
4. deduplicate by provider connection plus delivery ID;
5. enqueue normalization/reconciliation independently from provider response;
6. apply an allowed transition using optimistic call version.

Provider timestamps do not establish authorization or canonical total order.
Out-of-order observations are retained and may fill missing evidence, but cannot
rewind state. A material callback conflict creates
`call.reconciliation_required`. After a terminal event, later callbacks can
update a separate reconciled outcome/diagnostic record through an audited
controller action; they do not erase or duplicate the terminal lifecycle event.

## Turns, Interruption, and Partial Transcripts

Each active turn has a stable turn ID and state:

```text
LISTENING
INPUT_FINALIZED
THINKING
TOOL_WAIT
SPEAKING
INTERRUPTED
COMPLETED
FAILED
CANCELLED
```

### End-of-turn detection

A turn ends on a **model-based end-of-turn signal** carrying provider confidence,
gated by a configured threshold, with a configured maximum-silence cap as a
backstop. End-of-turn detection is not a lifecycle state; it is an observation
that produces the transition into `INPUT_FINALIZED`. Its elapsed time is recorded
as its own metric and is additive to perceived latency rather than folded into it.

Where the provider withholds unfinalized prompts and eager end-of-turn signals by
default, the adapter must enable that channel explicitly; otherwise end-of-turn
latency is unmeasurable and the `INPUT_FINALIZED` transition is the first visible
event of the turn, which hides the caller's pause entirely.

### Partial and final transcripts

- Partial STT text is ephemeral observation by default. Only a provider-marked
  final transcript accepted by the adapter can become a finalized input item.
- Partial or late text never becomes a fabricated completed instruction and
  cannot directly execute a tool.
- A partial text that arrives after the turn was finalized is retained only as
  diagnostic evidence; it cannot mutate the finalized input.

### Interruption

- Barge-in records `call.interruption_detected`, stops/cancels the exact TTS/turn
  where supported, and starts a new listening turn only after controller state
  permits it.
- **Backchannel filtering and false-interruption recovery are different
  capabilities.** Filtering prevents a spurious stop; recovery resumes a reply
  after one. Only the second requires retaining what was already spoken, so their
  capabilities are declared separately and a provider is not credited with one
  because it has the other.
- Interrupting speech does not undo an already reserved external side effect.
  Cancellation/reconciliation follows the tool contract.

### Tool wait and progress

A tool wait can be held, ended, or continued by policy. Progress filler is
permitted only through a **provider-documented** mechanism, and a result that only
drafted or proposed something must never be spoken as completed. A provider-
reported outcome is a claim; the durable tool outcome is authoritative. Approval
that cannot complete within voice policy becomes an explicit follow-up or step-up
response, not an indefinitely open stream.

## Transfer and Voicemail

Transfer requires a typed target, policy decision, approval where configured,
provider capability evidence, deadline, and expected state version. Success,
failure, remote acceptance, and source-leg disconnect are separate observations.
Failed transfer returns to `ACTIVE` only when the source leg is proven usable;
otherwise it proceeds to `ENDING`/terminal reconciliation.

Voicemail detection is provider evidence, not certainty. Leaving a message is an
external communication action bound to approved content/purpose. It cannot start
new arbitrary tools or continue a live private conversation after the message
policy completes.

## Latency, Timeouts, and Cleanup

Record bounded metrics/deadlines for ingress auth, **end-of-turn detection**, STT
finalization, context build, model first token, tool wait, TTS first audio,
interruption stop, transfer, and total turn/call duration. Exceeding a deadline
produces a typed transition or degraded behavior; it never silently extends an
approval or budget.

Perceived latency is reported **per component, never as one summed number**, and
is attributed to the pipeline that produced it rather than to "voice". Two rules
apply to every recorded figure:

- the start of the clock is when the provider reported end of speech, which is
  earlier than when the caller actually stopped, by up to the configured
  maximum-silence cap;
- a figure whose pipeline emits text mid-flight and one whose pipeline has already
  synthesized audio are not comparable, and the record names which it is.

End/cancel performs bounded provider stop, stream shutdown, child-task
cancellation, final callback grace, artifact finalization, and credential
revocation. Cleanup timeout leaves a truthful `FAILED` or reconciliation state,
not false completion. Daemon restart resumes from durable call/effect/callback
state.

## Privacy and Retention

Recording, transcript, analysis, and provider-history settings resolve from
workspace/call policy before the provider session starts. Audio/transcript
artifacts carry workspace, consent, sensitivity, retention, and deletion state.
Provider-side deletion is reported only when current evidence and a verified
operation support it.

## Required Tests

1. generated transition table and every allowed/denied state pair;
2. duplicate, missing, late, conflicting, and out-of-order callbacks;
3. crash before/after command reservation, provider acceptance, and terminal
   event, including no-double-ring;
4. known/unknown/spoofed identity and route/workspace binding;
5. partial/final transcript ordering and no action from partial text;
6. interruption during listening, thinking, tool wait, and speaking;
7. transfer success/failure/source-leg loss and voicemail policy;
8. user end, provider hangup, timeout, outage, daemon restart, and cleanup limit;
9. latency budgets, quiet hours, consent, cost, and approval expiry;
10. recording/transcript disabled, redaction, export, delete, and late callback;
11. provider adapter replacement against the same canonical fixtures.

These tests provide evidence for `ACC-062` through `ACC-067`.
