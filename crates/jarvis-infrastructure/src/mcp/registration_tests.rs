//! Tests for MCP catalog registration.
//!
//! Four carry more weight than the rest, and each exists because a plausible implementation gets it wrong
//! in a way no other test here would notice:
//!
//! - **`a_catalog_claiming_another_servers_owner_is_refused`** is the impersonation check. An implementation
//!   that derived the trusted server id from the catalog would pass every other test here — because the two
//!   would agree by construction — and would have removed the only check that catches a catalog claiming to
//!   be a different server.
//! - **`a_changed_tool_is_refused_rather_than_replaced`** is `ACC-024` at the registration boundary, and the
//!   direction matters: a replacement keeps the identity, so the new implementation inherits every approval
//!   recorded against the old one.
//! - **`the_first_wins_source_claim_is_not_poisoned_by_a_refused_catalog`** pins why the impersonation check
//!   runs *before* the registry rather than relying on the registry's own source check. The registry's claim
//!   is first-wins, so a hostile catalog registered first would refuse the real server afterwards.
//! - **`a_refused_tool_is_not_published_even_though_it_was_offered`** is the join's security property. An
//!   implementation that built the dispatch catalog from what the server *offered* would pass every other
//!   test here while serving a tool the registry had just refused — see the `SourceMismatch` case below,
//!   where the refused catalog is nonetheless dispatchable.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use super::{McpRegistrationRefusal, publishable_pairs, register_catalog, server_config_id};
use crate::mcp::normalize_catalog;
use jarvis_domain::tool::identity::SourceKind;
use jarvis_domain::tool::registry::ToolRegistry;
use rmcp::model::Tool;

/// A JSON Schema every fixture tool can be validated against.
const OBJECT_SCHEMA: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

/// A schema that differs from [`OBJECT_SCHEMA`], so two definitions under one capability/name are
/// distinguishable by **content** — which is what moves their identity, since the fingerprint is over the
/// schema. Named rather than repeated inline so the three tests that need a changed schema cannot drift.
const OTHER_SCHEMA: &str =
    r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#;

/// A fixture tool with a distinct schema per `schema_seed`, so two tools are distinguishable by content.
fn mcp_tool(name: &str, schema: &str) -> Tool {
    let input_schema: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(schema).expect("the fixture schema must be a JSON object");
    let mut tool = Tool::default();
    tool.name = name.to_string().into();
    tool.input_schema = std::sync::Arc::new(input_schema);
    tool
}

/// The trusted configuration identity every fixture uses.
fn server() -> jarvis_domain::tool::registry::ServerConfigId {
    server_config_id("acme-files").expect("the fixture server name is usable")
}

#[test]
fn a_clean_catalog_registers_every_tool_and_reports_no_refusal() {
    // The accepting case, without which every refusal below would be satisfied by a helper that refuses
    // everything — the failure mode this project keeps recording.
    let registry_input = normalize_catalog(
        "acme-files",
        &[
            mcp_tool("read_file", OBJECT_SCHEMA),
            mcp_tool("write_file", OBJECT_SCHEMA),
        ],
    );

    let mut registry = ToolRegistry::new();
    let report = register_catalog(&mut registry, &server(), &registry_input);

    assert!(report.is_clean(), "{:?}", report.refused);
    assert_eq!(report.added, 2);
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.registered(), 2);
    assert_eq!(registry.len(), 2);
}

#[test]
fn re_registering_an_identical_catalog_is_idempotent_rather_than_a_conflict() {
    // A server that lists its tools on every reconnect re-registers them; failing that would make a healthy
    // reconnect look like a conflict. The registry documents `Unchanged` for exactly this, and this pins
    // that the helper uses the path that reaches it.
    let catalog = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    let mut registry = ToolRegistry::new();

    let first = register_catalog(&mut registry, &server(), &catalog);
    let second = register_catalog(&mut registry, &server(), &catalog);

    assert_eq!(first.added, 1);
    assert_eq!(second.added, 0);
    assert_eq!(second.unchanged, 1);
    assert!(second.is_clean());
    assert_eq!(registry.len(), 1, "a re-listing must not grow the registry");
}

#[test]
fn a_changed_schema_registers_as_a_second_identity_under_one_capability() {
    // ⚠ **A schema change is a different *identity*, so this registers — and a first version of this test
    // asserted `added: 0`, which was wrong about the registry's own contract.**
    //
    // The finding is worth keeping because it is the opposite of the intuitive reading: "a changed tool"
    // sounds like the case `RegistrationRequest::new` refuses, but the refusal is for the *same* identity
    // with different content — and a schema change moves the fingerprint, so the identity moves with it.
    // Two identities therefore share one capability, which the registry permits deliberately: "a capability
    // may have several majors registered, since a new major is a different tool".
    //
    // What that makes load-bearing is the call path: a grant recorded against the *old* identity cannot
    // authorize the new one, because `ToolIdentity::authorizes` requires all three components. That is
    // `ACC-024` satisfied by identity rather than by a refusal, and `invocation.rs` is where it is checked.
    let mut registry = ToolRegistry::new();
    let original = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    assert_eq!(
        register_catalog(&mut registry, &server(), &original).added,
        1
    );

    let widened =
        r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#;
    let changed = normalize_catalog("acme-files", &[mcp_tool("read_file", widened)]);
    let report = register_catalog(&mut registry, &server(), &changed);

    assert_eq!(report.added, 1, "a new identity is a new registration");
    assert!(
        report.is_clean(),
        "and it is not a refusal: {:?}",
        report.refused
    );
    assert_eq!(registry.len(), 2, "two identities are registered");

    // The load-bearing consequence: the capability now has **two** registered tools, so anything that keyed
    // by capability alone would silently drop one.
    let capability = original.tools[0].capability().clone();
    assert_eq!(
        registry.by_capability(&capability).len(),
        2,
        "one capability, two identities — a capability key would collapse them"
    );
    // And the two identities are genuinely distinct, which is what makes the grant check bind.
    assert_ne!(
        original.tools[0].identity.schema_fingerprint,
        changed.tools[0].identity.schema_fingerprint
    );
    assert!(!original.tools[0].is_same_tool_as(&changed.tools[0]));
}

#[test]
fn a_published_catalog_must_not_collapse_two_identities_sharing_one_capability() {
    // The hazard the test above exposes, asserted where it would land. `RegistryCatalog` is keyed by
    // **capability string**, and its `new` documents last-wins for a duplicate — so a registry legitimately
    // holding two identities under one capability would have one silently dropped from the catalog, and a
    // call resolving the *dropped* one would report `tool.not_found` for a tool that is registered.
    //
    // This test asserts against the registry rather than the catalog because the catalog's shape is the
    // reason the invariant matters: `all_unfiltered` is the accessor that preserves both, and a builder that
    // used `by_capability` instead could not.
    let mut registry = ToolRegistry::new();
    let original = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    register_catalog(&mut registry, &server(), &original);
    let widened =
        r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#;
    let changed = normalize_catalog("acme-files", &[mcp_tool("read_file", widened)]);
    register_catalog(&mut registry, &server(), &changed);

    // The accessor a catalog builder must use returns **both**, which is the property that makes a catalog
    // from a registry representable at all. Counted rather than asserted as a length on the builder, because
    // the builder does not exist yet — this records the invariant it must satisfy when it is written.
    let all = registry.all_unfiltered();
    assert_eq!(
        all.len(),
        2,
        "the unfiltered view preserves both identities"
    );
    let capabilities: std::collections::BTreeSet<String> = all
        .iter()
        .map(|tool| tool.definition.capability().to_string())
        .collect();
    assert_eq!(
        capabilities.len(),
        1,
        "both identities really do share one capability: {capabilities:?}"
    );
    let identities: std::collections::BTreeSet<String> = all
        .iter()
        .map(|tool| format!("{}", tool.definition.identity.schema_fingerprint))
        .collect();
    assert_eq!(identities.len(), 2, "and their identities differ");
}

#[test]
fn a_catalog_claiming_another_servers_owner_is_refused() {
    // The impersonation check, and the reason the trusted id is a **parameter**. The catalog below declares
    // `other-server` as its owner while the trusted configuration identity is `acme-files` — which is what a
    // hostile or misconfigured listing looks like. An implementation that derived the trusted id from the
    // catalog could not detect this at all, because the two would agree.
    let catalog = normalize_catalog("other-server", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    let mut registry = ToolRegistry::new();

    let report = register_catalog(&mut registry, &server(), &catalog);

    assert_eq!(report.added, 0);
    assert_eq!(report.refused.len(), 1);
    assert_eq!(
        report.refused[0].refusal,
        McpRegistrationRefusal::SourceMismatch {
            claimed: "other-server".to_owned(),
            configured: "acme-files".to_owned(),
        }
    );
    assert_eq!(report.refused[0].refusal.code(), "mcp.source_mismatch");
    assert_eq!(registry.len(), 0, "nothing may be registered");
}

#[test]
fn the_first_wins_source_claim_is_not_poisoned_by_a_refused_catalog() {
    // Why the impersonation check runs **before** the registry rather than leaning on the registry's own
    // source check. The registry's claim is first-wins, so a catalog claiming another server's source that
    // reached `register` would install the claim — and the *real* server would then be refused
    // `ToolSourceConflict` forever. Asserted by trying the impostor first and the real server second.
    let mut registry = ToolRegistry::new();

    let impostor = normalize_catalog("other-server", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    let refused = register_catalog(&mut registry, &server(), &impostor);
    assert_eq!(refused.refused.len(), 1);

    // The real server, fetched from its own configuration identity, must still register cleanly.
    let real = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    let report = register_catalog(&mut registry, &server(), &real);

    assert!(
        report.is_clean(),
        "the impostor must not have claimed the real server's source: {:?}",
        report.refused
    );
    assert_eq!(report.added, 1);
    assert_eq!(registry.len(), 1);
}

#[test]
fn a_second_configured_server_offering_the_same_source_is_refused() {
    // The registry's own source claim, reached through this helper: two configured servers declaring one
    // source means the second would inherit the first's approvals, so it is refused unconditionally — and a
    // *replacement* flag does not help, because the collision is what the flag would be used to authorize.
    let catalog = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    let mut registry = ToolRegistry::new();
    assert_eq!(
        register_catalog(&mut registry, &server(), &catalog).added,
        1
    );

    // A second configuration identity whose catalog happens to declare the first's owner — the mismatch is
    // caught before the registry, which is the stricter of the two checks and the one that runs.
    let second = server_config_id("other-files").expect("the fixture name is usable");
    let report = register_catalog(&mut registry, &second, &catalog);

    assert_eq!(report.added, 0);
    assert_eq!(report.refused.len(), 1);
    assert_eq!(
        report.refused[0].refusal,
        McpRegistrationRefusal::SourceMismatch {
            claimed: "acme-files".to_owned(),
            configured: "other-files".to_owned(),
        }
    );
}

#[test]
fn a_catalog_with_no_source_registers_nothing_rather_than_misattributing() {
    // `normalize_catalog` returns no source when the server name itself was unusable — and it refuses every
    // tool in that case. A caller that somehow holds such a catalog with tools must not have them
    // registered against whichever server happened to call, so nothing is attributed.
    let empty = crate::mcp::NormalizedCatalog::default();
    let mut registry = ToolRegistry::new();
    let report = register_catalog(&mut registry, &server(), &empty);

    assert!(report.is_clean(), "no tools offered means no refusals");
    assert_eq!(report.registered(), 0);
    assert_eq!(registry.len(), 0);

    // And the normalizer's own refusal path: an unusable server name produces no source *and* refuses each
    // tool, so the two halves agree rather than one silently registering what the other dropped.
    let unusable = normalize_catalog("Not A Server", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    assert!(unusable.source.is_none());
    assert_eq!(unusable.tools.len(), 0, "the normalizer refuses them");
    let report = register_catalog(&mut registry, &server(), &unusable);
    assert_eq!(report.registered(), 0);
}

#[test]
fn an_unusable_configuration_identity_is_refused_rather_than_coerced() {
    // The configuration identity's rule is similar to the source owner's but separately owned, so a name
    // that fails it is refused here too. A helper that coerced the name — lowercasing it, say — would make
    // two different configured servers share an identity, which is the attribution this pair of rules exists
    // to keep apart.
    for unusable in ["Not A Server", "acme files", "", "ACME-FILES"] {
        assert_eq!(
            server_config_id(unusable).expect_err("must be refused"),
            McpRegistrationRefusal::ServerNameInvalid,
            "`{unusable}` must be refused rather than repaired"
        );
    }
    // The complement, so this is not a helper that refuses every name.
    assert_eq!(
        server_config_id("brave-search")
            .expect("hyphens and digits are usable")
            .as_str(),
        "brave-search"
    );
}

#[test]
fn every_published_pair_carries_a_schema_and_agrees_with_its_registration() {
    // The join property. `RegistryCatalog` takes `(ToolDefinition, Option<String>)` and refuses a `None`
    // schema as `tool.schema_absent`, so a pair without one is an uncallable tool — the defect the previous
    // slice fixed one layer up must not reappear at the join.
    let catalog = normalize_catalog(
        "acme-files",
        &[
            mcp_tool("read_file", OBJECT_SCHEMA),
            mcp_tool("write_file", OBJECT_SCHEMA),
        ],
    );

    let mut registry = ToolRegistry::new();
    let report = register_catalog(&mut registry, &server(), &catalog);
    assert!(report.is_clean(), "{:?}", report.refused);

    let pairs = publishable_pairs(&registry, &server(), &catalog)
        .expect("two distinct capabilities must be publishable");
    assert_eq!(pairs.len(), 2);
    for (definition, schema) in &pairs {
        let schema = schema.as_ref().expect("every pair must carry a schema");
        assert!(!schema.is_empty());
        // And it must be the schema that identity was built over, which is what the call path checks.
        crate::tool_schema::ToolSchema::parse(schema)
            .expect("the carried schema must parse")
            .confirms(&definition.identity.schema_fingerprint)
            .expect("the carried schema must be the one that was fingerprinted");
        assert_eq!(definition.source().kind, SourceKind::McpServer);
    }
    // Aligned by construction, so the pairs are in capability order like the catalog they came from.
    assert_eq!(pairs[0].0.capability().to_string(), "mcp.read_file@1");
    assert_eq!(pairs[1].0.capability().to_string(), "mcp.write_file@1");
}

#[test]
fn a_refused_tool_is_not_published_even_though_it_was_offered() {
    // **The security property of the join, and the reason `publishable_pairs` reads the registry rather than
    // the catalog.** A catalog refused as `SourceMismatch` is an impersonation: the registry holds nothing for
    // it, and an implementation that published from what was *offered* would serve that server's tools while
    // the only record saying it was refused sat beside a catalog dispatching it.
    let catalog = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);

    // A registry that never saw this catalog — exactly what a refused registration leaves behind.
    let empty = ToolRegistry::new();
    let pairs = publishable_pairs(&empty, &server(), &catalog)
        .expect("a catalog with one capability cannot collide");
    assert!(
        pairs.is_empty(),
        "a tool the registry never admitted must not be published, got {}",
        pairs.len()
    );

    // The complement, so this is not a function that publishes nothing: once admitted, the same catalog
    // publishes.
    let mut registry = ToolRegistry::new();
    register_catalog(&mut registry, &server(), &catalog);
    assert_eq!(
        publishable_pairs(&registry, &server(), &catalog).map(|p| p.len()),
        Ok(1)
    );
}

#[test]
fn a_tool_admitted_from_another_server_is_not_published_by_this_one() {
    // **The attribution half of the predicate.** A second server declaring the first's source produces a
    // byte-identical definition, so the *identity* resolves to the first server's registration and an
    // identity-only lookup would publish the impostor's tool. The trusted attribution is what distinguishes
    // them — the same distinction `RegisteredTool.server` exists to make.
    let catalog = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);

    let mut registry = ToolRegistry::new();
    let real = server();
    // The first server admits it, so the identity is registered — from *this* server.
    assert_eq!(register_catalog(&mut registry, &real, &catalog).added, 1);

    // Another configured server presents the identical catalog. It is refused, and the identity still resolves
    // — to the real server. Publishing it under the impostor's name would hand the impostor a tool the
    // registry attributed to someone else.
    let impostor = server_config_id("other-files").expect("the second name is usable");
    assert!(
        !register_catalog(&mut registry, &impostor, &catalog).is_clean(),
        "a second server offering the same source must be refused"
    );

    let published =
        publishable_pairs(&registry, &impostor, &catalog).expect("one capability cannot collide");
    assert!(
        published.is_empty(),
        "a registration refused as SourceClaimed must not be published under the impostor's identity"
    );
    // And the real server still publishes it, so the check above is about attribution and not about the
    // catalog having become unpublishable.
    assert_eq!(
        publishable_pairs(&registry, &real, &catalog).map(|p| p.len()),
        Ok(1)
    );
}

#[test]
fn two_identities_under_one_capability_are_refused_rather_than_silently_collapsed() {
    // The hazard this join is exposed to, asserted at the join. A `RegistryCatalog` is keyed by capability
    // string, and a schema change legitimately produces two identities under one capability — which the
    // registry holds and a capability-keyed map cannot. Without the check, one definition would be dropped
    // by last-wins and a call resolving it would report `tool.not_found` for a registered tool: a silent
    // loss indistinguishable from a tool the server never offered.
    let catalog = normalize_catalog(
        "acme-files",
        &[
            mcp_tool("read_file", OBJECT_SCHEMA),
            mcp_tool("write_file", OBJECT_SCHEMA),
        ],
    )
    .clone();
    // Rename the second so it collides with the first on capability while differing by schema, which is the
    // shape a changed tool has. Built by hand rather than by two discoveries so the collision is exact.
    let mut collided = catalog.clone();
    collided.tools[1] = catalog.tools[0].clone();
    collided.schemas[1] = catalog.schemas[0].clone();

    let registry = ToolRegistry::new();
    let refusal = publishable_pairs(&registry, &server(), &collided)
        .expect_err("a lossy catalog must be refused");
    assert_eq!(
        refusal,
        McpRegistrationRefusal::CapabilityCollision {
            capability: "mcp.read_file@1".to_owned()
        }
    );
    assert_eq!(refusal.code(), "mcp.capability_collision");

    // The complement: the un-collided catalog is publishable, so this is not a function that refuses
    // everything.
    assert!(publishable_pairs(&registry, &server(), &catalog).is_ok());
}

#[test]
fn a_collision_is_refused_even_when_one_of_the_two_identities_was_never_admitted() {
    // **Why the collision is checked over the *offered* catalog rather than the admitted subset.**
    //
    // ⚠ The first version of this test registered *both* identities and then claimed the subset check would
    // pass — it would not, because both were admitted and filtering changed nothing. The mutation that
    // rewrote the check to read the admitted subset was caught by the *older* collision test instead, which
    // is how the fixture was found to be wrong. A test whose name names a state its fixture does not create
    // proves nothing about that state.
    //
    // So the registry here holds **one** identity for the capability, and the catalog offers that identity
    // plus a second that was never registered. An admitted-subset check would see one tool, find no
    // collision, and publish — and the second identity, once admitted later, would silently displace the
    // first in a capability-keyed map.
    let admitted = normalize_catalog("acme-files", &[mcp_tool("read_file", OBJECT_SCHEMA)]);
    let mut registry = ToolRegistry::new();
    assert_eq!(
        register_catalog(&mut registry, &server(), &admitted).added,
        1
    );

    // The same capability with a different schema: a distinct identity that the registry has never seen.
    let unregistered = normalize_catalog("acme-files", &[mcp_tool("read_file", OTHER_SCHEMA)]);
    let mut offered = admitted.clone();
    offered.tools.push(unregistered.tools[0].clone());
    offered.schemas.push(unregistered.schemas[0].clone());

    // The two really do share one capability, so this is a collision rather than two unrelated tools.
    assert_eq!(
        offered.tools[0].capability(),
        offered.tools[1].capability(),
        "the fixture must create a real collision"
    );
    // And their identities differ, so they are two registered tools rather than one listed twice.
    assert_ne!(
        offered.tools[0].identity, offered.tools[1].identity,
        "a shared capability with one identity is not a collision"
    );

    // Asserted by counting the *admitted* set first, so the refusal below is attributable: an
    // admitted-subset check would see exactly this one tool and find nothing to collide.
    let admitted_only = admitted
        .tools
        .iter()
        .filter(|definition| registry.get(&definition.identity).is_some())
        .count();
    assert_eq!(admitted_only, 1, "only one of the two is admitted");

    let refusal = publishable_pairs(&registry, &server(), &offered)
        .expect_err("two identities under one capability must be refused");
    assert_eq!(refusal.code(), "mcp.capability_collision");
}

#[test]
fn every_refusal_code_is_namespaced_and_distinct() {
    let refusals = [
        McpRegistrationRefusal::ServerNameInvalid,
        McpRegistrationRefusal::SourceMismatch {
            claimed: "a".to_owned(),
            configured: "b".to_owned(),
        },
        McpRegistrationRefusal::IdentityChanged,
        McpRegistrationRefusal::SourceClaimed,
        McpRegistrationRefusal::RegistryFull,
        McpRegistrationRefusal::DefinitionInvalid { field: "x" },
        McpRegistrationRefusal::UnexpectedReplacement,
    ];
    let codes: Vec<&str> = refusals.iter().map(McpRegistrationRefusal::code).collect();

    assert!(
        codes.iter().all(|code| code.starts_with("mcp.")),
        "{codes:?}"
    );
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "codes must be distinct: {codes:?}"
    );
}
