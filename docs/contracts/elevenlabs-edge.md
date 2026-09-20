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
mapped explicitly by configured provider metadata, never name-only. Initial
categories may include:

- end call;
- language detection/switch;
- transfer to agent or number;
- skip turn;
- voicemail detection.

JARVIS decides whether the runtime/model may propose each call-control action.
ElevenLabs executes provider-owned call control only after the edge emits a valid
function call. General JARVIS tools remain in the canonical tool fabric.

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
- each supported system-tool call;
- `elevenlabs_extra_body`/session token placement actually observed;
- reasoning summary enabled/disabled without exposing hidden reasoning;
- malformed request and auth failure;
- interruption/disconnect/cancellation;
- slow first token and tool wait;
- unknown additive fields;
- provider version/config changes.

Run a gated live voice/telephone test before declaring this integration stable.