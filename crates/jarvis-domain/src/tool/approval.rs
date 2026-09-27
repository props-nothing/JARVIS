//! Durable approval records: their lifecycle, their state machine, and what a preview may say.
//!
//! `TLS-005` asks for "durable approval records and action fingerprinting". Two of those are here,
//! and the third deliberately is not: [the fingerprint](crate::tool::policy::ApprovalRecord) is a
//! **type** in this module, but *computing* one needs a canonicalization whose specification has not
//! been reviewed, and inventing one would put an unreviewed canonicalization underneath every
//! approval decision in the product. That gap is named in `TLS-005` rather than papered over.
//!
//! The state machine is transcribed from `docs/contracts/approval-contract.md`, and its single
//! non-obvious rule is enforced by the shape of [`ApprovalState::can_transition_to`]: **terminal
//! states are absorbing**. "Terminal decisions are immutable" is a sentence about the transition
//! table, not a comment, so a rejected approval cannot later become approved and a consumed one
//! cannot be consumed again — the second of which is the difference between one email and two.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{ApprovalId, PrincipalId, RunId, ToolCallId, WorkspaceId};
use crate::time::UtcTimestamp;

use super::identity::ToolIdentity;

/// The longest accepted approval summary.
///
/// A summary is shown to the user in a prompt, so it is bounded where it is *accepted* and it is
/// not allowed to carry control characters — a prompt is a security boundary in this project, and a
/// summary that could emit terminal escapes into it would be the one place a preview could lie.
pub const MAX_SUMMARY_BYTES: usize = 512;

/// The largest number of channels one approval may be offered on.
pub const MAX_APPROVAL_CHANNELS: usize = 8;

/// The longest accepted channel name.
pub const MAX_CHANNEL_BYTES: usize = 32;

/// The largest number of preview items one approval may carry.
pub const MAX_PREVIEW_ITEMS: usize = 64;

/// The longest accepted preview key or value.
pub const MAX_PREVIEW_TEXT_BYTES: usize = 1024;

/// The longest accepted decision note (a decision's comment, or a cancellation's reason).
///
/// **Bounded because it is caller-supplied text that reaches a durable row and an operator display.** It
/// is deliberately *not* content: the contract's decision body calls the comment "an operator comment" and
/// its cancellation body a "reason code", so the value names *why* a human decided rather than carrying
/// anything about the action. 512 bytes is the same bound the run cancel reason uses, because the two are
/// the same kind of value and two bounds for one kind of thing is how one comes to be raised alone.
pub const MAX_DECISION_NOTE_BYTES: usize = 512;

/// A bounded, non-interpreted note attached to a decision or a cancellation.
///
/// **A type rather than a `String`, and the reason is a defect this round found.** `DecideApprovalRequest`
/// carried a `comment` whose doc said "stored with the decision", and nothing stored it: the field was
/// deserialized, bounded by nothing, and dropped. The same was true of the cancellation's `reason`. A type
/// makes the value *constructible only through its own bound*, so a caller cannot pass one that was never
/// validated and a writer cannot omit validation by forgetting to call it.
///
/// The text is **never interpreted** — not parsed, not matched, not rendered into a prompt — which is why it
/// may contain a newline: an operator writing "asked Bob, he said no" may write two lines. A control
/// character other than whitespace is refused, because the value is echoed in an operator display and a
/// record whose note can emit terminal escapes is the one place a stored audit value could lie about
/// itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DecisionNote(String);

impl DecisionNote {
    /// Validates and wraps a note.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `note` when the value is empty, over
    /// [`MAX_DECISION_NOTE_BYTES`], or contains a control character outside `\t`, `\n`, and `\r`.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > MAX_DECISION_NOTE_BYTES {
            return Err(DomainError::ToolDefinitionInvalid { field: "note" });
        }
        // `is_control` is true for `\t`, `\n`, and `\r` as well, so the allowed three are excluded
        // explicitly rather than by a range comparison — a range would be unreadable and would have to be
        // edited for every character someone later wanted to allow.
        let has_disallowed_control = value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\t' | '\n' | '\r'));
        if has_disallowed_control {
            return Err(DomainError::ToolDefinitionInvalid { field: "note" });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the note's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DecisionNote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DecisionNote {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived**, the systemic defect `wire_validation_tests`
        // records for nine other types in this crate: a derived impl on a transparent newtype wraps the
        // inner value directly, so a note that arrived over the wire would bypass the bound the constructor
        // enforces — and a note is exactly the value a caller controls.
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

/// A version counter for an approval record's optimistic concurrency.
///
/// Newtyped for the same reason [`crate::run::state::RunVersion`] is: a version and a count are both
/// "a number", and passing one where the other is required is silent. The first version is `1`
/// rather than `0` so an uninitialised field cannot read as a valid stored version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalVersion(u64);

impl ApprovalVersion {
    /// The version a freshly created approval has.
    pub const FIRST: Self = Self(1);

    /// Wraps a stored value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the inner value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next version.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ApprovalVersionConflict`] when the counter is exhausted, which is
    /// unreachable in practice and is a typed refusal rather than a wrapping add: a version that
    /// wrapped would let a stale writer match a current version and overwrite a decision.
    pub fn next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::ApprovalVersionConflict {
                expected: self,
                actual: self,
            })
    }
}

impl fmt::Display for ApprovalVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Where an approval is in its lifecycle.
///
/// The seven states the approval contract lists, and the two that are **terminal** are the pair the
/// contract calls so: a decision, once taken, is not reconsidered. `Consumed` and `Invalidated` are
/// terminal as well, because a one-shot approval that has been spent cannot be un-spent and a
/// decision whose action changed cannot be un-changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// Waiting for a decision.
    Pending,
    /// Approved and not yet spent.
    Approved,
    /// Refused.
    Rejected,
    /// Lapsed before a decision or before use.
    Expired,
    /// Withdrawn before a decision.
    Cancelled,
    /// Spent by the exact tool call it authorized.
    Consumed,
    /// The action it authorized is no longer the action being performed.
    Invalidated,
}

impl ApprovalState {
    /// Returns whether the state is terminal.
    ///
    /// A **decision** is terminal: `Approved` is not, because it is a decision waiting to be spent,
    /// while `Rejected` is. The pair is easy to get backwards, so the classification is stated once
    /// here and every caller asks this method rather than re-deriving it.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Rejected | Self::Expired | Self::Cancelled | Self::Consumed | Self::Invalidated
        )
    }

    /// Returns whether a transition from `self` to `to` is legal.
    ///
    /// Two rules, and the second is the one that needs saying:
    ///
    /// 1. **Terminal states are absorbing.** Nothing leaves `Rejected`, `Expired`, `Cancelled`,
    ///    `Consumed`, or `Invalidated`. This is the contract's "terminal decisions are immutable"
    ///    made structural, and it is what stops a consumed one-shot approval from authorizing a
    ///    second call.
    /// - **`Pending` is the only source of a decision**, and every decision is reachable from it.
    ///   `Approved` is deliberately the sole source of `Consumed` and `Invalidated`, because an
    ///   approval that was never given cannot be spent or superseded.
    ///
    /// A no-op transition to the **same** state is refused, including for a terminal state. It is
    /// not a legal edge, because accepting it would let a second consume of a spent approval record
    /// a transition that looks like progress while the effect stays single — the audit trail would
    /// claim two reservations for one call.
    ///
    /// **⚠ Absorption is enforced by the match table, not by a guard above it.** A previous version
    /// began `if self.is_terminal() { return false; }` and then listed the legal edges — and disabling
    /// that guard was a mutation that **compiled and passed every test**, because no arm in the table
    /// has a terminal source, so the guard was unreachable. It read as enforcement while enforcing
    /// nothing, which is the same "exists but enforced nowhere" shape as an unused constant. The table
    /// below now pairs each legal edge with `false` for everything else, so absorbing is a *property
    /// of the data* rather than a branch that can be silently deleted.
    #[must_use]
    pub const fn can_transition_to(self, to: Self) -> bool {
        // Spelled as one table rather than an early return plus a list, so absorbing is a property of
        // the data rather than a branch that can be silently deleted.
        //
        // Seven edges, and `Rejected` is reachable **only** from `Pending`. That is the one boundary a
        // grouped pattern must not blur: `Approved` reaches the three outcomes describing a *granted*
        // approval ending (spent, invalidated, lapsed) but not `Rejected`, because a rejection is a
        // decision that was never taken once an approval was given. Grouping the two sources with one
        // target list was a first attempt here, and the exhaustive table test caught it immediately.
        matches!(
            (self, to),
            (
                Self::Pending,
                Self::Approved | Self::Rejected | Self::Expired | Self::Cancelled
            ) | (
                Self::Approved,
                Self::Consumed | Self::Invalidated | Self::Expired
            )
        )
    }

    /// Returns whether `self` can be left at all.
    ///
    /// The complement of [`Self::is_terminal`], stated separately so the absorbing rule has one
    /// consultable definition and a test can assert the two agree rather than re-deriving the list.
    /// A state that is non-terminal must have **some** legal target, which is the property that keeps
    /// an approval from sitting somewhere with no way out.
    #[must_use]
    pub const fn can_advance(self) -> bool {
        !self.is_terminal()
    }

    /// Returns whether any target is reachable from `self`.
    ///
    /// Used by a test that asserts every non-terminal state has at least one legal edge, because the
    /// failure this guards against is an approval that can never be decided — the "no legal way out"
    /// shape this project has found six times. It is a *method* rather than a test-local loop so the
    /// property is checked against the same table `can_transition_to` answers from.
    #[must_use]
    pub const fn has_any_transition(self) -> bool {
        matches!(self, Self::Pending | Self::Approved)
    }

    /// Returns the spelling the approval contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Consumed => "consumed",
            Self::Invalidated => "invalidated",
        }
    }

    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `state` for any other value. An
    /// unrecognised stored state must not be read as `Pending`, which is the fail-open direction for
    /// a decision.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            "consumed" => Ok(Self::Consumed),
            "invalidated" => Ok(Self::Invalidated),
            _ => Err(DomainError::ToolDefinitionInvalid { field: "state" }),
        }
    }
}

impl fmt::Display for ApprovalState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// Whether an approval authorizes one call or a repeated pattern.
///
/// The contract says an approval authorizes "one exact action **or** a narrowly defined standing
/// rule", and the distinction is what the executor needs to decide whether the approval is spent on
/// use. It is a separate field rather than a state, because both kinds pass through `Approved` and
/// end in different terminal states: a one-shot ends `Consumed`, a standing one ends `Expired` or
/// `Invalidated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScopeKind {
    /// Authorizes exactly one call with this fingerprint, spent when it is reserved.
    OneShot,
    /// Authorizes a narrowly defined pattern until it expires or is revoked.
    Standing,
}

impl ApprovalScopeKind {
    /// Returns whether the approval is spent by the call it authorizes.
    #[must_use]
    pub const fn is_consumed_on_use(self) -> bool {
        matches!(self, Self::OneShot)
    }

    /// Returns the spelling the approval contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::Standing => "standing",
        }
    }
}

/// A channel an approval may be decided on.
///
/// A closed set, and it is closed on purpose: the contract lists `allowed_channels` in an approval
/// request and requires the decision to come **from** one of them, so a channel outside this set
/// cannot be validated against and must not be accepted. The set is the contract's own example
/// (`["cli", "desktop"]`) plus the surfaces this build can plausibly serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChannel {
    /// The command-line client.
    Cli,
    /// The desktop application.
    Desktop,
    /// A mobile client.
    Mobile,
    /// A voice interaction.
    Voice,
    /// The HTTP API.
    Api,
}

impl ApprovalChannel {
    /// Returns the spelling the approval contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
            Self::Voice => "voice",
            Self::Api => "api",
        }
    }

    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `allowed_channels`. A channel JARVIS
    /// cannot verify is refused rather than carried, because the whole point of recording the
    /// channel is to check where a decision came from.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "cli" => Ok(Self::Cli),
            "desktop" => Ok(Self::Desktop),
            "mobile" => Ok(Self::Mobile),
            "voice" => Ok(Self::Voice),
            "api" => Ok(Self::Api),
            _ => Err(DomainError::ToolDefinitionInvalid {
                field: "allowed_channels",
            }),
        }
    }
}

/// One line of an approval preview.
///
/// A key and a value rather than a free-form string, so the preview is structured enough to be
/// redacted field by field and to be rendered consistently. The contract requires a preview to be
/// "schema-defined, bounded, redacted, and sufficient for informed consent" — this is the *shape*
/// half; redaction is the producer's job, because only the producer knows which of its own values is
/// sensitive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewItem {
    /// The field being shown.
    pub key: String,
    /// The value to show. Already redacted by the producer.
    pub value: String,
}

impl PreviewItem {
    /// Validates and builds a preview line.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `preview` when either half is empty,
    /// over-long, or carries a control character. The control-character rule is not cosmetic: a
    /// preview is rendered inside a prompt a user reads to decide, and it is the one place where a
    /// crafted string could make the prompt misrepresent the action.
    pub fn new(key: &str, value: &str) -> Result<Self, DomainError> {
        if !usable(key) || !usable(value) {
            return Err(DomainError::ToolDefinitionInvalid { field: "preview" });
        }
        Ok(Self {
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }
}

/// Returns whether a preview or summary string is usable.
fn usable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PREVIEW_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

/// A bounded, structured preview of the exact action being approved.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalPreview(Vec<PreviewItem>);

impl ApprovalPreview {
    /// Validates and wraps the preview lines.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `preview` when the list exceeds
    /// [`MAX_PREVIEW_ITEMS`]. An **empty** preview is accepted, because whether a preview can be
    /// empty is a policy question about the tool rather than a domain invariant — a read-only tool
    /// genuinely may have nothing to show — and refusing it here would make that a compile-time
    /// argument instead of a reviewable decision.
    pub fn new(items: Vec<PreviewItem>) -> Result<Self, DomainError> {
        if items.len() > MAX_PREVIEW_ITEMS {
            return Err(DomainError::ToolDefinitionInvalid { field: "preview" });
        }
        Ok(Self(items))
    }

    /// Returns the preview lines.
    #[must_use]
    pub fn items(&self) -> &[PreviewItem] {
        &self.0
    }

    /// Returns whether the preview shows nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The channels an approval may be decided on.
///
/// A bounded set, and **never empty**: an approval with no permitted channel could never be decided,
/// so it would sit `Pending` until it expired — the "no legal way out" shape this project keeps
/// finding. Refusing the empty case at construction makes that unrepresentable.
///
/// **`Deserialize` goes through [`Self::new`]**, so an empty channel set cannot arrive over the wire.
/// A derived impl would have accepted one, restoring exactly the unreachable state the constructor
/// exists to prevent — and a stored approval with no channel is the state most easily mistaken for a
/// working permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AllowedChannels(Vec<ApprovalChannel>);

impl AllowedChannels {
    /// Validates and wraps the channel set.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `allowed_channels` when the set is empty
    /// or exceeds [`MAX_APPROVAL_CHANNELS`].
    pub fn new(channels: Vec<ApprovalChannel>) -> Result<Self, DomainError> {
        if channels.is_empty() || channels.len() > MAX_APPROVAL_CHANNELS {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "allowed_channels",
            });
        }
        Ok(Self(channels))
    }

    /// Returns whether a channel may decide this approval.
    #[must_use]
    pub fn permits(&self, channel: ApprovalChannel) -> bool {
        self.0.contains(&channel)
    }

    /// Returns the permitted channels.
    #[must_use]
    pub fn as_slice(&self) -> &[ApprovalChannel] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AllowedChannels {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Vec::<ApprovalChannel>::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// The facts an approval request is built from.
///
/// A struct rather than eight parameters, because several of its fields are identifiers of the same
/// shape and a positional call would make a workspace/principal transposition possible — the reason
/// `BRN-008`'s controller groups its helpers' arguments the same way.
#[derive(Debug, Clone)]
pub struct ApprovalRequestParts {
    /// The workspace the action happens in. Resolved server-side.
    pub workspace: WorkspaceId,
    /// The principal asking.
    pub requesting_principal: PrincipalId,
    /// The run that asked.
    pub run: RunId,
    /// The tool call that asked.
    pub tool_call: ToolCallId,
    /// The exact tool identity being invoked.
    pub identity: ToolIdentity,
    /// A digest over the exact action.
    ///
    /// **Typed rather than text, so a caller cannot contribute one that no computation produced.** The
    /// contract requires the fingerprint to be "computed" over a versioned object with an explicit
    /// algorithm prefix; accepting a bare `String` here meant a malformed or differently-typed digest was
    /// stored and later *compared*, which is a check that silently stops applying. The computation is
    /// [`crate::tool::canonical`](super::canonical)'s plus `jarvis_infrastructure`'s, and the value is built
    /// through [`ActionDigest::parse`](super::canonical::ActionDigest::parse) or from bytes.
    pub action_digest: super::canonical::ActionDigest,
    /// The tool's risk, as a label for the prompt.
    pub risk: super::classification::Risk,
    /// The tool's effects, for the prompt.
    pub effects: Vec<super::classification::Effect>,
    /// A one-line summary of the action.
    pub summary: String,
    /// The structured preview.
    pub preview: ApprovalPreview,
    /// Which channels may decide it.
    pub allowed_channels: AllowedChannels,
    /// When the request stops being decidable.
    pub expires_at: UtcTimestamp,
    /// Whether it authorizes one call or a pattern.
    pub scope: ApprovalScopeKind,
}

/// A durable approval record: what was asked, what was decided, and by whom.
///
/// Every field the contract's example carries is here. The state is **private** behind
/// [`DurableApproval::apply`], exactly as [`crate::run::lifecycle::RunLifecycle`]'s is, so no caller
/// can write a state the transition table does not permit by assigning to a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableApproval {
    /// The approval's own identity.
    pub id: ApprovalId,
    /// The workspace the action happens in.
    pub workspace: WorkspaceId,
    /// Who asked.
    pub requesting_principal: PrincipalId,
    /// The run that asked.
    pub run: RunId,
    /// The tool call that asked.
    pub tool_call: ToolCallId,
    /// The exact tool identity, including source and schema fingerprint.
    pub identity: ToolIdentity,
    /// A digest over the exact action. See [`ApprovalRecord`](super::policy::ApprovalRecord) for why the
    /// *computation* is not here.
    pub action_digest: super::canonical::ActionDigest,
    /// The tool's risk at the time of asking, recorded so a later prompt shows what was decided.
    pub risk: super::classification::Risk,
    /// The tool's effects at the time of asking.
    pub effects: Vec<super::classification::Effect>,
    /// A one-line summary of the action.
    pub summary: String,
    /// The structured preview.
    pub preview: ApprovalPreview,
    /// Which channels may decide it.
    pub allowed_channels: AllowedChannels,
    /// When the request stops being decidable.
    pub expires_at: UtcTimestamp,
    /// Whether it authorizes one call or a pattern.
    pub scope: ApprovalScopeKind,
    /// The current state. Private so every change goes through [`DurableApproval::apply`].
    state: ApprovalState,
    /// The current version.
    version: ApprovalVersion,
    /// Who decided it, once a decision exists.
    decided_by: Option<PrincipalId>,
    /// Which channel the decision came from.
    decided_via: Option<ApprovalChannel>,
    /// The assurance the decider **proved**, once a decision exists.
    ///
    /// **A field on the record rather than only on the actor, and the distinction is a defect this
    /// round found.** The contract's audit section requires "request/decision/consumption identities,
    /// assurance, channel ...", so the assurance is a property of the *decision on this record*. Recording
    /// it only inside the transition actor meant a later non-decision transition — a consumption, an
    /// invalidation, an expiry — overwrote the column with NULL, and the reconstructed record then had no
    /// assurance while its own transition row still carried one. The two live together here, exactly as
    /// the principal and the channel do.
    decided_assurance: Option<crate::model::exception::RequiredAssurance>,
    /// When the decision was made.
    decided_at: Option<UtcTimestamp>,
}

impl DurableApproval {
    /// Creates a pending approval.
    ///
    /// The state and version are not parameters: an approval is created pending at version one, and
    /// letting a caller choose would allow a record to be constructed already decided, which is the
    /// one thing an approval must never be.
    #[must_use]
    pub fn request(parts: ApprovalRequestParts) -> Self {
        Self {
            id: ApprovalId::from_uuid(uuid::Uuid::now_v7()),
            workspace: parts.workspace,
            requesting_principal: parts.requesting_principal,
            run: parts.run,
            tool_call: parts.tool_call,
            identity: parts.identity,
            action_digest: parts.action_digest,
            risk: parts.risk,
            effects: parts.effects,
            summary: parts.summary,
            preview: parts.preview,
            allowed_channels: parts.allowed_channels,
            expires_at: parts.expires_at,
            scope: parts.scope,
            state: ApprovalState::Pending,
            version: ApprovalVersion::FIRST,
            decided_by: None,
            decided_via: None,
            decided_assurance: None,
            decided_at: None,
        }
    }

    /// Returns the current state.
    #[must_use]
    pub const fn state(&self) -> ApprovalState {
        self.state
    }

    /// Returns the current version.
    #[must_use]
    pub const fn version(&self) -> ApprovalVersion {
        self.version
    }

    /// Returns whether a decision has been taken.
    #[must_use]
    pub const fn is_decided(&self) -> bool {
        self.decided_at.is_some()
    }

    /// Returns who decided it, if anyone.
    #[must_use]
    pub const fn decided_by(&self) -> Option<PrincipalId> {
        self.decided_by
    }

    /// Returns which channel the decision came from.
    #[must_use]
    pub const fn decided_via(&self) -> Option<ApprovalChannel> {
        self.decided_via
    }

    /// Returns the assurance the decider proved, once a decision exists.
    ///
    /// Exposed so the **adapter can store it and a reader can report it**: the contract's audit section
    /// requires the assurance recorded, and a field with a writer and no accessor cannot be persisted —
    /// the "value nothing can read" shape this project has found in several slices.
    #[must_use]
    pub const fn decided_assurance(&self) -> Option<crate::model::exception::RequiredAssurance> {
        self.decided_assurance
    }

    /// Returns when the decision was made, if one was.
    ///
    /// Exposed for the same reason `dispatched_at` is on the ledger row: the instant is a **stored
    /// column**, and a reader that had the boolean but not the value would have to invent one. The
    /// approval store orders a principal's history by this value, so it is read rather than derived
    /// from `updated_at`, which a later non-decision transition would have moved.
    #[must_use]
    pub const fn decided_at(&self) -> Option<UtcTimestamp> {
        self.decided_at
    }

    /// Returns whether the approval has lapsed at `now`.
    ///
    /// **Lapsed is not the same as `Expired`.** This is a computation from the clock; the *state*
    /// only becomes `Expired` when a transition records it. That separation matters because an
    /// approval that lapsed while nobody was looking still has to be refused, and a check that
    /// trusted only the stored state would honour it — the same "a field with a reader and no writer"
    /// trap in reverse.
    #[must_use]
    pub fn is_lapsed_at(&self, now: UtcTimestamp) -> bool {
        now >= self.expires_at
    }

    /// Returns whether this approval permits `action_digest` at `now`.
    ///
    /// The four conditions are the ones that must all hold for an approval to authorize anything,
    /// and they are gathered here so there is **one** definition of "this approval covers this
    /// action" rather than one per caller. A caller that checked three of the four would be a second,
    /// weaker answer — the shape of every scoping defect this project has found.
    #[must_use]
    pub fn covers(
        &self,
        action_digest: &super::canonical::ActionDigest,
        now: UtcTimestamp,
    ) -> bool {
        self.state == ApprovalState::Approved
            && self.action_digest == *action_digest
            && !self.is_lapsed_at(now)
    }

    /// Applies a transition, or refuses it with a typed reason.
    ///
    /// The checks are ordered so the caller receives the *most specific true* answer, following
    /// `RunLifecycle::apply`'s reasoning: the version is checked before the edge, because an illegal
    /// edge computed from a stale view may be legal from the current one. There is no separate
    /// terminal check here because terminal states are absorbing in the transition table itself, so
    /// a transition from one is refused as an illegal edge with both states named.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ApprovalVersionConflict`] for a stale expected version, and
    /// [`DomainError::ApprovalStateConflict`] for a transition the contract's state set does not
    /// permit.
    pub fn apply(
        &mut self,
        to: ApprovalState,
        expected_version: ApprovalVersion,
        actor: ApprovalActor,
        occurred_at: UtcTimestamp,
    ) -> Result<ApprovalTransitionRecord, DomainError> {
        if expected_version != self.version {
            return Err(DomainError::ApprovalVersionConflict {
                expected: expected_version,
                actual: self.version,
            });
        }
        if !self.state.can_transition_to(to) {
            return Err(DomainError::ApprovalStateConflict {
                from: self.state,
                to,
            });
        }
        // A decision may only come from a permitted channel. Checked **after** the edge check so an
        // illegal transition is reported as an illegal transition rather than as a channel problem,
        // which would send a caller to inspect the request's channels when the state was the issue.
        if let Some(channel) = actor.channel()
            && matches!(to, ApprovalState::Approved | ApprovalState::Rejected)
            && !self.allowed_channels.permits(channel)
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "allowed_channels",
            });
        }
        let from = self.state;
        let prior_version = self.version;
        self.state = to;
        self.version = self.version.next()?;
        if let ApprovalActor::Decided {
            principal,
            channel,
            assurance,
            ..
        } = actor
        {
            // `decided_at` is set once. A second decision cannot arrive because every decision state
            // is terminal, so this is bookkeeping rather than a guard — recorded so a later reader
            // can see the reasoning that produced the state machine. The assurance is written with the
            // other three **and is read back by the adapter**, which is what makes the audit requirement
            // satisfiable; recording it only on the transition actor left it lost the moment a later
            // non-decision transition overwrote the column.
            self.decided_by = Some(principal);
            self.decided_via = Some(channel);
            self.decided_assurance = Some(assurance);
            self.decided_at = Some(occurred_at);
        }
        Ok(ApprovalTransitionRecord {
            id: self.id,
            from,
            to,
            prior_version,
            version: self.version,
            actor,
            occurred_at,
        })
    }
}

/// Who caused an approval transition.
///
/// Typed rather than a bare principal, because **only some transitions have a decider**: expiry and
/// invalidation are caused by time and by a changed action, not by a person, and a record that
/// claimed a user expired an approval late at night would be an audit trail that lies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalActor {
    /// A principal decided, on one channel.
    Decided {
        /// Who decided.
        principal: PrincipalId,
        /// Which channel they used.
        channel: ApprovalChannel,
        /// The assurance the decider **proved**, as the server resolved it.
        ///
        /// **The contract's audit section requires "assurance", and this record did not carry it.**
        /// It is the strongest evidence an operator has that a consequential decision was made by a
        /// stepped-up caller rather than an ordinary session: the channel is the surface, while this is
        /// the strength of the credential behind it. Recorded as the value the **server** derived from the
        /// authenticated request — never anything a body stated, which is the `BRN-024` rule — and it is
        /// written with the decision rather than derived later, because the assurance at decision time
        /// cannot be reconstructed from a record that omits it.
        assurance: crate::model::exception::RequiredAssurance,
        /// The operator's own note, when one was given.
        ///
        /// **The contract's decision body carries a `comment`, and this is where it lives.** It was
        /// declared on the wire with a doc comment saying "stored with the decision" and nothing stored it:
        /// the field was deserialized and dropped, which made the doc a claim about a capability that did
        /// not exist. It sits on the actor rather than on the record because it explains *this* transition
        /// — a decision is the only transition a human authors — and it is `None` when the caller gave none,
        /// which is distinct from an empty note (refused at construction) because "said nothing" and "said
        /// nothing in particular" are different records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<DecisionNote>,
    },
    /// The approval lapsed because its expiry passed.
    ///
    /// Carries `expires_at` so the record states *which* deadline passed — the value may since have
    /// been changed by a superseding record, and "it expired" alone would not say what it expired
    /// against.
    Expired {
        /// The deadline that passed.
        expires_at: UtcTimestamp,
    },
    /// The action changed, so the approval no longer describes it.
    Invalidated {
        /// The digest the approval was for.
        was_for: String,
    },
    /// The exact tool call was reserved, spending a one-shot approval.
    Consumed {
        /// The call that spent it.
        tool_call: ToolCallId,
    },
    /// The requesting principal withdrew it.
    Cancelled {
        /// Who withdrew it.
        by: PrincipalId,
        /// Why it was withdrawn, when the caller said.
        ///
        /// **The contract says a cancellation uses "`expected_version`, reason code, and
        /// `Idempotency-Key`", and the reason is what makes the audit trail say *why* a prompt was
        /// withdrawn rather than only that it was** — the same claim the wire type made while nothing stored
        /// it. It lives on the actor for the same reason the decision's note does: it explains this one
        /// transition rather than being a durable property of the approval.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<DecisionNote>,
    },
}

impl ApprovalActor {
    /// Returns the channel, when the actor decided on one.
    #[must_use]
    pub fn channel(&self) -> Option<ApprovalChannel> {
        match self {
            Self::Decided { channel, .. } => Some(*channel),
            _ => None,
        }
    }

    /// Returns the assurance a decider proved, when the actor decided.
    ///
    /// Exposed so an **adapter can store it and a reader can report it**, because the contract's audit
    /// section requires the assurance to be recorded and a value with no accessor could be written but
    /// never checked — the "field with a writer and no reader" shape this project has found repeatedly.
    #[must_use]
    pub fn assurance(&self) -> Option<crate::model::exception::RequiredAssurance> {
        match self {
            Self::Decided { assurance, .. } => Some(*assurance),
            _ => None,
        }
    }

    /// Returns the operator's own note, when the actor carries one.
    ///
    /// A decision's `comment` and a cancellation's `reason` are the same kind of value — caller-supplied
    /// text explaining a human's choice — so one accessor serves both, and a reader asking "why was this
    /// transition taken" gets the same answer for either.
    #[must_use]
    pub fn note(&self) -> Option<&DecisionNote> {
        match self {
            Self::Decided { note, .. } => note.as_ref(),
            Self::Cancelled { reason, .. } => reason.as_ref(),
            _ => None,
        }
    }
}

/// The durable record of an approval transition that was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalTransitionRecord {
    /// The approval the transition applied to.
    pub id: ApprovalId,
    /// The state that was left.
    pub from: ApprovalState,
    /// The state that was entered.
    pub to: ApprovalState,
    /// The version before.
    pub prior_version: ApprovalVersion,
    /// The version after.
    pub version: ApprovalVersion,
    /// Who or what caused it.
    pub actor: ApprovalActor,
    /// When it happened.
    pub occurred_at: UtcTimestamp,
}

impl fmt::Display for ApprovalTransitionRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "approval {} {} -> {} at version {}",
            self.id, self.from, self.to, self.version,
        )
    }
}

/// Returns whether a summary is usable as a prompt line.
///
/// Exposed because the summary's bound is checked where a request is built, and the *reason* a
/// summary is refused must be available to that caller without duplicating these rules.
#[must_use]
pub fn is_usable_summary(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_SUMMARY_BYTES && !value.chars().any(char::is_control)
}
