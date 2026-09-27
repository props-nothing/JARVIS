//! The idempotent tool-call ledger and its execution state machine.
//!
//! `TLS-006` asks for two things, and the second is the one with real consequences:
//!
//! > Implement idempotent tool-call ledger and execution state machine.
//!
//! The ledger records one row per **attempt** at a tool call, keyed so that a duplicate submission
//! finds the existing row rather than causing a second effect. The state machine is
//! [`ToolCallState`], and its shape follows one rule from the tool contract that decides everything
//! else:
//!
//! > `EXECUTING` is recorded before an external effect. An unknown outcome uses `RECONCILING`, not
//! > automatic retry.
//!
//! That sentence is why [`ToolCallState::Executing`] and [`ToolCallState::Reconciling`] are separate
//! states rather than one "in flight" flag. A call that might have happened and a call that has not
//! happened are different facts, and the safe action differs: the first needs to be **looked up**,
//! the second may be retried. Collapsing them gives one answer for both, and the wrong direction
//! either sends twice or refuses a call that never happened.
//!
//! **The idempotency key is scoped by five things**, and this module reuses the same reasoning that
//! fixed `BRN-007`: the tool identity, the workspace, the principal, the logical operation, and the
//! caller's key. A key scoped by the caller's string alone would let one principal's retry collide
//! with another's fresh call — the same three-of-five-dimensions disclosure, in a place where the
//! consequence is a second side effect rather than a leaked identifier.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{PrincipalId, RunId, ToolCallId, ToolCallRecordId, WorkspaceId};
use crate::time::UtcTimestamp;

use super::error_class::ToolErrorClass;
use super::identity::ToolIdentity;

/// The longest accepted idempotency key.
///
/// Bounded because it reaches a unique index and an operator's diagnostics, and because an unbounded
/// key is an unbounded index entry.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// The largest number of ledger rows one reservation pass may consider.
///
/// A bound on the *query*, not on the ledger: the ledger grows with real calls, while any single
/// lookup should consider one row. The bound exists so a caller cannot ask for an unbounded scan and
/// turn a diagnostic into an outage.
pub const MAX_LEDGER_SCAN: usize = 1024;

/// A version counter for a ledger row's optimistic concurrency.
///
/// Newtyped for the same reason `RunVersion` and `ApprovalVersion` are: a version and a count are
/// both "a number", and transposing them is silent. `FIRST` is `1` so an uninitialised field cannot
/// read as a valid stored version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallVersion(u64);

impl ToolCallVersion {
    /// The version a freshly reserved call has.
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
    /// Returns [`DomainError::ToolCallVersionConflict`] when the counter is exhausted. A typed
    /// refusal rather than a wrapping add, because a wrapped version would let a stale writer match a
    /// current one and overwrite a recorded outcome.
    pub fn next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::ToolCallVersionConflict {
                expected: self,
                actual: self,
            })
    }
}

impl fmt::Display for ToolCallVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Where one tool-call attempt is in its lifecycle.
///
/// The tool contract's eleven states. Two pairs are deliberately distinct and the distinction is the
/// module's whole reason for existing:
///
/// - [`Self::Executing`] versus [`Self::Reconciling`] — a call that *is* running versus one whose
///   outcome is unknown. The contract requires `RECONCILING` for the second and forbids automatic
///   retry, because the effect may already have happened.
/// - [`Self::WaitingApproval`] versus [`Self::Approved`] — a call that needs a decision versus one
///   that has one. A caller that treated "approved" as "may proceed" without reserving would race a
///   second caller holding the same approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallState {
    /// The model asked for this call.
    Requested,
    /// Arguments validated against the schema.
    Validated,
    /// Policy refused it.
    Denied,
    /// Policy requires an approval that does not exist yet.
    WaitingApproval,
    /// An approval exists for this exact action.
    Approved,
    /// The call's key is reserved, so no second attempt can start.
    Reserved,
    /// The adapter is running. **Recorded before the external effect.**
    Executing,
    /// The outcome is unknown and must be reconciled rather than retried.
    Reconciling,
    /// The call completed.
    Succeeded,
    /// The call failed. Whether it may be retried is a separate question.
    Failed,
    /// The call was cancelled.
    Cancelled,
}

impl ToolCallState {
    /// Every state, in the order the contract lists them.
    pub const ALL: &'static [Self] = &[
        Self::Requested,
        Self::Validated,
        Self::Denied,
        Self::WaitingApproval,
        Self::Approved,
        Self::Reserved,
        Self::Executing,
        Self::Reconciling,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Returns whether the state is terminal.
    ///
    /// **`Reconciling` is not terminal**, which is the point of it: a call in reconciliation has an
    /// outcome that is not yet known, and treating it as finished would either lose a successful
    /// effect or leave a failed one recorded as outstanding.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Denied | Self::Succeeded | Self::Failed | Self::Cancelled
        )
    }

    /// Returns whether the call has been **dispatched**, so that an effect may exist.
    ///
    /// This is the predicate every retry decision has to consult, and it is deliberately about
    /// dispatch rather than about failure: a call that failed after dispatch may have had its
    /// effect, and one that failed before it cannot have. The contract's own wording draws the line
    /// here — "`EXECUTING` is recorded before an external effect" — so the state is exactly the
    /// memory of whether that write happened.
    #[must_use]
    pub const fn was_dispatched(self) -> bool {
        matches!(self, Self::Executing | Self::Reconciling)
    }

    /// Returns whether the call's outcome is unknown.
    #[must_use]
    pub const fn is_unsettled(self) -> bool {
        matches!(self, Self::Reconciling)
    }

    /// Returns whether a transition from `self` to `to` is legal.
    ///
    /// A table rather than a guard plus a list, for the reason `TLS-005` recorded: an early-return
    /// guard is unreachable when the table already excludes its cases, so it reads as enforcement
    /// while enforcing nothing and a mutation that deletes it passes. Terminal states are excluded
    /// here by there being no arm with a terminal source, and
    /// [`Self::has_any_transition`] gives a test a reachable thing to assert.
    ///
    /// Three edges are the ones worth naming:
    ///
    /// - **`Executing -> Reconciling`**, never `Executing -> Failed` directly for an unknown outcome.
    ///   A provider that crashed mid-call leaves the effect unknown, and recording it as failed would
    ///   invite a retry that duplicates it.
    /// - **`Executing -> Executing`** is refused. Re-entering execution is a second dispatch, which
    ///   is precisely what the reservation exists to prevent.
    /// - **`Reserved -> Executing`** rather than `Approved -> Executing`, so the reservation cannot be
    ///   skipped: a caller holding an approval must still reserve before it may dispatch.
    #[must_use]
    pub const fn can_transition_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (
                Self::Requested,
                Self::Validated | Self::Denied | Self::WaitingApproval | Self::Cancelled
            ) | (
                Self::Validated,
                Self::Denied
                    | Self::WaitingApproval
                    | Self::Approved
                    | Self::Reserved
                    | Self::Cancelled
            ) | (
                Self::WaitingApproval,
                Self::Approved | Self::Denied | Self::Cancelled
            ) | (Self::Approved, Self::Reserved | Self::Cancelled)
                | (Self::Reserved, Self::Executing | Self::Cancelled)
                | (
                    Self::Executing,
                    Self::Succeeded | Self::Failed | Self::Reconciling | Self::Cancelled
                )
                | (Self::Reconciling, Self::Succeeded | Self::Failed)
        )
    }

    /// Returns whether any target is reachable from `self`.
    ///
    /// The complement test for the table above, so a test can assert that every non-terminal state
    /// has a legal exit — the "no legal way out" shape this project has now found seven times.
    #[must_use]
    pub const fn has_any_transition(self) -> bool {
        !self.is_terminal()
    }

    /// Returns the spelling JARVIS stores and puts on the wire.
    ///
    /// **Lowercase, and it has to agree with the serde form.** The tool contract lists the states in
    /// uppercase prose (`REQUESTED`, `VALIDATED`, …) while its own JSON examples are lowercase
    /// (`"status": "succeeded"`), so the document itself uses two spellings for one value. A first
    /// version of this method returned the uppercase prose form while the derive produced
    /// `snake_case`, which meant a state had **two** spellings in JARVIS: one to store and one to
    /// parse. That is the same "two spellings denote one value" defect this project refuses for
    /// identifiers, and it surfaced as a failing assertion about the wire form rather than as a
    /// runtime bug only because a test checked the serialized bytes.
    ///
    /// One spelling is chosen — the lowercase one, because it is what the JSON examples show and what
    /// serde already produced — and `every_state_serializes_to_its_own_spelling` asserts that this
    /// method and the derive agree for every variant, so neither can drift from the other.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Validated => "validated",
            Self::Denied => "denied",
            Self::WaitingApproval => "waiting_approval",
            Self::Approved => "approved",
            Self::Reserved => "reserved",
            Self::Executing => "executing",
            Self::Reconciling => "reconciling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses the stored spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `state`. An unrecognised stored state
    /// must not be read as `Requested`, which is the fail-open direction: a call whose state column
    /// was corrupted would look like a fresh request rather than as something needing attention.
    /// The uppercase prose form is **not** accepted, because accepting it would restore the second
    /// spelling this method exists to eliminate — a caller with uppercase input has the contract's
    /// prose, not a stored value.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_contract_str() == value)
            .ok_or(DomainError::ToolDefinitionInvalid { field: "state" })
    }
}

impl fmt::Display for ToolCallState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// The five dimensions a reservation key is scoped by.
///
/// **All five, and none of them optional**, which is the lesson `BRN-007` and `TLS-004` both
/// recorded: a key naming fewer dimensions reads as complete while letting one scope's entry answer
/// another's question. Here the consequence is a second side effect rather than a disclosure, which
/// is worse — a retry that finds another principal's row would either be refused as a duplicate
/// (confusing but safe) or, if the lookup were looser, be answered with that row's outcome and skip
/// the call the caller actually asked for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReservationKey {
    /// The exact tool identity, including source and schema fingerprint.
    pub identity: ToolIdentity,
    /// The workspace the call happens in. Resolved server-side.
    pub workspace: WorkspaceId,
    /// The principal the call is made as.
    pub principal: PrincipalId,
    /// The caller's idempotency key.
    pub idempotency_key: String,
}

impl ReservationKey {
    /// Validates and builds a key.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `idempotency_key` when it is empty,
    /// over-long, or carries a control character. An empty key is refused rather than treated as
    /// "no key", because a caller that meant "no key" must say so by not asking for a reservation —
    /// a defaulted empty string would make every unkeyed call collide with every other.
    pub fn new(
        identity: ToolIdentity,
        workspace: WorkspaceId,
        principal: PrincipalId,
        idempotency_key: &str,
    ) -> Result<Self, DomainError> {
        if idempotency_key.is_empty()
            || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "idempotency_key",
            });
        }
        Ok(Self {
            identity,
            workspace,
            principal,
            idempotency_key: idempotency_key.to_owned(),
        })
    }
}

/// The operation a reservation is for.
///
/// **Not part of [`ReservationKey`], and that is a considered choice rather than an omission.** The
/// key identifies *which invocation* this is, while the operation describes *what the attempt is
/// doing with it* — an execute, a reconcile, a probe — and one invocation legitimately has several
/// operations performed against it over its life. Folding the operation into the key would give one
/// call several keys, which would defeat the reservation it exists to provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerOperation {
    /// Dispatch the call.
    Execute,
    /// Establish an unknown outcome by reading the provider's state.
    Reconcile,
    /// Read the recorded outcome without changing it.
    Read,
}

/// One durable ledger row.
///
/// The state is **private** behind [`LedgerEntry::apply`], exactly as `RunLifecycle`'s and
/// `DurableApproval`'s are, so no caller can write a state the table does not permit by assigning to
/// a field. The `outcome` is recorded in the same transition that marks the call terminal, so a row
/// cannot claim success while carrying no result — the failure mode a caller would see as "the tool
/// succeeded and returned nothing".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// The row's own identity.
    pub id: ToolCallRecordId,
    /// The canonical call identifier, stable across attempts.
    pub call_id: ToolCallId,
    /// Which attempt this row records, counting from 1. Private so a zero attempt is unrepresentable.
    attempt: u32,
    /// The run the call belongs to.
    pub run: RunId,
    /// The reservation key.
    pub key: ReservationKey,
    /// The logical operation this row was opened for.
    pub operation: LedgerOperation,
    /// The state. Private so every change goes through [`LedgerEntry::apply`].
    state: ToolCallState,
    /// The current version. Private for the same reason.
    version: ToolCallVersion,
    /// When the row was created.
    pub created_at: UtcTimestamp,
    /// When the row last changed.
    updated_at: UtcTimestamp,
    /// **When the call was dispatched**, or `None` if it never was.
    ///
    /// Recorded as a fact rather than derived from the current state, and this is a defect fix rather
    /// than a refactor. The first version answered "may this call have had an effect?" with
    /// `state.was_dispatched()`, which is a question about *where the call is now* — and a call that
    /// reached `EXECUTING` and then ended `FAILED` is no longer in a dispatched state, so the answer
    /// became `false`. That inverted the one conclusion `ACC-025` depends on: a dispatched call that
    /// failed would have been treated as having provably had **no** effect, and retried, duplicating
    /// the effect. Dispatch is a thing that happened once; the ledger is exactly the place such a fact
    /// belongs, and re-deriving it from a state that has since moved on is how it gets lost.
    dispatched_at: Option<UtcTimestamp>,
    /// The terminal error class, once the call has one.
    outcome: Option<ToolErrorClass>,
    /// Whether the outcome is known to have had no effect.
    ///
    /// Set only by a transition that can prove it — a pre-dispatch refusal — and deliberately **not**
    /// inferred from the error class. `was_dispatched` is the state's own memory; this is the
    /// separate claim that a dispatched call produced nothing, which only reconciliation can make.
    no_effect_confirmed: bool,
}

impl LedgerEntry {
    /// Opens a ledger row in `REQUESTED`.
    ///
    /// The state and version are not parameters: a row is created requested at version one, and
    /// letting a caller choose would allow a row constructed already succeeded — a ledger that
    /// reports an effect nobody performed.
    #[must_use]
    pub fn reserve(
        call_id: ToolCallId,
        attempt: u32,
        run: RunId,
        key: ReservationKey,
        operation: LedgerOperation,
        now: UtcTimestamp,
    ) -> Self {
        Self {
            id: ToolCallRecordId::from_uuid(uuid::Uuid::now_v7()),
            call_id,
            attempt: attempt.max(1),
            run,
            key,
            operation,
            state: ToolCallState::Requested,
            version: ToolCallVersion::FIRST,
            created_at: now,
            updated_at: now,
            dispatched_at: None,
            outcome: None,
            no_effect_confirmed: false,
        }
    }

    /// Returns the current state.
    #[must_use]
    pub const fn state(&self) -> ToolCallState {
        self.state
    }

    /// Returns which attempt this row records.
    ///
    /// An accessor rather than a public field, because "an attempt number is at least one" is only a
    /// property of the type while nothing outside can assign to it. The first version left `attempt`
    /// public and normalised `0` in `reserve`, so a caller that set the field afterwards could put a
    /// zero attempt into the ledger — and "attempt zero" has no meaning, so nothing downstream could
    /// report it coherently.
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the current version.
    #[must_use]
    pub const fn version(&self) -> ToolCallVersion {
        self.version
    }

    /// Returns when the row last changed.
    #[must_use]
    pub const fn updated_at(&self) -> UtcTimestamp {
        self.updated_at
    }

    /// Returns the terminal error class, if one was recorded.
    #[must_use]
    pub const fn outcome(&self) -> Option<ToolErrorClass> {
        self.outcome
    }

    /// Returns whether the call is known to have produced no effect.
    #[must_use]
    pub const fn no_effect_confirmed(&self) -> bool {
        self.no_effect_confirmed
    }

    /// Returns whether this call was ever dispatched, so an effect may exist.
    ///
    /// **The fact, not an inference from the current state.** `EXECUTING` and `RECONCILING` are where a
    /// call *is* while it may have effected; this says whether it ever got there, which is what a retry
    /// decision needs after the call has moved on to a terminal state.
    #[must_use]
    pub const fn was_dispatched(&self) -> bool {
        self.dispatched_at.is_some()
    }

    /// Returns whether this row may have produced an effect.
    ///
    /// Two facts combined, and the combination is the point: a call that was dispatched may have
    /// effected **unless** reconciliation has since established that it did not. Neither half is
    /// sufficient — the first alone would forbid retrying a call proven harmless, and the second alone
    /// would permit retrying one whose effect is unknown.
    #[must_use]
    pub const fn may_have_effected(&self) -> bool {
        self.was_dispatched() && !self.no_effect_confirmed
    }

    /// Applies a transition, or refuses it with a typed reason.
    ///
    /// The version is checked before the edge, following the ordering `RunLifecycle::apply`
    /// established: a caller working from a stale view must learn its view is stale **before** it is
    /// told an edge is illegal, because an illegal edge computed from a stale state may be legal
    /// from the current one.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolCallVersionConflict`] for a stale expected version and
    /// [`DomainError::ToolCallStateConflict`] for an edge the table does not contain.
    pub fn apply(
        &mut self,
        to: ToolCallState,
        expected_version: ToolCallVersion,
        outcome: Option<ToolErrorClass>,
        occurred_at: UtcTimestamp,
    ) -> Result<ToolCallTransition, DomainError> {
        if expected_version != self.version {
            return Err(DomainError::ToolCallVersionConflict {
                expected: expected_version,
                actual: self.version,
            });
        }
        if !self.state.can_transition_to(to) {
            return Err(DomainError::ToolCallStateConflict {
                from: self.state,
                to,
            });
        }
        let from = self.state;
        let prior_version = self.version;
        self.state = to;
        self.version = self.version.next()?;
        self.updated_at = occurred_at;
        // The dispatch fact is stamped the first time the call enters `EXECUTING`, and never
        // un-stamped: a call that was dispatched stays dispatched whatever happens to it afterwards.
        // Stamping it here rather than deriving it later is what keeps the fact after the state moves
        // on — the defect this field was added to fix.
        if to == ToolCallState::Executing && self.dispatched_at.is_none() {
            self.dispatched_at = Some(occurred_at);
        }
        // The outcome is recorded only for a terminal state, and a terminal state must carry one.
        // Refusing the mismatch rather than defaulting is what keeps "terminal" and "has an outcome"
        // from being two facts that can disagree: a succeeded row with no outcome would tell a caller
        // the tool worked and produced nothing, and a failed row with no class would leave the
        // retry decision unanswerable.
        if to.is_terminal() {
            self.outcome = outcome;
            // **An OR assignment, not an assignment.** A claim that no effect happened is only ever
            // added, never cleared: a call refused before dispatch is proven harmless, and no later
            // transition may take that proof away. Writing the plain assignment here is what would
            // let a `FAILED` recorded after reconciliation un-confirm an effect the provider was
            // already read about.
            self.no_effect_confirmed = self.no_effect_confirmed || !self.was_dispatched();
        }
        Ok(ToolCallTransition {
            id: self.id,
            from,
            to,
            prior_version,
            version: self.version,
            outcome,
            occurred_at,
        })
    }

    /// Records that reconciliation established the call produced no effect, ending it as failed.
    ///
    /// **A transition rather than a flag, and that is a correction.** The first version set
    /// `no_effect_confirmed` and left the row in `RECONCILING`, so the row still looked unsettled to
    /// every reader — including the reservation check, which would report a duplicate as needing
    /// reconciliation *after the reconciliation had already happened*. Reconciliation has an outcome
    /// like any other: the provider was read, and the answer was that nothing landed. So it ends the
    /// call as `Failed` (it did not succeed) with the no-effect claim recorded, and the row becomes a
    /// settled duplicate that a retry may act on.
    ///
    /// This is the only path by which a **dispatched** call can become safe to repeat, because the
    /// provider is the sole authority on whether an effect landed. A JARVIS-side assumption that a
    /// timed-out call did nothing is exactly the guess that sends twice.
    ///
    /// # Errors
    ///
    /// Returns the same refusals [`LedgerEntry::apply`] does, so a stale version or an illegal edge is
    /// reported by one implementation rather than two.
    pub fn confirm_no_effect(
        &mut self,
        expected_version: ToolCallVersion,
        occurred_at: UtcTimestamp,
    ) -> Result<ToolCallTransition, DomainError> {
        // The claim is set first and `apply` ORs it, so the ordering cannot lose it. Recorded with
        // `ProviderError` because the call did not succeed: the provider accepted the work and JARVIS
        // could not establish that it was done, which is a provider fault from the caller's side.
        self.no_effect_confirmed = true;
        self.apply(
            ToolCallState::Failed,
            expected_version,
            Some(ToolErrorClass::ProviderError),
            occurred_at,
        )
    }
}

/// The durable record of a ledger transition that was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallTransition {
    /// The row the transition applied to.
    pub id: ToolCallRecordId,
    /// The state that was left.
    pub from: ToolCallState,
    /// The state that was entered.
    pub to: ToolCallState,
    /// The version before.
    pub prior_version: ToolCallVersion,
    /// The version after.
    pub version: ToolCallVersion,
    /// The error class, when the transition recorded one.
    pub outcome: Option<ToolErrorClass>,
    /// When it happened.
    pub occurred_at: UtcTimestamp,
}

/// What a reservation attempt found.
///
/// Four outcomes rather than a `Result`, because "an equivalent call already exists" is not a failure
/// — it is the whole point of a reservation — and because the three duplicate shapes need different
/// caller responses:
///
/// - [`Self::AlreadyTerminal`] means the effect is recorded and the caller can **read the answer**.
/// - [`Self::InFlight`] means another attempt holds the reservation, so the caller must **wait**.
/// - [`Self::Unsettled`] means the outcome is unknown, so the caller must **reconcile** and must not
///   retry.
///
/// A `Result<(), Error>` collapses those into "no", and a caller given "no" would retry — which is
/// the single most dangerous response available here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationOutcome {
    /// The reservation was taken; this caller may dispatch.
    Granted,
    /// An equivalent call already finished. Its outcome is available to read.
    AlreadyTerminal {
        /// The state it finished in.
        state: ToolCallState,
        /// The recorded error class, if it failed.
        outcome: Option<ToolErrorClass>,
        /// Whether the finished call is known to have had no effect.
        no_effect_confirmed: bool,
    },
    /// An equivalent call holds the reservation and has not finished.
    InFlight {
        /// The state the other attempt is in.
        state: ToolCallState,
    },
    /// An equivalent call has an unknown outcome.
    Unsettled {
        /// The row whose outcome must be established.
        row: ToolCallRecordId,
    },
}

impl ReservationOutcome {
    /// Returns whether the caller may dispatch.
    #[must_use]
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted)
    }

    /// Returns whether the caller should read an existing answer instead of calling.
    #[must_use]
    pub fn is_settled_duplicate(&self) -> bool {
        matches!(self, Self::AlreadyTerminal { .. })
    }

    /// Returns whether a retry is permissible **for this outcome alone**.
    ///
    /// Only a granted reservation, or a settled duplicate that provably produced **no effect**, may
    /// be followed by a dispatch. Everything else — in flight, or unsettled — is refused, and the
    /// unsettled case is refused *even though* the call might never have happened, because only
    /// reconciliation can tell and a retry that guesses is the one that duplicates.
    #[must_use]
    pub fn permits_dispatch(&self) -> bool {
        match self {
            Self::Granted => true,
            Self::AlreadyTerminal {
                no_effect_confirmed,
                outcome,
                ..
            } => *no_effect_confirmed && outcome.is_some(),
            Self::InFlight { .. } | Self::Unsettled { .. } => false,
        }
    }
}

/// The ledger: what has been attempted, in which state, and under which key.
///
/// Holds **no policy and no approvals**: it records what happened, and the decision to dispatch was
/// taken by [`crate::tool::policy::evaluate`] before a reservation was asked for. A ledger that also
/// decided whether a call was allowed would be a second authorization layer, and the one consulted
/// first would be the one that mattered.
#[derive(Debug, Default)]
pub struct ToolCallLedger {
    /// Rows by reservation key. A `BTreeMap` keyed on the whole key, so a lookup is a membership test
    /// on all four dimensions rather than a comparison against each row — the shape that made the
    /// scoping rule structural in `TLS-002`'s cache as well.
    by_key: BTreeMap<ReservationKey, LedgerEntry>,
    /// Rows by their own identity, so a caller holding a row id can find it.
    by_id: BTreeMap<ToolCallRecordId, ToolCallId>,
}

impl ToolCallLedger {
    /// Builds an empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns how many rows the ledger holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Returns whether the ledger holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Attempts to reserve an equivalent call.
    ///
    /// **The lookup and the insert are one operation**, and that is the property that makes the
    /// reservation atomic in-memory. A caller that looked up, decided, and then inserted would have a
    /// window between the two in which a second caller could do the same — and the durable adapter
    /// gets the same guarantee from a single `INSERT ... ON CONFLICT` rather than from two statements.
    pub fn reserve(&mut self, entry: LedgerEntry) -> ReservationOutcome {
        if let Some(existing) = self.by_key.get(&entry.key) {
            return match existing.state {
                ToolCallState::Reconciling => ReservationOutcome::Unsettled { row: existing.id },
                state if state.is_terminal() => ReservationOutcome::AlreadyTerminal {
                    state,
                    outcome: existing.outcome,
                    no_effect_confirmed: existing.no_effect_confirmed,
                },
                state => ReservationOutcome::InFlight { state },
            };
        }
        self.by_id.insert(entry.id, entry.call_id);
        self.by_key.insert(entry.key.clone(), entry);
        ReservationOutcome::Granted
    }

    /// Returns the row for a key, if any.
    #[must_use]
    pub fn get(&self, key: &ReservationKey) -> Option<&LedgerEntry> {
        self.by_key.get(key)
    }

    /// Returns the row for a key mutably, for a transition.
    #[must_use]
    pub fn get_mut(&mut self, key: &ReservationKey) -> Option<&mut LedgerEntry> {
        self.by_key.get_mut(key)
    }

    /// Returns every row with an unknown outcome.
    ///
    /// **The reconciliation work list.** A crash between `EXECUTING` and the outcome leaves such a
    /// row, and a startup pass has to find them without knowing their keys — the same shape as
    /// `RunRepository::incomplete_runs`, and deliberately unscoped for the same reason: startup
    /// reconciliation is a whole-profile concern, and scoping it to one workspace would leave every
    /// other workspace's unsettled calls unsettled with no symptom.
    #[must_use]
    pub fn unsettled(&self) -> Vec<&LedgerEntry> {
        self.by_key
            .values()
            .filter(|entry| entry.state.is_unsettled())
            .collect()
    }

    /// Returns every dispatch that may have produced an effect and has no recorded outcome.
    ///
    /// Narrower than [`Self::unsettled`]: a call in `EXECUTING` when the daemon stopped had already
    /// been dispatched and its outcome is unknown, even though its *state* does not yet say
    /// `RECONCILING`. That gap is exactly what a crash leaves, so a scan for it is what makes the
    /// recovery able to find the calls `ACC-025` is about.
    #[must_use]
    pub fn possibly_effecting_without_outcome(&self) -> Vec<&LedgerEntry> {
        self.by_key
            .values()
            .filter(|entry| {
                matches!(
                    entry.state,
                    ToolCallState::Executing | ToolCallState::Reconciling
                ) && entry.outcome.is_none()
            })
            .take(MAX_LEDGER_SCAN)
            .collect()
    }

    /// Returns the attempt count recorded for a call's key family, at most 1.
    ///
    /// A helper rather than a scan because the ledger keys on the whole reservation key, so *all*
    /// attempts of one logical call share a key — which is deliberate: the key identifies the
    /// invocation, and the attempt number distinguishes retries of it. Exposed so an executor can
    /// report which attempt it is on without walking the map.
    #[must_use]
    pub fn attempt_for(&self, key: &ReservationKey) -> Option<u32> {
        self.by_key.get(key).map(|entry| entry.attempt)
    }
}

/// Classifies a call found in `state` after a restart.
///
/// Returns `None` for a terminal state, because a finished call is left exactly as it is: rewriting a
/// recorded outcome would mean re-reporting an effect that already landed or erasing the record of
/// one that did.
///
/// **Exhaustive over the non-terminal states rather than defaulting**, following
/// `jarvis_domain::run::recovery::classify`'s reasoning: a state added to [`ToolCallState`] will fail
/// to compile here, which forces the person adding it to decide what an interrupted call in it means,
/// where a default would silently report the wrong thing for a state nobody considered.
#[must_use]
pub const fn classify_interrupted(state: ToolCallState) -> Option<InterruptedCallAction> {
    match state {
        // Nothing was dispatched, so no effect can exist and the call may safely be retried.
        ToolCallState::Requested
        | ToolCallState::Validated
        | ToolCallState::WaitingApproval
        | ToolCallState::Approved
        | ToolCallState::Reserved => Some(InterruptedCallAction::SafeToRetry { was_in: state }),
        // Dispatched, so an effect may exist. **Reconcile, never retry.**
        ToolCallState::Executing | ToolCallState::Reconciling => {
            Some(InterruptedCallAction::Reconcile { was_in: state })
        }
        // Terminal: left exactly as recorded.
        ToolCallState::Denied
        | ToolCallState::Succeeded
        | ToolCallState::Failed
        | ToolCallState::Cancelled => None,
    }
}

/// What startup must do with a tool call found non-terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptedCallAction {
    /// The call had not been dispatched, so no effect can exist and it may be retried safely.
    ///
    /// **Only this variant permits a retry**, and it is safe for a reason that is about dispatch
    /// rather than about the tool's own idempotency declaration: nothing reached the provider, so
    /// repeating the request cannot duplicate anything.
    SafeToRetry {
        /// The state it was interrupted in.
        was_in: ToolCallState,
    },
    /// The call had been dispatched and its outcome is unknown.
    ///
    /// The contract's `RECONCILING` case. A retry here is forbidden regardless of what the tool
    /// declared, because the declaration governs whether a *repeated request* is safe — and the
    /// question here is whether the *first* one already happened, which only the provider can answer.
    Reconcile {
        /// The state it was interrupted in.
        was_in: ToolCallState,
    },
}

impl InterruptedCallAction {
    /// The state the row must be moved to.
    #[must_use]
    pub const fn target_state(self) -> ToolCallState {
        match self {
            Self::SafeToRetry { .. } => ToolCallState::Cancelled,
            Self::Reconcile { .. } => ToolCallState::Reconciling,
        }
    }

    /// The stable reason recorded with the recovery transition.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::SafeToRetry { .. } => "interrupted_before_dispatch",
            Self::Reconcile { .. } => "interrupted_after_dispatch",
        }
    }

    /// Returns whether a retry is permissible.
    ///
    /// The one question the caller actually needs, stated as a method so there is one definition:
    /// a caller reading `target_state` would have to know that `Cancelled` here means "retryable",
    /// and that inference is exactly the kind that gets made wrongly.
    #[must_use]
    pub const fn permits_retry(self) -> bool {
        matches!(self, Self::SafeToRetry { .. })
    }
}
