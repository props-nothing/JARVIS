# Normalized Model Stream Contract

Status: ACCEPTED
Contract version: 0.1.0
Lifecycle: DRAFT

## Request

```json
{
  "call_id": "019...",
  "run_id": "019...",
  "route_requirements": {
    "modalities": ["text"],
    "tools": true,
    "structured_output": false,
    "local_only": false
  },
  "input": [],
  "tools": [],
  "output_schema": null,
  "settings": {},
  "limits": {
    "deadline": "2026-09-20T12:00:00Z",
    "max_output_tokens": 2048,
    "max_cost_microunits": null
  }
}
```

Principal/workspace/security context is trusted application context and not an
arbitrary request body field.

## Input Items

Supported normalized item kinds begin with:

- system policy reference;
- user/assistant message content blocks;
- tool call and paired tool result;
- artifact/document/image/audio reference;
- concise reasoning summary where user-visible and permitted;
- provider continuation reference inside adapter-specific metadata.

Tool call/result pairs retain stable canonical call IDs. Compaction cannot leave
orphaned tool results.

## Stream Envelope

```json
{
  "call_id": "019...",
  "event_id": "019...",
  "sequence": 3,
  "type": "output.text.delta",
  "payload": {"item_id": "out-1", "delta": "Hello"},
  "provider_metadata": {"request_id": "safe-provider-id"}
}
```

Sequence increases monotonically. Adapters emit exactly one terminal event:
`call.completed`, `call.failed`, or `call.cancelled`.

## Event Types

```text
call.started
output.item.added
output.text.delta
output.item.completed
tool.call.added
tool.call.arguments.delta
tool.call.completed
reasoning.summary.delta
usage.updated
provider.warning
call.completed
call.failed
call.cancelled
```

Argument deltas are not executable. The completed tool call must parse and pass
schema validation before becoming a tool intent.

## Completion

`call.completed` includes finish reason, normalized output item references,
final usage if available, provider request/continuation references, and safety/
refusal metadata. A stream ending without a terminal event is an interrupted
call, not success.

## Usage

Usage tracks provider-reported and estimated values separately:

```json
{
  "input_tokens": 0,
  "output_tokens": 0,
  "cached_input_tokens": 0,
  "reasoning_tokens": 0,
  "provider_reported": true,
  "estimated_cost_microunits": null,
  "currency": "USD"
}
```

Unknown is not zero. Missing usage fields remain `null`/absent according to the
generated schema.

## Cancellation and Retry

Cancellation is cooperative with a hard adapter deadline. Late provider frames
after local terminal state are ignored and safely counted. A retry creates a new
attempt under the same logical model call only when request ownership and
provider acceptance/idempotency semantics make that safe.

## Provider Extensions

Provider-only behavior lives in a namespaced extension validated by that adapter
and excluded from generic clients unless explicitly exposed. Hard capability
requirements never degrade into ignored extension fields.

## Tests

- arbitrary chunk boundaries and Unicode;
- interleaved parallel tool calls;
- incomplete/invalid JSON arguments;
- usage before/after output;
- refusal and safety events;
- disconnect without terminal;
- cancellation race and late frames;
- duplicate/provider-replayed events;
- structured output mismatch;
- redaction of provider error and request data.

### Implemented evidence (Milestone 2)

`jarvis_domain::model::stream` implements this contract as types, so the rules
above are enforced rather than restated, and the test names mirror the list:

| Rule here | Enforced by | Falsified by |
| --- | --- | --- |
| sequence increases monotonically | `ModelStreamState::accept` refuses `<` or `==` | a duplicate and a reordered sequence each return `jarvis.stream_sequence_not_monotonic` |
| exactly one terminal event | a second terminal arrives after the terminal state and is ignored, not applied | `late_frames_after_the_terminal_state_are_ignored_and_counted` |
| late frames are ignored and counted | `StreamAdmission::IgnoredAfterTerminal` plus a counter | the same test asserts the terminal reason does **not** change |
| a stream without a terminal is interrupted | `StreamOutcome::Interrupted`, and `is_terminal()` is the only success predicate | `a_stream_without_a_terminal_event_is_interrupted_not_successful` |
| argument deltas are not executable | `ToolArguments::executable_raw` answers `None` while streaming | `a_streaming_tool_call_exposes_no_executable_arguments` |
| the completion matches the assembled deltas | `ModelStreamState::accept` compares them | `argument_deltas_are_assembled_and_the_completion_must_match` |
| unknown usage is not zero | every `Usage` counter is `Option` | `usage_distinguishes_unreported_from_zero` asserts absent stays absent on the wire |
| a tool result is never orphaned | `InputItems::new` **and** its `Deserialize` | `a_tool_result_that_precedes_its_call_is_refused`, `a_deserialized_item_list_is_held_to_the_same_pairing_rule` |

Two deliberate omissions are visible in the types. A requested output schema is
carried as bounded `JsonText` instead of a parsed value, because schema subset
validation belongs to the tool fabric, and there is **no** provider extension map
on the portable request, because this contract makes extensions adapter-owned and
namespaced. A test asserts the serialized request contains no provider-owned
field, which is what stops a provider value from being read as a portable one.

An unknown provider finish reason is preserved with its raw value rather than
mapped to `stop`, so a new provider reason is visible to an operator instead of
making an unknown terminal look like a clean one.