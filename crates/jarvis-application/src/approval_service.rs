//! The approval use cases: list, read, decide, and cancel an approval record.
//!
//! `TLS-005` built the approval *record* and its state machine, and round 91 gave it a port and a
//! SQLite adapter. What was missing is the layer between them and a client: nothing could list a
//! pending prompt, resolve a decision against the authenticated caller, or withdraw a request. This
//! module is that layer, and it exists so the HTTP and CLI surfaces hold no approval policy of their
//! own.
//!
//! Four contract rules shape every method, and each one is the reason a field is where it is:
//!
//! 1. **The server derives the decider.** The contract says a decision body "cannot assert" the
//!    deciding principal, channel, assurance, or time. So `decide` takes a
//!    [`RequestContext`](crate::request_context::RequestContext) — which only trusted code can build —
//!    and reads the principal and channel from it, never from the caller's payload. The same rule
//!    `BRN-024` fixed for policy grants, applied to the surface where the decision *is* the
//!    authorization.
//! 2. **Channel and assurance are checked against the request.** The record's `allowed_channels`
//!    names which surfaces may decide it, so a decision arriving on any other channel is
//!    [`ApprovalServiceError::ScopeDenied`]; the assurance ladder is shared with policy grants so the
//!    two cannot disagree about what `Standard` permits.
//! 3. **Expiry is authoritative server time, evaluated on every read.** The contract says so in as many
//!    words — "expiry is evaluated on every read/decision/reservation" — and for several rounds only
//!    `decide` did it. That left the two surfaces where a prompt is *shown* reporting a record the daemon
//!    would refuse as expired: a detail read returning `pending` past its deadline, and a listing offering
//!    a row nobody can act on and spending its page budget on it. Both now evaluate the lapse and
//!    **record** it — a refusal that left the row `pending` would leave the prompt in every later listing,
//!    which is the "no legal way out" shape this project has found repeatedly. The listing expires and
//!    then **re-reads** rather than filtering, because a filtered page comes back short and a client that
//!    receives a short list concludes there is nothing left to decide.
//! 4. **An idempotent repeat is not an error.** A user double-tapping "approve" sends two requests,
//!    and the second must return the original decision. Only a repeat that *differs* is a conflict,
//!    and only a stale version is a version conflict. Collapsing these into one `Err` would make the
//!    ordinary double-tap look like a failure and hide the case that needs the user's attention.
//!
//! **What this module deliberately does not do**, each named rather than left implied:
//!
//! - It does not **compute** the action fingerprint, and that is now a layering statement rather than a
//!   gap: the canonicalization is [`jarvis_domain::tool::canonical`]'s (RFC 8785, evidence note
//!   `docs/research/integrations/rfc8785-canonicalization.md`) and the SHA-256 is
//!   `jarvis_infrastructure`'s, so this layer receives a computed
//!   [`ActionDigest`](jarvis_domain::tool::canonical::ActionDigest) and **compares** it. It never
//!   produces one, because hashing is a concrete implementation this crate must not depend on.
//! - It does not **create** approvals. A request comes from a policy `Ask` decision on a tool call,
//!   and no executor exists, so there is no producer — the port's `request` is still the only writer.
//! - It does not **resume** a waiting run or emit an outbox event. The contract's step 6 is "persists
//!   the immutable decision and outbox/resume signal atomically", and the outbox is `AUT-004`; the
//!   transition and its audit row are written in one transaction here, and the event is the caller's.

use std::sync::Arc;

use jarvis_domain::ids::ApprovalId;
use jarvis_domain::model::exception::RequiredAssurance;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    ApprovalActor, ApprovalChannel, ApprovalState, ApprovalVersion, DecisionNote, DurableApproval,
};
use jarvis_domain::tool::canonical::ActionDigest;

use crate::repository::RepositoryError;
use crate::repository::approval::{
    ApprovalCursor, ApprovalListFilter, ApprovalRepository, ApprovalsPage, DecideOutcome,
    MAX_PENDING_PAGE,
};
use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};

/// The largest cancellation reason accepted.
///
/// **Kept as an alias of the domain's own bound rather than declared here, because the two were the same
/// number written twice.** The reason and a decision's comment are one kind of value — caller-supplied text
/// explaining a human's choice — so they share one bound, and `DecisionNote::new` enforces it. A second
/// literal here would let this constant be raised alone while the domain still refused the longer value,
/// which reads to a caller as an unexplained `request.invalid`.
pub use jarvis_domain::tool::approval::MAX_DECISION_NOTE_BYTES as MAX_CANCEL_REASON_BYTES;

/// What a caller asks a decision to be.
///
/// **A struct rather than four parameters, and the argument bound forced the question rather than the
/// answer.** Three of the four are values a caller could transpose without the compiler noticing — a
/// version and a digest are both text-shaped to the eye, and a note is optional text — and the fourth is
/// the requested state. Grouping them also lets the handler build one value from the body it parsed, so
/// there is no positional call site to mis-order. The same reasoning `ApprovalRequestParts` records.
#[derive(Debug, Clone, Copy)]
pub struct DecisionCommand<'a> {
    /// Approve or reject.
    pub decision: Decision,
    /// The version the caller believes is current.
    pub expected_version: ApprovalVersion,
    /// The fingerprint the caller reviewed.
    pub fingerprint: &'a ActionDigest,
    /// The operator's own comment, when one was given.
    ///
    /// **Present because the wire carried a `comment` whose doc claimed it was stored and nothing stored
    /// it.** An absent note is `None` and distinct from an empty one, which `DecisionNote::new` refuses.
    pub note: Option<&'a DecisionNote>,
}

/// Why an approval operation could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalServiceError {
    /// No approval with that identifier exists in the caller's workspace.
    ///
    /// **Indistinguishable from a foreign-workspace record**, which the contract requires: "A
    /// foreign-workspace record is indistinguishable from a missing record." The repository enforces
    /// it, and this variant is the only way a miss is reported, so a client cannot tell them apart.
    NotFound,
    /// The caller is not permitted to inspect or decide this approval.
    ///
    /// Carries a code rather than a message, because the contract's stable-error list distinguishes
    /// the cases and a client keys on them: a scope refusal tells a user to seek authority, while a
    /// channel refusal tells them to use a different surface.
    ScopeDenied {
        /// The stable, named code for the refusal.
        code: &'static str,
    },
    /// The request lapsed before the operation.
    Expired,
    /// The action the caller reviewed is not the action the approval authorizes.
    FingerprintMismatch,
    /// The caller's stated version is not the stored version.
    VersionConflict {
        /// The version the caller stated.
        expected: u64,
        /// The version the store holds.
        actual: u64,
    },
    /// A one-shot approval has already been spent.
    AlreadyConsumed,
    /// The record is in a position the requested operation cannot move it from.
    ///
    /// Distinct from [`Self::AlreadyConsumed`] and from a version conflict, because the caller's
    /// remedy differs: a decision is terminal, so re-reading cannot fix it.
    StateConflict,
    /// The caller made a malformed request: an unknown verb, an unbounded reason, or an unusable
    /// version.
    Invalid {
        /// The stable, named code for what was wrong.
        code: &'static str,
    },
    /// The caller held no verified identity.
    Unauthenticated,
    /// The caller proved an identity, but not at the level this operation requires.
    InsufficientAssurance,
    /// The caller's page cursor was not bound to the query it is being used for.
    ///
    /// **A refusal rather than an empty page**, because the cursor carries the channel it was minted
    /// for and the two answer different questions: an empty page would tell the caller "nothing
    /// follows", while the truth is "this position is not meaningful against this view". Reporting
    /// the second as the first would also let a caller probe another channel's rows by observing
    /// whether a page came back empty.
    InvalidCursor,
    /// The store could not be read or written.
    Storage(RepositoryError),
}

impl ApprovalServiceError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "approval.not_found",
            // Both carry the code the layer that refused produced, rather than a variant name: the
            // caller needs the specific reason, and the equality here is REAL rather than incidental —
            // a scope refusal's code names which authority was missing, and an invalid request's names
            // what was wrong with the body.
            Self::ScopeDenied { code } | Self::Invalid { code } => code,
            Self::InvalidCursor => "request.invalid_cursor",
            Self::Expired => "approval.expired",
            Self::FingerprintMismatch => "approval.fingerprint_mismatch",
            Self::VersionConflict { .. } => "approval.version_conflict",
            Self::AlreadyConsumed => "approval.already_consumed",
            Self::StateConflict => "approval.state_conflict",
            // An identity never proved reports this surface's own authentication code rather than an
            // approval code, following the decision `policy_service` recorded: sending an operator to
            // inspect an approval for a request that never authenticated is the wrong place to look.
            Self::Unauthenticated => "auth.credential_rejected",
            Self::InsufficientAssurance => "approval.assurance_insufficient",
            Self::Storage(error) => error.code(),
        }
    }

    /// Returns whether retrying the same request unchanged could succeed.
    ///
    /// A version conflict and a storage fault can — the remedy is a re-read and a recomputation — and
    /// nothing else can. A decision is terminal, so a state conflict repeats forever; a lapsed request
    /// stays lapsed; and a channel refusal is a property of the *surface* the caller used, so
    /// resending on the same one reaches the same refusal.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::VersionConflict { .. } => true,
            Self::Storage(error) => error.retryable(),
            Self::NotFound
            | Self::ScopeDenied { .. }
            | Self::Expired
            | Self::FingerprintMismatch
            | Self::AlreadyConsumed
            | Self::StateConflict
            | Self::Invalid { .. }
            | Self::InvalidCursor
            | Self::Unauthenticated
            | Self::InsufficientAssurance => false,
        }
    }
}

/// Reads and decides approval records.
pub struct ApprovalService {
    approvals: Arc<dyn ApprovalRepository>,
}

impl ApprovalService {
    /// Builds the service over an approval store.
    ///
    /// Takes a trait object rather than a concrete repository, so a test drives the use cases with the
    /// in-memory double and the daemon with SQLite — and so this module cannot accidentally depend on
    /// a SQL type, which would put storage in the application layer.
    #[must_use]
    pub fn new(approvals: Arc<dyn ApprovalRepository>) -> Self {
        Self { approvals }
    }

    /// Lists the approvals awaiting a decision visible to the caller.
    ///
    /// **The channel filter is the store's, not this method's.** An earlier version read the
    /// workspace's pending rows and filtered by `allowed_channels` here — which was wrong in a way that
    /// silently short-changed an operator: the page bound had already been applied to rows the caller
    /// could not decide, so a page could come back with fewer rows than its bound while more decidable
    /// approvals existed, and a client that received a short list concludes there is nothing more to
    /// act on. The predicate belongs where the `LIMIT` is, so the bound applies to what the caller can
    /// actually see.
    ///
    /// The store is still **unscoped by principal** on purpose: an operator view has to show every
    /// pending prompt in the workspace, and a listing scoped to one principal would hide the ones
    /// waiting on somebody else.
    ///
    /// `bounded` tells the caller whether the store stopped at its bound, which is the fact a client
    /// needs to decide whether to keep reading — and `next` is the cursor that makes "keep reading"
    /// possible. **The doc here used to say this build "serves no cursor" and that a cursor would
    /// "claim a stable position this listing does not yet guarantee across a concurrent decision";
    /// that was wrong about the position.** The listing's order is `(expires_at, id)`, which is total
    /// and stable: `expires_at` is an immutable column and `id` is primary. What a concurrent decision
    /// changes is *membership*, not order — so a keyset cursor is exactly as stable as the order it
    /// names, and an offset would be the unstable choice.
    ///
    /// `after` resumes from a cursor a previous page produced. It is passed to the store rather than
    /// applied here, because the bound has to run where the `LIMIT` runs: filtering a page here would
    /// apply the limit to the wrong window.
    ///
    /// **`filter` is a narrowing the store applies to the same `WHERE` the `LIMIT` bounds**, for the
    /// reason above: a risk filter applied here, over an already-bounded page, would return fewer rows
    /// than the caller asked for on a query that has more — a short page, which a client reads as "no
    /// more to decide". It is threaded to the store and to the expiry sweep's re-read alike, so the
    /// page the caller receives and the page the sweep replaces are the same view.
    ///
    /// `limit` is clamped to the store's bound rather than refused, so a client asking for more
    /// receives a full page.
    ///
    /// # Errors
    ///
    /// Returns [`ApprovalServiceError::Unauthenticated`] for a guest and
    /// [`ApprovalServiceError::Storage`] when the store cannot be read.
    pub async fn list(
        &self,
        context: &RequestContext,
        filter: ApprovalListFilter,
        limit: u32,
        after: Option<ApprovalCursor>,
        at: UtcTimestamp,
    ) -> Result<ApprovalsPage, ApprovalServiceError> {
        // A guest is refused before the read, so an unauthenticated caller cannot learn anything about
        // the workspace's queue.
        let _ = assurance_of(context.assurance)?;
        let limit = limit.min(MAX_PENDING_PAGE);
        let channel = approval_channel_of(context.channel);
        // **A cursor for another channel is refused rather than honoured.** The cursor carries the
        // channel it was bound to, and the store filters by *that* value; accepting one minted for a
        // different channel would let a caller page through a view it cannot otherwise ask for, which
        // is how a listing becomes a probe for what it excludes. Refused as an invalid request rather
        // than as an empty page, because an empty page would answer the question the probe asked.
        if let Some(cursor) = after
            && cursor.channel != channel
        {
            return Err(ApprovalServiceError::InvalidCursor);
        }
        // **A cursor minted under a different risk narrow is refused, and the harm is a SKIPPED row.**
        // The position is the *last row of a narrowed page*, so replaying it under another narrow makes
        // the `>` comparison skip every row that expires before it — rows the caller asked for and would
        // never see, which is the "a skipped approval is a prompt nobody decides" failure the keyset
        // bound exists to prevent, arriving through the filter rather than through an offset. A cursor
        // with **no** recorded narrow is tolerated: it names a superset position, so no row a narrower
        // view wanted can fall behind it.
        if let Some(cursor) = after
            && cursor.risk.is_some()
            && cursor.risk != filter.risk
        {
            return Err(ApprovalServiceError::InvalidCursor);
        }
        let page = self
            .approvals
            .pending_in(context.workspace_id, channel, filter, limit, after)
            .await
            .map_err(ApprovalServiceError::Storage)?;
        // **The contract's "expiry is evaluated on every read", which this method did not do.** A listing
        // is the surface a *prompt* appears on, so it is the one place a lapsed record must not be offered:
        // a client that received it would render a decision the daemon then refuses as expired, and — worse
        // — the page budget would be spent on rows nobody can act on.
        Ok(self
            .expire_lapsed(context, page, filter, limit, after, at)
            .await)
    }

    /// Reads one approval, scoped to the caller's workspace.
    ///
    /// A record whose `allowed_channels` exclude the caller's channel is
    /// [`ApprovalServiceError::ScopeDenied`] with `approval.channel_not_allowed` — not a `not_found`,
    /// because the two send a user to different places: a missing record means the identifier is
    /// wrong, while a channel refusal means the surface is.
    ///
    /// # Errors
    ///
    /// Returns [`ApprovalServiceError::NotFound`] for an absent or foreign-workspace record,
    /// [`ApprovalServiceError::ScopeDenied`] when the caller's channel may not decide it, and
    /// [`ApprovalServiceError::Storage`] when the store cannot be read.
    pub async fn read(
        &self,
        context: &RequestContext,
        approval: ApprovalId,
        at: UtcTimestamp,
    ) -> Result<DurableApproval, ApprovalServiceError> {
        let _ = assurance_of(context.assurance)?;
        let stored = self.load(context, approval).await?;
        // The channel is authorized against the record as stored, before the lapse is recorded: a caller
        // whose channel may not decide this approval must be refused the same way whether or not the
        // deadline has passed, or the refusal would leak which surface was permitted.
        authorize_deciding(context.channel, &stored)?;
        drop(stored);
        // **The contract's "expiry is evaluated on every read", which this method did not do.** A detail
        // read is how a client checks what it is about to decide — and how an operator surface *shows* a
        // prompt — so a lapsed record reported here would render a decision the daemon then refuses. The
        // lapse is **recorded** and the re-read record returned, so the caller sees the state its refusal
        // came from rather than a `pending` row that no longer means what it says.
        //
        // `at` is a parameter for the same reason [`Self::decide`] takes one: a read that consulted its own
        // clock could not be replayed, and the fixture's deadline would have to be far in the future for
        // every unrelated test.
        self.expire_on_read(context, approval, at).await
    }

    /// Applies a decision to an approval.
    ///
    /// The order of the checks is the contract's own `decide` list, and each position is deliberate:
    ///
    /// 1. the identity and assurance are resolved first, so an unauthenticated caller never reaches
    ///    the store and cannot learn whether an identifier exists;
    /// 2. the record is loaded in the caller's workspace;
    /// 3. the caller's channel is checked against the request;
    /// 4. a lapsed request is **expired and then refused**, so the prompt leaves every later listing;
    /// 5. the fingerprint is compared **before** the version, because a re-approval cannot fix a
    ///    digest that still will not match — reporting "stale version" would send the user to do
    ///    exactly that;
    /// 6. a repeat of the same decision returns the record with `applied: false`;
    /// 7. the transition is applied through the domain's own `apply`, so no illegal edge is written.
    ///
    /// `at` is a parameter rather than a clock read, so a decision is reproducible and a test can
    /// assert the exact instant it recorded.
    ///
    /// # Errors
    ///
    /// Returns the variants named on [`ApprovalServiceError`], each for the condition its doc states.
    pub async fn decide(
        &self,
        context: &RequestContext,
        approval: ApprovalId,
        command: DecisionCommand<'_>,
        at: UtcTimestamp,
    ) -> Result<DecidedApproval, ApprovalServiceError> {
        let DecisionCommand {
            decision,
            expected_version,
            fingerprint,
            note,
        } = command;
        let _ = assurance_of(context.assurance)?;
        let mut stored = self.load(context, approval).await?;
        authorize_deciding(context.channel, &stored)?;
        // **The contract's "checks current channel and assurance against the approval request/policy",
        // which was unimplemented.** The assurance was resolved and *recorded* but never *required*, so
        // `approval.assurance_insufficient` sat in the contract's stable-error list with no producer,
        // and a critical action could be decided by an ordinary session — the one prompt the risk label
        // exists to make the user step up for. Placed here, after the channel check, because the two
        // refusals send the user to different places: a channel refusal means "use another surface",
        // while an assurance refusal means "prove who you are again" on the surface they are already on.
        require_deciding_assurance(context.assurance, &stored)?;

        if stored.is_lapsed_at(at) {
            self.expire(context, &stored, at).await;
            return Err(ApprovalServiceError::Expired);
        }
        if stored.state() == ApprovalState::Consumed {
            return Err(ApprovalServiceError::AlreadyConsumed);
        }
        if stored.action_digest != *fingerprint {
            return Err(ApprovalServiceError::FingerprintMismatch);
        }

        let target = decision.state();
        // **A repeat of the same decision is idempotence, and it is checked BEFORE the version.**
        // The contract requires that "same-key/same-request retry returns the original decision", and
        // a same-request retry carries the `expected_version` from the *original* body — so a version
        // check first would refuse the ordinary double-tap as stale and defeat the rule. The state is
        // what the caller asked for, nothing is overwritten, and `applied: false` says this call did
        // not perform it, so the answer is honest as well as idempotent.
        if stored.state() == target {
            return Ok(DecidedApproval {
                approval: stored,
                applied: false,
            });
        }
        if stored.version() != expected_version {
            return Err(ApprovalServiceError::VersionConflict {
                expected: expected_version.get(),
                actual: stored.version().get(),
            });
        }
        let actor = ApprovalActor::Decided {
            principal: context.principal_id,
            channel: approval_channel_of(context.channel),
            // The assurance the caller **proved**, which the server resolved from the credential — the
            // contract's audit section requires it recorded, and it is the fact that distinguishes a
            // decision by a stepped-up caller from one by an ordinary session. Taken from the context
            // rather than from the request body, for the `BRN-024` reason: a caller-supplied assurance
            // would let whoever filled in the body choose how strong the decision looked.
            assurance: assurance_of(context.assurance)?,
            // The operator's own comment, when the request carried one. The wire comment was declared
            // with a doc saying "stored with the decision" while nothing stored it, so it reached this
            // layer and stopped — the caller's note is now part of the transition the caller authored.
            note: note.cloned(),
        };
        let transition = stored
            .apply(target, expected_version, actor, at)
            .map_err(|error| map_domain_refusal(&error))?;
        let outcome = self
            .approvals
            .apply_transition(
                context.workspace_id,
                &transition,
                expected_version,
                &transition.actor,
                &stored,
            )
            .await
            .map_err(map_storage_refusal)?;
        Ok(DecidedApproval {
            approval: stored,
            applied: matches!(outcome, DecideOutcome::Applied),
        })
    }

    /// Withdraws a pending or approved-but-unconsumed request.
    ///
    /// **The requesting principal, or a caller on a permitted channel.** The contract names both, and
    /// the distinction is why a channel check alone is not enough: the requester may always withdraw
    /// its own request, which is the one case where the channel does not apply — a principal that
    /// asked on the CLI and then authenticated on the desktop is still the principal that asked.
    ///
    /// `reason` is caller text and is bounded. It is **not** placed on any public event by this layer,
    /// so no escaping is needed here; the bound is what stops one request writing an unbounded row.
    ///
    /// # Errors
    ///
    /// Returns [`ApprovalServiceError::StateConflict`] for a decided, consumed, expired, or rejected
    /// record — the contract says those are immutable and an idempotent repeat returns the current
    /// state, which the `cancelled` case below does — and the other variants as [`Self::decide`]
    /// documents.
    pub async fn cancel(
        &self,
        context: &RequestContext,
        approval: ApprovalId,
        expected_version: ApprovalVersion,
        reason: Option<&DecisionNote>,
        at: UtcTimestamp,
    ) -> Result<DecidedApproval, ApprovalServiceError> {
        let _ = assurance_of(context.assurance)?;
        let mut stored = self.load(context, approval).await?;
        authorize_cancelling(context, &stored)?;

        if stored.state() == ApprovalState::Cancelled {
            return Ok(DecidedApproval {
                approval: stored,
                applied: false,
            });
        }
        // Only the two cancellable positions reach the transition. Every other state is a decision, a
        // spend, or a lapse, and the contract makes those immutable.
        if stored.state() != ApprovalState::Pending && stored.state() != ApprovalState::Approved {
            return Err(ApprovalServiceError::StateConflict);
        }
        if stored.version() != expected_version {
            return Err(ApprovalServiceError::VersionConflict {
                expected: expected_version.get(),
                actual: stored.version().get(),
            });
        }
        let actor = ApprovalActor::Cancelled {
            by: context.principal_id,
            // The caller's own reason, which the contract lists as part of a cancellation and which the
            // wire type declared with a doc claiming the audit trail says *why* a prompt was withdrawn — a
            // claim that was false while nothing stored it.
            reason: reason.cloned(),
        };
        let transition = stored
            .apply(ApprovalState::Cancelled, expected_version, actor, at)
            .map_err(|error| map_domain_refusal(&error))?;
        let outcome = self
            .approvals
            .apply_transition(
                context.workspace_id,
                &transition,
                expected_version,
                &transition.actor,
                &stored,
            )
            .await
            .map_err(map_storage_refusal)?;
        Ok(DecidedApproval {
            approval: stored,
            applied: matches!(outcome, DecideOutcome::Applied),
        })
    }

    /// Returns the note the last human decision on `approval` carried, if any.
    ///
    /// **This exists because the decision's comment and the cancellation's reason were written and never
    /// read.** Both live on the transition the human authored, so reading them means reading the trail —
    /// and the port's `transitions` reader is what makes that possible. The note is taken from the **most
    /// recent deciding transition** rather than from any transition: a consumption or an expiry carries no
    /// note, and reporting the trail's last note regardless of its kind would answer with a comment that
    /// explains a different step.
    ///
    /// A record with no trail, or a trail whose steps are all machine-authored, answers `Ok(None)` — which
    /// is "nobody wrote a comment" and distinct from an error, so a caller can tell "absent" from
    /// "unreadable".
    ///
    /// # Errors
    ///
    /// Returns the variants [`Self::read`] documents.
    pub async fn last_decision_note(
        &self,
        context: &RequestContext,
        approval: ApprovalId,
    ) -> Result<Option<String>, ApprovalServiceError> {
        let _ = assurance_of(context.assurance)?;
        let stored = self.load(context, approval).await?;
        authorize_deciding(context.channel, &stored)?;
        let trail = self
            .approvals
            .transitions(context.workspace_id, approval)
            .await
            .map_err(ApprovalServiceError::Storage)?;
        // The **last decision**, not the last step: a consumption follows a decision and carries no note,
        // so taking the trail's final entry would lose the comment the user actually wrote.
        Ok(trail
            .iter()
            .rev()
            .find_map(|step| step.actor.note().map(DecisionNote::as_str))
            .map(str::to_owned))
    }

    /// Reads the store, mapping its refusals onto this surface's codes.
    async fn load(
        &self,
        context: &RequestContext,
        approval: ApprovalId,
    ) -> Result<DurableApproval, ApprovalServiceError> {
        match self.approvals.load(context.workspace_id, approval).await {
            Ok(stored) => Ok(stored),
            // A foreign-workspace record is `NotFound` from the store, which is exactly the
            // indistinguishable answer the contract requires, so it maps to the same variant.
            Err(RepositoryError::NotFound) => Err(ApprovalServiceError::NotFound),
            Err(error) => Err(ApprovalServiceError::Storage(error)),
        }
    }

    /// Expires a lapsed record and returns it in its **current** state.
    ///
    /// Called by the two read paths, which must not report a `pending` row a caller cannot act on. The
    /// **re-read after the write** is what keeps the answer honest under a race: a concurrent decision
    /// that won while this expiry was in flight leaves the newer state in the store, and returning the
    /// value constructed here would report that decision as still pending.
    ///
    /// A record that has not lapsed is returned untouched, so the common case costs one extra read rather
    /// than a write.
    async fn expire_on_read(
        &self,
        context: &RequestContext,
        approval: ApprovalId,
        at: UtcTimestamp,
    ) -> Result<DurableApproval, ApprovalServiceError> {
        let stored = self.load(context, approval).await?;
        if !stored.is_lapsed_at(at) {
            return Ok(stored);
        }
        self.expire(context, &stored, at).await;
        // Whichever write won, the store holds the answer — so this is the record a caller should see
        // rather than the one this method constructed.
        self.load(context, approval).await
    }

    /// Expires every lapsed record in `page` and re-reads until the page holds none.
    ///
    /// **The listing is the surface a prompt appears on, so it is the one place a lapsed record must not
    /// be offered.** A client that received one would render a decision the daemon then refuses as
    /// expired, and the page budget would be spent on rows nobody can act on — which is the short-page
    /// defect a channel filter already caused here once, arriving by a different route.
    ///
    /// Expiring and re-reading rather than filtering the lapsed rows out is deliberate: **filtering would
    /// return a short page**, and on a surface that serves no cursor a short page is how a client concludes
    /// there is nothing left to decide.
    ///
    /// The loop is **bounded**, and hitting the bound is reported rather than looped past. Each pass that
    /// finds a lapsed row transitions it, so the pending set strictly shrinks and the loop terminates; the
    /// bound is `limit + 1` passes because a pass that removes a row cannot also be the (limit+1)th such
    /// pass over a page of at most `limit` rows. A caller that hits it still receives a page whose
    /// `bounded` flag is the store's own answer, so the fact that more may remain is not lost.
    async fn expire_lapsed(
        &self,
        context: &RequestContext,
        mut page: ApprovalsPage,
        filter: ApprovalListFilter,
        limit: u32,
        after: Option<ApprovalCursor>,
        at: UtcTimestamp,
    ) -> ApprovalsPage {
        for _ in 0..=limit {
            let lapsed: Vec<ApprovalId> = page
                .approvals
                .iter()
                .filter(|approval| approval.is_lapsed_at(at))
                .map(|approval| approval.id)
                .collect();
            if lapsed.is_empty() {
                return page;
            }
            for id in lapsed {
                // A refusal here is a concurrent decision or a store fault, and both are already handled
                // by the re-read below — the fresh page is the answer either way, so the error is not
                // discarded into a wrong value, it is superseded by a newer read of the same fact.
                let _ = self.expire_on_read(context, id, at).await;
            }
            // **The re-read resumes from the caller's own position, not from the first page.** Sweeping
            // a page and then re-reading page one would answer a different question than the caller
            // asked: a request for page three would receive rows from the beginning, and because the
            // cursor it was given would then be the wrong one, the client's next fetch would jump
            // backwards. The bound is the same `after` for every pass, so each re-read replaces exactly
            // the window being swept.
            page = match self
                .approvals
                .pending_in(
                    context.workspace_id,
                    approval_channel_of(context.channel),
                    filter,
                    limit,
                    after,
                )
                .await
            {
                Ok(page) => page,
                // The store refused the re-read, so the page from before the sweep is returned rather than
                // an empty one. An empty page would tell a client the queue is clear, which is a stronger
                // and less true claim than "here is what was read".
                Err(_) => return page,
            };
        }
        page
    }

    /// Records a lapse as the `expired` transition.
    ///
    /// **Recorded rather than only refused**, because a refusal alone leaves the prompt in every later
    /// listing — a pending approval nobody can decide. The actor is [`ApprovalActor::Expired`], naming
    /// the deadline that passed, because the clock caused this and not a person.
    ///
    /// A write refused by a concurrent decision is deliberately swallowed: the caller is about to be
    /// told the request is expired either way, and the concurrent decision is the more recent fact.
    /// Turning that race into a storage fault would report the loser's write as a database problem.
    async fn expire(&self, context: &RequestContext, stored: &DurableApproval, at: UtcTimestamp) {
        let actor = ApprovalActor::Expired {
            expires_at: stored.expires_at,
        };
        let mut lapsed = stored.clone();
        let Ok(transition) = lapsed.apply(ApprovalState::Expired, stored.version(), actor, at)
        else {
            return;
        };
        let _ = self
            .approvals
            .apply_transition(
                context.workspace_id,
                &transition,
                stored.version(),
                &transition.actor,
                &lapsed,
            )
            .await;
    }
}

/// An approval after a decision or cancellation, and whether this call performed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecidedApproval {
    /// The record in its state after the write.
    pub approval: DurableApproval,
    /// Whether this call performed the transition, as opposed to finding it already applied.
    pub applied: bool,
}

/// The verb a decision takes.
///
/// Two variants rather than a bool, so `approve` and `reject` cannot be transposed by a caller passing
/// `true` for the wrong thing — and so the target state has one definition rather than a match at
/// each call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Grant the request.
    Approve,
    /// Refuse the request.
    Reject,
}

impl Decision {
    /// Parses the contract's verb.
    ///
    /// # Errors
    ///
    /// Returns [`ApprovalServiceError::Invalid`] for anything but `approve` and `reject`, because the
    /// contract fixes the allowed values and an unknown verb must not be guessed at.
    pub fn parse(value: &str) -> Result<Self, ApprovalServiceError> {
        match value {
            "approve" => Ok(Self::Approve),
            "reject" => Ok(Self::Reject),
            _ => Err(ApprovalServiceError::Invalid {
                code: "request.invalid",
            }),
        }
    }

    /// Returns the state a decision moves the approval to.
    #[must_use]
    pub const fn state(self) -> ApprovalState {
        match self {
            Self::Approve => ApprovalState::Approved,
            Self::Reject => ApprovalState::Rejected,
        }
    }
}

/// The channel a request arrived on, as an approval channel.
///
/// **Derived from the context, never supplied**, which is the contract's "server derives ... channel"
/// rule. Two of the seven `RequestChannel` values have no approval equivalent and map to the closest
/// *deciding* surface: `Web` is the API's browser-facing sibling and `Internal` is the daemon acting
/// for a run, and a daemon-internal decision is still a decision the `api` surface made. Mapping them
/// explicitly keeps this a total function — a `_ =>` arm would let a channel added later silently
/// inherit whichever branch happened to be last, which is the defect the `rejection_reason` spelling
/// bug had.
pub(crate) fn approval_channel_of(channel: RequestChannel) -> ApprovalChannel {
    match channel {
        RequestChannel::Cli => ApprovalChannel::Cli,
        RequestChannel::Desktop => ApprovalChannel::Desktop,
        RequestChannel::Mobile => ApprovalChannel::Mobile,
        RequestChannel::Voice => ApprovalChannel::Voice,
        RequestChannel::Api | RequestChannel::Web | RequestChannel::Internal => {
            ApprovalChannel::Api
        }
    }
}

/// Refuses a caller whose channel may not decide this approval.
fn authorize_deciding(
    channel: RequestChannel,
    stored: &DurableApproval,
) -> Result<(), ApprovalServiceError> {
    if !stored
        .allowed_channels
        .permits(approval_channel_of(channel))
    {
        return Err(ApprovalServiceError::ScopeDenied {
            code: "approval.channel_not_allowed",
        });
    }
    Ok(())
}

/// Refuses a caller whose assurance does not meet what the action's risk requires.
///
/// **The step-up rule, enforced rather than only recorded.** `approval.assurance_insufficient` is in
/// the contract's stable-error list and had no producer: the assurance reached `ApprovalActor::Decided`
/// as an audit fact while nothing compared it against a requirement, so "critical actions default to
/// step-up" was a sentence in the contract with no code behind it. The requirement comes from
/// [`RequiredAssurance::required_for`] — one place, so this surface and any later one cannot disagree
/// about the threshold.
///
/// The comparison is [`RequiredAssurance::is_satisfied_by`], which is a ladder: an elevated caller
/// satisfies a standard requirement. Writing it as an equality would refuse a more strongly
/// authenticated caller, which is the one refusal no operator wants and the one that pushes toward
/// weakening the requirement.
///
/// # Errors
///
/// Returns [`ApprovalServiceError::InsufficientAssurance`] when the caller holds less than the action
/// requires.
fn require_deciding_assurance(
    held: AuthenticationAssurance,
    stored: &DurableApproval,
) -> Result<(), ApprovalServiceError> {
    // A guest never reaches here — `assurance_of` refuses one before the store is read — so the
    // refusal below can only mean "proved an identity, but not strongly enough", which is what the
    // variant's own doc says and what makes the code's remedy ("re-authenticate harder") correct.
    let held = assurance_of(held)?;
    if RequiredAssurance::required_for(stored.risk).is_satisfied_by(held) {
        Ok(())
    } else {
        Err(ApprovalServiceError::InsufficientAssurance)
    }
}

/// Refuses a caller who is neither the requester nor on a permitted channel.
fn authorize_cancelling(
    context: &RequestContext,
    stored: &DurableApproval,
) -> Result<(), ApprovalServiceError> {
    if stored.requesting_principal == context.principal_id {
        return Ok(());
    }
    authorize_deciding(context.channel, stored).map_err(|_| ApprovalServiceError::ScopeDenied {
        code: "approval.scope_denied",
    })
}

/// Maps a store refusal from a transition onto this surface's codes.
///
/// **A version conflict is a `409` resource conflict, not a storage fault.** The port reports a stale
/// writer as [`RepositoryError::VersionConflict`], and passing that through unchanged would reach a
/// client as `storage.version_conflict` under a `500` — telling it the daemon had faulted when its own
/// view was merely stale, which is the one case where a retry after a re-read works. Round 57 fixed
/// exactly this on the run surface; the same conflict here is the same fact, so it is the same code.
fn map_storage_refusal(error: RepositoryError) -> ApprovalServiceError {
    match error {
        RepositoryError::VersionConflict { expected, actual } => {
            ApprovalServiceError::VersionConflict { expected, actual }
        }
        // A refused edge means the domain's table and the caller disagree, which is a state conflict
        // rather than a store fault — the caller's remedy is to re-read, not to retry blindly.
        RepositoryError::TransitionRefused { .. } => ApprovalServiceError::StateConflict,
        other => ApprovalServiceError::Storage(other),
    }
}

/// Maps a domain transition refusal onto this surface's codes.
///
/// The domain's `ApprovalVersionConflict` and `ApprovalStateConflict` are the two refusals `apply`
/// produces; mapping them here keeps the wire codes in one place rather than at each call site. Any
/// other domain error is a state conflict, because every other rule `apply` enforces is about the
/// position the record is in — and a channel refusal never reaches here, since the channel is checked
/// before the transition is attempted.
fn map_domain_refusal(error: &jarvis_domain::error::DomainError) -> ApprovalServiceError {
    match error {
        jarvis_domain::error::DomainError::ApprovalVersionConflict { expected, actual } => {
            ApprovalServiceError::VersionConflict {
                expected: expected.get(),
                actual: actual.get(),
            }
        }
        _ => ApprovalServiceError::StateConflict,
    }
}

/// Resolves the assurance level a verified caller holds, refusing an anonymous one.
///
/// **The ladder itself lives in [`crate::policy_service`]**, because a grant and a decision are the
/// same kind of authorization — a person permitting an action — and two copies of the ladder is how
/// they come to disagree about what `Standard` permits. This function exists only to translate a
/// refusal into this surface's error type, so the *rule* has one home even though the *code* differs.
///
/// **This resolves the level; it does not decide what is required.** The requirement is derived from
/// the record's own risk by [`RequiredAssurance::required_for`] and compared against this value in
/// [`require_deciding_assurance`], which is the real producer of
/// [`ApprovalServiceError::InsufficientAssurance`]. An earlier version of this comment said the
/// required level was always `Standard` because the record named no higher one; that stopped being
/// true the moment the risk-derived threshold was added, so it is corrected here rather than left to
/// mislead a reader into thinking a step-up is unrepresentable.
///
/// **`map_err` to a single variant rather than a `match` with a `_` arm, and the difference is the
/// point.** The previous form matched the shared resolver's nine-variant error and mapped everything
/// unrecognised onto [`ApprovalServiceError::InsufficientAssurance`] — an arm that **could never
/// fire**, because `policy_service::required_assurance_of` returns only `Unauthenticated`. So the arm
/// was unreachable code that nonetheless made the variant look produced, and nothing in the workspace
/// produced it: the same "a refusal that can never fire is dead protection" shape as the
/// terminal-state guard in `ApprovalState::can_transition_to`. Collapsing to `map_err` states the
/// function's actual contract — *any* failure to resolve an assurance here means the caller proved no
/// identity — which is true precisely because the shared resolver has one failure mode, and the test
/// that pins it is `a_guest_cannot_decide_and_is_refused_before_the_record_is_read`.
///
/// The rule itself is **delegated rather than restated**, so this surface and the policy surface
/// cannot disagree about what a guest is.
///
/// # Errors
///
/// Returns [`ApprovalServiceError::Unauthenticated`] for a guest, because an anonymous caller cannot
/// be the principal a decision is attributed to.
fn assurance_of(held: AuthenticationAssurance) -> Result<RequiredAssurance, ApprovalServiceError> {
    crate::policy_service::required_assurance_of(held)
        .map_err(|_| ApprovalServiceError::Unauthenticated)
}

#[cfg(test)]
#[path = "approval_service/tests.rs"]
mod tests;
