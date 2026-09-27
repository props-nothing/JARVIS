# Integration Evidence: OpenAI-compatible Model Provider

Status: ACCEPTED
Review scope: OpenAI Chat Completions wire contract, streaming SSE shape, auth, errors, limits, retention, and the transport decision for the first provider adapter
Owner: Model gateway
Last verified: 2026-09-27
Revalidate by: 2027-03-26
Implementation gate: PASSED

## Decision Summary

- Purpose: give JARVIS one real, streamed text model call through
  `jarvis_application::model::ModelProvider`, so `ACC-013` (model fallback),
  `ACC-015` (adapter replacement), and the Milestone 2 exit gate stop being
  unexercised contracts (`BRN-003`).
- JARVIS boundary: an adapter behind the `ModelProvider` port in
  `crates/jarvis-infrastructure/src/model_providers/openai-compatible/`. It
  translates HTTP/SSE frames into normalized `ModelStreamEvent` values at its own
  boundary. No provider type crosses into `jarvis-domain` or
  `jarvis-application`.
- Target wire contract: **OpenAI Chat Completions streaming**, `POST /v1/chat/completions`
  with `stream: true`, over server-sent events. This is the surface that
  OpenAI-compatible servers actually implement, and it is the surface this
  repository already names as its compatibility edge
  (`docs/architecture/api-protocols.md` "OpenAI-Compatible Edge",
  `docs/architecture/voice-telephony.md`). The Responses API
  (`POST /v1/responses`) is a *different* documented contract and is
  **not** the target of this adapter; see "Explicitly unsupported".
- Reference deployment for the gated smoke test: an OpenAI-compatible server the
  operator configures by base URL, reached over **plaintext HTTP/1.1 on
  loopback** (`EndpointClass::Local`).
- Explicitly unsupported in this slice: any `https://` endpoint, the Responses
  API, Chat Completions non-streaming collection, multi-choice (`n > 1`),
  multimodal content parts, audio, logprobs, and provider-hosted tools.
- Kill switch: the adapter is a configured provider entry. Removing it, or
  removing its base URL, leaves the daemon with no provider for that model, which
  `select_route` already reports as `model.provider_no_route`; it is not a
  runtime toggle inside the adapter and it cannot disable the daemon.

## Official Sources

| Source | URL | Version/date | Accessed | What it establishes |
| --- | --- | --- | --- | --- |
| Root `llms.txt` | https://developers.openai.com/llms.txt | Current (undated index) | 2026-09-27 | Official ownership; routes to the API documentation set and confirms Markdown twins exist at `<page>.md` |
| API `llms.txt` | https://developers.openai.com/api/llms.txt | Current (undated index) | 2026-09-27 | Confirms the guides index and the reference index are the official indexes for the API |
| Reference `llms.txt` | https://developers.openai.com/api/reference/llms.txt | Current (undated index) | 2026-09-27 | Canonical slug list; locates the Chat Completions streaming-events page |
| API overview | https://developers.openai.com/api/reference/overview.md | REST `v1` | 2026-09-27 | Bearer auth header, `X-Client-Request-Id`, `x-request-id`, rate-limit headers, backwards-compatibility rules (new event types are additive) |
| Chat Completions streaming events | https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events.md | `CreateChatCompletionStreamResponse` | 2026-09-27 | The exact chunk schema: `chat.completion.chunk`, `choices[].delta`, `finish_reason` value set, `usage` only with `stream_options.include_usage` |
| Streaming guide | https://developers.openai.com/api/docs/guides/streaming-responses.md | Current | 2026-09-27 | `stream: true` selects SSE; semantic events; error-after-stream-start must not be automatically replayed |
| Chat Completions overview | https://developers.openai.com/api/reference/chat-completions/overview.md | Current | 2026-09-27 | Chat Completions is the message-list endpoint; Responses is recommended for new projects |
| Responses create reference | https://developers.openai.com/api/reference/resources/responses/methods/create.md | `POST /responses` | 2026-09-27 | Used only to record what the *other* contract is: `input`/`instructions`, `response.output_text.delta`, `store` defaults `true` |
| Data controls | https://developers.openai.com/api/docs/guides/your-data.md | Current | 2026-09-27 | Per-endpoint training use, 30-day abuse-monitoring retention, application-state retention, ZDR/MAM behavior, per-region domains |
| Error codes | https://developers.openai.com/api/docs/guides/error-codes.md | Current | 2026-09-27 | HTTP status plus `error.type`/`error.code` pairs; which conditions are retryable and which need operator action |
| Rate limits | https://developers.openai.com/api/docs/guides/rate-limits.md | Current | 2026-09-27 | `Retry-After` semantics, the `x-ratelimit-*` header set, and "don't automatically replay a request after consuming stream output" |
| Quickstart | https://developers.openai.com/api/docs/quickstart.md | Current | 2026-09-27 | `OPENAI_API_KEY` from the environment; key never embedded in client code |

Attempted `llms.txt` URLs that did not exist:

- `https://developers.openai.com/api/reference/resources/chat/llms.txt` — 404. The
  reference index at `/api/reference/llms.txt` is the working index for that subtree.
- `https://developers.openai.com/api/reference/chat/create.md` — 404. The correct
  create-page slug was not reachable; the streaming-events page was used instead and
  is the page that owns the frame schema.

## Version Matrix

| Component | JARVIS target | Documentation target | Compatibility status |
| --- | --- | --- | --- |
| API/spec | OpenAI REST `v1`, Chat Completions, `stream: true` | REST `v1`; chunk schema `CreateChatCompletionStreamResponse` | DOCUMENTED |
| SDK | None. The adapter speaks the wire contract directly. | Official SDKs exist for seven languages | DELIBERATE: an SDK would put provider types one dependency away from a JARVIS boundary |
| Wire protocol | HTTP/1.1 request/response plus SSE response body | SSE over HTTP | DOCUMENTED |
| Streaming transport | `text/event-stream` response body | `stream: true` → server-sent events | DOCUMENTED |
| Terminal sentinel | Not relied upon; see Falsifiable Claims `OC-C004` | Not stated on the streaming-events page | UNVERIFIED |

## Contract

### Authentication and Authorization

- Credential type: a bearer API key issued by the operator's provider.
- Credential placement: the `Authorization: Bearer <key>` request header. The key
  is **a header value, never a URL component**, so no key can appear in a base URL,
  in a log line that prints a URL, or in an operator-visible error message.
- Required scopes: the provider's key is the whole authorization boundary; there is
  no per-endpoint scope to request. JARVIS therefore treats "which provider and
  which model" as its own authorization decision (`BRN-010` policy,
  `ProviderInventory`) rather than delegating it.
- Refresh/rotation behavior: none documented for a static key. A rotation is an
  operator action; the adapter holds a resolved header value for the lifetime of a
  call only, and never caches it beyond that.
- Tenant or workspace binding: **none.** The provider has no workspace concept, so
  JARVIS must bind the credential to a workspace itself and must never let a
  caller-supplied workspace ID select which credential is used.
- Webhook signature verification: not applicable. This adapter makes outbound
  calls only.

### Transport and Lifecycle

- Endpoint: `{base_url}/chat/completions`. JARVIS stores **only the base URL**, so
  the path is not a configurable string and a typo cannot silently post to a
  different path.
- Transport: HTTP/1.1. See "The transport decision" below for why this is the
  plaintext loopback case and why `https` is out of scope.
- Connection lifecycle: one TCP connection per call; `Connection: close` is
  acceptable because calls are long-lived relative to setup, and a pooled client
  is not required to satisfy this contract.
- Negotiation/versioning: no handshake. The REST path carries `v1`, and the
  backwards-compatibility rules make additive changes legal — new properties and
  **new stream event types** may appear without a version bump. The parser
  therefore must ignore unknown properties and unknown chunk-level fields rather
  than fail, and must treat an unknown `finish_reason` as
  `FinishReason::Other { provider_value }` rather than flattening it to `Stop`.
- Streaming frame shape: each SSE frame is `data: {<chat.completion.chunk JSON>}`
  followed by a blank line. Documented chunk fields:
  `id`, `object: "chat.completion.chunk"`, `created`, `model`,
  `choices[] { index, delta, finish_reason, logprobs }`, and optional
  `usage`, `system_fingerprint`, `service_tier`, `obfuscation`.
  `delta` carries `role`, `content`, `refusal`, `tool_calls`, and the deprecated
  `function_call`.
- Terminal event: **the first chunk carrying a non-null `finish_reason`**, or
  end-of-body when none arrives. Both are documented facts about the chunk schema;
  the `[DONE]` sentinel that SDKs consume is not stated on the page this note
  cites, so it is accepted as an ignorable sentinel rather than treated as the
  terminal (see `OC-C004`).
- Ordering and duplication: chunks are ordered by generation. `choices[0].index`
  is present on every chunk and is the only documented ordering key; a chunk whose
  `index` is not the one JARVIS requested is refused rather than appended.
- Cancellation and timeout: cancellation is JARVIS-owned. The adapter must abandon
  reading and close the socket; a server that ignores the close cannot be waited
  on. The provider documents no cancellation verb for this endpoint, so
  cancellation is "stop reading" and the run's terminal is JARVIS's
  `call.cancelled`, never the provider's `finish_reason`.
- Reconnect/resume: **not supported.** No resume token exists for this endpoint, and
  the rate-limit guide states directly that a request must not be automatically
  replayed once stream output has been consumed. A retry is only legal before the
  first content byte, which is why the adapter must distinguish "failed to open or
  failed before output" from "failed after output".

### Data and Limits

- Request schema (the subset JARVIS sends): `model`, `messages[] {role, content}`,
  `stream: true`, and `stream_options: { include_usage: true }`.
- Response schema: the chunk described above. `usage` is `null` on every chunk
  except the last when `stream_options.include_usage` is requested.
- Pagination: not applicable to this endpoint.
- Payload and concurrency limits: the API overview requires total request headers
  under 64 KiB and custom header values at or under 60 KiB. The adapter must bound
  its own outbound header set and must not pass operator-supplied headers through
  unbounded. Per-model context and output token limits are properties of the model,
  not the protocol, so they belong in the capability inventory (`BRN-011`), not in
  this adapter's constants.
- Rate-limit headers and behavior: `Retry-After` (seconds, minimum wait),
  `x-ratelimit-limit-requests`, `x-ratelimit-limit-tokens`,
  `x-ratelimit-remaining-requests`, `x-ratelimit-remaining-tokens`,
  `x-ratelimit-reset-requests`, `x-ratelimit-reset-tokens`, plus project-scoped
  variants. Limits are enforced per organization and per project, **not per user**,
  so a JARVIS workspace sharing one key shares one budget.
- Retention and privacy: this is the finding that constrains `BRN-010`.
  For `/v1/chat/completions` the official table records **data used for training:
  No**, **abuse-monitoring retention: 30 days**, and **application-state retention:
  None** (with documented exceptions: audio outputs are stored for one hour, and
  images or files flagged by the CSAM classifier are retained for review). Abuse
  monitoring is **on by default and cannot be turned off from the client side**;
  excluding content requires provider approval for Zero Data Retention or Modified
  Abuse Monitoring on the organization. A JARVIS policy that claims
  `maximum_provider_retention: none_documented` against a cloud endpoint of this
  contract is therefore **false by default**, and the honest inventory value is a
  bounded 30-day abuse-monitoring window. A loopback-compatible server the operator
  runs is a different case: retention is the operator's own, and the adapter cannot
  attest to it either.
- Data residency: non-US regions use per-region domains (`eu.api.openai.com` and
  others) and generally require prior approval. A residency requirement is a
  routing constraint, not an adapter flag; the adapter must not accept a
  region-looking hostname as proof of residency.
- Cost assumptions: **none are recorded here.** Pricing is a per-model, per-token
  figure that changes independently of the protocol, and `Usage.estimated_cost_microunits`
  plus a pricing source belong to the capability inventory. This adapter reports
  provider token counts and sets `provider_reported: true`; it does not invent a
  cost.

### Errors and Retries

The transport signal is an HTTP status plus a JSON body carrying `error.message`,
`error.type`, `error.param`, and `error.code`. The mapping below is the one the
adapter must implement, using the documented pairs rather than the status alone —
the official guide explicitly warns that different conditions share a status.

| Condition | Provider signal | Retry? | JARVIS behavior |
| --- | --- | --- | --- |
| Invalid credentials | `401`, `invalid_api_key` / `invalid_request_error` | No | `ProviderError::Authentication` (`model.provider_authentication`); surface an actionable "re-authenticate" diagnostic and never echo the key |
| Insufficient scope or plan | `403`, `insufficient_quota` when the cause is billing | No | `ProviderError::Authentication`; a retry cannot change an exhausted balance and the guide says so explicitly |
| Rate limited | `429`, `rate_limit_error`; code `slow_down` for ramp-rate, `rate_limit_exceeded` for a filled budget | Conditional | `ProviderError::RateLimited { retry_after_ms }` parsed from `Retry-After`; retry only if no content byte was consumed |
| Credit/spend/quota exhausted | `429` with `credit_balance_exhausted`, `organization_spend_limit_exceeded`, `project_spend_limit_exceeded`, `organization_usage_limit_exceeded` | No | `ProviderError::Unavailable` is wrong here — this is an operator action. Map to `Authentication`-class non-retryable with the code retained, so the run does not burn its budget retrying a billing state |
| Timeout | No HTTP response; socket read exceeds the budget | Conditional | `ProviderError::Timeout`; retry only before any content byte, and only inside the run's remaining deadline |
| Provider unavailable | `500`, or `503` with `service_unavailable_error` / `server_is_overloaded` | Yes | `ProviderError::Unavailable`; honour `Retry-After`, then bounded exponential backoff with jitter |
| Invalid request | `400`, `invalid_request_error` (including the `service_tier` case) | No | `ProviderError::InvalidRequest`; a malformed request is a JARVIS defect and repeating it wastes budget |
| Content refused | `200` with a chunk whose `delta.refusal` is populated | No | Terminal `call.completed { finish_reason: Refusal, refused: true }`. This is the case that is easy to get wrong: the HTTP status is success, so a reader keyed only on the status records a refusal as a finished answer |
| Error after streaming began | `200`, then an error frame or an aborted body | No | `call.failed` with the mapped code. The guide forbids automatic replay here; the run reconciles durable state instead of re-calling |
| Malformed frame | `200`, unparseable `data:` payload | Conditional | `ProviderError::Malformed`; refuse to guess. A single unparseable frame after output must fail the call rather than silently truncate the answer |
| Unknown success status | any other 2xx | n/a | Accept only what the schema allows; an unexpected body is refused rather than echoed, matching the existing client rule |

## Security Analysis

- Trust boundaries: the provider response is **untrusted data**. Every string that
  reaches JARVIS is bounded (`bounded_provider_string`) and validated; nothing from
  a chunk is executed, resolved as a path, or used as an identifier.
- Prompt-injection exposure: model output already arrives as data everywhere else in
  JARVIS. This adapter adds no new sink, and it must not promote a chunk field —
  for example `model` or `system_fingerprint` — into a routing or policy decision,
  because a compromised endpoint could then choose its own route.
- Secret leakage paths: the `Authorization` header is the only place the key
  appears. The adapter must not render a request, a URL, or an error body that
  contains it. The existing writer-level redaction covers registered secret values;
  because the key travels as a header and not in a URL, the URL-shaped redaction
  patterns do not cover it, so the adapter's own `Debug` output must not carry the
  header value.
- SSRF or callback risks: the adapter dials an operator-configured base URL. It must
  **not** follow a URL supplied by a model, a chunk, or any caller, and it must
  resolve and classify the endpoint (`EndpointClass`) before dialling. A provider
  that returns a redirect must not be followed to a new host.
- Tool side effects: none. This adapter performs one outbound read-only call. It
  exposes no tool and executes no model-proposed action; tool calls appear as
  `tool.call.*` events and remain subject to the canonical tool pipeline.
- Required approvals: none at call time. The provider entry itself is operator
  configuration.
- Redaction rules: the key never appears in a log, an error, a diagnostic bundle, or
  a trace. Provider-supplied `error.message` text is treated as untrusted and is
  bounded before it is stored or shown; it is not a JARVIS error code.
- Sandbox or network policy: deny-by-default except the configured provider host.
  `https` is explicitly not implemented in this slice, so there is no TLS
  configuration to get wrong and no certificate-validation bypass to add later.
- Abuse cases: a hostile endpoint that streams unbounded text (the adapter must bound
  total and per-frame bytes and the delta count); a hostile endpoint that never
  sends a terminal (the deadline must fire); a hostile endpoint that reports
  `finish_reason: "stop"` and then keeps sending content (content after the terminal
  is ignored and counted, not appended); a hostile endpoint that reports usage
  numbers the run's budget then acts on (usage is only ever accepted against a
  ceiling, and an absent usage block leaves the budget `unverifiable` rather than
  assumed satisfied).

### The transport decision

The first adapter targets a **loopback, plaintext** OpenAI-compatible endpoint, and
this is a decision with a reason rather than a convenience:

- `EndpointClass` in `jarvis-domain` already separates `Local` from
  `ApprovedCloud`, and an OpenAI-compatible server such as a local inference
  runtime is `Local`. Dialling it needs no TLS.
- **The workspace has no TLS implementation at all.** A lockfile inspection on
  2026-09-27 found `rustls`, `ring`, `aws-lc-rs`, `webpki`, `native-tls`,
  `openssl`, `hyper-rustls`, and `tokio-rustls` **absent**; only `hyper` (as a
  transitive server component), `hyper-util`, `tower-http`, `http-body-util`, and
  `httparse` resolve. Reaching a cloud endpoint therefore requires a new reviewed
  dependency, which per `AGENTS.md` is its own evidence obligation and its own
  decision rather than a side effect of this adapter.
- The existing Foundation client already establishes the pattern and its rationale:
  a minimal HTTP/1.1 exchange rather than a general client stack, chosen because one
  known peer does not justify a full client and because the hand-rolled path makes
  it structurally hard to send a credential somewhere unintended
  (`rust-foundation.md`, "The Foundation client is a minimal HTTP/1.1 exchange…").

Choosing plaintext-loopback-first is therefore the honest boundary: it delivers a
real streamed provider call without smuggling an unreviewed TLS stack into the
adapter, and it keeps the cloud path as a *later, separately evidenced* step rather
than an implicit one.

## Normalization Map

| Provider concept | JARVIS concept | Conversion/loss |
| --- | --- | --- |
| HTTP 200 with `stream: true` | `call.started` | JARVIS emits this itself; the provider has no start frame. `model` is filled from the chunk's `model` when present |
| `choices[0].delta.role` | (no event) | Absorbed. A role-only first chunk is not an output item |
| `choices[0].delta.content` (string) | `output.text.delta` | `item_id` is JARVIS-generated, because this contract has no per-item identifier. One item per call |
| first content chunk | `output.item.added` | Synthesized once, so the item exists before its first delta |
| `choices[0].delta.refusal` | `output.text.delta` | Refusal text is surfaced as output text, and the refusal *verdict* is carried on the terminal as `refused: true` so no reader has to infer intent from text |
| `choices[0].finish_reason` | `FinishReason` on `call.completed` | `stop` → `Stop`, `length` → `Length`, `tool_calls`/`function_call` → `ToolCalls`, `content_filter` → `ContentFilter`; any other value → `Other { provider_value }` |
| populated `delta.refusal` at terminal | `FinishReason::Refusal` + `refused: true` | Lossless for the verdict; the refusal text is also delivered as output |
| `usage { prompt_tokens, completion_tokens, total_tokens, prompt_tokens_details.cached_tokens, completion_tokens_details.reasoning_tokens }` | `Usage { input_tokens, output_tokens, cached_input_tokens, reasoning_tokens, provider_reported: true }` | `total_tokens` is dropped: `Usage` has no total, and computing one from the parts would be a JARVIS assertion rather than a provider fact |
| `id`, `system_fingerprint`, `service_tier` | `ProviderMetadata::request_id` / `provider_finish_reason` | `id` maps to `request_id`. `system_fingerprint` and `service_tier` have no field and are **dropped**, not stuffed into a string |
| `obfuscation` | (ignored) | A padding field; ignoring it is required, and treating it as content would corrupt the answer |
| `choices` empty with a populated `usage` | `usage.updated` | The documented final chunk shape |
| `n > 1` | refused | `InvalidRequest`. JARVIS models one answer, so a second choice is a request JARVIS must not make rather than one it must model |
| `logprobs` | dropped | Not modelled and not requested |
| SSE `data: [DONE]` | ignored sentinel | Accepted and skipped; never the terminal |
| HTTP status + `error.type`/`error.code` | `ProviderError` | See the errors table. The provider's message is retained as bounded text, never as the JARVIS code |

Do not expose provider SDK types in JARVIS domain contracts.

## Falsifiable Claims

| ID | Claim | Label | Evidence | Check that could disprove it |
| --- | --- | --- | --- | --- |
| `OC-C001` | A refusal arrives as HTTP 200 with `delta.refusal` populated, so a status-only reader records a refusal as a finished answer | DOCUMENTED | Streaming-events `ChatCompletionStreamResponse.delta.refusal`; `FinishReason::Refusal` exists for exactly this reason | A captured refusal stream returns a non-2xx status, or a passing test still shows `finish_reason: Stop` with `refused: false` |
| `OC-C002` | `usage` is `null` on every chunk except the last, and only when `stream_options.include_usage` is true | DOCUMENTED | Streaming-events `usage` field description | A captured stream omits the final usage chunk after `include_usage: true`, or carries usage mid-stream |
| `OC-C003` | `choices` can be empty on the final chunk | DOCUMENTED | Streaming-events `choices` description | Every captured final chunk carries one choice, making the empty-choice branch dead and its test vacuous |
| `OC-C004` | The stream terminates at the first non-null `finish_reason` or at end-of-body, and `[DONE]` is an ignorable sentinel | UNVERIFIED for `[DONE]`; DOCUMENTED for `finish_reason` | Streaming-events page documents `finish_reason`; it does **not** document `[DONE]` | A captured stream ends **without** any non-null `finish_reason` (terminal would then be reachable only via `[DONE]`), or `[DONE]` never appears in any capture |
| `OC-C005` | New chunk properties and new stream event types may be added without a version bump, so the parser must ignore unknowns | DOCUMENTED | API overview, "Backwards-compatible API changes" | A capture contains a field whose presence breaks parsing, or a release note declares a breaking frame change |
| `OC-C006` | Abuse-monitoring retention is 30 days and is not client-side disablable, so `none_documented` retention is false for a cloud endpoint of this contract | DOCUMENTED | Data-controls per-endpoint table and prose | The published terms change, or a ZDR-approved project is used and the inventory reflects that project specifically |
| `OC-C007` | `finish_reason` has more values than the five named | DOCUMENTED | The page renders the type as five literals followed by "or 2 more" | A capture shows only the five named values and the docs stop saying "or more" |
| `OC-C008` | A `429` can mean a filled budget, an exhausted balance, or a ramp-rate limit, and only the last is retryable | DOCUMENTED | Error-codes and rate-limits guides; `Retry-After` "does not mean that quota, billing, or other errors that require user action can be resolved by retrying" | A `429` with `credit_balance_exhausted` succeeds on retry |
| `OC-C009` | A request must not be automatically replayed after stream output was consumed | DOCUMENTED | Rate-limits guide, "For streaming requests… don't automatically replay a request after consuming output" | Replaying a post-output failure produces a coherent, non-duplicated result |
| `OC-C010` | The workspace has no TLS implementation, so a cloud endpoint needs a new reviewed dependency | VERIFIED (lockfile inspection, 2026-09-27) | `Cargo.lock` contains no `rustls`, `native-tls`, `openssl`, `ring`, `webpki`, or `hyper-rustls` | A TLS crate appears in the lockfile without a dependency evidence change |

## Test Plan

### Deterministic Tests

- [x] Schema and normalization: every branch of the chunk-to-event mapping,
  including a role-only first chunk, empty `choices`, a mid-stream `usage`, an
  unknown `finish_reason`, an unknown extra field, and an `obfuscation` field.
- [x] Policy and scope: the adapter refuses a frame whose `choices[0].index` is not
  the requested one, and refuses `n > 1` at request construction.
- [x] Error mapping: a table-driven test over every row of the errors table,
  asserting the `ProviderError` variant *and* that the provider's raw
  `error.code` text never becomes a JARVIS code.
- [x] Retry/idempotency behavior: a failure before the first content byte is
  retryable; the same failure after a content byte is not. `Retry-After` is
  parsed into `retryable_ms` and an invalid value falls back to backoff.
- [x] Redaction: a seeded key value cannot appear in the adapter's `Debug`, in any
  rendered error, or in a captured diagnostic.
- [x] A refusal is not recorded as a clean completion.
- [x] Content after the terminal is ignored and counted.

### Contract Fixtures

- [ ] Real, safely redacted payload captured — **a synthetic fixture only proves
  the implementation agrees with itself, so this is required before the item is
  marked done.** Capture one stream from the operator-configured endpoint.
- [ ] Fixture provenance and capture date recorded beside the file.
- [ ] Official schema validation: the captured frames are checked against the
  documented `CreateChatCompletionStreamResponse` field set.
- [ ] Malformed and forward-compatible payloads covered: a truncated frame, a
  non-JSON `data:` line, an unknown field, an unknown `finish_reason`, and a
  `usage` block with a missing detail object.
- [ ] The `OC-C004` question is settled by the capture and the note is updated.

### Implemented (this slice)

The adapter exists at
`crates/jarvis-infrastructure/src/model_providers/openai_compatible/`, in three
layers that are separately testable:

- `sse.rs` reassembles frames from an arbitrarily chunked byte stream. Pure; 8
  tests, including a frame split across two reads, CRLF framing, a multi-line
  payload, the `[DONE]` sentinel, comments and non-`data` fields, and an oversized
  frame.
- `translate.rs` maps one parsed chunk onto normalized events. Pure; 15 tests,
  including the role-only first chunk, the empty-`choices` usage chunk, an
  unmodelled `finish_reason`, content after the terminal, a second choice, a
  mid-stream error, and the "unknown is not zero" rule on every counter.
- `mod.rs` owns the socket, the HTTP/1.1 exchange, chunked decoding, error mapping,
  and the credential. Its request builder refuses a model the adapter does not
  serve, a requested output schema, unsupported settings, an artifact reference, and
  more tools than the bound. 19 tests cover the constructor refusals, the status/code
  mapping, the body builder, and the stream pump against byte fixtures.

`tests/openai_compatible_stream.rs` drives all of it against a **real loopback
socket** with a hand-written server, because the two pure layers were already
covered by fixtures and what remained unproven was exactly the transport: the
exchange, the chunked decoding, the header construction, and the wiring from the
port to the normalized stream. Twelve tests cover a full streamed answer, a `401`,
cancellation, a truncated chunked body, a body with no terminal, a refusal, a
refused connection, a request the adapter cannot express, and a credential canary.

**Two real defects were found by that socket-level test, not by the unit tests.**

1. **The chunked decoder handed chunk framing to the SSE parser.** Its first version
   drained the payload and left a reader that delivered the whole remaining buffer,
   so the *next* decode read payload bytes as a hex size line. The symptom was
   `model.provider_malformed` against a perfectly healthy stream — a framing bug
   wearing a protocol error's clothes. Fixed by leaving the payload buffered and
   tracking a `payload_remaining` counter, so a chunk larger than the reader's output
   buffer is delivered across several reads instead of being dropped. **Verified by
   falsification**: re-introducing the drain fails three of the twelve socket tests
   with exactly that code, so the fix is not merely accompanied by a passing test.
2. **The lifetime shape of `ModelProvider::open` was only visible from outside the
   workspace.** `open` borrows the context, request, and cancellation scope for the
   stream's lifetime, which is correct — and it means a caller must own all three.
   The end-to-end test is what made that concrete, which is why it is written the way
   it is.

**One decision changed during implementation.** The refusal flag is **not** folded
into the finish reason here. The run controller's `completed_reason` already owns
that fold and upgrades a plain `Stop` and nothing else, so folding in the translator
too would have put one rule in two layers that could drift — and a second
implementation of "did this model decline" is the duplicate the architecture
forbids. The translator reports the provider's reason verbatim with the flag beside
it.

**Explicitly still unproven.** No capture from a real endpoint has been taken, so
every claim about a real provider's behaviour remains `DOCUMENTED` rather than
`OBSERVED`, and the `[DONE]` question stays open. The `[DONE]` sentinel is
recognized but not relied on: the stream terminates on the first non-null
`finish_reason`, so a provider that omits `[DONE]` is handled and one that sends
only `[DONE]` is not — which is exactly what a capture would settle.

### Composed into the daemon

The adapter is no longer a library the daemon could not reach. The daemon composes
its provider **once, at startup**, from the operator's configuration document, and
the composition has four properties the tests hold:

- **No provider table means the scripted provider** — the default for a fresh install
  and every test, so a profile naming no endpoint cannot silently reach the network.
- **A configured endpoint that cannot be composed is a startup refusal**, not a
  fallback. Falling back would let a typo route every run to a fixed acknowledgement
  while the operator saw successful runs, so the misconfiguration would be invisible.
  This is the property `a_configuration_the_adapter_refuses_stops_the_daemon_from_being_wired_to_it`
  and `an_unresolvable_credential_is_a_startup_refusal_not_a_fallback` exist to keep.
- **The credential is resolved once, at composition**, rather than per request: the
  value is held for the process's lifetime, is never written to a record, and is never
  rendered. For a long-lived daemon, composition is the last responsible moment.
- **The provider id and model names are validated** with the domain's own identifier
  rule, because both reach a persisted row and a routing decision.

The configuration schema version moved 1 → 2 for the added `[model.provider]` table, and
version 1 remains readable. The version moved because a document naming an endpoint is not
one a version-1 binary can read — that binary would report a *parse* failure when the
accurate diagnostic is "written by a newer JARVIS".

The end-to-end proof is `tests/daemon_provider_composition.rs`: a **real daemon** over a
**real socket**, pointed at a **local fake OpenAI-compatible server**, with a run created
through the **real control API**. It asserts what only the assembled product can show —
that the fake server *received* the request (naming the routed model, with the credential as
a header), and that the streamed answer is durable in the run's own events. Replacing the
composition with the scripted provider fails it by showing the fake server was never called,
so the test falsifies the wiring rather than describing it.

### Gated Live Tests

- [ ] Authentication and capability discovery: an unauthenticated call maps to
  `Authentication`, proving the mapping rather than assuming it.
- [ ] One representative streamed call. Opt-in via environment variable, skips
  clearly when unset, and **never** spends money on a cloud endpoint without an
  explicit test account.
- [ ] Cancellation/timeout: cancel mid-stream and assert `call.cancelled` with the
  run's terminal, not the provider's `finish_reason`.
- [ ] Rate-limit or simulated backoff: a local stub returning `429` with
  `Retry-After` proves the delay is honoured.
- [ ] Cleanup leaves no external resources: this adapter creates none.

## Operational Readiness

- [ ] Health probe: a cheap authenticated request, or reporting the last call's
  outcome. Not a synthetic model call on every probe.
- [ ] Safe diagnostics: endpoint class, model, last status, and last error code.
  Never a key, an authorization header, or a raw provider error body.
- [ ] Metrics and trace fields: call outcome, time to first byte, delta count,
  retry count.
- [x] Setup and reauthentication: **a config edit plus a daemon restart.** Composition
  happens at startup, so the plain form is a restart rather than a live reload — a live reload
  would let the provider identity change underneath a run that had already selected it.
- [x] Disable/unload: removing the `[model.provider]` table composes the scripted provider
  instead. A run then records a route naming the adapter's model, and the controller requires the
  provider to still serve it, so the run fails with `run.no_model_served` rather than silently
  answering from the scripted provider. **Removing a provider therefore surfaces as a run
  failure, not as a quiet switch of source** — which is the behavior the `BRN-003` note should
  state rather than the `model.provider_no_route` this item originally guessed.
- [x] Migration: no persisted provider payload is introduced by this adapter. The added
  configuration is non-secret and a version-1 document still loads.
- [x] Credential rotation: **a new key takes effect on the next daemon start**, not the next
  call, because the credential is resolved once at composition. This corrects the original
  "takes effect on the next call", which would have required a per-call environment read that the
  composition deliberately does not perform.
- [ ] Provider outage behavior: bounded backoff, then a terminal run failure with an
  actionable code; the daemon must not be affected.
- [ ] Operator runbook: what to check for `model.provider_authentication` versus
  `model.provider_rate_limited` versus an exhausted balance.

## Open Questions

- Does this contract emit a `[DONE]` sentinel, and is it ever the *only* terminal?
  (`OC-C004`). The adapter is designed not to depend on the answer, and the
  fixture capture settles it.
- Which specific OpenAI-compatible server will the gated smoke test target, and does
  that server conform? A third-party server is an external product in its own right,
  so if it deviates from this contract it needs its own note. This note scopes
  itself to the contract, and the capture is what proves the chosen server conforms.
- When the cloud path is enabled, which TLS stack is reviewed, and does the
  adapter then need to distinguish residency domains from ordinary hostnames?

## Change Log

| Date | Change | Evidence |
| --- | --- | --- |
| 2026-09-27 | Initial research; gate passed for the loopback, plaintext Chat Completions streaming contract. Records the per-endpoint retention finding that constrains the data policy, the refusal-is-HTTP-200 trap, the closed set of `finish_reason` values, and the absence of any TLS implementation in the workspace as the reason the first slice is loopback-only | `llms.txt` root/API/reference indexes, Chat Completions streaming-events page, streaming guide, API overview, data-controls guide, error-codes guide, rate-limits guide, and a lockfile inspection |
| 2026-09-27 | The adapter is implemented at `crates/jarvis-infrastructure/src/model_providers/openai_compatible/` (three layers: SSE reassembly, chunk translation, transport + status mapping) with a loopback socket-level contract test at `tests/openai_compatible_stream.rs`. Adds the "Implemented (this slice)" section and its two socket-only findings; records that the refusal flag is folded by the run controller rather than the translator. **No real capture yet**, so every behavioural claim stays `DOCUMENTED` and `OC-C004` stays open. The manifest's `implementation_paths` gained the underscore spellings, because a module directory cannot contain a hyphen and the hyphen-only globs matched no real path | The adapter and its tests; `Cargo.lock` (still no TLS crate); the changed-file documentation gate run against each new path |
| 2026-09-27 | The adapter is **composed into the daemon**. `jarvis_infrastructure::model_providers::resolve` maps a configuration document plus a secret resolver to the provider a daemon calls; `jarvisd` composes it before startup and refuses a configuration it cannot serve rather than falling back to the scripted provider. Config schema 1 → 2 for the optional `[model.provider]` table (version 1 still readable). Proven end to end at `tests/daemon_provider_composition.rs` — a real daemon, a real socket, a local fake server, and a run through the real control API — and confirmed by mutation. Adds the "Composed into the daemon" section; corrects the Operational Readiness items for setup, disable/unload, migration, and credential rotation, which the composition changes | The composition module and its tests; the daemon-level composition test; the config loader tests for both supported versions |
