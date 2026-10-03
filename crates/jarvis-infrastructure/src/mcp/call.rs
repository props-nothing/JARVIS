//! Calling a discovered MCP tool: the pure decisions between a JARVIS call and `tools/call`.
//!
//! This is the caller `invocation.rs` and `outcome.rs` were written for. `outcome.rs` normalizes what a
//! server *returns*, `invocation.rs` decides what an *invocation* may be, and until now neither had a
//! production caller — the decisions were tested and unreached.
//!
//! # The name on the wire is derived from the canonical capability, and that is not cosmetic
//!
//! A call carries a canonical [`ToolIdentity`], and the server expects the name it listed. Those are
//! different strings: the capability is `mcp.<name>@1`, and the wire name is `<name>`. [`wire_name`]
//! reverses [`super::capability_for`] through the domain's own accessors rather than by string surgery, so
//! the two directions agree by construction and a capability from another namespace or major is refused
//! rather than coerced.
//!
//! It deliberately does **not** send the canonical string and strip a prefix on failure. A transformation
//! applied on failure is one applied to a value the server never agreed to, and it fails quietly: a server
//! that happened to accept it would work while the mapping stayed wrong.
//!
//! # Two failure classes that are easy to conflate, and whose difference is whether an effect happened
//!
//! `outcome.rs` distinguishes "never delivered" from "delivered and then unsettled", and the class carried
//! here preserves it rather than flattening both into a generic failure. `TransportSend` maps to
//! `Unavailable` — nothing ran, so a retry is safe — while `TransportClosed` and `Timeout` map to
//! `ProviderError`, where the request may already have executed and a retry could duplicate it. Reporting a
//! lost response as `Unavailable` would tell a caller a write can be repeated when it may already have
//! happened, so the mapping is `outcome.rs`'s [`super::outcome::classify_service_error`] and this module
//! neither restates nor re-exports it — a second spelling of one fact is the defect this workspace
//! records most often.
//!
//! # What is decided here, and what is deliberately not
//!
//! Everything in this module is a pure function of values, so each decision is testable without a server —
//! the split `client.rs` and `outcome.rs` already use. The multi round-trip **loop** is the SDK's:
//! driving it needs its client, and it is not interceptable per-round, which is why this module owns the
//! per-response decision ([`decide_response`]) and nothing at all owns a round counter. JARVIS's
//! contribution to the loop is the **cap** it passes — [`super::invocation::MAX_MRTR_ROUNDS`] — and the
//! refusal *policy* for a round it is asked about.
//!
//! **Two of this module's arms are unreachable from a live session, and a reader must not treat them as
//! live.** `decide_response` is not called on the call path at all — `McpToolExecutor::invoke` goes
//! straight through `normalize_response` — and the SDK's loop is what meets a multi round-trip response:
//! it converts a task handle itself, and for `input_required` it routes each request to `JarvisClient`,
//! which refuses by kind. Both unreachable arms are nonetheless correct and kept, because they are the
//! honest answer for the *response shape* and the fail-safe direction if the SDK's helper ever stops
//! swallowing a variant. What a caller actually meets is pinned against a real child in
//! `mcp_executor_process`.

use jarvis_domain::tool::call::{ToolArguments, ToolResultBody};
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{ToolCapability, ToolIdentity};
use rmcp::model::{CallToolRequestParams, CallToolResponse};

use super::invocation::{McpInvocationRefusal, authorizes_invocation, decide_input_round};
use super::outcome::{McpCallOutcome, normalize_call_response};
use super::{CONTRACT_NAMESPACE, INITIAL_CONTRACT_MAJOR};

/// Why an MCP call did not produce a JARVIS result.
///
/// Distinct from [`McpInvocationRefusal`], which is a *decision* to refuse something, and from
/// [`ToolErrorClass`], which is what the ledger records. This is the boundary value: a reason a call
/// produced no result, each naming a different operator action.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpCallRefusal {
    /// The identity names a capability this adapter did not construct.
    ///
    /// Reported rather than guessed at: the alternative puts a name on the wire the server never listed.
    #[error("capability `{capability}` is not one this adapter can address a server with")]
    NotAnMcpCapability {
        /// The capability that was presented.
        capability: String,
    },
    /// The invocation guard refused — an identity that moved, or an input request this build will not
    /// service.
    #[error("mcp invocation refused: {refusal}")]
    Refused {
        /// The guard's own reason.
        refusal: McpInvocationRefusal,
    },
    /// The server's answer could not be normalized into a JARVIS result.
    #[error("mcp tool result was refused: {reason}")]
    ResultRefused {
        /// The outcome refusal's stable code.
        code: &'static str,
        /// The refusal in prose, so an operator reads it without a lookup.
        reason: String,
    },
    /// The server reported a tool-level failure, or the call failed in a way that carries its own class.
    #[error("mcp tool call failed: {class}")]
    Failed {
        /// The contract's error class, as the ledger records it.
        class: ToolErrorClass,
    },
    /// The arguments are not a JSON object, which `tools/call` requires.
    #[error("mcp tool arguments are not a JSON object")]
    ArgumentsNotAnObject,
}

impl McpCallRefusal {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotAnMcpCapability { .. } => "mcp.not_an_mcp_capability",
            Self::Refused { refusal } => refusal.code(),
            Self::ResultRefused { code, .. } => code,
            Self::Failed { class } => class.as_contract_str(),
            Self::ArgumentsNotAnObject => "mcp.arguments_invalid",
        }
    }

    /// Returns the class the ledger records for this refusal.
    ///
    /// **`Unavailable` for everything that never reached the server**, because that class's `Safe` retry
    /// posture is a claim about an effect: a refusal is a local decision, and a non-object argument
    /// document never left. `ResultRefused` is `OutputInvalid` — the server answered and the answer was
    /// unusable — and a failure that arrived carrying its own class is passed through rather than
    /// re-derived, so this cannot disagree with `outcome.rs`.
    #[must_use]
    pub const fn class(&self) -> ToolErrorClass {
        match self {
            Self::NotAnMcpCapability { .. } | Self::Refused { .. } | Self::ArgumentsNotAnObject => {
                ToolErrorClass::Unavailable
            }
            Self::ResultRefused { .. } => ToolErrorClass::OutputInvalid,
            Self::Failed { class } => *class,
        }
    }
}

/// The wire name for a canonical MCP capability, or a refusal.
///
/// # Errors
///
/// Returns [`McpCallRefusal::NotAnMcpCapability`] when the namespace or major is not this adapter's. The
/// name needs no further validation: it came from a `ToolCapability`, which already refuses anything that
/// is not a usable segment — so a value reaching a server is one the canonical construction accepted.
pub fn wire_name(capability: &ToolCapability) -> Result<&str, McpCallRefusal> {
    if capability.namespace() == CONTRACT_NAMESPACE && capability.major() == INITIAL_CONTRACT_MAJOR
    {
        return Ok(capability.name());
    }
    Err(McpCallRefusal::NotAnMcpCapability {
        capability: capability.to_string(),
    })
}

/// Builds the `tools/call` parameters for one invocation.
///
/// # Errors
///
/// Returns [`McpCallRefusal::NotAnMcpCapability`] when the identity is not addressable, and
/// [`McpCallRefusal::ArgumentsNotAnObject`] when the argument document is not a JSON object.
///
/// The object check is **not** redundant with schema validation. A schema-validated call should always
/// pass it, and this is the one place a non-object argument would otherwise become an error the *SDK*
/// raises — arriving as `UnexpectedResponse` about the response, which sends an operator to the wrong side
/// of the call.
pub fn call_params(
    identity: &ToolIdentity,
    arguments: &ToolArguments,
) -> Result<CallToolRequestParams, McpCallRefusal> {
    let name = wire_name(&identity.capability)?;
    let parsed: serde_json::Value = serde_json::from_str(arguments.as_str())
        .map_err(|_| McpCallRefusal::ArgumentsNotAnObject)?;
    let Some(object) = parsed.as_object() else {
        return Err(McpCallRefusal::ArgumentsNotAnObject);
    };
    let mut params = CallToolRequestParams::new(name.to_owned());
    params.arguments = Some(object.clone());
    Ok(params)
}

/// What one response means for the invocation that produced it.
///
/// Two arms, and the second is why this type exists rather than a `bool`: "the server asked for input
/// JARVIS will not provide" is a different operator fact from "the call failed", and a caller that
/// collapsed them would report a policy refusal as a server fault.
///
/// **Neither arm is reached through the executor, which is the point of the type rather than a defect in
/// it.** `McpToolExecutor::invoke` calls `normalize_response` directly — it never asks `decide_response` —
/// so the decision below is a *policy* expression whose production consumers are the handler overrides in
/// `client.rs` (same vocabulary, same codes) and the tests. Anything the loop cannot hand to the handler
/// reaches the port as an error class instead, which is why the executor's own assertions are about
/// [`ToolErrorClass`] and not about this enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseDecision {
    /// A completed result, ready for [`normalize_response`].
    Complete,
    /// The call must not continue, because the server asked for something this build refuses. The reason
    /// is carried so the operator sees which kind was asked for.
    Refused {
        /// The guard's reason.
        refusal: McpInvocationRefusal,
    },
}

/// Classifies one `tools/call` response against the invocation that produced it.
///
/// # The identity guard runs on every response, not only the first
///
/// A server may re-schema a tool between the listing and the call, and a multi round-trip continuation is
/// a fresh request against the same identity. Checking once at the start would leave every later round
/// unchecked, so this is a function of one response and repeats the check each time it is called.
/// `authorizes_invocation` is the single place that question is answered, so this cannot be the weaker
/// capability-only comparison.
///
/// # Why "asked for input" is a refusal and not a continuation
///
/// `invocation.rs` refuses every input kind this build can classify, and the reason is that all three ask
/// JARVIS to spend something it owns on a server's behalf — model budget, the shape of the filesystem, or
/// the user's attention. A "continue" arm would therefore be a decision to service one of them.
///
/// # Errors
///
/// Returns [`McpCallRefusal::Refused`] for a response that ends the call — an identity that moved, an
/// input request refused by policy — and [`McpCallRefusal::ResultRefused`] for a task handle or a response
/// kind a future SDK adds. The latter two are refused here rather than left to the normalizer because a
/// caller that *continued* on a task handle would be polling something this adapter does not implement.
pub fn decide_response(
    authorized: &ToolIdentity,
    offered: &ToolIdentity,
    response: &CallToolResponse,
) -> Result<ResponseDecision, McpCallRefusal> {
    authorizes_invocation(authorized, offered)
        .map_err(|refusal| McpCallRefusal::Refused { refusal })?;

    match response {
        CallToolResponse::Complete(_) => Ok(ResponseDecision::Complete),
        CallToolResponse::InputRequired(required) => {
            // `input_requests` is optional, and an absent one means the server asked for nothing. That is
            // answered by `decide_input_round`'s empty-round rule as `InputKindUnknown` rather than as
            // "nothing to do", because a continuation with no question cannot be answered and must not be
            // retried — so `unwrap_or_default` loses no information.
            let requests = required.input_requests.clone().unwrap_or_default();
            match decide_input_round(&requests) {
                Err(refusal) => Ok(ResponseDecision::Refused { refusal }),
                // Unreachable by that function's contract — it refuses every round, including an empty
                // one. Refused rather than continued, because an `Ok` here would mean the contract
                // changed, and continuing on a continuation nobody understood is the direction that
                // engages with a server's demand.
                Ok(()) => Ok(ResponseDecision::Refused {
                    refusal: McpInvocationRefusal::InputKindUnknown,
                }),
            }
        }
        // **Unreachable in the call path, for two reasons, and the second one is the interesting one.**
        //
        // The first is the same shape as the `InputRequired` arm above: the SDK's own loop matches these
        // variants before any JARVIS decision function is consulted, and it converts a task handle to
        // `ServiceError::UnexpectedResponse` (SEP-2663: `call_tool_with_mrtr_max_rounds` "does not drive the
        // task polling lifecycle").
        //
        // The second is that **a conforming server never sends this at all**, because the SDK's server half
        // refuses to emit a task handle unless the client declared the tasks extension capability — and
        // `JarvisClient` deliberately declares no server-initiated capabilities. So what a non-conforming or
        // older-peer server produces is the JSON-RPC code `-32021`
        // (`MISSING_REQUIRED_CLIENT_CAPABILITY`), which reaches this adapter as an error rather than as a
        // response, and `outcome::classify_error_code` classes it `Unavailable` — a *different* class from the
        // `OutputInvalid` assigned below.
        //
        // **This arm is kept and is correct**: it is the honest answer for the response shape, it is
        // unit-tested through `decide_response`, and it is the fail-safe direction if the SDK's helper ever
        // stops swallowing the variant. But a reader must not conclude from it that a task handle reaches a
        // caller as `mcp.response_not_complete`; `mcp_executor_process`'s
        // `a_task_handle_answer_is_refused_at_the_negotiated_capability_not_by_jarvis` pins what actually
        // arrives, against a real child process.
        CallToolResponse::Task(_) => Err(McpCallRefusal::ResultRefused {
            code: "mcp.response_not_complete",
            reason: "the server returned a task handle rather than a result".to_owned(),
        }),
        // `#[non_exhaustive]`: a kind a future SDK adds is refused rather than assumed complete, which is
        // the fail-safe direction for a type whose other variants are not results.
        _ => Err(McpCallRefusal::ResultRefused {
            code: "mcp.response_not_complete",
            reason: "the server returned an unrecognised response kind".to_owned(),
        }),
    }
}

/// Converts a completed response into a JARVIS result body, or the class its failure carries.
///
/// # Errors
///
/// Returns [`McpCallRefusal::ResultRefused`] when the result is one this adapter cannot represent, and
/// [`McpCallRefusal::Failed`] when the server reported a tool-level failure. Separate arms because only
/// the second is an outcome the model should be told about: the tool ran and said it did not work.
pub fn normalize_response(response: &CallToolResponse) -> Result<ToolResultBody, McpCallRefusal> {
    match normalize_call_response(response) {
        Ok(McpCallOutcome::Succeeded(body)) => Ok(body),
        Ok(McpCallOutcome::Failed(class)) => Err(McpCallRefusal::Failed { class }),
        Err(rejection) => Err(McpCallRefusal::ResultRefused {
            code: rejection.code(),
            reason: rejection.to_string(),
        }),
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
