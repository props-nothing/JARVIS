//! The SQLite implementation of the application's repository ports.
//!
//! This adapter lives in `storage/` alongside the connection and migration code
//! rather than in an `adapters/` or `storage_adapters/` subtree. That is a
//! deliberate placement, not an oversight: the evidence manifest treats
//! `crates/**/src/storage_adapters/**` as an **external integration path**, gated
//! on a Postgres/vendor evidence note, while SQLite is the reviewed default local
//! backend already covered by the Foundation note. Moving this file under
//! `storage_adapters/` would demand an evidence note for a dependency that has not
//! changed.
//!
//! Two properties matter more than the SQL itself:
//!
//! - **The state write and its activity event commit together.**
//!   [`RunRepositoryImpl::transition`] opens one transaction, applies the state
//!   change with an optimistic version predicate, and appends the event before
//!   committing. `docs/architecture/storage-data.md` names this the first required
//!   atomic use case, and doing it in two statements would leave a window in which
//!   an event describes a transition that is not durable.
//! - **A state the domain does not recognize is corruption, never "absent".** Every
//!   state read goes through a parse that returns
//!   [`RepositoryError::Corrupted`]; skipping an uninterpretable row would turn a
//!   migration problem into apparent data loss.

use sqlx::{Row as _, SqlitePool};

use jarvis_application::repository::conversation::{
    ConversationRepository, NewConversation, NewMessage, StoredConversation, StoredMessage,
};
use jarvis_application::repository::model_call::{
    ModelCallOutcome, ModelCallRepository, NewModelCall, StoredModelCall,
};
use jarvis_application::repository::run::{
    EventVisibility, IdempotencyClaim, MAX_EVENT_PAGE, NewActivityEvent, NewIdempotencyRecord,
    NewRun, RunEventPage, RunRepository, RunResumeState, RunWrite, StoredActivityEvent, StoredRun,
    validate_idempotency_key,
};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{
    ConversationId, MessageId, ModelCallId, PrincipalId, RunActivityEventId, RunId, WorkspaceId,
};
use jarvis_domain::model::identity::{ModelId, ModelRef, ModelRevision, ProviderId};
use jarvis_domain::model::stream::Role;
use jarvis_domain::run::state::{RunState, RunVersion};
use jarvis_domain::time::UtcTimestamp;

/// The SQLite-backed repository adapters.
///
/// One type implements all three ports because they share a pool and a profile;
/// splitting them into three structs would let a caller hold three inconsistent
/// views of one database by accident.
#[derive(Debug, Clone)]
pub struct SqliteRepositories {
    pool: SqlitePool,
}

impl SqliteRepositories {
    /// Builds repositories over an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Returns the pool, for a caller that needs a transaction of its own.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

impl jarvis_application::live_events::StreamDeltaSink for SqliteRepositories {
    fn output_text_delta(
        &self,
        workspace: WorkspaceId,
        run: RunId,
        item_id: String,
        delta: String,
        occurred_at: UtcTimestamp,
    ) -> RepositoryFuture<'_, u64> {
        Box::pin(async move {
            // The run is checked first so an absent or foreign run reports `NotFound`
            // and no orphan event is appended under it.
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE workspace_id = ? AND id = ?")
                    .bind(workspace.to_string())
                    .bind(run.to_string())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| RepositoryError::Query)?;
            if exists.is_none() {
                return Err(RepositoryError::NotFound);
            }

            // The payload is assembled here rather than by the controller, so the
            // application layer never builds JSON and cannot put prompt text or a
            // secret into a public payload. The shape is the contract's fixed delta
            // payload: an item id and the text.
            let payload = jarvis_protocol::run::output_text_delta_payload(&item_id, &delta);

            // Read the next sequence and insert in one transaction, so two concurrent
            // deltas cannot both claim the same position. The unique constraint is
            // the real guarantee; the transaction is what keeps the read honest.
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RepositoryError::Query)?;
            let maximum: Option<i64> = sqlx::query_scalar(
                "SELECT MAX(sequence) FROM run_activity_events WHERE run_id = ?",
            )
            .bind(run.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let sequence = maximum.map_or(1, |value| value.saturating_add(1));
            let sequence = u64::try_from(sequence)
                .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?;

            let inserted = sqlx::query(
                "INSERT INTO run_activity_events (\
                     id, workspace_id, run_id, sequence, event_type, payload_json, \
                     visibility, occurred_at\
                 ) VALUES (?, ?, ?, ?, ?, ?, 'public', ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(workspace.to_string())
            .bind(run.to_string())
            .bind(
                i64::try_from(sequence)
                    .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?,
            )
            .bind(jarvis_protocol::event_type::OUTPUT_TEXT_DELTA)
            .bind(payload.to_string())
            .bind(occurred_at.to_string())
            .execute(&mut *tx)
            .await;

            match inserted {
                Ok(result) if result.rows_affected() == 1 => {}
                // A duplicate sequence is the same caller-visible conflict whether the
                // constraint appears as a zero-row result or an error.
                Ok(_) | Err(_) => {
                    return Err(RepositoryError::Conflict {
                        what: "activity_sequence",
                    });
                }
            }

            tx.commit().await.map_err(|_| RepositoryError::Query)?;
            Ok(sequence)
        })
    }
}

/// Parses a stored run state.
fn parse_state(value: &str) -> Result<RunState, RepositoryError> {
    match value {
        "received" => Ok(RunState::Received),
        "context_building" => Ok(RunState::ContextBuilding),
        "planning" => Ok(RunState::Planning),
        "awaiting_model" => Ok(RunState::AwaitingModel),
        "awaiting_approval" => Ok(RunState::AwaitingApproval),
        "executing_tool" => Ok(RunState::ExecutingTool),
        "observing" => Ok(RunState::Observing),
        "waiting" => Ok(RunState::Waiting),
        "responding" => Ok(RunState::Responding),
        "completed" => Ok(RunState::Completed),
        "failed" => Ok(RunState::Failed),
        "cancelled" => Ok(RunState::Cancelled),
        // A state the domain does not know is corruption. Treating it as absent
        // would convert a migration problem into apparent data loss.
        _ => Err(RepositoryError::Corrupted { column: "state" }),
    }
}

/// Parses a stored message role.
fn parse_role(value: &str) -> Result<Role, RepositoryError> {
    match value {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "tool" => Ok(Role::Tool),
        _ => Err(RepositoryError::Corrupted { column: "role" }),
    }
}

/// Parses a stored timestamp, refusing an uninterpretable value.
fn parse_time(value: &str, column: &'static str) -> Result<UtcTimestamp, RepositoryError> {
    UtcTimestamp::parse(value).map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads a text column, mapping a driver/shape failure to corruption.
fn text(row: &sqlx::sqlite::SqliteRow, column: &'static str) -> Result<String, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads a nullable text column.
fn opt_text(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<Option<String>, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads an integer column.
fn int(row: &sqlx::sqlite::SqliteRow, column: &'static str) -> Result<i64, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column })
}

/// Builds a [`StoredRun`] from a row.
fn stored_run(row: &sqlx::sqlite::SqliteRow) -> Result<StoredRun, RepositoryError> {
    let state = parse_state(&text(row, "state")?)?;
    let version_value = int(row, "version")?;
    if version_value < 1 {
        // A version below the first would make the optimistic check meaningless.
        return Err(RepositoryError::Corrupted { column: "version" });
    }
    Ok(StoredRun {
        id: RunId::parse(&text(row, "id")?)
            .map_err(|_| RepositoryError::Corrupted { column: "id" })?,
        workspace_id: WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "workspace_id",
            }
        })?,
        conversation_id: ConversationId::parse(&text(row, "conversation_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "conversation_id",
            }
        })?,
        principal_id: PrincipalId::parse(&text(row, "principal_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "principal_id",
            }
        })?,
        state,
        version: RunVersion::new(
            u64::try_from(version_value)
                .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
        ),
        objective_ref: opt_text(row, "objective_ref")?,
        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
        started_at: opt_text(row, "started_at")?
            .map(|value| parse_time(&value, "started_at"))
            .transpose()?,
        updated_at: parse_time(&text(row, "updated_at")?, "updated_at")?,
        completed_at: opt_text(row, "completed_at")?
            .map(|value| parse_time(&value, "completed_at"))
            .transpose()?,
        error_code: opt_text(row, "error_code")?,
    })
}

/// Selects the run columns a transaction-local read needs.
const RUN_SELECT: &str = "SELECT id, workspace_id, conversation_id, principal_id, state, version, \
     objective_ref, created_at, started_at, updated_at, completed_at, \
     error_code, waiting_kind, waiting_ref \
     FROM agent_runs WHERE workspace_id = ? AND id = ?";

/// Reads one run inside `executor`.
///
/// Shared by the pre-write read and the post-write read so both observe the same
/// columns; a divergence between them would let the returned value differ from the
/// one the predicates were evaluated against.
async fn read_run<'e, E>(
    executor: E,
    workspace: WorkspaceId,
    run_id: &str,
) -> Result<Option<StoredRun>, RepositoryError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query(RUN_SELECT)
        .bind(workspace.to_string())
        .bind(run_id)
        .fetch_optional(executor)
        .await
        .map_err(|_| RepositoryError::Query)?;
    row.as_ref().map(stored_run).transpose()
}

/// Inserts a run row and its opening activity event inside one transaction.
///
/// A free function rather than a method because both plain create and the atomic
/// idempotent create need it, and having two copies of the insert would let the two
/// paths diverge on the constraints they enforce.
async fn insert_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    run: &NewRun,
    opening_event: &NewActivityEvent,
) -> Result<(), RepositoryError> {
    // The event must describe the run being created. Refusing a mismatch here rather
    // than letting the foreign key catch it keeps "the opening event belongs to this
    // run" a stated rule.
    if opening_event.run_id != run.id {
        return Err(RepositoryError::Conflict {
            what: "opening_event",
        });
    }
    // The first event is sequence 1 and no other, so a caller cannot create a run
    // whose stream starts at a position a client would read as a gap.
    if opening_event.sequence != 1 {
        return Err(RepositoryError::Conflict {
            what: "opening_event",
        });
    }

    let result = sqlx::query(
        "INSERT INTO agent_runs (\
             id, workspace_id, conversation_id, parent_run_id, principal_id, \
             objective_ref, state, version, created_at, updated_at\
         ) VALUES (?, ?, ?, NULL, ?, ?, 'received', 1, ?, ?)",
    )
    .bind(run.id.to_string())
    .bind(run.workspace_id.to_string())
    .bind(run.conversation_id.to_string())
    .bind(run.principal_id.to_string())
    .bind(run.objective_ref.as_deref())
    .bind(run.created_at.to_string())
    .bind(run.created_at.to_string())
    .execute(&mut **tx)
    .await;

    // A duplicate primary key or a missing conversation both arrive as an `Err` from
    // the constraint, and both are caller-visible conflicts rather than transport
    // faults.
    match result {
        Ok(created) if created.rows_affected() == 1 => {}
        Ok(_) | Err(_) => return Err(RepositoryError::Conflict { what: "run" }),
    }

    let inserted = sqlx::query(
        "INSERT INTO run_activity_events (\
             id, workspace_id, run_id, sequence, event_type, payload_json, \
             visibility, occurred_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(run.workspace_id.to_string())
    .bind(run.id.to_string())
    .bind(1_i64)
    .bind(&opening_event.event_type)
    .bind(opening_event.payload_json.as_deref())
    .bind(opening_event.visibility.as_str())
    .bind(opening_event.occurred_at.to_string())
    .execute(&mut **tx)
    .await;

    match inserted {
        Ok(event) if event.rows_affected() == 1 => Ok(()),
        Ok(_) | Err(_) => Err(RepositoryError::Conflict {
            what: "activity_sequence",
        }),
    }
}

/// Reads an existing idempotency record inside `executor`.
async fn read_idempotency<'e, E>(
    executor: E,
    workspace: WorkspaceId,
    operation: &str,
    key: &str,
) -> Result<Option<(String, String)>, RepositoryError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as(
        "SELECT request_digest, run_id FROM idempotency_records \
         WHERE workspace_id = ? AND operation = ? AND api_major = ? \
           AND idempotency_key = ?",
    )
    .bind(workspace.to_string())
    .bind(operation)
    .bind(i64::from(crate::http::API_MAJOR))
    .bind(key)
    .fetch_optional(executor)
    .await
    .map_err(|_| RepositoryError::Query)
}

impl RunRepository for SqliteRepositories {
    fn create(&self, run: NewRun, opening_event: NewActivityEvent) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RepositoryError::Query)?;
            insert_run(&mut tx, &run, &opening_event).await?;
            tx.commit().await.map_err(|_| RepositoryError::Query)
        })
    }

    fn load(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, StoredRun> {
        Box::pin(async move {
            // The workspace predicate is in the query, not applied afterwards, so a
            // run from another scope is `NotFound` rather than a forbidden result —
            // the API requires the two to be indistinguishable.
            read_run(&self.pool, workspace, &run.to_string())
                .await?
                .ok_or(RepositoryError::NotFound)
        })
    }

    fn load_for_resume(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> RepositoryFuture<'_, RunResumeState> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id, workspace_id, conversation_id, principal_id, state, version, \
                        objective_ref, created_at, started_at, updated_at, completed_at, \
                        error_code, waiting_kind, waiting_ref \
                 FROM agent_runs WHERE workspace_id = ? AND id = ?",
            )
            .bind(workspace.to_string())
            .bind(run.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?
            .ok_or(RepositoryError::NotFound)?;
            Ok(RunResumeState {
                run: stored_run(&row)?,
                waiting_kind: opt_text(&row, "waiting_kind")?,
                waiting_ref: opt_text(&row, "waiting_ref")?,
            })
        })
    }

    fn transition<'a>(
        &'a self,
        workspace: WorkspaceId,
        write: RunWrite<'a>,
    ) -> RepositoryFuture<'a, StoredRun> {
        Box::pin(async move {
            let transition = write.transition;

            // The write's own consistency is checked up front: a waiting target with
            // no dependency (or a non-waiting target that carries one) would violate
            // the schema's `CHECK`, and a typed conflict is more useful to a caller
            // than a decoded constraint failure from deep in the driver.
            if !write.is_consistent() {
                return Err(RepositoryError::Conflict { what: "waiting_on" });
            }

            let event = write.event;
            // The event must belong to the run the transition moves, or the
            // transaction would commit a state change and an unrelated event.
            let run_id = event.run_id.to_string();
            let waiting = write.waiting.as_ref();

            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RepositoryError::Query)?;

            // The current row is read first so the refusals can be ordered the way
            // the domain orders them: terminal, then version, then edge. Classifying
            // from a zero-row `UPDATE` alone cannot do that, because "no rows
            // matched" does not say *which* predicate failed — and an earlier version
            // of this method inferred the edge from the `state = ?` predicate, which
            // only proves the run was in the expected state, not that the edge is
            // legal. That reading accepted `Received -> Responding`, an edge the
            // architecture diagram does not contain.
            let Some(current) = read_run(&mut *tx, workspace, &run_id).await? else {
                // Absent or owned by another workspace — indistinguishable by design.
                return Err(RepositoryError::NotFound);
            };

            if current.state.is_terminal() {
                return Err(RepositoryError::TransitionRefused {
                    code: "jarvis.run_already_terminal",
                });
            }
            if current.version != transition.expected_version {
                return Err(RepositoryError::VersionConflict {
                    expected: transition.expected_version.get(),
                    actual: current.version.get(),
                });
            }
            // The edge is judged from the state the run is *actually* in. The
            // domain's transition table is the single authority for which edges
            // exist, so the adapter asks it rather than duplicating the table.
            if !current.state.can_transition_to(transition.to) {
                return Err(RepositoryError::TransitionRefused {
                    code: "jarvis.run_transition_not_allowed",
                });
            }

            // The optimistic predicate stays, so a writer that slipped in between the
            // read and this statement is refused rather than overwriting it.
            let terminal = transition.to.is_terminal();
            let update = sqlx::query(
                "UPDATE agent_runs SET \
                     state = ?, version = version + 1, updated_at = ?, \
                     completed_at = CASE WHEN ? = 1 THEN ? ELSE completed_at END, \
                     waiting_kind = ?, \
                     waiting_ref = ? \
                 WHERE workspace_id = ? AND id = ? AND version = ? AND state = ? \
                   AND state NOT IN ('completed', 'failed', 'cancelled')",
            )
            .bind(transition.to.to_string())
            .bind(transition.occurred_at.to_string())
            .bind(i64::from(terminal))
            .bind(transition.occurred_at.to_string())
            .bind(waiting.map(|w| w.kind.as_str()))
            .bind(waiting.map(|w| w.reference.as_str()))
            .bind(workspace.to_string())
            .bind(&run_id)
            .bind(
                i64::try_from(transition.expected_version.get())
                    .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
            )
            .bind(current.state.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|_| RepositoryError::Query)?;

            if update.rows_affected() == 0 {
                // Only reachable if another writer committed between the read above
                // and this statement. The version moved, so that is the honest
                // answer; the transaction is rolled back by dropping it.
                return Err(RepositoryError::VersionConflict {
                    expected: transition.expected_version.get(),
                    actual: transition.expected_version.get() + 1,
                });
            }

            let inserted = sqlx::query(
                "INSERT INTO run_activity_events (\
                     id, workspace_id, run_id, sequence, event_type, payload_json, \
                     visibility, occurred_at\
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(workspace.to_string())
            .bind(&run_id)
            .bind(
                i64::try_from(event.sequence)
                    .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?,
            )
            .bind(&event.event_type)
            .bind(event.payload_json.as_deref())
            .bind(event.visibility.as_str())
            .bind(event.occurred_at.to_string())
            .execute(&mut *tx)
            .await;

            match inserted {
                Ok(result) if result.rows_affected() == 1 => {}
                // A duplicate sequence means another writer appended at the same
                // position, and the unique constraint surfaces it as either an
                // unexpected zero-row result or an `Err`. Both are the *same*
                // caller-visible conflict, so both map to `Conflict` rather than one
                // of them becoming `Query` — a conflict is not a transport fault a
                // blind retry may fix.
                Ok(_) | Err(_) => {
                    return Err(RepositoryError::Conflict {
                        what: "activity_sequence",
                    });
                }
            }

            let Some(updated) = read_run(&mut *tx, workspace, &run_id).await? else {
                // Unreachable: the row was read and updated inside this transaction.
                // Reported as corruption rather than as a missing row, because a row
                // vanishing mid-transaction is not "not found".
                return Err(RepositoryError::Corrupted { column: "id" });
            };

            // Commit last, so a reader never sees the state without the event or
            // the event without the state.
            tx.commit().await.map_err(|_| RepositoryError::Query)?;
            Ok(updated)
        })
    }

    fn next_event_sequence(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, u64> {
        Box::pin(async move {
            // The run is checked first so an absent run reports `NotFound` rather
            // than a plausible-looking sequence 1, which a caller would then use.
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE workspace_id = ? AND id = ?")
                    .bind(workspace.to_string())
                    .bind(run.to_string())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| RepositoryError::Query)?;
            if exists.is_none() {
                return Err(RepositoryError::NotFound);
            }

            let maximum: Option<i64> = sqlx::query_scalar(
                "SELECT MAX(sequence) FROM run_activity_events WHERE run_id = ?",
            )
            .bind(run.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;

            let next = maximum.map_or(1, |value| value.saturating_add(1));
            u64::try_from(next).map_err(|_| RepositoryError::Corrupted { column: "sequence" })
        })
    }

    fn load_events(
        &self,
        workspace: WorkspaceId,
        run: RunId,
        from_sequence: u64,
        limit: u32,
    ) -> RepositoryFuture<'_, RunEventPage> {
        Box::pin(async move {
            // The run is read first, both to scope the read and to learn the terminal
            // state: a stream that connects after the run finished must deliver the
            // terminal event from retention and then close, so it has to know the run
            // is already finished rather than wait for a terminal that was published
            // before it connected.
            let Some(stored) = read_run(&self.pool, workspace, &run.to_string()).await? else {
                return Err(RepositoryError::NotFound);
            };

            // Visibility is a query predicate, not a post-filter: an operator-only
            // event must never be read into memory on a client-facing path.
            let bounded = i64::from(limit.clamp(1, MAX_EVENT_PAGE));
            let from = i64::try_from(from_sequence)
                .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?;

            let rows = sqlx::query(
                "SELECT id, run_id, sequence, event_type, payload_json, visibility, occurred_at \
                 FROM run_activity_events \
                 WHERE workspace_id = ? AND run_id = ? AND visibility = 'public' \
                   AND sequence >= ? \
                 ORDER BY sequence ASC LIMIT ?",
            )
            .bind(workspace.to_string())
            .bind(run.to_string())
            .bind(from)
            .bind(bounded)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;

            let mut events = Vec::with_capacity(rows.len());
            for row in &rows {
                let sequence = u64::try_from(int(row, "sequence")?)
                    .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?;
                events.push(StoredActivityEvent {
                    id: RunActivityEventId::parse(&text(row, "id")?)
                        .map_err(|_| RepositoryError::Corrupted { column: "event_id" })?,
                    run_id: RunId::parse(&text(row, "run_id")?)
                        .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?,
                    sequence,
                    event_type: text(row, "event_type")?,
                    payload_json: opt_text(row, "payload_json")?,
                    visibility: EventVisibility::parse(&text(row, "visibility")?)?,
                    occurred_at: parse_time(&text(row, "occurred_at")?, "occurred_at")?,
                });
            }

            Ok(RunEventPage {
                events,
                terminal_state: stored.state.is_terminal().then_some(stored.state),
            })
        })
    }

    fn claim_idempotency(
        &self,
        record: NewIdempotencyRecord,
    ) -> RepositoryFuture<'_, IdempotencyClaim> {
        Box::pin(async move {
            validate_idempotency_key(&record.key)?;

            // An existing record is read first so the common replay case answers
            // without attempting an insert that would fail on the unique constraint.
            let existing = read_idempotency(
                &self.pool,
                record.workspace_id,
                &record.operation,
                &record.key,
            )
            .await?;

            if let Some((digest, run_id)) = existing {
                if digest == record.request_digest {
                    let run_id = RunId::parse(&run_id)
                        .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?;
                    return Ok(IdempotencyClaim::Replay(run_id));
                }
                return Ok(IdempotencyClaim::Conflict);
            }

            // The run is checked before the insert so a record naming a run that does
            // not exist in this workspace reports `NotFound` rather than a conflict.
            // Without this, the foreign key would fire and be indistinguishable from
            // the concurrent-claim branch below, so a caller would be told "someone
            // else used this key" when the truth is "there is no such run".
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM agent_runs WHERE workspace_id = ? AND id = ?")
                    .bind(record.workspace_id.to_string())
                    .bind(record.run_id.to_string())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| RepositoryError::Query)?;
            if exists.is_none() {
                return Err(RepositoryError::NotFound);
            }

            let inserted = sqlx::query(
                "INSERT INTO idempotency_records (\
                     id, idempotency_key, workspace_id, operation, api_major, \
                     request_digest, run_id, created_at\
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(&record.key)
            .bind(record.workspace_id.to_string())
            .bind(&record.operation)
            .bind(i64::from(crate::http::API_MAJOR))
            .bind(&record.request_digest)
            .bind(record.run_id.to_string())
            .bind(record.created_at.to_string())
            .execute(&self.pool)
            .await;

            match inserted {
                Ok(result) if result.rows_affected() == 1 => Ok(IdempotencyClaim::Claimed),
                // A concurrent request claimed the same key between the read and this
                // insert. The unique constraint is what makes the claim atomic; the
                // honest answer is to report the conflict rather than to guess which
                // request won.
                Ok(_) | Err(_) => Ok(IdempotencyClaim::Conflict),
            }
        })
    }

    fn lookup_idempotency(
        &self,
        workspace: WorkspaceId,
        operation: &str,
        key: &str,
    ) -> RepositoryFuture<'_, Option<(String, RunId)>> {
        // The strings are copied before the future is built: the returned future is
        // tied to `&self`'s lifetime, so capturing shorter borrows would not compile.
        let operation = operation.to_owned();
        let key = key.to_owned();
        Box::pin(async move {
            let found = read_idempotency(&self.pool, workspace, &operation, &key).await?;
            found
                .map(|(digest, run_id)| {
                    Ok((
                        digest,
                        RunId::parse(&run_id)
                            .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?,
                    ))
                })
                .transpose()
        })
    }

    fn create_run_idempotent(
        &self,
        run: NewRun,
        opening_event: NewActivityEvent,
        record: NewIdempotencyRecord,
    ) -> RepositoryFuture<'_, IdempotencyClaim> {
        Box::pin(async move {
            validate_idempotency_key(&record.key)?;
            if record.run_id != run.id {
                return Err(RepositoryError::Conflict {
                    what: "idempotency_run",
                });
            }

            // Everything happens inside one transaction, so the decision to create and
            // the creation are the same unit. A separate check-then-insert would leave
            // exactly the window the contract's "acknowledged mutation state and
            // idempotency records are committed atomically" rule exists to close.
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RepositoryError::Query)?;

            let existing = read_idempotency(
                &mut *tx,
                record.workspace_id,
                &record.operation,
                &record.key,
            )
            .await?;
            if let Some((digest, run_id)) = existing {
                // Rolled back rather than committed: nothing was changed, and the
                // replay answer is read-only.
                tx.rollback().await.map_err(|_| RepositoryError::Query)?;
                if digest == record.request_digest {
                    let run_id = RunId::parse(&run_id)
                        .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?;
                    return Ok(IdempotencyClaim::Replay(run_id));
                }
                return Ok(IdempotencyClaim::Conflict);
            }

            insert_run(&mut tx, &run, &opening_event).await?;

            let inserted = sqlx::query(
                "INSERT INTO idempotency_records (\
                     id, idempotency_key, workspace_id, operation, api_major, \
                     request_digest, run_id, created_at\
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(&record.key)
            .bind(record.workspace_id.to_string())
            .bind(&record.operation)
            .bind(i64::from(crate::http::API_MAJOR))
            .bind(&record.request_digest)
            .bind(record.run_id.to_string())
            .bind(record.created_at.to_string())
            .execute(&mut *tx)
            .await;

            match inserted {
                Ok(result) if result.rows_affected() == 1 => {}
                // A concurrent request claimed the same key. Reported as a conflict so
                // the caller retries and replays rather than creating a duplicate.
                Ok(_) | Err(_) => {
                    return Ok(IdempotencyClaim::Conflict);
                }
            }

            tx.commit().await.map_err(|_| RepositoryError::Query)?;
            Ok(IdempotencyClaim::Claimed)
        })
    }
}

impl ConversationRepository for SqliteRepositories {
    fn create_conversation(&self, conversation: NewConversation) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            let result = sqlx::query(
                "INSERT INTO conversations (\
                     id, workspace_id, owner_user_id, title, status, channel_origin, \
                     created_at, updated_at, archived_at\
                 ) VALUES (?, ?, ?, ?, 'active', ?, ?, ?, NULL)",
            )
            .bind(conversation.id.to_string())
            .bind(conversation.workspace_id.to_string())
            .bind(conversation.owner_user_id.to_string())
            .bind(conversation.title.as_deref())
            .bind(&conversation.channel_origin)
            .bind(conversation.created_at.to_string())
            .bind(conversation.created_at.to_string())
            .execute(&self.pool)
            .await;

            match result {
                Ok(created) if created.rows_affected() == 1 => Ok(()),
                Ok(_) | Err(_) => Err(RepositoryError::Conflict {
                    what: "conversation",
                }),
            }
        })
    }

    fn load_conversation(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
    ) -> RepositoryFuture<'_, StoredConversation> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id, workspace_id, owner_user_id, title, status, \
                        created_at, updated_at \
                 FROM conversations WHERE workspace_id = ? AND id = ?",
            )
            .bind(workspace.to_string())
            .bind(conversation.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?
            .ok_or(RepositoryError::NotFound)?;

            let status = text(&row, "status")?;
            let archived = match status.as_str() {
                "archived" => true,
                "active" => false,
                _ => return Err(RepositoryError::Corrupted { column: "status" }),
            };
            Ok(StoredConversation {
                id: ConversationId::parse(&text(&row, "id")?)
                    .map_err(|_| RepositoryError::Corrupted { column: "id" })?,
                workspace_id: WorkspaceId::parse(&text(&row, "workspace_id")?).map_err(|_| {
                    RepositoryError::Corrupted {
                        column: "workspace_id",
                    }
                })?,
                owner_user_id: PrincipalId::parse(&text(&row, "owner_user_id")?).map_err(|_| {
                    RepositoryError::Corrupted {
                        column: "owner_user_id",
                    }
                })?,
                title: opt_text(&row, "title")?,
                archived,
                created_at: parse_time(&text(&row, "created_at")?, "created_at")?,
                updated_at: parse_time(&text(&row, "updated_at")?, "updated_at")?,
            })
        })
    }

    fn append_message(
        &self,
        workspace: WorkspaceId,
        message: NewMessage,
    ) -> RepositoryFuture<'_, u64> {
        Box::pin(async move {
            // The position is computed and inserted in one statement so two appends
            // cannot both claim it: a read-then-write would leave that window open.
            let result = sqlx::query(
                "INSERT INTO messages (\
                     id, workspace_id, conversation_id, role, content_schema_version, \
                     content_ref_or_json, sensitivity, source, sequence, created_at, deleted_at\
                 ) SELECT ?, ?, ?, ?, ?, ?, ?, ?, \
                     COALESCE(MAX(sequence), 0) + 1, ?, NULL \
                   FROM messages WHERE conversation_id = ?",
            )
            .bind(message.id.to_string())
            .bind(workspace.to_string())
            .bind(message.conversation_id.to_string())
            .bind(role_str(message.role))
            .bind(message.content_schema_version)
            .bind(&message.content)
            .bind(&message.sensitivity)
            .bind(&message.source)
            .bind(message.created_at.to_string())
            .bind(message.conversation_id.to_string())
            .execute(&self.pool)
            .await;

            match result {
                Ok(insert) if insert.rows_affected() == 1 => {}
                // The foreign key refuses a missing or foreign conversation, which
                // is reported as `NotFound` rather than as a generic conflict. A
                // duplicate position is a conflict, so the two are distinguished by
                // re-reading whether the conversation is visible to this workspace.
                Ok(_) | Err(_) => {
                    let visible: Option<i64> = sqlx::query_scalar(
                        "SELECT 1 FROM conversations WHERE workspace_id = ? AND id = ?",
                    )
                    .bind(workspace.to_string())
                    .bind(message.conversation_id.to_string())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| RepositoryError::Query)?;
                    return match visible {
                        Some(_) => Err(RepositoryError::Conflict {
                            what: "message_sequence",
                        }),
                        None => Err(RepositoryError::NotFound),
                    };
                }
            }

            let sequence: i64 = sqlx::query_scalar(
                "SELECT sequence FROM messages WHERE id = ? AND workspace_id = ?",
            )
            .bind(message.id.to_string())
            .bind(workspace.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;

            u64::try_from(sequence).map_err(|_| RepositoryError::Corrupted { column: "sequence" })
        })
    }

    fn load_messages(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<StoredMessage>> {
        Box::pin(async move {
            // The conversation is checked first so an absent one reports `NotFound`
            // rather than an empty page, which a caller would read as "no messages".
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM conversations WHERE workspace_id = ? AND id = ?")
                    .bind(workspace.to_string())
                    .bind(conversation.to_string())
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| RepositoryError::Query)?;
            if exists.is_none() {
                return Err(RepositoryError::NotFound);
            }

            let after = i64::try_from(after_sequence.unwrap_or(0))
                .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?;
            // The limit is clamped rather than trusted: an unbounded read would let
            // one call pull a whole conversation into memory.
            let bounded = i64::from(limit.clamp(1, 500));

            let rows = sqlx::query(
                "SELECT id, workspace_id, conversation_id, role, content_ref_or_json, \
                        sequence, sensitivity, created_at \
                 FROM messages WHERE workspace_id = ? AND conversation_id = ? \
                   AND sequence > ? AND deleted_at IS NULL \
                 ORDER BY sequence ASC LIMIT ?",
            )
            .bind(workspace.to_string())
            .bind(conversation.to_string())
            .bind(after)
            .bind(bounded)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;

            rows.iter()
                .map(|row| {
                    let sequence = int(row, "sequence")?;
                    Ok(StoredMessage {
                        id: MessageId::parse(&text(row, "id")?)
                            .map_err(|_| RepositoryError::Corrupted { column: "id" })?,
                        workspace_id: WorkspaceId::parse(&text(row, "workspace_id")?).map_err(
                            |_| RepositoryError::Corrupted {
                                column: "workspace_id",
                            },
                        )?,
                        conversation_id: ConversationId::parse(&text(row, "conversation_id")?)
                            .map_err(|_| RepositoryError::Corrupted {
                                column: "conversation_id",
                            })?,
                        role: parse_role(&text(row, "role")?)?,
                        content: text(row, "content_ref_or_json")?,
                        sequence: u64::try_from(sequence)
                            .map_err(|_| RepositoryError::Corrupted { column: "sequence" })?,
                        sensitivity: text(row, "sensitivity")?,
                        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
                    })
                })
                .collect()
        })
    }

    fn discard_if_empty(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
    ) -> RepositoryFuture<'_, bool> {
        Box::pin(async move {
            // The emptiness condition is part of the DELETE, so a message appended
            // between a check and this statement cannot be destroyed: the predicate is
            // evaluated inside the write, not before it.
            let deleted = sqlx::query(
                "DELETE FROM conversations \
                 WHERE workspace_id = ? AND id = ? \
                   AND NOT EXISTS (SELECT 1 FROM messages WHERE conversation_id = ?)",
            )
            .bind(workspace.to_string())
            .bind(conversation.to_string())
            .bind(conversation.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            Ok(deleted.rows_affected() == 1)
        })
    }
}

/// Returns the stored spelling of a role.
fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// Builds a [`StoredModelCall`] from a row.
fn stored_model_call(row: &sqlx::sqlite::SqliteRow) -> Result<StoredModelCall, RepositoryError> {
    let attempt = int(row, "attempt")?;
    Ok(StoredModelCall {
        id: ModelCallId::parse(&text(row, "id")?)
            .map_err(|_| RepositoryError::Corrupted { column: "id" })?,
        run_id: RunId::parse(&text(row, "run_id")?)
            .map_err(|_| RepositoryError::Corrupted { column: "run_id" })?,
        logical_call_id: ModelCallId::parse(&text(row, "logical_call_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "logical_call_id",
            }
        })?,
        attempt: u32::try_from(attempt)
            .map_err(|_| RepositoryError::Corrupted { column: "attempt" })?,
        provider_id: ProviderId::parse(&text(row, "provider_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "provider_id",
            }
        })?,
        model_id: ModelId::parse(&text(row, "model_id")?)
            .map_err(|_| RepositoryError::Corrupted { column: "model_id" })?,
        revision: opt_text(row, "model_revision")?
            .map(|value| {
                ModelRevision::parse(&value).map_err(|_| RepositoryError::Corrupted {
                    column: "model_revision",
                })
            })
            .transpose()?,
        state: jarvis_application::repository::model_call::ModelCallState::parse(&text(
            row, "state",
        )?)?,
        provider_request_id: opt_text(row, "provider_request_id")?,
        started_at: parse_time(&text(row, "started_at")?, "started_at")?,
        completed_at: opt_text(row, "completed_at")?
            .map(|value| parse_time(&value, "completed_at"))
            .transpose()?,
    })
}

impl ModelCallRepository for SqliteRepositories {
    fn record_attempt(&self, call: NewModelCall) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            let result = sqlx::query(
                "INSERT INTO model_calls (\
                     id, workspace_id, run_id, step_id, logical_call_id, attempt, \
                     provider_id, model_id, model_revision, route_decision_id, state, \
                     request_fingerprint, started_at\
                 ) VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, NULL, 'pending', ?, ?)",
            )
            .bind(call.id.to_string())
            .bind(call.workspace_id.to_string())
            .bind(call.run_id.to_string())
            .bind(call.logical_call_id.to_string())
            .bind(i64::from(call.attempt))
            .bind(call.model.provider_id.to_string())
            .bind(call.model.model_id.to_string())
            .bind(call.model.revision.as_ref().map(ToString::to_string))
            .bind(call.request_fingerprint.as_deref())
            .bind(call.started_at.to_string())
            .execute(&self.pool)
            .await;

            match result {
                Ok(insert) if insert.rows_affected() == 1 => Ok(()),
                // `(logical_call_id, attempt)` already exists: a retry must be a new
                // attempt number, not a rewrite of the first attempt's recorded
                // outcome, so this is refused rather than merged. The unique
                // constraint surfaces as an `Err`, which is the *same* conflict as an
                // unexpected zero-row result — mapping it to `Query` would report a
                // caller-visible conflict as a transport fault a retry might fix.
                Ok(_) | Err(_) => Err(RepositoryError::Conflict {
                    what: "logical_call_attempt",
                }),
            }
        })
    }

    fn load_attempt(
        &self,
        workspace: WorkspaceId,
        call: ModelCallId,
    ) -> RepositoryFuture<'_, StoredModelCall> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT id, run_id, logical_call_id, attempt, provider_id, model_id, \
                        model_revision, state, provider_request_id, started_at, completed_at \
                 FROM model_calls WHERE workspace_id = ? AND id = ?",
            )
            .bind(workspace.to_string())
            .bind(call.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?
            .ok_or(RepositoryError::NotFound)?;
            stored_model_call(&row)
        })
    }

    fn record_outcome(
        &self,
        workspace: WorkspaceId,
        call: ModelCallId,
        outcome: ModelCallOutcome,
    ) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            let usage_json = outcome
                .usage
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| RepositoryError::Corrupted {
                    column: "usage_json",
                })?;
            let finish_reason = outcome
                .finish_reason
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| RepositoryError::Corrupted {
                    column: "finish_reason",
                })?;

            // The predicate refuses an attempt that already reached a terminal
            // outcome, so a late writer cannot rewrite a recorded result.
            let result = sqlx::query(
                "UPDATE model_calls SET state = ?, provider_request_id = ?, \
                     continuation_ref = ?, usage_json = ?, estimated_cost_microunits = ?, \
                     finish_reason = ?, error_code = ?, first_output_at = ?, completed_at = ? \
                 WHERE workspace_id = ? AND id = ? \
                   AND state NOT IN ('completed', 'failed', 'cancelled')",
            )
            .bind(outcome.state.as_str())
            .bind(outcome.provider_request_id.as_deref())
            .bind(outcome.continuation_ref.as_deref())
            .bind(usage_json.as_deref())
            .bind(
                outcome
                    .estimated_cost_microunits
                    .and_then(|value| i64::try_from(value).ok()),
            )
            .bind(finish_reason.as_deref())
            .bind(outcome.error_code.as_deref())
            .bind(outcome.first_output_at.map(|value| value.to_string()))
            .bind(outcome.completed_at.map(|value| value.to_string()))
            .bind(workspace.to_string())
            .bind(call.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;

            if result.rows_affected() == 0 {
                // Nothing matched: either the attempt is absent/foreign, or it is
                // already terminal. The read resolves which, so the answer describes
                // what the update actually saw.
                let current: Option<String> = sqlx::query_scalar(
                    "SELECT state FROM model_calls WHERE workspace_id = ? AND id = ?",
                )
                .bind(workspace.to_string())
                .bind(call.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
                let Some(state) = current else {
                    return Err(RepositoryError::NotFound);
                };
                let parsed =
                    jarvis_application::repository::model_call::ModelCallState::parse(&state)?;
                if parsed.is_terminal() {
                    return Err(RepositoryError::VersionConflict {
                        expected: 0,
                        actual: 1,
                    });
                }
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
    }

    fn load_attempts(
        &self,
        workspace: WorkspaceId,
        logical_call_id: ModelCallId,
    ) -> RepositoryFuture<'_, Vec<StoredModelCall>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT id, run_id, logical_call_id, attempt, provider_id, model_id, \
                        model_revision, state, provider_request_id, started_at, completed_at \
                 FROM model_calls WHERE workspace_id = ? AND logical_call_id = ? \
                 ORDER BY attempt ASC",
            )
            .bind(workspace.to_string())
            .bind(logical_call_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            rows.iter().map(stored_model_call).collect()
        })
    }
}

/// Returns whether `event` belongs to `run`.
///
/// A free function rather than inline, so the check the transaction relies on is
/// separately named and testable.
#[must_use]
pub fn event_matches_run(event: &NewActivityEvent, run: RunId) -> bool {
    event.run_id == run
}

/// Returns whether `visibility` may be shown to an ordinary client.
#[must_use]
pub const fn is_client_visible(visibility: EventVisibility) -> bool {
    matches!(visibility, EventVisibility::Public)
}

#[cfg(test)]
#[path = "repositories_tests.rs"]
mod tests;

/// Rebuilds a [`ModelRef`] from its stored parts.
///
/// Exposed so a caller that only needs the provider/model pair does not have to
/// construct a [`StoredModelCall`] to get it.
///
/// # Errors
///
/// Returns [`RepositoryError::Corrupted`] when either part is not a valid name.
pub fn model_ref_of(
    provider: &str,
    model: &str,
    revision: Option<&str>,
) -> Result<ModelRef, RepositoryError> {
    let provider = ProviderId::parse(provider).map_err(|_| RepositoryError::Corrupted {
        column: "provider_id",
    })?;
    let model =
        ModelId::parse(model).map_err(|_| RepositoryError::Corrupted { column: "model_id" })?;
    let base = ModelRef::new(provider, model);
    match revision {
        Some(value) => Ok(base.with_revision(ModelRevision::parse(value).map_err(|_| {
            RepositoryError::Corrupted {
                column: "model_revision",
            }
        })?)),
        None => Ok(base),
    }
}
