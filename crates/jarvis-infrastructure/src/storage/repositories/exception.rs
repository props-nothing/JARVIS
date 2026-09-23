//! The SQLite model policy exception store.
//!
//! `model_policy_exceptions` holds "separately revocable relaxations", and these functions are
//! what make the contract's lifecycle structural rather than documented:
//!
//! - **A grant is insert-only.** A duplicate identity is a conflict behind the primary key, so
//!   rewriting an exception in place is impossible — which matters because an exception is the one
//!   record that *relaxes* a rule, and changing it after a decision was made would rewrite what
//!   that decision was permitted by.
//! - **Revocation and consumption are state changes, never deletes.** The row survives to explain
//!   a past call, which is the same historical-record rule the policy and decision tables follow.
//! - **Consumption is guarded by the statement, not by a pre-read.** The `UPDATE` carries
//!   `consumed_at IS NULL AND revoked_at IS NULL` in its `WHERE`, so two concurrent calls cannot
//!   both consume one single-use grant. A read followed by an unconditional write is exactly the
//!   shape that lets both succeed and reports a single-use exception as used twice.
//! - **`state` is not stored.** Usability is a question about an *instant*, so it is computed by
//!   the domain at the decision instant; a stored column would be wrong the moment the clock
//!   passed `expires_at`.
//!
//! The `scope_json` is re-parsed through the domain type on every read, so a row written by
//! another build — or corrupted — is reported as [`RepositoryError::Corrupted`] rather than passed
//! along as an opaque string.
//!
//! The functions are free rather than a second `impl ModelDataPolicyRepository` block, because Rust
//! allows one trait implementation per type: the trait's methods live together in `policy.rs` and
//! delegate here, which keeps the exception store in its own file without splitting the port.

use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::policy::{JarvisPolicyException, MAX_EXCEPTIONS_PER_WORKSPACE};
use jarvis_domain::ids::{ModelDataPolicyId, PolicyExceptionId, PrincipalId, WorkspaceId};
use jarvis_domain::model::exception::{ExceptionScope, PolicyRuleKey, RequiredAssurance};
use jarvis_domain::model::policy::PolicyVersionRef;
use jarvis_domain::time::UtcTimestamp;
use sqlx::Row as _;
use sqlx::SqlitePool;

/// The columns every exception read selects, spelled once.
///
/// One macro rather than a literal per query, because a duplicated column list is a defect
/// generator: the failure appears in whichever copy was forgotten, as this project has already
/// found once with the run columns.
macro_rules! exception_columns {
    () => {
        "id, workspace_id, policy_id, policy_version, granting_principal_id, rule_key, \
         scope_json, reason_ref, assurance, single_use, issued_at, expires_at, revoked_at, \
         consumed_at"
    };
}

/// Grants a durable exception.
///
/// # Errors
///
/// Returns [`RepositoryError::Conflict`] when the identity already exists or the record cannot be
/// serialized, and [`RepositoryError::Query`] for a driver failure.
pub(super) async fn grant(
    pool: &SqlitePool,
    workspace: WorkspaceId,
    exception: JarvisPolicyException,
) -> Result<(), RepositoryError> {
    let scope_json =
        serde_json::to_string(&exception.scope).map_err(|_| RepositoryError::Conflict {
            what: "exception_scope_unserializable",
        })?;

    let inserted = sqlx::query(
        "INSERT INTO model_policy_exceptions (\
             id, workspace_id, policy_id, policy_version, granting_principal_id, rule_key, \
             scope_json, reason_ref, assurance, single_use, issued_at, expires_at, \
             revoked_at, consumed_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(exception.id.to_string())
    .bind(workspace.to_string())
    .bind(exception.policy.policy_id.to_string())
    .bind(i64::from(exception.policy.version))
    .bind(exception.granting_principal_id.to_string())
    .bind(exception.rule.as_str())
    .bind(&scope_json)
    .bind(&exception.reason_ref)
    .bind(assurance_stored(exception.required_assurance))
    .bind(i64::from(exception.single_use))
    .bind(exception.issued_at.to_string())
    .bind(exception.expires_at.to_string())
    .bind(exception.revoked_at.map(|at| at.to_string()))
    .bind(exception.consumed_at.map(|at| at.to_string()))
    .execute(pool)
    .await;

    match inserted {
        Ok(result) if result.rows_affected() == 1 => Ok(()),
        // A duplicate identity is a conflict rather than a replacement, whether the driver reports
        // it as an error or as a zero-row result. Reporting a transport code instead would suggest
        // a blind retry could help when the honest answer is that this grant already exists.
        Ok(_) | Err(_) => Err(RepositoryError::Conflict {
            what: "policy_exception",
        }),
    }
}

/// Reads one exception, scoped to `workspace`.
///
/// # Errors
///
/// Returns [`RepositoryError::NotFound`] when it is absent or foreign, and
/// [`RepositoryError::Corrupted`] when a stored field cannot be read back.
pub(super) async fn load(
    pool: &SqlitePool,
    workspace: WorkspaceId,
    exception_id: PolicyExceptionId,
) -> Result<JarvisPolicyException, RepositoryError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        exception_columns!(),
        " FROM model_policy_exceptions WHERE workspace_id = ? AND id = ?"
    ))
    .bind(workspace.to_string())
    .bind(exception_id.to_string())
    .fetch_optional(pool)
    .await
    .map_err(|_| RepositoryError::Query)?;
    let Some(row) = row else {
        return Err(RepositoryError::NotFound);
    };
    read_exception(&row)
}

/// Reads every exception a workspace holds.
///
/// # Errors
///
/// Returns [`RepositoryError::Conflict`] when the workspace holds more than
/// [`MAX_EXCEPTIONS_PER_WORKSPACE`], [`RepositoryError::Corrupted`] when a stored field cannot be
/// read back, and [`RepositoryError::Query`] for a driver failure.
pub(super) async fn list(
    pool: &SqlitePool,
    workspace: WorkspaceId,
) -> Result<Vec<JarvisPolicyException>, RepositoryError> {
    // `LIMIT` one past the bound so an over-large set is *detected* rather than silently
    // truncated: a shortened list would drop a grant an operator believes is in force, and a route
    // would then be refused for a reason the policy does not actually give.
    let limit = i64::try_from(MAX_EXCEPTIONS_PER_WORKSPACE + 1)
        .map_err(|_| RepositoryError::Corrupted { column: "limit" })?;
    let rows = sqlx::query(concat!(
        "SELECT ",
        exception_columns!(),
        " FROM model_policy_exceptions WHERE workspace_id = ? ORDER BY issued_at LIMIT ?"
    ))
    .bind(workspace.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|_| RepositoryError::Query)?;

    if rows.len() > MAX_EXCEPTIONS_PER_WORKSPACE {
        return Err(RepositoryError::Conflict {
            what: "too_many_policy_exceptions",
        });
    }
    rows.iter().map(read_exception).collect()
}

/// Records that an exception was revoked.
///
/// # Errors
///
/// Returns [`RepositoryError::NotFound`] when it is absent or foreign, and
/// [`RepositoryError::Conflict`] when it was already consumed.
pub(super) async fn revoke(
    pool: &SqlitePool,
    workspace: WorkspaceId,
    exception_id: PolicyExceptionId,
    at: UtcTimestamp,
) -> Result<(), RepositoryError> {
    // A consumed grant cannot be revoked: the two are different decisions, and accepting both
    // would record a revocation that prevented nothing. `revoked_at IS NULL` also makes
    // re-revoking a no-op, so the FIRST instant is kept — a second instant would rewrite when the
    // decision was actually taken, which is the fact an audit reads this column for. Both
    // conditions are in the statement rather than a pre-read, for the same reason consumption's is.
    let updated = sqlx::query(
        "UPDATE model_policy_exceptions SET revoked_at = ? \
         WHERE workspace_id = ? AND id = ? AND consumed_at IS NULL AND revoked_at IS NULL",
    )
    .bind(at.to_string())
    .bind(workspace.to_string())
    .bind(exception_id.to_string())
    .execute(pool)
    .await
    .map_err(|_| RepositoryError::Query)?;

    if updated.rows_affected() == 1 {
        return Ok(());
    }
    // Nothing was updated, and the possible reasons are told apart by the record itself so the
    // caller is not told "not found" for a grant it can plainly read.
    match load(pool, workspace, exception_id).await {
        Ok(exception) if exception.consumed_at.is_some() => Err(RepositoryError::Conflict {
            what: "exception_already_consumed",
        }),
        // Already revoked, absent, or foreign. Re-revoking is idempotent, so an already-revoked
        // grant reports success rather than a conflict: the caller asked for a state the record is
        // already in.
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Records that a single-use exception was consumed.
///
/// # Errors
///
/// Returns [`RepositoryError::NotFound`] when it is absent or foreign, and
/// [`RepositoryError::Conflict`] when it was already consumed or revoked.
pub(super) async fn consume(
    pool: &SqlitePool,
    workspace: WorkspaceId,
    exception_id: PolicyExceptionId,
    at: UtcTimestamp,
) -> Result<(), RepositoryError> {
    // The guard is the statement: a grant is consumed at most once, and two concurrent calls
    // cannot both succeed because only one `UPDATE` can match a row whose `consumed_at` is still
    // `NULL`. `single_use = 1` is **part of the predicate** rather than a pre-read, so a
    // repeatable grant is left untouched without a second query and a race between the read and
    // the write cannot spend a grant that was never single-use.
    let updated = sqlx::query(
        "UPDATE model_policy_exceptions SET consumed_at = ? \
         WHERE workspace_id = ? AND id = ? AND single_use = 1 \
           AND consumed_at IS NULL AND revoked_at IS NULL",
    )
    .bind(at.to_string())
    .bind(workspace.to_string())
    .bind(exception_id.to_string())
    .execute(pool)
    .await
    .map_err(|_| RepositoryError::Query)?;

    if updated.rows_affected() == 1 {
        return Ok(());
    }
    // A zero-row update means the grant was already consumed or revoked, was never single-use,
    // or never existed. The distinction is reported so a caller is not sent to look for a missing
    // grant when the real answer is that its own exception is spent.
    match load(pool, workspace, exception_id).await {
        Ok(exception) if exception.consumed_at.is_some() => Err(RepositoryError::Conflict {
            what: "exception_already_consumed",
        }),
        Ok(exception) if exception.revoked_at.is_some() => Err(RepositoryError::Conflict {
            what: "exception_revoked",
        }),
        // A repeatable grant has nothing to spend, and saying so is *not* a refusal: the caller's
        // grant is intact and usable, so this is the one zero-row case that is not a conflict.
        Ok(exception) if !exception.single_use => Ok(()),
        Ok(_) => Err(RepositoryError::Conflict {
            what: "exception_not_consumable",
        }),
        Err(error) => Err(error),
    }
}

/// Reads one exception row, re-validating every field through the domain type.
///
/// The `scope_json` is parsed rather than passed along, so a row this build cannot interpret is
/// reported as corruption. The rule key and assurance are parsed the same way, and an unknown
/// value is `Corrupted` rather than a default: a row whose rule cannot be read must not be treated
/// as relaxing nothing, because it would still be counted as a grant an operator made.
fn read_exception(row: &sqlx::sqlite::SqliteRow) -> Result<JarvisPolicyException, RepositoryError> {
    let id = PolicyExceptionId::parse(&text(row, "id")?)
        .map_err(|_| RepositoryError::Corrupted { column: "id" })?;
    let workspace_id = WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "workspace_id",
        }
    })?;
    let policy_id = ModelDataPolicyId::parse(&text(row, "policy_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "policy_id",
        }
    })?;
    let version = json_i64(row, "policy_version")?;
    let version = u32::try_from(version).map_err(|_| RepositoryError::Corrupted {
        column: "policy_version",
    })?;
    let granting_principal_id =
        PrincipalId::parse(&text(row, "granting_principal_id")?).map_err(|_| {
            RepositoryError::Corrupted {
                column: "granting_principal_id",
            }
        })?;
    let rule = PolicyRuleKey::from_stored(&text(row, "rule_key")?)
        .ok_or(RepositoryError::Corrupted { column: "rule_key" })?;
    let scope: ExceptionScope = decode(&text(row, "scope_json")?, "scope_json")?;
    let required_assurance =
        assurance_from_stored(&text(row, "assurance")?).ok_or(RepositoryError::Corrupted {
            column: "assurance",
        })?;
    let single_use = match json_i64(row, "single_use")? {
        0 => false,
        1 => true,
        _ => {
            return Err(RepositoryError::Corrupted {
                column: "single_use",
            });
        }
    };

    Ok(JarvisPolicyException {
        id,
        workspace_id,
        policy: PolicyVersionRef { policy_id, version },
        granting_principal_id,
        rule,
        scope,
        reason_ref: text(row, "reason_ref")?,
        required_assurance,
        single_use,
        issued_at: parse_time(&text(row, "issued_at")?, "issued_at")?,
        expires_at: parse_time(&text(row, "expires_at")?, "expires_at")?,
        revoked_at: opt_time(row, "revoked_at")?,
        consumed_at: opt_time(row, "consumed_at")?,
    })
}

/// The stored spelling of an assurance requirement.
///
/// Written out rather than derived from `serde`, so the stored vocabulary is visible in one place
/// and a variant added to the domain cannot reach a database column without a decision here.
const fn assurance_stored(assurance: RequiredAssurance) -> &'static str {
    match assurance {
        RequiredAssurance::Standard => "standard",
        RequiredAssurance::Elevated => "elevated",
    }
}

/// Parses the stored assurance spelling.
fn assurance_from_stored(value: &str) -> Option<RequiredAssurance> {
    match value {
        "standard" => Some(RequiredAssurance::Standard),
        "elevated" => Some(RequiredAssurance::Elevated),
        _ => None,
    }
}

/// Deserializes a stored JSON part, mapping a shape failure to corruption.
fn decode<T: serde::de::DeserializeOwned>(
    json: &str,
    column: &'static str,
) -> Result<T, RepositoryError> {
    serde_json::from_str(json).map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads an integer column, mapping a shape failure to corruption.
fn json_i64(row: &sqlx::sqlite::SqliteRow, column: &'static str) -> Result<i64, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads a text column, mapping a driver/shape failure to corruption.
fn text(row: &sqlx::sqlite::SqliteRow, column: &'static str) -> Result<String, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads an optional text column.
fn opt_text(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<Option<String>, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads an optional timestamp column.
fn opt_time(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<Option<UtcTimestamp>, RepositoryError> {
    match opt_text(row, column)? {
        Some(value) => Ok(Some(parse_time(&value, column)?)),
        None => Ok(None),
    }
}

/// Parses a stored instant, mapping an unrepresentable value to corruption.
fn parse_time(value: &str, column: &'static str) -> Result<UtcTimestamp, RepositoryError> {
    UtcTimestamp::parse(value).map_err(|_| RepositoryError::Corrupted { column })
}
