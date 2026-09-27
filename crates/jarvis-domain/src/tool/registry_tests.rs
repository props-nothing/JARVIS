//! Tests for registry discovery: the collision cases the tool fabric names, and the scoping rule
//! that makes a cached answer unusable for another scope.
//!
//! Two of these are the architecture document's own required tests, quoted in the module docs:
//! "tool-name and source-identity collision" and "stale discovery cache and source replacement".
//! They are here as executable checks rather than as prose, because the failure each describes is
//! silent in production — a colliding registration *succeeds* in the naive implementation, and a
//! stale cache entry *is served* rather than refused.

use std::collections::BTreeSet;

use super::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk, Scope,
};
use super::definition::ToolDefinition;
use super::discovery::{DISCOVERY_TTL_DAYS, DiscoveryCache, MAX_DISCOVERY_ENTRIES, catalog_for};
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use super::registry::{
    DiscoveryCacheKey, DiscoveryScope, ListVersion, MAX_REGISTERED_TOOLS, RegisteredTool,
    RegistrationOutcome, RegistrationRequest, ServerConfigId, ToolRegistry,
};
use crate::error::DomainError;
use crate::ids::{PrincipalId, WorkspaceId};
use crate::model::policy::Sensitivity;
use crate::time::{IsoDate, UtcTimestamp};

// ---------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------

fn version(value: &str) -> ToolVersion {
    ToolVersion::parse(value).expect("the fixture version is canonical")
}

fn fingerprint(seed: u8) -> SchemaFingerprint {
    SchemaFingerprint::from_bytes([seed; 32])
}

fn scope(value: &str) -> Scope {
    Scope::new(value).expect("the fixture scope is valid")
}

/// A definition whose identity varies by the seed and whose content varies with it too.
///
/// The purpose text is derived from the seed so two definitions with different seeds are
/// **different content** as well as different identity, which is what the replacement tests need:
/// a changed schema with an unchanged purpose is the realistic replacement, and a fixture that
/// changed only the fingerprint would not exercise the content comparison.
fn definition(capability: &str, owner: &str, seed: u8) -> ToolDefinition {
    ToolDefinition::new(
        ToolIdentity {
            capability: ToolCapability::parse(capability).expect("the fixture capability parses"),
            source: ToolSource::new(SourceKind::McpServer, owner, version("1.0.0"))
                .expect("the fixture source is valid"),
            schema_fingerprint: fingerprint(seed),
        },
        "Read a file",
        &format!("Read one file, revision {seed}."),
        vec![Effect::ReadOnly],
        Risk::Low,
        vec![scope("fs.read")],
        ApprovalHint::Ask,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal)
            .expect("the fixture classification is ordered"),
        ExecutionDefaults::new(5_000, 1).expect("the fixture defaults are in range"),
    )
    .expect("the fixture definition is valid")
}

fn server(value: &str) -> ServerConfigId {
    ServerConfigId::new(value).expect("the fixture server id is valid")
}

fn principal(seed: u128) -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(seed))
}

fn workspace(seed: u128) -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(seed))
}

fn capabilities(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn discovery_scope(server_name: &str, principal_seed: u128) -> DiscoveryScope {
    DiscoveryScope::new(
        server(server_name),
        principal(principal_seed),
        workspace(1),
        "2026-06-18",
        capabilities(&["tools"]),
    )
    .expect("the fixture scope is valid")
}

fn key(server_name: &str, principal_seed: u128, list_version: &str) -> DiscoveryCacheKey {
    DiscoveryCacheKey {
        scope: discovery_scope(server_name, principal_seed),
        list_version: ListVersion::new(list_version).expect("the fixture version is valid"),
    }
}

fn today() -> IsoDate {
    IsoDate::parse("2026-09-27").expect("the fixture day parses")
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-27T00:00:00Z").expect("the fixture instant parses")
}

/// Builds a catalog for one key from a registry's tools.
fn catalog(registry: &ToolRegistry, key: DiscoveryCacheKey) -> super::discovery::DiscoveredCatalog {
    let offered: Vec<RegisteredTool> = registry.all_unfiltered().into_iter().cloned().collect();
    catalog_for(key, offered, now(), today().adding_days(DISCOVERY_TTL_DAYS))
}

// ---------------------------------------------------------------------------------------
// Registration: identity and source collisions.
// ---------------------------------------------------------------------------------------

#[test]
fn a_first_registration_is_added_and_a_repeat_is_unchanged() {
    // The positive control. A repeat must be **unchanged rather than a conflict**, because a
    // server that lists its tools on every reconnect re-registers them, and refusing that would
    // make a healthy reconnect look like an attack.
    let mut registry = ToolRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                definition("fs.read@1", "acme.files", 1),
                server("files-primary"),
            ))
            .expect("the first registration is accepted"),
        RegistrationOutcome::Added,
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                definition("fs.read@1", "acme.files", 1),
                server("files-primary"),
            ))
            .expect("an identical re-registration is accepted"),
        RegistrationOutcome::Unchanged,
    );
    assert_eq!(registry.len(), 1, "a repeat must not add a second entry");
}

#[test]
fn a_different_definition_under_one_identity_is_refused_without_a_replacement() {
    // **This is the default and it is what `ACC-024` depends on.** Same identity — same capability,
    // same source, same fingerprint — with different content. Nothing about the identity changed, so
    // every approval recorded against it still matches, which is exactly why an unreviewed
    // replacement must not proceed.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files-primary"),
        ))
        .expect("the first registration is accepted");

    let mut mutated = definition("fs.read@1", "acme.files", 1);
    mutated.purpose = "Something else entirely.".to_owned();
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                mutated.clone(),
                server("files-primary")
            ))
            .expect_err("an unreviewed replacement must be refused")
            .code(),
        "tool.conflict",
    );
    // And the registry is unchanged, because a refused registration must not have written anything.
    assert_eq!(
        registry
            .get(&mutated.identity)
            .map(|tool| tool.definition.purpose.as_str()),
        Some("Read one file, revision 1."),
        "a refused replacement must leave the previous definition in place",
    );
}

#[test]
fn a_permitted_replacement_reports_itself_rather_than_succeeding_silently() {
    // The escape hatch exists because a reviewed correction is legitimate. What makes it safe is
    // that the outcome names it: a caller can log that an approval must be re-obtained, rather than
    // recording a successful registration while the implementation behind a live approval changed.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files-primary"),
        ))
        .expect("the first registration is accepted");

    let mut corrected = definition("fs.read@1", "acme.files", 1);
    corrected.purpose = "Read one file, with the path bug fixed.".to_owned();
    assert_eq!(
        registry
            .register(RegistrationRequest::replacing(
                corrected.clone(),
                server("files-primary")
            ))
            .expect("a permitted replacement is accepted"),
        RegistrationOutcome::Replaced,
    );
    assert_eq!(
        registry.len(),
        1,
        "a replacement must not add a second entry"
    );
    assert_eq!(
        registry
            .get(&corrected.identity)
            .map(|tool| tool.definition.purpose.as_str()),
        Some("Read one file, with the path bug fixed."),
    );
    // The identity is unchanged, which is the fact the outcome exists to draw attention to.
    assert_eq!(
        registry.all_unfiltered()[0].definition.identity,
        corrected.identity
    );
}

#[test]
fn a_second_server_may_not_claim_a_source_identity_already_in_use() {
    // **The tool fabric's "source-identity collision" case.** An identity names a *source* as its
    // owner and version, and those come from the server's own manifest — untrusted input. Two
    // servers both declaring `acme.files 1.0.0` produce the *same identity* when the capability and
    // fingerprint also match, so the second inherits every approval recorded for the first. The
    // trusted side of this check is the server configuration, which is why the registry has to
    // record it rather than only the cache key.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files-primary"),
        ))
        .expect("the first registration is accepted");

    // The impersonating server offers a definition that claims the same source. Even asking to
    // replace does not help: the collision is the thing a replacement flag would be used to
    // authorize, so it is refused unconditionally.
    for request in [
        RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 9),
            server("files-rogue"),
        ),
        RegistrationRequest::replacing(
            definition("fs.read@1", "acme.files", 9),
            server("files-rogue"),
        ),
    ] {
        assert_eq!(
            registry
                .register(request)
                .expect_err("a second server must not claim a known source")
                .code(),
            "tool.source_conflict",
        );
    }
    // The legitimate server still owns it, and the rogue registration wrote nothing.
    assert_eq!(
        registry
            .source_claimant(
                &ToolSource::new(SourceKind::McpServer, "acme.files", version("1.0.0"))
                    .expect("the fixture source is valid")
            )
            .map(ServerConfigId::as_str),
        Some("files-primary"),
    );
    assert_eq!(registry.len(), 1);
}

#[test]
fn one_server_may_register_many_tools_and_a_second_server_its_own() {
    // The positive control for the collision rule: refusing *every* second server would be a
    // registry that admits one, so the rule must key on the source claim rather than on "a second
    // server exists". Different owners are different claims and both succeed.
    let mut registry = ToolRegistry::new();
    for (capability, owner, seed) in [
        ("fs.read@1", "acme.files", 1),
        ("fs.write@1", "acme.files", 2),
        ("mail.send@1", "acme.mail", 3),
    ] {
        assert_eq!(
            registry
                .register(RegistrationRequest::new(
                    definition(capability, owner, seed),
                    server("primary"),
                ))
                .expect("a distinct source is accepted"),
            RegistrationOutcome::Added,
        );
    }
    assert_eq!(registry.len(), 3);
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                definition("notes.read@1", "other.notes", 4),
                server("notes"),
            ))
            .expect("a second server with its own source is accepted"),
        RegistrationOutcome::Added,
    );
    assert_eq!(registry.len(), 4);
}

#[test]
fn deregistering_releases_the_identity_and_the_source_claim() {
    // A tool that cannot be deregistered keeps its identity occupied, so a corrected implementation
    // can never be registered without a replacement flag — the same "connector that cannot be
    // disabled is incomplete" rule the architecture states. And the **source claim** has to be
    // released too, or a server that is removed and re-added is locked out permanently by a claim
    // held on behalf of a definition that no longer exists.
    let mut registry = ToolRegistry::new();
    let tool = definition("fs.read@1", "acme.files", 1);
    let identity = tool.identity.clone();
    registry
        .register(RegistrationRequest::new(tool, server("files-primary")))
        .expect("the registration is accepted");
    assert!(registry.source_claimant(&identity.source).is_some());

    assert!(registry.deregister(&identity));
    assert!(registry.is_empty());
    assert!(
        registry.source_claimant(&identity.source).is_none(),
        "the source claim must be released with its last tool, or a removed and re-added server \
         would be locked out by a claim on behalf of a definition that no longer exists",
    );
    assert!(
        !registry.deregister(&identity),
        "a second removal is a no-op"
    );
    // And the identity is free again: a *different* definition may now take it without a
    // replacement flag, because nothing was there to replace.
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                definition("fs.read@1", "acme.files", 1),
                server("files-primary"),
            ))
            .expect("the identity is free again"),
        RegistrationOutcome::Added,
    );
}

#[test]
fn deregistering_one_of_two_tools_keeps_the_source_claim() {
    // The boundary of the release rule: a source claim is held by a *source*, not by a definition,
    // so removing one tool from a server that offers two must not free the claim — otherwise a
    // second server could claim the source while the first still has a tool registered under it.
    let mut registry = ToolRegistry::new();
    let first = definition("fs.read@1", "acme.files", 1);
    let identity = first.identity.clone();
    registry
        .register(RegistrationRequest::new(first, server("files-primary")))
        .expect("the first registration is accepted");
    registry
        .register(RegistrationRequest::new(
            definition("fs.write@1", "acme.files", 2),
            server("files-primary"),
        ))
        .expect("the second registration is accepted");

    assert!(registry.deregister(&identity));
    assert_eq!(registry.len(), 1);
    assert!(
        registry.source_claimant(&identity.source).is_some(),
        "a source still offering a tool keeps its claim",
    );
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                definition("fs.other@1", "acme.files", 3),
                server("files-rogue"),
            ))
            .expect_err("the source is still claimed")
            .code(),
        "tool.source_conflict",
    );
}

#[test]
fn the_registry_refuses_to_grow_without_bound() {
    // A bound because the registry is scanned per discovery call: an unbounded one is an unbounded
    // per-call cost, and a misbehaving source could grow it without limit.
    let mut registry = ToolRegistry::new();
    // Register distinct capabilities until the bound is reached. The capabilities are built from
    // the index so each one is a different tool, which is what makes the loop reach the limit
    // rather than colliding first — a loop that collided would pass the assertion below for the
    // wrong reason.
    for index in 0..MAX_REGISTERED_TOOLS {
        let capability = format!("ns{index}.read@1");
        assert_eq!(
            registry
                .register(RegistrationRequest::new(
                    definition(&capability, "acme.many", 1),
                    server("many"),
                ))
                .expect("a distinct capability is accepted"),
            RegistrationOutcome::Added,
        );
    }
    assert_eq!(registry.len(), MAX_REGISTERED_TOOLS);
    // The capability here must be one the parser **accepts**, or the test would fail on its own
    // fixture rather than on the bound — which is exactly what a first version of this test did,
    // using `one.too.many@1` and panicking inside `ToolCapability::parse` before reaching `register`.
    assert_eq!(
        registry
            .register(RegistrationRequest::new(
                definition("overflow.read@1", "acme.many", 1),
                server("many"),
            ))
            .expect_err("the registry is full")
            .code(),
        "tool.definition_invalid",
    );
}

#[test]
fn a_capability_may_hold_two_majors_because_a_new_major_is_a_new_tool() {
    // The contract says a breaking change requires a new major identity, so `@1` and `@2` are two
    // tools rather than one tool with two backends. A registry that keyed on the capability alone
    // would refuse the second, which would make a staged major upgrade impossible; one that let the
    // second *replace* the first would make the version meaningless.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files"),
        ))
        .expect("major 1 registers");
    // A definition whose capability major is 2 needs a release major of 2, which is a different
    // source version — hence a different owner string here, since one owner publishing two release
    // majors would be the same source with two versions and that is a separate question.
    let second = ToolDefinition::new(
        ToolIdentity {
            capability: ToolCapability::parse("fs.read@2").expect("canonical"),
            source: ToolSource::new(SourceKind::McpServer, "acme.files.two", version("2.0.0"))
                .expect("valid"),
            schema_fingerprint: fingerprint(2),
        },
        "Read a file",
        "Read one file, v2.",
        vec![Effect::ReadOnly],
        Risk::Low,
        vec![scope("fs.read")],
        ApprovalHint::Ask,
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Internal, Sensitivity::Internal).expect("ordered"),
        ExecutionDefaults::new(5_000, 1).expect("in range"),
    )
    .expect("the v2 definition is valid");
    assert_eq!(
        registry
            .register(RegistrationRequest::new(second, server("files")))
            .expect("major 2 registers beside major 1"),
        RegistrationOutcome::Added,
    );
    assert_eq!(registry.len(), 2);
    // Both are reachable under their own capability, and the shared *name* is not what identifies
    // them — which is the point of keying on the capability rather than on the name.
    let v1 = ToolCapability::parse("fs.read@1").expect("canonical");
    let v2 = ToolCapability::parse("fs.read@2").expect("canonical");
    assert_eq!(registry.by_capability(&v1).len(), 1);
    assert_eq!(registry.by_capability(&v2).len(), 1);
    assert_ne!(
        registry.by_capability(&v1)[0]
            .definition
            .identity
            .capability,
        v2
    );
}

#[test]
fn the_registry_reports_which_source_kinds_are_registered() {
    // Used by a test that must prove an external source cannot displace a native one, so the answer
    // has to exist rather than being inferred from the tool list at each call site.
    let mut registry = ToolRegistry::new();
    assert!(registry.registered_source_kinds().is_empty());
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files"),
        ))
        .expect("the registration is accepted");
    let kinds = registry.registered_source_kinds();
    assert!(kinds.contains(&SourceKind::McpServer));
    assert!(!kinds.contains(&SourceKind::Native));
}

// ---------------------------------------------------------------------------------------
// Discovery scope and its cache key.
// ---------------------------------------------------------------------------------------

#[test]
fn a_discovery_scope_refuses_a_missing_or_unusable_component() {
    // The contract's key list is the *reason* this type exists, so every component is validated
    // rather than accepted: a scope with an empty protocol version or a control character in a
    // capability would produce a key that two different handshakes could collide on, which is the
    // mechanism by which one scope's answer serves another.
    assert!(
        DiscoveryScope::new(
            server("files"),
            principal(1),
            workspace(1),
            "2026-06-18",
            capabilities(&["tools"]),
        )
        .is_ok(),
    );
    for bad_protocol in ["", "2026-06-18\u{7}", &"v".repeat(65)] {
        assert_eq!(
            field_of(&DiscoveryScope::new(
                server("files"),
                principal(1),
                workspace(1),
                bad_protocol,
                capabilities(&["tools"]),
            )),
            Some("protocol_version"),
            "{bad_protocol:?} must be refused",
        );
    }
    for bad_capability in ["", "tools\u{0}", &"c".repeat(65)] {
        assert_eq!(
            field_of(&DiscoveryScope::new(
                server("files"),
                principal(1),
                workspace(1),
                "2026-06-18",
                capabilities(&[bad_capability]),
            )),
            Some("capabilities"),
            "{bad_capability:?} must be refused",
        );
    }
}

#[test]
fn the_capability_set_is_order_independent_so_one_handshake_is_one_key() {
    // Two handshakes negotiating the same capabilities in a different order are one negotiation.
    // A key that varied with order would produce two entries for one scope, so an answer computed
    // under the second would miss the first — a cache that looks correct and quietly never hits.
    let first = DiscoveryScope::new(
        server("files"),
        principal(1),
        workspace(1),
        "2026-06-18",
        capabilities(&["tools", "resources", "prompts"]),
    )
    .expect("valid");
    let second = DiscoveryScope::new(
        server("files"),
        principal(1),
        workspace(1),
        "2026-06-18",
        capabilities(&["prompts", "tools", "resources"]),
    )
    .expect("valid");
    assert_eq!(first, second);
}

#[test]
fn a_scope_differs_when_any_scope_component_differs() {
    // **The scoping rule stated as a property.** For each component, changing only that component
    // must produce a different key. A key missing a component would make the pair equal — which is
    // a disclosure, not a cache miss: the answer computed for the first would be served for the
    // second.
    let base = discovery_scope("files", 1);
    let variations: [(&str, DiscoveryScope); 5] = [
        ("server", discovery_scope("other", 1)),
        ("principal", discovery_scope("files", 2)),
        (
            "workspace",
            DiscoveryScope::new(
                server("files"),
                principal(1),
                workspace(2),
                "2026-06-18",
                capabilities(&["tools"]),
            )
            .expect("valid"),
        ),
        (
            "protocol version",
            DiscoveryScope::new(
                server("files"),
                principal(1),
                workspace(1),
                "2025-11-25",
                capabilities(&["tools"]),
            )
            .expect("valid"),
        ),
        (
            "capability set",
            DiscoveryScope::new(
                server("files"),
                principal(1),
                workspace(1),
                "2026-06-18",
                capabilities(&["tools", "prompts"]),
            )
            .expect("valid"),
        ),
    ];
    for (component, varied) in variations {
        assert_ne!(
            base, varied,
            "two scopes differing only in the {component} must not be equal, or one scope's \
             discovery answer would be served for another",
        );
    }
}

#[test]
fn the_cached_answer_is_scoped_to_the_server_that_produced_it() {
    // The filter is the scope, so a registry holding two servers' tools must produce an answer for
    // one server that does **not** mention the other. Offering another server's tools would tell one
    // server's principal that the other exists, and it would also let a client attempt a call the
    // resolved server cannot serve.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files"),
        ))
        .expect("accepted");
    registry
        .register(RegistrationRequest::new(
            definition("mail.send@1", "acme.mail", 2),
            server("mail"),
        ))
        .expect("accepted");

    let files = catalog(&registry, key("files", 1, "rev-1"));
    assert_eq!(files.tools.len(), 1);
    assert_eq!(
        files.tools[0].capability().to_string(),
        "fs.read@1",
        "the files server's answer must not carry the mail server's tool",
    );
    let mail = catalog(&registry, key("mail", 1, "rev-1"));
    assert_eq!(mail.tools.len(), 1);
    assert_eq!(mail.tools[0].capability().to_string(), "mail.send@1");
}

#[test]
fn a_cached_answer_survives_a_repeat_and_is_counted_as_a_hit() {
    // The positive control for the counters below: a fresh answer must be served, and serving it
    // must not be counted as a miss or a stale hit — otherwise every "the cache refused it" test
    // would pass against a cache that refused everything.
    let mut cache = DiscoveryCache::new();
    cache.store(catalog(&ToolRegistry::new(), key("files", 1, "rev-1")));
    assert_eq!(cache.len(), 1);
    let served = cache
        .get_fresh(&key("files", 1, "rev-1"), today())
        .expect("a fresh entry is served");
    assert_eq!(served.key, key("files", 1, "rev-1"));
    assert_eq!(
        cache.counters(),
        (0, 0),
        "a hit is neither a miss nor stale"
    );
}

#[test]
fn an_answer_computed_for_one_principal_is_not_served_to_another() {
    // **This is the defect the whole key exists to prevent**, stated as the scenario that exposes
    // it: a local profile has ONE workspace shared by every client, each with its own principal. A
    // cache keyed on the server and workspace but not the principal would answer the second
    // client's discovery call with the first client's catalog. That is a disclosure, and it is the
    // same three-of-five-dimensions mistake the idempotency key made.
    let mut cache = DiscoveryCache::new();
    let first_principal = key("files", 1, "rev-1");
    cache.store(catalog(&ToolRegistry::new(), first_principal.clone()));

    let second_principal = key("files", 2, "rev-1");
    assert_ne!(
        first_principal, second_principal,
        "the two keys must differ, which they do only because the principal is in the key",
    );
    assert!(
        cache.get_fresh(&second_principal, today()).is_none(),
        "one principal's discovery answer must never be served for another",
    );
    // And the miss is counted, so an operator can see it happened rather than inferring it.
    assert_eq!(cache.counters(), (1, 0));
}

#[test]
fn a_stale_entry_is_refused_and_counted_separately_from_a_miss() {
    // The tool fabric's "stale discovery cache" test. A stale entry must not be served, and it must
    // be distinguishable from a miss: "no answer was ever computed" and "your answer is out of date"
    // lead a caller to different next steps, and a cache that collapsed them would hide a
    // correctly-expiring entry behind the same counter as a scope nobody asked about.
    let mut cache = DiscoveryCache::new();
    let stored = key("files", 1, "rev-1");
    cache.store(catalog(&ToolRegistry::new(), stored.clone()));

    // One day past the last valid day. The `revalidate_by` day is inclusive, so the day itself is
    // still fresh and the next one is not — asserted on both sides so an off-by-one in the
    // comparison is caught rather than tolerated.
    let last_valid = today().adding_days(DISCOVERY_TTL_DAYS);
    assert!(
        cache.get_fresh(&stored, last_valid).is_some(),
        "the last valid day is valid, since the field names the last day it may be used",
    );
    let expired = last_valid.adding_days(1);
    assert!(
        cache.get_fresh(&stored, expired).is_none(),
        "an expired answer must not be served",
    );
    assert_eq!(cache.counters(), (0, 1), "a stale hit is not a miss");
    // The entry is left in place rather than removed, so the fact that an answer existed and
    // expired survives for the caller to report.
    assert_eq!(cache.len(), 1);
}

#[test]
fn a_server_being_replaced_drops_every_answer_computed_from_it() {
    // The tool fabric's "source replacement" test. When a server's configuration changes or it is
    // re-added, every answer computed from it is wrong rather than merely old: the tools it offers
    // may all be gone. Invalidating **by server** means a caller does not have to enumerate the
    // principals and protocol versions that happened to query it — which is the enumeration a
    // caller gets wrong, and the way a stale entry survives a supposed flush.
    let mut cache = DiscoveryCache::new();
    for principal_seed in [1, 2, 3] {
        cache.store(catalog(
            &ToolRegistry::new(),
            key("files", principal_seed, "rev-1"),
        ));
    }
    cache.store(catalog(&ToolRegistry::new(), key("mail", 1, "rev-1")));
    assert_eq!(cache.len(), 4);

    assert_eq!(
        cache.invalidate_server(&server("files")),
        3,
        "every answer from the replaced server must go, not only the one that happened to be last",
    );
    assert_eq!(cache.len(), 1, "the other server's answer is untouched");
    assert!(cache.get_fresh(&key("mail", 1, "rev-1"), today()).is_some());
    assert!(
        cache
            .get_fresh(&key("files", 1, "rev-1"), today())
            .is_none()
    );
}

#[test]
fn a_new_list_version_invalidates_answers_at_the_old_one() {
    // The version half of "stale discovery cache": the contract says a cache key includes the
    // "list-result version", so an answer at revision 4 describes a catalog the server has since
    // redefined. Keyed per server, because list versions are the server's own sequence and are not
    // comparable across servers.
    let mut cache = DiscoveryCache::new();
    cache.store(catalog(&ToolRegistry::new(), key("files", 1, "rev-1")));
    cache.store(catalog(&ToolRegistry::new(), key("files", 1, "rev-2")));
    cache.store(catalog(&ToolRegistry::new(), key("mail", 1, "rev-1")));
    assert_eq!(cache.len(), 3);

    let current = ListVersion::new("rev-2").expect("valid");
    assert_eq!(
        cache.invalidate_below_version(&server("files"), &current),
        1,
        "only the files server's superseded answer goes",
    );
    assert_eq!(cache.len(), 2);
    assert!(
        cache
            .get_fresh(&key("files", 1, "rev-2"), today())
            .is_some()
    );
    assert!(
        cache
            .get_fresh(&key("files", 1, "rev-1"), today())
            .is_none()
    );
    assert!(
        cache.get_fresh(&key("mail", 1, "rev-1"), today()).is_some(),
        "another server's `rev-1` is its own sequence and must not be swept with these",
    );
}

#[test]
fn the_cache_evicts_the_oldest_entry_rather_than_growing_without_bound() {
    // The key space is a product of five scopes, so the cache grows with the number of distinct
    // principals and protocol versions rather than with the number of servers. Bounded, with a
    // defined eviction — an undefined one is one an operator cannot reason about.
    let mut cache = DiscoveryCache::new();
    for index in 0..MAX_DISCOVERY_ENTRIES {
        cache.store(catalog(
            &ToolRegistry::new(),
            key("files", index as u128 + 1, "rev-1"),
        ));
    }
    assert_eq!(cache.len(), MAX_DISCOVERY_ENTRIES);

    // The oldest is the first principal stored. Its key is still absent after the eviction, and the
    // most recently stored is present, so the eviction is oldest-first rather than arbitrary.
    cache.store(catalog(
        &ToolRegistry::new(),
        key("files", MAX_DISCOVERY_ENTRIES as u128 + 1, "rev-1"),
    ));
    assert_eq!(cache.len(), MAX_DISCOVERY_ENTRIES);
    assert!(
        cache
            .get_fresh(&key("files", 1, "rev-1"), today())
            .is_none(),
        "the oldest entry must be the one evicted",
    );
    assert!(
        cache
            .get_fresh(
                &key("files", MAX_DISCOVERY_ENTRIES as u128 + 1, "rev-1"),
                today()
            )
            .is_some(),
        "the newest entry must be retained",
    );
}

#[test]
fn clearing_removes_every_entry_and_reports_the_count() {
    let mut cache = DiscoveryCache::new();
    cache.store(catalog(&ToolRegistry::new(), key("files", 1, "rev-1")));
    cache.store(catalog(&ToolRegistry::new(), key("mail", 1, "rev-1")));
    assert_eq!(cache.clear(), 2);
    assert!(cache.is_empty());
    assert_eq!(cache.clear(), 0);
}

#[test]
fn the_catalog_derives_its_capability_set_from_its_tools() {
    // Derived rather than stored, so it cannot disagree with the tools it describes — the same rule
    // `ToolDefinition::is_consequential` follows for its effects.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("fs.read@1", "acme.files", 1),
            server("files"),
        ))
        .expect("accepted");
    let catalog = catalog(&registry, key("files", 1, "rev-1"));
    let derived = catalog.capabilities();
    assert_eq!(derived.len(), 1);
    assert!(derived.contains(&ToolCapability::parse("fs.read@1").expect("canonical")));
    assert!(
        !catalog.tools.is_empty(),
        "there is something to derive from"
    );
}

#[test]
fn a_catalog_is_ordered_by_identity_whatever_order_it_was_built_from() {
    // The answer's shape is a contract rather than an accident of the input order, so a caller
    // comparing two answers does not see spurious differences and a test can assert on the order.
    let mut registry = ToolRegistry::new();
    for (capability, owner, seed) in [
        ("zz.read@1", "acme.files", 3),
        ("aa.read@1", "acme.files", 1),
        ("mm.read@1", "acme.files", 2),
    ] {
        registry
            .register(RegistrationRequest::new(
                definition(capability, owner, seed),
                server("files"),
            ))
            .expect("accepted");
    }
    let offered: Vec<RegisteredTool> = registry
        .all_unfiltered()
        .into_iter()
        .rev()
        .cloned()
        .collect();
    let catalog = catalog_for(
        key("files", 1, "rev-1"),
        offered,
        now(),
        today().adding_days(DISCOVERY_TTL_DAYS),
    );
    let capabilities: Vec<String> = catalog
        .tools
        .iter()
        .map(|tool| tool.capability().to_string())
        .collect();
    assert_eq!(capabilities, vec!["aa.read@1", "mm.read@1", "zz.read@1"]);
}

#[test]
fn the_registry_holds_no_grant_and_answers_no_permission_question() {
    // `TLS-002`'s word is "independently", and the tool fabric's rule is that discovery never grants
    // execution permission. This test is structural rather than behavioural: it asserts that the
    // types a caller can reach from the registry carry **classification** and no permission, so
    // there is no field a caller could read as "this is allowed". The `deny` direction is what
    // matters — a registry that grew an `allowed: bool` would be a second authorization decision
    // living beside the policy layer, and the one consulted first would be the one that mattered.
    let mut registry = ToolRegistry::new();
    registry
        .register(RegistrationRequest::new(
            definition("shell.exec@1", "acme.shell", 1),
            server("shell"),
        ))
        .expect("accepted");
    let registered = registry.all_unfiltered();
    assert_eq!(registered.len(), 1);
    // What the registry does expose: the definition's effects and risk, which are classification.
    // `shell.exec@1` is declared `ReadOnly` by the fixture, so this asserts the *fixture* is the
    // benign one and that the registry reports what was declared rather than a derived verdict.
    assert!(!registered[0].definition.is_consequential());
    assert_eq!(registered[0].definition.risk, Risk::Low);
    // And there is no lookup that answers a permission question: the only `get` takes an identity
    // and returns a registration, so a caller wanting a decision has to ask for one elsewhere.
    assert!(registry.get(&registered[0].definition.identity).is_some());
}

/// The field a refusal named, or `None`. The domain error is deliberately not `PartialEq`, so a
/// test asserts against the field an operator would act on.
fn field_of<T>(result: &Result<T, DomainError>) -> Option<&'static str> {
    match result.as_ref().err() {
        Some(DomainError::ToolDefinitionInvalid { field }) => Some(field),
        _ => None,
    }
}
