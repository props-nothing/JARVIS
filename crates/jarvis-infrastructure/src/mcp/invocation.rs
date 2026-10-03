//! The invocation guard: what JARVIS refuses before an MCP call runs, and what a continuation may ask for.
//!
//! [`super::outcome`] refuses a multi round-trip (`input_required`) response with the sentence "it needs
//! JARVIS policy and approval routing for the requested input, and a bounded round count". This module is
//! that decision, and building it produced the finding that makes the refusal *correct* rather than
//! merely unimplemented.
//!
//! # The finding: every multi-round-trip input request is a capability JARVIS must not lend out
//!
//! `InputRequest` has exactly three variants, and each one is a feature the specification has deprecated
//! and this adapter already records as unsupported:
//!
//! | Variant | What it asks the *client* to do | Specification status |
//! | --- | --- | --- |
//! | `CreateMessage` (sampling) | Run a model call and return the completion | Deprecated, SEP-2577 — "integrate directly with LLM provider APIs instead" |
//! | `ListRoots` (roots) | Return the client's directory list | Deprecated, SEP-2577 — "pass directories or files via tool parameters" |
//! | `Elicitation` | Prompt the client's user for input | Elicitation was *replaced* by this very pattern |
//!
//! None of the three is a data interchange. Each is a request for JARVIS to spend something it owns on a
//! server's behalf: **model budget** (sampling), the **shape of the filesystem** (roots), or the **user's
//! attention and consent** (elicitation). Lending any of them to an untrusted server is the opposite of
//! what the tool fabric is for — and `ListRoots` in particular is a remote server enumerating the
//! workspace layout, which is an information-disclosure primitive dressed as a convenience.
//!
//! So the default here is **refuse, by name**, and that is a decision rather than a gap. A future
//! version that services one of them would be adding a capability with its own research gate, not
//! filling in a stub.
//!
//! # The second job: an invocation must be the tool authority was recorded against
//!
//! `MCP-C004`-adjacent and `ACC-024`'s rule: the tool that runs must be the tool a grant was recorded
//! against. Discovery is untrusted — a server can rename, re-describe, or re-schema a tool between the
//! listing and the call — so the check is made at invocation time and it **delegates to
//! [`ToolIdentity::authorizes`]** rather than comparing fields here. That method is the single place the
//! question is answered, and a second comparison would be a weaker answer living beside it.

use jarvis_domain::tool::identity::ToolIdentity;
use rmcp::model::{InputRequest, InputRequests};

/// The greatest number of multi-round-trip continuations JARVIS will drive for one call.
///
/// Three, and bounded rather than configurable upward here: each round is a server-initiated request that
/// JARVIS must decide, and an unbounded round count turns one tool call into an unbounded loop driven by
/// the peer.
///
/// **This is the value handed to the SDK's loop, and it is the only way JARVIS's bound takes effect.**
/// `McpToolExecutor::invoke` passes it as `max_rounds` to `call_tool_with_mrtr_max_rounds`, so the SDK's
/// loop stops at *this* number rather than at its own `DEFAULT_MRTR_MAX_ROUNDS` of 10.
///
/// A constant here used to be accompanied by a `next_round` function and a
/// `McpInvocationRefusal::RoundLimitExceeded` variant — a counter that was supposed to refuse *before*
/// handing the call over. Both were deleted: nothing called the function, because the loop that would
/// count rounds belongs to the SDK and is not interceptable at that level. Its doc claimed a caller that
/// did not exist and that JARVIS's refusal would be "the one an operator sees", and neither was true.
pub const MAX_MRTR_ROUNDS: usize = 3;

// **A compile-time invariant rather than a test, because both sides are constants.** JARVIS's bound is
// passed to the SDK's loop as `max_rounds`, so it is the number the loop honours; the SDK's own default is
// the fallback for callers that pass nothing. Keeping JARVIS's below it means the value in the launch path
// is always the one in force rather than being silently superseded.
const _: () = assert!(MAX_MRTR_ROUNDS < rmcp::model::DEFAULT_MRTR_MAX_ROUNDS);
const _: () = assert!(MAX_MRTR_ROUNDS > 0);

/// The kind of input a server asked for, named so a refusal is reportable.
///
/// A JARVIS-owned enum rather than the SDK's `InputRequest`, so the decision is not expressed in provider
/// types — the same boundary rule the rest of this adapter follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputKind {
    /// The server asked JARVIS to run a model call on its behalf (sampling).
    Sampling,
    /// The server asked for the client's directory list (roots).
    Roots,
    /// The server asked JARVIS to prompt its user (elicitation).
    Elicitation,
}

impl InputKind {
    /// Returns the kind of a request, or `None` for a variant a future SDK adds.
    ///
    /// `None` rather than a default kind, because an unrecognised request must not be classified as a
    /// known-but-refused one: the refusal's *reason* would then be wrong, and a reason that does not
    /// match the request sends an operator to the wrong place.
    #[must_use]
    pub const fn of(request: &InputRequest) -> Option<Self> {
        match request {
            InputRequest::CreateMessage(_) => Some(Self::Sampling),
            InputRequest::ListRoots(_) => Some(Self::Roots),
            InputRequest::Elicitation(_) => Some(Self::Elicitation),
            _ => None,
        }
    }

    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Sampling => "mcp.input_sampling_refused",
            Self::Roots => "mcp.input_roots_refused",
            Self::Elicitation => "mcp.input_elicitation_refused",
        }
    }

    /// Returns the contract's own reason for the refusal.
    ///
    /// Quoting the deprecation's migration path rather than inventing a rationale, so the operator sees
    /// where the specification expects the peer to go instead: a server wanting a directory receives it
    /// as a **tool parameter**, and one wanting a completion integrates with a provider itself.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Sampling => {
                "sampling asks JARVIS to spend its model budget on a server's behalf; \
                 the provider integration belongs to the server (SEP-2577)"
            }
            Self::Roots => {
                "roots asks JARVIS to enumerate its filesystem for a server; \
                 directories are passed as tool parameters instead (SEP-2577)"
            }
            Self::Elicitation => {
                "elicitation asks JARVIS to prompt its own user at a server's request; \
                 the prompt and its consent belong to JARVIS, not to the caller"
            }
        }
    }
}

/// Why an MCP invocation or one of its continuations was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpInvocationRefusal {
    /// The tool being invoked is not the one authority was recorded against.
    #[error("the offered tool is not the tool that was authorized")]
    ToolIdentityChanged,
    /// The server asked for an input kind JARVIS does not service.
    #[error("mcp input request `{kind:?}` is refused: {reason}")]
    InputRefused {
        /// The kind that was asked for.
        kind: InputKind,
        /// The contract's reason, so the message is actionable without a lookup.
        reason: &'static str,
    },
    /// The server asked for an input request kind this build cannot classify.
    #[error("mcp input request could not be classified and is refused")]
    InputKindUnknown,
}

impl McpInvocationRefusal {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ToolIdentityChanged => "mcp.tool_identity_changed",
            Self::InputRefused { kind, .. } => kind.code(),
            Self::InputKindUnknown => "mcp.input_kind_unknown",
        }
    }
}

/// Verifies that the tool about to be invoked is the one authority was recorded against.
///
/// **Delegates to [`ToolIdentity::authorizes`] rather than comparing capability, source, and fingerprint
/// here.** That method is documented as "the only place the question is answered", and a second
/// comparison would be a weaker answer beside it — one that could, for instance, compare only the
/// capability and accept a schema change.
///
/// # Errors
///
/// Returns [`McpInvocationRefusal::ToolIdentityChanged`] when the identities differ, and *only* then: an
/// identical identity authorizes, and a refusal for any other reason would be indistinguishable from a
/// mismatch.
pub fn authorizes_invocation(
    granted: &ToolIdentity,
    offered: &ToolIdentity,
) -> Result<(), McpInvocationRefusal> {
    if granted.authorizes(offered) {
        return Ok(());
    }
    Err(McpInvocationRefusal::ToolIdentityChanged)
}

/// Decides one server-initiated input request.
///
/// Refuses every kind this build can classify — see the module doc for why that is the decision rather
/// than a stub — and refuses an unclassifiable one separately, so the reason always matches the request.
///
/// # Errors
///
/// Returns [`McpInvocationRefusal::InputRefused`] naming the kind, or
/// [`McpInvocationRefusal::InputKindUnknown`] for a variant a future SDK adds.
pub fn decide_input_request(request: &InputRequest) -> Result<(), McpInvocationRefusal> {
    match InputKind::of(request) {
        Some(kind) => Err(McpInvocationRefusal::InputRefused {
            kind,
            reason: kind.reason(),
        }),
        None => Err(McpInvocationRefusal::InputKindUnknown),
    }
}

/// Decides a whole round of input requests, refusing if any one of them is refused.
///
/// **Refuses the round rather than the offending request alone.** A continuable round is answered as a
/// unit: servicing the acceptable requests while refusing one would tell the server that JARVIS engaged
/// with its demand, and the server chose the set — so the set is what is judged.
///
/// An **empty** round is refused too, and that is deliberate rather than a technicality: a server that
/// answers `input_required` and then asks for nothing is not asking a question, and retrying on that
/// basis would loop.
///
/// **`call.rs` is the production caller, and it is reached only for a round that names at least one
/// request.** The two other shapes never arrive here, and the reason differs by shape: a round that asks
/// for something is refused by the SDK's *server* half before it is sent (the client declared no
/// server-initiated capabilities), and an empty round is handled inside the SDK's own loop. So this
/// function's empty-round arm and [`decide_input_request`] are both correct and both unreachable from a
/// live session — see `client.rs`, whose handler overrides are what a server actually meets, and
/// `mcp_executor_process`'s input-required tests, which pin what a caller really receives.
///
/// # Errors
///
/// Returns the refusal of the first refused request, in the map's own (sorted) order so the answer is
/// stable, or [`McpInvocationRefusal::InputKindUnknown`] when the round is empty.
pub fn decide_input_round(requests: &InputRequests) -> Result<(), McpInvocationRefusal> {
    let Some(first) = requests.values().next() else {
        // No requests is not "nothing to decide": it is a continuation with no question, which cannot be
        // answered and must not be retried.
        return Err(McpInvocationRefusal::InputKindUnknown);
    };
    decide_input_request(first)
}

#[cfg(test)]
#[path = "invocation_tests.rs"]
mod tests;
