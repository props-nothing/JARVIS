//! The SQLite model data policy store.
//!
//! `model_data_policies` stores **immutable versions**, and this adapter is what makes
//! "immutable" structural rather than documented: `insert_version` is an `INSERT` with a
//! `UNIQUE (policy_id, version)` constraint behind it, so a second write at the same version
//! is a typed conflict rather than an overwrite. An `UPDATE` here would silently rewrite the
//! rules a past route decision was made under, which is the one thing the contract's
//! "historical records retain the policy version needed to explain a past decision" rule
//! exists to prevent.
//!
//! Rules are stored as JSON rather than as columns, and the reason is that `PolicyRules` is a
//! typed struct with seven enums and four sets: flattening it into twenty columns would create
//! twenty places for the stored form and the domain type to disagree, with no single reader
//! that could notice the divergence. One serialized value means there is exactly one
//! representation, and a read that cannot be reinterpreted is reported as
//! [`RepositoryError::Corrupted`] rather than as absence.

use jarvis_application::repository::policy::{
    ModelDataPolicyRepository, NewPolicyVersion, StoredPolicyVersion,
};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{ModelDataPolicyId, ModelRouteDecisionId, WorkspaceId};
use jarvis_domain::model::policy::{
    ModelDataPolicyStatus, ModelRouteDecision, PolicyRules, PolicyVersionRef,
};
use jarvis_domain::time::UtcTimestamp;
use sqlx::Row as _;

use super::SqliteRepositories;

/// The columns every policy read selects, spelled once.
///
/// A duplicated column list is a defect generator — an earlier round of this project lost a
/// column to exactly that — so the list exists in one macro and each query names it.
macro_rules! policy_columns {
    () => {
        "id, policy_id, version, workspace_id, name, status, rules_json, created_at"
    };
}

impl ModelDataPolicyRepository for SqliteRepositories {
    fn insert_version(&self, policy: NewPolicyVersion) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            let policy = policy.validated()?;
            // The rules are serialized here rather than by the caller, so the stored form is
            // produced by one place. A serialization failure is a programming error rather
            // than a caller's mistake, but it is still reported as a conflict with a named
            // subject instead of being unwrapped on a write path.
            let rules_json =
                serde_json::to_string(&policy.rules).map_err(|_| RepositoryError::Conflict {
                    what: "policy_rules_unserializable",
                })?;

            let inserted = sqlx::query(
                "INSERT INTO model_data_policies (\
                     id, policy_id, version, workspace_id, name, status, rules_json, created_at\
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid::Uuid::now_v7().to_string())
            .bind(policy.policy_id.to_string())
            .bind(i64::from(policy.version))
            .bind(policy.workspace_id.to_string())
            .bind(&policy.name)
            .bind(policy.status.as_str())
            .bind(&rules_json)
            .bind(policy.created_at.to_string())
            .execute(&self.pool)
            .await;

            match inserted {
                Ok(result) if result.rows_affected() == 1 => Ok(()),
                // A duplicate is the contract's version conflict, and it is reported as one
                // whether the driver surfaces it as an error or as a zero-row result.
                // Reporting it as a generic query failure would suggest a retry could help,
                // when the honest answer is that the caller's view of the version is stale.
                Ok(_) | Err(_) => Err(RepositoryError::VersionConflict {
                    expected: u64::from(policy.version),
                    // The store cannot know the current version without a second read, and
                    // inventing a number here would be a fabricated fact in an error a caller
                    // is expected to act on.
                    actual: 0,
                }),
            }
        })
    }

    fn load_version(
        &self,
        workspace: WorkspaceId,
        reference: PolicyVersionRef,
    ) -> RepositoryFuture<'_, StoredPolicyVersion> {
        Box::pin(async move {
            let row = sqlx::query(concat!(
                "SELECT ",
                policy_columns!(),
                " FROM model_data_policies \
                 WHERE workspace_id = ? AND policy_id = ? AND version = ?"
            ))
            .bind(workspace.to_string())
            .bind(reference.policy_id.to_string())
            .bind(i64::from(reference.version))
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };
            read_policy(&row)
        })
    }

    fn load_active(&self, workspace: WorkspaceId) -> RepositoryFuture<'_, StoredPolicyVersion> {
        Box::pin(async move {
            // One row, and the schema's `UNIQUE (workspace_id, name, version)` plus the
            // index on `(workspace_id, status)` are what make "the active policy" well
            // defined. Ordering by version descending picks the newest when a workspace has
            // archived and re-activated across several versions, so the read cannot return
            // an old active version merely because it was inserted first.
            let row = sqlx::query(concat!(
                "SELECT ",
                policy_columns!(),
                " FROM model_data_policies \
                 WHERE workspace_id = ? AND status = 'active' \
                 ORDER BY version DESC LIMIT 1"
            ))
            .bind(workspace.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };
            read_policy(&row)
        })
    }

    fn record_decision(
        &self,
        workspace: WorkspaceId,
        decision_id: ModelRouteDecisionId,
        decision: ModelRouteDecision,
    ) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            // Each part is serialized separately rather than the whole decision as one blob,
            // because the contract names them as distinct stored fields: the requested and
            // effective policies, the evidence references, and the candidate rejection
            // reasons are each read for a different question.
            let requested = encode(&decision.requested)?;
            let effective = encode(&decision.effective)?;
            let evidence = encode(&decision.evidence_refs)?;
            let rejections = encode(&decision.rejected_candidates)?;

            let inserted = sqlx::query(
                "INSERT INTO model_route_decisions (\
                     id, workspace_id, policy_id, policy_version, requested_json, \
                     effective_json, evidence_refs_json, rejections_json, exception_id, decided_at\
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(decision_id.to_string())
            .bind(workspace.to_string())
            .bind(decision.policy.policy_id.to_string())
            .bind(i64::from(decision.policy.version))
            .bind(requested)
            .bind(effective)
            .bind(evidence)
            .bind(rejections)
            .bind(decision.exception_ref.as_deref())
            .bind(decision.decided_at.to_string())
            .execute(&self.pool)
            .await;

            match inserted {
                Ok(result) if result.rows_affected() == 1 => Ok(()),
                // A decision explains a call that already happened, so a second write at the
                // same identity is a conflict rather than a replacement: overwriting it would
                // destroy the evidence instead of updating it.
                Ok(_) | Err(_) => Err(RepositoryError::Conflict {
                    what: "route_decision",
                }),
            }
        })
    }

    fn load_decision(
        &self,
        workspace: WorkspaceId,
        decision_id: ModelRouteDecisionId,
    ) -> RepositoryFuture<'_, ModelRouteDecision> {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT policy_id, policy_version, requested_json, effective_json, \
                        evidence_refs_json, rejections_json, exception_id, decided_at \
                 FROM model_route_decisions WHERE workspace_id = ? AND id = ?",
            )
            .bind(workspace.to_string())
            .bind(decision_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };

            let policy_id = ModelDataPolicyId::parse(&text(&row, "policy_id")?).map_err(|_| {
                RepositoryError::Corrupted {
                    column: "policy_id",
                }
            })?;
            let version = json_i64(&row, "policy_version", "policy_version")?;
            let version = u32::try_from(version).map_err(|_| RepositoryError::Corrupted {
                column: "policy_version",
            })?;

            Ok(ModelRouteDecision {
                policy: PolicyVersionRef { policy_id, version },
                requested: decode(&text(&row, "requested_json")?, "requested_json")?,
                effective: decode(&text(&row, "effective_json")?, "effective_json")?,
                // Absent rather than empty is the stored form for "no evidence notes", so a
                // NULL and an empty array both read as an empty list — the two are the same
                // fact and storing a distinction that nothing reads would be a false promise.
                evidence_refs: match opt_text(&row, "evidence_refs_json")? {
                    Some(json) => decode(&json, "evidence_refs_json")?,
                    None => Vec::new(),
                },
                rejected_candidates: match opt_text(&row, "rejections_json")? {
                    Some(json) => decode(&json, "rejections_json")?,
                    None => Vec::new(),
                },
                exception_ref: opt_text(&row, "exception_id")?,
                decided_at: parse_time(&text(&row, "decided_at")?, "decided_at")?,
            })
        })
    }
}

/// Reads one policy row.
///
/// The rules are re-parsed through the domain type rather than passed through as a string, so
/// a row whose rules this build cannot interpret is reported as corruption. Passing the JSON
/// along would push the failure to whoever eventually read it, and reporting the row as
/// absent instead would let a caller create a replacement for a policy that is still there.
fn read_policy(row: &sqlx::sqlite::SqliteRow) -> Result<StoredPolicyVersion, RepositoryError> {
    let policy_id = ModelDataPolicyId::parse(&text(row, "policy_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "policy_id",
        }
    })?;
    let version = json_i64(row, "version", "version")?;
    let version =
        u32::try_from(version).map_err(|_| RepositoryError::Corrupted { column: "version" })?;
    let workspace_id = WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "workspace_id",
        }
    })?;
    let status = ModelDataPolicyStatus::from_stored(&text(row, "status")?)
        .ok_or(RepositoryError::Corrupted { column: "status" })?;
    let rules: PolicyRules = decode(&text(row, "rules_json")?, "rules_json")?;

    Ok(StoredPolicyVersion {
        policy_id,
        version,
        workspace_id,
        name: text(row, "name")?,
        status,
        rules,
        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
    })
}

/// Serializes one part of a decision.
fn encode<T: serde::Serialize>(value: &T) -> Result<String, RepositoryError> {
    serde_json::to_string(value).map_err(|_| RepositoryError::Conflict {
        what: "route_decision_unserializable",
    })
}

/// Deserializes one part of a decision or a policy.
fn decode<T: serde::de::DeserializeOwned>(
    json: &str,
    column: &'static str,
) -> Result<T, RepositoryError> {
    serde_json::from_str(json).map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads an integer column, mapping a shape failure to corruption.
fn json_i64(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
    reported: &'static str,
) -> Result<i64, RepositoryError> {
    row.try_get(column)
        .map_err(|_| RepositoryError::Corrupted { column: reported })
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

/// Parses a stored timestamp, refusing an uninterpretable value.
fn parse_time(value: &str, column: &'static str) -> Result<UtcTimestamp, RepositoryError> {
    UtcTimestamp::parse(value).map_err(|_| RepositoryError::Corrupted { column })
}
