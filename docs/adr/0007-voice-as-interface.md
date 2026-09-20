# ADR-0007: Voice Is an Interface; JARVIS Remains the Brain

Status: ACCEPTED
Date: 2026-09-20
Supersedes: None
Superseded by: None

## Context

Voice/telephony providers offer excellent audio, phone, and agent services, but
making one provider the reasoning or memory authority would create a second
JARVIS with different tools and policy.

## Decision

Voice uses a provider-neutral port. Preferred ElevenLabs mode calls JARVIS's
authenticated OpenAI-compatible Responses/Chat Completions edge, while JARVIS
owns reasoning, context, memory, policy, and tools. A second mode lets an
ElevenLabs-owned agent consume a narrow JARVIS MCP export. Both remain subject
to JARVIS identity, approval, consent, and audit policy.

## Consequences

### Positive

- Voice quality/telephony can evolve independently.
- Phone and text interactions share canonical state and controls.
- Local or alternative voice providers remain possible.

### Negative

- Compatibility SSE and low-latency session handling require dedicated tests.
- Caller identity, callback order, consent, and telephony law add major risk.

## Alternatives Considered

- ElevenLabs agent as canonical JARVIS: rejected due to split authority.
- Build all STT/TTS/telephony locally first: rejected as delaying core proof;
  local voice remains a later adapter.

## Verification

- Current ElevenLabs evidence note and compatibility contract tests.
- Inbound identity and outbound no-double-ring/consent tests.
- Provider removal leaves canonical JARVIS data usable.