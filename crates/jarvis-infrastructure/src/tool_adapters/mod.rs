//! Adapters that make the tool fabric reachable: the four ports [`ToolCallService`] needs.
//!
//! [`ToolCallService`]: jarvis_application::tool_call::ToolCallService
//!
//! `TLS-001` through `TLS-011` built the fabric's layers in `jarvis-domain` and left three of them
//! with **no production caller at all**: `ToolSchema::parse`/`validate`, `action_fingerprint`, and
//! `ToolRegistry`. This module is where each acquires its first one, and it exists as one module
//! rather than three so the wiring that connects them is visible in one place — the defect being
//! closed is "a value nothing consults", and scattering the consumers would hide whether the last
//! one is present.
//!
//! Four implementations, and each is deliberately thin:
//!
//! | Adapter | Port | What it adds |
//! |---|---|---|
//! | [`RegistryCatalog`] | `ToolCatalog` | a snapshot of registered definitions, keyed by capability |
//! | [`ConfiguredGrants`] | `ToolGrantSource` | grants and deny rules from reviewed configuration |
//! | [`SchemaValidator`] | `ToolArgumentValidator` | `ToolSchema::parse` + `confirms` + `validate` |
//! | [`FingerprintHasher`] | `ActionFingerprint` | `action_fingerprint` |
//!
//! The two that carry real behaviour are the last two, because they are where a **value** becomes a
//! **check**: the validator is the only thing that can refuse a call whose schema and identity
//! disagree, and the hasher is the only producer of the digest an approval binds to.

use std::collections::BTreeMap;
use std::sync::Arc;

use jarvis_application::repository::tool_grant::{StoredDenyRule, ToolGrantRepository};
use jarvis_application::tool_call::{
    ActionFingerprint, GrantRead, ResolvedTool, ToolArgumentRefusal, ToolArgumentValidator,
    ToolCatalog, ToolGrantSource,
};
use jarvis_domain::ids::{PrincipalId, WorkspaceId};
use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::canonical::{ActionDigest, FingerprintInput};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::identity::ToolIdentity;
use jarvis_domain::tool::policy::{DenyRule, Grant};

use crate::config::ReviewedDenyRule;
use crate::tool_fingerprint::action_fingerprint;
use crate::tool_schema::ToolSchema;

/// A catalog built from a snapshot of registered definitions.
///
/// **A snapshot rather than a live registry, and the choice is a boundary rather than a convenience.**
/// `ToolRegistry` is a domain value that is mutated by registration, and handing the service a
/// `Mutex<ToolRegistry>` would put a lock inside the request path and let a registration land between
/// two steps of one call. A snapshot is taken once and is immutable for the catalogue's lifetime, so
/// a call's resolved definition cannot change under it — which is what makes the fingerprint, the
/// grant, and the execution all describe the same tool.
///
/// A production reload swaps the whole catalog through whatever holds it, rather than mutating one
/// in place. That is the same shape the policy store uses for versions.
pub struct RegistryCatalog {
    by_capability: BTreeMap<String, ResolvedTool>,
}

impl RegistryCatalog {
    /// Builds a catalog from `(definition, schema)` pairs.
    ///
    /// The schema is `Option` because a definition may declare none — and the **service refuses such
    /// a tool rather than the catalog hiding it**, so an operator sees `tool.schema_absent` instead
    /// of a tool that mysteriously cannot be found.
    ///
    /// A later pair with the same capability replaces an earlier one, so a caller building from an
    /// iterator gets last-wins rather than a silent drop. The duplicate case is refused upstream by
    /// `ToolRegistry::register`; reaching it here means a caller bypassed the registry, and last-wins
    /// is the honest behaviour for a map being built in order.
    #[must_use]
    pub fn new(tools: impl IntoIterator<Item = (ToolDefinition, Option<String>)>) -> Self {
        let mut by_capability = BTreeMap::new();
        for (definition, input_schema) in tools {
            let capability = definition.identity.capability.to_string();
            by_capability.insert(
                capability,
                ResolvedTool {
                    definition,
                    input_schema,
                },
            );
        }
        Self { by_capability }
    }

    /// Returns how many tools the catalog can resolve.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_capability.len()
    }

    /// Returns whether the catalog is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_capability.is_empty()
    }
}

impl std::fmt::Debug for RegistryCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The definitions and their schemas are omitted: a schema is reviewed configuration but a
        // diagnostic line that dumps every one of them is noise, and the count is what a reader
        // needs to know whether the catalog was populated.
        formatter
            .debug_struct("RegistryCatalog")
            .field("tools", &self.by_capability.len())
            .finish_non_exhaustive()
    }
}

/// The longest tool description sent to a model, in characters.
const MAX_OFFER_DESCRIPTION_CHARS: usize = 512;

/// Bounds and cleans a definition's purpose for a prompt: control characters become spaces, whitespace is
/// collapsed, and the result is cut at [`MAX_OFFER_DESCRIPTION_CHARS`].
pub(crate) fn offer_description(purpose: &str) -> String {
    purpose
        .split(|character: char| character.is_control() || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_OFFER_DESCRIPTION_CHARS)
        .collect()
}

impl ToolCatalog for RegistryCatalog {
    fn resolve(&self, _workspace: WorkspaceId, capability: &str) -> Option<ResolvedTool> {
        // The scope is **accepted and not yet a filter**, and saying so here is better than a
        // signature that omits it: a catalog that could not receive a workspace could never be
        // scoped, while one that receives it and ignores it is a named gap. Workspace-scoped
        // visibility arrives with the connector and plugin sources that publish per-workspace
        // tools (`TLS-014`/`TLS-015`); every native tool is visible in every workspace because it
        // is a property of the daemon rather than of a workspace.
        self.by_capability.get(capability).cloned()
    }

    fn capabilities(&self) -> Vec<String> {
        // `BTreeMap` order, so the list is stable across calls and a test can assert on it — the
        // same reason `ToolRegistry::all_unfiltered` is ordered by identity.
        self.by_capability.keys().cloned().collect()
    }

    fn offers(&self) -> Vec<jarvis_domain::model::stream::ToolOffer> {
        // The definition's own purpose and the schema its identity binds, in the same order as
        // `capabilities`. The purpose of an MCP tool is **text a server wrote**, so it is bounded and
        // stripped of control characters here: it goes into a model prompt, and a server must not be
        // able to smuggle an instruction block or an escape sequence in through a tool description.
        self.by_capability
            .iter()
            .map(|(name, tool)| jarvis_domain::model::stream::ToolOffer {
                name: name.clone(),
                description: offer_description(&tool.definition.purpose),
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    }
}

/// Grants and deny rules from reviewed configuration.
///
/// **In-memory and immutable**, which is honest about what exists: there is no grant store until
/// `TLS-015`, and an adapter that read an empty table would be indistinguishable from a policy that
/// refuses everything. The grants a deployment holds are configuration, and this type is the seam a
/// durable store replaces without changing the service.
///
/// The deny rules are held **even though a grant is still required**, because a deny rule is the one
/// input that can refuse an action a grant would otherwise allow. A source that dropped them would
/// silently make every explicit refusal advisory.
#[derive(Debug, Default)]
pub struct ConfiguredGrants {
    grants: Vec<Grant>,
    deny_rules: Vec<DenyRule>,
}

impl ConfiguredGrants {
    /// Builds a source from the grants and deny rules in force.
    #[must_use]
    pub fn new(grants: Vec<Grant>, deny_rules: Vec<DenyRule>) -> Self {
        Self { grants, deny_rules }
    }

    /// Builds a source that grants nothing.
    ///
    /// The **fail-closed default**, named rather than left to `Default` so a caller reading the
    /// construction site can see that nothing was granted on purpose rather than by omission.
    #[must_use]
    pub fn denying_all() -> Self {
        Self::default()
    }
}

impl ToolGrantSource for ConfiguredGrants {
    fn read(
        &self,
        principal: PrincipalId,
        workspace: WorkspaceId,
    ) -> jarvis_application::tool_call::GrantReadFuture<'_> {
        // Filtered by the two dimensions the *source* can decide, and no further: the identity, the
        // scopes, the effects, and the ceiling are the evaluator's to match, and pre-matching them
        // here would be a second authorization rule beside the domain's — the defect the policy
        // module's own doc names.
        let grants = self
            .grants
            .iter()
            .filter(|grant| grant.principal == principal && grant.workspace == workspace)
            .cloned()
            .collect();
        // A deny rule may name no workspace at all, in which case it applies everywhere, so
        // filtering by workspace here would drop the broadest refusals — the direction that loses a
        // restriction. The evaluator already treats an unnamed dimension as "not a constraint".
        let deny_rules = self.deny_rules.clone();
        Box::pin(async move { Ok(GrantRead { grants, deny_rules }) })
    }
}

/// Grants for the daemon's own **read-only, low-risk, native** tools.
///
/// # Why this exists at all
///
/// The policy evaluator requires a grant before an action is allowed; the read-only-and-low-risk
/// fast path is reachable only **through** a grant, because a tool that skipped the grant requirement
/// would be the fail-open direction the whole evaluator exists to prevent. So a daemon that ships a
/// native tool must hold a grant for it, and until `TLS-015` provides a grant store the grant is
/// **reviewed configuration** — the same thing the tool's definition is.
///
/// # What it grants, and what it deliberately does not
///
/// A grant is emitted **only** for a definition that satisfies all three of:
///
/// - its effects are exactly `ReadOnly` — no other effect may be present, so a tool that reads and
///   also writes does not qualify;
/// - its risk is `Low`; and
/// - its source kind is `Native`, so an MCP server, connector, plugin, or external runtime tool
///   cannot be granted by shipping it.
///
/// The emitted grant repeats those bounds as its own ceiling (`effects = {ReadOnly}`,
/// `risk_ceiling = Low`), so the evaluator's `failed_constraints` re-checks them per call rather than
/// trusting this constructor. A definition that changed after the grant was built would then be
/// refused rather than silently covered — the same "the identity moved, so the grant no longer
/// applies" property a store would give, reached through the ceiling.
///
/// **The principal is taken from the request**, and that is the one dimension this adapter cannot
/// name ahead of time: a local profile's principal is derived from the authenticated client's id, so
/// there is no principal to write into a static configuration. Emitting the grant for the resolved
/// principal states the rule the local profile actually has — *any authenticated principal of this
/// profile may use the daemon's own read-only tools* — rather than pretending a per-principal grant
/// exists. `TLS-015` replaces this with a store that names principals explicitly; until then the
/// restriction lives in the three conditions above, which is what keeps the exception narrow.
///
/// A **deny rule still wins**: this source reports the rules it was built with alongside the grants,
/// so an operator can refuse one of these tools and the evaluator consults that first.
#[derive(Debug)]
pub struct NativeReadOnlyGrants {
    tools: Vec<ResolvedTool>,
    /// The operator's reviewed refusals, paired with the capability each names.
    ///
    /// **A pair rather than a bare `DenyRule`**, because the domain's rule type has no capability field — a
    /// rule stored by *identity* would stop refusing after a tool was recompiled, which is the direction that
    /// loses a restriction. The capability is kept beside the rule and expanded into the identities that
    /// currently offer it when the source is read.
    deny_rules: Vec<ReviewedDenyRule>,
}

impl NativeReadOnlyGrants {
    /// Builds the grant source over the daemon's catalog.
    ///
    /// Takes the tools rather than reading a registry, so the grants describe the **same** definitions
    /// the catalog resolves. A second list would let a tool be dispatchable while ungranted, or
    /// granted while unresolvable, and both read as a JARVIS fault rather than as configuration.
    #[must_use]
    pub fn new(tools: Vec<ResolvedTool>) -> Self {
        Self {
            tools,
            deny_rules: Vec::new(),
        }
    }

    /// Returns this source with the operator's deny rules attached.
    ///
    /// Takes [`ReviewedDenyRule`]s — the validated configuration form — rather than bare `DenyRule`s, so a
    /// capability-keyed refusal is expanded into the identities that currently offer it. A bare rule could
    /// only name an identity, and a reviewed refusal that named one would stop applying after a tool was
    /// recompiled, so accepting the pair is what makes the reviewed form able to express a refusal at all.
    #[must_use]
    pub fn with_deny_rules(mut self, deny_rules: Vec<ReviewedDenyRule>) -> Self {
        self.deny_rules = deny_rules;
        self
    }

    /// Returns whether a definition is one this source grants.
    ///
    /// One predicate with one home, so the constructor's doc and the filter cannot disagree — the rule
    /// is stated once and read by the filter rather than paraphrased in each.
    #[must_use]
    pub fn qualifies(definition: &ToolDefinition) -> bool {
        definition.identity.source.kind == jarvis_domain::tool::identity::SourceKind::Native
            && definition.risk == jarvis_domain::tool::classification::Risk::Low
            && definition.effects.len() == 1
            && definition
                .effects
                .contains(&jarvis_domain::tool::classification::Effect::ReadOnly)
    }

    /// Returns how many tools this source grants.
    #[must_use]
    pub fn granted_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|tool| Self::qualifies(&tool.definition))
            .count()
    }
}

impl ToolGrantSource for NativeReadOnlyGrants {
    fn read(
        &self,
        principal: PrincipalId,
        workspace: WorkspaceId,
    ) -> jarvis_application::tool_call::GrantReadFuture<'_> {
        let grants: Vec<Grant> = self
            .tools
            .iter()
            .filter(|tool| Self::qualifies(&tool.definition))
            .map(|tool| Grant {
                identity: tool.definition.identity.clone(),
                workspace,
                principal,
                // The tool's own declared scopes, so a read-only tool that *does* require a scope still
                // requires it — the grant confers what the definition asks for and no more.
                scopes: tool.definition.required_scopes.iter().cloned().collect(),
                // **The two ceilings, restated rather than inherited.** The evaluator checks the
                // request's effects and risk against these on every call, so a definition replaced
                // after this grant was built is refused by the ceiling even though the identity
                // matched — which is the one property a per-definition grant would otherwise lose.
                effects: [jarvis_domain::tool::classification::Effect::ReadOnly]
                    .into_iter()
                    .collect(),
                risk_ceiling: jarvis_domain::tool::classification::Risk::Low,
                sensitivity_ceiling: tool.definition.data_classes.input,
                // No expiry, because a standing grant for a read-only tool is exactly the case the
                // domain's own `Grant::expires_at` doc names as legitimate.
                expires_at: None,
            })
            .collect();
        let deny_rules = self.reviewed_deny_rules();
        // Answered without an await, because the reviewed grants are compiled configuration — an
        // in-process list. The async signature is the port's, so this adapter pays one boxing per call
        // and no I/O, which is the right trade for a source that will be replaced by the store.
        Box::pin(async move { Ok(GrantRead { grants, deny_rules }) })
    }
}

impl NativeReadOnlyGrants {
    /// Expands every reviewed refusal into the identity-keyed rules the evaluator applies.
    ///
    /// A method rather than an inline expression in `read`, because the expansion needs the catalog's
    /// identities grouped by capability and building that map per call would repeat work on a path taken on
    /// every dispatch. It is rebuilt here from `tools` rather than stored, because the reviewed refusals are
    /// fixed configuration while the tool list is the source's whole reason to exist — a stored map would be
    /// a second representation of the same list.
    fn reviewed_deny_rules(&self) -> Vec<DenyRule> {
        if self.deny_rules.is_empty() {
            return Vec::new();
        }
        let mut identities_by_capability: BTreeMap<String, Vec<ToolIdentity>> = BTreeMap::new();
        for tool in &self.tools {
            identities_by_capability
                .entry(tool.definition.identity.capability.to_string())
                .or_default()
                .push(tool.definition.identity.clone());
        }
        self.deny_rules
            .iter()
            .flat_map(|reviewed| {
                expand_deny_rule(
                    reviewed.capability.as_deref(),
                    reviewed.rule.clone(),
                    &identities_by_capability,
                )
            })
            .collect()
    }
}

/// A grant source over the durable grant store, with the reviewed grants as a fallback.
///
/// **This is the adapter that makes tool authorization configurable**, and the sentence it exists to make
/// true is the user-facing one: *what a deployment allows is a row an operator can write, not a
/// constructor.* Before it, `NativeReadOnlyGrants` was the only source, so "always ask about `email.send`
/// but never ask about `clock.now`" could be expressed only by changing code.
///
/// # The fallback, stated explicitly because it is a policy decision rather than an oversight
///
/// A stored grant **replaces** the reviewed defaults for that principal; with none stored, the defaults
/// apply. The rule is chosen rather than incidental, and both alternatives are worse:
///
/// - **"Stored grants only"** would make a fresh profile allow nothing at all — including the daemon's own
///   read-only clock tool — so a new install's first tool call would be refused with `NoGrant` and the
///   only way to fix it would be to author a grant before the product worked. That reads as a broken
///   install rather than as a policy.
/// - **"Union of both"** would make the reviewed grants unauditable: an operator who wrote a narrow grant
///   for `clock.now` could not *remove* the broad default, because the default would still be there — and
///   a refusal an operator cannot make is worse than a default they have to change.
///
/// So the reviewed grants are the **default posture of an unconfigured principal**, and the first stored
/// grant switches that principal to explicit configuration. The transition is per-principal, so two
/// principals of one workspace are configured independently.
///
/// # A store failure is an error, not a fallback
///
/// The read is propagated rather than degraded, and the direction matters: answering "use the defaults"
/// when the store is unreachable would **re-authorize** a principal an operator had narrowed, and
/// answering "nothing" would refuse work they had configured. Neither is true, so the call is reported as
/// a storage fault — an operator sees a broken store, which is what it is.
pub struct StoredGrants {
    store: Arc<dyn ToolGrantRepository>,
    defaults: NativeReadOnlyGrants,
    /// The catalog's identities, grouped by capability.
    ///
    /// **Needed because a stored deny rule names a capability and the evaluator compares identities.** A
    /// rule that refused "every tool with the `destructive` effect" is stored by effect alone and needs no
    /// expansion, but a rule naming one capability must become one rule per identity that currently offers
    /// it — and a capability nothing offers expands to nothing, correctly, because a refusal for a tool
    /// that is not installed has no call to refuse.
    ///
    /// Grouping here rather than querying the catalog per rule keeps the conversion a pure function of
    /// values, and it means the expansion sees **the same definitions the catalog resolves** — a second
    /// list would let a rule refuse an identity the daemon can no longer dispatch, which reads as a
    /// refusal that does nothing.
    identities_by_capability: BTreeMap<String, Vec<ToolIdentity>>,
}

impl StoredGrants {
    /// Builds a source over the store, with `defaults` as the unconfigured posture.
    ///
    /// Takes the tools as well as the store because the deny-rule conversion needs the catalog's
    /// identities; see [`Self::identities_by_capability`] for why the expansion cannot be done from the
    /// rule alone.
    #[must_use]
    pub fn new(
        store: Arc<dyn ToolGrantRepository>,
        defaults: NativeReadOnlyGrants,
        tools: &[ResolvedTool],
    ) -> Self {
        let mut identities_by_capability: BTreeMap<String, Vec<ToolIdentity>> = BTreeMap::new();
        for tool in tools {
            identities_by_capability
                .entry(tool.definition.identity.capability.to_string())
                .or_default()
                .push(tool.definition.identity.clone());
        }
        Self {
            store,
            defaults,
            identities_by_capability,
        }
    }

    /// Expands one stored rule into the rules the evaluator can apply.
    ///
    /// Delegates to the shared [`expand_deny_rule`], so a stored refusal and a reviewed one cannot be
    /// expanded differently — see that function for the four shapes and why each matters.
    fn expand_rule(&self, stored: StoredDenyRule) -> Vec<DenyRule> {
        expand_deny_rule(
            stored.capability.as_deref(),
            stored.rule,
            &self.identities_by_capability,
        )
    }
}

/// Expands one capability-keyed refusal into the identity-keyed rules the evaluator applies.
///
/// **A free function rather than a method, because there are two callers and the rule must have one home.**
/// A *stored* rule arrives as a `StoredDenyRule` and a *reviewed* rule as a `ReviewedDenyRule` — two types
/// because one carries a database identity and the other the operator's reason — but the expansion they need
/// is identical, and it is the part where a mistake is silent: a rule expanded to the wrong identities still
/// matches nothing and still looks applied. Two copies would be two chances to get that wrong, and the
/// second copy would be the one written later.
///
/// Four shapes, and each is a different statement:
///
/// - **a capability and effects** — one rule per identity offering that capability, each carrying the
///   effects, so the refusal is scoped to the tool the operator named *and* the effects they named;
/// - **a capability alone** — one rule per identity offering it, with no effect constraint, which refuses
///   that tool entirely;
/// - **no capability** — the rule already names only the dimensions the domain's `DenyRule` carries, so it
///   is passed through unchanged. This is the "refuse every destructive tool" form and it must **not** be
///   expanded, or it would become a rule per tool;
/// - **a capability nothing offers** — zero rules, which is the honest reading rather than an oversight. The
///   alternative, keeping a rule with no identity, would be one the evaluator's `matches` reports as naming
///   nothing: the same outcome reached by a path a reader cannot check, and invisible in a listing as an
///   active refusal.
fn expand_deny_rule(
    capability: Option<&str>,
    mut rule: DenyRule,
    identities_by_capability: &BTreeMap<String, Vec<ToolIdentity>>,
) -> Vec<DenyRule> {
    let Some(capability) = capability else {
        return vec![rule];
    };
    let Some(identities) = identities_by_capability.get(capability) else {
        return Vec::new();
    };
    identities
        .iter()
        .map(|identity| DenyRule {
            identity: Some(identity.clone()),
            // The rule's own scope is **taken** rather than rebuilt, so the principal and workspace the
            // caller supplied survive the expansion. A reviewed rule carries `None` for both — a
            // configuration file that could name a principal would make one profile's refusal another's —
            // while a stored rule may name one, and neither may be lost here.
            principal: rule.principal.take(),
            workspace: rule.workspace.take(),
            effects: rule.effects.clone(),
        })
        .collect()
}

impl std::fmt::Debug for StoredGrants {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The store is not printed: it holds a connection pool, and the interesting fact is the type.
        formatter
            .debug_struct("StoredGrants")
            .field("default_tools", &self.defaults.granted_count())
            .finish_non_exhaustive()
    }
}

impl ToolGrantSource for StoredGrants {
    fn read(
        &self,
        principal: PrincipalId,
        workspace: WorkspaceId,
    ) -> jarvis_application::tool_call::GrantReadFuture<'_> {
        Box::pin(async move {
            // **One instant for both reads.** The grants and the deny rules are read for one decision, and
            // two independent reads could observe the store at two instants — a grant written between them
            // would be seen while its matching refusal was not. `GrantRead` exists to make that
            // unrepresentable, so the pair is assembled here rather than returned as two calls.
            let grants = self.store.grants_for(workspace, principal).await?;
            // The stored rules are capability-keyed, and the evaluator compares identities — so a rule
            // naming a capability must be narrowed to the identities that currently offer it. The
            // reviewed rules are expanded the **same way**, through the same function, so the two sources
            // cannot be converted differently: see `expand_deny_rule`.
            let mut deny_rules = self.defaults.reviewed_deny_rules();
            for stored in self.store.deny_rules(workspace).await? {
                deny_rules.extend(self.expand_rule(stored));
            }
            // The stored grants **replace** the defaults for this principal; see the type's doc.
            let grants = if grants.is_empty() {
                // Answered through the trait so the defaults and the store cannot build a grant two
                // different ways — a second construction site is how the two would drift about expiry.
                match self.defaults.read(principal, workspace).await {
                    Ok(read) => read.grants,
                    Err(_) => Vec::new(),
                }
            } else {
                grants
            };
            Ok(GrantRead { grants, deny_rules })
        })
    }
}

/// Validates arguments against a tool's schema and binds the schema to its identity.
///
/// **This is the adapter that finally calls `ToolSchema`**, and it performs the three checks the
/// module was written for in the order that makes each meaningful:
///
/// 1. **the identity is confirmed** — `ToolSchema::confirms` recomputes the fingerprint of the
///    schema text and refuses if it disagrees with the definition's stated one, so a call cannot be
///    validated against a looser document than the approval was recorded over (`BRN-063`'s gap);
/// 2. **the schema parses** — a document this build cannot honour is a configuration defect, and
///    refusing it here reports that rather than admitting a call the schema would not have allowed;
/// 3. **the arguments validate** — the actual schema check.
///
/// The ordering is the part that matters. Validating first would produce "your arguments are wrong"
/// for a tool whose schema is not the one its identity names, which is a diagnosis pointing at a
/// model's arguments instead of at a packaging fault — the same "wrong layer reports it" defect as
/// recording a terminal call without its outcome.
pub struct SchemaValidator;

impl SchemaValidator {
    /// Returns a validator.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for SchemaValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolArgumentValidator for SchemaValidator {
    fn validate(
        &self,
        tool: &ResolvedTool,
        arguments: &ToolArguments,
    ) -> Result<(), ToolArgumentRefusal> {
        // A tool with no schema is **refused**, not accepted. The pipeline's step is "validate", and
        // a tool that skips it would be the one capability in the product whose arguments nothing
        // checked — the fail-open direction a validation boundary must not have.
        let Some(text) = tool.input_schema.as_deref() else {
            return Err(ToolArgumentRefusal::NoSchema);
        };
        // The parse cannot fail for a document that was already parsed into the catalog, but the
        // adapter does not assume that: a catalog may be built from a raw string, and a schema this
        // build cannot honour must be refused rather than skipped.
        let schema = ToolSchema::parse(text).map_err(|_| ToolArgumentRefusal::NoSchema)?;
        // **The pair check.** A schema and a fingerprint that disagree mean the identity an approval
        // binds to names rules that are not the ones in force, which is `ACC-024` reached by a pair
        // of values rather than by a replaced name.
        schema
            .confirms(&tool.definition.identity.schema_fingerprint)
            .map_err(|_| ToolArgumentRefusal::FingerprintMismatch)?;
        schema
            .validate(arguments)
            .map_err(|_| ToolArgumentRefusal::Violated)
    }
}

/// Computes the action digest an approval binds to.
///
/// The whole adapter is one function call, and that is the point: `action_fingerprint` performs the
/// RFC 8785 canonicalization and the SHA-256 hash together, so a caller cannot perform one and not
/// the other. It is the first and only production caller of that function.
pub struct FingerprintHasher;

impl FingerprintHasher {
    /// Returns a hasher.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for FingerprintHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionFingerprint for FingerprintHasher {
    fn fingerprint(&self, input: &FingerprintInput) -> ActionDigest {
        action_fingerprint(input)
    }
}

/// Routing by source kind, the answer to the pipeline's single executor slot.
///
/// Declared last, beside the other adapters, so the module's shape reads as "the pieces the composition
/// needs, then the one that chooses between them".
pub mod routing;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
