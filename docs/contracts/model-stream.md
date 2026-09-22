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

### Portable Settings

`settings` carries only controls whose meaning is the same across providers:
sampling temperature, nucleus mass, and a reasoning-effort hint. A knob specific
to one provider belongs in that adapter's namespaced extension.

Sampling values are expressed in **thousandths**, not as floats: `0.7` is `700`,
the temperature range is `0..=2000` (`0.0..=2.0`), and the nucleus range is
`1..=1000`. An out-of-range value is refused with `model.settings_invalid` rather
than clamped, because a clamped call runs with settings the caller did not choose
and the caller cannot tell a clamped value from an honored one.

The block is `deny_unknown_fields`, so a provider-only key placed inside the
portable block is a **parse failure** rather than an ignored field. That is the
same rule as the absent extension map, stated from the other direction: a
provider value must not be readable as a portable one either.

An absent `settings` and an empty one are the same fact on the wire — both omit
the block.

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

### Implemented evidence: the provider port and scripted provider (`BRN-002`)

`jarvis_application::model` owns the `ModelProvider` port and the deterministic
`ScriptedProvider` that makes the contract assertable without a network, a
credential, or a paid call. The port is narrower than a provider SDK: an adapter
maps its own protocol into normalized events and its transport failures into a
`ProviderError`, so nothing provider-specific reaches the application layer —
which is what lets `ACC-015` replace one adapter without changing the run or
conversation schema. The stream the port returns is the `ModelStream` **trait**,
not a struct, so an adapter in another crate can implement it.

| Rule here | Enforced by | Falsified by |
| --- | --- | --- |
| frames are numbered monotonically, starting at 1 | `FrameStamper` assigns the sequence, so a script cannot produce a non-monotonic stream | `the_first_frame_is_call_started_and_the_sequence_starts_at_one` |
| a script produces the same frames twice | identifiers come from the injected `IdGenerator`, never a raw UUID call | `a_scripted_stream_is_reproducible_frame_for_frame` |
| the port's output is accepted by the domain unchanged | the same frames are fed to `ModelStreamState` | `a_normalized_stream_satisfies_the_domain_state_machine` |
| a disconnect is interrupted, not completed | `interrupt()` drops terminal steps | `a_script_without_a_terminal_is_interrupted_not_completed` |
| cancellation ends the stream `call.cancelled` | the scope is observed before each frame | `cancellation_closes_the_stream_with_a_terminal_event_not_a_silent_stop` |
| cancellation is observed *while waiting* | `AdapterStream` selects on the scope and the channel | `an_adapter_stream_reports_cancellation_while_waiting_for_a_frame` |
| a closed transport is interrupted, not completed | `AdapterStream` returns `None` when the channel ends with no terminal | `an_adapter_stream_delivers_frames_then_ends_without_a_terminal` |
| a provider's own terminal passes through once | the wrapper produces no second terminal | `an_adapter_stream_passes_an_adapters_own_terminal_through_once` |
| a cancelled call is refused, not answered empty | `open` returns `ProviderError::Cancelled` | `a_cancelled_scope_refuses_to_open_rather_than_returning_an_empty_stream` |
| late frames reach the state machine | the transport does not swallow frames after a terminal | `frames_after_a_terminal_are_still_delivered_to_the_state_machine` |
| a replayed sequence and a foreign call are refused | `ScriptStep::Raw` reproduces a frame the provider itself could not produce | `a_scripted_duplicate_sequence_is_refused_by_the_state_machine`, `a_scripted_frame_for_another_call_is_refused_by_the_state_machine` |
| the trusted request identity reaches the adapter | the start frame's provider metadata echoes the server-derived request id | `the_trusted_request_identity_reaches_the_adapter` |
| a failure is retryable in exactly one place | `ProviderError::retryable` is the only decider | `only_transient_failures_are_retryable` |
| an adapter outside this crate can implement the port | the returned stream is a trait, and the test defines one without module internals | `an_adapter_can_implement_the_port_without_this_modules_internals` |

Two of those tests found defects while they were being written, which is the
reason the negative cases are scripted rather than reasoned about:

1. A `Raw` step had its **call rewritten** to the stream's call, so the
   contract's "a frame for another call is refused" case was unproducible while
   the code appeared to support it.
2. The stream closed its cancellation path as soon as the script merely
   *contained* a terminal, so a caller cancelling while the first frame was still
   queued received the script's `call.completed` instead of `call.cancelled` —
   a cancellation recorded as success. The phase now closes when a terminal is
   handed out, not when one is queued.

A third defect was a **design** one the tests could not have caught, because every
test used the one provider that happens to live in the same crate as the port: the
port originally returned a concrete stream struct with a private frame buffer, so
although `ModelProvider` read as a general trait, no adapter in another crate
could implement it — `BRN-003` would have had to add its own constructor inside
this crate. The port now returns the **`ModelStream` trait**, and the shared
implementation an adapter should use is:

- `AdapterStream` — an adapter sends normalized frames into a bounded channel and
  hands the receiver over. It observes cancellation **while waiting** for a frame
  (an adapter that checked only between frames would hang a cancelled call whose
  provider had gone quiet) and produces **exactly one** terminal; a channel that
  closes with no terminal and no cancellation ends the stream with `None`, which
  the state machine records as *interrupted* — the accurate answer for a transport
  that died, never a fabricated completion.
- `BoxedModelStream`/`OpenResult`/`NextEventFuture` — the aliases that keep that
  signature readable.

`an_adapter_can_implement_the_port_without_this_modules_internals` is the test
that would have caught it: it defines a provider and a stream outside the module
using only the public surface. **A port is only as general as the crates that can
implement it**, and a single in-crate implementor hides the difference.

`ProviderError` maps transport outcomes to JARVIS codes (`model.provider_*`) and
deliberately keeps a provider **refusal** apart from an **invalid request**: a
refusal is a decision that repeating cannot change, while a malformed request is
a defect in JARVIS, and the two need different operator responses.

The scripted provider is also the only way the *unimplemented* half of the
contract is exercised today. It can produce an unfinished tool call, which proves
that `ModelStreamState::finish` refuses to close a stream while a tool call is
open and that the argument string never becomes executable — the first half of
the tool-fabric boundary, assertable before the tool fabric exists.