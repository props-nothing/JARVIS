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
    session: Arc<RunningService<RoleClient, JarvisClient>>,
    /// The identities this executor may call, from the catalog the session produced.
    ///
    /// A **set of identities**, not the catalog itself: the call path needs membership, and holding the
    /// schemas here would put a second copy of them beside the argument validator's.
    callable: Arc<std::collections::BTreeSet<ToolIdentity>>,
    /// How long one call may take, including every continuation.
    timeout_ms: u64,
}

impl std::fmt::Debug for McpToolExecutor {
    /// Reports the callable count and the timeout, never the identities' sources or the session.
    ///
    /// The session is not usefully printable, and an identity list can be long — the count is the fact a
    /// diagnostic line needs.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpToolExecutor")
            .field("callable", &self.callable.len())
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

impl McpToolExecutor {
    /// Builds an executor over a live session and the identities it may call.
    ///
    /// `timeout_ms` is bounded by the caller, and every call is wrapped in it: without a bound a server that
    /// accepts a request and never answers holds the dispatcher open with no clock, which is the failure the
    /// `ToolExecutor` port's own documentation names.
    #[must_use]
    pub fn new(
        session: Arc<RunningService<RoleClient, JarvisClient>>,
        callable: impl IntoIterator<Item = ToolIdentity>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            session,
            callable: Arc::new(callable.into_iter().collect()),
            timeout_ms,
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

    /// Sends one `tools/call` and normalizes what comes back.
    ///
    /// # Errors
    ///
    /// Returns the class the call is recorded under. A refusal this adapter made itself (an unaddressable
    /// capability, an argument document that is not an object, an identity it may not call) carries the
    /// class `call.rs` assigns that refusal; a transport failure carries the class `outcome.rs` assigns it.
    async fn invoke(
        &self,
        identity: &ToolIdentity,
        arguments: &jarvis_domain::tool::call::ToolArguments,
        cancel: &CancellationScope,
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
        let params = match call_params(identity, arguments) {
            Ok(params) => params,
            Err(refusal) => return Err(class_of_refusal(&refusal)),
        };

        let call = self
            .session
            .call_tool_with_mrtr_max_rounds(params, MAX_MRTR_ROUNDS);
        // The timeout bounds the whole exchange including continuations, and the cancellation race is
        // **outside** it so a cancelled call reports `Cancelled` rather than a timeout: a caller that
        // stopped the work must not be told the server was slow.
        let bounded = tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), call);
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
                .invoke(request.identity, request.arguments, cancel)
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
