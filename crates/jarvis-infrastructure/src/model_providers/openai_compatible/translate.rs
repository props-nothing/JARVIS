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
    /// The provider's model identifier, reported on the first chunk that carries one.
    model: Option<String>,
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
            model: None,
        }
    }

    /// Returns how many output deltas were translated.
    ///
    /// Used by the adapter to report the measurement `BRN-011` aggregates, from the same counter the
    /// events came from rather than a second tally that could disagree.
    #[must_use]
    pub const fn delta_count(&self) -> u32 {
        self.delta_count
    }

    /// Returns how many deltas arrived after the terminal and were discarded.
    #[must_use]
    pub const fn late_deltas_ignored(&self) -> u64 {
        self.late_deltas_ignored
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
                // `role`, `tool_calls`, and `function_call` are recognized and not translated. Tool
                // calling is explicitly out of this slice's scope (the evidence note records it),
                // and silently emitting a tool-call event for a tool the adapter never advertised
                // would propose an action the run could not resolve.
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

    #[test]
    fn a_content_delta_mints_the_item_once_and_then_streams() {
        // The item is opened before its first delta and only once, because this contract carries no
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
