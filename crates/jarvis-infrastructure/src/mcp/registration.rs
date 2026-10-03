//! Registering a discovered MCP catalog, which is where the untrusted data meets the trusted claim.
//!
//! This is the join the previous slices left open: [`super::normalize_catalog`] produces a
//! [`super::NormalizedCatalog`], [`ToolRegistry`] holds canonical definitions, and `tool_adapters::RegistryCatalog`
//! consumes `(ToolDefinition, Option<String>)` pairs. Each piece existed; what did not was the step that
//! takes a server's listing and either admits it or refuses it **by name**.
//!
//! # Two identities, and only one of them is trusted
//!
//! `RegisteredTool` says it plainly: `server` is "the configured server that supplied it. **Trusted**,
//! unlike `definition.identity.source`". That distinction is the whole reason this module takes the
//! trusted server id as a **parameter** rather than deriving it from the catalog:
//!
//! - The **configuration identity** comes from JARVIS's own profile — an operator wrote it.
//! - The **source** inside each definition is server-reported data that this adapter normalized. A server
//!   chooses its own owner and version strings, so `ToolSource` is an untrusted claim *even though* this
//!   adapter validated its shape.
//!
//! A helper that derived the trusted id from the catalog would make the two agree by construction and look
//! safer, while removing the only check that catches a catalog claiming to be a different server. The
//! mismatch is therefore an explicit refusal, and it is asserted with a catalog whose owner is not the
//! configured server.
//!
//! # The refusal is the default, and that is a security decision rather than a conservative one
//!
//! Every registration here uses [`RegistrationRequest::new`], which **refuses** a change to a known
//! identity. The alternative, `replacing`, accepts one — and a replacement keeps the identity, so "every
//! approval recorded against it still matches, while the implementation behind it is new", which is
//! `ACC-024`'s failure mode verbatim. A server that re-schemas `mcp.read_file@1` between two discoveries
//! would otherwise have its new implementation inherit every grant the old one held.
//!
//! So a *changed* tool is reported as a refusal an operator acts on, not as a silent replacement. There is
//! deliberately no parameter that widens this: a re-registration after a reviewed change is a different
//! operation with its own review, and a flag here would be the thing an attacker uses it through.
//!
//! # What a caller gets back
//!
//! A count of what happened per tool, including the refusals with their capability and reason, because
//! "the server offered forty tools" and "thirty-nine were registered and one was refused for a changed
//! schema" are different operator facts — and a caller holding only a count cannot tell them apart. The
//! same reasoning [`super::NormalizedCatalog`] already applies to dropped tools.

use jarvis_domain::error::DomainError;
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::registry::{
    RegistrationOutcome, RegistrationRequest, ServerConfigId, ToolRegistry,
};

/// Why one tool could not be registered.
///
/// Per-tool rather than one error for the catalog, so one refused tool does not discard the rest — and
/// each variant is a distinct operator fact with its own code, because collapsing them would leave an
/// operator unable to tell an impersonation attempt from a schema change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpRegistrationRefusal {
    /// The configured server identity is not usable.
    #[error("mcp server configuration identity is not usable")]
    ServerNameInvalid,
    /// The catalog's declared source owner is not the server it was fetched from.
    ///
    /// The impersonation check. The source inside a definition is untrusted, so a catalog whose owner is
    /// another server's name is either a defect or an attempt to inherit that server's approvals.
    #[error("mcp catalog claims source owner `{claimed}` but was fetched from `{configured}`")]
    SourceMismatch {
        /// The owner the catalog declared.
        claimed: String,
        /// The trusted configuration identity it was fetched from.
        configured: String,
    },
    /// The registry already knows this identity with **different** content.
    ///
    /// `ACC-024`'s case: the tool's schema changed under an identity grants are recorded against. Refused
    /// rather than replaced, because a replacement would let the new implementation inherit the old one's
    /// approvals.
    #[error("mcp tool identity is already registered with different content")]
    IdentityChanged,
    /// Another configured server already claims this tool's source.
    #[error("mcp tool source is claimed by a different configured server")]
    SourceClaimed,
    /// The registry is full.
    #[error("mcp tool registry is full")]
    RegistryFull,
    /// The definition was refused by the registry for a field this adapter cannot attribute.
    #[error("mcp tool definition was refused for field `{field}`")]
    DefinitionInvalid {
        /// The field the registry named.
        field: &'static str,
    },
    /// The registry reported a replacement this helper never asked for.
    ///
    /// **Defensive rather than expected.** Every request here is `RegistrationRequest::new`, which cannot
    /// produce [`RegistrationOutcome::Replaced`], so reaching this arm would mean that contract changed.
    /// A named refusal rather than an `unreachable!()` for two reasons: a panic in a registration path is a
    /// worse outcome than a reported refusal, and this project denies `panic` in library code. Silently
    /// counting it would be the third option and the worst one — a defect dressed as success.
    #[error("mcp registry reported an unexpected replacement")]
    UnexpectedReplacement,
    /// Two definitions share one capability, which a capability-keyed catalog cannot represent.
    #[error("mcp tools `{capability}` collide on one capability and cannot share a catalog")]
    CapabilityCollision {
        /// The capability the two definitions share.
        capability: String,
    },
}

impl McpRegistrationRefusal {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ServerNameInvalid => "mcp.server_config_invalid",
            Self::SourceMismatch { .. } => "mcp.source_mismatch",
            Self::IdentityChanged => "mcp.identity_changed",
            Self::SourceClaimed => "mcp.source_claimed",
            Self::RegistryFull => "mcp.registry_full",
            Self::DefinitionInvalid { .. } => "mcp.definition_invalid",
            Self::UnexpectedReplacement => "mcp.unexpected_replacement",
            Self::CapabilityCollision { .. } => "mcp.capability_collision",
        }
    }
}

/// A tool that was offered and not registered, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedRegistration {
    /// The canonical capability, so the refusal names a tool rather than a position in a list.
    pub capability: String,
    /// Why it was refused.
    pub refusal: McpRegistrationRefusal,
}

/// What one catalog's registration did.
///
/// `replaced` is deliberately **absent**: this module never asks for a replacement, and a field that can
/// only ever be zero would read like an outcome a caller should handle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationReport {
    /// Tools whose identity was new.
    pub added: usize,
    /// Tools that were already registered with identical content — a reconnect re-listing.
    pub unchanged: usize,
    /// Tools that were not registered, in the catalog's own order.
    pub refused: Vec<RefusedRegistration>,
}

impl RegistrationReport {
    /// Returns whether every offered tool was registered without a refusal.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.refused.is_empty()
    }

    /// Returns the number of tools that are now registered, whether new or already known.
    #[must_use]
    pub const fn registered(&self) -> usize {
        self.added + self.unchanged
    }
}

/// Builds the trusted configuration identity for a server name.
///
/// # Errors
///
/// Returns [`McpRegistrationRefusal::ServerNameInvalid`] when the name is not a usable configuration
/// identity. Note that this rule and the tool-source owner rule are *similar but separately owned*: a name
/// accepted here must also be accepted by [`super::server_source`], and a caller that validated one and not
/// the other would get a catalog whose every tool is refused.
pub fn server_config_id(server: &str) -> Result<ServerConfigId, McpRegistrationRefusal> {
    ServerConfigId::new(server).map_err(|_| McpRegistrationRefusal::ServerNameInvalid)
}

/// Registers every tool a catalog offers, refusing rather than replacing.
///
/// `server` is the **trusted** configuration identity the catalog was fetched from; it is not derived from
/// the catalog, because a derived value would agree by construction and the mismatch check above would be
/// unfalsifiable.
///
/// Returns a report rather than a `Result`, because a refusal is per-tool: a catalog of forty tools with one
/// changed schema should register thirty-nine, and one `Result` would force a caller to discard all of them
/// or to ignore the refusal.
///
/// A catalog with no `source` — the shape [`super::normalize_catalog`] returns when the server name itself
/// was unusable — registers nothing and refuses every offered tool, since there is nothing to attribute a
/// definition to. That is the same choice the normalizer made, carried forward rather than re-decided.
pub fn register_catalog(
    registry: &mut ToolRegistry,
    server: &ServerConfigId,
    catalog: &super::NormalizedCatalog,
) -> RegistrationReport {
    let mut report = RegistrationReport::default();
    let Some(source) = catalog.source.as_ref() else {
        report.refused = catalog
            .tools
            .iter()
            .map(|definition| RefusedRegistration {
                capability: definition.capability().to_string(),
                // The normalizer refused every tool with this reason and produced no source, so a caller
                // that reached here with tools present is in an impossible state; refusing each one is the
                // honest answer rather than inventing an attribution.
                refusal: McpRegistrationRefusal::ServerNameInvalid,
            })
            .collect();
        return report;
    };

    // The impersonation check, made explicit. The catalog's owner is untrusted even though its *shape* was
    // validated, so a catalog declaring a different owner is refused before anything reaches the registry —
    // which matters because the registry's own source claim is **first-wins**, so an attacker whose
    // registration landed first would otherwise poison the claim and refuse the real server.
    if source.owner != server.as_str() {
        report.refused = catalog
            .tools
            .iter()
            .map(|definition| RefusedRegistration {
                capability: definition.capability().to_string(),
                refusal: McpRegistrationRefusal::SourceMismatch {
                    claimed: source.owner.clone(),
                    configured: server.as_str().to_owned(),
                },
            })
            .collect();
        return report;
    }

    for definition in &catalog.tools {
        let capability = definition.capability().to_string();
        // `new` and never `replacing`: see the module doc. A changed tool is a refusal an operator acts on.
        let request = RegistrationRequest::new(definition.clone(), server.clone());
        match registry.register(request) {
            Ok(RegistrationOutcome::Added) => report.added += 1,
            Ok(RegistrationOutcome::Unchanged) => report.unchanged += 1,
            Ok(RegistrationOutcome::Replaced) => report.refused.push(RefusedRegistration {
                capability,
                refusal: McpRegistrationRefusal::UnexpectedReplacement,
            }),
            Err(error) => report.refused.push(RefusedRegistration {
                capability,
                refusal: refusal_for(&error),
            }),
        }
    }
    report
}

/// Maps a registry error onto a refusal, keeping the registry's own reasons rather than restating them.
///
/// The `field` for a `ToolDefinitionInvalid` is carried through, so a registry-full refusal and an
/// unattributable one are distinguishable — the registry names `registry` for the former.
fn refusal_for(error: &DomainError) -> McpRegistrationRefusal {
    match error {
        DomainError::ToolIdentityConflict => McpRegistrationRefusal::IdentityChanged,
        DomainError::ToolSourceConflict => McpRegistrationRefusal::SourceClaimed,
        DomainError::ToolDefinitionInvalid { field: "registry" } => {
            McpRegistrationRefusal::RegistryFull
        }
        DomainError::ToolDefinitionInvalid { field } => {
            McpRegistrationRefusal::DefinitionInvalid { field }
        }
        // Any other domain error from `register` is a shape this adapter did not anticipate. Mapped to the
        // unattributable refusal rather than swallowed, so it reaches an operator as a refusal rather than
        // as a registration that silently did not happen.
        _ => McpRegistrationRefusal::DefinitionInvalid {
            field: "definition",
        },
    }
}

/// Returns the pairs a `RegistryCatalog` may be built from, for the tools the registry actually admitted.
///
/// The join made explicit: `RegistryCatalog::new` takes `(ToolDefinition, Option<String>)`, and the schema
/// half must be `Some` because a tool whose arguments nothing checks is refused as `tool.schema_absent`.
/// Returning `Some` for every pair is therefore the point rather than a convenience — `None` would produce
/// a catalog whose every entry is uncallable.
///
/// # Why this reads the registry instead of the catalog, and why that is the whole point
///
/// The catalog is what the server **offered**; the registry is what was **admitted**. They are different
/// sets, and an earlier version of this function returned pairs for the offered set — which published tools
/// that [`register_catalog`] had just refused. The consequences were not theoretical:
///
/// - A tool refused as [`McpRegistrationRefusal::IdentityChanged`] is `ACC-024` exactly — its schema moved
///   under an identity approvals are recorded against. Publishing it puts an implementation into the
///   dispatch catalog that no approval was ever matched to.
/// - A catalog refused as [`McpRegistrationRefusal::SourceMismatch`] is an **impersonation**. Publishing it
///   would dispatch a server's tools while the registry recorded that it was never admitted, so the one
///   record that says "this server was refused" would sit beside a catalog that serves it.
///
/// So the predicate is an **identity-and-attribution lookup per tool**, and both halves are load-bearing:
///
/// - The **identity** is the full tuple, so a definition whose content changed is a different identity and
///   is simply absent — which is why `get` returning `Some` is not sufficient on its own.
/// - The **attribution** (`registered.server == server`) covers the case the identity cannot: a second
///   server that declares the first's source and produces a byte-identical definition resolves to the
///   *first* server's registration. It was refused as `SourceClaimed`, and only the trusted attribution
///   distinguishes it.
///
/// The filter is not silent in the way a drop normally is: every tool it excludes was already reported in
/// the [`RegistrationReport`] returned alongside, with its capability and reason. A caller that ignores
/// the report gets a *smaller* catalog, which is the fail-closed direction.
///
/// # ⚠ A caller must pass definitions that share no capability
///
/// `RegistryCatalog` is keyed by **capability string**, and two identities under one capability are a state
/// the registry permits deliberately — "a capability may have several majors registered, since a new major
/// is a different tool", and a changed schema produces exactly that. A capability-keyed map cannot hold
/// both, so a catalog built from such definitions would **silently drop one**, and a call resolving the
/// dropped identity would report `tool.not_found` for a registered tool.
///
/// This function therefore **refuses to produce a lossy catalog**: a duplicate capability is reported rather
/// than collapsed, because a silent drop here is indistinguishable from a tool the server never offered.
/// The collision is checked over the **whole offered catalog** rather than the admitted subset, because two
/// identities under one capability is a fact about what the registry now holds — and checking the subset
/// would let the collision through whenever the second identity happened to be the refused one.
///
/// # Errors
///
/// Returns [`McpRegistrationRefusal::CapabilityCollision`] when two offered definitions share a capability,
/// since a capability-keyed catalog cannot represent both. A caller that needs both must resolve by identity
/// — which is what the call path does — rather than through a capability index.
pub fn publishable_pairs(
    registry: &ToolRegistry,
    server: &ServerConfigId,
    catalog: &super::NormalizedCatalog,
) -> Result<Vec<(ToolDefinition, Option<String>)>, McpRegistrationRefusal> {
    let mut seen = std::collections::BTreeSet::new();
    for definition in &catalog.tools {
        if !seen.insert(definition.capability().to_string()) {
            return Err(McpRegistrationRefusal::CapabilityCollision {
                capability: definition.capability().to_string(),
            });
        }
    }
    Ok(catalog
        .tools
        .iter()
        .zip(catalog.schemas.iter())
        .filter(|(definition, _)| is_admitted(registry, server, definition))
        .map(|(definition, schema)| (definition.clone(), Some(schema.clone())))
        .collect())
}

/// Returns whether the registry admitted **this** definition **from this server**.
///
/// Split out from [`publishable_pairs`] so the predicate has a name and can be asserted directly, rather
/// than being a closure a reader has to reconstruct from a `filter`.
fn is_admitted(
    registry: &ToolRegistry,
    server: &ServerConfigId,
    definition: &ToolDefinition,
) -> bool {
    registry
        .get(&definition.identity)
        .is_some_and(|registered| {
            registered.server == *server && registered.definition == *definition
        })
}

#[cfg(test)]
#[path = "registration_tests.rs"]
mod tests;
