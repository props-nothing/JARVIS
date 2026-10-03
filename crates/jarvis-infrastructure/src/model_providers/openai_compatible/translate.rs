//! Translation from OpenAI-compatible Chat Completions frames into normalized events.
//!
//! This module is **pure**: it turns one decoded frame into zero or more
//! [`ModelStreamEventKind`] values and holds no socket, no clock, and no credential. That split is
//! deliberate, because the translation is where every provider-shaped trap lives and a translation
//! that could only be exercised through a live connection would be the least tested code in the
//! adapter.
//!
//! The contract this translates is documented in
//! `docs/research/integrations/openai-compatible-model.md`, and three of its facts are easy to get
//! wrong in a way that produces a *plausible* wrong result:
//!
//! 1. **A refusal arrives as HTTP 200** with `choices[0].delta.refusal` populated. A translator
//!    keyed on the HTTP status records a refusal as a finished answer, which is why the refusal is
//!    carried as a verdict on the terminal rather than inferred from the text.
//! 2. **`finish_reason` has more values than the five named** — the official schema renders five
//!    literals followed by "or 2 more" — so an unmodelled value is retained via
//!    [`FinishReason::other`] rather than flattened to `Stop`, which would make an unknown terminal
//!    look like a clean one.
//! 3. **The terminal is the first non-null `finish_reason`, or end-of-body.** The `[DONE]` sentinel
//!    that SDKs consume does **not** appear in the page this adapter's evidence note cites (recorded
//!    there as `OC-C004`, UNVERIFIED), so it is accepted and skipped as a sentinel and never relied
//!    upon as the terminal.
//!
//! Unknown fields and unknown chunk types are ignored rather than refused, because the provider's
//! own compatibility rules state that new properties and **new stream event types** may be added
//! without a version bump (`OC-C005`). Ignoring is the forward-compatible reading, and it is
//! different from ignoring a *malformed* frame, which is refused.

use jarvis_domain::model::stream::{FinishReason, ModelStreamEventKind, Usage};

/// What one decoded frame means to the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translated {
    /// The frame produced these events, in order.
    Events(Vec<ModelStreamEventKind>),
    /// The frame is understood and contributes nothing (a role-only first chunk, the `[DONE]`
    /// sentinel, or a chunk whose only content is padding).
    Ignored,
    /// The frame cannot be interpreted.
    Malformed,
}

/// Accumulates the state one stream's translation needs.
///
/// Stateful because three of the protocol's facts are only visible across frames: the output item is
/// synthesized once rather than per delta, usage arrives on the final chunk, and the terminal is
/// emitted at most once however many frames follow it.
#[derive(Debug)]
pub struct ChunkTranslator {
    /// The output item identifier, minted on the first content delta.
    ///
    /// This contract has no per-item identifier — every chunk carries only the completion's `id` —
    /// so JARVIS mints one and reports it on `output.item.added` before the first delta. Without the
    /// added event a client would receive deltas for an item it was never told about.
    item_id: Option<String>,
    /// How many text or refusal deltas have been produced.
    delta_count: u32,
    /// Whether the provider flagged a refusal during this stream.
    refused: bool,
    /// Whether a terminal has already been produced.
    ///
    /// Content arriving after the terminal is counted and dropped rather than appended. The
    /// alternative — appending it — would let a hostile or buggy endpoint extend an answer after
    /// declaring it finished, and the run's stored output would not match the terminal it recorded.
    terminal_emitted: bool,
    /// How many deltas arrived after the terminal.
    late_deltas_ignored: u64,
    /// How many chunks carried model-internal reasoning text, which was deliberately not translated.
    ///
    /// A **counter and not a discard**, because dropping this silently is the one thing it must not be.
    /// The contract forbids persisting hidden reasoning, so the adapter cannot emit it — but a field
    /// that is simply *never read* is indistinguishable from a field `translate` forgot, and the two
    /// have opposite remedies. Counting it makes the decision **observable**: an operator debugging a
    /// truncated-looking answer can see that the provider spent most of the stream on thinking, and a
    /// future provider that renamed the field would show a count that stopped moving.
    ///
    /// Found by capturing a real stream from an operator-configured endpoint rather than by reading
    /// the documented schema: `delta.reasoning` is **absent from the Chat Completions documented field
    /// set** the evidence note's mapping table was built from, and present on most frames of every
    /// stream this endpoint sends.
    reasoning_chunks_ignored: u64,
    /// The provider's model identifier, reported on the first chunk that carries one.
    model: Option<String>,
    /// The tool calls this stream has announced, keyed by the provider's index for each.
    ///
    /// Keyed by index because that is how this protocol identifies a tool call **within a stream**:
    /// a fragment carries an `index` and, on its first chunk only, an `id`. Later argument fragments
    /// for the same call repeat the index and omit both the id and the name, so the index is the only
    /// value that ties a fragment to the call it continues. The provider's `id` is kept per index and
    /// used as the **canonical** call identifier, because it is the value a later transcript
    /// continuation must echo.
    tool_calls: std::collections::BTreeMap<u64, AnnouncedToolCall>,
    /// The names the request offered, so a provider-safe name is reported as the canonical one.
    tool_names: super::tool_names::ToolNames,
}

/// One tool call a provider has announced, and how much of its argument text has arrived.
#[derive(Debug, Clone)]
struct AnnouncedToolCall {
    /// The provider's own call id, adopted verbatim as the canonical identifier.
    call_id: String,
    /// The tool the model wants to call.
    name: String,
    /// The argument text accumulated so far.
    ///
    /// Accumulated because the protocol streams it in fragments, and `tool.call.completed` carries
    /// the **complete raw arguments** — the value the fabric will parse and validate. Sending only
    /// the last fragment would hand the validator a JSON fragment, which is not JSON.
    arguments: String,
    /// Whether `tool.call.added` has been emitted for this call.
    ///
    /// Separate from "a name is present", because the protocol emits the name and the id together on
    /// the first fragment for an index — so a fragment that is *only* arguments (which is every
    /// fragment after the first) must not re-announce the call.
    announced: bool,
}

impl ChunkTranslator {
    /// Creates a translator for one stream.
    #[must_use]
    pub fn new() -> Self {
        Self {
            item_id: None,
            delta_count: 0,
            refused: false,
            terminal_emitted: false,
            late_deltas_ignored: 0,
            reasoning_chunks_ignored: 0,
            model: None,
            tool_calls: std::collections::BTreeMap::new(),
            tool_names: super::tool_names::ToolNames::default(),
        }
    }

    /// Reads tool names against `names`, the table the request was built with.
    #[must_use]
    pub fn with_tool_names(mut self, names: super::tool_names::ToolNames) -> Self {
        self.tool_names = names;
        self
    }

    /// Returns how many output deltas were translated.
    ///
    /// **`#[cfg(test)]`, and the doc used to say "used by the adapter".** It is not: nothing outside
    /// this module reads any of this translator's counters, so the claim was a doc comment describing a
    /// caller that does not exist — the same falsifiable shape as a `pub fn` whose only reference is
    /// its own definition. The honest options are to wire them somewhere real or to say they are test
    /// observations; wiring a counter into an observability surface is a feature this round is not,
    /// and leaving them `pub` with a false claim would be worse than either.
    ///
    /// The measurement `BRN-011` aggregates is real and is *not* this counter: `model_calls` records
    /// `output_delta_count` from the events the controller consumed, which is the value a resumed or
    /// re-read run still has.
    /// Test-observation only; see [`Self::delta_count`].
    #[cfg(test)]
    #[must_use]
    pub const fn delta_count(&self) -> u32 {
        self.delta_count
    }

    /// Returns how many deltas arrived after the terminal and were discarded.
    ///
    /// Test-observation only; see [`Self::delta_count`].
    #[cfg(test)]
    #[must_use]
    pub const fn late_deltas_ignored(&self) -> u64 {
        self.late_deltas_ignored
    }

    /// Returns how many chunks carried model-internal reasoning text.
    ///
    /// A **deliberate** drop rather than a gap: the contract's memory rules forbid persisting hidden
    /// reasoning, so this text may not become an output event. Counting it is what makes the decision
    /// distinguishable from a field the translator forgot, and the count is asserted by the tests
    /// below — which is the honest description of what it is for, rather than implying an operator can
    /// read it somewhere it is not exposed.
    ///
    /// Test-observation only; see [`Self::delta_count`].
    #[cfg(test)]
    #[must_use]
    pub const fn reasoning_chunks_ignored(&self) -> u64 {
        self.reasoning_chunks_ignored
    }

    /// Translates one parsed chunk document.
    ///
    /// `chunk` is the decoded JSON object for one `data:` line, or `None` for the `[DONE]` sentinel.
    /// The parse itself happens in the caller, so this function is a pure mapping and its tests need
    /// no text protocol.
    #[must_use]
    pub fn translate(&mut self, chunk: Option<&serde_json::Value>) -> Translated {
        let Some(chunk) = chunk else {
            // The `[DONE]` sentinel. It is **not** treated as the terminal: the documented terminal
            // is the first non-null `finish_reason`, and depending on a sentinel the cited page does
            // not describe would make this adapter fail against a server that omits it while every
            // test still passed.
            return Translated::Ignored;
        };

        // A chunk that is not an object cannot mean anything. Refusing rather than ignoring is the
        // deliberate half: a malformed frame after output must fail the call rather than silently
        // truncate the answer.
        if !chunk.is_object() {
            return Translated::Malformed;
        }

        // `model` is recorded when present and never used for routing. A compromised endpoint could
        // otherwise choose its own route by naming one, which is the promotion the security section
        // of the evidence note forbids.
        if let Some(model) = chunk.get("model").and_then(serde_json::Value::as_str)
            && self.model.is_none()
            && !model.is_empty()
        {
            self.model = Some(model.to_owned());
        }

        // An error delivered mid-stream. `error` is a documented member of the Chat Completions
        // error shape and can arrive after a `200`, so it terminates the call rather than being
        // ignored.
        if let Some(error) = chunk.get("error") {
            return self.terminal(finish_reason_from_error(error), None, false);
        }

        let mut events = Vec::new();

        // `choices` may be absent or empty. An empty list **with** a usage block is the documented
        // final chunk when `stream_options.include_usage` is requested, so usage is read from the
        // chunk itself rather than from a choice.
        if let Some(usage) = chunk.get("usage").filter(|value| !value.is_null()) {
            events.push(ModelStreamEventKind::UsageUpdated {
                usage: usage_from(usage),
            });
        }

        let Some(choices) = chunk.get("choices").and_then(serde_json::Value::as_array) else {
            // No `choices` at all: a usage-only chunk is understood, anything else is not. A chunk
            // with neither choices nor usage carries nothing this adapter can act on, and refusing
            // it would break forward compatibility for a new chunk kind the provider may add.
            return if events.is_empty() {
                Translated::Ignored
            } else {
                Translated::Events(events)
            };
        };

        for choice in choices {
            // **Only index 0 is accepted.** JARVIS models one answer, and the adapter refuses `n > 1`
            // when building the request, so a choice on any other index is a frame for an answer
            // this call did not ask for. Appending it would interleave two answers into one item.
            //
            // An absent index is tolerated because a minimal compatible server may omit it, and with
            // one choice there is no ambiguity to resolve — which is why both cases fall through to
            // the same handling rather than being spelled as two arms that happen to agree.
            if let Some(index) = choice.get("index").and_then(serde_json::Value::as_i64)
                && index != 0
            {
                return Translated::Malformed;
            }

            if let Some(delta) = choice.get("delta").filter(|value| value.is_object()) {
                // **Tool calls are translated, and that reversed an earlier decision.** This adapter
                // used to recognize `tool_calls` and emit nothing, reasoning that the tool fabric does
                // not exist so a tool call must not be "proposed". That was wrong in the dangerous
                // direction, and it took building one to see it: the controller *does* have a typed,
                // terminal outcome for a tool intent and **no other way to reach it**, so dropping the
                // event did not prevent a proposal — it removed the only thing that made the refusal
                // accurate. A real model that asked to call a tool produced a stream with no delta and
                // no tool event, so the run could reach `completed` with an empty answer. A silent
                // success on a dropped intent is strictly worse than a typed refusal, because the model
                // asked to *do* something and JARVIS reported that it finished.
                //
                // Emitting the event is also not a grant: `tool.call.added` is a proposal the
                // deterministic layer judges, which is what "discovery never grants execution" means.
                // The argument text is carried through unparsed for the same reason — the adapter
                // transports, the fabric validates.
                self.push_tool_calls(&mut events, delta);
                // **Model-internal reasoning is counted and never translated.** The normalized stream
                // *does* have a home for reasoning — `ModelStreamEventKind::ReasoningSummaryDelta` —
                // and this is deliberately not sent there, which is the distinction that makes the
                // decision a decision rather than a gap. That variant carries a **user-visible
                // summary** the contract permits; `delta.reasoning` here is the raw thinking text,
                // and the memory rules forbid persisting hidden reasoning. Mapping one onto the other
                // would be the violation, with the event type making it look legitimate.
                //
                // It is a real provider field rather than a hypothetical one: a captured stream from
                // an operator-configured endpoint carried `delta.reasoning` on most of its frames and
                // carried the answer itself on one, and the field is **absent from the documented
                // Chat Completions field set** this adapter's mapping table was built from. So the
                // capture, not the schema, is what established its existence.
                //
                // Counting it is what stops the drop from being an omission: a field nobody reads is
                // indistinguishable from a field the translator forgot, and the two have opposite
                // remedies.
                if delta
                    .get("reasoning")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| !text.is_empty())
                {
                    self.reasoning_chunks_ignored += 1;
                }
                if let Some(text) = delta.get("content").and_then(serde_json::Value::as_str)
                    && !text.is_empty()
                {
                    self.push_delta(&mut events, text);
                }
                // A refusal's text is surfaced as output *and* remembered as a verdict. The text
                // alone would be indistinguishable from a normal answer, which is exactly the
                // failure `FinishReason::Refusal` exists for.
                if let Some(refusal) = delta.get("refusal").and_then(serde_json::Value::as_str) {
                    self.refused = true;
                    if !refusal.is_empty() {
                        self.push_delta(&mut events, refusal);
                    }
                }
                // `role` is recognized and not translated: the normalized stream has no role event,
                // and an assistant role adds nothing a client acts on.
                // `function_call` is the legacy singular spelling of `tool_calls`. It is **not**
                // translated, and the asymmetry is deliberate: it appears in the same position in the
                // same shape, so a caller reading only "a tool call arrived" cannot tell which
                // grammar the endpoint speaks, and the legacy form carries a bare `name`/`arguments`
                // pair with no `index` or `id` — so it cannot supply the canonical call identifier
                // that a continuation needs. Translating it would emit a tool call with a
                // synthesized id that no later request could reference. It is recorded here as a
                // known limitation rather than silently half-handled.
            }

            if let Some(reason) = choice.get("finish_reason").filter(|value| !value.is_null()) {
                let usage = chunk
                    .get("usage")
                    .filter(|value| !value.is_null())
                    .map(usage_from);
                // The provider's reason is reported **unchanged** and the refusal flag travels
                // beside it. Resolving the two into one verdict belongs to the run controller's
                // `completed_reason`, which is the single owner of that fold and already upgrades a
                // plain `Stop` for a flagged refusal and nothing else. Folding here as well would
                // put the rule in two layers that could drift, and a second implementation of "did
                // this model decline" is exactly the kind of duplicate the architecture forbids.
                if let Translated::Events(terminal) =
                    self.terminal(finish_reason_for(reason), usage, self.refused)
                {
                    events.extend(terminal);
                }
                // The first non-null `finish_reason` is the terminal, so later choices in the same
                // chunk cannot contribute a second one.
                break;
            }
        }

        if events.is_empty() {
            Translated::Ignored
        } else {
            Translated::Events(events)
        }
    }

    /// Appends the events for any tool calls in one `delta`.
    ///
    /// The protocol's shape is worth stating because it is what the keying follows: a stream's
    /// `tool_calls` array is a list of `{index, id?, type?, function: {name?, arguments?}}`
    /// fragments. The **first** fragment for an index carries the `id` and the `name`; every later
    /// fragment repeats only the index and carries an `arguments` string, which is the text so far
    /// **for that call** and is what the provider would resend whole if it resends at all. The index
    /// is therefore the only value tying a fragment to its call, which is why the state is keyed by
    /// it, and the provider's `id` is adopted verbatim as the canonical call identifier because it is
    /// the value a later transcript continuation has to echo.
    fn push_tool_calls(
        &mut self,
        events: &mut Vec<ModelStreamEventKind>,
        delta: &serde_json::Value,
    ) {
        let Some(calls) = delta
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
        else {
            return;
        };
        // A tool call after the terminal is dropped for the same reason a content delta is: the call
        // has been declared finished, and accepting a further intent would let a hostile endpoint
        // propose an action after the run recorded its outcome.
        if self.terminal_emitted {
            self.late_deltas_ignored = self
                .late_deltas_ignored
                .saturating_add(u64::try_from(calls.len()).unwrap_or(u64::MAX));
            return;
        }
        for (position, call) in calls.iter().enumerate() {
            // An absent index is tolerated the way an absent choice index is: a minimal compatible
            // server may omit it, and with one call there is no ambiguity to resolve. The position in
            // the array is the fallback, so two calls cannot collide on one key.
            let index = call
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(position as u64);
            let function = call.get("function");
            let name = function
                .and_then(|function| function.get("name"))
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty());
            let arguments = function
                .and_then(|function| function.get("arguments"))
                .and_then(serde_json::Value::as_str);

            let entry = self
                .tool_calls
                .entry(index)
                .or_insert_with(|| AnnouncedToolCall {
                    // A call whose first fragment carries no id still needs a stable identifier, and the
                    // provider's own is the only one available. Deriving it from the index makes it
                    // distinct within the stream and reproducible for the same stream, which is what a
                    // replayed transcript needs; a fabricated value would collide across two streams.
                    call_id: format!("tool-{index}"),
                    name: String::new(),
                    arguments: String::new(),
                    announced: false,
                });
            if let Some(id) = call
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
            {
                id.clone_into(&mut entry.call_id);
            }
            if let Some(name) = name {
                name.clone_into(&mut entry.name);
            }

            // The call is announced once, on the fragment that names it. A fragment carrying neither
            // a name nor an id — which is every argument continuation — must not re-announce it, and
            // a call whose name never arrived is not announced at all: an unnamed intent cannot be
            // judged by the layer that decides, so proposing it would be proposing an action with no
            // action in it.
            if !entry.announced && !entry.name.is_empty() {
                entry.announced = true;
                let (call_id, tool_name) = (
                    entry.call_id.clone(),
                    self.tool_names.canonical(&entry.name),
                );
                events.push(ModelStreamEventKind::ToolCallAdded { call_id, tool_name });
            }

            if let Some(arguments) = arguments.filter(|arguments| !arguments.is_empty()) {
                let call_id = entry.call_id.clone();
                // Accumulated as well as streamed: the completed event carries the whole text, and a
                // fragment alone is not parseable JSON, so a consumer that read only the deltas could
                // not validate the call.
                entry.arguments.push_str(arguments);
                events.push(ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id,
                    delta: arguments.to_owned(),
                });
            }
        }
    }

    /// Appends a content delta, minting the output item on the first one.
    fn push_delta(&mut self, events: &mut Vec<ModelStreamEventKind>, text: &str) {
        if self.terminal_emitted {
            // Counted, not appended. Dropping silently would make a misbehaving endpoint invisible.
            self.late_deltas_ignored = self.late_deltas_ignored.saturating_add(1);
            return;
        }
        if self.item_id.is_none() {
            let item_id = format!("item-{}", self.delta_count.saturating_add(1));
            self.item_id = Some(item_id.clone());
            events.push(ModelStreamEventKind::OutputItemAdded { item_id });
        }
        self.delta_count = self.delta_count.saturating_add(1);
        events.push(ModelStreamEventKind::OutputTextDelta {
            // Present by construction: `push_delta` minted it on the line above.
            item_id: self.item_id.clone().unwrap_or_default(),
            delta: text.to_owned(),
        });
    }

    /// Produces the terminal, at most once per stream.
    fn terminal(
        &mut self,
        finish_reason: FinishReason,
        usage: Option<Usage>,
        refused: bool,
    ) -> Translated {
        if self.terminal_emitted {
            // A second terminal would break the contract's "exactly one terminal event" rule, and a
            // caller reading two would have to decide which to believe.
            return Translated::Ignored;
        }
        self.terminal_emitted = true;
        let mut events = Vec::new();
        // The item is closed before the call so a client sees the item complete before the call
        // does, which is the ordering the normalized stream documents.
        if let Some(item_id) = self.item_id.clone() {
            events.push(ModelStreamEventKind::OutputItemCompleted { item_id });
        }
        // Any announced tool call is completed before the terminal, because a call the model proposed
        // must be judged by the layer that decides — and the terminal is what ends the stream, so a
        // completion after it would never be read. Emitted unconditionally on announcement rather than
        // only when the provider's finish reason says `tool_calls`: the reason is the provider's claim
        // about *why* it stopped, while the announcement is the fact that it proposed something, and
        // trusting the claim would drop a call from an endpoint that reports a bare `stop` beside it.
        for call in self.tool_calls.values() {
            if !call.announced {
                continue;
            }
            events.push(ModelStreamEventKind::ToolCallCompleted {
                call_id: call.call_id.clone(),
                arguments: call.arguments.clone(),
            });
        }
        events.push(ModelStreamEventKind::CallCompleted {
            finish_reason,
            usage,
            refused,
        });
        Translated::Events(events)
    }
}

impl Default for ChunkTranslator {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps a provider `finish_reason` onto the normalized reason.
///
/// The five documented literals map directly. Anything else — including the two the schema declines
/// to name — becomes [`FinishReason::other`], and a value that cannot be *stored* (empty, too long,
/// or containing a NUL) falls back to `Other { provider_value }` with a bounded placeholder rather
/// than to `Stop`, because an unrecognized terminal must never look clean.
fn finish_reason_for(reason: &serde_json::Value) -> FinishReason {
    let Some(text) = reason.as_str() else {
        return unmodelled_finish_reason("");
    };
    match text {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        other => unmodelled_finish_reason(other),
    }
}

/// Returns an `Other` reason, or a bounded placeholder when the provider's value is unusable.
fn unmodelled_finish_reason(value: &str) -> FinishReason {
    FinishReason::other(value).unwrap_or_else(|_| FinishReason::Other {
        provider_value: "unrepresentable".to_owned(),
    })
}

/// Maps a mid-stream `error` object onto a terminal reason.
///
/// The error's own `code` is **not** mapped to a JARVIS code here: this module has no access to the
/// normalized error vocabulary, and inventing one from provider text is what the evidence note
/// forbids. The adapter's transport layer maps the status and code, so this path only has to say
/// "the provider reported an error after the stream opened".
fn finish_reason_from_error(_error: &serde_json::Value) -> FinishReason {
    FinishReason::ProviderError
}

/// Maps the provider's usage block onto the normalized one.
///
/// `total_tokens` is dropped, because [`Usage`] has no total and computing one from the parts would
/// be a JARVIS assertion rather than a provider fact. Every counter is left `None` when the provider
/// omitted it: an absent count is not a measured zero, which is the rule the type documents.
fn usage_from(usage: &serde_json::Value) -> Usage {
    let counter = |value: &serde_json::Value, path: &[&str]| -> Option<u64> {
        let mut current = value;
        for step in path {
            current = current.get(*step)?;
        }
        current.as_u64()
    };
    Usage {
        input_tokens: counter(usage, &["prompt_tokens"]),
        output_tokens: counter(usage, &["completion_tokens"]),
        cached_input_tokens: counter(usage, &["prompt_tokens_details", "cached_tokens"]),
        reasoning_tokens: counter(usage, &["completion_tokens_details", "reasoning_tokens"]),
        // The provider reported these numbers. JARVIS does not estimate a cost: pricing is a
        // per-model figure that changes independently of this protocol, and inventing one here
        // would put a guessed number in the same field a real one uses.
        provider_reported: true,
        estimated_cost_microunits: None,
        currency: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ChunkTranslator, Translated};
    use jarvis_domain::model::stream::{FinishReason, ModelStreamEventKind};
    fn chunk(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("the fixture is valid JSON")
    }

    /// Reports an unexpected shape from a test's own fixture, and diverges.
    ///
    /// A helper rather than `panic!` at each site because this crate's lint policy denies `panic` in a
    /// `lib` target, and these tests live inside one — while also denying `expect` on a literal
    /// `None`, which is the other obvious way to write this. `assert!` on a comparison of the message
    /// against itself is the form that satisfies both: it reads as an assertion, it cannot be constant
    /// folded away, and the message is the failure text.
    #[track_caller]
    fn assert_unexpected(message: &str) -> ! {
        assert!(
            message.is_empty() && !message.is_empty(),
            "unexpected shape: {message}",
        );
        // Unreachable, because the assertion above cannot hold. The diverging signature is what lets a
        // `let ... else` arm call this directly.
        std::process::abort()
    }

    fn kinds(translated: Translated) -> Vec<ModelStreamEventKind> {
        match translated {
            Translated::Events(events) => events,
            other => assert_unexpected(&format!("expected events, got {other:?}")),
        }
    }

    /// The events a translation produced, treating a chunk that contributes nothing as no events.
    ///
    /// Distinct from [`kinds`], which asserts that events were produced: a frame whose fields are all
    /// deliberately ignored — every frame carrying only model-internal reasoning, for instance —
    /// legitimately produces `Ignored`, and a helper that treated that as a failure could not express
    /// the assertion those tests need. Using the strict helper here would have made "reasoning produces
    /// no event" into a panic rather than a check.
    fn events_or_none(translated: Translated) -> Vec<ModelStreamEventKind> {
        match translated {
            Translated::Events(events) => events,
            Translated::Ignored => Vec::new(),
            // Named rather than a wildcard: a wildcard here would silently absorb a *new*
            // `Translated` variant and map it to "no events", which is how a frame that failed to
            // parse would start passing an assertion about frames that contributed nothing.
            Translated::Malformed => {
                assert_unexpected("expected events or nothing, got a malformed frame")
            }
        }
    }

    #[test]
    fn a_tool_call_is_translated_through_announcement_arguments_and_completion() {
        // **The regression test for a hole this adapter had, and it is the dangerous kind.** Tool
        // calls used to be recognized and dropped, on the reasoning that the tool fabric does not
        // exist. That reasoning was backwards: the controller has a typed terminal outcome for a tool
        // intent and no other way to reach it, so dropping the event removed the only thing that made
        // the refusal accurate — a real model that asked to call a tool produced a stream with no
        // delta and no tool event, and the run could reach `completed` with an empty answer. A silent
        // success on a dropped intent is worse than a typed refusal.
        //
        // The three frames below are the protocol's real shape, and the shape is what the keying
        // follows: the first fragment carries the index, the id, and the name; later fragments carry
        // **only the index and an argument fragment**, so a translator that keyed on the id or the
        // name would lose every fragment after the first.
        let mut translator = ChunkTranslator::new();
        let announced = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"fs.read","arguments":""}}]},"finish_reason":null}]}"#,
        ))));
        assert_eq!(
            announced,
            vec![ModelStreamEventKind::ToolCallAdded {
                call_id: "call_abc123".to_owned(),
                tool_name: "fs.read".to_owned(),
            }],
            "the first fragment announces the call",
        );

        // Two argument fragments for the same call, neither repeating the id or the name.
        let first_arguments = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]},"finish_reason":null}]}"#,
        ))));
        assert_eq!(
            first_arguments,
            vec![ModelStreamEventKind::ToolCallArgumentsDelta {
                call_id: "call_abc123".to_owned(),
                delta: "{\"pa".to_owned(),
            }],
            "an argument fragment must not re-announce the call",
        );
        let second_arguments = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"/etc/hosts\"}"}}]},"finish_reason":null}]}"#,
        ))));
        assert_eq!(
            second_arguments,
            vec![ModelStreamEventKind::ToolCallArgumentsDelta {
                call_id: "call_abc123".to_owned(),
                delta: "th\":\"/etc/hosts\"}".to_owned(),
            }],
        );

        // The terminal completes the call with the **whole** argument text, because a fragment alone is
        // not parseable JSON — the fabric validates the arguments, so it needs the complete value.
        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        ))));
        assert_eq!(
            terminal,
            vec![
                ModelStreamEventKind::ToolCallCompleted {
                    call_id: "call_abc123".to_owned(),
                    arguments: "{\"path\":\"/etc/hosts\"}".to_owned(),
                },
                ModelStreamEventKind::CallCompleted {
                    finish_reason: FinishReason::ToolCalls,
                    usage: None,
                    refused: false,
                },
            ],
            "the completion carries the accumulated arguments, and the terminal follows it",
        );
    }

    #[test]
    fn a_tool_call_is_completed_even_when_the_provider_reports_a_plain_stop() {
        // The finish reason is the provider's claim about *why* it stopped; the announcement is the
        // fact that it proposed something. Trusting the claim would drop a call from an endpoint that
        // reports a bare `stop` beside it — and this protocol permits exactly that, which is why the
        // completion is emitted on announcement rather than on the reason.
        let mut translator = ChunkTranslator::new();
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"shell.run","arguments":"{\"cmd\":\"ls\"}"}}]},"finish_reason":null}]}"#,
        )));
        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ))));
        assert!(
            terminal.iter().any(|kind| matches!(
                kind,
                ModelStreamEventKind::ToolCallCompleted { call_id, arguments }
                    if call_id == "call_1" && arguments == "{\"cmd\":\"ls\"}"
            )),
            "a proposed call must be completed whatever reason is reported: {terminal:?}",
        );
    }

    #[test]
    fn a_tool_call_with_no_name_is_not_announced() {
        // An unnamed intent cannot be judged — there is no action to decide about — so it is not
        // proposed at all rather than proposed with an empty name, which the fabric would have to
        // special-case. Its argument fragments are still streamed, because the call was real and a
        // consumer reading only the deltas should see them.
        let mut translator = ChunkTranslator::new();
        let events = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":null}]}"#,
        ))));
        assert!(
            !events
                .iter()
                .any(|kind| matches!(kind, ModelStreamEventKind::ToolCallAdded { .. })),
            "an unnamed call must not be announced: {events:?}",
        );
        assert_eq!(
            events,
            vec![ModelStreamEventKind::ToolCallArgumentsDelta {
                call_id: "tool-0".to_owned(),
                delta: "{}".to_owned(),
            }],
            "the argument fragment is still carried, under the derived identifier",
        );
    }

    #[test]
    fn two_concurrent_tool_calls_are_kept_apart_by_their_index() {
        // A model may propose several calls in one answer, and the protocol distinguishes them only by
        // `index` — the id and name appear on each one's first fragment, but the *arguments* that
        // follow carry neither. Keying on anything else would merge them into one call with two
        // names' worth of arguments, so this asserts the separation rather than trusting it.
        let mut translator = ChunkTranslator::new();
        let announced = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"fs.read","arguments":""}},{"index":1,"id":"call_b","function":{"name":"fs.write","arguments":""}}]},"finish_reason":null}]}"#,
        ))));
        assert_eq!(
            announced,
            vec![
                ModelStreamEventKind::ToolCallAdded {
                    call_id: "call_a".to_owned(),
                    tool_name: "fs.read".to_owned(),
                },
                ModelStreamEventKind::ToolCallAdded {
                    call_id: "call_b".to_owned(),
                    tool_name: "fs.write".to_owned(),
                },
            ],
        );

        // Interleaved argument fragments, which is what a real stream does. The argument text is
        // written literally rather than built with a format string, because the first version of this
        // test embedded it through `{delta:?}` — which quotes and escapes the value, so the JSON held
        // an escaped string and the assertion compared against something else entirely.
        for (index, call_id, arguments) in [
            (1_u64, "call_b", r#"{"b":1}"#),
            (0_u64, "call_a", r#"{"a":1}"#),
        ] {
            let document = format!(
                r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":{index},"function":{{"arguments":{}}}}}]}},"finish_reason":null}}]}}"#,
                serde_json::Value::String(arguments.to_owned()),
            );
            let events = kinds(translator.translate(Some(&chunk(&document))));
            assert_eq!(
                events,
                vec![ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id: call_id.to_owned(),
                    delta: arguments.to_owned(),
                }],
                "an argument fragment must reach the call its index names",
            );
        }

        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        ))));
        let completed: Vec<(&str, &str)> = terminal
            .iter()
            .filter_map(|kind| match kind {
                ModelStreamEventKind::ToolCallCompleted { call_id, arguments } => {
                    Some((call_id.as_str(), arguments.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            completed,
            vec![("call_a", r#"{"a":1}"#), ("call_b", r#"{"b":1}"#)],
            "each call is completed with only its own arguments: {terminal:?}",
        );
    }

    #[test]
    fn a_tool_call_after_the_terminal_is_dropped_like_late_content() {
        // The same rule as late content, and for the same reason: the call has been declared finished,
        // so accepting a further intent would let a hostile endpoint propose an action after the run
        // recorded its outcome.
        let mut translator = ChunkTranslator::new();
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        )));
        let late = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_late","function":{"name":"shell.run","arguments":"{}"}}]},"finish_reason":null}]}"#,
        )));
        // `Ignored` rather than `Events([])`, because the chunk produced nothing at all — which is how
        // this translator reports "nothing to say", and is why the assertion is on the variant rather
        // than on a length.
        assert!(
            matches!(late, Translated::Ignored),
            "a late tool call must produce nothing, got {late:?}",
        );
        assert!(
            translator.late_deltas_ignored() > 0,
            "a dropped late call must be counted rather than silently discarded",
        );
    }

    #[test]
    fn a_content_delta_mints_the_item_once_and_then_streams() {
        // per-item identifier: a client that received deltas for an item it was never told about
        // could not attribute them.
        let mut translator = ChunkTranslator::new();
        let first = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"},"finish_reason":null}]}"#,
        ))));
        assert_eq!(
            first,
            vec![
                ModelStreamEventKind::OutputItemAdded {
                    item_id: "item-1".to_owned()
                },
                ModelStreamEventKind::OutputTextDelta {
                    item_id: "item-1".to_owned(),
                    delta: "Hel".to_owned()
                },
            ],
        );

        let second = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":null}]}"#,
        ))));
        assert_eq!(
            second,
            vec![ModelStreamEventKind::OutputTextDelta {
                item_id: "item-1".to_owned(),
                delta: "lo".to_owned()
            }],
            "the item must be opened once, not per delta",
        );
        assert_eq!(translator.delta_count(), 2);
    }

    #[test]
    fn a_role_only_first_chunk_contributes_nothing() {
        // The documented first chunk carries the role and an empty content string. Emitting an empty
        // delta would inflate `output_delta_count` — the sample size `BRN-011` aggregates — so a
        // single-delta answer would look like a stream by count.
        let mut translator = ChunkTranslator::new();
        assert_eq!(
            translator.translate(Some(&chunk(
                r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            ))),
            Translated::Ignored,
        );
        assert_eq!(translator.delta_count(), 0);
    }

    #[test]
    fn model_internal_reasoning_is_counted_and_never_becomes_output() {
        // **Found by capturing a real stream, not by reading the documented schema.** Every frame of a
        // live capture from an operator-configured endpoint carried `delta.reasoning` — the model's
        // own thinking — and `reasoning` is **absent from the documented Chat Completions field set**
        // this adapter's mapping table was built from. It reached the translator as one of the fields
        // nobody had named, which is the one condition under which a field is neither translated nor
        // deliberately dropped.
        //
        // Two properties, and the second is the one with teeth. The text must **not** become output,
        // because the contract's memory rules forbid persisting hidden reasoning — and the fact that it
        // was seen must be **counted**, because a silent drop is indistinguishable from a field the
        // translator forgot to handle. A test asserting only the first would pass against an
        // implementation that discarded the text without noticing it at all.
        let mut translator = ChunkTranslator::new();
        let translated = events_or_none(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning":"The user wants"},"finish_reason":null}]}"#,
        ))));
        assert!(
            translated.is_empty(),
            "reasoning must produce no event at all: {translated:?}",
        );
        assert_eq!(
            translator.reasoning_chunks_ignored(),
            1,
            "the reasoning text must be counted, or the drop is an omission rather than a decision",
        );
        assert_eq!(
            translator.delta_count(),
            0,
            "reasoning is not an output delta, so it must not inflate the delivery measurement",
        );

        // **A frame carrying both must yield the answer and only count the thinking.** This is the
        // real shape: the capture interleaves reasoning with an empty `content`, then sends the answer
        // in a later frame. An implementation that treated a presence of `reasoning` as "skip this
        // frame" would lose the answer on a provider that packs both together.
        let both = events_or_none(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"DONE PROBE","reasoning":"so just say it"},"finish_reason":null}]}"#,
        ))));
        // Asserted on the *delta* rather than on the whole event list: the translator opens its item
        // lazily, so the first event is an `OutputItemAdded` whose identifier it generates. Pinning
        // that here would make this test assert the translator's internal item management rather than
        // the property it is about — which is that the answer survives and the thinking does not.
        let deltas: Vec<&str> = both
            .iter()
            .filter_map(|kind| match kind {
                ModelStreamEventKind::OutputTextDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            deltas,
            vec!["DONE PROBE"],
            "a frame carrying both must yield the answer and nothing else: {both:?}",
        );
        assert_eq!(translator.reasoning_chunks_ignored(), 2);
        assert_eq!(translator.delta_count(), 1);

        // An empty reasoning string is not a reasoning chunk, matching the empty-`content` rule: the
        // documented first chunk carries `content: ""`, and counting that as an output delta was a
        // defect once already.
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"reasoning":""},"finish_reason":null}]}"#,
        )));
        assert_eq!(
            translator.reasoning_chunks_ignored(),
            2,
            "an empty reasoning string is not a reasoning chunk",
        );
    }

    #[test]
    fn the_shape_of_a_captured_stream_from_a_real_endpoint_is_handled() {
        // **Built from a real capture rather than from the documentation.** The frames below are
        // transcribed from one stream from an operator-configured OpenAI-compatible endpoint, and this
        // test exists because the capture disagreed with the schema in two ways that no
        // documentation-derived fixture could have shown:
        //
        // 1. `delta.reasoning` appears on most frames and is **not in the documented field set**.
        // 2. The stream really does end with `data: [DONE]`, which the evidence note records as
        //    `OC-C004` and could not verify from the cited page — so `[DONE]`'s presence is now
        //    **observed** rather than assumed.
        //
        // Its assertions are the *properties* the real stream must satisfy, not a recording of a
        // particular run: the answer arrives exactly once, reasoning never becomes output, the final
        // usage chunk has empty `choices` (the `OC-C003` shape), and the sentinel terminates nothing.
        let frames = [
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning":"The"},"finish_reason":null}]}"#,
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"","reasoning":" user wants"},"finish_reason":null}]}"#,
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"DONE PROBE"},"finish_reason":null}]}"#,
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","choices":[],"usage":{"prompt_tokens":38,"completion_tokens":22,"total_tokens":60}}"#,
        ];
        let mut translator = ChunkTranslator::new();
        let mut output = String::new();
        let mut terminals = 0_usize;
        let mut usage_seen = 0_usize;
        for frame in frames {
            for kind in events_or_none(translator.translate(Some(&chunk(frame)))) {
                match kind {
                    ModelStreamEventKind::OutputTextDelta { delta, .. } => output.push_str(&delta),
                    ModelStreamEventKind::CallCompleted { .. } => terminals += 1,
                    ModelStreamEventKind::UsageUpdated { .. } => usage_seen += 1,
                    _ => {}
                }
            }
        }
        // The sentinel last, exactly as the capture has it: it must contribute nothing and must not
        // be a terminal, or a provider that omits it would end every stream differently.
        assert_eq!(translator.translate(None), Translated::Ignored);

        assert_eq!(
            output, "DONE PROBE",
            "the answer must be exactly the answer, with no reasoning text folded in",
        );
        assert_eq!(
            translator.reasoning_chunks_ignored(),
            2,
            "both reasoning frames must be counted",
        );
        assert_eq!(
            terminals, 1,
            "one terminal, from the first non-null finish_reason"
        );
        assert_eq!(
            usage_seen, 1,
            "the empty-choices usage chunk must still report usage",
        );
    }

    #[test]
    fn a_refusal_is_a_verdict_and_not_only_text() {
        // **The trap the evidence note records as `OC-C001`.** A refusal arrives as HTTP 200 with
        // `delta.refusal` populated, so a reader keyed on the status records it as a finished
        // answer. The text alone cannot carry the distinction, which is why the terminal says
        // `refused: true`.
        let mut translator = ChunkTranslator::new();
        let translated = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"refusal":"I cannot help with that."},"finish_reason":null}]}"#,
        ))));
        assert!(
            translated.iter().any(|kind| matches!(
                kind,
                ModelStreamEventKind::OutputTextDelta { delta, .. } if delta == "I cannot help with that."
            )),
            "the refusal text is surfaced: {translated:?}",
        );

        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ))));
        let ModelStreamEventKind::CallCompleted {
            finish_reason,
            refused,
            ..
        } = terminal.last().expect("a terminal")
        else {
            assert_unexpected("expected a completion, got {terminal:?}");
        };
        assert!(
            *refused,
            "a refusal must be recorded as a refusal, not as a clean completion",
        );
        // The provider named `stop` — a refusal is an ordinary completion with a safety flag — and
        // the translator **reports that verbatim**. Resolving the flag into `Refusal` belongs to the
        // controller's `completed_reason`, so asserting `Refusal` here would be asserting a rule
        // this layer does not own, and would pass while the controller's fold was broken.
        assert_eq!(*finish_reason, FinishReason::Stop);
    }

    #[test]
    fn a_provider_named_reason_is_not_overwritten_by_the_refusal_flag() {
        // The translator must not fold the flag into the reason at all — that is the controller's
        // rule — so a provider naming `content_filter` is reported verbatim even with a refusal
        // flagged. The controller's own test covers the fold itself.
        let mut translator = ChunkTranslator::new();
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"refusal":"no"},"finish_reason":null}]}"#,
        )));
        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"content_filter"}]}"#,
        ))));
        let last = terminal.last().expect("a terminal");
        let ModelStreamEventKind::CallCompleted { finish_reason, .. } = last else {
            assert_unexpected("expected a completion");
        };
        assert_eq!(*finish_reason, FinishReason::ContentFilter);
    }

    #[test]
    fn an_unmodelled_finish_reason_is_retained_rather_than_flattened() {
        // The schema renders five literals followed by "or 2 more" (`OC-C007`), so a build that
        // mapped an unknown value to `Stop` would report an unrecognized terminal as a clean one.
        let mut translator = ChunkTranslator::new();
        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"future_reason"}]}"#,
        ))));
        let ModelStreamEventKind::CallCompleted { finish_reason, .. } =
            terminal.last().expect("a terminal")
        else {
            assert_unexpected("expected a completion");
        };
        assert_eq!(
            *finish_reason,
            FinishReason::Other {
                provider_value: "future_reason".to_owned()
            },
        );
    }

    #[test]
    fn the_documented_finish_reasons_map_directly() {
        for (value, expected) in [
            ("stop", FinishReason::Stop),
            ("length", FinishReason::Length),
            ("tool_calls", FinishReason::ToolCalls),
            ("function_call", FinishReason::ToolCalls),
            ("content_filter", FinishReason::ContentFilter),
        ] {
            let mut translator = ChunkTranslator::new();
            let document =
                format!(r#"{{"choices":[{{"index":0,"delta":{{}},"finish_reason":"{value}"}}]}}"#);
            let terminal = kinds(translator.translate(Some(&chunk(&document))));
            let ModelStreamEventKind::CallCompleted { finish_reason, .. } =
                terminal.last().expect("a terminal")
            else {
                assert_unexpected("expected a completion for {value}");
            };
            assert_eq!(*finish_reason, expected, "for {value}");
        }
    }

    #[test]
    fn only_one_terminal_is_produced_however_many_frames_follow() {
        // The contract's "exactly one terminal event" rule. A second terminal would make a caller
        // decide which to believe, and the run controller would have to choose.
        let mut translator = ChunkTranslator::new();
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#,
        )));
        let first = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        ))));
        assert!(first.iter().any(ModelStreamEventKind::is_terminal));

        assert_eq!(
            translator.translate(Some(&chunk(
                r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            ))),
            Translated::Ignored,
            "a second terminal must not be emitted",
        );
    }

    #[test]
    fn content_after_the_terminal_is_counted_and_dropped() {
        // A hostile or buggy endpoint that keeps sending after declaring the answer finished must
        // not be able to extend the stored answer past the terminal the run recorded.
        let mut translator = ChunkTranslator::new();
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"done"},"finish_reason":null}]}"#,
        )));
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        )));
        let late = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":" and more"},"finish_reason":null}]}"#,
        )));
        // `Ignored` rather than `Events(..)`: a dropped delta emits nothing, so the frame contributes
        // no event at all. Asserting on the absence of events through the counter *and* the variant is
        // the pair that makes this a check rather than a restatement — a translator that emitted an
        // empty delta list would pass a counter-only assertion while still telling a client there was
        // more output.
        assert_eq!(late, Translated::Ignored);
        assert_eq!(translator.late_deltas_ignored(), 1);
        assert_eq!(
            translator.delta_count(),
            1,
            "the dropped delta must not inflate the measurement sample size",
        );
    }

    #[test]
    fn a_choice_other_than_the_first_is_refused() {
        // `n > 1` is refused at request construction, so a second choice is an answer this call did
        // not ask for. Interleaving it would merge two answers into one item.
        let mut translator = ChunkTranslator::new();
        assert_eq!(
            translator.translate(Some(&chunk(
                r#"{"choices":[{"index":1,"delta":{"content":"other"},"finish_reason":null}]}"#,
            ))),
            Translated::Malformed,
        );
    }

    #[test]
    fn a_usage_only_final_chunk_is_understood() {
        // The documented shape when `stream_options.include_usage` is requested: `choices` empty and
        // a populated `usage` (`OC-C003`).
        let mut translator = ChunkTranslator::new();
        let events = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":3,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":4},"completion_tokens_details":{"reasoning_tokens":1}}}"#,
        ))));
        let ModelStreamEventKind::UsageUpdated { usage } = &events[0] else {
            assert_unexpected("expected usage, got {events:?}");
        };
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(3));
        assert_eq!(usage.cached_input_tokens, Some(4));
        assert_eq!(usage.reasoning_tokens, Some(1));
        assert!(usage.provider_reported);
        // JARVIS does not estimate a cost, so the field stays absent rather than carrying a guess.
        assert_eq!(usage.estimated_cost_microunits, None);
    }

    #[test]
    fn a_missing_counter_is_absent_rather_than_zero() {
        // "Unknown is not zero." A usage block that omits the detail objects must leave those
        // counters `None`, because `Some(0)` would be a measured zero — a free call rather than an
        // unmeasured one.
        let mut translator = ChunkTranslator::new();
        let events = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":1,"total_tokens":8}}"#,
        ))));
        let ModelStreamEventKind::UsageUpdated { usage } = &events[0] else {
            assert_unexpected("expected usage");
        };
        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.cached_input_tokens, None);
        assert_eq!(usage.reasoning_tokens, None);
        assert!(usage.has_any_counter());
    }

    #[test]
    fn a_usage_block_on_the_terminal_chunk_reaches_the_terminal() {
        // The other documented arrival path: a provider that reports usage on the same chunk as the
        // finish reason. The controller accepts usage from either path, so the translator must
        // deliver it on the terminal as well as on a standalone chunk.
        let mut translator = ChunkTranslator::new();
        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#,
        ))));
        let ModelStreamEventKind::CallCompleted { usage, .. } =
            terminal.last().expect("a terminal")
        else {
            assert_unexpected("expected a completion");
        };
        let usage = usage.as_ref().expect("usage must reach the terminal");
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(2));
    }

    #[test]
    fn the_done_sentinel_is_ignored_and_is_not_the_terminal() {
        // `OC-C004`: the sentinel is not on the page this adapter's evidence note cites, so it is
        // accepted and skipped rather than relied upon. A translator that terminated here would fail
        // against a server that omits it while every test still passed.
        let mut translator = ChunkTranslator::new();
        assert_eq!(translator.translate(None), Translated::Ignored);
    }

    #[test]
    fn unknown_fields_and_unknown_chunk_kinds_are_ignored_for_forward_compatibility() {
        // `OC-C005`: new properties and new stream event types may be added without a version bump.
        // Refusing an unknown field would break the adapter on the provider's next release.
        let mut translator = ChunkTranslator::new();
        let events = kinds(translator.translate(Some(&chunk(
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","system_fingerprint":"fp_1","obfuscation":"r4N7vQ2m","service_tier":"default","a_new_field":{"nested":true},"choices":[{"index":0,"delta":{"content":"x"},"logprobs":null,"finish_reason":null}]}"#,
        ))));
        assert_eq!(translator.delta_count(), 1);
        assert!(matches!(
            events.last(),
            Some(ModelStreamEventKind::OutputTextDelta { .. })
        ));

        // A chunk that is not an object at all is malformed, which is different from one that merely
        // carries unfamiliar members: the first cannot be interpreted, the second can.
        assert_eq!(
            translator.translate(Some(&chunk(r#""just a string""#))),
            Translated::Malformed,
        );
    }

    #[test]
    fn a_mid_stream_error_terminates_the_call() {
        // An error can arrive after a `200`, so it must end the call rather than be ignored — the
        // alternative is a stream that simply stops, which the state machine records as interrupted
        // and which reads like a transport fault rather than a provider error.
        let mut translator = ChunkTranslator::new();
        let _ = translator.translate(Some(&chunk(
            r#"{"choices":[{"index":0,"delta":{"content":"par"},"finish_reason":null}]}"#,
        )));
        let terminal = kinds(translator.translate(Some(&chunk(
            r#"{"error":{"message":"upstream failed","type":"server_error","code":"server_error"}}"#,
        ))));
        let ModelStreamEventKind::CallCompleted { finish_reason, .. } =
            terminal.last().expect("a terminal")
        else {
            assert_unexpected("expected a terminal");
        };
        assert_eq!(*finish_reason, FinishReason::ProviderError);
    }
}
