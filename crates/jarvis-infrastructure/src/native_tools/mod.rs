//! Native tools built into the daemon, and the executor that runs them.
//!
//! The tool fabric's first reference tools: capabilities whose implementation is JARVIS's own code
//! rather than a connector, plugin, MCP server, or external runtime. Each is a **reviewed
//! definition** ([`Definition::build`]) plus an **executor arm**, and the two live together so a
//! definition without an implementation — a tool a model can see and policy can allow but nothing
//! can run — is not constructible.
//!
//! ## One executor for every native tool, dispatching on identity
//!
//! [`NativeExecutor`] implements `ToolExecutor` and routes a call by its canonical capability. The
//! alternative — one adapter type per tool — would make the catalog's population and the executor's
//! coverage two lists that can silently disagree: a definition registered with no matching adapter
//! is a tool the model calls and nothing answers. Dispatching from one place means
//! [`NativeExecutor::supports`] and [`catalog`] are derived from the **same** list, so the two
//! cannot drift.
//!
//! ## The dispatch is on the canonical capability, never on a display name
//!
//! A display name is an alias that can be re-pointed, and the architecture is explicit that it never
//! identifies a tool. Routing on one would be `ACC-024`'s failure mode in the executor: a call whose
//! identity says one implementation would run another. The match is on
//! `identity.capability.to_string()`, which includes the major version — so `clock.now@2` is a
//! *different* tool and, with no arm for it, is refused rather than answered by `@1`.

pub mod clock;

use jarvis_application::tool_call::{
    ToolExecutionError, ToolExecutionFuture, ToolExecutionRequest, ToolExecutor,
};
use jarvis_domain::clock::Clock;
use jarvis_domain::tool::call::{ContentBlock, ToolArguments, ToolResultBody};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::ToolIdentity;

use jarvis_application::tool_call::ResolvedTool;

use crate::tool_fingerprint::schema_fingerprint_of;

/// A native tool: its reviewed definition and the schema text its fingerprint was derived from.
///
/// The pair rather than the definition alone, because a [`ToolCatalog`] needs the schema document to
/// hand the validator, and recomputing it from a constant at two call sites is how a catalog and a
/// definition come to describe different schemas.
///
/// [`ToolCatalog`]: jarvis_application::tool_call::ToolCatalog
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// The reviewed definition.
    pub definition: ToolDefinition,
    /// The input schema document, exactly the text the fingerprint was computed over.
    pub input_schema: String,
}

impl Definition {
    /// Builds one native tool's definition.
    ///
    /// # Errors
    ///
    /// Returns the domain's construction refusal when the reviewed constants are inconsistent. A
    /// composition fault in JARVIS's own configuration, reported rather than panicked so a daemon
    /// that cannot build its catalog fails startup with a code.
    ///
    /// `build` is the per-tool constructor — [`clock::definition`] is the first — so this type is
    /// generic over the tools rather than listing their fields.
    pub fn build(
        build: fn() -> Result<ToolDefinition, jarvis_domain::error::DomainError>,
        input_schema: &str,
    ) -> Result<Self, jarvis_domain::error::DomainError> {
        Ok(Self {
            definition: build()?,
            input_schema: input_schema.to_owned(),
        })
    }
}

/// Every native tool this build ships, as reviewed configuration.
///
/// **One list, consumed by both the catalog and the executor**, which is what makes "every
/// registered tool has an implementation" a property of the code rather than of two lists agreeing.
/// Adding a tool means adding it here and adding its arm to [`NativeExecutor::execute`]; the
/// compiler cannot check the second, so `NativeExecutor::refuses_an_unimplemented_capability` exists
/// and asserts the dispatch refuses a capability no arm answers — the fail-closed direction being a
/// refusal rather than a panic or an empty success.
///
/// # Errors
///
/// Returns a construction refusal when a reviewed definition is inconsistent — a typo in a schema or
/// a version whose major disagrees with its capability.
pub fn definitions() -> Result<Vec<Definition>, jarvis_domain::error::DomainError> {
    Ok(vec![Definition::build(
        clock::definition,
        clock::INPUT_SCHEMA,
    )?])
}

/// The catalog of native tools, ready for the tool-call service.
///
/// # Errors
///
/// Returns a construction refusal from [`definitions`].
pub fn catalog() -> Result<Vec<ResolvedTool>, jarvis_domain::error::DomainError> {
    Ok(definitions()?
        .into_iter()
        .map(|tool| ResolvedTool {
            definition: tool.definition,
            input_schema: Some(tool.input_schema),
        })
        .collect())
}

/// Runs the daemon's own native tools.
///
/// Holds the injected [`Clock`] rather than reading the operating system's directly, and that is the
/// load-bearing part of the design: the result a test asserts on is the value the fixture's clock
/// holds, so "the model was told the time" is a checkable claim rather than a comparison against
/// whatever instant the wall clock happened to be at. The same port the controller, the budget, and
/// the approval service use.
#[derive(Clone)]
pub struct NativeExecutor {
    clock: std::sync::Arc<dyn Clock>,
}

impl std::fmt::Debug for NativeExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The clock is not printed: a `Debug` rendering must not walk a port that could hold
        // configuration, and the fact a reader needs is only that this executor has one.
        formatter
            .debug_struct("NativeExecutor")
            .finish_non_exhaustive()
    }
}

impl NativeExecutor {
    /// Builds an executor over `clock`.
    #[must_use]
    pub fn new(clock: std::sync::Arc<dyn Clock>) -> Self {
        Self { clock }
    }

    /// Returns whether this executor implements `identity`.
    ///
    /// Derived from the **same canonical capability** the dispatch matches, so a tool the catalog
    /// offers and this answers cannot disagree — the drift one-list-per-tool would permit.
    #[must_use]
    pub fn supports(identity: &ToolIdentity) -> bool {
        identity.capability.to_string() == clock::CAPABILITY
    }

    /// Returns the capability this executor refuses, for its own fail-closed assertion.
    ///
    /// A capability nothing implements. Exposed as a constant rather than written in a test so the
    /// assertion and the dispatch read the same value: a test naming a *different* unknown
    /// capability would keep passing after the arm for this one was added.
    pub const UNIMPLEMENTED_CAPABILITY: &'static str = "clock.not_a_real_tool@1";
}

impl ToolExecutor for NativeExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest<'a>,
        cancel: &'a jarvis_application::cancellation::CancellationScope,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            // Cancellation is checked **before** the effect, which for a clock read is the only
            // window there is: the read cannot be interrupted once started. The check is still
            // required, because the contract is that a cancelled call ends `Cancelled` rather than
            // succeeding with a value nobody asked for any more.
            if cancel.is_cancelled() {
                return Err(ToolExecutionError::Cancelled);
            }
            match request.identity.capability.to_string().as_str() {
                clock::CAPABILITY => read_clock(self.clock.as_ref(), request.arguments),
                // **A capability with no arm is refused, never answered.** An executor that returned
                // an empty success for an unknown capability would make every catalog entry look
                // implemented, which is the one thing this dispatch exists to make impossible.
                _ => Err(ToolExecutionError::Failed(ToolErrorClass::NotFound)),
            }
        })
    }
}

/// Reads the clock and builds the bounded result.
///
/// **Synchronous, and deliberately so.** A clock read performs no I/O — the trait's own contract is
/// that it reports an instant — so an `async` wrapper here would add a state machine and an await
/// point for no effect, and an await point in this position is exactly what would make the
/// "`EXECUTING` before the effect" ordering testable only by luck.
///
/// # Errors
///
/// Returns `Failed(OutputInvalid)` when the instant cannot be rendered, and
/// `Failed(ProviderError)` when the clock itself is unavailable — a distinction an operator acts
/// on: an unavailable clock is an environment fault, while an unrenderable instant would be
/// impossible and is reported as a result fault rather than blamed on the clock.
fn read_clock(
    clock: &dyn Clock,
    arguments: &ToolArguments,
) -> Result<ToolResultBody, ToolExecutionError> {
    let now = clock
        .now()
        .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::ProviderError))?;
    // The instant is rendered as the contract's RFC 3339 `Z` form, which is the same spelling the
    // domain stores and puts on the wire — so a model reading the result sees the identical string
    // a client would, and there is no second timestamp format to reconcile.
    let rendered = now.to_string();
    let block = ContentBlock::json(&format!("{{\"utc\":\"{rendered}\"}}"))
        .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))?;
    // The arguments are **not** echoed back. A result that repeated its input would put the model's
    // own text into the transcript twice, and for this tool the arguments are the empty object — so
    // echoing them would be pure noise. The `_arguments` binding is deliberate rather than an
    // unused parameter: a future arm that needs them reads this one.
    let _ = arguments;
    ToolResultBody::new(
        vec![block],
        None,
        jarvis_domain::model::policy::Sensitivity::Internal,
    )
    .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))
}

/// Recomputes the schema fingerprint of a native tool's schema.
///
/// Exposed so a **definition test** can assert that the fingerprint a definition carries is derived
/// from the schema text the catalog hands the validator — the pair `SchemaValidator::confirms`
/// checks at call time. Asserting it here as well means a tool added to [`definitions`] with a
/// hand-written fingerprint fails at the definition rather than at every one of its calls.
#[must_use]
pub fn schema_fingerprint_of_native(schema: &str) -> String {
    schema_fingerprint_of(schema).to_string()
}
