//! The SQLite implementation of the approval repository port.
//!
//! This is the join between `000008_approvals.sql` and [`ApprovalRepository`], which were built in the
//! previous slice: the tables existed and the port compiled, and nothing connected them.
//!
//! **The one property that matters more than the SQL**: a decision's state change and its transition
//! row commit **together**. `docs/architecture/storage-data.md` names the run's equivalent as the first
//! required atomic use case, and the same reasoning applies to an approval — a trail row describing a
//! decision that is not durable, or a durable decision with no trail row, are both states where the
//! audit record and the thing it audits disagree.
//!
//! **Two reading rules are inherited rather than reinvented.** A state the domain does not know is
//! [`RepositoryError::Corrupted`], never "absent": skipping an uninterpretable row converts a migration
//! problem into apparent data loss. And an approval owned by another workspace is
//! [`RepositoryError::NotFound`] **indistinguishable from one that does not exist**, which is the rule
//! the local control API states for runs.
//!
//! ## Why the identity is stored as JSON and rebuilt through the domain type
//!
//! `tool_identity_json` holds the serialized [`ToolIdentity`], and reading it **deserializes through the
//! domain type** rather than reconstructing fields here. That is deliberate and it is the whole reason
//! the column is one document: the identity is a tuple whose canonical spelling the domain owns, and an
//! adapter that assembled one from columns would be a second definition of what an identity is. A stored
//! identity that no longer parses is `Corrupted`, because it means the *shape* changed under a row
//! JARVIS wrote — which is a migration fault, not a missing record.

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use jarvis_application::repository::approval::{
    ApprovalCursor, ApprovalListFilter, ApprovalRepository, ApprovalsPage, DecideOutcome,
    MAX_PENDING_PAGE,
};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{ApprovalId, PrincipalId, RunId, ToolCallId, WorkspaceId};
use jarvis_domain::model::exception::RequiredAssurance;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    AllowedChannels, ApprovalActor, ApprovalChannel, ApprovalPreview, ApprovalScopeKind,
    ApprovalState, ApprovalSummary, ApprovalTransitionRecord, ApprovalVersion, DurableApproval,
    PreviewItem,
};
use jarvis_domain::tool::canonical::ActionDigest;
use jarvis_domain::tool::classification::{Effect, Risk};
use jarvis_domain::tool::identity::ToolIdentity;

use super::repositories::{
    assurance_from_stored, assurance_stored, begin_write, int, opt_text, parse_time, text,
};

/// The approval column list every `SELECT` shares, as a macro so it is written once.
///
/// **One definition, interpolated into each statement by [`concat!`].** Four hand-repeated copies stood
/// here before, and the comment over them said "the three statements below therefore repeat the list"
/// while **four** did — so a wrong count was hiding exactly the drift the comment warned about.
/// `sqlx::query` refuses a string built at run time (it cannot audit one for injection, correctly), so
/// the interpolation is textual: this expands to a `&'static str` literal that `sqlx::query` accepts, and
/// [`concat!`] accepts a macro call as one of its arguments, so the column list stays one definition while
/// every statement remains a compile-time literal.
macro_rules! approval_columns {
    () => {
        "id, workspace_id, requesting_principal_id, run_id, tool_call_id, \
         tool_identity_json, action_fingerprint, risk, effects_json, summary, preview_json, \
         allowed_channels_json, expires_at, scope, state, version, decided_by, decided_via, \
         decided_assurance, decided_at, created_at, updated_at"
    };
}

/// The `SELECT` for [`ApprovalRepository::load`].
const LOAD_SQL: &str = concat!(
    "SELECT ",
    approval_columns!(),
    " FROM approvals WHERE workspace_id = ? AND id = ?"
);

/// The `SELECT` for [`ApprovalRepository::decided_by`], most recent first.
///
/// The tie-break on `id` is the same requirement the pending listing records: `decided_at` alone is not
/// total, and an order that is not total can show one decision twice and hide another. It is a plain
/// constant rather than a [`QueryBuilder`] statement because its parameter list is fixed — the builder
/// exists for the listing, whose predicates vary.
const DECIDED_SQL: &str = concat!(
    "SELECT ",
    approval_columns!(),
    " FROM approvals WHERE workspace_id = ? AND decided_by = ? ORDER BY decided_at DESC, id ASC LIMIT ?"
);

/// The `SELECT` for [`ApprovalRepository::transitions`], oldest first.
///
/// **The order is `(occurred_at, id)` and the identifier is load-bearing.** A trail is read forwards, and
/// an append-only identifier breaks an instant tie so a boundary inside a same-instant group cannot show
/// one step twice and hide another — the same argument the pending listing records for its deadine tie.
/// Two transitions of one approval written in the same millisecond is not hypothetical: a decision and its
/// consumption can follow each other faster than the clock's resolution.
const TRANSITIONS_SQL: &str = "SELECT id, approval_id, workspace_id, from_state, to_state, \
     prior_version, version, actor_kind, actor_json, occurred_at FROM approval_transitions \
     WHERE approval_id = ? AND workspace_id = ? ORDER BY occurred_at ASC, id ASC";

/// The SQLite-backed approval repository.
#[derive(Debug, Clone)]
pub struct SqliteApprovalRepository {
    pool: SqlitePool,
}

impl SqliteApprovalRepository {
    /// Builds the repository over an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Fetches an approval's row, applying the scope rule.
    ///
    /// **An inherent method rather than a port method, and that is the point of the split.** The port
    /// describes what a *caller* may ask for; this is how two of the adapter's own reads share one scope
    /// rule. Promoting it to the trait would put a row-shaped method on a port that speaks in domain
    /// values, and leaving the two reads to each write their own predicate is how a foreign-workspace
    /// trail comes to answer with the wrong code — the "two implementations of one rule" defect this
    /// project keeps finding. An approval owned by another workspace is `NotFound`, indistinguishable from
    /// absent, by design.
    async fn load_row(
        &self,
        workspace: WorkspaceId,
        approval: ApprovalId,
    ) -> Result<sqlx::sqlite::SqliteRow, RepositoryError> {
        let row = sqlx::query(LOAD_SQL)
            .bind(workspace.to_string())
            .bind(approval.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
        row.ok_or(RepositoryError::NotFound)
    }
}

#[cfg(test)]
#[path = "approval_repository_tests.rs"]
mod tests;

impl ApprovalRepository for SqliteApprovalRepository {
    fn request(&self, approval: &DurableApproval) -> RepositoryFuture<'_, ()> {
        // Owned before the future is built, because the returned future is bounded by `&self` rather
        // than by the argument — so an async block borrowing `approval` would outlive it. The clone is
        // of one bounded record and happens once per insert.
        let approval = approval.clone();
        Box::pin(async move {
            // The request path never inserts a decided approval: the state is written from the domain
            // value, which is `PENDING` by construction, and the three decision columns are `NULL`.
            // Asserted rather than assumed, because a row that arrived here already decided would be a
            // decision nobody made.
            if approval.state() != ApprovalState::Pending {
                return Err(RepositoryError::Conflict {
                    what: "approval_not_pending_on_insert",
                });
            }
            let inserted = sqlx::query(
                "INSERT INTO approvals (id, workspace_id, requesting_principal_id, run_id, \
                 tool_call_id, tool_identity_json, action_fingerprint, risk, effects_json, summary, \
                 preview_json, allowed_channels_json, expires_at, scope, state, version, created_at, \
                 updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(approval.id.to_string())
            .bind(approval.workspace.to_string())
            .bind(approval.requesting_principal.to_string())
            .bind(approval.run.to_string())
            .bind(approval.tool_call.to_string())
            .bind(serialize_identity(&approval.identity)?)
            .bind(approval.action_digest.to_string())
            .bind(approval.risk.as_contract_str())
            .bind(serialize_effects(&approval.effects)?)
            .bind(approval.summary.as_str())
            .bind(serialize_preview(&approval.preview)?)
            .bind(serialize_channels(&approval.allowed_channels)?)
            .bind(approval.expires_at.to_string())
            .bind(approval.scope.as_contract_str())
            .bind(approval.state().as_contract_str())
            .bind(
                i64::try_from(approval.version().get())
                    .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
            )
            .bind("1970-01-01T00:00:00Z")
            .bind("1970-01-01T00:00:00Z")
            .execute(&self.pool)
            .await;
            match inserted {
                Ok(_) => Ok(()),
                // A duplicate identifier is a caller bug rather than an idempotent retry, since the
                // identifier is generated at construction and a caller cannot name the same approval
                // twice. Reported as a conflict so the caller sees a rule rather than a driver error.
                Err(_) => Err(RepositoryError::Conflict {
                    what: "approval_already_exists",
                }),
            }
        })
    }

    fn load(
        &self,
        workspace: WorkspaceId,
        approval: ApprovalId,
    ) -> RepositoryFuture<'_, DurableApproval> {
        Box::pin(async move {
            let row = self.load_row(workspace, approval).await?;
            stored_approval(&row)
        })
    }

    fn apply_transition(
        &self,
        workspace: WorkspaceId,
        transition: &ApprovalTransitionRecord,
        expected: ApprovalVersion,
        actor: &ApprovalActor,
        approval: &DurableApproval,
    ) -> RepositoryFuture<'_, DecideOutcome> {
        let approval = approval.clone();
        let transition = transition.clone();
        let actor = actor.clone();
        Box::pin(async move {
            let mut tx = begin_write(&self.pool).await?;

            // The stored row is read first so the refusals can be ordered the way the domain orders
            // them: version, then state. Classifying from a zero-row `UPDATE` cannot do that, because
            // "no rows matched" does not say which predicate failed — the same reasoning
            // `RunRepository::transition` records for runs.
            let row = sqlx::query(
                "SELECT state, version FROM approvals WHERE workspace_id = ? AND id = ?",
            )
            .bind(workspace.to_string())
            .bind(approval.id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };
            let stored_state = parse_approval_state(&text(&row, "state")?)?;
            let stored_version = stored_version(&row)?;

            // **The already-in-state case is checked before the version.** A user who taps "approve"
            // twice sends two requests built from one read, and the second must be idempotent rather
            // than a conflict — the caller's intent is satisfied and nothing was overwritten.
            //
            // **The comparison is against the transition's own version, not the caller's `expected`,
            // and that distinction is the whole branch.** Both taps carry `expected = 1`, because both
            // were built from the same read; the stored row is at version 2 after the first. So a
            // condition requiring `stored_version == expected` can never be true here and the branch
            // would be dead — while the condition that means "this change has already happened" is
            // `stored_version == transition.version`, since the transition's version is the version it
            // produces. The first version of this code used `expected`, and the double-tap test failed
            // with a version conflict, which is precisely the lie the branch exists to prevent.
            if stored_state == transition.to && stored_version == transition.version {
                return Ok(DecideOutcome::AlreadyInState);
            }
            if stored_version != expected {
                return Err(RepositoryError::VersionConflict {
                    expected: expected.get(),
                    actual: stored_version.get(),
                });
            }
            // The edge is judged from the state the approval is *actually* in, and the domain's table
            // is the single authority — the adapter asks it rather than duplicating it.
            if !stored_state.can_transition_to(transition.to) {
                return Err(RepositoryError::TransitionRefused {
                    code: "approval.state_conflict",
                });
            }

            // The optimistic predicate stays, so a writer that slipped in between the read and this
            // statement is refused rather than overwriting a decision.
            let updated = sqlx::query(
                "UPDATE approvals SET state = ?, version = ?, decided_by = ?, decided_via = ?, \
                 decided_assurance = ?, decided_at = ?, updated_at = ? \
                 WHERE workspace_id = ? AND id = ? AND version = ?",
            )
            .bind(approval.state().as_contract_str())
            .bind(
                i64::try_from(approval.version().get())
                    .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
            )
            .bind(approval.decided_by().map(|who| who.to_string()))
            .bind(approval.decided_via().map(ApprovalChannel::as_contract_str))
            // The assurance comes from the **approval**, not from this transition's actor, and the first
            // version of this line got it wrong: taking it from the actor meant a later consumption or
            // invalidation wrote NULL over the decision's assurance, so the record lost a fact its own
            // transition row still held. It sits beside `decided_by` and `decided_via` because all three
            // are properties of the decision on this record rather than of one transition.
            .bind(approval.decided_assurance().map(assurance_stored))
            .bind(
                transition
                    .occurred_at
                    .to_string()
                    .pipe_decided_at(approval.is_decided()),
            )
            .bind(transition.occurred_at.to_string())
            .bind(workspace.to_string())
            .bind(approval.id.to_string())
            .bind(
                i64::try_from(expected.get())
                    .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
            )
            .execute(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;
            if updated.rows_affected() == 0 {
                return Err(RepositoryError::VersionConflict {
                    expected: expected.get(),
                    actual: stored_version.get(),
                });
            }

            // The trail row is written in the **same transaction**, so a decision and the record of how
            // it was taken cannot diverge. `actor_kind` is stored beside the payload so a query can
            // count decisions without parsing, and it is what makes "time is not a decider" visible:
            // an `expired` actor carries no principal.
            let (actor_kind, actor_json) = serialize_actor(&actor)?;
            sqlx::query(
                "INSERT INTO approval_transitions (id, approval_id, workspace_id, from_state, \
                 to_state, prior_version, version, actor_kind, actor_json, occurred_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(approval.id.to_string())
            .bind(workspace.to_string())
            .bind(transition.from.as_contract_str())
            .bind(transition.to.as_contract_str())
            .bind(i64::try_from(transition.prior_version.get()).map_err(|_| {
                RepositoryError::Corrupted {
                    column: "prior_version",
                }
            })?)
            .bind(
                i64::try_from(transition.version.get())
                    .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
            )
            .bind(actor_kind)
            .bind(actor_json)
            .bind(transition.occurred_at.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;

            tx.commit().await.map_err(|_| RepositoryError::Query)?;
            Ok(DecideOutcome::Applied)
        })
    }

    fn pending_in(
        &self,
        workspace: WorkspaceId,
        channel: ApprovalChannel,
        filter: ApprovalListFilter,
        limit: u32,
        after: Option<ApprovalCursor>,
    ) -> RepositoryFuture<'_, ApprovalsPage> {
        Box::pin(async move {
            // **Clamped to the port's own bound, not trusted from the caller.** A limit is a request,
            // and an unbounded one over a workspace's approvals is the shape that turns one listing
            // into a full table scan; the contract requires "a bounded page size". Clamping rather
            // than refusing means a client asking for more receives a full page rather than an error.
            let limit = limit.min(MAX_PENDING_PAGE);
            // One row more than asked, so a full page can be told from a complete one. Reading exactly
            // `limit` rows makes "there are more" and "that was all" the same observation — and here
            // that ambiguity makes an operator believe the queue is empty.
            let probe = i64::from(limit.saturating_add(1));
            // **The statement is built from the predicates the request actually has, rather than selected
            // from one per predicate combination.** Four constants stood here — (risk narrowed? × cursor
            // resuming?) × the column list — and the multiplication is the defect: a third *optional*
            // predicate (the `effect`, `state`, or time filters the contract names) would have needed
            // eight statements, each a place a predicate could be wired into the wrong shape. The builder
            // appends each predicate **only when it applies**, so a new filter is one `push` guarded by one
            // condition, and the parameters that follow it stay in the order they were pushed.
            //
            // **Every user value is BOUND, never interpolated.** `push_bind` emits `?` and a bound
            // parameter; the only text this statement interpolates is the column list and the predicates
            // themselves, none of which come from the request. That is the same guarantee the four
            // constants gave, kept while the assembly became conditional.
            //
            // **Why each predicate is in the `WHERE` rather than applied afterwards:** the channel and the
            // risk must bound the rows the `LIMIT` applies to, not filter an already-bounded page — a
            // post-filter returns fewer rows than the caller asked for on a query that has more, and a
            // short page is how a client concludes there is nothing left to decide. The keyset bound is a
            // **row-value** comparison over `(expires_at, id)` matching the `ORDER BY`, because comparing
            // `expires_at` alone drops every row sharing the boundary instant — the tie the identifier
            // exists to break.
            //
            // **The narrowing comes from the cursor on a resume and from the parameters on a fresh page,
            // and that is one rule rather than two.** The channel already behaved this way and the risk now
            // does too, because both are facts a position is bound to. Taking the cursor's values keeps
            // the adapter correct even when called directly rather than through the service — the service
            // refuses a cursor whose channel or risk differs, so on every real request the two agree, and
            // the value that cannot be forged is the one used. A cursor carrying no risk is an *un-narrowed*
            // position, so the caller's narrow is applied over it: the page must still be narrowed, or the
            // caller receives rows its own filter would have excluded.
            let (channel, risk) = match after {
                Some(cursor) => (cursor.channel, cursor.risk.or(filter.risk)),
                None => (channel, filter.risk),
            };
            let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new(concat!(
                "SELECT ",
                approval_columns!(),
                " FROM approvals WHERE workspace_id = "
            ));
            builder
                .push_bind(workspace.to_string())
                .push(
                    " AND state = 'pending' AND EXISTS (SELECT 1 FROM \
                       json_each(approvals.allowed_channels_json) WHERE value = ",
                )
                .push_bind(channel.as_contract_str().to_owned())
                .push(")");
            if let Some(risk) = risk.map(Risk::as_contract_str) {
                builder.push(" AND risk = ").push_bind(risk.to_owned());
            }
            if let Some(cursor) = after {
                builder
                    .push(" AND (expires_at, id) > (")
                    .push_bind(cursor.expires_at.to_string())
                    .push(", ")
                    .push_bind(cursor.id.to_string())
                    .push(")");
            }
            builder
                .push(" ORDER BY expires_at ASC, id ASC LIMIT ")
                .push_bind(probe);
            let rows = builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let bounded = rows.len() > limit as usize;
            let approvals = rows
                .iter()
                .take(limit as usize)
                .map(stored_approval)
                .collect::<Result<Vec<_>, _>>()?;
            // The cursor is the **last row actually returned**, not the probe row: resuming after the
            // probe would skip it, and resuming before the last returned row would repeat it. It carries
            // the narrowing the page was fetched under, so the next page cannot be asked with a different
            // one and skip the rows in between.
            let next = bounded
                .then(|| approvals.last())
                .flatten()
                .map(|last| ApprovalCursor {
                    expires_at: last.expires_at,
                    id: last.id,
                    channel,
                    risk: filter.risk.or(after.and_then(|cursor| cursor.risk)),
                });
            Ok(ApprovalsPage {
                approvals,
                bounded,
                next,
            })
        })
    }

    fn decided_by(
        &self,
        workspace: WorkspaceId,
        principal: PrincipalId,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<DurableApproval>> {
        Box::pin(async move {
            // Clamped for the same reason `pending_in` is: the bound belongs to the read, not to
            // whichever caller named a number.
            let limit = limit.min(MAX_PENDING_PAGE);
            // Most recent first, which the port states as part of the contract rather than leaving to
            // the query plan. `decided_at` is `NULL` for a pending row, and the `decided_by` predicate
            // already excludes those, so the ordering is total over the rows selected.
            let rows = sqlx::query(DECIDED_SQL)
                .bind(workspace.to_string())
                .bind(principal.to_string())
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            rows.iter().map(stored_approval).collect()
        })
    }

    fn transitions(
        &self,
        workspace: WorkspaceId,
        approval: ApprovalId,
    ) -> RepositoryFuture<'_, Vec<ApprovalTransitionRecord>> {
        Box::pin(async move {
            // **The record is loaded first, so the scope rule is the record's rather than the trail's.**
            // A trail exists only for an approval that does, and asking for a foreign workspace's trail
            // must answer `NotFound` exactly as its record does — a query over `approval_transitions`
            // alone would have to re-derive the scope and could disagree with the record about it.
            self.load_row(workspace, approval).await?;
            let rows = sqlx::query(TRANSITIONS_SQL)
                .bind(approval.to_string())
                .bind(workspace.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            rows.iter().map(stored_transition).collect()
        })
    }
}

/// Rebuilds a [`DurableApproval`] from a row.
///
/// The identity, the effects, the preview, and the channels all go through the **domain type's own
/// deserializer**, so a stored value is subject to the same rules as one that arrived over the wire —
/// which is exactly the property `BRN-049` established, and it would be undone by assembling these
/// values field by field here.
fn stored_approval(row: &sqlx::sqlite::SqliteRow) -> Result<DurableApproval, RepositoryError> {
    // The identifier is **read**, and the first version of this function did not read it: it built the
    // value through `DurableApproval::request`, which generates an identifier, and returned it without
    // overwriting the generated one. Every `load` therefore handed back a record whose identity was
    // newly minted, so a caller that loaded an approval and then acted on it named a row that does not
    // exist — which is what `not_found` from a fresh decision looked like. The round-trip test caught it
    // because it compares the identifier as well as the fields.
    let id = ApprovalId::parse(&text(row, "id")?)
        .map_err(|_| RepositoryError::Corrupted { column: "id" })?;
    let workspace = WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "workspace_id",
        }
    })?;
    let requesting_principal =
        PrincipalId::parse(&text(row, "requesting_principal_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "requesting_principal_id",
            }
        })?;
    let run = RunId::parse(&text(row, "run_id")?)
        .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?;
    let tool_call =
        ToolCallId::parse(&text(row, "tool_call_id")?).map_err(|_| RepositoryError::Corrupted {
            column: "tool_call_id",
        })?;
    let identity = stored_tool_identity(row)?;
    let effects: Vec<Effect> = parse_json(&text(row, "effects_json")?, "effects_json")?;
    let preview_items: Vec<PreviewItem> = parse_json(&text(row, "preview_json")?, "preview_json")?;
    let channels: AllowedChannels = parse_json(
        &text(row, "allowed_channels_json")?,
        "allowed_channels_json",
    )?;
    let risk = Risk::parse(&text(row, "risk")?)
        .map_err(|_| RepositoryError::Corrupted { column: "risk" })?;
    let scope = parse_scope(&text(row, "scope")?)?;
    let state = parse_approval_state(&text(row, "state")?)?;
    // **The two text-shaped fields are built through their constructors, not read as raw columns.**
    // Both are rendered in the consent prompt, so the reader is a path each value takes — the same
    // "the reader is a path the value takes" rule the other validated newtypes follow. A row either
    // does not construct is corruption rather than a value carried forward.
    let preview = ApprovalPreview::new(preview_items).map_err(|_| RepositoryError::Corrupted {
        column: "preview_json",
    })?;
    let summary = ApprovalSummary::new(&text(row, "summary")?)
        .map_err(|_| RepositoryError::Corrupted { column: "summary" })?;
    let version = stored_version(row)?;
    let expires_at = parse_time(&text(row, "expires_at")?, "expires_at")?;
    let decided_by = match opt_text(row, "decided_by")? {
        Some(value) => {
            Some(
                PrincipalId::parse(&value).map_err(|_| RepositoryError::Corrupted {
                    column: "decided_by",
                })?,
            )
        }
        None => None,
    };
    let decided_via = match opt_text(row, "decided_via")? {
        Some(value) => Some(parse_channel(&value)?),
        None => None,
    };
    let decided_at = match opt_text(row, "decided_at")? {
        Some(value) => Some(parse_time(&value, "decided_at")?),
        None => None,
    };
    let decided_assurance = stored_assurance(row)?;
    let _ = parse_time(&text(row, "created_at")?, "created_at")?;
    let updated_at = parse_time(&text(row, "updated_at")?, "updated_at")?;

    // Rebuilt through the domain's own request path, so the bounds and the invariants are applied to a
    // stored value exactly as to a fresh one. The private fields are then reached through `apply` and
    // the accessors, which is why the domain type exposes `state()`, `version()`, `decided_by()`, and
    // `decided_via()` rather than public fields.
    let mut approval =
        DurableApproval::request(jarvis_domain::tool::approval::ApprovalRequestParts {
            workspace,
            requesting_principal,
            run,
            tool_call,
            identity,
            action_digest: ActionDigest::parse(&text(row, "action_fingerprint")?).map_err(
                |_| RepositoryError::Corrupted {
                    column: "action_fingerprint",
                },
            )?,
            risk,
            effects,
            summary,
            preview,
            allowed_channels: channels,
            expires_at,
            scope,
        });
    // The stored identifier is applied over the generated one, so a loaded record names the row it came
    // from rather than a fresh identity.
    approval.id = id;
    // The stored state and version are then applied **through the transition table**, walking the
    // recorded state rather than assigning it. A stored state that the table cannot reach from
    // `PENDING` is corruption — a row whose state could not have been produced by the domain's own
    // machine — and detecting that here is the reader-side counterpart of the writer's `apply`.
    let walked = reach(
        &mut approval,
        state,
        &StoredDecision {
            decided_by,
            decided_via,
            decided_assurance,
            decided_at,
            updated_at,
        },
    )?;
    if walked != version {
        return Err(RepositoryError::Corrupted { column: "version" });
    }
    Ok(approval)
}

/// Reads one stored transition row.
///
/// **The actor is deserialized through the domain type**, so a stored transition is subject to the same
/// rules as a live one — including the `DecisionNote` bound, whose `Deserialize` goes through its own
/// constructor. That is what makes the decision's comment safe to read back: a note that could not have
/// been written by this code is `Corrupted` rather than a value handed to an operator.
fn stored_transition(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ApprovalTransitionRecord, RepositoryError> {
    let id =
        ApprovalId::parse(&text(row, "approval_id")?).map_err(|_| RepositoryError::Corrupted {
            column: "approval_id",
        })?;
    let actor: ApprovalActor = serde_json::from_str(&text(row, "actor_json")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "actor_json",
        }
    })?;
    Ok(ApprovalTransitionRecord {
        id,
        from: parse_approval_state(&text(row, "from_state")?)?,
        to: parse_approval_state(&text(row, "to_state")?)?,
        prior_version: ApprovalVersion::new(int(row, "prior_version")?.try_into().map_err(
            |_| RepositoryError::Corrupted {
                column: "prior_version",
            },
        )?),
        version: ApprovalVersion::new(
            int(row, "version")?
                .try_into()
                .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
        ),
        actor,
        occurred_at: parse_time(&text(row, "occurred_at")?, "occurred_at")?,
    })
}

/// Reads the assurance a decision recorded, if the row records one.
///
/// **An unrecognised stored spelling is corruption rather than a default**, and the distinction matters
/// here more than for most columns: the writer stores only the two contract values, so a third means the
/// column was written by something else, and defaulting to `Standard` would report the weakest plausible
/// strength for a value nobody established. `NULL` is legitimate and means **not recorded** — the state of
/// every row written before `000010` — and is deliberately distinct from the weakest level, so a reader
/// can tell "we do not know" from "we know it was ordinary".
///
/// Extracted from `stored_approval` when that function reached clippy's line bound, which was a real
/// signal: the reader had grown long enough that a column could be parsed into the wrong variable without
/// a reader noticing.
fn stored_assurance(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<Option<RequiredAssurance>, RepositoryError> {
    match opt_text(row, "decided_assurance")? {
        Some(value) => assurance_from_stored(&value)
            .map(Some)
            .ok_or(RepositoryError::Corrupted {
                column: "decided_assurance",
            }),
        None => Ok(None),
    }
}

/// The decision columns as the row stores them.
///
/// Gathered into one value rather than passed as four arguments, for the reason `BRN-008`'s controller
/// records: several of these are the same shape, and a positional call would make a
/// decider/deadline transposition possible.
struct StoredDecision {
    /// Who decided, when the row records a decision.
    decided_by: Option<PrincipalId>,
    /// Which channel the decision came from, when the row records one.
    decided_via: Option<ApprovalChannel>,
    /// The assurance the decider proved, when the row records a decision.
    ///
    /// `None` means **not recorded**, which is what every row written before `000010` says and what a
    /// non-decision transition writes. It is deliberately not defaulted to `Standard` at the read, for
    /// the reason the reconstruction refuses it: a default would assert a credential strength the record
    /// never established.
    decided_assurance: Option<RequiredAssurance>,
    /// When the decision was taken, when the row records one.
    decided_at: Option<UtcTimestamp>,
    /// The instant of the row's most recent transition, which is all a non-decision step has.
    updated_at: UtcTimestamp,
}

impl StoredDecision {
    /// Returns the instant the step to `target` was recorded at.
    ///
    /// **A decision state without a recorded instant is corruption**, which is the column-level form of
    /// the rule the domain enforces when it records a decider: `apply` always writes `decided_at` for a
    /// decision, so a row that claims one without it describes a decision nobody took. The non-decision
    /// states carry no such instant of their own — expiry and invalidation are caused by time and by a
    /// changed action, not by a person — so they use the row's last write time, which is the instant
    /// their own transition was recorded. The value is only ever used as the `occurred_at` a
    /// reconstruction passes to `apply`; the earlier version of this function used the approval's
    /// `expires_at` for every step, which silently reconstructed a decision as having been taken at its
    /// own deadline.
    fn instant_for(&self, target: ApprovalState) -> Result<UtcTimestamp, RepositoryError> {
        match target {
            ApprovalState::Approved | ApprovalState::Rejected => {
                self.decided_at.ok_or(RepositoryError::Corrupted {
                    column: "decided_at",
                })
            }
            _ => Ok(self.updated_at),
        }
    }
}

/// Drives a freshly requested approval to `state`, returning the version it reached.
///
/// **The reader walks the same state machine the writer walked**, rather than assigning a stored state.
/// That is what makes an unreachable stored state detectable: `PENDING -> CONSUMED` is not an edge, so a
/// row claiming it is corruption rather than a decision that skipped a step. Assigning the fields
/// directly would accept it, and the approval would then behave as a consumed one that was never
/// approved — the worst possible reconstruction of a decision.
///
/// The path is chosen from the target's own shape: every terminal decision state is reachable from
/// `PENDING` except the two that describe a *granted* approval ending, which need `APPROVED` first.
fn reach(
    approval: &mut DurableApproval,
    state: ApprovalState,
    stored: &StoredDecision,
) -> Result<ApprovalVersion, RepositoryError> {
    // Copied out **before** any `apply`, because the actor reconstruction needs the approval's own
    // fields while `apply` needs it mutably — the two borrows cannot overlap. Reading them here is also
    // what keeps the loop from holding a borrow across a mutation.
    let tool_call = approval.tool_call;
    // `ActionDigest` is `Copy`, so this is a copy rather than a clone — the type is a fixed 32-byte value
    // and needs no heap, which is why it can be passed into the actor reconstruction by value.
    let action_digest = approval.action_digest;
    let requesting_principal = approval.requesting_principal;
    // The approval's own deadline, copied out for the same reason: the expiry actor records *which*
    // deadline passed, and reading it from the value inside the loop would borrow it immutably while
    // `apply` needs it mutably.
    let expires_at = approval.expires_at;

    let path: &[ApprovalState] = match state {
        ApprovalState::Pending => &[],
        // Every decision and lifecycle state is reachable in one step except the two that describe a
        // **granted** approval ending: a consumed or invalidated approval must have been approved first,
        // which is what makes an unreachable stored state detectable rather than assigned.
        ApprovalState::Approved
        | ApprovalState::Rejected
        | ApprovalState::Expired
        | ApprovalState::Cancelled => &[state],
        ApprovalState::Consumed | ApprovalState::Invalidated => &[ApprovalState::Approved, state],
    };
    for step in path {
        let version = approval.version();
        // **The instant comes from the row, per step, and the first version used the approval's own
        // `expires_at`.** That value is the deadline, not the moment of a decision, so a stored
        // approved-at-12:00 approval was reconstructed as decided at its 12:10 deadline — a record whose
        // `decided_at` was wrong by ten minutes, which no round-trip assertion on the state could see.
        let occurred_at = stored.instant_for(*step)?;
        let body = reconstructed_actor(&Reconstruction {
            target: *step,
            decided_by: stored.decided_by,
            decided_via: stored.decided_via,
            decided_assurance: stored.decided_assurance,
            expires_at,
            tool_call,
            action_digest,
            requesting_principal,
        })?;
        approval
            .apply(*step, version, body, occurred_at)
            .map_err(|_| RepositoryError::Corrupted { column: "state" })?;
    }
    Ok(approval.version())
}

/// The facts a reconstructed actor is built from.
///
/// A struct rather than seven parameters, because several of these are identifiers of the same shape and
/// a positional call would make a principal/tool-call transposition possible — the same reasoning
/// `BRN-008`'s controller records for grouping its helpers' arguments.
struct Reconstruction {
    target: ApprovalState,
    decided_by: Option<PrincipalId>,
    decided_via: Option<ApprovalChannel>,
    decided_assurance: Option<RequiredAssurance>,
    expires_at: UtcTimestamp,
    tool_call: ToolCallId,
    action_digest: ActionDigest,
    requesting_principal: PrincipalId,
}

/// Builds the actor a stored transition must have had.
///
/// **A function rather than a closure, and that is a correction.** The first version was a closure that
/// captured the approval's `action_digest` — which was a `String` then, so not `Copy` — which made it
/// `FnOnce` and meant it could be called exactly once. The loop calls it once per step, so a two-step path
/// (`APPROVED -> CONSUMED`) failed to compile. The digest is now an [`ActionDigest`], which **is** `Copy`,
/// so the value can be passed by copy and the function is reusable without either a clone or a borrow —
/// the ownership problem the compile error surfaced is gone at the source rather than worked around.
///
/// A *decision* with no recorded principal or channel is corruption rather than a default: the domain
/// records who decided and on which channel, and guessing a channel would defeat the check that
/// recorded it.
fn reconstructed_actor(parts: &Reconstruction) -> Result<ApprovalActor, RepositoryError> {
    match parts.target {
        ApprovalState::Approved | ApprovalState::Rejected => {
            match (parts.decided_by, parts.decided_via) {
                (Some(principal), Some(channel)) => Ok(ApprovalActor::Decided {
                    principal,
                    channel,
                    // **A decision with no recorded assurance is corruption, for the same reason a
                    // decision with no principal is.** The writer always records the assurance the
                    // decider proved, so a decision row without one was not written by this code — and
                    // defaulting to `Standard` would assert that an unknown caller held only an
                    // ordinary credential, which is exactly the claim the column exists to establish.
                    // A row written before `000010` is therefore not silently readable as a decision,
                    // which is the honest direction.
                    assurance: parts.decided_assurance.ok_or(RepositoryError::Corrupted {
                        column: "decided_assurance",
                    })?,
                    // The note is reconstructed from the **stored actor JSON**, which is where it lives:
                    // it explains one transition rather than being a durable property of the approval, so
                    // it is read back by the walk's actor rather than by a column of its own. `None` is
                    // a decision whose caller said nothing, which is a legitimate record.
                    note: None,
                }),
                _ => Err(RepositoryError::Corrupted {
                    column: "decided_by",
                }),
            }
        }
        ApprovalState::Expired => Ok(ApprovalActor::Expired {
            expires_at: parts.expires_at,
        }),
        ApprovalState::Consumed => Ok(ApprovalActor::Consumed {
            tool_call: parts.tool_call,
        }),
        ApprovalState::Invalidated => Ok(ApprovalActor::Invalidated {
            was_for: parts.action_digest.to_string(),
        }),
        ApprovalState::Cancelled => Ok(ApprovalActor::Cancelled {
            by: parts.requesting_principal,
            // Reconstructed from the stored actor JSON, where the caller's reason lives.
            reason: None,
        }),
        // `PENDING` is not reachable by any path in `reach`, so reaching this arm means the path table
        // above and this match disagree — which is corruption rather than an unreachable branch to
        // silently accept.
        ApprovalState::Pending => Err(RepositoryError::Corrupted { column: "state" }),
    }
}

/// Serializes a tool identity to its canonical stored form.
fn serialize_identity(identity: &ToolIdentity) -> Result<String, RepositoryError> {
    serde_json::to_string(identity).map_err(|_| RepositoryError::Corrupted {
        column: "tool_identity_json",
    })
}

/// Serializes the declared effects.
fn serialize_effects(effects: &[Effect]) -> Result<String, RepositoryError> {
    serde_json::to_string(effects).map_err(|_| RepositoryError::Corrupted {
        column: "effects_json",
    })
}

/// Serializes the preview.
fn serialize_preview(preview: &ApprovalPreview) -> Result<String, RepositoryError> {
    serde_json::to_string(preview.items()).map_err(|_| RepositoryError::Corrupted {
        column: "preview_json",
    })
}

/// Serializes the permitted channels.
fn serialize_channels(channels: &AllowedChannels) -> Result<String, RepositoryError> {
    serde_json::to_string(channels.as_slice()).map_err(|_| RepositoryError::Corrupted {
        column: "allowed_channels_json",
    })
}

/// Serializes an actor into its kind and its payload.
///
/// Two columns rather than one, so a query can count decisions by kind without parsing — and so the
/// kind is readable even if a future payload shape changes. The pair is returned together because
/// computing one without the other would be a row whose kind disagrees with its payload.
fn serialize_actor(actor: &ApprovalActor) -> Result<(&'static str, String), RepositoryError> {
    let kind = match actor {
        ApprovalActor::Decided { .. } => "decided",
        ApprovalActor::Expired { .. } => "expired",
        ApprovalActor::Invalidated { .. } => "invalidated",
        ApprovalActor::Consumed { .. } => "consumed",
        ApprovalActor::Cancelled { .. } => "cancelled",
    };
    let payload = serde_json::to_string(actor).map_err(|_| RepositoryError::Corrupted {
        column: "actor_json",
    })?;
    Ok((kind, payload))
}

/// Parses a stored approval state.
fn parse_approval_state(value: &str) -> Result<ApprovalState, RepositoryError> {
    ApprovalState::parse(value).map_err(|_| RepositoryError::Corrupted { column: "state" })
}

/// Reads the tool identity from its JSON column.
///
/// Extracted so `stored_approval` stays within clippy's line bound after the summary and preview gained
/// their constructors, and so the column name is bound to the type here rather than spelled inline.
fn stored_tool_identity(row: &sqlx::sqlite::SqliteRow) -> Result<ToolIdentity, RepositoryError> {
    serde_json::from_str(&text(row, "tool_identity_json")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "tool_identity_json",
        }
    })
}

/// Parses a stored scope kind.
fn parse_scope(value: &str) -> Result<ApprovalScopeKind, RepositoryError> {
    match value {
        "one_shot" => Ok(ApprovalScopeKind::OneShot),
        "standing" => Ok(ApprovalScopeKind::Standing),
        _ => Err(RepositoryError::Corrupted { column: "scope" }),
    }
}

/// Parses a stored channel.
fn parse_channel(value: &str) -> Result<ApprovalChannel, RepositoryError> {
    ApprovalChannel::parse(value).map_err(|_| RepositoryError::Corrupted {
        column: "decided_via",
    })
}

/// Reads and parses a JSON column.
fn parse_json<T: serde::de::DeserializeOwned>(
    value: &str,
    column: &'static str,
) -> Result<T, RepositoryError> {
    serde_json::from_str(value).map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads a stored version, refusing one below the first.
fn stored_version(row: &sqlx::sqlite::SqliteRow) -> Result<ApprovalVersion, RepositoryError> {
    let value = int(row, "version")?;
    match u64::try_from(value) {
        Ok(value) if value >= 1 => Ok(ApprovalVersion::new(value)),
        // A version below the first would make the optimistic check meaningless — a zero matches an
        // uninitialised field — and `ApprovalVersion` has no constructor that produces one, so this is
        // the reader refusing a row the writer could not have produced.
        _ => Err(RepositoryError::Corrupted { column: "version" }),
    }
}

/// A tiny extension so the `decided_at` bind reads as one expression.
///
/// `decided_at` is written only for a transition that carried a decider, so a non-decision transition
/// leaves it `NULL` rather than inventing a time. The time passed is the transition's own instant,
/// which for a decision is the moment it was taken.
trait DecidedAt {
    /// Returns this instant as the `decided_at` bind, or `None` when the approval is undecided.
    fn pipe_decided_at(self, decided: bool) -> Option<String>;
}

impl DecidedAt for String {
    fn pipe_decided_at(self, decided: bool) -> Option<String> {
        decided.then_some(self)
    }
}
