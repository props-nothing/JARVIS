//! The normalized model stream: request, input items, events, usage, terminal state.
//!
//! This module is the domain half of `docs/contracts/model-stream.md`. It exists
//! so a provider adapter cannot invent its own event vocabulary and so the rules
//! the contract states in prose are enforced by construction:
//!
//! - **Sequence increases monotonically.** [`ModelStreamState::accept`] refuses a
//!   duplicate or out-of-order frame instead of silently reordering it, because a
//!   reordered stream produces a plausible transcript that never happened.
//! - **Exactly one terminal event.** A stream that ends without one is
//!   *interrupted*, which is a distinct outcome from success, and a second
//!   terminal event is refused.
//! - **Late frames after the local terminal state are ignored and counted**, as
//!   the contract requires, rather than being an error: a provider may flush a few
//!   frames after a local cancellation, and treating that as a fault would make a
//!   correct cancellation look broken.
//! - **Unknown usage is not zero.** Every usage field is optional, because
//!   `input_tokens: 0` and "the provider did not report input tokens" are
//!   different facts and only one of them is safe to bill against.
//! - **Argument deltas are not executable.** [`ToolArguments`] exposes a raw string
//!   for execution *only* in the complete state, so a half-received argument
//!   string cannot be dispatched by a caller that forgot to check.
//! - **A tool result cannot be orphaned.** [`InputItems::new`] refuses a result
//!   whose call is absent, which is what stops context compaction from leaving a
//!   result that answers nothing.
//!
//! Two payload classes are deliberately **not** modelled here. A requested output
//! schema is carried as [`JsonText`] rather than a parsed value, because schema
//! validation belongs to the tool fabric, and a provider extension map is absent
//! entirely because the contract makes extensions *adapter-owned*. The domain
//! therefore keeps the dependency set its evidence record describes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{ModelCallId, ModelStreamEventId, RunId};
use crate::model::capability::Capability;
use crate::model::identity::ModelRef;
use crate::model::policy::Locality;
use crate::time::UtcTimestamp;

/// The longest accepted opaque provider string (request ID, continuation, reason).
///
/// Provider strings reach logs, persisted records, and diagnostics, so they are
/// bounded at the boundary rather than where they are displayed.
pub const MAX_PROVIDER_STRING_BYTES: usize = 256;

/// The longest accepted JSON document carried as text.
pub const MAX_JSON_TEXT_BYTES: usize = 64 * 1024;

/// Returns a bounded, non-empty, NUL-free copy of an opaque provider string.
fn bounded_provider_string(value: &str) -> Result<String, DomainError> {
    if value.is_empty() || value.len() > MAX_PROVIDER_STRING_BYTES || value.contains('\0') {
        return Err(DomainError::UnboundedProviderValue);
    }
    Ok(value.to_owned())
}

/// A JSON document carried as bounded text.
///
/// The domain does not parse this. A requested output schema is validated by the
/// tool fabric against a supported schema subset, so parsing it here would put a
/// JSON implementation inside the domain layer and would also mean two places
/// decide what a valid schema is. Carrying it as bounded text keeps the bound and
/// the `UTF-8` encoding rule in one place and leaves the meaning to its owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonText(String);

impl JsonText {
    /// Validates and wraps a JSON document.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::UnboundedProviderValue`] when `value` is empty,
    /// contains a NUL byte, or exceeds [`MAX_JSON_TEXT_BYTES`]. The value is
    /// **not** parsed: this constructor answers "is this a usable document to
    /// carry", not "is this a valid schema", because only the schema's owner can
    /// answer the second question.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > MAX_JSON_TEXT_BYTES || value.contains('\0') {
            return Err(DomainError::UnboundedProviderValue);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the document text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A request-relative ordering position.
///
/// Newtyped so a sequence cannot be passed where a count or a token total is
/// expected; the two are both "a number" and transposing them is silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sequence(u64);

impl Sequence {
    /// The first event of a stream.
    pub const FIRST: Self = Self(0);

    /// Wraps a sequence number.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next sequence number.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::StreamSequenceExhausted`] at `u64::MAX` rather than
    /// wrapping, because a wrapped sequence would look like a duplicate to the
    /// monotonicity check and silently truncate a very long stream.
    pub const fn next(self) -> Result<Self, DomainError> {
        match self.0.checked_add(1) {
            Some(next) => Ok(Self(next)),
            None => Err(DomainError::StreamSequenceExhausted),
        }
    }
}

/// An input or output modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Text.
    Text,
    /// A still image.
    Image,
    /// Audio.
    Audio,
    /// Video.
    Video,
}

/// The conversation role that produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The system/developer policy role.
    System,
    /// The user.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
}

/// Hard requirements a route must satisfy for one call.
///
/// These are *requirements*, not preferences, so a candidate that cannot be
/// attested for each required capability is rejected rather than downgraded. The
/// contract is explicit that a hard requirement never degrades into an ignored
/// extension field.
///
/// The requirements reuse the **same** [`Capability`] vocabulary the descriptor
/// attests and the same [`Locality`] the data policy resolves. That is deliberate:
/// a second, parallel set of booleans would let a requirement exist that no
/// capability key can satisfy, and the mismatch would be invisible because both
/// sides would still compile. It is also why `local_only` is a [`Locality`] rather
/// than a flag — "the route must be local" and "the policy resolved to local-only"
/// are then the same value instead of two facts that can disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRequirements {
    /// Modalities the call needs. A set, so a duplicated modality is impossible.
    pub modalities: BTreeSet<Modality>,
    /// The capabilities the selected candidate must attest with fresh, verified
    /// evidence. Measured incremental delivery is one of these, which is what keeps
    /// "streams" from being requested where "delivers incrementally" is meant.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub required_capabilities: BTreeSet<Capability>,
    /// The locality the candidate must satisfy.
    pub locality: Locality,
}

impl RouteRequirements {
    /// Requirements for a plain text call with no special capabilities.
    #[must_use]
    pub fn text() -> Self {
        Self {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: BTreeSet::new(),
            locality: Locality::ApprovedCloudAllowed,
        }
    }

    /// Returns whether the call requires **measured** incremental delivery.
    #[must_use]
    pub fn requires_incremental_delivery(&self) -> bool {
        self.required_capabilities
            .contains(&Capability::IncrementalDelivery)
    }

    /// Returns whether the call requires native tool calling.
    #[must_use]
    pub fn requires_tool_calling(&self) -> bool {
        self.required_capabilities
            .contains(&Capability::ToolCalling)
    }

    /// Returns whether the call must stay on the local device.
    #[must_use]
    pub fn requires_local_only(&self) -> bool {
        matches!(self.locality, Locality::LocalOnly)
    }
}

/// Per-call budget. Every field is optional so an unset budget is not zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallLimits {
    /// A wall-clock deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<UtcTimestamp>,
    /// The maximum output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// The maximum estimated cost in millionths of the billing currency unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_microunits: Option<u64>,
}

/// A block of message content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentBlock {
    /// A text block.
    Text {
        /// The text itself.
        text: String,
    },
    /// A reference to a stored artifact, never inline binary.
    ArtifactRef {
        /// The artifact identifier.
        artifact_id: String,
        /// The declared media type.
        media_type: String,
    },
}

/// One normalized input item.
///
/// The contract fixes the supported kinds. A provider continuation reference is
/// carried in the adapter's own metadata rather than as a base input item, so the
/// base request has no arbitrary field a provider could smuggle state through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputItem {
    /// A reference to system policy rather than inline policy text.
    ///
    /// The policy text is resolved by JARVIS, not supplied by a caller, so a
    /// request body cannot rewrite the system policy.
    SystemPolicyRef {
        /// The policy identifier to resolve.
        policy_ref: String,
    },
    /// A conversation message.
    Message {
        /// Who produced it.
        role: Role,
        /// The content blocks.
        blocks: Vec<ContentBlock>,
    },
    /// A model-proposed tool call.
    ToolCall {
        /// The stable canonical call identifier.
        call_id: String,
        /// The tool name.
        tool_name: String,
        /// The raw arguments as received, before completion.
        arguments: ToolArguments,
    },
    /// The result of a tool call, paired by canonical call ID.
    ToolResult {
        /// The call this answers.
        call_id: String,
        /// Whether the tool reported success.
        is_error: bool,
        /// The bounded result text.
        content: String,
    },
    /// A concise, user-visible reasoning summary.
    ///
    /// This is deliberately a *summary* the provider published for display. Hidden
    /// chain-of-thought is never a JARVIS input item or output event.
    ReasoningSummary {
        /// The summary text.
        summary: String,
    },
}

impl InputItem {
    /// Returns the canonical tool-call ID this item pairs on, if any.
    #[must_use]
    pub fn call_id(&self) -> Option<&str> {
        match self {
            Self::ToolCall { call_id, .. } | Self::ToolResult { call_id, .. } => Some(call_id),
            Self::SystemPolicyRef { .. } | Self::Message { .. } | Self::ReasoningSummary { .. } => {
                None
            }
        }
    }
}

/// Tool-call arguments, which are not executable until they are complete.
///
/// There is deliberately no accessor that returns a partial argument string for
/// execution. A caller must go through [`ToolArguments::executable_raw`], which
/// answers `None` while the arguments are still streaming, so "argument deltas are
/// not executable" is a property of the type rather than a rule each call site has
/// to remember. Parsing and schema validation happen in the tool fabric, which is
/// the layer that owns what a valid tool input is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ToolArguments {
    /// A partial argument string assembled from deltas. Not executable.
    Streaming {
        /// The bytes received so far.
        raw: String,
    },
    /// A complete argument string. Still unvalidated; see [`Self::executable_raw`].
    Complete {
        /// The complete raw string.
        raw: String,
    },
}

impl ToolArguments {
    /// Returns whether the arguments are complete.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    /// Returns the raw argument string for execution, only once it is complete.
    ///
    /// This is the single door to execution, so a caller cannot dispatch a tool on
    /// half-received arguments by reading the buffer directly. The returned string
    /// is unvalidated: the tool fabric still parses and validates it, and an
    /// invalid document must be refused rather than treated as empty arguments.
    #[must_use]
    pub fn executable_raw(&self) -> Option<&str> {
        match self {
            Self::Complete { raw } => Some(raw),
            Self::Streaming { .. } => None,
        }
    }

    /// Returns the raw string regardless of state, for diagnostics and assembly.
    ///
    /// Named to make its purpose obvious at a call site: a reader that only needs
    /// the bytes must not reach for this when it meant to execute the call.
    #[must_use]
    pub fn raw_for_assembly(&self) -> &str {
        match self {
            Self::Streaming { raw } | Self::Complete { raw } => raw,
        }
    }
}

/// A validated, ordered list of input items.
///
/// The only way to construct the list is [`InputItems::new`], which refuses a tool
/// result whose tool call is absent. That is the invariant the contract states for
/// context compaction: a result cannot survive its call, so a compacted request
/// never sends a result that answers nothing. Deserialization also goes through
/// [`InputItems::new`], so the rule holds for a payload that arrived over the wire
/// and not only for values built inside this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputItems(Vec<InputItem>);

impl InputItems {
    /// Validates and wraps an ordered item list.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::OrphanedToolResult`] when a tool result has no
    /// corresponding tool call anywhere earlier in the list. The check is
    /// order-sensitive on purpose: a result must follow the call it answers, so
    /// searching the whole list would accept a result that precedes its call.
    ///
    /// Returns [`DomainError::DuplicateToolCallId`] when one canonical call ID is
    /// declared twice, because a result answering it would then be ambiguous and
    /// resolving to "the last one" would pair it with the wrong arguments.
    pub fn new(items: Vec<InputItem>) -> Result<Self, DomainError> {
        let mut seen_calls: BTreeMap<&str, &str> = BTreeMap::new();
        for item in &items {
            match item {
                InputItem::ToolCall {
                    call_id, tool_name, ..
                } => {
                    if seen_calls.insert(call_id, tool_name).is_some() {
                        return Err(DomainError::DuplicateToolCallId);
                    }
                }
                InputItem::ToolResult { call_id, .. } => {
                    if !seen_calls.contains_key(call_id.as_str()) {
                        return Err(DomainError::OrphanedToolResult);
                    }
                }
                InputItem::SystemPolicyRef { .. }
                | InputItem::Message { .. }
                | InputItem::ReasoningSummary { .. } => {}
            }
        }
        Ok(Self(items))
    }

    /// Returns the items in order.
    #[must_use]
    pub fn as_slice(&self) -> &[InputItem] {
        &self.0
    }

    /// Returns the number of items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether there are no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for InputItems {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Serialized as a plain sequence so the wire form carries no wrapper the
        // domain invariant would then have to be re-checked against.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for InputItems {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialization goes through `new`, so a remote payload cannot introduce
        // an orphaned or duplicated tool call that `new` would have refused. A
        // derived deserializer would build the value directly and skip the check,
        // which would make the invariant hold only for callers inside this crate.
        let items = Vec::<InputItem>::deserialize(deserializer)?;
        Self::new(items).map_err(serde::de::Error::custom)
    }
}

/// A normalized model call request.
///
/// A provider extension map is deliberately absent: the contract makes extensions
/// **adapter-owned and namespaced**, so they are attached at the adapter boundary
/// rather than living as an arbitrary field on the portable request, where a
/// provider-specific value could be read as a portable one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCallRequest {
    /// The logical call identifier.
    pub call_id: ModelCallId,
    /// The run this call belongs to.
    pub run_id: RunId,
    /// Hard route requirements.
    pub route_requirements: RouteRequirements,
    /// The ordered input items.
    pub input: InputItems,
    /// The canonical tool names available to this call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// The requested output schema document, when structured output is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonText>,
    /// Budgets for this call.
    pub limits: CallLimits,
}

/// A normalized provider usage block.
///
/// Every counter is optional because unknown is not zero: a missing count means
/// the provider did not report it, which must not be recorded as a measured zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Reported input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Reported output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Reported cached input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    /// Reported reasoning tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Whether these values are provider-reported rather than JARVIS-estimated.
    pub provider_reported: bool,
    /// The estimated cost in millionths of the billing currency unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_microunits: Option<u64>,
    /// The billing currency, when a cost is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl Usage {
    /// Returns whether any counter was reported.
    ///
    /// Used so "no usage at all" is distinguishable from "usage reported as
    /// zero", which is the difference between an unmeasured call and a free one.
    #[must_use]
    pub const fn has_any_counter(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.reasoning_tokens.is_some()
    }
}

/// Why a model call stopped producing output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FinishReason {
    /// The model finished its answer.
    Stop,
    /// The output limit was reached.
    Length,
    /// The model asked for a tool call.
    ToolCalls,
    /// A provider content filter stopped the output.
    ContentFilter,
    /// The model refused.
    Refusal,
    /// The provider reported an error after the stream opened.
    ProviderError,
    /// The call was cancelled.
    Cancelled,
    /// A finish reason this build does not model.
    ///
    /// The raw provider value is retained beside it so a new provider reason is
    /// visible to an operator instead of being flattened into `Stop`, which would
    /// make an unknown terminal look like a clean one.
    Other {
        /// The provider's own value, bounded.
        provider_value: String,
    },
}

impl FinishReason {
    /// Validates and bounds an unmodelled provider finish reason.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::UnboundedProviderValue`] when the value is empty,
    /// too long, or contains a NUL byte.
    pub fn other(provider_value: &str) -> Result<Self, DomainError> {
        Ok(Self::Other {
            provider_value: bounded_provider_string(provider_value)?,
        })
    }
}

/// Provider-supplied metadata retained in typed fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMetadata {
    /// The provider's request identifier, for support correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// A provider continuation reference for resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_ref: Option<String>,
    /// The provider's raw finish reason, when it differs from the normalized one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_finish_reason: Option<String>,
}

impl ProviderMetadata {
    /// Validates and bounds every populated field.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::UnboundedProviderValue`] for an out-of-bound value,
    /// so an unbounded provider string is refused at the boundary rather than
    /// persisted and discovered later.
    pub fn validated(mut self) -> Result<Self, DomainError> {
        for field in [
            self.request_id.as_mut(),
            self.continuation_ref.as_mut(),
            self.provider_finish_reason.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            *field = bounded_provider_string(field)?;
        }
        Ok(self)
    }
}

/// The payload of one stream event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStreamEventKind {
    /// The call opened.
    CallStarted {
        /// The provider/model actually selected, once known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelRef>,
    },
    /// An output item was allocated.
    OutputItemAdded {
        /// The output item identifier.
        item_id: String,
    },
    /// A text delta for an output item.
    OutputTextDelta {
        /// The output item identifier.
        item_id: String,
        /// The delta text.
        delta: String,
    },
    /// An output item completed.
    OutputItemCompleted {
        /// The output item identifier.
        item_id: String,
    },
    /// A tool call was proposed.
    ToolCallAdded {
        /// The stable canonical call identifier.
        call_id: String,
        /// The tool name.
        tool_name: String,
    },
    /// A tool-argument delta. Never executable on its own.
    ToolCallArgumentsDelta {
        /// The stable canonical call identifier.
        call_id: String,
        /// The argument delta.
        delta: String,
    },
    /// A tool call completed and its arguments are final.
    ToolCallCompleted {
        /// The stable canonical call identifier.
        call_id: String,
        /// The complete raw arguments.
        arguments: String,
    },
    /// A user-visible reasoning summary delta.
    ReasoningSummaryDelta {
        /// The summary delta.
        delta: String,
    },
    /// A usage update. May arrive before, with, or after output completion.
    UsageUpdated {
        /// The usage block.
        usage: Usage,
    },
    /// A provider warning.
    ProviderWarning {
        /// A bounded, user-safe warning code.
        code: String,
    },
    /// Terminal: the call completed.
    CallCompleted {
        /// Why it finished.
        finish_reason: FinishReason,
        /// The final usage, when the provider reported it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// Whether the provider flagged a safety or refusal outcome.
        refused: bool,
    },
    /// Terminal: the call failed.
    CallFailed {
        /// A stable, namespaced error code.
        code: String,
        /// Whether the failure is safe to retry unchanged.
        retryable: bool,
    },
    /// Terminal: the call was cancelled.
    CallCancelled {
        /// Whether a provider frame arrived after the local cancellation.
        late_frames_ignored: u64,
    },
}

impl ModelStreamEventKind {
    /// Returns the wire name of this event type.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::CallStarted { .. } => "call.started",
            Self::OutputItemAdded { .. } => "output.item.added",
            Self::OutputTextDelta { .. } => "output.text.delta",
            Self::OutputItemCompleted { .. } => "output.item.completed",
            Self::ToolCallAdded { .. } => "tool.call.added",
            Self::ToolCallArgumentsDelta { .. } => "tool.call.arguments.delta",
            Self::ToolCallCompleted { .. } => "tool.call.completed",
            Self::ReasoningSummaryDelta { .. } => "reasoning.summary.delta",
            Self::UsageUpdated { .. } => "usage.updated",
            Self::ProviderWarning { .. } => "provider.warning",
            Self::CallCompleted { .. } => "call.completed",
            Self::CallFailed { .. } => "call.failed",
            Self::CallCancelled { .. } => "call.cancelled",
        }
    }

    /// Returns whether this is a terminal event.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::CallCompleted { .. } | Self::CallFailed { .. } | Self::CallCancelled { .. }
        )
    }
}

/// One normalized stream envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelStreamEvent {
    /// The logical call this belongs to.
    pub call_id: ModelCallId,
    /// This event's identifier.
    pub event_id: ModelStreamEventId,
    /// The monotonic position in the stream.
    pub sequence: Sequence,
    /// The event payload.
    #[serde(flatten)]
    pub kind: ModelStreamEventKind,
    /// Provider metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ModelStreamEvent {
    /// Returns whether this event is terminal.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.kind.is_terminal()
    }
}

/// The result of offering one frame to the stream state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamAdmission {
    /// The frame was accepted into the stream.
    Accepted,
    /// The frame arrived after the local terminal state and was ignored.
    ///
    /// Not an error: the contract requires late provider frames to be ignored and
    /// counted, because a provider may flush a few frames after a local cancel.
    IgnoredAfterTerminal,
}

/// How a stream ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The stream reached a terminal event.
    Terminal(FinishReason),
    /// The stream ended with no terminal event.
    ///
    /// This is **not** success. The contract is explicit that a stream ending
    /// without a terminal event is an interrupted call, so the type keeps the two
    /// apart rather than reporting a boolean completion.
    Interrupted {
        /// How many frames were accepted before the stream ended.
        accepted: u64,
    },
}

impl StreamOutcome {
    /// Returns whether the call reached a terminal event.
    ///
    /// Named so a caller cannot read `Interrupted` as success by accident, and so
    /// the one place that decides "did this call finish" is testable.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal(_))
    }
}

/// The state machine that enforces the stream's ordering and terminal rules.
#[derive(Debug, Clone)]
pub struct ModelStreamState {
    call_id: ModelCallId,
    last_sequence: Sequence,
    started: bool,
    terminal: Option<FinishReason>,
    accepted: u64,
    late_frames_ignored: u64,
    open_tool_calls: BTreeMap<String, String>,
    assembled_arguments: BTreeMap<String, String>,
}

impl ModelStreamState {
    /// Creates state for a stream of `call_id`.
    #[must_use]
    pub fn new(call_id: ModelCallId) -> Self {
        Self {
            call_id,
            last_sequence: Sequence::FIRST,
            started: false,
            terminal: None,
            accepted: 0,
            late_frames_ignored: 0,
            open_tool_calls: BTreeMap::new(),
            assembled_arguments: BTreeMap::new(),
        }
    }

    /// Offers one frame to the stream.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the frame does not belong to this call, when the
    /// sequence is not strictly increasing, when a second stream start arrives,
    /// when an argument delta names a tool call that was never added, when a
    /// completion names one that is not open, or when the assembled deltas do not
    /// match the completion payload.
    pub fn accept(&mut self, event: &ModelStreamEvent) -> Result<StreamAdmission, DomainError> {
        if event.call_id != self.call_id {
            return Err(DomainError::StreamCallMismatch);
        }
        if self.terminal.is_some() {
            // Required behaviour, not a defect: a late provider frame after the
            // local terminal state is ignored and counted so cancellation stays
            // truthful without turning a provider flush into failure.
            self.late_frames_ignored += 1;
            return Ok(StreamAdmission::IgnoredAfterTerminal);
        }
        if event.sequence <= self.last_sequence {
            // A duplicate or out-of-order frame is refused rather than applied,
            // because applying it fabricates a transcript that never happened.
            return Err(DomainError::StreamSequenceNotMonotonic);
        }
        self.apply(&event.kind)?;
        self.last_sequence = event.sequence;
        self.accepted += 1;
        if event.kind.is_terminal() {
            self.terminal = terminal_reason(&event.kind);
        }
        Ok(StreamAdmission::Accepted)
    }

    /// Applies one payload-level check to a frame whose sequence was accepted.
    fn apply(&mut self, kind: &ModelStreamEventKind) -> Result<(), DomainError> {
        match kind {
            ModelStreamEventKind::CallStarted { .. } => {
                if self.started {
                    return Err(DomainError::StreamAlreadyStarted);
                }
                self.started = true;
            }
            ModelStreamEventKind::ToolCallAdded { call_id, tool_name } => {
                self.assembled_arguments
                    .insert(call_id.clone(), String::new());
                self.open_tool_calls
                    .insert(call_id.clone(), tool_name.clone());
            }
            ModelStreamEventKind::ToolCallArgumentsDelta { call_id, delta } => {
                let buffer = self
                    .assembled_arguments
                    .get_mut(call_id)
                    .ok_or(DomainError::UnknownToolCall)?;
                buffer.push_str(delta);
            }
            ModelStreamEventKind::ToolCallCompleted { call_id, arguments } => {
                if self.open_tool_calls.remove(call_id).is_none() {
                    return Err(DomainError::UnknownToolCall);
                }
                // The contract requires the assembled deltas and the completion to
                // describe the same arguments; a mismatch means a frame was lost,
                // so the call is refused rather than executed on partial input.
                let assembled = self.assembled_arguments.remove(call_id);
                if assembled.as_deref().is_some_and(|seen| seen != arguments) {
                    return Err(DomainError::ToolArgumentsMismatch);
                }
            }
            ModelStreamEventKind::OutputItemAdded { .. }
            | ModelStreamEventKind::OutputTextDelta { .. }
            | ModelStreamEventKind::OutputItemCompleted { .. }
            | ModelStreamEventKind::ReasoningSummaryDelta { .. }
            | ModelStreamEventKind::UsageUpdated { .. }
            | ModelStreamEventKind::ProviderWarning { .. }
            | ModelStreamEventKind::CallCompleted { .. }
            | ModelStreamEventKind::CallFailed { .. }
            | ModelStreamEventKind::CallCancelled { .. } => {}
        }
        Ok(())
    }

    /// Returns the terminal reason when the stream has ended.
    #[must_use]
    pub const fn terminal_reason(&self) -> Option<&FinishReason> {
        self.terminal.as_ref()
    }

    /// Returns how many frames arrived after the terminal state and were ignored.
    #[must_use]
    pub const fn late_frames_ignored(&self) -> u64 {
        self.late_frames_ignored
    }

    /// Returns how many frames were accepted.
    #[must_use]
    pub const fn accepted_frames(&self) -> u64 {
        self.accepted
    }

    /// Returns the tool calls that were added but never completed.
    #[must_use]
    pub fn open_tool_call_ids(&self) -> Vec<&str> {
        self.open_tool_calls.keys().map(String::as_str).collect()
    }

    /// Closes the stream and reports how it ended.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::UnfinishedToolCall`] when a tool call was added but
    /// never completed, because the call would otherwise become executable on
    /// partial arguments.
    pub fn finish(&self) -> Result<StreamOutcome, DomainError> {
        if !self.open_tool_calls.is_empty() {
            return Err(DomainError::UnfinishedToolCall);
        }
        match &self.terminal {
            Some(reason) => Ok(StreamOutcome::Terminal(reason.clone())),
            None => Ok(StreamOutcome::Interrupted {
                accepted: self.accepted,
            }),
        }
    }
}

/// Extracts the normalized terminal reason from a terminal event.
fn terminal_reason(kind: &ModelStreamEventKind) -> Option<FinishReason> {
    match kind {
        ModelStreamEventKind::CallCompleted { finish_reason, .. } => Some(finish_reason.clone()),
        ModelStreamEventKind::CallFailed { .. } => Some(FinishReason::ProviderError),
        ModelStreamEventKind::CallCancelled { .. } => Some(FinishReason::Cancelled),
        _ => None,
    }
}

/// A user-safe rendering of a terminal outcome.
///
/// Exists so a caller can report what happened without re-deriving it from the
/// event, and so "interrupted" cannot be displayed as success.
impl fmt::Display for StreamOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Terminal(reason) => write!(formatter, "terminal: {reason:?}"),
            Self::Interrupted { accepted } => {
                write!(formatter, "interrupted after {accepted} frame(s)")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CallLimits, FinishReason, InputItem, InputItems, JsonText, MAX_JSON_TEXT_BYTES,
        MAX_PROVIDER_STRING_BYTES, Modality, ModelCallRequest, ModelStreamEvent,
        ModelStreamEventKind, ModelStreamState, Role, RouteRequirements, Sequence, StreamAdmission,
        StreamOutcome, ToolArguments, Usage,
    };
    use crate::ids::{ModelCallId, ModelStreamEventId, RunId};
    use crate::model::capability::Capability;
    use crate::model::policy::Locality;

    fn call() -> ModelCallId {
        ModelCallId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d").expect("valid")
    }

    fn other_call() -> ModelCallId {
        ModelCallId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5e").expect("valid")
    }

    fn event(sequence: u64, kind: ModelStreamEventKind) -> ModelStreamEvent {
        ModelStreamEvent {
            call_id: call(),
            event_id: ModelStreamEventId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c60")
                .expect("valid"),
            sequence: Sequence::new(sequence),
            kind,
            provider_metadata: None,
        }
    }

    fn delta(sequence: u64, text: &str) -> ModelStreamEvent {
        event(
            sequence,
            ModelStreamEventKind::OutputTextDelta {
                item_id: "out-1".to_owned(),
                delta: text.to_owned(),
            },
        )
    }

    #[test]
    fn a_well_formed_stream_ends_terminal() {
        let mut state = ModelStreamState::new(call());
        assert_eq!(
            state
                .accept(&event(1, ModelStreamEventKind::CallStarted { model: None },))
                .expect("the first frame is accepted"),
            StreamAdmission::Accepted,
        );
        state.accept(&delta(2, "Hel")).expect("accepted");
        state.accept(&delta(3, "lo")).expect("accepted");
        state
            .accept(&event(
                4,
                ModelStreamEventKind::CallCompleted {
                    finish_reason: FinishReason::Stop,
                    usage: Some(Usage {
                        output_tokens: Some(2),
                        provider_reported: true,
                        ..Usage::default()
                    }),
                    refused: false,
                },
            ))
            .expect("the terminal frame is accepted");

        let outcome = state.finish().expect("no unfinished tool call");
        assert!(outcome.is_terminal());
        assert_eq!(outcome, StreamOutcome::Terminal(FinishReason::Stop));
        assert_eq!(state.accepted_frames(), 4);
    }

    #[test]
    fn an_out_of_order_or_duplicate_sequence_is_refused() {
        let mut state = ModelStreamState::new(call());
        state.accept(&delta(1, "a")).expect("accepted");
        state.accept(&delta(2, "b")).expect("accepted");

        for sequence in [1, 2] {
            let error = state
                .accept(&delta(sequence, "c"))
                .expect_err("a repeated sequence must be refused");
            assert_eq!(error.code(), "jarvis.stream_sequence_not_monotonic");
        }
    }

    #[test]
    fn a_lower_sequence_after_a_higher_one_is_refused_not_reordered() {
        let mut state = ModelStreamState::new(call());
        state.accept(&delta(5, "a")).expect("accepted");
        let error = state
            .accept(&delta(3, "b"))
            .expect_err("a reordered frame must be refused");
        assert_eq!(error.code(), "jarvis.stream_sequence_not_monotonic");
    }

    #[test]
    fn a_frame_for_another_call_is_refused() {
        let mut state = ModelStreamState::new(call());
        let mut frame = delta(1, "a");
        frame.call_id = other_call();
        let error = state.accept(&frame).expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.stream_call_mismatch");
    }

    #[test]
    fn a_second_stream_start_is_refused() {
        let mut state = ModelStreamState::new(call());
        state
            .accept(&event(1, ModelStreamEventKind::CallStarted { model: None }))
            .expect("accepted");
        let error = state
            .accept(&event(2, ModelStreamEventKind::CallStarted { model: None }))
            .expect_err("a second start must be refused");
        assert_eq!(error.code(), "jarvis.stream_already_started");
    }

    #[test]
    fn late_frames_after_the_terminal_state_are_ignored_and_counted() {
        // The contract requires late provider frames to be ignored, not treated as
        // a fault: a provider may flush after a local cancellation.
        let mut state = ModelStreamState::new(call());
        state
            .accept(&event(
                1,
                ModelStreamEventKind::CallCancelled {
                    late_frames_ignored: 0,
                },
            ))
            .expect("accepted");

        for sequence in [2, 3] {
            assert_eq!(
                state.accept(&delta(sequence, "late")).expect("ignored"),
                StreamAdmission::IgnoredAfterTerminal,
            );
        }
        assert_eq!(state.late_frames_ignored(), 2);
        assert_eq!(
            state.terminal_reason(),
            Some(&FinishReason::Cancelled),
            "the terminal state must not change",
        );
        assert_eq!(state.accepted_frames(), 1, "late frames are not accepted");
    }

    #[test]
    fn a_stream_without_a_terminal_event_is_interrupted_not_successful() {
        let mut state = ModelStreamState::new(call());
        state.accept(&delta(1, "partial")).expect("accepted");
        state.accept(&delta(2, " text")).expect("accepted");

        let outcome = state.finish().expect("no unfinished tool call");
        assert!(!outcome.is_terminal(), "interrupted is not success");
        assert_eq!(outcome, StreamOutcome::Interrupted { accepted: 2 });
        assert!(outcome.to_string().contains("interrupted"), "{outcome}");
    }

    #[test]
    fn argument_deltas_are_assembled_and_the_completion_must_match() {
        let mut state = ModelStreamState::new(call());
        state
            .accept(&event(
                1,
                ModelStreamEventKind::ToolCallAdded {
                    call_id: "call-1".to_owned(),
                    tool_name: "fs.read".to_owned(),
                },
            ))
            .expect("accepted");
        state
            .accept(&event(
                2,
                ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id: "call-1".to_owned(),
                    delta: "{\"path\":".to_owned(),
                },
            ))
            .expect("accepted");
        state
            .accept(&event(
                3,
                ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id: "call-1".to_owned(),
                    delta: "\"/tmp\"}".to_owned(),
                },
            ))
            .expect("accepted");

        // A completion that disagrees with the deltas means a frame was lost.
        let error = state
            .accept(&event(
                4,
                ModelStreamEventKind::ToolCallCompleted {
                    call_id: "call-1".to_owned(),
                    arguments: "{\"path\":\"/tmp\"]}".to_owned(),
                },
            ))
            .expect_err("a mismatched completion must be refused");
        assert_eq!(error.code(), "jarvis.tool_arguments_mismatch");
    }

    #[test]
    fn a_matching_completion_closes_the_call() {
        let mut state = ModelStreamState::new(call());
        state
            .accept(&event(
                1,
                ModelStreamEventKind::ToolCallAdded {
                    call_id: "call-1".to_owned(),
                    tool_name: "fs.read".to_owned(),
                },
            ))
            .expect("accepted");
        state
            .accept(&event(
                2,
                ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id: "call-1".to_owned(),
                    delta: "{}".to_owned(),
                },
            ))
            .expect("accepted");
        state
            .accept(&event(
                3,
                ModelStreamEventKind::ToolCallCompleted {
                    call_id: "call-1".to_owned(),
                    arguments: "{}".to_owned(),
                },
            ))
            .expect("completed");
        assert!(state.open_tool_call_ids().is_empty());
    }

    #[test]
    fn an_argument_delta_for_an_unknown_call_is_refused() {
        let mut state = ModelStreamState::new(call());
        let error = state
            .accept(&event(
                1,
                ModelStreamEventKind::ToolCallArgumentsDelta {
                    call_id: "never-added".to_owned(),
                    delta: "{}".to_owned(),
                },
            ))
            .expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.unknown_tool_call");
    }

    #[test]
    fn finishing_with_an_uncompleted_tool_call_is_refused() {
        let mut state = ModelStreamState::new(call());
        state
            .accept(&event(
                1,
                ModelStreamEventKind::ToolCallAdded {
                    call_id: "call-1".to_owned(),
                    tool_name: "fs.read".to_owned(),
                },
            ))
            .expect("accepted");
        state
            .accept(&event(
                2,
                ModelStreamEventKind::CallCompleted {
                    finish_reason: FinishReason::ToolCalls,
                    usage: None,
                    refused: false,
                },
            ))
            .expect("accepted");

        assert_eq!(state.open_tool_call_ids(), vec!["call-1"]);
        let error = state.finish().expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.unfinished_tool_call");
    }

    #[test]
    fn a_streaming_tool_call_exposes_no_executable_arguments() {
        let streaming = ToolArguments::Streaming {
            raw: "{\"path\":".to_owned(),
        };
        assert!(!streaming.is_complete());
        assert_eq!(streaming.executable_raw(), None);
        assert_eq!(streaming.raw_for_assembly(), "{\"path\":");

        let complete = ToolArguments::Complete {
            raw: "{\"path\":\"/tmp\"}".to_owned(),
        };
        assert!(complete.is_complete());
        assert_eq!(complete.executable_raw(), Some("{\"path\":\"/tmp\"}"));
    }

    #[test]
    fn usage_distinguishes_unreported_from_zero() {
        let unreported = Usage::default();
        assert!(!unreported.has_any_counter());

        let reported_zero = Usage {
            output_tokens: Some(0),
            provider_reported: true,
            ..Usage::default()
        };
        assert!(reported_zero.has_any_counter());

        // The distinction survives serialization: absent stays absent.
        let json = serde_json::to_string(&unreported).expect("serializes");
        assert!(!json.contains("input_tokens"), "{json}");
        assert!(!json.contains("estimated_cost"), "{json}");
    }

    #[test]
    fn a_tool_result_without_its_call_is_refused() {
        let orphaned = vec![
            InputItem::Message {
                role: Role::Assistant,
                blocks: Vec::new(),
            },
            InputItem::ToolResult {
                call_id: "call-1".to_owned(),
                is_error: false,
                content: "ok".to_owned(),
            },
        ];
        let error = InputItems::new(orphaned).expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.orphaned_tool_result");
    }

    #[test]
    fn a_tool_result_that_precedes_its_call_is_refused() {
        // Order-sensitive on purpose: a result must follow the call it answers.
        let reordered = vec![
            InputItem::ToolResult {
                call_id: "call-1".to_owned(),
                is_error: false,
                content: "ok".to_owned(),
            },
            InputItem::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "fs.read".to_owned(),
                arguments: ToolArguments::Complete {
                    raw: "{}".to_owned(),
                },
            },
        ];
        let error = InputItems::new(reordered).expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.orphaned_tool_result");
    }

    #[test]
    fn a_paired_call_and_result_is_accepted() {
        let items = InputItems::new(vec![
            InputItem::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "fs.read".to_owned(),
                arguments: ToolArguments::Complete {
                    raw: "{\"path\":\"/tmp\"}".to_owned(),
                },
            },
            InputItem::ToolResult {
                call_id: "call-1".to_owned(),
                is_error: false,
                content: "contents".to_owned(),
            },
        ])
        .expect("a paired call and result is the supported shape");
        assert_eq!(items.len(), 2);
        assert!(!items.is_empty());
    }

    #[test]
    fn a_duplicate_call_id_is_refused_so_a_result_is_never_ambiguous() {
        let duplicated = vec![
            InputItem::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "fs.read".to_owned(),
                arguments: ToolArguments::Complete {
                    raw: "{}".to_owned(),
                },
            },
            InputItem::ToolCall {
                call_id: "call-1".to_owned(),
                tool_name: "fs.write".to_owned(),
                arguments: ToolArguments::Complete {
                    raw: "{}".to_owned(),
                },
            },
        ];
        let error = InputItems::new(duplicated).expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.duplicate_tool_call_id");
    }

    #[test]
    fn a_deserialized_item_list_is_held_to_the_same_pairing_rule() {
        // The invariant must hold for a payload that arrived over the wire, not
        // only for a value built by a caller inside this crate: a derived
        // deserializer would bypass `new` and accept an orphaned result.
        let json = r#"[{"kind":"tool_result","call_id":"call-1","is_error":false,"content":"ok"}]"#;
        let error = serde_json::from_str::<InputItems>(json)
            .expect_err("an orphaned result must not deserialize");
        assert!(
            error.to_string().contains("tool result"),
            "the refusal must name the cause: {error}",
        );

        let paired = r#"[{"kind":"tool_call","call_id":"call-1","tool_name":"fs.read","arguments":{"state":"complete","raw":"{}"}},{"kind":"tool_result","call_id":"call-1","is_error":false,"content":"ok"}]"#;
        let items: InputItems =
            serde_json::from_str(paired).expect("a paired list must deserialize");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn an_unknown_finish_reason_is_preserved_rather_than_flattened() {
        let reason = FinishReason::other("content_filter_extended").expect("a valid reason");
        assert_ne!(
            reason,
            FinishReason::Stop,
            "an unknown terminal must not look like a clean one",
        );
        assert_eq!(
            reason,
            FinishReason::Other {
                provider_value: "content_filter_extended".to_owned(),
            },
        );
    }

    #[test]
    fn an_empty_or_over_long_provider_string_is_refused() {
        assert_eq!(
            FinishReason::other("").expect_err("must be refused").code(),
            "jarvis.unbounded_provider_value",
        );
        let too_long = "a".repeat(MAX_PROVIDER_STRING_BYTES + 1);
        assert_eq!(
            FinishReason::other(&too_long)
                .expect_err("must be refused")
                .code(),
            "jarvis.unbounded_provider_value",
        );
        assert!(
            FinishReason::other(&"a".repeat(MAX_PROVIDER_STRING_BYTES)).is_ok(),
            "exactly at the bound is inside it",
        );
    }

    #[test]
    fn provider_metadata_is_bounded_on_validation() {
        let metadata = super::ProviderMetadata {
            request_id: Some("req-1".to_owned()),
            continuation_ref: None,
            provider_finish_reason: Some("a".repeat(MAX_PROVIDER_STRING_BYTES + 1)),
        };
        let error = metadata.validated().expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.unbounded_provider_value");

        let ok = super::ProviderMetadata {
            request_id: Some("req-1".to_owned()),
            continuation_ref: Some("cont-1".to_owned()),
            provider_finish_reason: None,
        }
        .validated()
        .expect("bounded values are accepted");
        assert_eq!(ok.request_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn a_json_document_is_bounded_and_never_empty() {
        assert!(JsonText::new("{\"type\":\"object\"}").is_ok());
        assert_eq!(
            JsonText::new("").expect_err("must be refused").code(),
            "jarvis.unbounded_provider_value",
        );
        assert_eq!(
            JsonText::new(&"a".repeat(MAX_JSON_TEXT_BYTES + 1))
                .expect_err("must be refused")
                .code(),
            "jarvis.unbounded_provider_value",
        );
        assert_eq!(
            JsonText::new("{}").expect("accepted").as_str(),
            "{}",
            "the document is carried verbatim",
        );
    }

    #[test]
    fn event_type_names_match_the_contract_list() {
        let names = [
            ModelStreamEventKind::CallStarted { model: None }.type_name(),
            ModelStreamEventKind::OutputItemAdded {
                item_id: "i".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::OutputTextDelta {
                item_id: "i".to_owned(),
                delta: "d".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::OutputItemCompleted {
                item_id: "i".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::ToolCallAdded {
                call_id: "c".to_owned(),
                tool_name: "t".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::ToolCallArgumentsDelta {
                call_id: "c".to_owned(),
                delta: "d".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::ToolCallCompleted {
                call_id: "c".to_owned(),
                arguments: "{}".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::ReasoningSummaryDelta {
                delta: "d".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::UsageUpdated {
                usage: Usage::default(),
            }
            .type_name(),
            ModelStreamEventKind::ProviderWarning {
                code: "c".to_owned(),
            }
            .type_name(),
            ModelStreamEventKind::CallCompleted {
                finish_reason: FinishReason::Stop,
                usage: None,
                refused: false,
            }
            .type_name(),
            ModelStreamEventKind::CallFailed {
                code: "model.failed".to_owned(),
                retryable: true,
            }
            .type_name(),
            ModelStreamEventKind::CallCancelled {
                late_frames_ignored: 0,
            }
            .type_name(),
        ];
        assert_eq!(
            names,
            [
                "call.started",
                "output.item.added",
                "output.text.delta",
                "output.item.completed",
                "tool.call.added",
                "tool.call.arguments.delta",
                "tool.call.completed",
                "reasoning.summary.delta",
                "usage.updated",
                "provider.warning",
                "call.completed",
                "call.failed",
                "call.cancelled",
            ],
        );
    }

    #[test]
    fn a_normalized_request_has_no_provider_extension_map() {
        // The contract makes provider extensions adapter-owned, so the portable
        // request must not carry an arbitrary field for them.
        let request = ModelCallRequest {
            call_id: call(),
            run_id: RunId::parse("018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c61").expect("valid"),
            route_requirements: RouteRequirements::text(),
            input: InputItems::new(Vec::new()).expect("an empty list is valid"),
            tools: Vec::new(),
            output_schema: None,
            limits: CallLimits {
                deadline: None,
                max_output_tokens: Some(2048),
                max_cost_microunits: None,
            },
        };
        let json = serde_json::to_string(&request).expect("serializes");
        assert!(
            !json.contains("extension") && !json.contains("provider_"),
            "the portable request must have no provider-owned field: {json}",
        );
        assert_eq!(
            request.route_requirements.modalities,
            [Modality::Text].into_iter().collect(),
        );
        assert!(request.limits.max_cost_microunits.is_none());
    }

    #[test]
    fn a_route_requirement_uses_the_same_capability_vocabulary_as_a_descriptor() {
        // The point of sharing the vocabulary: a requirement is expressed as a
        // capability key that a descriptor can attest, so no requirement can exist
        // that nothing could ever satisfy.
        let requirements = RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            required_capabilities: [Capability::IncrementalDelivery, Capability::ToolCalling]
                .into_iter()
                .collect(),
            locality: Locality::LocalOnly,
        };
        assert!(requirements.requires_incremental_delivery());
        assert!(requirements.requires_tool_calling());
        assert!(requirements.requires_local_only());
        assert!(
            !RouteRequirements::text().requires_incremental_delivery(),
            "a plain text call must not require measured incremental delivery",
        );
    }

    #[test]
    fn a_sequence_at_the_ceiling_refuses_to_wrap() {
        let maximum = Sequence::new(u64::MAX);
        let error = maximum.next().expect_err("must not wrap");
        assert_eq!(error.code(), "jarvis.stream_sequence_exhausted");
        assert_eq!(Sequence::new(0).next().expect("advances").get(), 1);
    }
}
