//! The tool registry: what is discoverable, and what a discovery answer was computed under.
//!
//! `TLS-002` asks for "registry discovery independently from grants", and the word doing the work
//! is **independently**. The tool fabric states it as a rule:
//!
//! > Discovery affects model context, not authorization at execution time.
//! > Tool discovery never grants execution permission.
//!
//! So this module answers exactly one question — *which definitions exist for this principal,
//! workspace, and negotiated protocol* — and answers nothing else. It deliberately holds **no
//! grants**, so a caller cannot read a permission out of it even by accident, and the only way to
//! reach a tool's effects is through the definition, which is metadata rather than authorization.
//!
//! The `TLS-002` sentence *"Implement registry discovery **independently from grants**"* was
//! unambiguous about the direction to build in: the registry is the half that can exist without
//! the other, and building it first is what stops the registry growing a permission check that a
//! later policy layer would be tempted to trust.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::DomainError;
use crate::ids::{PrincipalId, WorkspaceId};

use super::definition::ToolDefinition;
use super::identity::{SourceKind, ToolCapability, ToolIdentity, ToolSource};

/// The number of definitions one registry may hold.
///
/// A bound because every definition carries four bounded strings and the registry is scanned on
/// every discovery call: an unbounded registry is an unbounded per-call cost. The limit is far
/// above a plausible catalog and exists so a misbehaving source cannot grow it without limit.
pub const MAX_REGISTERED_TOOLS: usize = 4096;

// ---------------------------------------------------------------------------------------
// Server configuration identity.
// ---------------------------------------------------------------------------------------

/// Which configured server a tool's registration came from.
///
/// **Distinct from [`ToolSource`] by design, and the distinction is load-bearing.** `ToolSource`
/// is a claim the tool makes about itself — its `owner` and `version` come from the manifest or
/// the MCP server's own handshake, so they are *untrusted input*. This configuration identity is
/// assigned by JARVIS and comes from configuration, so it is *trusted*. The tool fabric's
/// required test "tool-name and source-identity collision" can only be answered by comparing the
/// trusted one: two servers both declaring `acme.files 1.0.0` are indistinguishable in
/// `ToolSource`, and refusing the second is what stops it inheriting the first's approvals.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ServerConfigId(String);

impl ServerConfigId {
    /// The longest accepted configuration identity.
    const MAX_BYTES: usize = 128;

    /// Validates and wraps a configuration identity.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `server_config_id` when the value is
    /// empty, over-long, or contains anything outside lowercase letters, digits, dots, underscores,
    /// and hyphens. The same rule as a [`super::classification::Scope`], for the same reason: the
    /// value reaches a persisted registration and operator output.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        let usable = !value.is_empty()
            && value.len() <= Self::MAX_BYTES
            && value.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '.'
                    || character == '_'
                    || character == '-'
            });
        if !usable {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "server_config_id",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the configuration identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServerConfigId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ServerConfigId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------------------
// Discovery scope and cache key.
// ---------------------------------------------------------------------------------------

/// The scopes a discovery answer is computed under.
///
/// Every field is from the contract's list for a discovery cache key — "server configuration
/// identity, authenticated principal, workspace/account, protocol version, capability set, and
/// list-result version/TTL" — except the last, which is not a scope: it identifies *which
/// revision* of the answer a key produced, and it is what makes a stale answer detectable rather
/// than merely old.
///
/// **Why every field, and not just the server.** A cache keyed on the server alone would answer
/// principal A's discovery call with the answer computed for principal B. In a local profile that
/// is the common case rather than the exotic one: one workspace is shared by every enrolled
/// client, each with its own principal, so a key missing the principal returns one client's
/// catalog to another. That is a disclosure, and it is the same mistake `BRN-007`'s idempotency
/// scope made — a key naming three of five dimensions, which reads as complete.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct DiscoveryScope {
    /// The configured server whose catalog is being asked for.
    pub server: ServerConfigId,
    /// The authenticated principal asking.
    pub principal: PrincipalId,
    /// The resolved workspace.
    pub workspace: WorkspaceId,
    /// The negotiated protocol version.
    pub protocol_version: String,
    /// The negotiated capability set.
    ///
    /// A `BTreeSet`, so the key is order-independent: two handshakes that negotiated the same
    /// capabilities must not produce two cache entries, because that is how one scope's answer
    /// could be missed rather than mis-returned.
    pub capabilities: BTreeSet<String>,
}

impl DiscoveryScope {
    /// The longest accepted protocol version.
    const MAX_PROTOCOL_BYTES: usize = 64;

    /// Builds a scope, validating the protocol version and capability names.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `protocol_version` or `capabilities`
    /// when a value is empty, over-long, or carries control characters. A capability name comes
    /// from the server's own handshake, so it is bounded here rather than trusted.
    pub fn new(
        server: ServerConfigId,
        principal: PrincipalId,
        workspace: WorkspaceId,
        protocol_version: &str,
        capabilities: BTreeSet<String>,
    ) -> Result<Self, DomainError> {
        if protocol_version.is_empty()
            || protocol_version.len() > Self::MAX_PROTOCOL_BYTES
            || protocol_version.chars().any(char::is_control)
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "protocol_version",
            });
        }
        // Bounded per name and bounded in count: both are needed, since a server could send many
        // short capabilities or one enormous one.
        if capabilities.len() > super::classification::MAX_TOOL_SCOPES * 4
            || capabilities.iter().any(|capability| {
                capability.is_empty()
                    || capability.len() > Self::MAX_PROTOCOL_BYTES
                    || capability.chars().any(char::is_control)
            })
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "capabilities",
            });
        }
        Ok(Self {
            server,
            principal,
            workspace,
            protocol_version: protocol_version.to_owned(),
            capabilities,
        })
    }
}

/// Identifies one revision of a discovery answer.
///
/// A list result has its own version, and the contract ties the cache's lifetime to it: an answer
/// computed at revision 4 is stale the moment the server reports revision 5. The value is opaque
/// because what a server uses for it varies — an MCP server may send a string, a native source a
/// counter — so the domain stores the digest of whatever the adapter normalized rather than
/// imposing a shape on a foreign protocol. Carrying it as bounded text keeps the bound and the
/// encoding rule in one place and leaves the meaning to the owner, exactly as `JsonText` does.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct ListVersion(String);

impl ListVersion {
    /// The longest accepted version token.
    const MAX_BYTES: usize = 128;

    /// Validates and wraps a version token.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `list_version` when the value is
    /// empty, over-long, or carries control characters.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > Self::MAX_BYTES || value.chars().any(char::is_control)
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "list_version",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the version token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> serde::Deserialize<'de> for ListVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

/// A discovery cache key, complete by construction.
///
/// The scope and the list version are held together because a key that omitted either would be
/// wrong in a way that reads as correct: without the scope one principal's answer serves another,
/// and without the version a stale answer is indistinguishable from a fresh one. Holding them in
/// one type means there is no way to build a partial key — the failure mode the field-per-argument
/// shape would have permitted.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct DiscoveryCacheKey {
    /// What the answer was computed under.
    pub scope: DiscoveryScope,
    /// Which revision of the server's list it reflects.
    pub list_version: ListVersion,
}

// ---------------------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------------------

/// One registered definition together with the trusted facts about where it came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegisteredTool {
    /// The definition, validated at construction by [`ToolDefinition::new`].
    pub definition: ToolDefinition,
    /// The configured server that supplied it. Trusted, unlike `definition.identity.source`.
    pub server: ServerConfigId,
}

/// What a registration did, so a caller can distinguish a first registration from a re-discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The identity was not known and is now registered.
    Added,
    /// The identical definition was already registered from the same server.
    ///
    /// **Idempotent rather than an error, and the distinction matters.** A server that lists its
    /// tools on every reconnect re-registers them; failing that would make a healthy reconnect
    /// look like a conflict. What is refused is a *different* definition under a known identity
    /// (see below), so this arm covers only the case where nothing changed.
    Unchanged,
    /// A definition with the same identity and different content replaced the previous one.
    ///
    /// Requires [`ToolRegistry::register`]'s caller to have asked for it via
    /// [`RegistrationRequest::replacing`], because an unreviewed replacement is exactly what
    /// `ACC-024` forbids: the identity is the same, so every approval recorded against it still
    /// matches, while the implementation behind it is new. Returning this outcome rather than
    /// silently succeeding is what lets an adapter log that an approval must be re-obtained.
    Replaced,
}

/// A registration request, with the replacement decision made explicit.
#[derive(Debug, Clone)]
pub struct RegistrationRequest {
    /// The definition being registered.
    pub definition: ToolDefinition,
    /// The configured server supplying it.
    pub server: ServerConfigId,
    replacement: Replacement,
}

/// Whether a registration may replace an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replacement {
    /// Refuse a change; only an identical re-registration is accepted.
    Refused,
    /// Accept a change, reporting it as a replacement.
    Allowed,
}

impl RegistrationRequest {
    /// A registration that may not change what is already known.
    ///
    /// The default, because the safe direction is the one that refuses.
    #[must_use]
    pub fn new(definition: ToolDefinition, server: ServerConfigId) -> Self {
        Self {
            definition,
            server,
            replacement: Replacement::Refused,
        }
    }

    /// A registration that may replace a known identity with different content.
    ///
    /// Named `replacing` rather than `allowing_replacement` so a call site reads as the decision
    /// it is: the caller is stating that a review happened elsewhere and this replacement is
    /// intended, which is the thing an audit has to be able to find.
    #[must_use]
    pub fn replacing(definition: ToolDefinition, server: ServerConfigId) -> Self {
        Self {
            definition,
            server,
            replacement: Replacement::Allowed,
        }
    }
}

// ---------------------------------------------------------------------------------------
// The registry.
// ---------------------------------------------------------------------------------------

/// What is discoverable, with no notion of who may call it.
///
/// The absence of grants here is the point of `TLS-002`. A registry that also answered "may this
/// be called" would be a second authorization decision living beside the policy layer, and the
/// one that got consulted first would be the one that mattered — which is how a discovery cache
/// becomes an authorization cache.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    /// Registered tools by identity. A `BTreeMap` keyed on the identity itself, so a lookup is a
    /// membership test on the tuple rather than a comparison against each entry.
    by_identity: BTreeMap<ToolIdentity, RegisteredTool>,
    /// Which server first claimed each source identity. This is the trusted side of the
    /// collision check; `ToolSource` is the untrusted claim it is compared against.
    source_claims: BTreeMap<ToolSource, ServerConfigId>,
}

impl ToolRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns how many tools are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_identity.len()
    }

    /// Returns whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_identity.is_empty()
    }

    /// Registers a definition.
    ///
    /// # Errors
    ///
    /// - [`DomainError::ToolIdentityConflict`] when the identity is known with **different**
    ///   content and the request did not allow a replacement.
    /// - [`DomainError::ToolSourceConflict`] when the definition's source is already claimed by a
    ///   **different** server configuration. This is the "source-identity collision" the tool
    ///   fabric names: an identity names a source, so a second server declaring the same source
    ///   inherits every approval recorded for the first. Refused unconditionally — a replacement
    ///   flag does not help, because the collision is what the flag would be used to authorize.
    /// - [`DomainError::ToolDefinitionInvalid`] naming `registry` when the registry is full.
    pub fn register(
        &mut self,
        request: RegistrationRequest,
    ) -> Result<RegistrationOutcome, DomainError> {
        let RegisteredTool { definition, server } = RegisteredTool {
            definition: request.definition,
            server: request.server,
        };
        let identity = definition.identity.clone();

        // The source claim is checked **first**, before the identity, because it is the check with
        // no override: whatever the identity's state, a second server claiming a known source is
        // refused. Checking it second would let a replacement path reach the identity branch and
        // return an outcome for a registration that should never have proceeded.
        match self.source_claims.get(&identity.source) {
            Some(claimant) if claimant != &server => {
                return Err(DomainError::ToolSourceConflict);
            }
            _ => {}
        }

        if let Some(existing) = self.by_identity.get(&identity) {
            if existing.definition == definition {
                // Identical content from the same server: a reconnect re-listing its tools. Not a
                // change, so not a failure.
                return Ok(RegistrationOutcome::Unchanged);
            }
            if request.replacement == Replacement::Refused {
                return Err(DomainError::ToolIdentityConflict);
            }
            // A permitted replacement keeps the identity, so the capability index needs no update
            // beyond what is already there.
            self.by_identity
                .insert(identity.clone(), RegisteredTool { definition, server });
            return Ok(RegistrationOutcome::Replaced);
        }

        if self.by_identity.len() >= MAX_REGISTERED_TOOLS {
            return Err(DomainError::ToolDefinitionInvalid { field: "registry" });
        }

        // Nothing checks the capability here, and that is deliberate rather than an omission. Two
        // tools may share a capability legitimately — `fs.read@1` and `fs.read@2` are different
        // tools by the contract's own rule, and two servers may each offer one — so "this
        // capability is taken" is not a conflict. What *is* a conflict is a second server claiming
        // a **source**, which was checked above, and that check is the mechanism by which
        // impersonation is impossible: a tool's `ToolSource` is untrusted input, so without the
        // trusted claim a second server would simply declare the first's owner and version and
        // reach the same identity. A capability index was written here in the first version and
        // removed, because it was written and never read — the "field with no reader" shape — and
        // its presence implied a collision check that did not exist.
        self.source_claims
            .entry(identity.source.clone())
            .or_insert_with(|| server.clone());
        self.by_identity
            .insert(identity, RegisteredTool { definition, server });
        Ok(RegistrationOutcome::Added)
    }

    /// Removes a registration, returning whether one was present.
    ///
    /// The counterpart of `register` and needed for the same reason a connector "cannot be
    /// disabled" is incomplete: a tool that cannot be deregistered keeps its identity occupied, so
    /// a corrected implementation can never be registered without a replacement flag. Removing the
    /// last tool of a source also releases its source claim, so a server that is removed and
    /// re-added is not permanently locked out.
    pub fn deregister(&mut self, identity: &ToolIdentity) -> bool {
        let Some(removed) = self.by_identity.remove(identity) else {
            return false;
        };
        // The **source claim** is the derived state that has to be recomputed, and only the claim:
        // leaving it in place would refuse a legitimate later registration by a server that had been
        // removed and re-added, while the registry itself still looked correct. The claim is held by
        // a *source*, so it is released only when the last tool of that source goes — a source still
        // offering one tool must keep it, or a second server could claim a source the first is still
        // serving from.
        let source_still_claimed = self
            .by_identity
            .values()
            .any(|tool| tool.definition.identity.source == removed.definition.identity.source);
        if !source_still_claimed {
            self.source_claims
                .remove(&removed.definition.identity.source);
        }
        true
    }

    /// Returns the registered tool with this identity, if any.
    #[must_use]
    pub fn get(&self, identity: &ToolIdentity) -> Option<&RegisteredTool> {
        self.by_identity.get(identity)
    }

    /// Returns every registered tool whose capability is `capability`.
    ///
    /// A capability may have several majors registered, since a new major is a different tool, so
    /// this returns all of them rather than one. Ordered by identity, so the order is stable across
    /// calls and a test can assert on it.
    #[must_use]
    pub fn by_capability(&self, capability: &ToolCapability) -> Vec<&RegisteredTool> {
        self.by_identity
            .iter()
            .filter(|(identity, _)| &identity.capability == capability)
            .map(|(_, tool)| tool)
            .collect()
    }

    /// Returns which server claimed a source identity, if any.
    #[must_use]
    pub fn source_claimant(&self, source: &ToolSource) -> Option<&ServerConfigId> {
        self.source_claims.get(source)
    }

    /// Returns every registered tool, ordered by identity.
    ///
    /// **Unfiltered, and named so.** This is what a discovery answer for one scope is computed
    /// *from*; it is not what a client is shown. Returning it under the name `catalog` would
    /// invite exactly the mistake this module exists to prevent — answering a scoped question with
    /// the unscoped set — so the name says which set it is.
    #[must_use]
    pub fn all_unfiltered(&self) -> Vec<&RegisteredTool> {
        self.by_identity.values().collect()
    }

    /// Returns every source kind currently registered.
    ///
    /// Used by an operator view and by a test that must prove an external source cannot displace a
    /// native one; a set rather than a list because the question is membership.
    #[must_use]
    pub fn registered_source_kinds(&self) -> BTreeSet<SourceKind> {
        self.by_identity
            .values()
            .map(|tool| tool.definition.identity.source.kind)
            .collect()
    }
}
