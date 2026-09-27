//! The canonical tool definition, and the invariants a usable one must satisfy.
//!
//! `TLS-001` asks for the schema, identity, origin, effects, scopes, risk, timeout, and retry
//! metadata of a capability. Those live in [`super::identity`] and [`super::classification`];
//! this module is the value that holds them together and the validation that makes the
//! combination meaningful rather than merely present.
//!
//! Three rules are enforced **across** fields, which is why they cannot live on any one
//! field's own type:
//!
//! 1. A definition that declares no effects has not declared `read_only` — it has declared
//!    nothing, and `read_only` is a claim about behaviour rather than the absence of a claim.
//!    So the list must be non-empty, and `read_only` may not be combined with any other
//!    effect, because a tool that says it reads and also writes is not read-only.
//! 2. A tool that can act outside JARVIS must not also claim a `Deny`-by-default posture with
//!    `Allow`'s consequences: a `Critical` risk tool whose default approval is `Allow` would
//!    invite policy to skip the one prompt the risk label exists to trigger.
//! 3. The capability's major version and the implementation's major version must agree. They
//!    are separate numbers — one changes the tool's identity, the other is a release — and a
//!    definition where `email.send@2` is implemented by version `1.4.0` has no coherent
//!    reading: either the capability's major is wrong, or the release is.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

use super::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, MAX_TOOL_EFFECTS,
    MAX_TOOL_SCOPES, Risk, Scope, is_display_name,
};
use super::identity::{ToolCapability, ToolIdentity, ToolSource, ToolVersion};

/// A canonical tool, as reviewed and trusted configuration declares it.
///
/// Constructed only through [`ToolDefinition::new`], so every instance satisfies the
/// cross-field rules above. Field visibility is public for reading — the fabric's policy layer
/// reads these — while construction is private to the module, which is what makes "a
/// `ToolDefinition` is always valid" true rather than aspirational.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The canonical identity: capability, source, and schema fingerprint.
    pub identity: ToolIdentity,
    /// A human-readable alias. Never authorization.
    pub display_name: String,
    /// The concise purpose shown to a user.
    pub purpose: String,
    /// Where the implementation was published from.
    pub source_version: ToolVersion,
    /// What the tool does.
    pub effects: Vec<Effect>,
    /// How dangerous that is.
    pub risk: Risk,
    /// The capability or resource scopes a caller must hold.
    pub required_scopes: Vec<Scope>,
    /// The approval posture when no grant exists. A hint, not a decision.
    pub default_approval: ApprovalHint,
    /// How a repeated call must be treated.
    pub idempotency: Idempotency,
    /// The sensitivity of arguments and results.
    pub data_classes: DataClasses,
    /// Bounded timeout and attempt defaults.
    pub execution: ExecutionDefaults,
}

impl ToolDefinition {
    /// Builds a reviewed definition, enforcing every cross-field rule.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the field at fault, or
    /// [`DomainError::ToolIdentifierNotCanonical`] when a display name is unusable. Each
    /// refusal names a field because a definition is assembled from several sources and "the
    /// definition is invalid" alone does not tell an operator where to look.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: ToolIdentity,
        display_name: &str,
        purpose: &str,
        effects: Vec<Effect>,
        risk: Risk,
        required_scopes: Vec<Scope>,
        default_approval: ApprovalHint,
        idempotency: Idempotency,
        data_classes: DataClasses,
        execution: ExecutionDefaults,
    ) -> Result<Self, DomainError> {
        if !is_display_name(display_name) {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "display_name",
            });
        }
        if purpose.is_empty() || purpose.chars().all(char::is_whitespace) {
            return Err(DomainError::ToolDefinitionInvalid { field: "purpose" });
        }
        if effects.is_empty() || effects.len() > MAX_TOOL_EFFECTS {
            // An empty list is refused rather than defaulted to `read_only`, because those are
            // different claims: defaulting would report a tool as safe because its author
            // forgot a field, and `read_only` is exactly the classification a reviewer trusts.
            return Err(DomainError::ToolDefinitionInvalid { field: "effects" });
        }
        let mut deduped = effects.clone();
        deduped.sort_unstable();
        deduped.dedup();
        if deduped.len() != effects.len() {
            return Err(DomainError::ToolDefinitionInvalid { field: "effects" });
        }
        if effects.contains(&Effect::ReadOnly) && effects.len() > 1 {
            // "Read and write" is a contradiction a policy branch would resolve by picking one.
            return Err(DomainError::ToolDefinitionInvalid { field: "effects" });
        }
        if required_scopes.len() > MAX_TOOL_SCOPES {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "required_scopes",
            });
        }
        let mut scopes = required_scopes.clone();
        scopes.sort();
        scopes.dedup();
        if scopes.len() != required_scopes.len() {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "required_scopes",
            });
        }
        // **A consequential tool may not default to `Allow`.** The risk label is a review
        // artifact; letting the same definition say "this moves money" and "do it without
        // asking" would make the label decorative. `ASK` and `DENY` are both fine here — the
        // rule refuses only the combination that removes the prompt.
        if required_scopes.is_empty()
            && default_approval == ApprovalHint::Allow
            && risk >= Risk::High
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "default_approval",
            });
        }
        // The capability's major is identity; the release's major is the release. An identity
        // at major 2 implemented by a major-1 release has no coherent reading.
        if identity.capability.major() != identity.source.version.major() {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "source.version",
            });
        }
        Ok(Self {
            source_version: identity.source.version.clone(),
            identity,
            display_name: display_name.to_owned(),
            purpose: purpose.to_owned(),
            effects,
            risk,
            required_scopes,
            default_approval,
            idempotency,
            data_classes,
            execution,
        })
    }

    /// Returns the canonical capability.
    #[must_use]
    pub fn capability(&self) -> &ToolCapability {
        &self.identity.capability
    }

    /// Returns the source the implementation came from.
    #[must_use]
    pub fn source(&self) -> &ToolSource {
        &self.identity.source
    }

    /// Returns whether this definition declares the given effect.
    #[must_use]
    pub fn has_effect(&self, effect: Effect) -> bool {
        self.effects.contains(&effect)
    }

    /// Returns whether any declared effect is consequential.
    ///
    /// Defined here rather than left to each caller so "is this tool consequential" has one
    /// answer, computed from the effects rather than from a separately maintained flag that
    /// could disagree with them.
    #[must_use]
    pub fn is_consequential(&self) -> bool {
        self.effects.iter().any(|effect| effect.is_consequential())
    }

    /// Returns whether this definition and `candidate` are the same tool.
    ///
    /// Delegates to [`ToolIdentity::authorizes`] rather than comparing capabilities, so a
    /// caller holding a grant checks the same tuple the grant was recorded against.
    #[must_use]
    pub fn is_same_tool_as(&self, candidate: &Self) -> bool {
        self.identity.authorizes(&candidate.identity)
    }
}

impl fmt::Display for ToolDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {})",
            self.capability(),
            self.display_name,
            self.risk
        )
    }
}
