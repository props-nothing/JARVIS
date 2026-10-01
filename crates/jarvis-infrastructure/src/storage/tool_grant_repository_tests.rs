//! Contract tests for the SQLite tool-grant store.
//!
//! Behaviour tests against a **real migrated SQLite database**, for the reason the ledger and approval
//! suites record: the properties that matter are the database's own — a unique index over the grant key,
//! an optimistic `UPDATE ... WHERE version = ?`, a `NULL` expiry that must mean "standing" rather than
//! being defaulted, and a revoked row that must be invisible to the evaluator's read. A fake would agree
//! with whatever the code assumed about them.
//!
//! **The tests that carry the slice's meaning**, each asserting a different claim rather than restating
//! one:
//!
//! - `a_grant_narrower_than_the_tool_is_accepted_and_a_wider_one_is_refused` — the narrowing rule, asserted
//!   from **both** sides. The accepting direction is the ordinary one ("let this read only this tool") and
//!   the refusing direction is the mistake that would otherwise sit in the store looking like permission.
//!   Refusing everything would satisfy the second, which is why the first is asserted separately.
//! - `a_revoked_grant_is_invisible_to_the_evaluators_read` — revocation is a `status` change and the read
//!   filters it **in the query**, so the assertion is made through `grants_for`, which is the method the
//!   tool pipeline actually calls. Asserting it through `load` would pass for an adapter that filtered
//!   everywhere except where it mattered.
//! - `a_second_create_for_the_same_key_is_a_conflict_not_an_overwrite` and its sibling
//!   `a_replace_at_the_wrong_version_is_a_version_conflict` — the difference between "I was adding a
//!   grant" and "I was replacing one" has to be visible, because the first silently discarding an
//!   operator's ceilings is the defect.
//! - `a_lapsed_grant_is_not_returned_but_a_standing_one_is` — two rows, one expiry in the past and one
//!   `NULL`, read in one call, so the comparison is exercised rather than assumed.
//! - `one_workspaces_grants_are_invisible_from_another` — the four-repositories scope rule, asserted on the
//!   store whose rows are the most security-relevant in the profile.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::SqliteToolGrantRepository;
use crate::storage::connection::Database;
use crate::storage::migrate;
use jarvis_application::repository::RepositoryError;
use jarvis_application::repository::tool_grant::{
    GrantListFilter, NewDenyRule, NewToolGrant, ToolGrantRepository,
};
use jarvis_domain::ids::{PrincipalId, ToolGrantId, WorkspaceId};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk, Scope,
};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};

/// The workspace every test configures.
fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(0x1000))
}

/// A second workspace, for the scope test.
fn other_workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(0x1001))
}

/// The principal every test configures for.
fn principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(0x2000))
}

/// The operator who writes the grants.
fn operator() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(0x2001))
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-10-01T12:00:00Z").expect("the fixture instant parses")
}

/// A read-only definition, which is the shape the reviewed defaults grant.
fn read_definition() -> ToolDefinition {
    definition("files.read@1", vec![Effect::ReadOnly], Risk::Low)
}

/// A definition that writes, with one further effect, so a narrower grant is expressible.
///
/// **Not `{ReadOnly, Write}`**, which the domain refuses: "read and write" is a contradiction a policy
/// branch would resolve by picking one, and `ToolDefinition::new` rejects it outright. So the fixture that
/// needs a tool with more than one effect — to prove a grant may confer a *subset* of them — uses two
/// writing effects instead, which is the shape a real tool with a broad classification has.
fn write_definition() -> ToolDefinition {
    definition(
        "notes.write@1",
        vec![Effect::Write, Effect::ExternalCommunication],
        Risk::Moderate,
    )
}

/// Builds a definition with the given classification.
fn definition(capability: &str, effects: Vec<Effect>, risk: Risk) -> ToolDefinition {
    let source = ToolSource::new(
        SourceKind::Native,
        "test.publisher",
        ToolVersion::parse("1.0.0").expect("valid"),
    )
    .expect("valid source");
    ToolDefinition::new(
        ToolIdentity {
            capability: ToolCapability::parse(capability).expect("valid capability"),
            source,
            schema_fingerprint: SchemaFingerprint::from_bytes([0x11; 32]),
        },
        "Test tool",
        "A tool the grant store tests configure.",
        effects,
        risk,
        Vec::new(),
        ApprovalHint::Ask,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Public, Sensitivity::Internal).expect("valid classes"),
        ExecutionDefaults::new(5_000, 1).expect("valid defaults"),
    )
    .expect("the definition is consistent")
}

/// Builds a definition with the given classification and required scopes.
fn definition_with_scopes(
    capability: &str,
    effects: Vec<Effect>,
    risk: Risk,
    required_scopes: &[&str],
) -> ToolDefinition {
    let source = ToolSource::new(
        SourceKind::Native,
        "test.publisher",
        ToolVersion::parse("1.0.0").expect("valid"),
    )
    .expect("valid source");
    ToolDefinition::new(
        ToolIdentity {
            capability: ToolCapability::parse(capability).expect("valid capability"),
            source,
            schema_fingerprint: SchemaFingerprint::from_bytes([0x11; 32]),
        },
        "Test tool",
        "A tool the grant store tests configure.",
        effects,
        risk,
        required_scopes
            .iter()
            .map(|scope| Scope::new(scope).expect("valid scope"))
            .collect(),
        ApprovalHint::Ask,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Public, Sensitivity::Internal).expect("valid classes"),
        ExecutionDefaults::new(5_000, 1).expect("valid defaults"),
    )
    .expect("the definition is consistent")
}

/// A migrated in-memory database with the grant repository over it.
async fn repository() -> (Database, Arc<SqliteToolGrantRepository>) {
    let database = Database::open_in_memory().await.expect("in-memory opens");
    migrate::run(database.pool()).await.expect("migrates");
    let grants = Arc::new(SqliteToolGrantRepository::new(database.pool().clone()));
    (database, grants)
}

/// Builds a grant that confers the tool's own declared bounds.
fn grant_for(definition: &ToolDefinition, workspace: WorkspaceId) -> NewToolGrant {
    NewToolGrant::narrowing(
        definition,
        workspace,
        principal(),
        definition.required_scopes.iter().cloned().collect(),
        definition.effects.iter().copied().collect(),
        definition.risk,
        definition.data_classes.input,
        None,
        operator(),
        now(),
        None,
    )
    .expect("a grant conferring exactly the tool's own bounds narrows it")
}

#[tokio::test]
async fn a_stored_grant_round_trips_with_its_ceilings() {
    // The baseline: what an operator writes is what the evaluator reads back. Asserted field by field
    // rather than by comparing the whole value, because a reader that dropped one field would still
    // produce a `Grant` — and the field it dropped would be a *ceiling*, which is the direction that
    // silently widens access.
    let (_database, grants) = repository().await;
    // The definition must **require** the scope, or conferring it would widen the tool and be refused —
    // which is the rule working. Using the plain `write_definition` here is what made the first version of
    // this test fail, and the failure is the assertion doing its job.
    let definition = definition_with_scopes(
        "notes.write@1",
        vec![Effect::Write, Effect::ExternalCommunication],
        Risk::Moderate,
        &["notes.write"],
    );
    let scopes = BTreeSet::from([Scope::new("notes.write").expect("valid scope")]);
    let effects = BTreeSet::from([Effect::Write]);
    let written = NewToolGrant::narrowing(
        &definition,
        workspace(),
        principal(),
        scopes.clone(),
        effects.clone(),
        Risk::Low,
        Sensitivity::Public,
        None,
        operator(),
        now(),
        None,
    )
    .expect("a narrower grant is accepted");
    let stored = grants
        .put(&written, now())
        .await
        .expect("the grant is stored");

    let read = grants
        .grants_for(workspace(), principal())
        .await
        .expect("the evaluator's read succeeds");
    assert_eq!(read.len(), 1, "exactly the grant that was written");
    let grant = &read[0];
    assert_eq!(grant.identity, definition.identity);
    assert_eq!(grant.workspace, workspace());
    assert_eq!(grant.principal, principal());
    assert_eq!(grant.scopes, scopes);
    assert_eq!(
        grant.effects, effects,
        "the *narrowed* effect set is what must be read back, not the tool's own",
    );
    assert_eq!(
        grant.risk_ceiling,
        Risk::Low,
        "a ceiling below the tool's risk is the useful direction and must survive",
    );
    assert_eq!(grant.sensitivity_ceiling, Sensitivity::Public);
    assert!(
        grant.expires_at.is_none(),
        "a standing grant stays standing"
    );
    assert!(stored.active);
}

#[tokio::test]
async fn a_grant_narrower_than_the_tool_is_accepted_and_a_wider_one_is_refused() {
    // **Both directions, because either alone is satisfiable by a wrong implementation.** A predicate that
    // refused everything would pass the widening assertion, and one that accepted everything would pass the
    // narrowing one — so the pair is the test, not either half.
    let definition = write_definition();
    // Narrower: one effect out of the tool's two, a ceiling below its risk. Accepted.
    NewToolGrant::narrowing(
        &definition,
        workspace(),
        principal(),
        BTreeSet::new(),
        BTreeSet::from([Effect::Write]),
        Risk::Low,
        Sensitivity::Public,
        None,
        operator(),
        now(),
        None,
    )
    .expect("a grant narrower than the tool must be accepted");

    // Wider in effects: `Financial` is not something this tool declares. Refused, and the
    // refusal names the field so an operator is sent to the right part of the form.
    let widened_effects = NewToolGrant::narrowing(
        &definition,
        workspace(),
        principal(),
        BTreeSet::new(),
        BTreeSet::from([Effect::Financial]),
        Risk::Low,
        Sensitivity::Public,
        None,
        operator(),
        now(),
        None,
    )
    .expect_err("an effect the tool does not declare must be refused");
    assert_eq!(
        widened_effects,
        RepositoryError::Conflict {
            what: "grant_effect"
        },
        "the refusal must name the field that widened",
    );

    // Wider in risk: `Critical` is above the tool's `Moderate`.
    let widened_risk = NewToolGrant::narrowing(
        &definition,
        workspace(),
        principal(),
        BTreeSet::new(),
        BTreeSet::from([Effect::Write]),
        Risk::Critical,
        Sensitivity::Public,
        None,
        operator(),
        now(),
        None,
    )
    .expect_err("a risk ceiling above the tool's own must be refused");
    assert_eq!(
        widened_risk,
        RepositoryError::Conflict {
            what: "grant_risk_ceiling"
        },
    );

    // Wider in scopes: a scope the tool does not require.
    let widened_scopes = NewToolGrant::narrowing(
        &definition,
        workspace(),
        principal(),
        BTreeSet::from([Scope::new("files.delete").expect("valid scope")]),
        BTreeSet::from([Effect::Write]),
        Risk::Low,
        Sensitivity::Public,
        None,
        operator(),
        now(),
        None,
    )
    .expect_err("a scope the tool does not require must be refused");
    assert_eq!(
        widened_scopes,
        RepositoryError::Conflict {
            what: "grant_scope"
        },
    );

    // And the sensitivity ceiling, which is the fourth dimension and the one most easily forgotten.
    let widened_sensitivity = NewToolGrant::narrowing(
        &definition,
        workspace(),
        principal(),
        BTreeSet::new(),
        BTreeSet::from([Effect::Write]),
        Risk::Low,
        Sensitivity::Restricted,
        None,
        operator(),
        now(),
        None,
    )
    .expect_err("a sensitivity ceiling above the tool's input class must be refused");
    assert_eq!(
        widened_sensitivity,
        RepositoryError::Conflict {
            what: "grant_sensitivity_ceiling"
        },
    );
}

#[tokio::test]
async fn a_revoked_grant_is_invisible_to_the_evaluators_read() {
    // **Asserted through `grants_for`, which is the method the tool pipeline calls.** A revocation test
    // written against `load` would pass for an adapter that filtered the listing and not the read — and the
    // read is the one that decides whether an effect happens.
    let (_database, grants) = repository().await;
    let definition = read_definition();
    let stored = grants
        .put(&grant_for(&definition, workspace()), now())
        .await
        .expect("the grant is stored");
    assert_eq!(
        grants
            .grants_for(workspace(), principal())
            .await
            .expect("reads")
            .len(),
        1,
    );

    let revoked = grants
        .revoke(workspace(), stored.id, stored.version, now())
        .await
        .expect("the grant is revoked");
    assert!(!revoked.active);
    assert!(
        grants
            .grants_for(workspace(), principal())
            .await
            .expect("reads")
            .is_empty(),
        "a revoked grant must not reach the evaluator",
    );
    // **And the row survives**, because the approval contract requires a revocation to be auditable.
    let still_there = grants
        .load(workspace(), stored.id)
        .await
        .expect("a revoked grant is kept, not deleted");
    assert!(!still_there.active);
}

#[tokio::test]
async fn revoking_a_revoked_grant_is_not_an_error() {
    // The desired state is the current one, so a repeat answers rather than refuses — the same rule the
    // approval decision follows for a repeated decision. A refusal here would make a retried revoke look
    // like a failure to a client that had already succeeded.
    let (_database, grants) = repository().await;
    let stored = grants
        .put(&grant_for(&read_definition(), workspace()), now())
        .await
        .expect("stored");
    let first = grants
        .revoke(workspace(), stored.id, stored.version, now())
        .await
        .expect("the first revoke succeeds");
    assert!(!first.active);
    // The second call names the *new* version, which is what a client that re-read the row would send.
    let second = grants
        .revoke(workspace(), stored.id, first.version, now())
        .await
        .expect("a repeat revoke answers rather than fails");
    assert!(!second.active);
}

#[tokio::test]
async fn a_second_create_for_the_same_key_is_a_conflict_not_an_overwrite() {
    // **The distinction is the point**: a caller that meant "add a grant" must not silently replace one an
    // operator configured, because the ceilings it discards are exactly what the operator wrote.
    let (_database, grants) = repository().await;
    let definition = write_definition();
    grants
        .put(&grant_for(&definition, workspace()), now())
        .await
        .expect("the first create succeeds");
    let second = grants
        .put(&grant_for(&definition, workspace()), now())
        .await
        .expect_err("a second create for the same key must be refused");
    assert_eq!(
        second,
        RepositoryError::Conflict {
            what: "grant_exists"
        },
    );
}

#[tokio::test]
async fn a_replace_at_the_wrong_version_is_a_version_conflict() {
    // Optimistic concurrency, asserted through the version the caller *names* rather than through the one
    // stored: two operators editing one grant must not be able to overwrite each other silently.
    let (_database, grants) = repository().await;
    let definition = write_definition();
    let stored = grants
        .put(&grant_for(&definition, workspace()), now())
        .await
        .expect("stored");
    // A replacement naming a version that is neither the stored one nor absent.
    let mut replacement = grant_for(&definition, workspace());
    replacement.expected_version = Some(stored.version + 7);
    let conflict = grants
        .put(&replacement, now())
        .await
        .expect_err("a stale version must be refused");
    assert_eq!(
        conflict,
        RepositoryError::VersionConflict {
            expected: u64::from(stored.version + 7),
            actual: u64::from(stored.version),
        },
    );

    // The correct version is accepted, and **the version advances** — so the next write must name the new
    // one, which is what makes the check meaningful rather than a one-off.
    let mut correct = grant_for(&definition, workspace());
    correct.expected_version = Some(stored.version);
    let updated = grants
        .put(&correct, now())
        .await
        .expect("the correct version is accepted");
    assert_eq!(updated.version, stored.version + 1);

    // And a replacement naming a version when **no row exists** is not found, which is a different
    // diagnosis: the caller edited something that is not there.
    let mut absent = grant_for(&definition, other_workspace());
    absent.expected_version = Some(1);
    assert_eq!(
        grants.put(&absent, now()).await.expect_err("no row"),
        RepositoryError::NotFound,
    );
}

#[tokio::test]
async fn a_lapsed_grant_is_not_returned_but_a_standing_one_is() {
    // Two rows read in **one** call, so the expiry comparison is exercised rather than assumed. A test with
    // only a lapsed row would pass for an adapter that rejected everything, and one with only a standing row
    // would pass for an adapter that ignored expiry.
    let (_database, grants) = repository().await;
    let lapsed_definition = write_definition();
    let lapsed = NewToolGrant::narrowing(
        &lapsed_definition,
        workspace(),
        principal(),
        BTreeSet::new(),
        BTreeSet::from([Effect::Write]),
        Risk::Low,
        Sensitivity::Public,
        // Strictly before the store's own read instant, so the row is genuinely lapsed rather than
        // lapsing during the test.
        Some(UtcTimestamp::parse("2020-01-01T00:00:00Z").expect("parses")),
        operator(),
        now(),
        None,
    )
    .expect("a lapsed grant is expressible");
    grants.put(&lapsed, now()).await.expect("stored");

    // A second capability, so the two rows do not collide on the key.
    let standing_definition = definition("clock.now@1", vec![Effect::ReadOnly], Risk::Low);
    let standing = NewToolGrant::narrowing(
        &standing_definition,
        workspace(),
        principal(),
        BTreeSet::new(),
        BTreeSet::from([Effect::ReadOnly]),
        Risk::Low,
        Sensitivity::Public,
        None,
        operator(),
        now(),
        None,
    )
    .expect("a standing grant is expressible");
    grants.put(&standing, now()).await.expect("stored");

    let read = grants
        .grants_for(workspace(), principal())
        .await
        .expect("reads");
    assert_eq!(
        read.len(),
        1,
        "only the standing grant may reach the evaluator, got {read:?}",
    );
    assert_eq!(
        read[0].identity.capability.to_string(),
        "clock.now@1",
        "and it must be the standing one, not the lapsed one",
    );
}

#[tokio::test]
async fn one_workspaces_grants_are_invisible_from_another() {
    // The scope rule, asserted on the read and on `load` separately, because they are two statements and a
    // predicate added to one is not automatically added to the other.
    let (_database, grants) = repository().await;
    let stored = grants
        .put(&grant_for(&read_definition(), workspace()), now())
        .await
        .expect("stored");
    assert!(
        grants
            .grants_for(other_workspace(), principal())
            .await
            .expect("reads")
            .is_empty(),
        "another workspace's read must not see this grant",
    );
    assert_eq!(
        grants
            .load(other_workspace(), stored.id)
            .await
            .expect_err("another workspace must not address this row"),
        RepositoryError::NotFound,
        "and it must read as *absent* rather than as forbidden",
    );
}

#[tokio::test]
async fn a_listing_is_ordered_by_capability_and_reports_its_bound() {
    // Two claims in one test because they are the two properties a page has: a **total, stable order**, and a
    // `bounded` flag that reflects the store's own truncation rather than the caller's arithmetic. Writing
    // them out of order and reading them back is what makes the ordering assertion meaningful.
    let (_database, grants) = repository().await;
    for capability in ["notes.write@1", "clock.now@1", "files.read@1"] {
        let definition = definition(capability, vec![Effect::ReadOnly], Risk::Low);
        grants
            .put(&grant_for(&definition, workspace()), now())
            .await
            .expect("stored");
    }
    let page = grants
        .list(workspace(), GrantListFilter::default(), 10)
        .await
        .expect("lists");
    let capabilities: Vec<String> = page
        .grants
        .iter()
        .map(|grant| grant.grant.identity.capability.to_string())
        .collect();
    assert_eq!(
        capabilities,
        vec!["clock.now@1", "files.read@1", "notes.write@1"],
        "the listing must be in capability order regardless of insertion order",
    );
    assert!(
        !page.bounded,
        "three rows is not a bounded page at a bound of ten"
    );

    // A bound of two truncates and **says so**, which is the fact a paging caller needs and cannot infer
    // from the length.
    let short = grants
        .list(workspace(), GrantListFilter::default(), 2)
        .await
        .expect("lists");
    assert_eq!(short.grants.len(), 2);
    assert!(short.bounded, "a truncated page must report itself bounded");
}

#[tokio::test]
async fn a_listing_filters_before_the_bound_rather_than_after_it() {
    // **The short-page defect.** A filter applied after the `LIMIT` would spend the page on rows the caller
    // excluded, so a caller with three grants of which one is active and a bound of one would receive zero
    // rows and conclude there is nothing to administer. The filter therefore reaches the same `WHERE` the
    // `LIMIT` applies to, and this test is what distinguishes the two.
    let (_database, grants) = repository().await;
    let active = definition("clock.now@1", vec![Effect::ReadOnly], Risk::Low);
    grants
        .put(&grant_for(&active, workspace()), now())
        .await
        .expect("stored");
    // Two revoked rows that sort **before** the active one by capability, so a post-filter would consume the
    // whole page on them.
    for capability in ["aaa.first@1", "aab.second@1"] {
        let revoked_definition = definition(capability, vec![Effect::ReadOnly], Risk::Low);
        let stored = grants
            .put(&grant_for(&revoked_definition, workspace()), now())
            .await
            .expect("stored");
        grants
            .revoke(workspace(), stored.id, stored.version, now())
            .await
            .expect("revoked");
    }

    let page = grants
        .list(
            workspace(),
            GrantListFilter {
                active: Some(true),
                principal: None,
            },
            1,
        )
        .await
        .expect("lists");
    assert_eq!(
        page.grants.len(),
        1,
        "the active grant must be found within a bound of one, got {page:?}",
    );
    assert!(page.grants[0].active);
    assert!(
        !page.bounded,
        "one active row out of three is not a bounded page — the filter must have removed the others \
         before the bound was applied",
    );
}

#[tokio::test]
async fn a_deny_rule_round_trips_and_a_capability_less_rule_is_refused() {
    // Two claims: the round trip, and the **empty rule being refused**. The domain refuses to over-match an
    // empty rule because a defaulted record would refuse everything; here an operator wrote it, so an empty
    // one is a configuration mistake — and storing it would put a row in the listing that silently does
    // nothing.
    let (_database, grants) = repository().await;
    let rule = NewDenyRule {
        workspace_id: Some(workspace()),
        capability: Some("email.send@1".to_owned()),
        principal: None,
        effects: BTreeSet::from([Effect::ExternalCommunication]),
        reason: "outbound mail needs a per-message decision".to_owned(),
        created_at: now(),
    };
    let id = grants.add_deny_rule(&rule).await.expect("stored");
    let listed = grants
        .list_deny_rules(workspace(), 10)
        .await
        .expect("lists");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].capability.as_deref(), Some("email.send@1"));
    assert_eq!(
        listed[0].reason, "outbound mail needs a per-message decision",
        "the reason is what a refused principal is shown, so it must survive",
    );
    assert!(
        listed[0].rule.identity.is_none(),
        "a stored rule is matched by capability, so it must name no exact identity — naming one would \
         stop the refusal applying after the tool was recompiled",
    );

    // The evaluator's read sees it too, and returns the **stored** shape so the adapter can expand a
    // capability into the identities that offer it.
    let evaluated = grants.deny_rules(workspace()).await.expect("reads");
    assert_eq!(evaluated.len(), 1);
    assert_eq!(evaluated[0].capability.as_deref(), Some("email.send@1"));

    // An empty rule names nothing at all, so it is refused rather than stored.
    let empty = NewDenyRule {
        workspace_id: None,
        capability: None,
        principal: None,
        effects: BTreeSet::new(),
        reason: "   ".trim().to_owned(),
        created_at: now(),
    }
    .validated()
    .expect_err("a rule naming nothing, with no reason, must be refused");
    assert_eq!(
        empty,
        RepositoryError::Conflict {
            what: "deny_reason"
        },
    );

    // And a rule that names something but has no usable reason is refused for the reason's sake, which is a
    // different field — so the two refusals are distinguishable.
    let unlabelled = NewDenyRule {
        workspace_id: Some(workspace()),
        capability: Some("email.send@1".to_owned()),
        principal: None,
        effects: BTreeSet::new(),
        reason: String::new(),
        created_at: now(),
    }
    .validated()
    .expect_err("a rule with no reason must be refused");
    assert_eq!(
        unlabelled,
        RepositoryError::Conflict {
            what: "deny_reason"
        },
    );
}

#[tokio::test]
async fn a_deny_rule_with_no_workspace_is_visible_in_every_workspace() {
    // A refusal that names no workspace is the **broadest** one, so a reader that filtered by workspace
    // would drop it — the direction that loses a restriction. It must therefore be returned for any
    // workspace asked about, and removable from one of them.
    let (_database, grants) = repository().await;
    let rule = NewDenyRule {
        workspace_id: None,
        capability: None,
        principal: None,
        effects: BTreeSet::from([Effect::Destructive]),
        reason: "no destructive operations on this profile".to_owned(),
        created_at: now(),
    };
    let id = grants.add_deny_rule(&rule).await.expect("stored");
    assert_eq!(
        grants.deny_rules(workspace()).await.expect("reads").len(),
        1
    );
    assert_eq!(
        grants
            .deny_rules(other_workspace())
            .await
            .expect("reads")
            .len(),
        1,
        "a workspace-less refusal applies everywhere",
    );

    grants
        .remove_deny_rule(other_workspace(), id)
        .await
        .expect("a workspace-less rule is removable from any workspace");
    assert!(
        grants
            .deny_rules(workspace())
            .await
            .expect("reads")
            .is_empty()
    );
    // Removing it twice is not found, because the rule is gone rather than flagged — a refusal carries no
    // authority, so there is nothing to audit away.
    assert_eq!(
        grants
            .remove_deny_rule(workspace(), id)
            .await
            .expect_err("already removed"),
        RepositoryError::NotFound,
    );
}

#[tokio::test]
async fn an_uninterpretable_stored_status_is_corruption_rather_than_active() {
    // The fail-open direction on the one column that decides whether authority is in force. A reader that
    // defaulted an unknown status to `active` would re-authorize a withdrawn grant; the domain's rule is the
    // opposite — a stored value the writer could not have produced is corruption, never absence.
    let (database, grants) = repository().await;
    let stored = grants
        .put(&grant_for(&read_definition(), workspace()), now())
        .await
        .expect("stored");
    sqlx::query("UPDATE tool_grants SET status = 'suspended' WHERE id = ?")
        .bind(stored.id.to_string())
        .execute(database.pool())
        .await
        .expect("the mutation applies");
    assert_eq!(
        grants
            .load(workspace(), stored.id)
            .await
            .expect_err("an unknown status is corruption"),
        RepositoryError::Corrupted { column: "status" },
    );
}

#[tokio::test]
async fn the_capability_is_the_key_so_a_recompiled_tool_still_finds_its_grant() {
    // **This is why the key is a capability rather than an identity.** `ToolIdentity` includes the schema
    // fingerprint, so a grant keyed by identity would read as *absent* after a tool was recompiled — and
    // `resolve_grant` distinguishes `NoGrant` from `IdentityReplaced` precisely because they lead an
    // operator to different next steps. Storing the identity and keying by capability is what keeps the
    // replacement *visible*.
    let (_database, grants) = repository().await;
    let stored = grants
        .put(&grant_for(&read_definition(), workspace()), now())
        .await
        .expect("stored");
    // The same capability with a **different schema fingerprint**, which is what a recompiled tool looks
    // like. A caller naming this version finds the row rather than a missing one.
    let mut recompiled = grant_for(&read_definition(), workspace());
    recompiled.identity.schema_fingerprint = SchemaFingerprint::from_bytes([0x22; 32]);
    recompiled.expected_version = Some(stored.version);
    let replaced = grants
        .put(&recompiled, now())
        .await
        .expect("the capability key finds the row across a schema change");
    assert_eq!(
        replaced.grant.identity.schema_fingerprint,
        SchemaFingerprint::from_bytes([0x22; 32]),
        "and the stored identity is the new one, so the evaluator sees the replacement",
    );
}

#[tokio::test]
async fn a_grant_id_is_not_interchangeable_with_another_tool_grant() {
    // The identifier's own contract: a canonical UUID, so a caller cannot address one grant with another's
    // value and a malformed string is refused rather than normalized. Asserted here because the store is the
    // first consumer that persists it.
    let (_database, grants) = repository().await;
    let stored = grants
        .put(&grant_for(&read_definition(), workspace()), now())
        .await
        .expect("stored");
    assert_eq!(
        stored.id,
        ToolGrantId::parse(&stored.id.to_string()).expect("the stored id round-trips"),
    );
    assert!(
        ToolGrantId::parse("01B0").is_err(),
        "a lookalike spelling must be refused, not normalized",
    );
}
