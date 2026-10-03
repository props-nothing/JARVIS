//! The tool-call service: one governed path from a model's proposal to an effect.
//!
//! This is the module that makes the tool fabric **reachable**. `TLS-001` through `TLS-011` built
//! every horizontal layer of the fabric — a canonical definition, a registry, a JSON-Schema
//! validator, deterministic policy, durable approvals, a durable ledger, rooted path grants — and
//! nothing joined them: `ToolRegistry` had no consumer, `ToolSchema::validate` had no production
//! caller, `action_fingerprint` had none either, and the run controller refused every tool intent
//! with `run.tools_not_implemented`. Each layer was complete and unreachable, which is this
//! project's most-repeated defect ("a value with a producer and no consumer") at the scale of a
//! whole milestone.
//!
//! [`ToolCallService::invoke`] is the single path the architecture requires:
//!
//! ```text
//! resolve  ->  validate  ->  fingerprint  ->  evaluate  ->  reserve  ->  execute  ->  record
//! ```
//!
//! Two rules from the tool fabric are structural here rather than documented:
//!
//! - **"Every executable capability is a canonical JARVIS tool ... pass through the same
//!   validation, policy, approval, idempotency, timeout, and audit pipeline."** There is one
//!   method, and every step is a call inside it. A second entry point would be a second pipeline,
//!   and the one a caller reached first would be the one that governed.
//! - **"`EXECUTING` is recorded before an external effect."** The ledger's `Executing` transition
//!   is committed **before** [`ToolExecutor::execute`] is called, so a crash mid-effect leaves a
//!   row that says an effect may have happened. Recording it afterwards — or in the same durable
//!   write as the outcome — would lose that fact, and the loss is a duplicated side effect on the
//!   next startup.
//!
//! ## What arrives through a port, and why
//!
//! Four of the pipeline's steps need a computation this layer must not perform:
//!
//! | Step | Port | Why not here |
//! |---|---|---|
//! | resolve | [`ToolCatalog`] | the registry is a domain value; a store of definitions is not |
//! | validate | [`ToolArgumentValidator`] | JSON Schema needs a JSON dependency `jarvis-domain` forbids |
//! | fingerprint | [`ActionFingerprint`] | hashing is a concrete implementation |
//! | execute | [`ToolExecutor`] | an effect is I/O, and this layer owns orchestration |
//!
//! Each port is narrow — one method — because a wide port is one a caller can satisfy while
//! disagreeing with the pipeline about what it means. The catalog returns a `ToolDefinition` and
//! nothing about permission; the validator answers one question and cannot return a grant.
//!
//! ## A refusal is an outcome, not an error
//!
//! [`ToolCallOutcome`] has a `Refused` arm carrying a [`ToolErrorClass`], and that is deliberate
//! rather than a `Result` shaped like one. A denied call, an unknown tool, and invalid arguments
//! are all **facts the model must be told about**, because a model that cannot learn "you are not
//! allowed to do that" will simply propose it again. They are not failures of this service, and
//! collapsing them into `Err` would make the controller report a run fault for a working refusal
//! — the same "never model a decision as an error" rule the reservation outcome records.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use jarvis_domain::clock::Clock;
use jarvis_domain::ids::{
    ApprovalId, PrincipalId, RunId, ToolCallId, ToolCallRecordId, WorkspaceId,
};
use jarvis_domain::run::budget::RunBudget;
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::approval::{
    AllowedChannels, ApprovalActor, ApprovalChannel, ApprovalPreview, ApprovalRequestParts,
    ApprovalScopeKind, ApprovalState, ApprovalSummary, DurableApproval, PreviewItem,
};
use jarvis_domain::tool::call::{ToolArguments, ToolCallIntent, ToolResultBody};
use jarvis_domain::tool::canonical::{
    ActionDigest, FINGERPRINT_FORMAT_VERSION, FingerprintInput, FingerprintParts,
};
use jarvis_domain::tool::classification::Effect;
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::ToolIdentity;
use jarvis_domain::tool::ledger::{
    LedgerEntry, LedgerOperation, ReservationKey, ReservationOutcome, ToolCallState,
    ToolCallVersion,
};
use jarvis_domain::tool::policy::{
    ApprovalRecord, DenyRule, Grant, PolicyInputs, PolicyOutcome, PolicyReason, PolicyRequest,
    evaluate,
};

use crate::cancellation::CancellationScope;
use crate::repository::RepositoryError;
use crate::repository::approval::ApprovalRepository;
use crate::repository::tool_call::ToolCallRepository;
use crate::request_context::RequestContext;

/// How long a requested approval may sit undecided before it lapses.
///
/// **A bound rather than an open prompt, and the reason is the one the approval domain records.** An
/// approval that never expires is a standing permission a user may not remember granting, and a
/// pending prompt with no deadline occupies the operator's listing forever. Fifteen minutes is
/// deliberately short: a prompt a user has not answered within it is one they were not present for,
/// and re-asking is cheap while a stale permission is not.
///
/// The value is passed to [`RunBudget::expiring_after`], so it is the domain that performs the
/// addition and enforces its own ceiling rather than this layer re-deriving time arithmetic — the
/// reason there is no direct `jiff` dependency here.
pub const APPROVAL_WINDOW_MS: u64 = 15 * 60 * 1000;

/// The future a [`ToolExecutor`] returns.
pub type ToolExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolResultBody, ToolExecutionError>> + Send + 'a>>;

/// A definition resolved for one call, together with the schema document its identity names.
///
/// **Two values rather than one, and the pair is the point.** A [`ToolDefinition`] carries a
/// `SchemaFingerprint` but not the schema text it was computed over, so a validator handed only the
/// definition could not check that the document it validated against is the one the identity binds —
/// which is `ACC-024`'s failure mode (`BRN-063`) reached by a pair of values that disagree rather
/// than by a display name. Carrying both here is what lets [`ToolArgumentValidator`] confirm them,
/// and it is why this type exists rather than the catalog returning a definition alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    /// The reviewed definition: identity, classification, and metadata.
    pub definition: ToolDefinition,
    /// The input schema document, when the tool declares one.
    ///
    /// `None` is a real state and is refused by the validator rather than accepted — a tool whose
    /// arguments nothing checks must not be invokable.
    pub input_schema: Option<String>,
}

impl ResolvedTool {
    /// Returns the canonical identity.
    #[must_use]
    pub fn identity(&self) -> &ToolIdentity {
        &self.definition.identity
    }
}

/// Resolves a tool name the model emitted to the definition and schema a call is authorized against.
///
/// **A name in, a [`ResolvedTool`] out**, because that is exactly the trust boundary: the model emits
/// a name, and a name is resolved against a registry rather than believed. The method returns `None`
/// for an unknown or out-of-scope name, which the service reports as `tool.not_found` — a case that
/// must be **representable**, because a model naming a tool that does not exist is an ordinary
/// occurrence rather than a malformed request.
///
/// The catalog answers **which definition** exists; it cannot answer whether a call is permitted,
/// because what it returns carries classification rather than authorization. That separation is
/// `TLS-002`'s rule ("tool discovery never grants execution permission") expressed as a signature: a
/// caller holding this port's answer still has to ask policy.
pub trait ToolCatalog: Send + Sync {
    /// Returns the resolved tool for `capability` in `workspace`, if one is discoverable there.
    ///
    /// `workspace` is a parameter rather than implied, so a catalog cannot answer across scopes by
    /// accident — the scoping rule every other repository port records.
    fn resolve(&self, workspace: WorkspaceId, capability: &str) -> Option<ResolvedTool>;

    /// Returns the canonical names of every tool the catalog can resolve.
    ///
    /// **The model has to be told what it may propose**, and the list must be the catalog's own
    /// rather than a second one: a name the model was offered but the service could not resolve
    /// would produce `tool.not_found` for a tool the model believes exists, which reads as a JARVIS
    /// bug rather than as a refusal. Ordering is the implementation's, and stable within one catalog.
    fn capabilities(&self) -> Vec<String>;

    /// Returns what the model should be told about each tool: its name, description, and argument schema.
    ///
    /// Defaults to names alone, so a catalog that knows nothing more is still correct; an implementation
    /// that holds definitions overrides it. The names are exactly [`Self::capabilities`], in the same order.
    fn offers(&self) -> Vec<jarvis_domain::model::stream::ToolOffer> {
        self.capabilities()
            .into_iter()
            .map(|name| jarvis_domain::model::stream::ToolOffer {
                name,
                description: String::new(),
                input_schema: None,
            })
            .collect()
    }
}

/// The future a [`ToolGrantSource`] returns.
///
/// A named alias rather than an inline `Pin<Box<dyn Future>>` at each declaration, so the port's two
/// methods cannot drift into two different shapes — the same reason [`ToolExecutionFuture`] exists.
pub type GrantReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GrantRead, RepositoryError>> + Send + 'a>>;

/// What one authorization read found.
///
/// **One value for the pair rather than two calls**, and the reason is consistency rather than tidiness:
/// the grants and the deny rules are read for the same decision, so two independent reads could observe
/// the store at two instants — with the consequence that a grant written between them would be seen while
/// its matching deny rule was not. A rule that refuses can only narrow, so the failure is a call refused
/// that the operator expected to be allowed, or worse the reverse if the reads were ordered the other way.
/// One read, one instant, one decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRead {
    /// The grants held by the principal, unscoped beyond principal and workspace.
    pub grants: Vec<Grant>,
    /// The deny rules that apply in the workspace.
    pub deny_rules: Vec<DenyRule>,
}

impl GrantRead {
    /// Returns a read that grants nothing and refuses nothing.
    ///
    /// The **fail-closed empty**, named rather than written inline so a caller reading a construction site
    /// can see that nothing was authorized on purpose. It is not a safe default for a source: an adapter
    /// that returned this on a read failure would report a broken store as a policy that denies everything,
    /// which is why [`ToolGrantSource::read`] returns a `Result` rather than this value on error.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            grants: Vec::new(),
            deny_rules: Vec::new(),
        }
    }
}

/// The grants and deny rules in force for one principal in one workspace.
///
/// **Asynchronous, because this reads durable state.** The port was synchronous while the only adapter
/// answered from compiled configuration — an in-process list. `TLS-015`'s grant store makes it a database
/// read, and the change is the port's rather than the adapter's: an adapter cannot make a synchronous
/// method asynchronous, so a store behind a synchronous port would have to block a runtime worker thread
/// (or bridge runtimes, which deadlocks under load). The signature follows the dependency, which is the
/// same reason `ToolCallRepository` is a port rather than an in-memory map.
///
/// It is deliberately **not** a repository over stored grants: choosing which grant matches is the policy
/// evaluator's job, and a source that pre-filtered could filter wrongly — the argument
/// `PolicyInputs::grants` records. What an adapter may do is answer the two dimensions it owns, principal
/// and workspace, and return the rest.
pub trait ToolGrantSource: Send + Sync {
    /// Returns the grants and deny rules in force for `principal` in `workspace`.
    ///
    /// **One method rather than two**, so a decision cannot see a grant written after the deny rules were
    /// read — see [`GrantRead`] for why that ordering matters.
    ///
    /// It returns **all** the grants, not the matching one: choosing the match is the policy evaluator's
    /// job, and a source that pre-filtered could filter wrongly — the argument `PolicyInputs::grants`
    /// records. What an adapter may do is answer the two dimensions it owns, principal and workspace.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when a durable store cannot be read. **A read failure is deliberately
    /// not "no grants"**: both lead to a refusal, but only one is a transient fault an operator should see,
    /// and collapsing them would make an unreachable store look like a policy that refuses everything.
    fn read(&self, principal: PrincipalId, workspace: WorkspaceId) -> GrantReadFuture<'_>;
}

/// Why an argument document could not be accepted.
///
/// Three variants rather than a boolean, because [`Self::NoSchema`] and
/// [`Self::FingerprintMismatch`] are **configuration gaps** rather than a caller's mistake, and an
/// operator reading a refusal needs to know which happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolArgumentRefusal {
    /// The arguments violate the tool's input schema.
    Violated,
    /// The tool declares no input schema, so its arguments could not be validated at all.
    ///
    /// **Refused rather than accepted**, and this is the one direction a validation boundary must
    /// not have. A tool with no schema is a tool whose arguments nothing checks, so accepting the
    /// call would mean the pipeline's "validate" step silently did nothing for exactly the tools
    /// that declared least.
    NoSchema,
    /// The schema document does not hash to the fingerprint the tool's identity carries.
    ///
    /// **An identity defect rather than a caller's mistake.** The identity is what an approval and a
    /// grant bind to, so a document that disagrees with its own fingerprint means the rules that
    /// decide acceptance are not the ones the approval was recorded against — `ACC-024`'s failure
    /// mode reached by a pair of values instead of by a replaced name.
    FingerprintMismatch,
}

impl ToolArgumentRefusal {
    /// Returns the tool error class this refusal maps to.
    ///
    /// A fingerprint mismatch is `Unavailable` rather than `SchemaInvalid`, and the difference is the
    /// one a caller acts on: `schema_invalid` tells a model to fix its arguments, while a tool whose
    /// schema and identity disagree is one **nobody** can invoke until an operator fixes it.
    /// Reporting the second as the first would send a model into a loop of rewording.
    #[must_use]
    pub const fn error_class(self) -> ToolErrorClass {
        match self {
            Self::Violated | Self::NoSchema => ToolErrorClass::SchemaInvalid,
            Self::FingerprintMismatch => ToolErrorClass::Unavailable,
        }
    }

    /// Returns a stable, namespaced code for operator output.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Violated => "tool.argument_violation",
            Self::NoSchema => "tool.schema_absent",
            Self::FingerprintMismatch => "tool.schema_fingerprint_mismatch",
        }
    }
}

/// Validates an argument document against a tool's declared input schema.
///
/// `jarvis-domain` forbids a JSON dependency, so the validator cannot live there — the decision
/// `jarvis_infrastructure::tool_schema` records. This port is the seam that lets the application
/// layer require validation without owning it, and it is the first production caller that module has
/// ever had.
pub trait ToolArgumentValidator: Send + Sync {
    /// Decides whether `arguments` satisfy the resolved tool's input schema.
    ///
    /// Takes the [`ResolvedTool`] rather than a definition or a schema alone: the definition supplies
    /// the fingerprint to check and the schema supplies the rules to apply, and an implementation
    /// that received only one could not perform the confirmation the pair exists for.
    ///
    /// # Errors
    ///
    /// Returns [`ToolArgumentRefusal`] when the document is refused. The refusal is not an error
    /// type because it is an expected outcome of a model's output.
    fn validate(
        &self,
        tool: &ResolvedTool,
        arguments: &ToolArguments,
    ) -> Result<(), ToolArgumentRefusal>;
}

/// Computes the digest that binds an approval to the exact action.
///
/// Takes the domain's own [`FingerprintInput`] rather than the raw facts, and that is the whole
/// point of the signature: the RFC 8785 canonical form is built **here**, so the port cannot be
/// handed something that was not canonicalized — the two-step pipeline
/// `jarvis_infrastructure::tool_fingerprint` documents, with the canonicalization on the side that
/// owns the specification and the hash behind the port.
pub trait ActionFingerprint: Send + Sync {
    /// Returns the digest of one action envelope.
    fn fingerprint(&self, input: &FingerprintInput) -> ActionDigest;
}

/// Why a validated, authorized call could not produce a result.
///
/// Three variants rather than one, because the caller's response differs for each and the
/// difference is load-bearing: a `Failed` call may be retried according to the tool's declaration,
/// an `Ambiguous` one **must not** be retried until it is reconciled, and a `Cancelled` one is the
/// caller's own action recorded as such rather than as a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionError {
    /// The tool failed. Whether a repeat is permissible is the error class's own answer.
    Failed(ToolErrorClass),
    /// The outcome is unknown, so the call must be reconciled rather than repeated.
    ///
    /// **Not a failure to report and move on.** The effect may or may not have happened, and only a
    /// read of the tool's own state can establish which. A caller that retried here would send
    /// twice.
    Ambiguous,
    /// The caller cancelled before or during the effect.
    Cancelled,
}

impl ToolExecutionError {
    /// Returns the class recorded on the ledger row.
    #[must_use]
    pub const fn error_class(self) -> ToolErrorClass {
        match self {
            Self::Failed(class) => class,
            Self::Ambiguous => ToolErrorClass::OutcomeAmbiguous,
            Self::Cancelled => ToolErrorClass::Cancelled,
        }
    }

    /// Returns the ledger state the call ends in.
    ///
    /// **`Ambiguous` ends `Failed` with an `OutcomeAmbiguous` class rather than staying
    /// `Reconciling`**, and the distinction matters: `Reconciling` is a state a recovery pass
    /// converts, while a row left in it forever looks unsettled to every reader including the
    /// reservation check. This build has no provider-side read to perform — the executor is local —
    /// so the honest record is "an effect may exist and nothing could establish it".
    #[must_use]
    pub const fn terminal_state(self) -> ToolCallState {
        match self {
            Self::Failed(_) | Self::Ambiguous => ToolCallState::Failed,
            Self::Cancelled => ToolCallState::Cancelled,
        }
    }
}

/// One call to execute: everything the executor needs and nothing that authorizes it.
///
/// **Carries no principal, workspace, or grant**, for the reason [`ToolCallIntent`] does not: those
/// are trusted context, and an executor holding them would be tempted to make an authorization
/// decision of its own. The identity is present because a tool must know *which* implementation it
/// is, and a scoped credential or handle is the adapter's business to resolve from it.
#[derive(Debug, Clone, Copy)]
pub struct ToolExecutionRequest<'a> {
    /// The canonical identity being invoked.
    pub identity: &'a ToolIdentity,
    /// The tool's display name, for a diagnostic the tool itself emits.
    pub display_name: &'a str,
    /// The validated argument document.
    pub arguments: &'a ToolArguments,
    /// When the call started, from JARVIS's clock.
    pub started_at: UtcTimestamp,
    /// The tool's declared timeout in milliseconds.
    pub timeout_ms: u64,
}

/// Performs the effect for one validated, authorized call.
///
/// The **only** port that touches the world on a call's behalf, and it is deliberately the last
/// step: every check that can refuse the call has already run, so an executor cannot be reached by
/// a call policy denied. That ordering is what makes "a denied model request cannot bypass policy
/// through native, MCP, or runtime routes" (`TLS-012`) a property of the pipeline rather than of
/// each executor's diligence — an executor is never *asked* about a denied call.
pub trait ToolExecutor: Send + Sync {
    /// Runs the tool and returns its bounded result.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecutionError`] when the tool failed, when its outcome is unknown, or when
    /// the caller cancelled. A success that exceeds the result bound is refused by
    /// [`ToolResultBody::new`] rather than truncated, and the executor must propagate that as
    /// `Failed(OutputInvalid)`.
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest<'a>,
        cancel: &'a CancellationScope,
    ) -> ToolExecutionFuture<'a>;
}

/// What one invoked call produced.
///
/// Three arms, and the third is separate from the second on purpose: a **refusal** is a decision
/// the model must be told about and can act on ("you are not allowed", "no such tool"), while a
/// **waiting approval** is a prompt a human has to answer. A caller that collapsed them would tell
/// a model to stop asking about an action a user is about to permit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallOutcome {
    /// The tool ran and produced a result.
    Completed {
        /// The bounded result.
        result: ToolResultBody,
    },
    /// The call was refused or failed, with the class an observation reports.
    Refused {
        /// The contract's error class.
        class: ToolErrorClass,
        /// The ledger state the call ended in.
        state: ToolCallState,
    },
    /// The call needs a human decision; the request is durable and listed.
    WaitingApproval {
        /// The approval that was requested.
        approval: ApprovalId,
    },
    /// An equivalent call had already finished, so this one was not run again.
    ///
    /// **A distinct arm rather than a `Completed`**, because the result body is not durable in this
    /// build (`TLS-003`'s named residual), so the service cannot return the earlier result and will
    /// not fabricate one. Reporting the recorded outcome is honest; returning a synthesized success
    /// would be a lie a caller could not detect.
    Duplicate {
        /// The state the earlier call recorded.
        state: ToolCallState,
        /// Its recorded error class, if it failed.
        class: Option<ToolErrorClass>,
    },
}

impl ToolCallOutcome {
    /// Returns whether the call produced a result.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    /// Returns the class an observation would report, when the call did not succeed.
    #[must_use]
    pub fn refusal_class(&self) -> Option<ToolErrorClass> {
        match self {
            Self::Completed { .. } | Self::WaitingApproval { .. } => None,
            Self::Refused { class, .. } => Some(*class),
            Self::Duplicate { state, class } => {
                Some(class.unwrap_or(if *state == ToolCallState::Succeeded {
                    // A duplicate that succeeded is not a refusal, but the caller has no result to
                    // show, so it is reported as a conflict rather than as a success. The class is
                    // the honest one: nothing failed, and nothing new was done either.
                    ToolErrorClass::Conflict
                } else {
                    ToolErrorClass::Conflict
                }))
            }
        }
    }
}

/// Why the service could not reach an outcome at all.
///
/// Only storage, clock, and envelope-construction faults, because every **decision** is a
/// [`ToolCallOutcome`]. A service that returned `Err` for a denial would make the controller fail a
/// run for a working policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolServiceError {
    /// A durable store failed.
    Storage(RepositoryError),
    /// The injected clock reported no usable instant.
    ClockUnavailable,
    /// JARVIS's own envelope or prompt could not be built.
    ///
    /// A construction fault in code JARVIS owns — an over-large fingerprint field, a preview line
    /// that would not validate — rather than a caller's request. It carries a code rather than a
    /// message so a caller reports a stable identifier.
    Internal {
        /// The stable, namespaced code.
        code: &'static str,
    },
}

impl ToolServiceError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Storage(error) => error.code(),
            Self::ClockUnavailable => "run.clock_unavailable",
            Self::Internal { code } => code,
        }
    }

    /// Returns whether retrying the same call unchanged could succeed.
    ///
    /// A construction fault is **not** retryable: the same inputs build the same envelope, so a
    /// repeat fails identically. Reporting it as retryable would spend a budget on a certain failure.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Storage(error) => error.retryable(),
            Self::ClockUnavailable | Self::Internal { .. } => false,
        }
    }
}

impl fmt::Display for ToolServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Storage(_) => "the tool-call record could not be written",
            Self::ClockUnavailable => "the clock reported no usable instant",
            Self::Internal { code } => code,
        };
        formatter.write_str(text)
    }
}

impl From<RepositoryError> for ToolServiceError {
    fn from(error: RepositoryError) -> Self {
        Self::Storage(error)
    }
}

/// What the authorization step concluded about one call.
///
/// **Three arms rather than a `Result<Option<_>>`, and each one earns its place.** A permit and a
/// refusal are the two obvious answers; the third is a call deferred to a person, and it must carry
/// the prompt's identity because that identity is the only handle a user has to answer it. Encoding
/// that as `Ok(None)` would have lost the handle, which is the shape of defect this module's own
/// header records: a value with a producer and no consumer.
enum Decision {
    /// Policy allowed the call; the pipeline proceeds to the effect.
    Permitted,
    /// Policy refused it, and the durable row already records which class.
    Denied,
    /// Policy deferred it to a person, whose prompt is now durable.
    Waiting {
        /// The durable approval the call is blocked on.
        approval: ApprovalId,
    },
}

/// Everything needed to raise one durable approval prompt.
///
/// **Grouped rather than passed positionally, and here the argument count is not the main reason.**
/// Five of these eight are identifiers of the same UUID shape, and a call site with eight positional
/// values of four types is one where a `run`/`workspace` swap is invisible to the compiler — in the
/// function that decides which principal a prompt asks, which is the one place such a transposition
/// would be a permission defect. The struct also lets the caller write the field names the contract
/// uses, so a reader comparing this against the contract's request body sees the same vocabulary.
struct ApprovalRequest<'a> {
    /// The reviewed definition, which supplies the summary and the preview's facts.
    definition: &'a ToolDefinition,
    /// The argument document, used **only** for its size — never for its content.
    arguments: &'a ToolArguments,
    /// The ledger row's own call identifier, so the two records name one call.
    call_id: ToolCallId,
    /// The run the call belongs to.
    run: RunId,
    /// The workspace the action happens in.
    workspace: WorkspaceId,
    /// Who asked, taken from the trusted request context.
    principal: PrincipalId,
    /// The digest of the exact action, so the decision can be matched to it.
    digest: ActionDigest,
    /// The instant the prompt was raised, and the base of its deadline.
    now: UtcTimestamp,
}

/// Returns the canonical call identifier this pipeline persists for one invocation.
///
/// **One function because two records must agree, not because the computation is interesting.** The
/// ledger row's `call_id` and the approval's `tool_call_id` name the same call, so the value is
/// derived in exactly one place; a second derivation — however simple — is a second opportunity for
/// the two records to disagree, which is the defect `a_prompt_names_the_very_row_the_ledger_opened`
/// exists to catch.
///
/// The conversion is by `to_string`, so the identifier JARVIS stores is the canonical hyphenated
/// lowercase form whatever the source's own spelling was: the type guarantees one spelling per
/// identity, and re-rendering is what makes the persisted text that spelling rather than a lookalike.
///
/// **The `Err` arm covers a value the type itself refuses.** `ToolCallId::parse` rejects the nil and
/// maximum identifiers, and `parse(nil.to_string())` is exactly that refusal — so the arm is
/// reachable only for an id no constructor would have produced. It maps back to the nil id rather
/// than panicking because this is a library boundary and the workspace denies `panic!` outright; the
/// row is then refused by whatever reads it, which is a diagnosable outcome rather than a crash in
/// the pipeline that decides an effect.
fn canonical_call_id(source: uuid::Uuid) -> ToolCallId {
    match ToolCallId::parse(&source.to_string()) {
        Ok(id) => id,
        Err(_) => ToolCallId::from_uuid(uuid::Uuid::nil()),
    }
}

/// Runs one tool call through the whole governed pipeline.
pub struct ToolCallService {
    catalog: Arc<dyn ToolCatalog>,
    grants: Arc<dyn ToolGrantSource>,
    validator: Arc<dyn ToolArgumentValidator>,
    fingerprint: Arc<dyn ActionFingerprint>,
    executor: Arc<dyn ToolExecutor>,
    ledger: Arc<dyn ToolCallRepository>,
    approvals: Arc<dyn ApprovalRepository>,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for ToolCallService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The ports are not printed: a catalog or an executor can hold a credential, and a
        // diagnostic line is not a place to duplicate one.
        formatter
            .debug_struct("ToolCallService")
            .finish_non_exhaustive()
    }
}

impl ToolCallService {
    /// Builds the service over its ports.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: Arc<dyn ToolCatalog>,
        grants: Arc<dyn ToolGrantSource>,
        validator: Arc<dyn ToolArgumentValidator>,
        fingerprint: Arc<dyn ActionFingerprint>,
        executor: Arc<dyn ToolExecutor>,
        ledger: Arc<dyn ToolCallRepository>,
        approvals: Arc<dyn ApprovalRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            catalog,
            grants,
            validator,
            fingerprint,
            executor,
            ledger,
            approvals,
            clock,
        }
    }

    /// Returns the canonical names of the tools this service can dispatch.
    ///
    /// The controller reads it to build the model request, so what a model is offered is exactly
    /// what the pipeline can resolve — one list rather than two that could disagree.
    #[must_use]
    pub fn capabilities(&self) -> Vec<String> {
        self.catalog.capabilities()
    }

    /// Returns what the model is told about each tool, from the same catalog as [`Self::capabilities`].
    #[must_use]
    pub fn offers(&self) -> Vec<jarvis_domain::model::stream::ToolOffer> {
        self.catalog.offers()
    }

    /// Runs `intent` through the pipeline and reports what happened.
    ///
    /// The workspace and principal are taken from `context`, which only trusted code can build —
    /// never from the intent, whose own type carries neither. That is the trust boundary the
    /// architecture states: "principal/workspace/grants are trusted context, not accepted from
    /// model arguments."
    ///
    /// `idempotency_key` scopes the reservation and is supplied by the caller rather than derived
    /// here, because only the caller knows whether a repeat is the *same logical operation* — the
    /// model's call identifier is the right key for one turn, while a workflow step's key outlives
    /// it.
    ///
    /// # Errors
    ///
    /// Returns [`ToolServiceError`] for a storage or clock fault. Every decision — including a
    /// refusal — is a [`ToolCallOutcome`].
    pub async fn invoke(
        &self,
        context: &RequestContext,
        run: RunId,
        intent: &ToolCallIntent,
        idempotency_key: &str,
        cancel: &CancellationScope,
    ) -> Result<ToolCallOutcome, ToolServiceError> {
        let workspace = context.workspace_id;
        let principal = context.principal_id;
        let now = self.now()?;

        // Step 1: resolve. A name the model emitted becomes a definition or nothing, and
        // "nothing" is `tool.not_found` rather than an error — a model naming a tool that does not
        // exist is ordinary, and a refusal it can read is how it learns.
        let Some(tool) = self.catalog.resolve(workspace, &intent.capability) else {
            return Ok(ToolCallOutcome::Refused {
                class: ToolErrorClass::NotFound,
                // No ledger row exists: a reservation key needs an identity, and an unresolved name
                // has none. `Requested` is the honest report of "nothing was ever recorded".
                state: ToolCallState::Requested,
            });
        };
        let definition = tool.definition.clone();
        let identity = definition.identity.clone();

        // The key is built before the row, because the row's identity *is* the key. A key that
        // could not be built (an empty caller key) means the caller asked for a reservation it
        // cannot have, so the call is refused rather than run unreserved — running it would be the
        // one path that can duplicate an effect.
        let Ok(key) = ReservationKey::new(identity.clone(), workspace, principal, idempotency_key)
        else {
            return Ok(ToolCallOutcome::Refused {
                class: ToolErrorClass::Conflict,
                state: ToolCallState::Requested,
            });
        };

        // The row is opened `Requested`, **then** validated. The order follows the domain's
        // own transition table (`Requested -> Validated | Denied`), and it is what gives a refused
        // call a durable row saying so rather than no record at all.
        //
        // The call identifier is minted **once**, here, and the row carries it. An approval raised
        // for this call references `entry.call_id`, so the two records name the same call **by
        // construction**. The first version minted a fresh `ToolCallId` inside `raise_approval`
        // instead, which gave one logical call two identities: the prompt a user decided named a
        // call that appears in no ledger row, so nothing could connect a decision to the row it
        // releases and the record was unusable for the resume it exists to enable.
        let call_id = canonical_call_id(intent.call_id.as_uuid());
        let mut entry = LedgerEntry::reserve(call_id, 1, run, key, LedgerOperation::Execute, now);
        // The row's own identifier **is** the call's identifier, so an approval that names the call
        // can name its row. A parked call is resumed from the approval alone — nothing else survives a
        // restart that says which row it was — and a random row identifier would make that lookup
        // impossible without a second index. The two ids never refer to different things: a row is one
        // attempt of one call.
        entry.id = ToolCallRecordId::from_uuid(call_id.as_uuid());
        // Each arm below is a **decision** the ledger already settled: an equivalent call that
        // finished, one still in flight, or one whose outcome is unknown. All three are answers
        // rather than faults, so they arrive as `Some` and `?` is left for the storage errors that
        // are faults.
        if let Some(settled) = self.settled_by_reservation(&entry).await? {
            return Ok(settled);
        }

        // Step 2a: validate. A violation is recorded as `Denied` with a `SchemaInvalid` class —
        // the row exists, the call is refused, and the model is told which.
        if let Err(refusal) = self.validator.validate(&tool, &intent.arguments) {
            self.transition(
                &mut entry,
                ToolCallState::Denied,
                Some(refusal.error_class()),
                now,
            )
            .await?;
            return Ok(ToolCallOutcome::Refused {
                class: refusal.error_class(),
                state: ToolCallState::Denied,
            });
        }
        self.transition(&mut entry, ToolCallState::Validated, None, now)
            .await?;

        // Steps 3 to 5: decide. Extracted rather than inlined because the decision has a single
        // return in each of three shapes, and a 60-line block in the middle of the pipeline hides
        // the fact that this is the *only* authorization layer.
        match self
            .decide(
                &mut entry,
                &definition,
                workspace,
                principal,
                run,
                &intent.arguments,
                now,
            )
            .await?
        {
            Decision::Permitted => {}
            // Refused or blocked on a prompt: the row already says which, and the class is read
            // from the row so the state a client reads and the state that was written cannot
            // disagree.
            Decision::Denied => {
                return Ok(ToolCallOutcome::Refused {
                    class: entry.outcome().unwrap_or(ToolErrorClass::PermissionDenied),
                    state: entry.state(),
                });
            }
            Decision::Waiting { approval } => {
                return Ok(ToolCallOutcome::WaitingApproval { approval });
            }
        }

        // Steps 5 to 7: the permitted path. Extracted rather than inlined because these are the
        // three writes and the one effect that only an allowed call reaches, and keeping them in one
        // function makes the ordering claim readable as a sequence rather than as a stretch of
        // `invoke` after the decision.
        self.apply_permitted(
            &mut entry,
            &definition,
            &identity,
            &intent.arguments,
            cancel,
        )
        .await
    }

    /// Continues a call that parked on an approval, now that the approval has been decided.
    ///
    /// **The same ledger row carries the call from `WaitingApproval` to its outcome**, so one row tells
    /// the whole story: asked, decided, executed. An approved decision re-enters the pipeline at
    /// policy — the approval is evidence for policy to weigh, not a bypass — so a grant revoked or a
    /// tool changed while the prompt sat open still refuses the call. A decision that is not an
    /// approval closes the row `Denied` with the matching class, and the model reads it as a refusal.
    ///
    /// Idempotent: a row that is no longer waiting reports what it settled as instead of running the
    /// call again, so a repeated resume cannot double an effect.
    ///
    /// # Errors
    ///
    /// Returns [`ToolServiceError`] for a storage or clock fault, or when the approval does not
    /// belong to `run` and the context's principal — a mismatch is a bug in the caller, never a
    /// refusal a model should read.
    pub async fn resume(
        &self,
        context: &RequestContext,
        run: RunId,
        approval: &DurableApproval,
        intent: &ToolCallIntent,
        cancel: &CancellationScope,
    ) -> Result<ToolCallOutcome, ToolServiceError> {
        let workspace = context.workspace_id;
        let principal = context.principal_id;
        let now = self.now()?;
        let mut entry = self
            .ledger
            .load(
                workspace,
                ToolCallRecordId::from_uuid(approval.tool_call.as_uuid()),
            )
            .await?;
        if entry.run != run
            || approval.run != run
            || approval.workspace != workspace
            || entry.key.principal != principal
        {
            return Err(ToolServiceError::Internal {
                code: "tool.resume_mismatch",
            });
        }
        if entry.state() != ToolCallState::WaitingApproval {
            return Ok(if entry.state().is_terminal() {
                ToolCallOutcome::Duplicate {
                    state: entry.state(),
                    class: entry.outcome(),
                }
            } else {
                ToolCallOutcome::Refused {
                    class: ToolErrorClass::Conflict,
                    state: entry.state(),
                }
            });
        }
        let refusal = match approval.state() {
            ApprovalState::Approved => None,
            ApprovalState::Expired => Some(ToolErrorClass::ApprovalExpired),
            ApprovalState::Rejected
            | ApprovalState::Cancelled
            | ApprovalState::Consumed
            | ApprovalState::Invalidated => Some(ToolErrorClass::ApprovalRejected),
            ApprovalState::Pending => {
                return Err(ToolServiceError::Internal {
                    code: "tool.resume_undecided",
                });
            }
        };
        if let Some(class) = refusal {
            return self.deny_waiting(&mut entry, class, now).await;
        }

        // Everything below re-checks what the first pass checked, because the catalog, the grants,
        // and the schema may all have moved while a person decided.
        let Some(tool) = self.catalog.resolve(workspace, &intent.capability) else {
            return self
                .deny_waiting(&mut entry, ToolErrorClass::NotFound, now)
                .await;
        };
        if tool.definition.identity != entry.key.identity {
            return self
                .deny_waiting(&mut entry, ToolErrorClass::Conflict, now)
                .await;
        }
        if let Err(refusal) = self.validator.validate(&tool, &intent.arguments) {
            return self
                .deny_waiting(&mut entry, refusal.error_class(), now)
                .await;
        }
        let definition = tool.definition.clone();
        match self
            .decide(
                &mut entry,
                &definition,
                workspace,
                principal,
                run,
                &intent.arguments,
                now,
            )
            .await?
        {
            Decision::Permitted => {
                let identity = definition.identity.clone();
                self.apply_permitted(
                    &mut entry,
                    &definition,
                    &identity,
                    &intent.arguments,
                    cancel,
                )
                .await
            }
            Decision::Denied => Ok(ToolCallOutcome::Refused {
                class: entry.outcome().unwrap_or(ToolErrorClass::PermissionDenied),
                state: entry.state(),
            }),
            Decision::Waiting { approval } => Ok(ToolCallOutcome::WaitingApproval { approval }),
        }
    }

    /// Closes a waiting row as denied and reports the refusal.
    async fn deny_waiting(
        &self,
        entry: &mut LedgerEntry,
        class: ToolErrorClass,
        at: UtcTimestamp,
    ) -> Result<ToolCallOutcome, ToolServiceError> {
        self.transition(entry, ToolCallState::Denied, Some(class), at)
            .await?;
        Ok(ToolCallOutcome::Refused {
            class,
            state: ToolCallState::Denied,
        })
    }

    /// Marks the decision durably, runs the effect, and records the outcome.
    ///
    /// **The ordering here is the module's second structural rule, so it is stated once and obeyed
    /// once.** `Approved`, `Reserved`, and `Executing` are three separate writes rather than one
    /// combined write, because what the contract requires is that `EXECUTING` is *on disk before the
    /// world changes*: a single write carrying state-and-outcome would place it there only
    /// afterwards, which is precisely the window a crash duplicates an effect in.
    ///
    /// # Errors
    ///
    /// Returns [`ToolServiceError`] for a storage or clock fault. A tool that failed is a
    /// [`ToolCallOutcome::Refused`], not an error — the class and terminal state come from the
    /// executor's own error rather than from a mapping invented here.
    async fn apply_permitted(
        &self,
        entry: &mut LedgerEntry,
        definition: &ToolDefinition,
        identity: &ToolIdentity,
        arguments: &ToolArguments,
        cancel: &CancellationScope,
    ) -> Result<ToolCallOutcome, ToolServiceError> {
        // The instant the decision was made is read again rather than threaded through, because the
        // decision and the effect are different moments and a row that recorded the decision's clock
        // reading for the effect would misdate it.
        let decided_at = self.now()?;
        self.transition(entry, ToolCallState::Approved, None, decided_at)
            .await?;
        self.transition(entry, ToolCallState::Reserved, None, decided_at)
            .await?;
        self.transition(entry, ToolCallState::Executing, None, decided_at)
            .await?;

        // Step 6: the effect. Every refusal that could be made has been made, so the executor is
        // reached only by an allowed call.
        let started_at = self.now()?;
        let execution = self
            .executor
            .execute(
                ToolExecutionRequest {
                    identity,
                    display_name: &definition.display_name,
                    arguments,
                    started_at,
                    timeout_ms: definition.execution.timeout_ms,
                },
                cancel,
            )
            .await;

        // Step 7: record the outcome in the same transition that makes the call terminal, so a row
        // cannot claim success while carrying no result.
        let occurred_at = self.now()?;
        match execution {
            Ok(result) => {
                self.transition(entry, ToolCallState::Succeeded, None, occurred_at)
                    .await?;
                Ok(ToolCallOutcome::Completed { result })
            }
            Err(error) => {
                let class = error.error_class();
                let state = error.terminal_state();
                self.transition(entry, state, Some(class), occurred_at)
                    .await?;
                Ok(ToolCallOutcome::Refused { class, state })
            }
        }
    }

    /// Reports a call the ledger already settled, or `None` when the reservation was granted.
    ///
    /// **Every arm is a decision, not a fault**, and that is why this returns `Option` rather than
    /// propagating: an equivalent call that already finished, one still in flight, and one whose
    /// outcome nobody knows are all answers the caller must be told, and mapping any of them to
    /// `Err` would report a working refusal as a service failure.
    ///
    /// The unresettled arm carries the class the contract reserves for precisely this — *never
    /// retried*, because repeating a call whose effect may or may not have happened is how one
    /// effect becomes two.
    async fn settled_by_reservation(
        &self,
        entry: &LedgerEntry,
    ) -> Result<Option<ToolCallOutcome>, ToolServiceError> {
        Ok(match self.ledger.reserve(entry).await? {
            ReservationOutcome::Granted => None,
            // An equivalent call already finished. Report the recorded outcome rather than run
            // again; the result body itself is not durable in this build.
            ReservationOutcome::AlreadyTerminal { state, outcome, .. } => {
                Some(ToolCallOutcome::Duplicate {
                    state,
                    class: outcome,
                })
            }
            ReservationOutcome::InFlight { .. } => Some(ToolCallOutcome::Refused {
                class: ToolErrorClass::Conflict,
                state: ToolCallState::Reserved,
            }),
            ReservationOutcome::Unsettled { .. } => Some(ToolCallOutcome::Refused {
                class: ToolErrorClass::OutcomeAmbiguous,
                state: ToolCallState::Reconciling,
            }),
        })
    }

    /// Fingerprints the action, evaluates policy, and reports which of the three conclusions it
    /// reached.
    ///
    /// The three arms are the whole of the decision step, and they are an enum rather than a
    /// `Result<Option<_>>` because the middle one carries a value the caller must not lose: a
    /// pending prompt is identified by its [`ApprovalId`], and a caller that reported "waiting"
    /// without it would leave the user no way to answer.
    ///
    /// The digest computed here is **not** returned: nothing downstream of an `Allow` reads it,
    /// and returning it would invite a second computation elsewhere — which could differ, and would
    /// then bind an approval to an action nobody authorized.
    ///
    /// # Errors
    ///
    /// Returns [`ToolServiceError`] for a storage or clock fault, or an internal code when a
    /// fingerprint or an approval prompt could not be constructed.
    #[allow(clippy::too_many_arguments)]
    async fn decide(
        &self,
        entry: &mut LedgerEntry,
        definition: &ToolDefinition,
        workspace: WorkspaceId,
        principal: PrincipalId,
        run: RunId,
        arguments: &ToolArguments,
        now: UtcTimestamp,
    ) -> Result<Decision, ToolServiceError> {
        // Step 3: fingerprint the exact action. This runs **before** policy because the digest is
        // an input to the decision — an approval is matched against it — and computing it later
        // would mean policy judged an action it could not identify.
        let digest = self.fingerprint_of(definition, workspace, principal, arguments)?;

        // Step 4: evaluate. Deterministic, pure, and the only authorization layer.
        let grants = self
            .grants
            .read(principal, workspace)
            .await
            .map_err(ToolServiceError::Storage)?;
        let grants_read = grants.grants;
        let deny_rules = grants.deny_rules;
        let approved = self
            .approved_for(workspace, principal, &definition.identity)
            .await?;
        let approvals: Vec<ApprovalRecord> = approved.iter().map(record_of).collect();
        let effects: BTreeSet<Effect> = definition.effects.iter().copied().collect();
        let scopes: BTreeSet<jarvis_domain::tool::classification::Scope> =
            definition.required_scopes.iter().cloned().collect();
        let decision = evaluate(
            &PolicyRequest {
                identity: &definition.identity,
                principal,
                workspace,
                effects: &effects,
                risk: definition.risk,
                required_scopes: &scopes,
                default_approval: definition.default_approval,
                sensitivity: definition.data_classes.input,
                action_digest: digest,
                tool_enabled: true,
                deadline: None,
            },
            &PolicyInputs {
                now,
                grants: &grants_read,
                approvals: &approvals,
                deny_rules: &deny_rules,
            },
        );

        match decision.outcome {
            PolicyOutcome::Deny => {
                let class = decision
                    .error_class()
                    .unwrap_or(ToolErrorClass::PermissionDenied);
                self.transition(entry, ToolCallState::Denied, Some(class), now)
                    .await?;
                Ok(Decision::Denied)
            }
            PolicyOutcome::Ask => {
                // The prompt is made **durable** before the caller is told to wait, so a user who
                // opens the approval listing sees the call that is blocked on them. Recording it
                // afterwards would leave a window in which a run waits for a prompt nobody can
                // answer.
                let approval = self
                    .raise_approval(ApprovalRequest {
                        definition,
                        arguments,
                        call_id: entry.call_id,
                        run,
                        workspace,
                        principal,
                        digest,
                        now,
                    })
                    .await?;
                // The row carries **no** outcome class here, because a pending call has not been
                // refused by anything: it is a live request waiting on a person, and stamping it
                // with a refusal class would make an unanswered prompt read as a decision.
                // A call being **resumed** is already `WaitingApproval`; the edge from a state to
                // itself is not in the table, and the row correctly keeps waiting on the new prompt.
                if entry.state() != ToolCallState::WaitingApproval {
                    self.transition(entry, ToolCallState::WaitingApproval, None, now)
                        .await?;
                }
                Ok(Decision::Waiting { approval })
            }
            PolicyOutcome::Allow if decision.reasons.contains(&PolicyReason::ApprovalMatched) => {
                self.spend_approval(entry, &approved, (workspace, principal), digest, now)
                    .await
            }
            PolicyOutcome::Allow => Ok(Decision::Permitted),
        }
    }

    /// Spends the one-shot approval that let a call through.
    ///
    /// **Policy says an approval matched; it does not say which, and it cannot spend one.** The
    /// consumption belongs here, before the call is reserved: an approval left `Approved` would
    /// authorize the same action again, which is the difference between one email and two. A
    /// concurrent call that spent it first makes this write a version conflict, and that call is
    /// refused rather than allowed to share the approval.
    async fn spend_approval(
        &self,
        entry: &mut LedgerEntry,
        approved: &[DurableApproval],
        scope: (WorkspaceId, PrincipalId),
        digest: ActionDigest,
        now: UtcTimestamp,
    ) -> Result<Decision, ToolServiceError> {
        let (workspace, principal) = scope;
        // The same four conditions policy matched on, so this picks the approval policy relied on.
        let Some(matched) = approved.iter().find(|approval| {
            approval.workspace == workspace
                && approval.requesting_principal == principal
                && approval.action_digest == digest
                && now < approval.expires_at
        }) else {
            return Ok(Decision::Permitted);
        };
        if !matched.scope.is_consumed_on_use() {
            return Ok(Decision::Permitted);
        }
        let mut spent = matched.clone();
        let expected = spent.version();
        let transition = spent
            .apply(
                ApprovalState::Consumed,
                expected,
                ApprovalActor::Consumed {
                    tool_call: entry.call_id,
                },
                now,
            )
            .map_err(|error| ToolServiceError::Internal { code: error.code() })?;
        match self
            .approvals
            .apply_transition(workspace, &transition, expected, &transition.actor, &spent)
            .await
        {
            Ok(_) => Ok(Decision::Permitted),
            Err(RepositoryError::VersionConflict { .. }) => {
                self.transition(
                    entry,
                    ToolCallState::Denied,
                    Some(ToolErrorClass::ApprovalRejected),
                    now,
                )
                .await?;
                Ok(Decision::Denied)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Applies one transition to the row and persists it with its trail.
    ///
    /// The version is passed from the row's own current value rather than tracked here, so the
    /// optimistic check is against what the store holds and a row another writer moved is refused
    /// rather than overwritten — the `apply_transition` port's own contract.
    async fn transition(
        &self,
        entry: &mut LedgerEntry,
        to: ToolCallState,
        outcome: Option<ToolErrorClass>,
        at: UtcTimestamp,
    ) -> Result<(), ToolServiceError> {
        let expected: ToolCallVersion = entry.version();
        let transition = entry
            .apply(to, expected, outcome, at)
            .map_err(|error| ToolServiceError::Internal { code: error.code() })?;
        self.ledger
            .apply_transition(
                entry.key.workspace,
                &transition,
                transition.prior_version,
                entry,
            )
            .await?;
        Ok(())
    }

    /// Builds the action envelope and computes its digest.
    ///
    /// A construction refusal is mapped to `LimitExceeded` rather than propagated as an error: the
    /// envelope is assembled from JARVIS's own facts, so the ways it can fail are a field too large
    /// to bind — which is a bound the call exceeded, not a fault. Reporting it as its own class
    /// keeps a caller from retrying it, because a repeat with the same arguments fails identically.
    fn fingerprint_of(
        &self,
        definition: &ToolDefinition,
        workspace: WorkspaceId,
        principal: PrincipalId,
        arguments: &ToolArguments,
    ) -> Result<ActionDigest, ToolServiceError> {
        let input = FingerprintInput::new(FingerprintParts {
            version: FINGERPRINT_FORMAT_VERSION,
            identity: &definition.identity,
            workspace,
            principal,
            effects: &definition.effects,
            risk: definition.risk,
            arguments,
        })
        .map_err(|error| ToolServiceError::Internal { code: error.code() })?;
        Ok(self.fingerprint.fingerprint(&input))
    }

    /// Reads the recorded approvals that could cover this tool.
    ///
    /// Filtered by identity rather than read whole, because the policy evaluator's approval lookup
    /// is scoped to the tool and a principal's whole decision history would grow without bound. The
    /// filter is a **narrowing**, not a decision: policy still re-checks the digest, the workspace,
    /// and the expiry, so a row that slipped through this read could not authorize anything.
    async fn approved_for(
        &self,
        workspace: WorkspaceId,
        principal: PrincipalId,
        identity: &ToolIdentity,
    ) -> Result<Vec<DurableApproval>, ToolServiceError> {
        let decided = self
            .approvals
            .decided_by(workspace, principal, MAX_APPROVAL_READ)
            .await?;
        // **Only `Approved` records can authorize anything.** The read returns everything the
        // principal decided, and the first version of this function turned every one of those into
        // a record policy could match — a *rejected* approval then read as permission. A spent
        // approval is `Consumed` and a withdrawn one `Cancelled`, so excluding all but `Approved`
        // is also what makes a one-shot approval single-use without a second flag.
        Ok(decided
            .into_iter()
            .filter(|approval| {
                &approval.identity == identity && approval.state() == ApprovalState::Approved
            })
            .collect())
    }

    /// Records a durable prompt for the action and returns its identity.
    ///
    /// The preview is built from the tool's own declared purpose rather than from the argument
    /// document, and that is deliberate: the arguments are untrusted model output, and copying them
    /// into a prompt a user reads to decide would put untrusted bytes in the one display that
    /// decides an effect. A structured preview of the *action* — which tool, which class of effect,
    /// how dangerous — is what the approval contract asks for, and redaction is a later slice.
    ///
    /// `call_id` is the **ledger row's own** canonical call identifier, passed in rather than
    /// minted here. The contract's request body carries a `tool_call_id`, and it has to name the
    /// call that is actually blocked: a second identifier would make the approval and the ledger row
    /// describe one action under two names, which is the defect this parameter exists to prevent.
    ///
    /// # Errors
    ///
    /// Returns [`ToolServiceError::Internal`] when a summary, preview, channel set, or expiry
    /// cannot be constructed, and [`ToolServiceError::Storage`] when the prompt cannot be written.
    /// Each construction refusal is a fault in JARVIS's own envelope rather than a caller's request,
    /// so none of them is retryable.
    async fn raise_approval(
        &self,
        request: ApprovalRequest<'_>,
    ) -> Result<ApprovalId, ToolServiceError> {
        let ApprovalRequest {
            definition,
            arguments,
            call_id,
            run,
            workspace,
            principal,
            digest,
            now,
        } = request;
        let summary = ApprovalSummary::new(&format!(
            "{}: {}",
            definition.display_name, definition.purpose
        ))
        .map_err(|_| ToolServiceError::Internal {
            code: "tool.approval_summary_unusable",
        })?;
        let preview = ApprovalPreview::new(preview_items(definition, arguments)).map_err(|_| {
            ToolServiceError::Internal {
                code: "tool.approval_preview_unusable",
            }
        })?;
        // The permitted channels are the two this build can actually verify a decision on. A
        // `desktop` or `voice` channel would be a prompt no surface can answer, which is the "no
        // legal way out" shape `AllowedChannels` exists to prevent — and the domain refuses an empty
        // set, so naming the served ones is the only honest choice.
        let channels = AllowedChannels::new(vec![ApprovalChannel::Cli, ApprovalChannel::Api])
            .map_err(|_| ToolServiceError::Internal {
                code: "tool.approval_channels_unusable",
            })?;
        // The expiry is computed by the domain's own bounded addition rather than by a second
        // time-arithmetic implementation in this layer — the reason there is no `jiff` dependency
        // here. `expiring_after` refuses a zero or over-ceiling window, so the value it returns is
        // bounded by construction.
        let expires_at = RunBudget::expiring_after(now, APPROVAL_WINDOW_MS)
            .map_err(|_| ToolServiceError::Internal {
                code: "tool.approval_window_unrepresentable",
            })?
            .deadline
            .ok_or(ToolServiceError::Internal {
                code: "tool.approval_window_unrepresentable",
            })?;
        let approval = DurableApproval::request(ApprovalRequestParts {
            workspace,
            requesting_principal: principal,
            run,
            tool_call: call_id,
            identity: definition.identity.clone(),
            action_digest: digest,
            risk: definition.risk,
            effects: definition.effects.clone(),
            summary,
            preview,
            allowed_channels: channels,
            expires_at,
            scope: ApprovalScopeKind::OneShot,
        });
        let id = approval.id;
        self.approvals.request(&approval).await?;
        Ok(id)
    }

    /// Returns the current instant, or the clock's refusal.
    fn now(&self) -> Result<UtcTimestamp, ToolServiceError> {
        self.clock
            .now()
            .map_err(|_| ToolServiceError::ClockUnavailable)
    }
}

/// Renders an approved approval as the record policy matches against.
///
/// `consumed` is always false: an approval that was spent is no longer `Approved`, so reaching this
/// function already means it has not been.
fn record_of(approval: &DurableApproval) -> ApprovalRecord {
    ApprovalRecord {
        identity: approval.identity.clone(),
        principal: approval.requesting_principal,
        workspace: approval.workspace,
        action_digest: approval.action_digest,
        expires_at: approval.expires_at,
        consumed: false,
    }
}

/// The largest number of decided approvals one policy evaluation reads.
///
/// A page bound rather than a total, and named so a future caller that needs more pages rather than
/// widening it — the shape `MAX_APPROVAL_PAGE` records. It is generous against the one approval a
/// call needs and exists so a principal's whole history cannot be pulled in to answer one question.
pub const MAX_APPROVAL_READ: u32 = 200;

/// Builds the preview lines for a prompt.///
/// **The argument document is deliberately not included**, and it is not an omission: the arguments
/// are untrusted model output, and the contract's preview rule is that a preview is shown *inside*
/// the consent prompt. A value that could reshape that prompt is exactly what the control-character
/// rule on [`PreviewItem`] defends against, and the cheapest way not to depend on redaction being
/// right is not to put the value there at all. What is shown is the *action* — which tool, which
/// effects, how dangerous — which is what a user needs to decide.
///
/// The argument document's **size** is shown rather than its content, because "this will send 4 KB
/// of text" is a fact a user can act on and contains nothing.
fn preview_items(definition: &ToolDefinition, arguments: &ToolArguments) -> Vec<PreviewItem> {
    let mut items = Vec::new();
    if let Ok(item) = PreviewItem::new("tool", &definition.identity.capability.to_string()) {
        items.push(item);
    }
    if let Ok(item) = PreviewItem::new("risk", definition.risk.as_contract_str()) {
        items.push(item);
    }
    // The effects and the argument size are the two facts a decision turns on that are not already
    // the tool's name, and both are bounded values rather than caller text.
    for effect in &definition.effects {
        if let Ok(item) = PreviewItem::new("effect", effect.as_contract_str()) {
            items.push(item);
        }
    }
    if let Ok(item) = PreviewItem::new("argument_bytes", &arguments.len().to_string()) {
        items.push(item);
    }
    items
}
#[cfg(test)]
#[path = "tool_call_tests.rs"]
mod tool_call_tests;
