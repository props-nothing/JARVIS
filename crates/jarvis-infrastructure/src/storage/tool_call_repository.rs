//! The SQLite implementation of the tool-call ledger port.
//!
//! This is the join between `000009_tool_call_ledger.sql` and
//! [`ToolCallRepository`], and it exists to make one sentence true. `TLS-006` built the ledger's state
//! machine, its transition table, and its four-variant reservation verdict, and then recorded that
//!
//! > the reservation is atomic only within one process's memory.
//!
//! That is the gap. Two calls can arrive at **two** daemons — a retried CLI invocation while the first
//! is still running, a client that reconnects and re-sends, a second process started against the same
//! profile — and an in-memory map cannot see the other process's row. The store can.
//!
//! ## The reservation is one statement, and its outcome is the verdict
//!
//! [`SqliteToolCallRepository::reserve`] does **not** read, decide, and then write. It performs a single
//! `INSERT` against the unique index
//! `(workspace_id, principal_id, tool_identity_json, idempotency_key)` and classifies the result:
//!
//! - the insert succeeded → [`ReservationOutcome::Granted`];
//! - the insert violated the unique index → **read the row that won** and classify *it*, in a second
//!   query with no lock held between the two.
//!
//! The second step is a read of a row that already exists, so it cannot race the way a
//! read-then-decide-then-insert can: by the time the constraint fired, the winner's row was durable for
//! the rest of the transaction. That distinction is the entire reason this adapter is worth writing, and
//! it is asserted directly — see `two_connections_racing_the_same_key_produce_one_grant` in the tests,
//! which reserves from two separate connections on one file database.
//!
//! ## Three reading rules, inherited rather than reinvented
//!
//! - A state, operation, outcome, or identity the domain does not know is
//!   [`RepositoryError::Corrupted`], never "absent": skipping an uninterpretable row converts a
//!   migration problem into apparent data loss.
//! - A row owned by another workspace is [`RepositoryError::NotFound`] **indistinguishable from one that
//!   does not exist**, the rule the local control API states for runs.
//! - **A terminal row with no recorded outcome is corruption.** `LedgerEntry::apply` records an outcome
//!   for every terminal transition, so a terminal row without one is a row that claims to have finished
//!   without saying what happened — and defaulting it would let a caller read an unknown outcome as a
//!   recorded failure and retry it.
//!
//! ## Why `no_effect_confirmed` is read strictly
//!
//! The column is an integer rather than a nullable boolean so there is no third state to interpret, and
//! the reader accepts exactly `0` and `1`. A stored `2` is a value the writer could not have produced,
//! which is a migration fault; treating any non-zero value as `true` would accept it as a claim about
//! effects. This is the same "refuse a stored value the writer cannot produce" rule the approval adapter
//! applies to a version of zero.

use sqlx::SqlitePool;

use jarvis_application::repository::tool_call::{EffectingScan, ToolCallRepository};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{PrincipalId, RunId, ToolCallId, ToolCallRecordId, WorkspaceId};
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::ToolIdentity;
use jarvis_domain::tool::ledger::{
    LedgerEntry, LedgerEntryParts, LedgerOperation, ReservationKey, ReservationOutcome,
    ToolCallState, ToolCallTransition, ToolCallVersion,
};

use super::repositories::{begin_write, int, opt_text, parse_time, text};

/// Builds a `SELECT` over the ledger columns with `predicate` appended.
///
/// A macro rather than a `format!` at each call site, so the concatenation happens at compile time and
/// the result is still a `&'static str` — the two properties `sqlx` requires and that a runtime-built
/// string cannot satisfy.
///
/// **The column list lives here and nowhere else.** A named constant listing the same columns was the
/// first attempt and was deleted: with it present there were two copies of the list, one of which the
/// macro did not use, so an added column would have updated the wrong one and the select would have kept
/// working with a silently narrower row. The "exists but enforced nowhere" shape, in the one place where
/// the failure is a *missing field* rather than an error.
macro_rules! select_sql {
    ($predicate:literal) => {
        concat!(
            "SELECT id, call_id, workspace_id, principal_id, run_id, tool_identity_json, \
             idempotency_key, attempt, operation, state, version, dispatched_at, outcome, \
             no_effect_confirmed, created_at, updated_at FROM tool_call_records WHERE ",
            $predicate
        )
    };
}

/// The `SELECT` for [`ToolCallRepository::load`].
const LOAD_SQL: &str = select_sql!("workspace_id = ? AND id = ?");

/// The `SELECT` for the duplicate read inside a reservation, by the unique key.
const BY_KEY_SQL: &str = select_sql!(
    "workspace_id = ? AND principal_id = ? AND tool_identity_json = ? AND idempotency_key = ? \
     ORDER BY attempt ASC LIMIT 1"
);

/// The `SELECT` for the reconciliation scan.
const EFFECTING_SQL: &str = select_sql!(
    "state IN ('executing', 'reconciling') AND outcome IS NULL ORDER BY updated_at ASC LIMIT ?"
);

/// The `SELECT` for the **conversion input**.
///
/// `executing` alone, and that narrowing is load-bearing rather than an optimisation: converting a row
/// moves it to `reconciling`, which the effecting scan above returns again. A pass that paged *that* scan
/// would re-read its own output, and since the ordering ties on `updated_at` a later page can hold only
/// converted rows — leaving unconverted ones beyond the page unreachable for ever. This predicate makes
/// the set strictly shrink as the pass converts, so the paging terminates.
const AWAITING_CONVERSION_SQL: &str =
    select_sql!("state = 'executing' AND outcome IS NULL ORDER BY updated_at ASC LIMIT ?");

/// The SQLite-backed tool-call ledger.
#[derive(Debug, Clone)]
pub struct SqliteToolCallRepository {
    pool: SqlitePool,
}

impl SqliteToolCallRepository {
    /// Builds the repository over an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

impl ToolCallRepository for SqliteToolCallRepository {
    fn reserve(&self, entry: &LedgerEntry) -> RepositoryFuture<'_, ReservationOutcome> {
        // Owned before the future is built, because the returned future is bounded by `&self` rather
        // than by the argument. The clone is of one bounded row and happens once per reservation.
        let entry = entry.clone();
        Box::pin(async move {
            // **One statement decides, and its failure is the answer.** No read precedes this, so there
            // is no window in which two processes could both observe an absent row and both insert —
            // the unique index is what makes "the first writer wins" a property of the store rather than
            // of the timing.
            let inserted = sqlx::query(
                "INSERT INTO tool_call_records (id, call_id, workspace_id, principal_id, run_id, \
                 tool_identity_json, idempotency_key, attempt, operation, state, version, \
                 dispatched_at, outcome, no_effect_confirmed, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(entry.id.to_string())
            .bind(entry.call_id.to_string())
            .bind(entry.key.workspace.to_string())
            .bind(entry.key.principal.to_string())
            .bind(entry.run.to_string())
            .bind(serialize_identity(&entry.key.identity)?)
            .bind(&entry.key.idempotency_key)
            .bind(i64::from(entry.attempt()))
            .bind(entry.operation.as_contract_str())
            .bind(entry.state().as_contract_str())
            .bind(version_to_i64(entry.version())?)
            .bind(entry.dispatched_at().map(|at| at.to_string()))
            .bind(entry.outcome().map(ToolErrorClass::as_contract_str))
            .bind(i64::from(entry.no_effect_confirmed()))
            .bind(entry.created_at.to_string())
            .bind(entry.updated_at().to_string())
            .execute(&self.pool)
            .await;

            match inserted {
                Ok(_) => Ok(ReservationOutcome::Granted),
                Err(error) if is_unique_violation(&error) => {
                    // The row that won is read *after* the constraint fired, so it is durable for the
                    // rest of this transaction. Classifying it here rather than deriving the verdict from
                    // the insert means the caller is answered from the state the store actually holds,
                    // which is what keeps this adapter in step with `ToolCallLedger::reserve`.
                    let row = self.read_by_key(&entry.key).await?;
                    let Some(row) = row else {
                        // The constraint fired but the row is not visible: the winner's transaction
                        // rolled back between the two statements. Reporting "granted" would let this
                        // caller dispatch while the other may still do so; reporting a conflict would
                        // send the caller somewhere it cannot look. This is a store-level anomaly rather
                        // than a reservation verdict, so it is an error.
                        return Err(RepositoryError::Conflict {
                            what: "reservation_vanished",
                        });
                    };
                    classify(&row)
                }
                // Any other driver failure is a query failure, not a duplicate: the distinction matters
                // because a duplicate is a verdict and this is a fault.
                Err(_) => Err(RepositoryError::Query),
            }
        })
    }

    fn load(
        &self,
        workspace: WorkspaceId,
        record: ToolCallRecordId,
    ) -> RepositoryFuture<'_, LedgerEntry> {
        Box::pin(async move {
            let row = sqlx::query(LOAD_SQL)
                .bind(workspace.to_string())
                .bind(record.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };
            stored_entry(&row)
        })
    }

    fn apply_transition(
        &self,
        workspace: WorkspaceId,
        transition: &ToolCallTransition,
        expected: ToolCallVersion,
        entry: &LedgerEntry,
    ) -> RepositoryFuture<'_, ()> {
        let entry = entry.clone();
        let transition = transition.clone();
        Box::pin(async move {
            let mut tx = begin_write(&self.pool).await?;

            // The stored row is read first so the refusals are ordered the way the domain orders them:
            // version, then state. Classifying from a zero-row `UPDATE` cannot do that, because "no rows
            // matched" does not say which predicate failed — the same reasoning the run and approval
            // repository transitions record.
            let row = sqlx::query(
                "SELECT state, version FROM tool_call_records WHERE workspace_id = ? AND id = ?",
            )
            .bind(workspace.to_string())
            .bind(entry.id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };
            let stored_state = parse_state(&text(&row, "state")?)?;
            let stored_version = stored_version(&row)?;
            if stored_version != expected {
                return Err(RepositoryError::VersionConflict {
                    expected: expected.get(),
                    actual: stored_version.get(),
                });
            }
            // **The edge is judged from the stored state, by the domain's own table.** A caller hands in
            // a transition record it built from its own value, so a record describing an edge the store
            // is not on would otherwise be written as if it had happened. `TransitionRefused` carries the
            // domain's code rather than a fresh string, so a caller sees one error namespace.
            if !stored_state.can_transition_to(transition.to) {
                return Err(RepositoryError::TransitionRefused {
                    code: "tool.state_conflict",
                });
            }

            let updated = sqlx::query(
                "UPDATE tool_call_records SET state = ?, version = ?, dispatched_at = ?, \
                 outcome = ?, no_effect_confirmed = ?, updated_at = ? \
                 WHERE workspace_id = ? AND id = ? AND version = ?",
            )
            .bind(entry.state().as_contract_str())
            .bind(version_to_i64(entry.version())?)
            // The dispatch fact is written from the caller's value, which the domain stamped on the
            // first entry into `EXECUTING`. It is never cleared, so a row that was dispatched keeps
            // saying so whatever state it reaches afterwards — the defect `TLS-006` fixed in memory,
            // preserved here by writing the column rather than deriving it.
            .bind(entry.dispatched_at().map(|at| at.to_string()))
            .bind(entry.outcome().map(ToolErrorClass::as_contract_str))
            .bind(i64::from(entry.no_effect_confirmed()))
            .bind(transition.occurred_at.to_string())
            .bind(workspace.to_string())
            .bind(entry.id.to_string())
            .bind(version_to_i64(expected)?)
            .execute(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;
            if updated.rows_affected() == 0 {
                return Err(RepositoryError::VersionConflict {
                    expected: expected.get(),
                    actual: stored_version.get(),
                });
            }

            // The trail row is written in the **same transaction**, so a state change and the record of
            // it cannot diverge. For a ledger row the trail is load-bearing rather than decorative: it is
            // the durable answer to "was this call ever dispatched".
            sqlx::query(
                "INSERT INTO tool_call_transitions (id, record_id, workspace_id, from_state, \
                 to_state, prior_version, version, outcome, occurred_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(entry.id.to_string())
            .bind(workspace.to_string())
            .bind(transition.from.as_contract_str())
            .bind(transition.to.as_contract_str())
            .bind(version_to_i64(transition.prior_version)?)
            .bind(version_to_i64(transition.version)?)
            .bind(transition.outcome.map(ToolErrorClass::as_contract_str))
            .bind(transition.occurred_at.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;

            tx.commit().await.map_err(|_| RepositoryError::Query)?;
            Ok(())
        })
    }

    fn awaiting_conversion(&self, limit: u32) -> RepositoryFuture<'_, EffectingScan> {
        scan(&self.pool, AWAITING_CONVERSION_SQL, limit)
    }

    fn possibly_effecting(&self, limit: u32) -> RepositoryFuture<'_, EffectingScan> {
        scan(&self.pool, EFFECTING_SQL, limit)
    }
}

impl SqliteToolCallRepository {
    /// Reads the row a reservation's key names, if one exists.
    async fn read_by_key(
        &self,
        key: &ReservationKey,
    ) -> Result<Option<sqlx::sqlite::SqliteRow>, RepositoryError> {
        sqlx::query(BY_KEY_SQL)
            .bind(key.workspace.to_string())
            .bind(key.principal.to_string())
            .bind(serialize_identity(&key.identity)?)
            .bind(&key.idempotency_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)
    }
}

/// Runs one of the two scans and reports whether the page was full.
///
/// **One row more than the caller asked for**, so a full page can be told from a complete one: reading
/// exactly `limit` rows cannot distinguish "there are more" from "that was all", and for these scans the
/// difference is durable work left unsettled rather than a slightly shorter list — the same technique
/// `RunRepository::incomplete_runs` uses. The statement comes from the caller because the two scans differ
/// only in their predicate, and a second copy of the body would be a second place the probe could be
/// forgotten.
fn scan<'a>(
    pool: &'a SqlitePool,
    statement: &'static str,
    limit: u32,
) -> RepositoryFuture<'a, EffectingScan> {
    let pool = pool.clone();
    Box::pin(async move {
        let rows = sqlx::query(statement)
            .bind(i64::from(limit.saturating_add(1)))
            .fetch_all(&pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
        let bounded = rows.len() > limit as usize;
        let records = rows
            .iter()
            .take(limit as usize)
            .map(stored_entry)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EffectingScan { records, bounded })
    })
}

/// Returns whether an error is the unique-index violation, by SQLite's own code.
///
/// The code rather than the message, because a message is prose that a driver version may reword while
/// the code is the contract. `2067` is `SQLITE_CONSTRAINT_UNIQUE` and `1555` is
/// `SQLITE_CONSTRAINT_PRIMARYKEY`; both mean "this reservation already exists", and the two are
/// distinguished only by which index fired — which the caller does not need, since both are answered by
/// reading the winner.
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database) if matches!(database.code().as_deref(), Some("2067" | "1555"))
    )
}

/// Classifies the existing row a reservation collided with.
///
/// **The domain's four verdicts, and the in-memory ledger is the specification.** `ToolCallLedger::reserve`
/// answers the same way from a map, and the branch order is the same: an `executing` or `reconciling` row
/// means the effect may already exist, so a reconciling row is `Unsettled` (its outcome is unknown and
/// must be established) and an executing one is `InFlight` (somebody is running it). Getting those two
/// the wrong way round would either retry a call whose effect may have landed or leave a caller waiting
/// on a row nobody is running — which is why the tests assert each state's verdict individually rather
/// than asserting that "a duplicate is not granted".
fn classify(row: &sqlx::sqlite::SqliteRow) -> Result<ReservationOutcome, RepositoryError> {
    let state = parse_state(&text(row, "state")?)?;
    match state {
        ToolCallState::Reconciling => Ok(ReservationOutcome::Unsettled {
            row: parse_record_id(&text(row, "id")?)?,
        }),
        state if state.is_terminal() => Ok(ReservationOutcome::AlreadyTerminal {
            state,
            outcome: parse_optional_outcome(row)?,
            no_effect_confirmed: parse_no_effect(row)?,
        }),
        state => Ok(ReservationOutcome::InFlight { state }),
    }
}

/// Rebuilds a [`LedgerEntry`] from a row.
///
/// The identity goes through the **domain type's own deserializer**, so a stored document is subject to
/// the same rules as one that arrived over the wire. The state, the operation, and the outcome go
/// through the domain's own parsers for the same reason, and a version below the first is refused by the
/// domain's reader-side constructor rather than by a local comparison.
fn stored_entry(row: &sqlx::sqlite::SqliteRow) -> Result<LedgerEntry, RepositoryError> {
    let id = parse_record_id(&text(row, "id")?)?;
    let call_id = ToolCallId::parse(&text(row, "call_id")?)
        .map_err(|_| RepositoryError::Corrupted { column: "call_id" })?;
    let workspace = WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "workspace_id",
        }
    })?;
    let principal = PrincipalId::parse(&text(row, "principal_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "principal_id",
        }
    })?;
    let run = RunId::parse(&text(row, "run_id")?)
        .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?;
    let identity: ToolIdentity =
        serde_json::from_str(&text(row, "tool_identity_json")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "tool_identity_json",
            }
        })?;
    let key = ReservationKey::new(
        identity,
        workspace,
        principal,
        &text(row, "idempotency_key")?,
    )
    .map_err(|_| RepositoryError::Corrupted {
        column: "idempotency_key",
    })?;

    // The attempt is refused at zero by the domain's own rule, restated here because the column can
    // hold one and `reserve` normalises it — so a stored zero is a row the writer could not have
    // produced, which is corruption rather than a first attempt.
    let attempt = u32::try_from(int(row, "attempt")?)
        .map_err(|_| RepositoryError::Corrupted { column: "attempt" })?;
    let operation = LedgerOperation::parse(&text(row, "operation")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "operation",
        }
    })?;
    let state = parse_state(&text(row, "state")?)?;
    let version = stored_version(row)?;
    let dispatched_at = match opt_text(row, "dispatched_at")? {
        Some(value) => Some(parse_time(&value, "dispatched_at")?),
        None => None,
    };
    let outcome = parse_optional_outcome(row)?;
    let no_effect_confirmed = parse_no_effect(row)?;

    // **The invariants live in the domain, and the adapter maps each refusal to the column it concerns.**
    // The four checks a stored row must satisfy — a non-zero attempt, a version at least the first, a
    // terminal state with an outcome, and a state that requires a dispatch recording one — are the
    // domain's own rules, so they are stated once in `LedgerEntry::restore` rather than re-implemented
    // here. This function's job is only to say *which column* the refusal was about, which the domain
    // cannot know, and to name it in the error a support bundle will show.
    LedgerEntry::restore(LedgerEntryParts {
        id,
        call_id,
        attempt,
        run,
        key,
        operation,
        state,
        version,
        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
        updated_at: parse_time(&text(row, "updated_at")?, "updated_at")?,
        dispatched_at,
        outcome,
        no_effect_confirmed,
    })
    .map_err(|error| RepositoryError::Corrupted {
        // The domain names the field it refused; a refusal that names none is attributed to `state`,
        // which is the column all the remaining rules concern.
        column: match error.code() {
            "tool.definition_invalid" => "attempt_or_dispatch",
            _ => "outcome",
        },
    })
}

/// Parses a stored state.
fn parse_state(value: &str) -> Result<ToolCallState, RepositoryError> {
    ToolCallState::parse(value).map_err(|_| RepositoryError::Corrupted { column: "state" })
}

/// Parses a stored row identifier.
fn parse_record_id(value: &str) -> Result<ToolCallRecordId, RepositoryError> {
    ToolCallRecordId::parse(value).map_err(|_| RepositoryError::Corrupted { column: "id" })
}

/// Reads a stored outcome, if the column holds one.
fn parse_optional_outcome(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<Option<ToolErrorClass>, RepositoryError> {
    match opt_text(row, "outcome")? {
        Some(value) => {
            Ok(Some(ToolErrorClass::parse(&value).map_err(|_| {
                RepositoryError::Corrupted { column: "outcome" }
            })?))
        }
        None => Ok(None),
    }
}

/// Reads the no-effect claim, accepting exactly `0` and `1`.
///
/// Strictly, because the column is an integer rather than a nullable boolean precisely so there is no
/// third state to interpret. Treating any non-zero value as `true` would accept a stored `2` — which the
/// writer cannot produce — as a claim that an effect definitely did not happen, which is the one claim
/// that must never be made loosely.
fn parse_no_effect(row: &sqlx::sqlite::SqliteRow) -> Result<bool, RepositoryError> {
    match int(row, "no_effect_confirmed")? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(RepositoryError::Corrupted {
            column: "no_effect_confirmed",
        }),
    }
}

/// Reads a stored version, refusing one below the first.
fn stored_version(row: &sqlx::sqlite::SqliteRow) -> Result<ToolCallVersion, RepositoryError> {
    let value = u64::try_from(int(row, "version")?)
        .map_err(|_| RepositoryError::Corrupted { column: "version" })?;
    ToolCallVersion::from_stored(value)
        .map_err(|_| RepositoryError::Corrupted { column: "version" })
}

/// Converts a version to its stored integer.
fn version_to_i64(version: ToolCallVersion) -> Result<i64, RepositoryError> {
    i64::try_from(version.get()).map_err(|_| RepositoryError::Corrupted { column: "version" })
}

/// Serializes a tool identity to its canonical stored form.
///
/// **The determinism of this call is what makes the unique index sound.** The index deduplicates on the
/// serialized text, so two equal identities that produced different documents would be stored as two
/// rows and the reservation would silently stop deduplicating — while the index still existed, which is
/// the "exists but enforced nowhere" shape. `ToolIdentity` is a struct of fields serde writes in
/// declaration order, so equal values serialize identically; the tests assert that rather than assuming
/// it, because the assumption is load-bearing for a uniqueness guarantee.
fn serialize_identity(identity: &ToolIdentity) -> Result<String, RepositoryError> {
    serde_json::to_string(identity).map_err(|_| RepositoryError::Corrupted {
        column: "tool_identity_json",
    })
}

#[cfg(test)]
#[path = "tool_call_repository_tests.rs"]
mod tests;
