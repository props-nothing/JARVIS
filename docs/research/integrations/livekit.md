# Integration Evidence: LiveKit Agents and LiveKit SIP

Status: ACCEPTED
Review scope: architecture only; refresh required before implementation
Owner: Runtime platform
Last verified: 2026-09-21
Revalidate by: 2027-03-21
Implementation gate: NOT READY

## Decision Summary

- Purpose: candidate voice-agent orchestration framework, and a separate
  candidate WebRTC/SIP telephony transport.
- JARVIS boundary: **two distinct decisions that must not be merged.**
  1. **LiveKit Agents** is a Python/Node framework. There is no Rust Agents SDK.
     Adopting it means an **external runtime** behind a versioned runtime adapter
     (ADR-0005), not a library inside `jarvisd`.
  2. **The `livekit` Rust crate** is a realtime client/server SDK. It is an
     alternative *transport* that a Rust daemon could use without adopting the
     Python framework at all.
  3. **Realtime media state is a third decision.** Rooms, participants, tracks,
     capture, and the data plane are governed by
     [the media session contract](../../contracts/media-session.md) and
     [ADR-0011](../../adr/0011-realtime-media-session-boundary.md); the transport
     is replaceable and its state is not canonical.
- Proposed version: none pinned. `@livekit/agents` 1.9.0 was exercised in a spike;
  the Rust crates were read but not built here.
- Supported deployment modes: LiveKit Cloud for the agent server and SIP service,
  or self-hosted LiveKit with a separately deployed SIP service.
- Explicitly unsupported: making a LiveKit agent the JARVIS reasoning authority,
  canonical memory, or tool-permission decision point; treating the framework's
  checkpoints as JARVIS state.
- Kill switch: disable the runtime adapter or the transport; JARVIS core, canonical
  call state, CLI, tools, and policy must remain usable with LiveKit removed.

## Why This Note Exists

The spike that measured LiveKit is **not part of this repository** (excluded from
the repository and from the docs gate). This note is the durable record of the
review, so the LiveKit decision is discoverable, reviewable, and referenced from
the canonical plan instead of living only in an excluded folder.

It also corrects a framing error that the spike's own summary made. The spike
concluded "build Jarvis on LiveKit Agents". Two facts change how that should be
read:

1. **There is no Rust Agents SDK.** LiveKit Agents is Python and Node only. JARVIS
   is a Rust control plane, so the framework cannot be a core dependency; it can
   only be an external runtime, which places it under ADR-0005 and the runtime
   protocol — including process isolation, scoped tool grants, heartbeat, and
   timeout enforcement.
2. **The competitor is already measured.** A cascaded pipeline on the incumbent
   carrier was measured at ~1306 ms perceived, and a native speech-to-speech
   pipeline at ~1155 ms. The difference was inside single-call noise, and it
   disappeared once streaming was added to the cascaded path. The remaining gap
   was the transport, not the brain.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` (agents) | https://docs.livekit.io/agents/llms.txt | 230 pages, rendered 2026-09-21 | 2026-09-21 | Full Build Agents index; every page resolves as Markdown by appending `.md` |
| `llms.txt` (all docs) | https://docs.livekit.io/llms.txt | Current | 2026-09-21 | Every documentation section |
| Full docs corpus | https://docs.livekit.io/llms-full.txt | Current | 2026-09-21 | Every page inlined |
| Docs MCP server | https://docs.livekit.io/mcp/ | Current | 2026-09-21 | Documented coding-agent access path |
| Test framework | https://docs.livekit.io/agents/start/testing/test-framework.md | Current | 2026-09-21 | Text-mode tests and assertions without a room |
| Agent simulations | https://docs.livekit.io/agents/start/testing/simulations.md | Current | 2026-09-21 | Simulated users and scenario files |
| Turn detection | https://docs.livekit.io/agents/logic/turns.md | Current | 2026-09-21 | Turn handling overview |
| Adaptive interruption handling | https://docs.livekit.io/agents/logic/turns/adaptive-interruption-handling.md | Current | 2026-09-21 | Distinguishes interruptions from backchanneling |
| Turn-taking tuning | https://docs.livekit.io/agents/logic/turns/tuning.md | Current | 2026-09-21 | Endpointing, interruption, and preemptive generation knobs |
| Fallback strategies | https://docs.livekit.io/agents/logic/fallback-strategies.md | Current | 2026-09-21 | STT/LLM/TTS fallback configuration |
| Async tools | https://docs.livekit.io/agents/logic/tools/async.md | Current | 2026-09-21 | Long-running tool handling so the agent keeps talking |
| OpenAI-compatible LLMs | https://docs.livekit.io/agents/models/llm/openai-compatible-llms.md | Current | 2026-09-21 | `base_url` custom endpoint support; Chat Completions format for custom providers |
| Realtime plugins | https://docs.livekit.io/agents/models/realtime.md | Current | 2026-09-21 | Speech-to-speech model integrations |
| Telephony introduction | https://docs.livekit.io/telephony.md | Rendered 2026-09-21 | 2026-09-21 | SIP participant, trunks, dispatch rules, connectors, supported features |
| Cold transfer | https://docs.livekit.io/telephony/features/transfers/cold.md | Current | 2026-09-21 | REFER-based transfer |
| Warm transfer | https://docs.livekit.io/telephony/features/transfers/warm.md | Current | 2026-09-21 | Agent-assisted transfer with SIP dialing and hold music |
| DTMF | https://docs.livekit.io/telephony/features/dtmf.md | Current | 2026-09-21 | DTMF support |
| Secure trunking | https://docs.livekit.io/telephony/features/secure-trunking.md | Current | 2026-09-21 | SRTP |
| SIP server self-hosting | https://docs.livekit.io/transport/self-hosting/sip-server.md | Current | 2026-09-21 | The SIP service must be deployed separately when self-hosting |
| Rust SDK repository | https://github.com/livekit/rust-sdks | Current | 2026-09-21 | `livekit`, `livekit-api`, `livekit-protocol`, `livekit-token`; Apache-2.0 |
| Node agents repository | https://github.com/livekit/agents-js | Current | 2026-09-21 | The Node framework exercised in the spike |
| Python agents repository | https://github.com/livekit/agents | Current | 2026-09-21 | The primary framework |
| Transport section index | https://docs.livekit.io/transport/llms.txt | 59 pages, rendered 2026-09-21 | 2026-09-21 | Every transport page: media, data, encryption, egress/ingress, self-hosting |
| Core concepts | https://docs.livekit.io/intro/basics/rooms-participants-tracks | Current | 2026-09-21 | Room, participant, and track model; a screen share is a published video track |
| Data overview | https://docs.livekit.io/transport/data | Current | 2026-09-21 | Text streams, byte streams, RPC, data tracks, data packets, state sync, and their delivery patterns |
| Remote procedure calls | https://docs.livekit.io/transport/data/rpc | Current | 2026-09-21 | Peer-to-peer method calls, 15KiB payload limit, 10s default timeout, error codes, hidden participants excluded |
| Sending text | https://docs.livekit.io/transport/data/text-streams | Current | 2026-09-21 | Topics, per-stream ordering, no server-side persistence, joining mid-stream receives nothing |
| Screen sharing | https://docs.livekit.io/transport/media/screenshare | Current | 2026-09-21 | Screen published as a video track; tab-audio capture is browser-dependent |
| Webhooks and events | https://docs.livekit.io/intro/basics/rooms-participants-tracks/webhooks-events | Current | 2026-09-21 | JWT-signed webhook over a sha256 of the payload, no delivery guarantee, sequenced retries, signal vs media connection phases, connection quality |
| Distributed self-hosting | https://docs.livekit.io/transport/self-hosting/distributed | Current | 2026-09-21 | Redis is required as shared data store and message bus for distributed mode |

Attempted `llms.txt` URLs that did not exist:

- None. `https://docs.livekit.io/agents/llms.txt` and
  `https://docs.livekit.io/llms.txt` both returned usable indexes. Note the index
  advertises `https://docs.livekit.io/mcp/` as a documentation MCP server.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| LiveKit Agents (framework) | Not pinned; external runtime only | Python and Node, current | DOCUMENTED; no Rust SDK exists |
| `@livekit/agents` (Node) | 1.9.0 exercised in the spike | 1.x | OBSERVED headless in the spike |
| `livekit` (Rust realtime) | Not pinned | Current | DOCUMENTED; requires libwebrtc and custom `rustflags` |
| `livekit-api` (Rust server API) | Not pinned | Current | DOCUMENTED; SIP grants, participant creation, agent dispatch |
| LiveKit SIP | Cloud or self-hosted service | Current | UNVERIFIED in JARVIS |
| LiveKit Inference | Not adopted | ElevenLabs retired from the bundle on 2026-08-31 | DOCUMENTED; use your own provider accounts |
| Realtime transport (rooms, tracks, data) | Not pinned | Current | DOCUMENTED; rooms, participants, tracks, text/byte streams, RPC, data tracks, state sync, E2EE |
| Self-hosted server (single node) | Not pinned | Current | DOCUMENTED; local mode needs no Redis, distributed mode does |

## Contract

### What Was Actually Verified

Headless, with no LiveKit account, no API key, and no phone call:

- Tool calling works and is assertable in CI. A fake model drives a text-mode
  agent session with no room and no account; the tool is asserted called by name
  with controlled arguments.
- A real model chooses the tools. One turn against a real model, detected from the
  framework's own function-tool-executed event.
- Two independent signals agree — the framework event and a recorder inside the
  tool both report the same tool name. A disagreement between them would itself be
  the bug.

Two API traps found by running it, both of which produce misleading errors:

1. Tools must be built with the framework's `tool()` helper and a schema library.
   A plain object with a JSON-Schema `parameters` field throws
   `unknown tool type: object` at agent construction — the error names the *type*,
   not the missing wrapper, so it reads like a wrong container.
2. A logger must be initialized before constructing anything. Without it every test
   fails with a logger error and the suite is reported as **skipped**, which looks
   like a harness problem rather than a missing setup call.

### What Was Not Verified

Stated plainly, because these are the parts that would change the
recommendation if they went the other way:

- **Telephony.** SIP trunks, inbound/outbound calls, DTMF, transfers,
  answering-machine detection, and dispatch rules were never exercised. The
  incumbent carrier is documented as a supported SIP provider, so it should work,
  but "should" is not verified.
- **Turn-detection latency.** Only measurable on a live audio session. The audio
  turn detector is a local inference model, so first use may download weights.
- **Agent server deployment.** Requires the CLI and an account.
- **Cost.** LiveKit Inference is Cloud billing; self-hosting moves cost to your own
  provider keys; the noise-cancellation feature used in their examples is
  Cloud-only.

### Telephony Contract (as documented)

Endpoint(s): LiveKit Cloud service APIs; a SIP service is a separate deployment
when self-hosting.

Transport(s): SIP over UDP, TCP, or TLS. SIPREG and SIPREC are documented as
**not supported**. Secure RTP is supported.

Connection lifecycle: a SIP participant represents a caller or callee and is
managed with the same participant APIs as any other join. Inbound calls create a
SIP participant automatically; outbound calls require an explicit participant
create. Trunks bridge the third-party SIP provider and LiveKit; dispatch rules
attach to a trunk and decide which room receives an inbound call, and can add
custom participant attributes.

Negotiation/versioning: not a versioned handshake at the SIP layer; behavior is
governed by trunk and dispatch-rule configuration held in the platform.

Ordering and duplication: a SIP participant's arrival, its attributes, and any
connector events are platform observations that must be normalized and deduped
exactly like any other provider callback.

Cancellation and timeout: transfer is REFER-based for cold transfer and
agent-assisted for warm transfer. Both are side-effecting and must be policy-gated.

Reconnect/resume: not established here. Treat a disconnect as a call observation,
not as completion.

### Two Things That Matter for JARVIS's Architecture

**1. The framework is usable behind the OpenAI-compatible edge.** The framework's
custom-endpoint support takes a base URL and key for any OpenAI-compatible
provider, and custom providers use the **Chat Completions** format. That is
directly relevant to JARVIS, which already plans an OpenAI-compatible compatibility
edge for voice: that edge is a plausible integration point for an external runtime
as well, which would let a runtime propose turns without gaining direct access to
JARVIS tools.

This is a capability observation, not an approved design. Reaching JARVIS this way
bypasses the runtime protocol's scoped-tool-grant model, so it must be an explicit,
reviewed decision with its own threat model rather than an accident of
compatibility.

**2. Native tool-loop features overlap JARVIS's canonical capabilities.** The
framework ships async tools, tool-loop design guidance, MCP client support, task
and workflow abstractions, agent handoffs, session orchestration, and fallback
strategies for each pipeline stage. Every one of those is a JARVIS-owned concern
under ADR-0001/0004. Using them inside a runtime is acceptable only when the
runtime's proposals are still validated against the canonical tool fabric; using
them *as* the JARVIS implementation would create the second control plane that
ADR-0007 forbids.

### Data and Limits

- Self-hosting LiveKit does not remove the SIP service: it is a separate deployment
  step.
- LiveKit Inference no longer serves a major TTS provider's models as of
  2026-08-31. The plugin still works with your own provider account. This is a
  concrete example of platform risk: a bundled component can be removed, and using
  your own accounts insulates against that.
- Concurrency, quota, and pricing are `UNVERIFIED`; they are Cloud-plan dependent.
- Noise cancellation is documented as a Cloud feature.
- Data residency is a Cloud-region or self-hosting decision and must be recorded
  per deployment.

### Errors and Retries

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Invalid API key or secret | Auth error | No | Fail closed; no context disclosure |
| SIP trunk misconfiguration | Participant never joins / trunk failure | Bounded | Typed failure; do not claim the call connected |
| Room/agent dispatch failure | Dispatch error | Bounded | Reconcile by dispatch identity; no duplicate agent |
| Media failure | Participant disconnect reason | Conditional | Mark degraded; preserve truthful outcome |
| Runtime process crash | Supervisor observation | Yes, isolated | Contain; never crash the daemon |
| Provider unavailable | Transport error | Bounded | Degrade to a configured fallback route only if policy allows |

## Security Analysis

- Trust boundaries: an external runtime is a separate process with its own failure
  domain. It must never receive the daemon's ambient credentials.
- Prompt-injection exposure: a runtime's own turn-taking, memory, and prompt
  handling must not become policy. Tool intents remain proposals.
- Secret leakage paths: provider keys, API secrets, and SIP credentials. Resolve
  them inside the adapter; never place them in a URL or a loggable field.
- SSRF or callback risks: media endpoints, trunk addresses, and webhooks must be
  configuration, never caller-supplied.
- Tool side effects: transfer and outbound dialing are communication actions
  requiring policy, consent, budget, and approvals.
- Required approvals: any side-effecting call control must bind the exact action
  fingerprint.
- Redaction rules: transcripts, audio, participant identities, and phone numbers
  are sensitive; redact errors, logs, traces, diagnostics, and support exports.
- Sandbox or network policy: the runtime process needs a deny-by-default policy for
  filesystem and network except its documented endpoints.
- Abuse cases: the same set as any telephony provider — toll fraud, destination
  scanning, prompt injection, and call flooding.

### Dependency and Build Considerations

- The Rust realtime crate requires **libwebrtc** and custom `rustflags` in the
  Cargo configuration. That conflicts with the installable-foundation exit gate,
  which requires no development runtime on the target machine, and with
  `AGENTS.md`'s dependency-budget rule. Any adoption must resolve that explicitly.
- No dependency may be added without completing the evidence gate and a
  dependency-ledger entry or a full note; this note records the review, not the
  approval.

## Normalization Map

| LiveKit concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| Participant | Scoped external participant reference | Never canonical identity |
| Room | Session/transport grouping | Runtime state, not canonical session |
| SIP participant | Inbound/outbound call leg | Normalize to the neutral call record |
| Trunk | Provider connection binding | Configuration, scoped by connection |
| Dispatch rule | Inbound routing rule | Must resolve workspace server-side |
| Turn detector events | Turn and interruption observations | Feed the call controller; do not become policy |
| Tool invocation | Tool intent proposal | Validated by JARVIS policy before any effect |
| Agent dispatch | Runtime invocation | Map to a JARVIS run; not a run identity |
| Agent server job | Runtime worker lifecycle | Supervisor-managed, isolated |
| Session/task state | Runtime checkpoint | Never canonical JARVIS state |
| Room metadata / participant attributes | Untrusted broadcast observation | Never authorization, a workspace selector, or a secret store |
| Track (audio, video, screen, data) | Media stream subject to a JARVIS grant | Grant recorded on the track and re-checked at publication |
| RPC method | Provider-shaped function call between peers | Fixed JARVIS method set, schema-validated, canonical policy path |
| Text or byte stream | Bounded untrusted message | Never a tool invocation; history is JARVIS-owned |
| Signal vs media connection | Two distinct session phases | A signalled participant is not one that can carry media |

Do not expose framework or SDK types in JARVIS domain contracts.

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `LK-C001` | Agent behaviour, including tool invocation, is testable with no room, no account, and no phone call | `OBSERVED` | Headless spike: text-mode session with a fake model, tool asserted by name | A tool assertion requires a room connection or an account |
| `LK-C002` | A real model selects the tools through the framework | `OBSERVED` | Spike smoke run; framework event and in-tool recorder agree | The model's selection is not observable or the two signals disagree |
| `LK-C003` | There is no Rust Agents SDK, so the framework can only be an external runtime | `DOCUMENTED` | Agents repositories are Python and Node; the Rust repository is a client/server SDK | An official Rust agent framework is published |
| `LK-C004` | Turn detection is enabled by default and is model-based rather than a silence window | `DOCUMENTED` | Turn-handling documentation | A default configuration uses a fixed silence timer |
| `LK-C005` | A custom OpenAI-compatible endpoint is supported via an explicit base URL, using the Chat Completions format | `DOCUMENTED` | OpenAI-compatible LLM page | A custom endpoint cannot be configured, or requires a provider wrapper |
| `LK-C006` | The incumbent carrier interoperates as a SIP provider | `UNVERIFIED` | Documented supported-provider list | A call cannot be placed over the trunk |
| `LK-C007` | The Rust realtime crate can be adopted without a development runtime on the target machine | `UNVERIFIED` | Crate documentation notes libwebrtc and `rustflags` requirements | It builds and links from a clean target profile |
| `LK-C008` | LiveKit telephony latency is comparable to the incumbent carrier's | `UNVERIFIED` | No live measurement | A live call is materially slower or faster |
| `LK-C009` | Adopting the framework without the runtime protocol creates a second control plane | `INFERRED` | Framework owns tools, MCP, tasks, handoffs, and fallbacks | The framework's proposals are fully mediated by the canonical tool fabric |
| `LK-C010` | Webhook delivery is not guaranteed and a delivery may be retried | `DOCUMENTED` | The webhook page states delivery has no guarantees and that events are retried with sequencing | A delivery is asserted exactly-once, or a duplicate is not observable |
| `LK-C011` | A participant can invoke methods on other participants in the same room, with a 15KiB payload limit and a 10s default timeout | `DOCUMENTED` | The RPC page and its error-code table | RPC is not reachable between participants, or the limits differ in practice |
| `LK-C012` | Text streams have no server-side persistence and a participant joining mid-stream receives none of it | `DOCUMENTED` | The text-stream page, sections "No message persistence" and "Joining mid-stream" | History is retrievable from the transport after the fact |
| `LK-C013` | The signal connection and the media connection are separate phases, so a participant can be connected and unable to carry media | `DOCUMENTED` | The connection-events section of the webhook page | `participant_joined` fires without an established media connection |
| `LK-C014` | Distributed self-hosting requires Redis as a shared data store and message bus; single-node local mode does not | `DOCUMENTED` | The distributed self-hosting page | A distributed deployment runs without Redis |
| `LK-C015` | A screen share is published as an ordinary video track, and tab-audio capture availability depends on the browser | `DOCUMENTED` | The screen-sharing page, including its browser-support note | Screen share requires a distinct track type, or tab audio works everywhere |

## Test Plan

### Deterministic Tests

- [ ] Runtime adapter conformance against fixture runtime behaviour
- [ ] Scoped tool grants: a runtime cannot invoke an undeclared tool
- [ ] Turn and interruption events map to controller transitions
- [ ] Dispatch and trunk configuration resolve workspace server-side
- [ ] Redaction of participant identities, phone numbers, and credentials

### Contract Fixtures

- [ ] Captured participant-attribute and connector event shapes
- [ ] Captured dispatch-rule and trunk configuration
- [ ] Captured transfer and DTMF flows
- [ ] Malformed and forward-compatible events

### Gated Live Tests

- [ ] One inbound call over a SIP trunk in a dedicated account
- [ ] One approved outbound call
- [ ] Turn-detection latency on a live audio session, with weight-download time
      recorded separately
- [ ] Cold and warm transfer
- [ ] Process crash and reconnect containment
- [ ] Cleanup leaves no billable resource behind

## Operational Readiness

- [ ] Health probe for the runtime process and the transport
- [ ] Safe diagnostics: configuration with no credentials
- [ ] Separate metrics for transport, turn detection, and generation
- [ ] Setup and reauthentication
- [ ] Disable/unload of the adapter
- [ ] Migration of provider-specific identifiers
- [ ] Credential rotation
- [ ] Provider outage behavior
- [ ] Operator runbook
- [ ] Version pinning and upgrade policy for a fast-moving framework

## Open Questions

- Does a live call over the incumbent carrier's SIP trunk work at all, and with
  what latency?
- Does the turn detector download weights on first use, and what is the cold-start
  cost?
- Can the Rust realtime crate be built and linked on all tier-1 targets without a
  development runtime, and what is the artifact-size cost?
- Is there a Rust path to framework-equivalent behaviour, or does adoption
  necessarily mean a Python/Node process?
- What is the self-hosting operational cost of the separate SIP service?
- Does the transport's reconnection support resume a session without ambiguity about
  media that was missed while disconnected?
- Can track-level JARVIS grants be enforced so a transport permission bug cannot
  widen them, and is subscribe authorization observable before the first frame?
- Is a single-node self-hosted media deployment sufficient for the intended
  personal-use volume, or does the Redis requirement arrive with the first
  second node?
- Is the OpenAI-compatible edge a legitimate runtime-facing surface, or does it
  bypass the runtime protocol's scoped grants?
- Current Cloud plan pricing and concurrency for the intended personal-use volume.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-21 | Initial evidence note; absorbs the headless spike result, the two API traps, and the documented telephony surface from the excluded telephony folder | Official `llms.txt` indexes, turn-handling and OpenAI-compatible pages, telephony reference, Rust SDK repository |
| 2026-09-21 | Added the realtime media surface: rooms/participants/tracks, the data plane including peer RPC, capture and screen sharing, webhook signature and delivery semantics, connection phases and quality, and the distributed self-hosting Redis requirement. Claims `LK-C010` through `LK-C015` added. Scope widened from telephony-only to the transport axis, which is now owned by [ADR-0011](../../adr/0011-realtime-media-session-boundary.md) and [the media session contract](../../contracts/media-session.md) | `transport/llms.txt`, `intro/llms.txt` and the pages listed in Official Sources |
