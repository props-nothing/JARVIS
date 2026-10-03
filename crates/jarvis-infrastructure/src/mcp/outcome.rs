//! MCP call outcomes and errors normalized to canonical JARVIS terms.
//!
//! The sibling of [`super`]: that module normalizes what a server *offers*, this one normalizes
//! what it *returns*. Both are pure, and both refuse rather than coerce.
//!
//! # Why this is not a detail
//!
//! Three canonical types already existed with **no production consumer** — [`ToolResultBody`],
//! [`ContentBlock`], and [`ToolErrorClass`]. They were reachable from tests alone, which is a gap the
//! workspace records in its own words: `tool_schema`'s doc says the whole validator "is reachable from
//! tests alone; that gap is named in `TLS-003`". This module is what turns three reviewed types into
//! three used ones, and the reason it matters is the retry decision: `ToolErrorClass` is where
//! "may a retry duplicate an effect" is answered, and it is `const fn retryable_for` on the class
//! rather than a rule each caller re-derives.
//!
//! # Two halves that are not symmetric
//!
//! A result and an error need different treatment, and the asymmetry is the interesting part.
//!
//! - **Errors are classified from a closed code set.** MCP's JSON-RPC codes are a fixed vocabulary,
//!   so the mapping is total and testable — with one deliberate exception, noted below.
//! - **Content is refused unless it is text.** MCP carries images, audio, embedded blobs, and
//!   resource links. JARVIS has no artifact store yet, and `ContentBlock::Artifact` requires an
//!   identifier JARVIS can resolve — so there is nothing honest to map a base64 image *to*. The
//!   alternatives are worse than refusing: inlining base64 into a text block pushes an unbounded,
//!   useless payload into a model's context, and fabricating an artifact id invents a reference that
//!   cannot be resolved. So non-text content is refused with a named reason, which is a limitation an
//!   operator can see rather than a silent widening.
//!
//! # The one mapping that is a judgement, and why it is stated
//!
//! A transport that **closes** mid-call and a transport that fails to **send** are not the same
//! event, and collapsing them would lose the thing that matters:
//!
//! - `TransportSend` means the request never left, so retrying cannot duplicate an effect.
//! - `TransportClosed` means it may have been delivered and executed, because `2026-07-28` removed
//!   SSE resumability — "a broken response stream loses the in-flight request; clients MUST re-issue
//!   it". So the outcome is **unsettled**, and the contract's rule for an unsettled outcome is
//!   reconciliation rather than a second write.
//!
//! Mapping both to one class would make an at-most-once call look safely repeatable.

use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::call::{ContentBlock, MAX_RESULT_BLOCKS, ToolResultBody};
use jarvis_domain::tool::error_class::ToolErrorClass;
use rmcp::ServiceError;
use rmcp::model::{
    CallToolResponse, CallToolResult, ContentBlock as McpContentBlock, ErrorCode, ResourceContents,
};

/// Why one MCP call outcome could not be normalized into JARVIS terms.
///
/// A single enum for both halves because a caller handles them in one place — it either accepted an
/// outcome or it did not — and the variants are still distinct so the reason is reportable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpOutcomeRejection {
    /// The result carried no block this adapter can represent.
    #[error("mcp result contained no mappable content")]
    EmptyContent,
    /// The result carried more blocks than [`MAX_RESULT_BLOCKS`].
    #[error("mcp result carried more than {max} content blocks")]
    TooManyBlocks {
        /// The bound that was exceeded.
        max: usize,
    },
    /// The result carried a content kind this adapter deliberately does not map.
    #[error("mcp content kind `{kind}` is not mapped")]
    UnsupportedContent {
        /// The MCP content kind, named so the limitation is visible.
        kind: &'static str,
    },
    /// The assembled result was refused by the domain's own rules.
    ///
    /// Kept as a field rather than collapsed into a generic failure, so a defect in *this* module is
    /// distinguishable from a server sending something unusable.
    #[error("tool result for field `{field}` was refused")]
    ResultRefused {
        /// The field the domain named.
        field: &'static str,
    },
    /// The response was not a completed result.
    ///
    /// `tools/call` can answer with more than a result: `2026-07-28` added the multi round-trip
    /// (`input_required`) response, and the Tasks extension adds `task`. Neither is a *result*, and a
    /// caller that read one as a completed call would record a success for a call that has not run
    /// yet. Naming the kind keeps that from being a silent default.
    #[error("mcp response kind `{kind}` is not a completed result")]
    NotACompletedResult {
        /// The response kind, named so the gap is visible rather than inferred.
        kind: &'static str,
    },
}

impl McpOutcomeRejection {
    /// Returns the stable code an operator or log line records.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyContent => "mcp.result_empty",
            Self::TooManyBlocks { .. } => "mcp.result_too_many_blocks",
            Self::UnsupportedContent { .. } => "mcp.content_unsupported",
            Self::ResultRefused { .. } => "mcp.result_refused",
            Self::NotACompletedResult { .. } => "mcp.response_not_complete",
        }
    }
}

/// What a call did, in canonical terms: it either produced a body or failed with a class.
///
/// **A reported failure and a returned body are mutually exclusive**, which is why this is one enum
/// rather than a body plus an optional error: a caller that received both would have to decide which
/// to believe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCallOutcome {
    /// The call returned a bounded, classified result.
    Succeeded(ToolResultBody),
    /// The call failed, with the class that decides its retry posture.
    Failed(ToolErrorClass),
}

/// Normalizes a `tools/call` **response**, dispatching on its kind.
///
/// [`normalize_call_result`] handles the completed case; this is the entry point a caller should use,
/// because it is the one that cannot read a non-result as a result. `CallToolResponse` has three
/// variants and is `#[non_exhaustive]`:
///
/// - `Complete` → normalized as a result.
/// - `InputRequired` → **refused here, deliberately.** This is the multi round-trip pattern: the
///   server is asking for more input, and the client must retry the original request with
///   `inputResponses` while echoing an opaque `requestState`. Driving that loop is control flow
///   rather than normalization — it needs JARVIS policy and approval routing for the requested input,
///   and a bounded round count — so it belongs with the call path. What belongs *here* is the
///   refusal, because "MRTR is not implemented yet" must not look like "the call succeeded".
/// - `Task` → refused for the same reason one level down: the server returned a task handle and the
///   client polls `tasks/get`. There is no result to normalize until the task completes.
///
/// # Errors
///
/// Returns [`McpOutcomeRejection::NotACompletedResult`] naming the kind for either non-result, and
/// otherwise whatever [`normalize_call_result`] returns.
pub fn normalize_call_response(
    response: &CallToolResponse,
) -> Result<McpCallOutcome, McpOutcomeRejection> {
    match response {
        CallToolResponse::Complete(result) => normalize_call_result(result),
        CallToolResponse::InputRequired(_) => Err(McpOutcomeRejection::NotACompletedResult {
            kind: "input_required",
        }),
        CallToolResponse::Task(_) => Err(McpOutcomeRejection::NotACompletedResult { kind: "task" }),
        // `#[non_exhaustive]`: a kind added by the SDK is refused rather than assumed complete,
        // which is the fail-safe direction for a type whose other variants are not results.
        _ => Err(McpOutcomeRejection::NotACompletedResult { kind: "unknown" }),
    }
}

/// Normalizes a `tools/call` result.
///
/// An MCP tool that fails *by reporting* (`isError: true`) is a `Failed` outcome rather than a
/// rejected one: the tool ran and said it did not work, which is information the caller needs.
///
/// # Errors
///
/// Returns a [`McpOutcomeRejection`] when the result has no mappable content, exceeds the block
/// bound, carries an unmapped content kind, or is refused by the domain's cross-field rules.
pub fn normalize_call_result(
    result: &CallToolResult,
) -> Result<McpCallOutcome, McpOutcomeRejection> {
    if result.is_error == Some(true) {
        // Classified as `ProviderError` and **not** inferred from the message. MCP does not
        // standardise tool-level error codes — `isError` carries no code at all — so reading
        // "not found" out of the text would be parsing untrusted prose into a policy input. The
        // class therefore says only what is knowable: the provider reported a failure, and its
        // `OnlyIfIdempotent` posture keeps a write from being repeated automatically.
        return Ok(McpCallOutcome::Failed(ToolErrorClass::ProviderError));
    }
    Ok(McpCallOutcome::Succeeded(body_for(result)?))
}

/// Builds a bounded, classified result body from a result's content blocks.
///
/// # Errors
///
/// As [`normalize_call_result`]: an unmapped content kind is refused rather than coerced, and a
/// result with no mappable block is [`McpOutcomeRejection::EmptyContent`] rather than an empty body,
/// because the domain refuses a body with no content — and a "successful" result that carries nothing
/// is a server defect worth surfacing, not a valid empty answer.
fn body_for(result: &CallToolResult) -> Result<ToolResultBody, McpOutcomeRejection> {
    if result.content.len() > MAX_RESULT_BLOCKS {
        return Err(McpOutcomeRejection::TooManyBlocks {
            max: MAX_RESULT_BLOCKS,
        });
    }
    let mut blocks = Vec::with_capacity(result.content.len());
    // `structuredContent` is mapped first when present. It is the field a `2026-07-28` server is
    // directed to use for a machine-readable result ("it can be any JSON value, including `null`"),
    // so folding it in as a JSON block keeps it from being lost when the text blocks are summaries.
    if let Some(structured) = &result.structured_content {
        blocks.push(content_block_for_json(structured)?);
    }
    for block in &result.content {
        blocks.push(content_block_for(block)?);
    }
    if blocks.is_empty() {
        return Err(McpOutcomeRejection::EmptyContent);
    }
    ToolResultBody::new(blocks, None, Sensitivity::Internal).map_err(|error| match error {
        jarvis_domain::error::DomainError::ToolDefinitionInvalid { field } => {
            McpOutcomeRejection::ResultRefused { field }
        }
        _ => McpOutcomeRejection::ResultRefused { field: "result" },
    })
}

/// Maps one MCP content block, or refuses it by name.
///
/// # Errors
///
/// Returns [`McpOutcomeRejection::UnsupportedContent`] for every kind this adapter does not map.
fn content_block_for(block: &McpContentBlock) -> Result<ContentBlock, McpOutcomeRejection> {
    match block {
        McpContentBlock::Text(text) => map_text(&text.text),
        // A text resource is its own text, so it maps; a **blob** resource is base64 and does not,
        // for the same reason an image does not.
        McpContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents { text, .. } => map_text(text),
            ResourceContents::BlobResourceContents { .. } => {
                Err(McpOutcomeRejection::UnsupportedContent {
                    kind: "blob_resource",
                })
            }
            // `ResourceContents` is `#[non_exhaustive]`, so a kind added by the SDK is refused
            // rather than falling into the text arm and being read as text it may not be.
            _ => Err(McpOutcomeRejection::UnsupportedContent {
                kind: "unknown_resource",
            }),
        },
        // A resource *link* is a URI rather than content. Mapping it to a text block would present a
        // reference as if it were the thing referenced, which is the conflation the artifact
        // indirection exists to prevent — so it is refused until JARVIS can resolve the URI itself.
        McpContentBlock::ResourceLink(_) => Err(McpOutcomeRejection::UnsupportedContent {
            kind: "resource_link",
        }),
        McpContentBlock::Image(_) => Err(McpOutcomeRejection::UnsupportedContent { kind: "image" }),
        McpContentBlock::Audio(_) => Err(McpOutcomeRejection::UnsupportedContent { kind: "audio" }),
        // `ContentBlock` is `#[non_exhaustive]`, so a kind added by the SDK must not silently become
        // a text or an empty block. Named `unknown` because the adapter cannot name it either.
        _ => Err(McpOutcomeRejection::UnsupportedContent { kind: "unknown" }),
    }
}

/// Maps a JSON value to a JSON content block through its serialized text.
///
/// # Errors
///
/// Returns [`McpOutcomeRejection::ResultRefused`] when the domain refuses the serialized document.
fn content_block_for_json(value: &serde_json::Value) -> Result<ContentBlock, McpOutcomeRejection> {
    let text = value.to_string();
    ContentBlock::json(&text).map_err(|_| McpOutcomeRejection::ResultRefused { field: "content" })
}

/// Maps text to a text content block.
///
/// # Errors
///
/// Returns [`McpOutcomeRejection::ResultRefused`] when the domain refuses the text, which is how an
/// over-bound or NUL-bearing payload is reported rather than trimmed.
fn map_text(text: &str) -> Result<ContentBlock, McpOutcomeRejection> {
    ContentBlock::text(text).map_err(|_| McpOutcomeRejection::ResultRefused { field: "content" })
}

/// Classifies any service-level failure, including the ones that never reached the peer.
///
/// # The `TransportSend` / `TransportClosed` asymmetry
///
/// See the module doc. A send failure is [`ToolErrorClass::Unavailable`] because nothing was
/// delivered; a closed transport is [`ToolErrorClass::ProviderError`] because the request may have
/// been received and executed, and its posture is `OnlyIfIdempotent` while its
/// `is_unsettled` is `true` — which is what routes the call to reconciliation instead of a repeat.
#[must_use]
pub fn classify_service_error(error: &ServiceError) -> ToolErrorClass {
    match error {
        ServiceError::McpError(inner) => classify_error_code(inner.code),
        // Never delivered: retrying cannot duplicate an effect.
        ServiceError::TransportSend(_) => ToolErrorClass::Unavailable,
        ServiceError::Timeout { .. } => ToolErrorClass::Timeout,
        ServiceError::Cancelled { .. } => ToolErrorClass::Cancelled,
        // A response shape this client cannot use is a result problem, not a transport one.
        ServiceError::UnexpectedResponse => ToolErrorClass::OutputInvalid,
        // Both are bounds: a subscription buffer and the MRTR round cap.
        ServiceError::SubscriptionLagged { .. }
        | ServiceError::InputRequiredRoundsExceeded { .. } => ToolErrorClass::LimitExceeded,
        // **A closed transport lands here, and so does an unclassified `#[non_exhaustive]` variant.**
        // A lost stream may have been received and executed, so the class is the *pessimistic*
        // `ProviderError`: its posture may permit a retry only for a proven-idempotent tool, and it is
        // *unsettled*, where `Unavailable` would assert that nothing was delivered — an assertion this
        // code cannot make about a variant it does not recognise, or about a stream it lost after
        // sending. A named `TransportClosed` arm beside this one would only restate the comment.
        _ => ToolErrorClass::ProviderError,
    }
}

/// Classifies a JSON-RPC error code from a peer.
///
/// **Every code the specification defines is mapped, and the mapping is written out rather than
/// defaulted**, so a code added by a future revision lands in the documented `ProviderError` branch
/// knowingly instead of inheriting a meaning.
///
/// "Every code the specification defines" is **nine values, not every integer in the reserved range**:
/// JSON-RPC 2.0's five general codes (`-32700`, `-32600`..`-32603`), this revision's three
/// (`-32020`, `-32021`, `-32022`), and the legacy `-32002` a `2025-11-25` peer may still send. The
/// surrounding range is deliberately *not* a vocabulary to enumerate: `-32096`..`-32099` is reserved
/// for **implementations** and this specification says nothing about it, and implementations **MUST
/// NOT** emit a code from `-32020`..`-32095` that the specification does not define — so for either
/// sub-range the fallback is the only honest answer and exhaustiveness is not a property this table
/// can have. (The first spelling of this doc said "every code the specification defines", which was
/// true, and read as "every code", which is not.)
#[must_use]
pub fn classify_error_code(code: ErrorCode) -> ToolErrorClass {
    match code {
        // The peer cannot speak our version. Mapped to `Unavailable` — "the tool exists but cannot be
        // reached" is the closest available class, and its `Safe` posture asserts the one thing that
        // is certainly true: a request refused at the door cannot have duplicated an effect.
        //
        // **The retry half of that sentence is not decided here.** An incompatible server is
        // quarantined by the health layer rather than retried, because its posture is about effect
        // duplication and not about whether a retry could ever succeed. A dedicated class for
        // permanent incompatibility is a contract change (a new `tool.*` code) and is proposed
        // separately rather than forced into an existing one.
        ErrorCode::UNSUPPORTED_PROTOCOL_VERSION => ToolErrorClass::Unavailable,
        // The tool, resource, or method is not there.
        //
        // **This arm catches the *legacy* number, and that is required rather than tidy.** `2026-07-28`
        // renumbered resource-not-found from `-32002` to `-32602` "to align with JSON-RPC" (SEP-2164)
        // while keeping the old code reserved, and says clients "SHOULD still accept `-32002` ... from
        // servers implementing earlier versions". Discovery accepts a peer one revision behind by
        // design, so a not-found from such a peer arrives as `-32002` — the number the pinned SDK's own
        // `ErrorCode::RESOURCE_NOT_FOUND` still holds, because a constant is what a client uses to
        // *recognize* the legacy code. Dropping this arm would classify a legacy peer's not-found as a
        // provider fault. The *modern* number is handled in the next arm, where its second meaning is
        // discussed.
        //
        // `METHOD_NOT_FOUND` shares this arm because on this revision a missing *tool* is answered
        // that way (the tool-name path of a `tools/call`), while `RESOURCE_NOT_FOUND` covers
        // resources — both are "the thing you named is not there".
        ErrorCode::RESOURCE_NOT_FOUND | ErrorCode::METHOD_NOT_FOUND => ToolErrorClass::NotFound,
        // The request or its arguments were rejected at the door.
        //
        // **`-32602` carries two meanings on this revision and both land here.** Because SEP-2164
        // moved resource-not-found onto `-32602`, the same number now signals either an unsatisfiable
        // argument document or a missing resource, and only the *message* distinguishes them.
        // `NotFound` is the wrong arm for the argument case — a caller shown `tool.not_found` for a
        // tool that is listed and callable is sent to look for something absent instead of to fix its
        // input — and a *reserved*-resource class does not exist in the fabric yet, so the argument
        // meaning wins and the ambiguity is stated rather than hidden.
        //
        // **A JARVIS-side cause lands here too, and that is the honest reading rather than a
        // misclassification.** A request missing a required per-request `_meta` field
        // (`io.modelcontextprotocol/protocolVersion` or `.../clientCapabilities`) is answered
        // `-32602` with HTTP `400` — the server saying JARVIS sent a malformed request, which *is* a
        // schema problem with the request. The SDK seeds those fields itself (`ClientRequestMetadata`,
        // set from `server/discover` or `initialize`), so this arm is not reached for the transports
        // this adapter uses; naming it keeps the class from looking like a claim that every `-32602`
        // came from the server's view of the arguments.
        ErrorCode::INVALID_PARAMS | ErrorCode::INVALID_REQUEST | ErrorCode::PARSE_ERROR => {
            ToolErrorClass::SchemaInvalid
        }
        // The peer refused the request **at the door**, before doing anything with it.
        //
        // **Three codes, one meaning, and grouping them is the correction rather than a tidy-up.**
        // `-32020` and `-32021` were falling into the catch-all below and therefore classified
        // `ProviderError` — retryable for an idempotent tool and *unsettled*, so a call that provably
        // never ran entered reconciliation and could be repeated against a server that had refused it
        // outright. Both are the peer stating a fact about the **request** rather than reporting a
        // failure it incurred: a required header was missing or wrong, or processing needs a client
        // capability JARVIS did not declare. Nothing was dispatched, so nothing can have been
        // duplicated.
        //
        // **⚠ "Declared" is about the client's `_meta` capabilities, and `-32021` is the code an
        // operator will actually see for a task handle.** `JarvisClient` declares no server-initiated
        // capabilities, so a server that tries to answer a `tools/call` with a task handle is refused by
        // the SDK's *server* half with exactly this code — verified against a real child in
        // `mcp_executor_process`'s
        // `a_task_handle_answer_is_refused_at_the_negotiated_capability_not_by_jarvis`. That matters
        // because `call.rs` has its own considered arm for a task handle, with its own code
        // (`mcp.response_not_complete`, classed `OutputInvalid`), and a reader would reasonably predict
        // *that* is what a caller meets. It is not: this arm is. Worth recording rather than deriving,
        // because the two classes differ in retry posture (`Unavailable` is `Safe`, `OutputInvalid` is
        // `Never`) and the guess is the wrong one.
        //
        // `Unavailable` is the class the sibling `UNSUPPORTED_PROTOCOL_VERSION` already uses for the
        // same reason, and it is the one whose `Safe` posture asserts the thing that is certainly true
        // here: a request refused at the door took no effect. Grouping the three also means the
        // note's own error table's "Retry? **No**" column has one implementation instead of a
        // per-code decision three call sites could get differently.
        //
        // Two of the three are defects in *this* adapter rather than at the peer — a header mismatch
        // and a missing declared capability both mean JARVIS sent something it should not have — and
        // `Safe` does **not** say "retry will help". It says a retry cannot duplicate an effect, and
        // whether one can succeed at all is the health layer's separate, permanent-failure question
        // (the same division of labour the incompatible-version arm documents above).
        ErrorCode::HEADER_MISMATCH | ErrorCode::MISSING_REQUIRED_CLIENT_CAPABILITY => {
            ToolErrorClass::Unavailable
        }
        // The peer reported a failure of its own, or sent a code added by a future revision.
        //
        // **Deliberately one arm.** Every code the specification defines is either named above or
        // belongs here, and an unclassified code joins rather than gaining an arm with the same body:
        // `match_same_arms` rejects that, and two spellings of one rule are what drift apart later.
        // `ProviderError` is also the *pessimistic* choice — its posture may permit a retry only for a
        // proven-idempotent tool, and it is `is_unsettled`, so a call that might have taken effect is
        // reconciled rather than repeated. The alternative, `Unavailable`, asserts that nothing was
        // delivered, which this code cannot know about a failure the peer reported after receiving the
        // request.
        //
        // `INTERNAL_ERROR` reaches here and is the clearest case for the arm: the peer accepted the
        // request and then failed, so the request may have been executed.
        _ => ToolErrorClass::ProviderError,
    }
}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod tests;
