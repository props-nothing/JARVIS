//! Executing an MCP tool through the `ToolExecutor` port: the MRTR loop over the call decisions.
//!
//! `call.rs` owns the per-step decisions — the wire name, the request parameters, whether a response
//! completes the call, and what a result means. What it deliberately does **not** own is the loop that
//! drives them, and this module is that loop: it holds a live session and drives `tools/call` until the
//! server answers, until cancellation, or until a bound is reached.
//!
//! # The MRTR loop is the SDK's, and JARVIS's contribution is which rounds to *stop* on
//!
//! `RunningService::call_tool_with_mrtr_max_rounds` already implements multi round-trip continuation: it
//! sends the request, and on `input_required` it delegates to the client handler to fulfil the requests and
//! retries with `input_responses` and the echoed `request_state`. So this adapter does not reimplement that
//! loop — and it must not, because a second implementation would disagree with the SDK about what a round is.
//!
//! What JARVIS supplies is the **policy for a round**. `call.rs`'s [`super::call::decide_response`] is asked
//! of every `input_required` response, and every input kind this build can classify is refused — so in
//! practice the SDK's loop reaches its first continuation, and this adapter's answer is "do not". That is
//! why this module passes the round cap explicitly: `MAX_MRTR_ROUNDS` is JARVIS's limit, asserted below the
//! SDK's own at compile time in `invocation.rs`, so an operator reading `mcp.round_limit_exceeded` learns
//! JARVIS's bound rather than the SDK's.
//!
//! # Three ways a call can end, and each is kept distinct
//!
//! | Ending | Class | Why |
//! | --- | --- | --- |
//! | The server answered | the result, or the class it reported | normal completion |
//! | The caller cancelled | `Cancelled` | JARVIS's own decision, not the server's fault |
//! | A bound was reached | `LimitExceeded` | the round cap, or a schema that no round could satisfy |
//!
//! Collapsing cancellation into a provider error would blame the server for JARVIS stopping, and collapsing
//! a bound into one would send an operator looking for a fault that does not exist.
//!
//! # The arguments are validated before this, and the identity after
//!
//! The pipeline validates the argument document against the tool's schema and checks the policy that
//! authorizes the call **before** reaching an executor — that ordering is what makes "a denied model request
//! cannot bypass policy through a native, MCP, or runtime route" a property of the pipeline rather than of
//! each executor's diligence. This executor therefore does no authorization, and re-checks the *identity* at
//! invocation because a server can re-schema a tool between the listing and the call: it looks the identity
//! up in the catalog it was built from and refuses anything the registry does not hold.

use std::sync::Arc;

use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{
    ToolExecutionError, ToolExecutionFuture, ToolExecutionRequest, ToolExecutor,
};
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::ToolIdentity;
use rmcp::RoleClient;
use rmcp::model::CallToolResult;
use rmcp::service::RunningService;

use super::call::{McpCallRefusal, call_params, normalize_response};
use super::client::JarvisClient;
use super::invocation::{MAX_MRTR_ROUNDS, McpInvocationRefusal};
use super::outcome::classify_service_error;

/// Runs one discovered MCP server's tools.
///
/// Built from a live session and the identities registration admitted, so the identity a call carries is
/// checked against what the server actually offered rather than against whatever the caller named.
///
/// # Why the `RunningService` and not its `Peer`, which is the type the calls *look* like they belong to
///
/// ⚠ **This was a wrong assumption, found by compiling.** `Peer<RoleClient>` owns the single-round-trip
/// senders — `call_tool_once` is there — but the **multi round-trip loop**
/// (`call_tool_with_mrtr_max_rounds`) is implemented on `RunningService<RoleClient, S>`, because it drives
/// several `call_tool_once` round trips and needs the client handler to fulfil the intermediate requests.
/// Holding only the peer would therefore mean reimplementing the loop, which is exactly the second
/// implementation this module exists to avoid.
///
/// The service is held behind an `Arc` because it is not `Clone` and the executor needs shared ownership;
/// the caller keeps the ability to cancel through [`super::discovery::DiscoveredServer`]'s own handle when
/// it wants it, and a shutdown drops the last reference, which kills the child. **`RunningService` must be
/// dropped inside a runtime** — its transport kills the child from `Drop`.
pub struct McpToolExecutor {
    /// The session, which owns both the senders and the MRTR loop.
    ///
    /// Behind a lock because a supervisor may **replace** it after the child dies ([`Self::replace_session`]).
    /// A call clones the `Arc` once and uses that clone for its whole exchange, so a replacement never changes
    /// the session a call in flight is talking to.
    session: std::sync::RwLock<Arc<RunningService<RoleClient, JarvisClient>>>,
    /// The identities this executor may call, from the catalog the session produced.
    ///
    /// A **set of identities**, not the catalog itself: the call path needs membership, and holding the
    /// schemas here would put a second copy of them beside the argument validator's.
    callable: Arc<std::collections::BTreeSet<ToolIdentity>>,
    /// The server's own declared default bound, used only when a request arrives without one.
    ///
    /// **The call's bound comes from the request, not from here** — see [`Self::invoke`]. This is the
    /// fallback for a caller that supplies `0`, which `ToolExecutionRequest` cannot forbid across a crate
    /// boundary and which would otherwise mean "no bound at all".
    default_timeout_ms: u64,
}

impl std::fmt::Debug for McpToolExecutor {
    /// Reports the callable count and the fallback bound, never the identities' sources or the session.
    ///
    /// The session is not usefully printable, and an identity list can be long — the count is the fact a
    /// diagnostic line needs.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpToolExecutor")
            .field("callable", &self.callable.len())
            .field("default_timeout_ms", &self.default_timeout_ms)
            .finish_non_exhaustive()
    }
}

impl McpToolExecutor {
    /// Builds an executor over a live session and the identities it may call.
    ///
    /// `default_timeout_ms` is the **fallback** bound, not the bound a call uses: `ToolExecutor::execute`
    /// carries the tool's declared timeout in its request, and that is what one call is measured against.
    /// This value stands in only when a request supplies `0`.
    #[must_use]
    pub fn new(
        session: Arc<RunningService<RoleClient, JarvisClient>>,
        callable: impl IntoIterator<Item = ToolIdentity>,
        default_timeout_ms: u64,
    ) -> Self {
        Self {
            session: std::sync::RwLock::new(session),
            callable: Arc::new(callable.into_iter().collect()),
            default_timeout_ms,
        }
    }

    /// Returns whether this executor may call `identity`.
    #[must_use]
    pub fn supports(&self, identity: &ToolIdentity) -> bool {
        self.callable.contains(identity)
    }

    /// Returns how many identities this executor may call.
    #[must_use]
    pub fn callable(&self) -> usize {
        self.callable.len()
    }

    /// Returns whether the session has closed — the child is gone or its transport ended.
    ///
    /// **The one thing a caller can ask before dispatching, and the caller that must is the one deciding**
    /// whether to route a call here at all. `McpComposition::health` reports the same fact in bulk; this is
    /// the single-server form so a router or a supervisor can consult it without holding a composition.
    /// Delegated to the session's own `is_closed` rather than tracked here: a cached flag beside a live
    /// signal is a second answer to one question, and the stale one is the one that gets read.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.session().is_closed()
    }

    /// Returns whether `identities` are exactly the identities this executor may call.
    ///
    /// The check a supervisor makes before substituting a restarted server: a replacement offering more,
    /// fewer, or different tools is not the server the grants were written against.
    #[must_use]
    pub fn offers_exactly(&self, identities: &[ToolIdentity]) -> bool {
        let offered: std::collections::BTreeSet<&ToolIdentity> = identities.iter().collect();
        offered.len() == self.callable.len() && self.callable.iter().all(|i| offered.contains(i))
    }

    /// Returns the current session.
    ///
    /// A clone of the `Arc`, so a caller holds the session it read even if a supervisor replaces it a moment
    /// later. A poisoned lock is read through: the guarded value is an `Arc` that is only ever replaced whole,
    /// so a panic elsewhere cannot leave it half-written.
    #[must_use]
    pub fn session(&self) -> Arc<RunningService<RoleClient, JarvisClient>> {
        Arc::clone(
            &self
                .session
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Replaces the session with a freshly launched one and returns the previous session.
    ///
    /// The caller is responsible for having verified that the new session offers **exactly** the identities
    /// this executor may call — an executor whose session offers different tools than its `callable` set
    /// describes would authorize one catalog and dispatch to another.
    pub fn replace_session(
        &self,
        session: Arc<RunningService<RoleClient, JarvisClient>>,
    ) -> Arc<RunningService<RoleClient, JarvisClient>> {
        let mut guard = self
            .session
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut *guard, session)
    }

    /// Sends one `tools/call` and normalizes what comes back.
    ///
    /// # Errors
    ///
    /// Returns the class the call is recorded under. A refusal this adapter made itself (an unaddressable
    /// capability, an argument document that is not an object, an identity it may not call) carries the
    /// class `call.rs` assigns that refusal; a transport failure carries the class `outcome.rs` assigns it.
    ///
    /// `call_timeout_ms` is the bound the **caller's request** declared, not one this executor chose. The
    /// port puts the tool's declared timeout in `ToolExecutionRequest`, and until this parameter existed that
    /// value was written by the pipeline and read by nobody — so the bound a tool was reviewed with was not
    /// the bound it ran under. A `0` means the caller supplied none, and falls back to the server's own
    /// declared default rather than meaning "unbounded".
    async fn invoke(
        &self,
        identity: &ToolIdentity,
        arguments: &jarvis_domain::tool::call::ToolArguments,
        cancel: &CancellationScope,
        call_timeout_ms: u64,
    ) -> Result<jarvis_domain::tool::call::ToolResultBody, ToolErrorClass> {
        // **Membership first, so an unauthorized identity never reaches the wire.** The pipeline authorized
        // this call against a grant, and the grant names an identity — a server that re-schemas a tool
        // between the listing and the call produces a different identity, which is exactly the case
        // `ACC-024` describes.
        //
        // **The class is the one the per-response identity guard produces, deliberately.** One condition
        // must have one class regardless of which layer noticed it, so this reuses
        // `decide_response`'s refusal rather than inventing a second answer — a `PermissionDenied` here
        // and an `Unavailable` there would make one identity change report as two different failures
        // depending on whether the server had answered yet.
        if !self.supports(identity) {
            return Err(class_of_refusal(&McpCallRefusal::Refused {
                refusal: McpInvocationRefusal::ToolIdentityChanged,
            }));
        }
        // **Liveness next, and the class it produces is the whole point of the check.** A session whose
        // worker has already finished — the child died, or its transport closed — cannot deliver anything, so
        // sending to it would fail as `TransportClosed`, which `outcome.rs` classifies `ProviderError`: an
        // *unsettled* outcome that sends the recovery pass looking for an effect that was never produced. The
        // call certainly did nothing, so `Unavailable` (settled, `Safe`) is the honest class.
        //
        // This is **not** a health monitor — it cannot see a child that died after the check, and a race is
        // still correctly reported as unsettled. It removes the *wrong* answer for the case the session
        // already knows, which is what a dispatch to a server known to be gone was reporting.
        if self.is_closed() {
            return Err(ToolErrorClass::Unavailable);
        }
        let params = match call_params(identity, arguments) {
            Ok(params) => params,
            Err(refusal) => return Err(class_of_refusal(&refusal)),
        };

        // One read of the session for the whole exchange; see the field for why.
        let session = self.session();
        let call = session.call_tool_with_mrtr_max_rounds(params, MAX_MRTR_ROUNDS);
        // **The bound is the caller's.** `ToolExecutionRequest::timeout_ms` is documented as "the tool's
        // declared timeout", and it is what the pipeline reviewed the tool with — so measuring the call
        // against anything else would enforce a bound nobody chose. `0` falls back to the server's declared
        // default rather than to no bound, because a request without one must not mean "unbounded".
        let bound_ms = if call_timeout_ms == 0 {
            self.default_timeout_ms
        } else {
            call_timeout_ms
        };
        // The timeout bounds the whole exchange including continuations, and the cancellation race is
        // **outside** it so a cancelled call reports `Cancelled` rather than a timeout: a caller that
        // stopped the work must not be told the server was slow.
        let bounded = tokio::time::timeout(std::time::Duration::from_millis(bound_ms), call);
        let outcome = tokio::select! {
            // `biased` so a cancellation that is already signalled wins over a call that is already
            // finished — the caller's decision is the one to report.
            biased;
            () = cancel.cancelled() => return Err(ToolErrorClass::Cancelled),
            result = bounded => result,
        };
        match outcome {
            // The timeout elapsed. `Timeout` is unsettled in the class's own table, which is the honest
            // reading: the server accepted the request and may be executing it.
            Err(_) => Err(ToolErrorClass::Timeout),
            Ok(Err(error)) => Err(classify_service_error(&error)),
            Ok(Ok(result)) => normalize_result(&result),
        }
    }
}

/// Maps this adapter's own refusal onto the class the ledger records.
///
/// Delegates to the refusal's own `class`, so the mapping has one definition. Written as a function rather
/// than inlined so the reason it is not a method on the executor is visible: the class belongs to the
/// refusal, not to whoever happened to observe it.
const fn class_of_refusal(refusal: &McpCallRefusal) -> ToolErrorClass {
    refusal.class()
}

/// Normalizes a completed `CallToolResult` into a body, or the class its failure carries.
///
/// `call_tool_with_mrtr_max_rounds` returns the **completed** result only, so this is the arm
/// `normalize_response` reaches through `CallToolResponse::Complete` — the two agree because the SDK's loop
/// returns `CallToolResult` for exactly the case `decide_response` calls complete.
fn normalize_result(
    result: &CallToolResult,
) -> Result<jarvis_domain::tool::call::ToolResultBody, ToolErrorClass> {
    match normalize_response(&rmcp::model::CallToolResponse::Complete(result.clone())) {
        Ok(body) => Ok(body),
        Err(refusal) => Err(class_of_refusal(&refusal)),
    }
}

impl ToolExecutor for McpToolExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest<'a>,
        cancel: &'a CancellationScope,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            match self
                .invoke(
                    request.identity,
                    request.arguments,
                    cancel,
                    request.timeout_ms,
                )
                .await
            {
                Ok(result) => Ok(result),
                // **A class that means the outcome is unsettled is reported as `Ambiguous`, not as a plain
                // failure.** The port distinguishes them because the recovery pass treats them differently:
                // `Ambiguous` is a call whose effect may exist and which only a read of the tool's own state
                // can settle, while `Failed` is one that provably did not have an effect. `Timeout` and
                // `ProviderError` are exactly the unsettled classes, and `is_unsettled` is the domain's own
                // answer to that question rather than a list repeated here.
                Err(class) if class.is_unsettled() => Err(ToolExecutionError::Ambiguous),
                Err(class) => Err(ToolExecutionError::Failed(class)),
            }
        })
    }
}
