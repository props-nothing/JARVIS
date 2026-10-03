//! Client-side decisions: which protocol versions to ask for, and what a failed startup means.
//!
//! The sibling of [`super::outcome`], one step earlier in the conversation. That module classifies what
//! a server returns from a call; this one classifies what happens when the *connection* fails, and
//! decides which versions JARVIS is willing to negotiate. Both are pure.
//!
//! # Why startup needs its own classifier
//!
//! `rmcp` surfaces startup failures as [`ClientInitializeError`], an eleven-variant enum with no
//! mapping into JARVIS terms — and the note's own error table has a row for exactly this
//! ("Incompatible protocol → mark incompatible, record the server's `supportedVersions`, explain
//! supported versions"). Without this module that row has no implementation, and every startup failure
//! would reach an operator as the same undifferentiated string.
//!
//! # Two questions, deliberately separate
//!
//! A failed startup raises two questions that look like one and are not:
//!
//! 1. **May this be retried?** Answered by [`ToolErrorClass`], whose posture is about *effect
//!    duplication*. A startup that failed before any tool call ran cannot have duplicated anything, so
//!    this answer is often permissive.
//! 2. **Should this server be quarantined?** Answered by [`is_permanent_startup_failure`], which is
//!    about *whether a retry could ever succeed*. A server whose `supportedVersions` excludes ours will
//!    answer identically forever.
//!
//! Conflating them is the mistake this module exists to prevent, in both directions: treating an
//! incompatible peer as retryable loops forever against a server that cannot be served, and treating a
//! transient failure as permanent quarantines a server that was merely restarting.
//!
//! # The one place this module is deliberately permissive
//!
//! `ToolErrorClass::Unavailable`'s posture is `Safe` — "retrying cannot duplicate an effect". That is
//! true of a startup, which has no side effect, even though its usual reading is about a request that
//! never left. The class is reused rather than a new `tool.*` code invented for it: adding a contract
//! code is a contract change, and the compatibility row in [`super::outcome`] already records the same
//! reasoning for the runtime case.

use jarvis_domain::tool::error_class::ToolErrorClass;
// **The allow is on the import rather than at the file level, and it is required for a reason worth
// stating.** These four types are the *signature* of the deprecated feature this client has to refuse, so
// naming them is how the refusal is written; a file-level `#![allow(deprecated)]` would also silence the
// lint everywhere else in this module, where a deprecated item would be a genuine mistake.
use rmcp::ErrorData;
use rmcp::model::ProtocolVersion;
#[allow(deprecated)]
use rmcp::model::{
    CreateMessageRequestParams, CreateMessageResult, ElicitRequestParams, ElicitResult,
    ListRootsResult,
};
use rmcp::service::{ClientInitializeError, RequestContext, RoleClient};

/// The protocol versions JARVIS prefers, newest first.
///
/// **The target is pinned first and the rest is derived from the SDK's own `KNOWN_VERSIONS`**, rather
/// than a list written out here. A hand-written list is a second source of truth for which revisions
/// exist, and it fails in the quiet direction: an SDK upgrade that adds a revision would leave this
/// function still offering the old set, and nothing would report the omission.
///
/// The target is still first **explicitly** rather than by taking the newest known version, because
/// that is the difference between "prefer what this adapter is written against" and "prefer whatever
/// the dependency happens to know". Reversing the known set alone would silently promote a future
/// revision ahead of the one the adapter has been reviewed against.
///
/// More than one entry matters: the discovery lifecycle resolves one version per session from the
/// intersection of both sides' sets, so a client offering only its favourite has no fallback when the
/// peer is one revision behind — and the note's compatibility story depends on offering more.
#[must_use]
pub fn preferred_protocol_versions() -> Vec<ProtocolVersion> {
    let mut versions = vec![TARGET_PROTOCOL_VERSION];
    versions.extend(
        ProtocolVersion::KNOWN_VERSIONS
            .iter()
            .rev()
            .filter(|version| *version != &TARGET_PROTOCOL_VERSION)
            .cloned(),
    );
    versions
}

/// The protocol version JARVIS targets, and the only one whose features this adapter assumes.
///
/// Separate from [`preferred_protocol_versions`] because the two answer different questions: that one
/// is what to *offer* during negotiation, this is what the adapter's behaviour was written against. A
/// session that negotiated `2025-11-25` is usable but does not have the stateless lifecycle, so code
/// that assumed per-request `_meta` would be wrong about it.
pub const TARGET_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// Resolves the version to use from what a peer reports it supports, or `None` if nothing is shared.
///
/// **Delegates to the SDK's own `select_protocol_version` rather than reimplementing the rule.** The
/// SDK's function is "the first client-preferred version the server supports", and JARVIS's only
/// contribution is the *preference order* in [`preferred_protocol_versions`]. Writing a second
/// selection rule here would mean two places decide which revision a session runs at, and a disagreement
/// between them would be invisible: both would return a version, and only one would be the one the
/// handshake actually used.
///
/// `None` is the exact condition the SDK raises [`ClientInitializeError::NoCompatibleProtocolVersion`]
/// for, so a caller can check before connecting and explain a refusal without parsing an error string.
#[must_use]
pub fn negotiate_protocol_version(server_supported: &[ProtocolVersion]) -> Option<ProtocolVersion> {
    rmcp::select_protocol_version(&preferred_protocol_versions(), server_supported)
}

/// The client identity JARVIS presents to an MCP server, for **every** session this adapter opens.
///
/// One type for discovery and for calls, and that is a correction rather than a tidy-up: the two were
/// separate (`DiscoveryClient` and `CallClient`) and both were empty, which is two declarations of one
/// posture. The posture is "declare no server-initiated capabilities", and it has one reason — every input
/// request the SDK could route here is refused by `invocation.rs`, so a client advertising a capability it
/// refuses to service would be claiming something untrue.
///
/// **A newtype rather than implementing `ClientHandler` for the unit type.** The SDK already provides
/// `impl ClientHandler for ()`, and implementing it here would make `()` the client identity everywhere it
/// appeared, including anywhere this adapter did not intend a session. A named type keeps the identity
/// greppable and lets `#![deny(clippy::...)]` on this crate stay meaningful about it.
///
/// The client identity JARVIS presents to an MCP server, for **every** session this adapter opens.
///
/// One type for discovery and for calls, and that is a correction rather than a tidy-up: the two were
/// separate (`DiscoveryClient` and `CallClient`) and both were empty, which is two declarations of one
/// posture. The posture is "declare no server-initiated capabilities", and it has one reason — every input
/// request the SDK could route here is refused by `invocation.rs`, so a client advertising a capability it
/// refuses to service would be claiming something untrue.
#[derive(Debug, Clone, Default)]
pub struct JarvisClient;

/// The default `get_info` is the SDK's stock client identity and capability set, which is what makes
/// "declares nothing" true rather than merely intended.
///
/// # Every server-initiated input request is refused **here**, and the defaults are not good enough
///
/// `invocation.rs` owns the policy — `decide_input_request` refuses sampling, roots, and elicitation by
/// name, and `decide_response` consults it. But the SDK's multi-round-trip loop does **not** call that
/// code: `RunningService::call_tool_with_mrtr_max_rounds` handles each `input_required` itself, routing
/// every request to this handler's `ClientHandler` methods. `decide_response`'s `InputRequired` arm is
/// therefore unreachable in production, and what a server actually meets is whatever these methods
/// answer. The SDK's defaults are **not** a refusal for two of the three:
///
/// - `create_message` (sampling) → `method_not_found`, which is a refusal and is left alone.
/// - `list_roots` → **`Ok(ListRootsResult::default())`**, i.e. the request is *served* with an empty root
///   list. That is the opposite of JARVIS's policy, which refuses roots as an information-disclosure
///   primitive, and a server receiving a success would be told JARVIS has no roots rather than that
///   JARVIS refuses the question.
/// - `create_elicitation` → `Ok(ElicitResult { action: Decline })`, which *declines on the user's
///   behalf* without ever asking them. That reaches the server as a user decision, when no user was
///   consulted — an invented consent outcome rather than a refusal.
///
/// So the three are overridden to return a JSON-RPC error carrying JARVIS's own reason and refusal code.
/// The message is built from `InputKind::reason()`, which is the same sentence `invocation.rs` refuses
/// with, so the operator's log and the server's error say one thing rather than two.
///
/// **`#[allow(deprecated)]` is deliberate and scoped.** Roots and sampling are deprecated by SEP-2577,
/// and the SDK marks the trait methods accordingly — but a deprecated *feature* still has to be
/// *refused* while it exists, and refusing it is precisely how this adapter honours the deprecation.
/// Silencing the lint for these three methods is the honest form of that: the alternative is not calling
/// the methods, which is what leaves the SDK's permissive defaults in place.
impl rmcp::ClientHandler for JarvisClient {
    /// Refuses a sampling request: it asks JARVIS to spend its model budget on a server's behalf.
    ///
    /// The SDK's default already refuses this one, and it is overridden anyway so all three refusals
    /// share one reason vocabulary and one code shape — a server that meets two JARVIS refusals in one
    /// session should not find that one of them is phrased like an SDK error.
    #[allow(deprecated)]
    fn create_message(
        &self,
        _params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<CreateMessageResult, ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        std::future::ready(Err(refused_input(super::invocation::InputKind::Sampling)))
    }

    /// Refuses a roots request rather than answering it with an empty list.
    ///
    /// **The one that matters.** The SDK's default returns success, so without this override a server
    /// asking for the filesystem shape gets a `200` with no roots — indistinguishable, to that server,
    /// from a client that has none, and a success where JARVIS's policy is a refusal.
    #[allow(deprecated)]
    fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ListRootsResult, ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        std::future::ready(Err(refused_input(super::invocation::InputKind::Roots)))
    }

    /// Refuses an elicitation request rather than declining it on the user's behalf.
    ///
    /// **The SDK's default invents an outcome no user gave.** `Decline` is a decision, and it travels to
    /// the server as one; the request was never shown to anyone. Refusing says the true thing — JARVIS
    /// does not service this — and leaves the prompt and its consent where `invocation.rs` says they
    /// belong.
    fn create_elicitation(
        &self,
        _request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> impl Future<Output = Result<ElicitResult, ErrorData>> + rmcp::service::MaybeSendFuture + '_
    {
        std::future::ready(Err(refused_input(
            super::invocation::InputKind::Elicitation,
        )))
    }
}

/// Builds the JSON-RPC error a refused server-initiated input request is answered with.
///
/// **One construction site, so the three overrides cannot drift.** The code travels in the error's
/// `data` as well as its message, because a JSON-RPC error code is a number and JARVIS's refusals are
/// named by a `mcp.*` string — the operator greps for the string, and a peer that wants to branch on it
/// has it without parsing prose.
fn refused_input(kind: super::invocation::InputKind) -> ErrorData {
    ErrorData::invalid_request(kind.reason(), None)
}

/// What a failed startup means, in both of the senses the module doc separates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupFailure {
    /// The retry posture: whether a repeat may duplicate an effect.
    pub class: ToolErrorClass,
    /// Whether a retry could ever succeed. `true` means quarantine rather than retry.
    pub permanent: bool,
    /// The stable code an operator or log line records.
    pub code: &'static str,
}

impl StartupFailure {
    /// Returns whether a retry could ever succeed.
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        self.permanent
    }

    /// Returns whether the failure may be retried, given the tool's idempotency.
    ///
    /// **A permanent failure is never retryable regardless of posture**, asserted here rather than left
    /// to each caller: the class's posture is about duplication and does not know about permanence, so a
    /// caller that consulted only the posture would loop against an incompatible peer. This is the one
    /// place the two answers are combined, so they cannot be combined differently elsewhere.
    #[must_use]
    pub const fn is_retryable_for(
        &self,
        idempotency: jarvis_domain::tool::classification::Idempotency,
    ) -> bool {
        !self.permanent && self.class.retryable_for(idempotency)
    }
}

/// Classifies a failed MCP startup in JARVIS terms.
///
/// Every variant [`ClientInitializeError`] documents is mapped by name. The enum is `#[non_exhaustive]`,
/// so a variant added by the SDK lands in the documented `OutputInvalid` fallback — the conservative
/// choice for a type whose members describe a peer's answer being unusable.
#[must_use]
pub fn classify_startup_error(error: &ClientInitializeError) -> StartupFailure {
    match error {
        // **The compatibility failure the note names.** A retry cannot help, so it is permanent; and
        // nothing ran, so `Unavailable`'s `Safe` posture is honest rather than merely convenient.
        //
        // The peer's `supported_versions` are deliberately **not** embedded in the message: they are
        // untrusted strings, and this struct reaches logs. The variant's own `Display` already carries
        // them, so an operator who needs them reads the SDK error rather than a copy of it here.
        ClientInitializeError::NoCompatibleProtocolVersion { .. } => StartupFailure {
            class: ToolErrorClass::Unavailable,
            permanent: true,
            code: "mcp.startup_no_compatible_version",
        },
        // A JARVIS configuration defect rather than a peer failure: the caller asked for discovery
        // without offering a version. Permanent because retrying the same call repeats the same defect.
        ClientInitializeError::NoPreferredProtocolVersion => StartupFailure {
            class: ToolErrorClass::ProviderError,
            permanent: true,
            code: "mcp.startup_no_preferred_version",
        },
        // The caller cancelled. Not a peer failure, and not something a retry repairs.
        ClientInitializeError::Cancelled => StartupFailure {
            class: ToolErrorClass::Cancelled,
            permanent: false,
            code: "mcp.startup_cancelled",
        },
        // The peer answered with a JSON-RPC error, so it is classified by its code rather than by its
        // prose — the same rule `super::outcome` applies to call errors.
        ClientInitializeError::JsonRpcError(data) => StartupFailure {
            class: super::outcome::classify_error_code(data.code),
            // An `UnsupportedProtocolVersionError` arrives here rather than as the variant above when a
            // server answers a request rather than a probe, and it is equally permanent.
            permanent: data.code == rmcp::model::ErrorCode::UNSUPPORTED_PROTOCOL_VERSION,
            code: "mcp.startup_jsonrpc_error",
        },
        // The connection dropped during startup. `ProviderError` because the stream was lost after
        // something may have been delivered — the pessimistic choice, and the same one
        // `super::outcome` makes for `TransportClosed`.
        ClientInitializeError::ConnectionClosed(_) => StartupFailure {
            class: ToolErrorClass::ProviderError,
            permanent: false,
            code: "mcp.startup_connection_closed",
        },
        ClientInitializeError::TransportError { .. } => StartupFailure {
            class: ToolErrorClass::ProviderError,
            permanent: false,
            code: "mcp.startup_transport_error",
        },
        // Discovery failed *and* the legacy fallback failed. The two are reported together by the SDK,
        // and the classification is the **discover** failure's: it is the one that describes the peer's
        // current behaviour, while the fallback's failure merely says the peer is not legacy either.
        // Recursing means a nested incompatibility is still recognised as permanent.
        ClientInitializeError::LegacyFallbackFailed { discover, .. } => {
            let mut classified = classify_startup_error(discover);
            classified.code = "mcp.startup_legacy_fallback_failed";
            classified
        }
        // The peer's answer did not match what a client can use: an unexpected message, an unexpected
        // result, an id that does not correlate, or a variant added by a future SDK. `OutputInvalid`
        // is exactly "the result did not validate ... or exceeded its bounds".
        //
        // Notably this includes the **correlation** failures: an answer whose id does not match its
        // request is not a transport problem but a protocol one, and treating it as retryable would
        // paper over a peer that is answering the wrong question.
        _ => StartupFailure {
            class: ToolErrorClass::OutputInvalid,
            permanent: false,
            code: "mcp.startup_output_invalid",
        },
    }
}

/// Returns whether a startup failure means the server should be quarantined rather than retried.
///
/// **The one place the "could a retry ever work" question is answered**, so a caller cannot answer it by
/// a weaker rule — a class check, say, which does not know about permanence. See the module doc for why
/// this is separate from the retry posture.
#[must_use]
pub fn is_permanent_startup_failure(error: &ClientInitializeError) -> bool {
    classify_startup_error(error).is_permanent()
}

/// Returns whether a negotiated version still uses the `initialize` handshake.
///
/// A caller needs this because a session may legitimately negotiate an older revision whose lifecycle
/// differs: from `2026-07-28` onward there is no handshake and every request carries its protocol
/// version in `_meta`, while earlier revisions complete an `initialize` exchange. Behaviour written
/// against the target version must therefore ask rather than assume.
///
/// **Delegates to the SDK's `has_initialize` rather than comparing against [`TARGET_PROTOCOL_VERSION`].**
/// The two are not the same test: the comparison says "is this exactly the revision we targeted", while
/// this says "does this session have a handshake" — and a *newer* revision than the target would also
/// have no handshake. A comparison would report that newer session as handshake-bearing and the caller
/// would wait for an exchange that never comes.
#[must_use]
pub fn requires_initialize_handshake(negotiated: &ProtocolVersion) -> bool {
    negotiated.has_initialize()
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
