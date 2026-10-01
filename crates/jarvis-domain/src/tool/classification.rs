//! Tool effects, risk, scopes, and the approval posture a definition declares.
//!
//! The tool contract separates two things that are easy to conflate: what a tool *does*
//! (its effects) and how dangerous that is (its risk). Both come from trusted reviewed
//! configuration, never from a model's description of the tool, and both are types here
//! rather than strings so a policy branch cannot be selected by a typo.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::model::policy::Sensitivity;

use super::identity::MAX_TOOL_SEGMENT_BYTES;

/// The largest number of effects one definition may declare.
///
/// A bound because the set is per-definition and reaches policy: an unbounded list would be
/// an unbounded classification, and the executor's per-effect checks would scale with it.
pub const MAX_TOOL_EFFECTS: usize = 8;

/// The largest number of required scopes one definition may declare.
pub const MAX_TOOL_SCOPES: usize = 16;

/// The longest accepted scope name.
pub const MAX_SCOPE_BYTES: usize = 128;

/// One base effect a tool can have.
///
/// The set is the contract's, and it is closed: **an unknown effect fails closed**, so the
/// parser refuses a value outside this list rather than ignoring it. Ignoring one would mean
/// a definition declaring an effect policy does not know about is classified as though it
/// declared nothing, which is the direction that loses a restriction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Reads data without changing anything.
    ReadOnly,
    /// Writes to a store JARVIS or a connector owns.
    Write,
    /// Sends something to a person or an external service.
    ExternalCommunication,
    /// Deletes or irreversibly changes data.
    Destructive,
    /// Executes code.
    CodeExecution,
    /// Moves money or creates a financial obligation.
    Financial,
    /// Changes authorization, credentials, or system configuration.
    Privileged,
    /// Acts on the physical world.
    Physical,
}

impl Effect {
    /// Every effect, in contract order.
    pub const ALL: &'static [Self] = &[
        Self::ReadOnly,
        Self::Write,
        Self::ExternalCommunication,
        Self::Destructive,
        Self::CodeExecution,
        Self::Financial,
        Self::Privileged,
        Self::Physical,
    ];

    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `effects` field for anything
    /// outside the closed set. The error names the field rather than the offending value
    /// because the value is untrusted input and may not be safe to echo.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .iter()
            .copied()
            .find(|effect| effect.as_contract_str() == value)
            .ok_or(DomainError::ToolDefinitionInvalid { field: "effects" })
    }

    /// Returns the spelling the tool contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Write => "write",
            Self::ExternalCommunication => "external_communication",
            Self::Destructive => "destructive",
            Self::CodeExecution => "code_execution",
            Self::Financial => "financial",
            Self::Privileged => "privileged",
            Self::Physical => "physical",
        }
    }

    /// Returns whether policy should treat this effect as irreversible or high-consequence.
    ///
    /// The classifier exists so "which effects are consequential" has one definition. A
    /// caller writing its own list would drift: adding an effect here would then leave that
    /// caller quietly treating it as benign.
    #[must_use]
    pub const fn is_consequential(self) -> bool {
        matches!(
            self,
            Self::Destructive | Self::Financial | Self::Privileged | Self::Physical
        )
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// A tool's risk label.
///
/// The contract is explicit that this is not the decision: "policy derives final decision from
/// more than this label". It is a *hint* the owner reviews, ordered so a comparison is
/// meaningful when a definition and a policy ceiling are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// No meaningful consequence.
    Low,
    /// Recoverable, bounded consequence.
    Moderate,
    /// Hard to reverse or wide-reaching.
    High,
    /// Irreversible, financial, or privileged.
    Critical,
}

impl Risk {
    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `risk` field for any other
    /// value. An unrecognised risk must not become `Low`.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "low" => Ok(Self::Low),
            "moderate" => Ok(Self::Moderate),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            _ => Err(DomainError::ToolDefinitionInvalid { field: "risk" }),
        }
    }

    /// Returns the spelling the tool contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Moderate => "moderate",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// A capability or resource scope a caller must hold.
///
/// Bounded text rather than a free `String` so a scope from an MCP server's manifest cannot
/// carry control characters or unbounded length into a persisted grant.
///
/// **`Deserialize` is hand-written to go through [`Self::new`]**, because the derived one would wrap
/// whatever string arrived and produce a scope the constructor would have refused. That makes the
/// bound "true for values this crate built and false for values that came over the wire", which is the
/// direction an attacker chooses — see `WorkspaceRelativePath` in this module for the same fix and the
/// full reasoning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Scope(String);

impl Scope {
    /// Validates and wraps a scope name.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming the `required_scopes` field when
    /// the value is empty, over-long, or contains anything outside lowercase letters, digits,
    /// dots, underscores, and hyphens. A scope reaches a grant record and an operator display,
    /// so a value carrying whitespace or control characters is refused at the boundary rather
    /// than escaped later in several places.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        let usable = !value.is_empty()
            && value.len() <= MAX_SCOPE_BYTES
            && value.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '.'
                    || character == '_'
                    || character == '-'
            });
        if !usable {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "required_scopes",
            });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the scope text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

/// What the definition says should happen without an existing grant.
///
/// The contract is emphatic that this is "a default policy hint, not authorization", and the
/// type name says so. It is deliberately **not** an `ALLOW`/`ASK`/`DENY` decision: a decision
/// is policy's output and carries reason codes and constraints, while this is one input to it.
/// Naming it `Decision` would invite a caller to use it as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalHint {
    /// Policy may allow this without asking, if nothing else constrains it.
    Allow,
    /// Ask the user unless a standing grant covers the exact action.
    Ask,
    /// Refuse unless an explicit grant exists.
    Deny,
}

impl ApprovalHint {
    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `default_approval`. An
    /// unrecognised value must not become `Allow`, which is the fail-open direction.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            _ => Err(DomainError::ToolDefinitionInvalid {
                field: "default_approval",
            }),
        }
    }

    /// Returns the spelling the tool contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// How a repeated call should be treated.
///
/// The contract says a definition declares `none`, `caller-keyed`, or `naturally idempotent`,
/// and the distinction is what the executor uses to decide whether a reservation is needed.
/// `None` is the **default**, because a definition that does not say cannot be assumed safe to
/// repeat: assuming otherwise would skip a reservation for a side effect nobody declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Idempotency {
    /// Not idempotent; a caller key is required before the effect.
    None,
    /// Safe to repeat when the caller supplies the same key.
    CallerKeyed,
    /// Safe to repeat with no key, because the operation converges.
    NaturallyIdempotent,
}

impl Idempotency {
    /// Parses the contract spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `idempotency`. An unrecognised
    /// value must not become `NaturallyIdempotent`, which would skip the reservation.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "none" => Ok(Self::None),
            "caller_keyed" => Ok(Self::CallerKeyed),
            "naturally_idempotent" => Ok(Self::NaturallyIdempotent),
            _ => Err(DomainError::ToolDefinitionInvalid {
                field: "idempotency",
            }),
        }
    }

    /// Returns the spelling the tool contract uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::CallerKeyed => "caller_keyed",
            Self::NaturallyIdempotent => "naturally_idempotent",
        }
    }

    /// Returns whether the executor must reserve a caller key before the effect.
    #[must_use]
    pub const fn requires_caller_key(self) -> bool {
        matches!(self, Self::CallerKeyed)
    }
}

impl fmt::Display for Idempotency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// The classification of a tool's input and output data.
///
/// Reuses the model gateway's [`Sensitivity`] rather than defining a second ladder. The two
/// would drift, and the drift is silent: a tool whose output was classified on one ladder and
/// sent to a model governed by the other would compare `Confidential` against `Confidential`
/// spelled by a different type and get *nothing*, or compare two integers that happen to be
/// equal and get *permission*. One ladder means the comparison is meaningful by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataClasses {
    /// The classification of the arguments a caller supplies.
    pub input: Sensitivity,
    /// The classification of the result the tool returns.
    pub output: Sensitivity,
}

impl DataClasses {
    /// Builds a pair from both sensitivities.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `data_classes` when the output is
    /// classified **below** the input. A tool that reads restricted data can derive a less
    /// sensitive summary, so raising the output label above the input is legitimate; lowering
    /// it is not derivable from the input alone and would let a tool launder a classification
    /// by declaring a smaller one. Refusing the direction makes that unrepresentable.
    pub fn new(input: Sensitivity, output: Sensitivity) -> Result<Self, DomainError> {
        if output < input {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "data_classes",
            });
        }
        Ok(Self { input, output })
    }
}

impl<'de> Deserialize<'de> for DataClasses {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            input: Sensitivity,
            output: Sensitivity,
        }
        let classes = Wire::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived.** The cross-field rule — output never below
        // input — is what stops a tool laundering a classification, and a derived impl rebuilt the
        // forbidden pair for any `ToolDefinition` read from a document.
        Self::new(classes.input, classes.output).map_err(serde::de::Error::custom)
    }
}

/// The bounded defaults a definition declares for time and retry.
///
/// Bounded here for the same reason `RunBudget` bounds its own: an unbounded timeout is an
/// unbounded wait. These are the definition's *defaults*; a run's budget can narrow them and
/// never widen them, which is why the upper bounds are checked at construction rather than
/// at use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExecutionDefaults {
    /// The default call timeout in milliseconds.
    pub timeout_ms: u64,
    /// The greatest number of attempts the definition permits.
    pub max_attempts: u32,
}

/// The largest accepted tool timeout, in milliseconds (one hour).
///
/// Matches `jarvis_domain::run::budget::MAX_STEP_TIMEOUT_MS` deliberately: a tool may not
/// claim a longer default than a run step is allowed to take, because such a default could
/// never be honoured.
pub const MAX_TOOL_TIMEOUT_MS: u64 = 3_600_000;

/// The greatest number of attempts a definition may permit.
pub const MAX_TOOL_ATTEMPTS: u32 = 5;

impl ExecutionDefaults {
    /// Validates the defaults.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `timeout_ms` or `max_attempts`.
    /// A zero timeout and a zero attempt count are both refused because neither describes an
    /// execution: zero attempts means the call can never be made, and a zero timeout means it
    /// is refused before it starts.
    pub fn new(timeout_ms: u64, max_attempts: u32) -> Result<Self, DomainError> {
        if timeout_ms == 0 || timeout_ms > MAX_TOOL_TIMEOUT_MS {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "timeout_ms",
            });
        }
        if max_attempts == 0 || max_attempts > MAX_TOOL_ATTEMPTS {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "max_attempts",
            });
        }
        Ok(Self {
            timeout_ms,
            max_attempts,
        })
    }
}

impl<'de> Deserialize<'de> for ExecutionDefaults {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            timeout_ms: u64,
            max_attempts: u32,
        }
        let defaults = Wire::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived.** A zero timeout or a zero attempt count is not
        // an execution — the call would be refused before it started, or never made — and the bounds
        // are what keep a definition from claiming a longer default than a run step may take. A
        // derived impl rebuilt both forbidden values for any definition read from a document.
        Self::new(defaults.timeout_ms, defaults.max_attempts).map_err(serde::de::Error::custom)
    }
}

/// Returns whether a display name is usable.
///
/// Looser than an identity segment because a display name is an alias shown to a user: it may
/// contain uppercase letters and spaces. It is still bounded and control-character free,
/// because it reaches an approval prompt and a log line.
#[must_use]
pub fn is_display_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOOL_SEGMENT_BYTES * 2
        && value.chars().all(|character| !character.is_control())
}
