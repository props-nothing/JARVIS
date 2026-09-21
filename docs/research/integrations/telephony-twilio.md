# Integration Evidence: Twilio Telephony (ConversationRelay and Media Streams)

Status: ACCEPTED
Review scope: carrier, audio pipeline, and measured latency baseline; fixtures
  and live call tests pending
Owner: Voice platform
Last verified: 2026-09-21
Revalidate by: 2027-03-21
Implementation gate: NOT READY

## Decision Summary

- Purpose: telephone carrier and inbound/outbound call transport for JARVIS,
  and the measured baseline for voice turn latency.
- JARVIS boundary: replaceable voice connector plus webhook ingress. Twilio never
  owns identity, memory, policy, tools, or canonical call state.
- Proposed API version: TwiML `<ConversationRelay>` noun and the Programmable
  Voice REST API as currently documented; no vendor SDK type becomes a domain
  contract.
- Supported deployment modes: ConversationRelay (Twilio owns STT/TTS and the turn
  boundary; JARVIS exchanges text) as the preferred mode; Media Streams (raw audio
  to JARVIS) as an alternative that JARVIS has measured but not adopted.
- Explicitly unsupported initially: batch/mass calling, caller-ID-only identity,
  Twilio-owned canonical memory, `multi` language mode without the provider
  pairing the platform requires.
- Kill switch: disable the voice adapter, the inbound route, outbound dialing, or
  one number/account. JARVIS call state, policy, and audit stay usable with Twilio
  removed.

## Evidence Provenance

Some claims below are measured on real calls, and the measurement came from a
Node spike that is **not part of this repository** (it is excluded from the
repository and from the docs gate). This note is the durable record of that
work; the measurements are reproducible from the method described under
"Latency Measurement Method" but the harness itself is not retained.

Labels follow the integration research policy: `VERIFIED` (live test or exact
official schema), `DOCUMENTED` (current official docs), `OBSERVED` (captured real
response), `UNVERIFIED` (must not drive production behavior).

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | https://www.twilio.com/docs/llms.txt | Current | 2026-09-21 | Product index; Programmable Voice and Conversation Relay pages |
| `<ConversationRelay>` reference | https://www.twilio.com/docs/voice/twiml/connect/conversationrelay | `dateModified` 2026-06-22 | 2026-09-21 | Every TwiML attribute, nested element, defaults, and `action` callback payloads |
| Conversation Relay overview | https://www.twilio.com/docs/voice/conversationrelay | `dateModified` 2026-08-24 | 2026-09-21 | Product shape, AI Nutrition Facts per provider |
| WebSocket messages | https://www.twilio.com/docs/voice/conversationrelay/websocket-messages | Current | 2026-09-21 | `setup`/`prompt`/`text`/`dtmf`/`end`/`error` frame shapes |
| Media Streams overview | https://www.twilio.com/docs/voice/media-streams | Current | 2026-09-21 | Raw-audio transport and its WebSocket message set |
| `<Stream>` reference | https://www.twilio.com/docs/voice/twiml/stream | Current | 2026-09-21 | Mutually exclusive alternative to `<ConversationRelay>` |
| Voice webhooks | https://www.twilio.com/docs/usage/webhooks/voice-webhooks | Current | 2026-09-21 | Inbound webhook contract |
| Webhook security | https://www.twilio.com/docs/usage/webhooks/webhooks-security | Current | 2026-09-21 | Signature verification requirement |
| Sign webhooks with shared keys | https://www.twilio.com/docs/usage/webhooks/webhook-shared-keys | Current | 2026-09-21 | Key rotation via managed shared keys |
| API security best practices | https://www.twilio.com/docs/usage/rest-api-best-practices | Current | 2026-09-21 | Credential handling and rate-limit guidance |
| Voice pricing | https://www.twilio.com/docs/voice/pricing | Current | 2026-09-21 | Per-minute rates (account-specific via API) |
| Voice Insights: Conversation Relay events | https://www.twilio.com/docs/voice/voice-insights/api/call/details-conversation-relay-events | Current | 2026-09-21 | Server-side latency and interaction markers |
| Answering machine detection | https://www.twilio.com/docs/voice/answering-machine-detection | Current | 2026-09-21 | Voicemail heuristic and its accuracy caveats |
| Trusted calling (SHAKEN/STIR) | https://www.twilio.com/docs/voice/trusted-calling-with-shakenstir | Current | 2026-09-21 | Outbound attestation |
| Voice dialing geographic permissions | https://www.twilio.com/docs/sip-trunking/voice-dialing-geographic-permissions | Current | 2026-09-21 | Destination allow/deny controls |
| Error dictionary | https://www.twilio.com/docs/api/errors | Current | 2026-09-21 | Numeric error semantics |

Attempted `llms.txt` URLs that did not exist:

- None. `https://www.twilio.com/docs/llms.txt` returned a usable index. Individual
  pages resolve as Markdown by appending `.md`.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| TwiML `<ConversationRelay>` | Attribute set as of 2026-06-22 | Current | DOCUMENTED; `speechModel=flux` behavior OBSERVED in docs only |
| WebSocket message protocol | `setup`/`prompt`/`text`/`dtmf`/`end`/`error` | Current | DOCUMENTED; no captured fixtures yet |
| Programmable Voice REST | Calls, recordings, AMD | Current | UNVERIFIED in JARVIS |
| Webhook signature | Request validator over raw bytes | Current | UNVERIFIED exact algorithm and replay window |
| Media Streams | `<Connect><Stream>`, μ-law 8 kHz | Current | MEASURED in the spike (not adopted) |

## Contract

### Authentication and Authorization

- Account credentials are provider credentials resolved inside the adapter only.
  They never appear in prompts, logs, traces, diagnostics, or URLs.
- Inbound webhooks must verify the provider signature over the **raw request
  bytes** before parsing. The exact algorithm, header set, timestamp skew, and
  replay window are `UNVERIFIED` until captured against a live request.
- Twilio now supports signing webhooks with a shared key that can be rotated
  instead of the account auth token. Prefer that for rotation; the account auth
  token must not be the only signing material.
- Caller ID (`From`) is **not** authentication. JARVIS resolves identity
  server-side per the voice architecture; a phone number is a routing hint and a
  session-binding candidate, never a principal.
- ConversationRelay exposes no per-call secret channel for a JARVIS session
  token other than `<Parameter>` values, which Twilio documents as **not** treated
  as sensitive/PCI data. Treat any value placed there as observable by the
  provider and by the platform, and therefore as a reference, not a bearer secret.
  A scoped, short-lived token is still required; its placement remains
  `UNVERIFIED`.

### Transport and Lifecycle

Endpoint(s):

```text
POST  <PUBLIC_URL>/voice/incoming        inbound call webhook (TwiML response)
WSS   <PUBLIC_URL>/ws/conversation-relay ConversationRelay session
POST  <PUBLIC_URL>/voice/connect-action  <Connect action> completion callback
WSS   <PUBLIC_URL>/ws/media-stream       Media Streams session (alternative mode)
```

Transport(s): HTTPS webhooks plus a provider-initiated WebSocket. The socket must
be `wss://`.

Connection lifecycle:

1. Twilio posts the inbound call to the configured webhook.
2. JARVIS answers with TwiML containing `<Connect><ConversationRelay url="wss://…">`.
3. Twilio opens the WebSocket and sends a `setup` frame with `sessionId` and
   `callSid`, plus any `customParameters` from `<Parameter>` elements.
4. Turns exchange `prompt` (caller speech as text) and `text` frames.
5. The `<Connect action>` URL receives one final POST when the verb ends.

Negotiation/versioning: there is no protocol handshake. Behavior is selected by
TwiML attributes, so a provider-side default change silently alters the pipeline
(see "Default Drift" below).

Streaming frame shape and terminal event: JARVIS sends incremental `text` frames
with `last: false` and exactly one terminal frame with `last: true` per turn.
Twilio reports what was actually played via `tokensPlayed` events when
`events="tokens-played"` is set.

Ordering and duplication:

- `tokensPlayed` arrives **once per text message sent**, so one streamed turn
  produces several frames. A playback assertion must accumulate per turn and only
  flag divergence once the total can no longer be a prefix of what was sent.
  Comparing each frame against the whole utterance produces a false TTS alarm on
  every healthy streamed call (`OBSERVED`).
- The `<Connect action>` callback and any status callback can arrive late,
  duplicated, or out of order. Normalize through the call controller's inbox.

Cancellation and timeout: barge-in is controlled by `interruptible`
(`none`/`dtmf`/`speech`/`any`), gated by `interruptSensitivity`
(`high`/`medium`/`low`), and optionally filtered by `ignoreBackchannel`.

Reconnect/resume: no documented session resume. A dropped WebSocket ends the
ConversationRelay session; the call leg may survive. JARVIS must treat a socket
loss as an interruption of the turn, not as call completion.

### TwiML Attributes That Change JARVIS Semantics

| Attribute | Default | JARVIS consequence |
| --- | --- | --- |
| `speechModel` | `nova-3-general` (Deepgram) | `flux` selects server-side turn detection |
| `transcriptionProvider` | `Deepgram` | Provider choice for STT |
| `ttsProvider` | `ElevenLabs` | Provider choice for TTS; closed list |
| `eotThreshold` | `0.8` (0.5–0.9) | Confidence required to finish a turn; **only applies with Deepgram + `flux`** |
| `partialPrompts` | **`false`** | Unfinalized prompts and eager end-of-turn are **not** sent unless enabled |
| `speechTimeout` | `auto` (600–5000) | Silence window; under `flux` it becomes a **maximum-silence cap** |
| `interruptible` | `any` | Whether caller input stops TTS |
| `interruptSensitivity` | `high` | How easily speech interrupts TTS |
| `reportInputDuringAgentSpeech` | **`none`** (was `any` before 2025-05) | Caller input during agent speech is **not** reported unless enabled |
| `ignoreBackchannel` | **`false`** | Filters "yeah"/"uh-huh"/"okay" so they neither trigger nor interrupt |
| `preemptible` | `false` | Whether the next talk cycle can interrupt current TTS |
| `elevenlabsTextNormalization` | `off` | TTS-side number/date/currency normalization; `auto` behaves as `off` here |
| `deepgramSmartFormat` | `true` | STT-side normalization of dates, numbers, currency, addresses |
| `dtmfDetection` | off | Whether keypresses are delivered |
| `hints` | none | Phrase biasing for names and domain vocabulary |
| `events` | none | `speaker-events` and `tokens-played` subscriptions |
| `welcomeGreetingInterruptible` | `any` | Interruption policy during the greeting |

**Default Drift is a first-class risk.** `reportInputDuringAgentSpeech` changed
its default from `any` to `none` in May 2025. A pipeline built against the older
default silently loses caller input during agent speech — including barge-in
attempts and DTMF — with no error. JARVIS must set every attribute it depends on
explicitly and assert the serialized TwiML, because the vendor SDK drops unknown
attribute names silently.

### Two Mutually Exclusive Modes

`<ConversationRelay>` and `<Connect><Stream>` cannot both appear in one response,
and **Twilio silently runs only one** with no error and no warning. A call
intended for one architecture can quietly run as the other, and every latency
figure collected from it is then attributed to the wrong system. The mode must be
chosen in exactly one place in JARVIS and logged on both inbound and outbound
routes.

### Authentication and Session-Data Placement

- `<Parameter name=… value=…>` values appear under `customParameters` in the
  `setup` frame.
- Twilio documents TwiML attributes such as `welcomeGreeting`, `hints`, and
  `<Parameter>` values as **not** treated as PCI data, and the same for
  `handoffData` in the `end` message.
- Therefore a JARVIS session binding placed there is a **reference**, not a
  secret, and must still be validated server-side against replay, expiry, and the
  provider call ID.

### Data and Limits

- Audio is G.711 μ-law, 8 kHz, mono on the wire — this is the format that makes
  Media Streams expensive to own (resampling to and from 16 kHz/24 kHz) and is
  why ConversationRelay is the preferred mode.
- `speechTimeout` accepts 600–5000 ms inclusive; values outside the range are
  rejected.
- `eotThreshold` accepts 0.5–0.9 inclusive.
- Concurrency and account limits are `UNVERIFIED`; ConversationRelay publishes a
  dedicated concurrency error code, so the adapter must map it to a typed failure
  and must not retry it blindly.
- Retention, recording, and region depend on account configuration. Twilio
  Regions and Edge Locations determine processing and storage location; this is a
  data-residency input to routing and must be recorded per deployment.
- Cost: ConversationRelay is billed as an AI bundle that includes STT and TTS,
  and is roughly an order of magnitude above raw-audio Media Streams. Exact
  current per-minute rates are account-specific; read them from the Pricing API
  rather than from documentation tables, and treat any figure in JARVIS docs as
  `UNVERIFIED`.

### Errors and Retries

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Invalid webhook auth | Signature mismatch | No | Reject before parse; audit safe client failure |
| Invalid TwiML provider/model pairing | Provider ends the session with an error and disconnects | No | Fail the call turn; fix configuration |
| ConversationRelay concurrency limit | Documented concurrency error code | No | Do not blind-retry; surface as typed capacity failure |
| Socket connect failure | Session error, `SessionStatus: failed` | Bounded | Mark call degraded; do not claim completion |
| Caller or provider hangup | `SessionStatus: completed` | No | Complete the call once via the controller |
| Caller ID spoofing attempt | Any | No | Resolve identity server-side; never trust `From` |
| Outbound dial ambiguous | Timeout after submit | No blind retry | Reconcile by idempotency key and provider status |

## Security Analysis

- Trust boundaries: the public webhook and the provider-initiated WebSocket are
  both internet-facing. Both need authentication, rate limiting, bounded payloads,
  and bounded concurrency.
- Prompt-injection exposure: caller speech is untrusted data-plane input. Spoken
  instructions, including anything transcribed from a recording or a voicemail
  greeting, must never become policy or select a workspace.
- Secret leakage paths: account SID and auth token, a shared signing key, and any
  session token placed in `<Parameter>`. A credential embedded in a URL becomes a
  substring of every log line that mentions the URL, so credentials must be split
  from endpoint addresses rather than pasted into them.
- SSRF or callback risks: `url` and the `<Connect action>` URL must be JARVIS-owned
  and validated; never accept a caller- or model-supplied URL.
- Tool side effects: call control (end, transfer, leave voicemail) is a
  side-effecting action and must be policy-gated like any communication tool.
- Required approvals: outbound dialing and transfers require consent, quiet-hour,
  budget, and destination policy plus an exact approval fingerprint.
- Redaction rules: phone numbers are personal data; transcripts and audio are
  sensitive artifacts; errors, logs, traces, diagnostics, and support exports must
  be redacted to the same standard as any other connector.
- Abuse cases: international revenue-sharing fraud (premium-rate dialing),
  destination scanning, DTMF/prompt injection, voicemail-box abuse, and call
  flooding. Mitigations are geographic dialing permissions, per-destination and
  global spend caps, call-rate limits, and deny-by-default destinations including
  emergency numbers.

## Normalization Map

| Twilio concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| `CallSid` | Scoped external call reference | Never canonical identity |
| `sessionId` | Provider session reference | Runtime state, not canonical call state |
| `setup` frame | Authenticated voice-session start | `customParameters` untrusted until bound server-side |
| `prompt` (`last: true`) | Finalized input item | Partial prompts are ephemeral observation by default |
| `prompt` (`last: false`) | Ephemeral partial transcript | Never a completed instruction; never executes a tool |
| `text` token | Output speech chunk | Sanitized and chunk-safe before send |
| `interrupt`/speaker events | `call.interruption_detected` | Interrupting speech does not undo a reserved side effect |
| `tokensPlayed` | Playback verification evidence | One frame per text message, so accumulate per turn |
| DTMF | Input event | Policy-gated like any other input |
| `<Connect action>` POST | Post-call ingress event | Verify, dedupe, then normalize; never let it select a tenant |
| Recording/transcription | Sensitive artifacts | Consent, retention, and deletion policy controlled |
| `handoffData` | Handoff reason reference | Documented as not PCI data; do not place secrets |

Do not expose Twilio SDK types in JARVIS domain contracts.

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `TW-C001` | ConversationRelay carries a streamed text conversation under 1300 ms perceived latency | `OBSERVED` | Real calls: 1306 ms p50, 1210–1540 ms range | A call over the same path exceeds 2000 ms p50 with the same model |
| `TW-C002` | Streaming the reply materially lowers perceived latency relative to whole-reply generation | `OBSERVED` | 1825–2018 ms before, 1306 ms after, same pipeline | Streaming and non-streaming produce indistinguishable perceived latency |
| `TW-C003` | `partialPrompts` must be enabled for the provider to emit eager end-of-turn and unfinalized prompts | `DOCUMENTED` | Attribute reference, default `false` | Eager end-of-turn events arrive with `partialPrompts` unset |
| `TW-C004` | `flux` moves turn boundary detection server-side and turns `speechTimeout` into a maximum-silence cap | `DOCUMENTED` | Attribute reference; independently corroborated by a framework that offers the same model as STT endpointing | `flux` behaves as a plain transcription model and `speechTimeout` still delays the prompt |
| `TW-C005` | `ignoreBackchannel` plus `interruptSensitivity` reduce false interruptions without a framework-level recovery mechanism | `DOCUMENTED` | Attribute reference | Backchannels still cancel replies with `ignoreBackchannel=true` |
| `TW-C006` | Twilio exposes a TTS-side text-normalization switch for ElevenLabs independent of the provider's enterprise plan gate | `UNVERIFIED` | Attribute exists; whether it is equivalent to the provider's normalization is undocumented here | The attribute has no audible or measured effect on non-enterprise accounts |
| `TW-C007` | `<ConversationRelay>` and `<Connect><Stream>` are silently mutually exclusive | `DOCUMENTED` | Mode-selection guidance | Both run in one response, or an error is returned |
| `TW-C008` | `tokensPlayed` is emitted once per text frame, not once per utterance | `OBSERVED` | Streamed call with two played frames for one turn | Exactly one played frame arrives for a multi-frame turn |
| `TW-C009` | ConversationRelay exposes no secret channel for a JARVIS session token | `UNVERIFIED` | `customParameters` documented as not PCI data | A documented per-call secret/dynamic field exists |

## Latency Measurement Method

The baseline above is reproducible. It is recorded because the measurement is
durable even though the harness was not retained.

- **`generation`**: end of caller speech → reply text ready. This is the part
  JARVIS owns.
- **`perceived`**: end of caller speech → the caller hears speech. This is what
  the caller felt.
- **`turnDetectionMs`**: the gap between the last time the caller's transcript
  *changed* and the final prompt arriving. This is the silence the turn detector
  is responsible for, and it is **additive** to `perceived`, because `perceived`
  starts after turn detection finishes.

Two caveats that must travel with the numbers:

1. `endOfSpeech` is when Twilio *said* the caller stopped, not when they actually
   stopped. The real wait is longer by up to `speechTimeout`.
2. A latency comparison between two different pipelines (for example a
   text-in/text-out relay against an audio-in/audio-out live model) is not
   apples-to-apples: one stops its clock when text exists and the other has
   already synthesized speech. Report the components separately and state which
   pipeline each number belongs to. Folding them into one number hides the only
   measurement that responds to a turn-detection change.

## Test Plan

### Deterministic Tests

- [ ] TwiML serialization asserts every attribute JARVIS depends on, because the
      vendor SDK silently drops unknown attribute names
- [ ] Mode selection produces exactly one of `<ConversationRelay>` or `<Stream>`
- [ ] Frame parsing for `setup`, `prompt` (final and partial), `text`, `dtmf`,
      `end`, and `error`, including unknown additive fields
- [ ] `tokensPlayed` accumulated per turn rather than per frame
- [ ] Transcript sanitization is identical when streamed at every chunk size and
      when sent whole
- [ ] Callback deduplication by delivery ID and monotonic transition on replay
- [ ] Partial prompts never become a completed instruction or a tool call
- [ ] Redaction of phone numbers, transcripts, and credentials

### Contract Fixtures

- [ ] Captured inbound webhook with signature headers, with capture date
- [ ] Captured `setup` frame including `customParameters`
- [ ] Captured streamed turn showing one `tokensPlayed` frame per text message
- [ ] Captured `<Connect action>` completion payload
- [ ] Captured error frame and provider disconnect
- [ ] Malformed and forward-compatible frames

### Gated Live Tests

- [ ] Inbound call through a dedicated test number
- [ ] One approved outbound call to a dedicated destination
- [ ] `flux` versus a plain transcription model, measured with `partialPrompts`
      enabled so the turn-detection metric has data
- [ ] Barge-in during listening and during speech, with and without backchannel
      filtering
- [ ] Transfer, voicemail, end, and provider outage
- [ ] Duplicate and out-of-order callbacks
- [ ] Cleanup leaves no billable resource behind

## Operational Readiness

- [ ] Health probe for the webhook and socket paths
- [ ] Safe diagnostics: resolved providers, models, and mode with no credentials
- [ ] Metrics for every latency component recorded separately
- [ ] Setup and reauthentication for a number and account
- [ ] Disable/unload of one number, one route, or the whole adapter
- [ ] Migration of provider-specific identifiers into the neutral call record
- [ ] Credential and signing-key rotation
- [ ] Provider outage behavior: typed failure, no false completion
- [ ] Operator runbook including the tunnel/webhook-URL mistake that presents as
      "inbound calls do nothing"
- [ ] Account-level geographic permissions and spend caps verified before any
      outbound test

## Open Questions

- Exact webhook signature algorithm, header set, timestamp skew, and replay window.
- Whether a scoped session token can be placed per call without being observable
  as provider-visible configuration.
- `elevenlabsTextNormalization` effectiveness outside an enterprise plan.
- Current per-minute ConversationRelay and Media Streams rates, and whether the
  AI bundle changes with provider selection.
- `flux` versus a plain model on real calls, with `partialPrompts` enabled.
- LiveKit telephony compatibility for the same trunk, tracked in the LiveKit note.
- Whether `preemptible` and `reportInputDuringAgentSpeech` should be enabled for
  the personal-assistant profile.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-21 | Initial evidence note; absorbs the measured latency baseline, the mode-exclusivity trap, and the attribute table from the excluded telephony spike | Official `llms.txt` and attribute reference, plus real-call measurements |
