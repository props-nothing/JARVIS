# ADR-0011: Realtime Media Sessions Are a Transport Boundary

Status: PROPOSED
Date: 2026-09-21
Supersedes: None
Superseded by: None

## Context

[ADR-0007](0007-voice-as-interface.md) makes voice a provider-neutral interface
with JARVIS as the brain, and
[voice-telephony.md](../architecture/voice-telephony.md) implements that for
telephone calls: carrier, audio pipeline, and reasoning are three separate
choices.

That model has no place for a realtime session that is **not** a telephone call.
A desktop client sharing a screen, a phone joining a live conversation, a camera
feed, a stream of telemetry, or a participant asking another participant to run a
method are all realtime capabilities with no owning requirement, contract, or
milestone. They are not calls, so the call contract cannot describe them, and they
are not request/response, so the local control API cannot describe them either.
[ADR-0005](0005-isolated-external-runtimes.md) covers external runtimes but says
nothing about media.

The current candidate transport also carries agent-orchestration features that
overlap JARVIS-owned capabilities, which makes it easy to adopt a transport and a
control plane in one step without deciding to. ADR-0007 already forbids the
second control plane; this ADR makes the transport half of that decision explicit
and separate.

Sources for the transport capabilities referenced here:
[LiveKit evidence](../research/integrations/livekit.md).

## Decision

Realtime media sessions are a **transport and perception boundary**, not a
reasoning or authority boundary.

1. **A media session is a durable, workspace-scoped JARVIS record.** A room,
   channel, or connection is an adapter's name for the same thing and never
   enters JARVIS state. JARVIS owns session identity, lifecycle, grants, capture
   state, recording state, and terminal reconciliation.
2. **Transport, perception, decision, and effect are four separate layers.** A
   transport carrying a stream is not a grant to interpret it, and interpreting it
   is not authorization to act on it. Every capability must be attributable to one
   of the four.
3. **The transport and the agent framework are two decisions on two axes.** A
   media transport may be adopted without adopting a framework's agents, or vice
   versa. Merging them is prohibited.
4. **Data-plane messages are policy input.** Participant attributes, metadata,
   text and byte streams, data tracks, and participant-to-participant calls are
   untrusted. None creates a JARVIS tool, grant, or identity. A
   request/response call is mediated like any other proposed effect.
5. **Capture is opt-in, per session, visible, and separately revocable.** A
   capture grant is not a perception grant; a perception grant is not an
   authorization.
6. **Grant checks occur at track granularity and are repeated at publication.**
   Transport-native permissions are defense in depth, never JARVIS authorization.
7. **A media adapter is optional.** Canonical session records, policy, tools, the
   CLI, and every non-media interface remain usable with the adapter removed.

Scope: local and remote interactive sessions for desktop, mobile, voice, and
bridged external legs. Out of scope: recording formats, rendering, and any
telephony carrier detail, which the call contract owns.

## Consequences

### Positive

- One boundary for realtime media instead of an implicit one inside whichever
  provider was integrated first.
- Screen, camera, and data-plane capabilities become decidable and testable
  rather than emergent.
- A framework's orchestration features cannot arrive as a side effect of adopting
  its transport.
- The session record is transport-independent, so a transport can be replaced
  without migrating canonical state.

### Negative

- A second session-like aggregate exists beside the call record, and the
  relationship must be maintained deliberately (a call is a participant).
- Track-level grants and capture state add contract surface before any transport
  is selected.
- Reconnection, degradation, and connection-phase semantics must be modelled per
  adapter rather than assumed uniform.

### Risks and Mitigations

| Risk | Mitigation/evidence |
| --- | --- |
| The session aggregate duplicates the call lifecycle | A bridged call is a participant; the call contract keeps call state and the media contract keeps session state, with one owner each |
| A transport's permission model is mistaken for JARVIS authorization | Track-level grants re-checked at publication; transport flags recorded as observations only (`ACC-081`) |
| A request/response surface becomes an unmediated tool path | Fixed method set, schema validation, canonical policy path, bounded timeout and payload (`ACC-080`) |
| Media transport is adopted because agent features are attractive | Two-axis rule in this ADR; framework adoption stays under ADR-0005 and `RTM-009` |
| Session state leaks into canonical memory or transcripts | Capture and retention default to off; media payloads retained only under explicit policy |

## Alternatives Considered

### Fold media into the voice call contract

Rejected. A call has carrier, dialing, consent, and telephony-law semantics that a
desktop screen share does not. Extending the call contract would force every media
session to carry call concepts and would make the call state machine ambiguous.

### Extend the local control API's WebSocket surface

Rejected as an owning model. The local control API is a control channel for JARVIS
operations; making it also the media and perception boundary would conflate
authorization with transport and leave camera, screen, and multi-participant
semantics undefined.

### Adopt a transport and its agent framework together

Rejected now, per ADR-0007. Adoption on both axes remains possible, but each
requires its own evidence, its own adapter, and its own scoped grants.

### Define media sessions only after a transport ships

Rejected. The boundary is cheapest to set before code exists, and a transport
adopted without it defines the boundary by accident — which is the failure this
ADR exists to prevent.

## Verification

- Domain and application crates contain no transport, room, or media SDK types.
- A transport-issued attribute cannot select a workspace or grant a capability.
- A track published without a matching grant is refused at publication.
- A participant-to-participant call cannot reach an unregistered method or execute
  a tool without policy.
- Capture defaults to off in the configuration and in tests.
- Canonical session records are readable through the CLI with the adapter
  uninstalled.
- Fitness function: every media capability is attributable to transport,
  perception, decision, or effect in its owning document.
