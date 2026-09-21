# ElevenLabs OpenAI-Compatible Edge Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT
Evidence date: 2026-09-20

## Scope

This edge lets ElevenLabs provide realtime audio and telephony while JARVIS owns
the agent turn. It is not a general OpenAI proxy and does not promise every field
or endpoint in OpenAI's API.

Implement exact schemas only after captured current ElevenLabs requests and
official documentation are recorded in the integration evidence note.

## Endpoints

Preferred:

```text
POST /v1/responses
```

Compatibility fallback:

```text
POST /v1/chat/completions
```

Both require TLS in remote use, an ElevenLabs-specific service credential, and a
short-lived opaque JARVIS voice session binding. Do not accept arbitrary model
proxy traffic on these routes.

## Session Binding

The adapter extracts a signed/opaque token from the currently verified provider-
supported location. The token binds provider client, agent/config, call/session,
candidate user/workspace, scopes, expiry, nonce, and direction.

Fields such as `user_id`, dynamic variables, `elevenlabs_extra_body`, caller ID,
or model ID are untrusted until matched to this server-side binding.

## Responses SSE

Current official ElevenLabs docs require SSE and identify these minimum events:

```text
event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"..."}

event: response.completed
data: {"type":"response.completed","response":{"id":"...","status":"completed"}}

data: [DONE]
```

The exact accepted request/tool/event schema is pinned by contract fixtures. The
edge emits one completion or error sequence and then ends. JARVIS public activity
events are translated; they are not sent raw.

## Chat Completions SSE

Current official ElevenLabs docs require chunks in this shape:

```text
data: {<OpenAI-compatible chat completion chunk JSON>}

data: [DONE]
```

Each chunk uses valid UTF-8 JSON and blank-line SSE framing. Tool/function calls
use the currently documented OpenAI-compatible streaming format. Arguments are
not executable until complete and validated.

## System Tools

ElevenLabs can include call-control tools in the request. Supported tools are
mapped explicitly by configured provider metadata, never name-only. The currently
documented set, with the parameters each call carries:

| Tool | Parameters | JARVIS treatment |
| --- | --- | --- |
| `end_call` | `reason` (required), `message` (optional) | Call end intent |
| `language_detection` | `reason` (required), `language` (required) | Language-switch observation; language must be in the configured list |
| `transfer_to_agent` | `reason` (optional), `agent_number` (required, **zero-indexed**) | Transfer to a configured agent; an out-of-range index is a denial, not a lookup |
| `transfer_to_number` | `reason` (optional), `transfer_number` (required), `client_message` (required), `agent_message` (required) | Transfer to a human |
| `skip_turn` | `reason` (optional) | Hold/continue decision |
| `voicemail_detection` | `reason` (required) | Voicemail evidence |

`transfer_to_number.agent_message` is **model-authored free text delivered to a
human** receiving the transfer. It is a context-leak path: it must be treated as
untrusted, must not carry private JARVIS context, secrets, or memory excerpts, and
is redacted or replaced before delivery if policy requires it.

JARVIS decides whether the runtime/model may propose each call-control action.
ElevenLabs executes provider-owned call control only after the edge emits a valid
function call. General JARVIS tools remain in the canonical tool fabric.

## Slow-Model Progress

The provider documents a **sanctioned** progress mechanism for endpoints that need
longer than usual: return an initial chunk whose content ends with an ellipsis and
a **trailing space**. The trailing space is load-bearing — without it the next
content is appended to the ellipsis and the audio is distorted. The edge may use
this when a turn involves real work.

Two limits apply. It is progress, not a result: it must never imply that an action
succeeded. And it does not replace a durable outcome — a tool that only drafted or
proposed something is reported as such, and the provider-reported result is a
claim that the durable tool outcome overrides.

## Errors and Disconnects

- Authentication/session failures return no private context.
- Invalid requests return a safe compatibility error before starting a run.
- Provider/client disconnect does not imply a completed JARVIS turn.
- JARVIS cancellation propagates to active model/tools and marks call activity.
- A tool approval that cannot complete within voice policy becomes an explicit
  follow-up/step-up response, not an indefinitely open stream.
- Partial generated text is not durable final output unless the run completes.

## Limits

The edge has independent bounds for request bytes, context, tools, output tokens,
time to first event, turn duration, concurrent calls, per-client rate, tool wait,
and total call budget. Exceeding a limit produces a safe spoken-compatible error
or handoff/end behavior according to call policy.

## Compatibility Tests

Maintain redacted captured fixtures for:

- Responses text stream and completion;
- Chat Completions text stream and `[DONE]`;
- each supported system-tool call, including `transfer_to_number` with its
  `agent_message`, and an out-of-range `agent_number`;
- `elevenlabs_extra_body`/session token placement actually observed;
- reasoning summary enabled/disabled without exposing hidden reasoning;
- the ellipsis-plus-trailing-space progress chunk, and its absence;
- malformed request and auth failure;
- interruption/disconnect/cancellation;
- slow first token and tool wait;
- unknown additive fields;
- provider version/config changes.

Run a gated live voice/telephone test before declaring this integration stable.