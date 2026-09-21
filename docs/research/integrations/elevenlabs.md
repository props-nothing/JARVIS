# Integration Evidence: ElevenLabs

Status: ACCEPTED
Review scope: architecture only; refresh required before implementation
Owner: Voice platform
Last verified: 2026-09-20
Revalidate by: 2026-12-19
Implementation gate: NOT READY

## Decision Summary

- Purpose: realtime voice/telephony transport for inbound and outbound JARVIS.
- JARVIS boundary: replaceable voice connector plus isolated OpenAI-compatible
  edge and optional scoped MCP export.
- Proposed SDK/API version: select during `VOI-002`; use current HTTP schemas,
  avoid making an SDK type a domain contract.
- Supported deployment modes: ElevenLabs Custom LLM as preferred JARVIS-brain
  mode; ElevenLabs agent as MCP client for specialized agents.
- Explicitly unsupported initially: batch/mass calling, voice-only critical
  approval, caller-ID-only identity, provider-owned canonical memory.
- Kill switch: disable provider, custom LLM client, MCP client, inbound route,
  outbound dialing, or one agent/number/account.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| `llms.txt` | https://elevenlabs.io/docs/llms.txt | Current | 2026-09-20 | Full docs/API index and agent instructions |
| Custom LLM | https://elevenlabs.io/docs/eleven-agents/customization/llm/custom-llm.md | Current | 2026-09-21 | Chat Completions/Responses endpoints, SSE, tools, extra body, reasoning, buffer words |
| MCP | https://elevenlabs.io/docs/eleven-agents/customization/tools/mcp.md | Current | 2026-09-20 | External MCP server setup, transports, approvals, restrictions |
| MCP security | https://elevenlabs.io/docs/eleven-agents/customization/tools/mcp/security.md | Current | 2026-09-20 | Third-party MCP risk guidance |
| SIP trunking | https://elevenlabs.io/docs/eleven-agents/phone-numbers/sip-trunking.md | Current | 2026-09-20 | Telephony setup entry |
| Twilio outbound | https://elevenlabs.io/docs/eleven-agents/api-reference/integrations/twilio/outbound-call.md | Current | 2026-09-20 | Outbound API entry |
| SIP outbound | https://elevenlabs.io/docs/eleven-agents/api-reference/sip-trunk/outbound-call.md | Current | 2026-09-20 | Outbound SIP API entry |
| Post-call webhooks | https://elevenlabs.io/docs/eleven-agents/workflows/post-call-webhooks.md | Current | 2026-09-20 | Completion callback entry |
| System tools | https://elevenlabs.io/docs/eleven-agents/customization/tools/system-tools | Current | 2026-09-21 | Exact system-tool names and parameter schemas |
| Agent auth | https://elevenlabs.io/docs/eleven-agents/customization/authentication.md | Current | 2026-09-20 | Agent access/auth entry |
| Privacy/retention | https://elevenlabs.io/docs/eleven-agents/customization/privacy.md | Current | 2026-09-20 | Recording/history retention entry |
| OpenAPI | https://elevenlabs.io/docs/openapi.json | OpenAPI 3.1 | 2026-09-20 | HTTP endpoint schema |
| AsyncAPI | https://elevenlabs.io/docs/asyncapi.json | AsyncAPI 2.6 | 2026-09-20 | WebSocket channel schema |

Attempted `llms.txt` URLs that did not exist:

- None. The docs also advertise section-specific indexes and `.md` pages.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| Custom LLM | Responses first; Chat Completions fallback | Current docs support both | DOCUMENTED, live fixtures pending |
| MCP | Streamable HTTP | Docs support Streamable HTTP and SSE | DOCUMENTED, negotiated version pending |
| Telephony | One dedicated test route/provider | SIP/Twilio/other endpoints listed | UNVERIFIED until account/live test |
| Webhooks | Current signed callback contract | Current docs page/OpenAPI | UNVERIFIED exact signature semantics |

## Contract

### Authentication and Authorization

- ElevenLabs API keys/secrets are provider credentials resolved in its adapter.
- Custom LLM/MCP calls into JARVIS require a separate scoped service client and
  short-lived voice-session binding.
- Current safe mechanism for passing the opaque JARVIS session token through
  ElevenLabs configuration remains UNVERIFIED until exact auth/dynamic-variable
  behavior is tested.
- Caller ID and request `user_id`/extra fields are not authentication.
- High-risk JARVIS actions still require JARVIS policy/approval.

### Transport and Lifecycle

- Custom LLM supports OpenAI-compatible `/v1/chat/completions` and
  `/v1/responses`, both SSE with `Content-Type: text/event-stream`.
- Current docs state both API formats are **fully supported** for custom LLM
  integration.
- Chat Completions chunks use `data: {json}\n\n` and end `data: [DONE]\n\n`.
- Responses uses typed `event:` plus `data:`; the documented minimum events are
  `response.output_text.delta` and `response.completed`, followed by `[DONE]`.
  An `error` event is the documented failure shape.
- **Reasoning must be returned separately from the answer**; the provider does not
  derive it from the final text. Chat Completions streams it in a `reasoning` or
  `reasoning_content` delta; Responses returns a `reasoning` output item with the
  text in its `summary` field. With reasoning summary enabled the provider sets
  `reasoning.summary` to `auto` on the request. Gemini-compatible endpoints are
  requested with a thinking-config flag and return thought content in a separate
  marker field. JARVIS stores and returns only a safe summary.
- **Documented slow-model progress mechanism.** When the endpoint needs longer to
  produce a full answer, it may return an initial chunk ending with an ellipsis
  and a **trailing space**; the documentation states the space matters because
  otherwise the next content is appended to the ellipsis and produces audio
  distortion. This is the provider-sanctioned progress filler and is preferable to
  inventing filler that could imply success. It is not a substitute for a durable
  outcome.
- Custom LLM receives configured system tools in standard OpenAI tool format and
  must return function calls to use them.
- MCP integration supports SSE and Streamable HTTP servers and per-server/tool
  approval modes. JARVIS selects Streamable HTTP for new work.
- Post-call and outbound call exact state/callback semantics need focused page
  and live review before implementation.

### System Tools

Exact names and parameters are documented and can be pinned as contract fixtures.
They are configured on the agent and arrive as standard OpenAI function
definitions in the request's `tools` array. JARVIS decides whether the runtime may
propose each one; the provider executes provider-owned call control only after a
valid function call is echoed back.

| Tool | Parameters | JARVIS mapping |
| --- | --- | --- |
| `end_call` | `reason` (required), `message` (optional) | Call end intent |
| `language_detection` | `reason` (required), `language` (required, must be in the configured list) | Language-switch observation |
| `transfer_to_agent` | `reason` (optional), `agent_number` (required, **zero-indexed** per configured transfer rules) | Transfer intent to a configured agent |
| `transfer_to_number` | `reason` (optional), `transfer_number` (required), `client_message` (required), `agent_message` (required) | Transfer to a human |
| `skip_turn` | `reason` (optional) | Hold/continue decision |
| `voicemail_detection` | `reason` (required) | Voicemail evidence |

`transfer_to_number.agent_message` is a **context-leak path**: it is
model-authored text describing the caller's situation, and it is delivered to the
human receiving the transfer. It is untrusted content and must not carry private
JARVIS context, secrets, or memory excerpts. `agent_number` is an index into
configuration JARVIS owns, so an out-of-range value is a denial, not a lookup.
General JARVIS tools remain in the canonical tool fabric.

### Data and Limits

- Docs support additional custom LLM parameters under
  `elevenlabs_extra_body`; treat them as untrusted.
- Reasoning summary must be returned separately in documented fields/events;
  JARVIS never exposes hidden chain-of-thought.
- MCP is disabled by default at workspace level in current docs.
- Current docs state MCP is unavailable for Zero Retention Mode or HIPAA-required
  users. This affects deployment/privacy options and must be rechecked.
- Token, concurrency, telephony, timeout, pricing, retention, recording, and
  regional limits require account/feature-specific evidence before launch.

### Errors and Retries

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Invalid JARVIS edge auth | HTTP auth failure | No | No context; audit safe client failure |
| Invalid Custom LLM request | 4xx/schema error | No | Fail call turn safely |
| Stream disconnect | EOF/network | Context-dependent | Mark interrupted; do not infer completion |
| Provider 429/5xx | HTTP/provider error | Bounded if safe | Respect headers/deadline/budget |
| Outbound call ambiguous | Timeout after submit | No blind retry | Reconcile by idempotency/provider status |
| Duplicate callback | Repeated delivery ID/call state | No duplicate processing | Inbox dedupe and monotonic transition |

## Security Analysis

- Public Custom LLM/MCP endpoints expand remote attack surface.
- Dynamic variables/extra body/caller metadata can attempt workspace spoofing.
- System tools and MCP tools have separate provider and JARVIS approval layers.
- Phone calls require consent, disclosure, quiet hours, destination restrictions,
  spend/rate limits, opt-out, and regional policy.
- Transcripts/audio can contain biometric/private/highly sensitive data.
- Webhooks need exact signature/raw-body/replay verification.
- Prompt injection spoken or retrieved during a call cannot become policy.
- Batch calling is explicitly deferred due to abuse/compliance risk.

## Normalization Map

| ElevenLabs concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| Agent conversation | Voice session plus agent run(s) | JARVIS owns canonical session/run |
| Custom LLM request | Authenticated voice turn | Extra metadata untrusted |
| System tool | Voice call-control intent | Explicit mapping/policy only |
| MCP server/tool approval | External client/export plus defense-in-depth approval | JARVIS approval remains authoritative |
| Provider call/conversation ID | Scoped external call reference | Never canonical identity |
| Post-call webhook | Connector ingress event | Verify/dedupe/normalize |
| Transcript/audio | Sensitive artifacts | Retention/consent-controlled |

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `EL-C001` | Custom LLM supports Responses and Chat Completions SSE | DOCUMENTED | Official custom LLM page | Captured request/stream is rejected |
| `EL-C002` | Responses minimum events and `[DONE]` are sufficient for basic text | DOCUMENTED | Official example | Live agent requires additional event/order |
| `EL-C003` | ElevenLabs can consume JARVIS Streamable HTTP MCP | DOCUMENTED | Official MCP page | Live integration cannot negotiate/call |
| `EL-C004` | Opaque signed session can be passed securely | UNVERIFIED | Architecture only | Provider cannot place/keep it secret or bind per call |
| `EL-C005` | Outbound idempotency can prevent duplicate dialing | UNVERIFIED | JARVIS ledger design | Provider API lacks safe lookup/key semantics |
| `EL-C006` | Webhook signature/replay contract can be verified before parse | UNVERIFIED | Webhook docs pending focused review | Official scheme differs or lacks required data |

## Test Plan

### Deterministic Tests

- [ ] Responses/Chat request normalization and exact SSE bytes
- [ ] System-tool mapping and JARVIS policy
- [ ] Session token expiry/replay/workspace spoofing
- [ ] Call transition monotonicity and callback dedupe
- [ ] Outbound consent/quiet hours/budget/idempotency
- [ ] Transcript/audio redaction and retention

### Contract Fixtures

- [ ] Capture current Custom LLM requests for both APIs
- [ ] Capture text, tool, reasoning-summary, error, and disconnect streams
- [ ] Capture current post-call webhook with signature headers
- [ ] Validate relevant OpenAPI/AsyncAPI operations
- [ ] Capture MCP initialize/list/call flow from dedicated agent

### Gated Live Tests

- [ ] Custom LLM text and voice turn
- [ ] Scoped MCP read tool and denied tool
- [ ] Known/unknown inbound call
- [ ] One explicitly approved outbound test call
- [ ] End/transfer/interruption/voicemail/provider failure
- [ ] Duplicate callback and no-double-ring recovery

## Operational Readiness

- [ ] Health/status and provider account/agent/number diagnostics
- [ ] Kill switches and credential rotation
- [ ] Callback lag/dead letter/reconciliation
- [ ] Cost/concurrency/latency metrics and budgets
- [ ] Privacy/recording/retention disclosure
- [ ] Legal/consent review for enabled regions/use cases
- [ ] Operator runbook and test-number isolation

## Open Questions

- Exact service-to-service authentication supported for Custom LLM endpoint.
- Exact per-call signed session metadata placement and secrecy.
- Current webhook signature algorithm, timestamp, retry, and delivery ID rules.
- Outbound API idempotency/status lookup guarantees.
- Account/plan support for Custom LLM, MCP, SIP/Twilio, ZRM, and regions.
- Whether Responses tool-call streaming needs additional required events.
- `transfer_to_number.agent_message`: what the human recipient actually sees, and
  whether JARVIS can redact or replace it before delivery.

Resolved 2026-09-21: reasoning must be returned separately and in which fields;
both API formats are fully supported; the exact system-tool names and parameter
schemas; the documented slow-model progress mechanism and its trailing-space
requirement.

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-20 | Initial architecture review | Official `llms.txt`, Custom LLM, MCP, OpenAPI/AsyncAPI indexes |
| 2026-09-21 | Recorded the exact system-tool names and parameters, reasoning-return fields, buffer-word progress mechanism, and the `agent_message` leak path | Official custom LLM and system-tools pages |