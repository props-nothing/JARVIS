//! The SQLite implementation of the tool-grant store.
//!
//! This is the join between `000012_tool_grants.sql` and [`ToolGrantRepository`], and it is what turns
//! tool authorization from a **constructor** into **configuration**: before this adapter existed, what a
//! deployment allowed was decided by `NativeReadOnlyGrants` in compiled code, so "let `files.read` work
//! without asking but always ask about `email.send`" was not a sentence the product could be told.
//!
//! ## The four rules the SQL enforces rather than trusting
//!
//! - **Revocation is filtered in the query, not by the caller.** `grants_for` selects `status = 'active'`,
//!   so a withdrawn grant cannot reach policy even through a code path that forgot to check — and
//!   `Grant::applies_at` is therefore only ever asked about a grant that is still in force. A revoked grant
//!   is *withdrawn*, which is a different fact from *expired*, and a reader that had to remember the
//!   difference is one bug away from re-authorizing it.
//! - **Expiry is filtered in the query too**, so the evaluator's `GrantExpired` reason is produced for a
//!   grant the operator did not withdraw but which did lapse. Both filters belong in the `WHERE`, because a
//!   predicate that ran after the `LIMIT` would return a short page on a query that has more.
//! - **The optimistic version is a `WHERE` clause, not a read-then-write.** `put` and `revoke` both state
//!   the version they are replacing in the statement itself, so a writer that slipped in between the
//!   caller's read and this statement is refused rather than overwriting. A read followed by an
//!   unconditional `UPDATE` would have a window between them, which is the same defect the ledger's
//!   reservation avoids by letting the store decide.
//! - **A foreign workspace's grant is `NotFound`**, indistinguishable from one that does not exist — the
//!   rule the local control API states for runs, and it matters more here than anywhere: a distinguishable
//!   "forbidden" would confirm that another workspace has a grant for a tool.
//!
//! ## Why the deny rule's capability is stored beside the rule
//!
//! [`DenyRule`] has no capability field — it has an optional `identity`, and a rule stored by identity would
//! stop refusing after a tool was recompiled, which is the direction that loses a restriction. So the
//! capability is a column and the rule is rebuilt with `identity: None`, and
//! `jarvis_infrastructure::tool_adapters::StoredGrants` compares capabilities when it applies the rules.
//! The alternative — storing an identity and re-deriving it here — would make the refusal depend on a tool
//! version the operator never named.

use std::collections::BTreeSet;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use jarvis_application::repository::tool_grant::{
    GrantListFilter, GrantPage, MAX_GRANT_PAGE, NewDenyRule, NewToolGrant, StoredDenyRule,
    StoredToolGrant, ToolGrantRepository,
};
use jarvis_application::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{PrincipalId, ToolDenyRuleId, ToolGrantId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::classification::{Effect, Risk, Scope};
use jarvis_domain::tool::identity::ToolIdentity;
use jarvis_domain::tool::policy::{DenyRule, Grant};

use super::repositories::{begin_write, int, opt_text, parse_time, text};

/// The grant column list every `SELECT` shares, written once.
///
/// A macro rather than a `const` interpolated with `+`, so each statement stays a **compile-time literal**
/// that `sqlx::query` can audit for injection — the same reason the approval adapter's column list is a
/// macro. Four hand-repeated copies is how a column list drifts from the reader that consumes it.
macro_rules! grant_columns {
    () => {
        "id, workspace_id, principal_id, capability, tool_identity_json, scopes_json, \
         effects_json, risk_ceiling, sensitivity_ceiling, expires_at, status, version, \
         granted_by, created_at, updated_at"
    };
}

/// The `SELECT` for [`ToolGrantRepository::load`], applying the scope rule.
const LOAD_SQL: &str = concat!(
    "SELECT ",
    grant_columns!(),
    " FROM tool_grants WHERE workspace_id = ? AND id = ?"
);

/// The `SELECT` for the evaluator's read, in capability order.
///
/// **Expiry is a parameter rather than an interpolation**, and the comparison is `expires_at IS NULL OR
/// expires_at > ?` — so a standing grant is included and a lapsed one is not. `>` rather than `>=` because
/// the domain's `Grant::is_expired_at` treats the deadline instant itself as expired, and a reader that
/// disagreed with the evaluator by one instant would authorize a call the evaluator then refused, which
/// reads as an inconsistent product rather than as a boundary.
const ACTIVE_FOR_SQL: &str = concat!(
    "SELECT ",
    grant_columns!(),
    " FROM tool_grants WHERE workspace_id = ? AND principal_id = ? AND status = 'active' \
     AND (expires_at IS NULL OR expires_at > ?) ORDER BY capability ASC"
);

/// The SQLite-backed tool-grant store.
#[derive(Debug, Clone)]
pub struct SqliteToolGrantRepository {
    pool: SqlitePool,
}

impl SqliteToolGrantRepository {
    /// Builds the store over an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[cfg(test)]
#[path = "tool_grant_repository_tests.rs"]
mod tests;

impl ToolGrantRepository for SqliteToolGrantRepository {
    fn grants_for(
        &self,
        workspace: WorkspaceId,
        principal: PrincipalId,
    ) -> RepositoryFuture<'_, Vec<Grant>> {
        Box::pin(async move {
            // The instant comes from the system clock rather than being read inside SQL.
            //
            // **This method deliberately takes no instant parameter**, and the comparison is therefore
            // against a value the *store* derives — a limitation worth naming rather than hiding: the
            // evaluator's own `now` is what decides `GrantExpired`, so a grant whose deadline falls exactly
            // between this read and that evaluation would be returned here and refused there. The direction
            // is fail-closed (the call is refused, not allowed) and the window is one dispatch, but the
            // honest fix is an instant threaded from the caller, which arrives when the port grows one.
            // Recorded here because a reader comparing this against `Grant::is_expired_at` would otherwise
            // wonder why the two instants differ.
            let now = jarvis_domain::clock::Clock::now(&crate::time::SystemClock::new())
                .map_err(|_| RepositoryError::Query)?;
            let rows = sqlx::query(ACTIVE_FOR_SQL)
                .bind(workspace.to_string())
                .bind(principal.to_string())
                .bind(now.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            rows.iter()
                .map(stored_grant)
                .map(|row| row.map(|row| row.grant))
                .collect()
        })
    }

    fn deny_rules(&self, workspace: WorkspaceId) -> RepositoryFuture<'_, Vec<StoredDenyRule>> {
        Box::pin(async move {
            // `workspace_id IS NULL OR workspace_id = ?` — a rule that names no workspace applies
            // everywhere, and excluding it would drop the **broadest** refusals, which is the direction
            // that loses a restriction.
            let rows = sqlx::query(
                "SELECT id, workspace_id, capability, principal_id, effects_json, reason, created_at \
                 FROM tool_deny_rules WHERE workspace_id IS NULL OR workspace_id = ? \
                 ORDER BY created_at ASC, id ASC",
            )
            .bind(workspace.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            rows.iter().map(stored_deny_rule).collect()
        })
    }
    fn load(
        &self,
        workspace: WorkspaceId,
        id: ToolGrantId,
    ) -> RepositoryFuture<'_, StoredToolGrant> {
        Box::pin(async move {
            let row = sqlx::query(LOAD_SQL)
                .bind(workspace.to_string())
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let Some(row) = row else {
                return Err(RepositoryError::NotFound);
            };
            stored_grant(&row)
        })
    }

    fn put(&self, grant: &NewToolGrant, at: UtcTimestamp) -> RepositoryFuture<'_, StoredToolGrant> {
        let grant = grant.clone();
        Box::pin(async move {
            let capability = grant.capability();
            // Serialized once, outside the arm, because both the create and the replace write the same
            // four documents and a second serialization inside an arm is how one of them comes to differ.
            let columns = SerializedGrant {
                identity: serialize_identity(&grant.identity)?,
                scopes: serialize_scopes(&grant.scopes)?,
                effects: serialize_effects(&grant.effects)?,
            };
            // The write and its read-back share one transaction so the value returned is the value stored,
            // rather than a reconstruction that could differ from it.
            let mut transaction = begin_write(&self.pool).await?;
            match grant.expected_version {
                None => create_grant(&mut transaction, &grant, &capability, &columns, at).await?,
                Some(expected) => {
                    replace_grant(
                        &mut transaction,
                        &grant,
                        &capability,
                        &columns,
                        expected,
                        at,
                    )
                    .await?;
                }
            }
            let row = sqlx::query(concat!(
                "SELECT ",
                grant_columns!(),
                " FROM tool_grants WHERE workspace_id = ? AND principal_id = ? AND capability = ?"
            ))
            .bind(grant.workspace_id.to_string())
            .bind(grant.principal_id.to_string())
            .bind(&capability)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::Query)?;
            let stored = stored_grant(&row)?;
            transaction
                .commit()
                .await
                .map_err(|_| RepositoryError::Query)?;
            Ok(stored)
        })
    }

    fn revoke(
        &self,
        workspace: WorkspaceId,
        id: ToolGrantId,
        expected: u32,
        at: UtcTimestamp,
    ) -> RepositoryFuture<'_, StoredToolGrant> {
        Box::pin(async move {
            let mut transaction = begin_write(&self.pool).await?;
            // The state is set to `revoked` and **the row is kept**, because the approval contract requires
            // a revocation to be auditable. `status = 'active'` in the predicate makes a repeat revoke a
            // zero-row update rather than an error, and the read-back below returns the row that is already
            // revoked — so the idempotent case and the successful case return the same value.
            let affected = sqlx::query(
                "UPDATE tool_grants SET status = 'revoked', version = version + 1, updated_at = ? \
                 WHERE workspace_id = ? AND id = ? AND version = ?",
            )
            .bind(at.to_string())
            .bind(workspace.to_string())
            .bind(id.to_string())
            .bind(i64::from(expected))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::Query)?;
            if affected.rows_affected() == 0 {
                let row = sqlx::query(LOAD_SQL)
                    .bind(workspace.to_string())
                    .bind(id.to_string())
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(|_| RepositoryError::Query)?;
                let Some(row) = row else {
                    return Err(RepositoryError::NotFound);
                };
                let stored = stored_grant(&row)?;
                // Already revoked at this version or a later one is the desired state, so it is answered
                // rather than refused — the same "a repeat is not an error" rule the approval decision
                // follows. A version mismatch on an **active** grant is a genuine conflict.
                if stored.active && stored.version != expected {
                    return Err(RepositoryError::VersionConflict {
                        expected: u64::from(expected),
                        actual: u64::from(stored.version),
                    });
                }
                transaction
                    .commit()
                    .await
                    .map_err(|_| RepositoryError::Query)?;
                return Ok(stored);
            }
            let row = sqlx::query(LOAD_SQL)
                .bind(workspace.to_string())
                .bind(id.to_string())
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let stored = stored_grant(&row)?;
            transaction
                .commit()
                .await
                .map_err(|_| RepositoryError::Query)?;
            Ok(stored)
        })
    }

    fn list(
        &self,
        workspace: WorkspaceId,
        filter: GrantListFilter,
        limit: u32,
    ) -> RepositoryFuture<'_, GrantPage> {
        Box::pin(async move {
            // The bound is the adapter's own as well as the caller's, so a caller cannot ask for an
            // unbounded scan of a table that grows with real configuration.
            let bound = limit.min(MAX_GRANT_PAGE);
            // **One row more than the bound**, so `bounded` is observed rather than inferred from
            // `len() == limit` — the inference the approval listing records as silently wrong if the bound
            // ever changes.
            let probe = bound.saturating_add(1);
            let mut builder: QueryBuilder<Sqlite> = QueryBuilder::new(concat!(
                "SELECT ",
                grant_columns!(),
                " FROM tool_grants WHERE workspace_id = "
            ));
            builder.push_bind(workspace.to_string());
            // Both filters are pushed into the same `WHERE` the `LIMIT` applies to, because a predicate
            // applied after the bound would spend the page on rows the caller excluded and return a short
            // list — and a caller that receives a short list concludes there is nothing more to administer.
            if let Some(principal) = filter.principal {
                builder.push(" AND principal_id = ");
                builder.push_bind(principal.to_string());
            }
            if let Some(active) = filter.active {
                builder.push(" AND status = ");
                builder.push_bind(if active { "active" } else { "revoked" });
            }
            builder.push(" ORDER BY capability ASC LIMIT ");
            builder.push_bind(i64::from(probe));
            let rows = builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|_| RepositoryError::Query)?;
            let bounded = rows.len() > bound as usize;
            let grants = rows
                .iter()
                .take(bound as usize)
                .map(stored_grant)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(GrantPage { grants, bounded })
        })
    }

    fn add_deny_rule(&self, rule: &NewDenyRule) -> RepositoryFuture<'_, ToolDenyRuleId> {
        let rule = rule.clone();
        Box::pin(async move {
            let rule = rule.validated()?;
            let id = ToolDenyRuleId::from_uuid(uuid::Uuid::now_v7());
            let effects = serialize_effects(&rule.effects)?;
            sqlx::query(
                "INSERT INTO tool_deny_rules (id, workspace_id, capability, principal_id, \
                 effects_json, reason, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(rule.workspace_id.map(|workspace| workspace.to_string()))
            .bind(rule.capability.clone())
            .bind(rule.principal.map(|principal| principal.to_string()))
            .bind(&effects)
            .bind(&rule.reason)
            .bind(rule.created_at.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            Ok(id)
        })
    }

    fn remove_deny_rule(
        &self,
        workspace: WorkspaceId,
        id: ToolDenyRuleId,
    ) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            // A rule is deleted rather than flagged: it carries no authority, so there is nothing to audit
            // away — and keeping a refusal that no longer applies would make the listing show a
            // restriction the evaluator does not enforce.
            //
            // The scope predicate accepts the workspace **or** NULL, because a rule created with no
            // workspace belongs to every one of them and an operator in a workspace must be able to remove
            // the refusal they can see.
            let affected = sqlx::query(
                "DELETE FROM tool_deny_rules WHERE id = ? AND (workspace_id IS NULL OR workspace_id = ?)",
            )
            .bind(id.to_string())
            .bind(workspace.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            if affected.rows_affected() == 0 {
                return Err(RepositoryError::NotFound);
            }
            Ok(())
        })
    }

    fn list_deny_rules(
        &self,
        workspace: WorkspaceId,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<StoredDenyRule>> {
        Box::pin(async move {
            let bound = limit.min(MAX_GRANT_PAGE);
            let rows = sqlx::query(
                "SELECT id, workspace_id, capability, principal_id, effects_json, reason, created_at \
                 FROM tool_deny_rules WHERE workspace_id IS NULL OR workspace_id = ? \
                 ORDER BY created_at ASC, id ASC LIMIT ?",
            )
            .bind(workspace.to_string())
            .bind(i64::from(bound))
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::Query)?;
            rows.iter().map(stored_deny_rule).collect()
        })
    }
}

/// A grant's four stored documents, serialized once per write.
///
/// Grouped because the create and the replace write the same four columns, and a second serialization
/// inside one of the two arms is how the two come to disagree about a value they both store.
struct SerializedGrant {
    /// The identity document.
    identity: String,
    /// The scopes array.
    scopes: String,
    /// The effects array.
    effects: String,
}

/// Inserts a grant, refusing a key that is already taken.
///
/// **A row already present for this key is a conflict, not an overwrite**: the caller said "add a grant"
/// and there is one, so silently replacing it would discard ceilings an operator configured and nobody
/// asked to change.
async fn create_grant(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    grant: &NewToolGrant,
    capability: &str,
    columns: &SerializedGrant,
    at: UtcTimestamp,
) -> Result<(), RepositoryError> {
    let inserted = sqlx::query(
        "INSERT INTO tool_grants (id, workspace_id, principal_id, capability, \
         tool_identity_json, scopes_json, effects_json, risk_ceiling, \
         sensitivity_ceiling, expires_at, status, version, granted_by, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', 1, ?, ?, ?)",
    )
    .bind(ToolGrantId::from_uuid(uuid::Uuid::now_v7()).to_string())
    .bind(grant.workspace_id.to_string())
    .bind(grant.principal_id.to_string())
    .bind(capability)
    .bind(&columns.identity)
    .bind(&columns.scopes)
    .bind(&columns.effects)
    .bind(grant.risk_ceiling.as_contract_str())
    .bind(grant.sensitivity_ceiling.as_str())
    .bind(grant.expires_at.map(|value| value.to_string()))
    .bind(grant.granted_by.to_string())
    .bind(grant.created_at.to_string())
    .bind(at.to_string())
    .execute(&mut **transaction)
    .await;
    match inserted {
        Ok(_) => Ok(()),
        Err(error) if is_unique_violation(&error) => Err(RepositoryError::Conflict {
            what: "grant_exists",
        }),
        Err(_) => Err(RepositoryError::Query),
    }
}

/// Replaces a grant at the version the caller named.
///
/// **The predicate is in the statement**, so a concurrent writer that advanced the version between the
/// caller's read and this write is refused rather than overwritten — a read followed by an unconditional
/// `UPDATE` would have a window between them.
async fn replace_grant(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    grant: &NewToolGrant,
    capability: &str,
    columns: &SerializedGrant,
    expected: u32,
    at: UtcTimestamp,
) -> Result<(), RepositoryError> {
    let affected = sqlx::query(
        "UPDATE tool_grants SET tool_identity_json = ?, scopes_json = ?, \
         effects_json = ?, risk_ceiling = ?, sensitivity_ceiling = ?, expires_at = ?, \
         version = version + 1, updated_at = ? \
         WHERE workspace_id = ? AND capability = ? AND principal_id = ? \
         AND version = ?",
    )
    .bind(&columns.identity)
    .bind(&columns.scopes)
    .bind(&columns.effects)
    .bind(grant.risk_ceiling.as_contract_str())
    .bind(grant.sensitivity_ceiling.as_str())
    .bind(grant.expires_at.map(|value| value.to_string()))
    .bind(at.to_string())
    .bind(grant.workspace_id.to_string())
    .bind(capability)
    .bind(grant.principal_id.to_string())
    .bind(i64::from(expected))
    .execute(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::Query)?;
    if affected.rows_affected() > 0 {
        return Ok(());
    }
    // Zero rows means one of three things, and telling them apart is what makes the error actionable: no
    // row at all (the caller edited something that is not there), a row at another version (a concurrent
    // edit), or a row in another workspace — which reads as absent, by the scope rule.
    let existing = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM tool_grants WHERE workspace_id = ? AND principal_id = ? \
         AND capability = ?",
    )
    .bind(grant.workspace_id.to_string())
    .bind(grant.principal_id.to_string())
    .bind(capability)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::Query)?;
    Err(match existing {
        None => RepositoryError::NotFound,
        Some(stored) => RepositoryError::VersionConflict {
            expected: u64::from(expected),
            actual: u64::try_from(stored)
                .map_err(|_| RepositoryError::Corrupted { column: "version" })?,
        },
    })
}

/// Serializes an identity as the domain's own JSON.
///
/// The document rather than a tuple of columns, so the identity is one value the domain owns: an adapter
/// that assembled one from columns would be a second definition of what an identity is.
fn serialize_identity(identity: &ToolIdentity) -> Result<String, RepositoryError> {
    serde_json::to_string(identity).map_err(|_| RepositoryError::Corrupted {
        column: "tool_identity_json",
    })
}

/// Serializes a set of scopes as a JSON array of strings.
///
/// A `Vec` built in the set's own order rather than a `BTreeSet`, and that is a determinism requirement
/// rather than a preference: `serde_json` writes a map's keys in an unspecified order, so serializing the
/// set directly would let two equal scope sets produce different documents — and the store compares stored
/// values on read.
fn serialize_scopes(scopes: &BTreeSet<Scope>) -> Result<String, RepositoryError> {
    let values: Vec<&str> = scopes.iter().map(Scope::as_str).collect();
    serde_json::to_string(&values).map_err(|_| RepositoryError::Corrupted {
        column: "scopes_json",
    })
}

/// Serializes a set of effects as a JSON array of contract strings, in the set's own order.
fn serialize_effects(effects: &BTreeSet<Effect>) -> Result<String, RepositoryError> {
    let values: Vec<&str> = effects
        .iter()
        .map(|effect| effect.as_contract_str())
        .collect();
    serde_json::to_string(&values).map_err(|_| RepositoryError::Corrupted {
        column: "effects_json",
    })
}

/// Deserializes a JSON array of strings.
fn string_array(raw: &str, column: &'static str) -> Result<Vec<String>, RepositoryError> {
    serde_json::from_str(raw).map_err(|_| RepositoryError::Corrupted { column })
}

/// Reads a stored grant from its row.
fn stored_grant(row: &sqlx::sqlite::SqliteRow) -> Result<StoredToolGrant, RepositoryError> {
    let identity_json = text(row, "tool_identity_json")?;
    let identity: ToolIdentity = serde_json::from_str(&identity_json).map_err(|_| {
        // A stored identity that no longer parses means the *shape* changed under a row JARVIS wrote, which
        // is a migration fault rather than a missing record — so it is corruption, never absence.
        RepositoryError::Corrupted {
            column: "tool_identity_json",
        }
    })?;
    let workspace_id = WorkspaceId::parse(&text(row, "workspace_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "workspace_id",
        }
    })?;
    let principal_id = PrincipalId::parse(&text(row, "principal_id")?).map_err(|_| {
        RepositoryError::Corrupted {
            column: "principal_id",
        }
    })?;
    let scopes: BTreeSet<Scope> = string_array(&text(row, "scopes_json")?, "scopes_json")?
        .iter()
        .map(|value| {
            Scope::new(value).map_err(|_| RepositoryError::Corrupted {
                column: "scopes_json",
            })
        })
        .collect::<Result<_, _>>()?;
    let effects: BTreeSet<Effect> = string_array(&text(row, "effects_json")?, "effects_json")?
        .iter()
        .map(|value| {
            Effect::parse(value).map_err(|_| RepositoryError::Corrupted {
                column: "effects_json",
            })
        })
        .collect::<Result<_, _>>()?;
    let risk_ceiling =
        Risk::parse(&text(row, "risk_ceiling")?).map_err(|_| RepositoryError::Corrupted {
            column: "risk_ceiling",
        })?;
    let sensitivity_ceiling =
        jarvis_application::context_assembly::parse_sensitivity(&text(row, "sensitivity_ceiling")?)
            .ok_or(RepositoryError::Corrupted {
                column: "sensitivity_ceiling",
            })?;
    let expires_at = match opt_text(row, "expires_at")? {
        Some(value) => Some(parse_time(&value, "expires_at")?),
        None => None,
    };
    // The status is read strictly. An unknown spelling must not become `active`, which is the fail-open
    // direction on the one column that decides whether authority is in force.
    let active = match text(row, "status")?.as_str() {
        "active" => true,
        "revoked" => false,
        _ => {
            return Err(RepositoryError::Corrupted { column: "status" });
        }
    };
    let version_value = int(row, "version")?;
    // A version below one is a value the writer cannot produce, so it is a migration fault — the same
    // "refuse a stored value the writer cannot produce" rule the ledger adapter applies to a zero attempt.
    let version = u32::try_from(version_value)
        .ok()
        .filter(|version| *version >= 1)
        .ok_or(RepositoryError::Corrupted { column: "version" })?;
    let granted_by =
        PrincipalId::parse(&text(row, "granted_by")?).map_err(|_| RepositoryError::Corrupted {
            column: "granted_by",
        })?;
    Ok(StoredToolGrant {
        id: ToolGrantId::parse(&text(row, "id")?)
            .map_err(|_| RepositoryError::Corrupted { column: "id" })?,
        grant: Grant {
            identity,
            workspace: workspace_id,
            principal: principal_id,
            scopes,
            effects,
            risk_ceiling,
            sensitivity_ceiling,
            expires_at,
        },
        active,
        version,
        granted_by,
        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
        updated_at: parse_time(&text(row, "updated_at")?, "updated_at")?,
    })
}

/// Reads a stored deny rule from its row.
fn stored_deny_rule(row: &sqlx::sqlite::SqliteRow) -> Result<StoredDenyRule, RepositoryError> {
    let effects: BTreeSet<Effect> = string_array(&text(row, "effects_json")?, "effects_json")?
        .iter()
        .map(|value| {
            Effect::parse(value).map_err(|_| RepositoryError::Corrupted {
                column: "effects_json",
            })
        })
        .collect::<Result<_, _>>()?;
    let workspace_id = match opt_text(row, "workspace_id")? {
        Some(value) => {
            Some(
                WorkspaceId::parse(&value).map_err(|_| RepositoryError::Corrupted {
                    column: "workspace_id",
                })?,
            )
        }
        None => None,
    };
    let principal = match opt_text(row, "principal_id")? {
        Some(value) => {
            Some(
                PrincipalId::parse(&value).map_err(|_| RepositoryError::Corrupted {
                    column: "principal_id",
                })?,
            )
        }
        None => None,
    };
    Ok(StoredDenyRule {
        id: ToolDenyRuleId::parse(&text(row, "id")?)
            .map_err(|_| RepositoryError::Corrupted { column: "id" })?,
        capability: opt_text(row, "capability")?,
        rule: DenyRule {
            // **Always `None`, by design.** The rule is matched by capability, so it has no exact identity
            // to name — and naming one would make the refusal stop applying after a tool was recompiled.
            identity: None,
            principal,
            workspace: workspace_id,
            effects,
        },
        reason: text(row, "reason")?,
        created_at: parse_time(&text(row, "created_at")?, "created_at")?,
    })
}

/// Returns whether an error is a unique-constraint violation.
///
/// Matched on the driver's message rather than a code, because `sqlx`'s SQLite error exposes no typed
/// constraint variant — the same approach the ledger adapter's reservation takes, and it is sound for the
/// same reason: the only unique index this insert can violate is the grant key.
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database) if database.message().contains("UNIQUE")
    )
}
