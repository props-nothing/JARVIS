//! The Model Context Protocol adapter: MCP tool metadata normalized to canonical tools.
//!
//! This is the implementation half of `TLS-008`, built against
//! `docs/research/integrations/mcp.md`, whose implementation gate is passed. The note fixes two
//! facts that shape this module: MCP server metadata is **untrusted**, and MCP tool annotations are
//! **hints** that "are not guaranteed to provide a faithful description of tool behavior".
//!
//! # This module does no I/O
//!
//! It is the *normalization* layer only, and it is pure. The socket belongs to a sibling transport
//! module for the reason the OpenAI-compatible adapter already records: every provider-shaped trap
//! lives in the mapping, and code that can only be exercised through a live connection is the least
//! tested code in an adapter. Here the traps are a hostile tool name, a schema this workspace cannot
//! validate, and annotations that contradict each other or that lie.
//!
//! # What "untrusted" means concretely
//!
//! Three rules, each of which a server's metadata would otherwise be able to break:
//!
//! 1. **A server's version and a tool's version are not identity.** The canonical
//!    [`ToolSource`] is `SourceKind::McpServer` with the *configured* server name as owner and a
//!    JARVIS-owned contract version. A server that relabels itself with a new version cannot make
//!    its tools read as new implementations, and one that keeps a version cannot make a changed
//!    tool read as the old one. The schema fingerprint carries the change instead — which is what
//!    the domain's `ToolIdentity` was built to do.
//! 2. **An annotation can propose, never permit.** A `readOnlyHint` is the one annotation that
//!    *reduces* the declared danger, so trusting it would be the fail-open direction. It is
//!    therefore accepted as a claim and neutralized at the point that matters: nothing here ever
//!    produces [`ApprovalHint::Allow`], so no server can annotate its way past an approval prompt.
//!    A lying hint buys a `ReadOnly` label and still gets `Ask`.
//! 3. **Contradiction is refused, not trimmed.** A tool claiming `readOnlyHint` *and*
//!    `openWorldHint` is contradictory in this model, because `Effect::ReadOnly` may not be
//!    combined with another effect. The tool is refused and the conflict named, following the
//!    `tool_schema` precedent — the annotation is discarded instead of the schema, so a refused
//!    tool is visible to an operator where a dropped effect would only be visible to an auditor
//!    who already suspected something.
//!
//! # Why the schema goes through `tool_schema`
//!
//! An MCP `inputSchema` is JSON Schema, and this workspace implements a *supported subset* of
//! 2020-12 that refuses unknown keywords rather than treating them as annotations. A server schema
//! using `pattern` is therefore refused here, and that is the correct outcome rather than an
//! unfortunate one: accepting it would validate **less** than the server asked for while reporting
//! success, admitting calls the server meant to reject. The fingerprint that becomes part of the
//! tool's identity is taken from that same validated text, so a definition's fingerprint describes
//! the document calls are actually checked against.

use jarvis_domain::error::DomainError;
use jarvis_domain::tool::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk,
};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::identity::{
    SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use rmcp::model::{Tool, ToolAnnotations};

use crate::tool_schema::ToolSchema;

pub mod call;
pub mod client;
pub mod composition;
pub mod discovery;
pub mod executor;
pub mod invocation;
pub mod outcome;
pub mod process;
pub mod registration;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// The major version of the tool contract JARVIS exposes for MCP tools.
///
/// JARVIS-owned and deliberately **not** read from the server. The domain requires a capability's
/// major to equal its source version's major, and both are identity: bumping this is a deliberate
/// statement that the tools' effects or semantics changed incompatibly, which is a review action
/// rather than something a remote server gets to announce.
pub const INITIAL_CONTRACT_MAJOR: u32 = 1;

/// The capability namespace every MCP tool is published under.
///
/// **A fixed constant rather than the server name, and that is a correction rather than a
/// preference.** A canonical capability is `namespace.name@major`, and its namespace is a single
/// *segment* — lowercase letters, digits, and underscores, starting with a letter. A configured
/// server name is validated by a looser rule that permits hyphens and dots, because real servers
/// are named `brave-search` and `google.gmail`. Using the server name as the namespace therefore
/// refused every legitimately-named server, and "fixing" that by replacing `-` with `_` would be
/// exactly the silent transformation this adapter refuses everywhere else.
///
/// The server is not lost by fixing the namespace: it is the [`ToolSource::owner`], which is part
/// of [`ToolIdentity`], so two servers offering the same tool name produce the same capability but
/// **different identities** — and `ToolIdentity::authorizes` requires all three components to
/// match. Grants are recorded against identity, so a grant for one server's `read_file` does not
/// cover another's.
pub const CONTRACT_NAMESPACE: &str = "mcp";

/// The longest purpose text accepted from a server, in bytes.
///
/// A server's description reaches an approval prompt and a log line, so it is bounded where it is
/// *accepted* rather than where it is displayed. Bounded text that arrives unbounded is the same
/// hazard whether it is trusted or not.
pub const MAX_MCP_PURPOSE_BYTES: usize = 4_096;

/// The greatest number of tools accepted from one server in a single listing.
///
/// The contract requires a tool-count limit, and it belongs at the acceptance boundary as well as
/// on the wire: a server controls this number, and a listing is re-read on every discovery refresh.
/// Tools past the bound are refused individually so the operator sees how many were dropped.
pub const MAX_MCP_TOOLS: usize = 256;

/// The source version JARVIS records for an MCP server's tools.
///
/// See [`INITIAL_CONTRACT_MAJOR`]: the value must share that major, and it does not come from the
/// server. A changed schema does not need a new version because the fingerprint already makes it a
/// different identity; only a changed *contract* does.
pub const INITIAL_CONTRACT_VERSION: &str = "1.0.0";

/// The timeout applied to an MCP tool call when nothing more specific is known.
///
/// MCP removed SSE resumability and sessions, so a lost response stream loses the request and the
/// only recovery is a re-issue. Thirty seconds is a bound that keeps a hung server from holding a
/// run open indefinitely while still allowing a slow local tool to finish.
pub const DEFAULT_MCP_TIMEOUT_MS: u64 = 30_000;

/// The attempt count applied to an MCP tool call.
///
/// **One, deliberately.** An MCP call that is not known to be idempotent must not be retried
/// automatically: the transport has no redelivery, so a retry after a lost response is a second
/// side effect rather than a recovery of the first. Retrying is a policy decision made against a
/// recorded idempotency key, not a default an adapter picks.
pub const DEFAULT_MCP_MAX_ATTEMPTS: u32 = 1;

/// Why one MCP tool could not be normalized into a canonical tool.
///
/// A typed reason per tool rather than one error for the catalog, because a server offering forty
/// tools with one unusable schema should contribute thirty-nine, and an operator needs to know which
/// one was dropped and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpToolRejection {
    /// The configured server name is not a usable tool source owner.
    #[error("mcp server name is not a usable tool source owner")]
    ServerNameInvalid,
    /// The tool's name is not usable as a canonical capability name segment.
    #[error("mcp tool name is not a usable canonical capability name")]
    ToolNameInvalid,
    /// The tool's description is longer than [`MAX_MCP_PURPOSE_BYTES`].
    #[error("mcp tool description is too long")]
    DescriptionTooLong,
    /// The server listed more tools than [`MAX_MCP_TOOLS`].
    #[error("mcp server listed more tools than JARVIS accepts")]
    TooManyTools,
    /// The tool declares an `inputSchema` this workspace cannot validate.
    #[error("mcp tool input schema was refused")]
    SchemaRejected,
    /// The annotations contradict each other under this model's rules.
    #[error("mcp tool annotations contradict each other")]
    AnnotationsConflict,
    /// The definition was refused by the domain's cross-field rules.
    ///
    /// A defect here means this module constructed a candidate it should have refused first, so the
    /// field is kept to make that visible rather than collapsing it into a generic failure.
    #[error("tool definition for field `{field}` was refused")]
    DefinitionInvalid {
        /// The field the domain named.
        field: &'static str,
    },
}

impl McpToolRejection {
    /// Returns the stable code an operator or log line records.
    ///
    /// Prefixed `mcp.` so a rejection is attributable to this adapter rather than to the tool
    /// fabric, which has its own `tool.*` codes.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ServerNameInvalid => "mcp.server_name_invalid",
            Self::ToolNameInvalid => "mcp.tool_name_invalid",
            Self::DescriptionTooLong => "mcp.description_too_long",
            Self::TooManyTools => "mcp.too_many_tools",
            Self::SchemaRejected => "mcp.schema_rejected",
            Self::AnnotationsConflict => "mcp.annotations_conflict",
            Self::DefinitionInvalid { .. } => "mcp.definition_invalid",
        }
    }
}

/// Details of a tool that was offered and refused, so the refusal is reportable rather than silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedTool {
    /// The name the server used.
    pub name: String,
    /// Why it was refused.
    pub rejection: McpToolRejection,
}

/// One offered tool that never became callable, with the stable reason code.
///
/// **A canonical form of the two ways a tool can be dropped**, and it exists because both were previously
/// expressible only inside one layer. A tool is refused either by the **normalizer** (its name or schema was
/// unusable — see [`RejectedTool`]) or by **registration** (its identity changed, a source was claimed — see
/// [`registration::RefusedRegistration`]). The normalizer's half was carried on the catalog and then dropped
/// by the composition, so a malformed tool was invisible for any server that also offered a usable one: the
/// server composed, `is_clean()` was true, and no log line mentioned the omission.
///
/// A `code` rather than the enum, because a caller holding this value reports it — the code is what an
/// operator greps for in the evidence note's error tables, and both source enums already define theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolRefusal {
    /// The tool as the server named it.
    pub name: String,
    /// The stable `mcp.*` code for why it was refused.
    pub code: &'static str,
}

/// Builds the canonical refusal for a tool the **normalizer** refused.
#[must_use]
pub fn refusal_for_rejected(tool: &RejectedTool) -> McpToolRefusal {
    McpToolRefusal {
        name: tool.name.clone(),
        code: tool.rejection.code(),
    }
}

/// The canonical tools one MCP server offered, and the ones it offered that were refused.
///
/// Both halves are returned rather than only the accepted tools, because "the server has three
/// tools" and "the server has three tools and one was dropped" are different operator facts, and a
/// caller holding only the accepted list cannot tell them apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizedCatalog {
    /// The server identity every accepted tool carries.
    pub source: Option<ToolSource>,
    /// The accepted definitions, ordered by capability so the answer is stable across calls.
    pub tools: Vec<ToolDefinition>,
    /// The accepted tools' **input schema documents**, aligned index-for-index with [`Self::tools`].
    ///
    /// **The schema text is carried, not discarded, and that is a correction rather than a
    /// convenience.** [`normalize_tool`] parses the schema to fingerprint it — and an earlier version
    /// then dropped the text, which left a discovered MCP tool with no schema at all. Downstream a
    /// `ResolvedTool` with `input_schema: None` is refused by the argument validator as
    /// `tool.schema_absent`, so a tool this adapter had just accepted would register and then be
    /// **uncallable** — a defect that would not appear until the call path resolved it, which is the
    /// "a value nothing consults" shape this project keeps closing one layer later.
    ///
    /// The text is the **validated** document, which is also what `ToolSchema::confirms` requires: the
    /// fingerprint in each definition is computed from exactly these bytes, so a caller that stored a
    /// different document under that identity would fail its own consistency check.
    pub schemas: Vec<String>,
    /// The refused tools, in the order the server listed them.
    pub rejected: Vec<RejectedTool>,
}

/// Builds the canonical source identity for one configured MCP server.
///
/// # Errors
///
/// Returns [`McpToolRejection::ServerNameInvalid`] when the configured name is not a usable owner.
/// The name reaches persisted approval records and logs, so it is validated where it is accepted
/// rather than where it is displayed.
pub fn server_source(server: &str) -> Result<ToolSource, McpToolRejection> {
    let version = ToolVersion::parse(INITIAL_CONTRACT_VERSION).map_err(|_| {
        McpToolRejection::DefinitionInvalid {
            field: "source.version",
        }
    })?;
    ToolSource::new(SourceKind::McpServer, server, version)
        .map_err(|_| McpToolRejection::ServerNameInvalid)
}

/// Normalizes every tool a server listed, keeping the refusals alongside the accepted tools.
///
/// The server identity is resolved once and then shared, so a bad server name fails the whole
/// catalog (there is nothing to be a source for) while a bad *tool* fails only that tool.
#[must_use]
pub fn normalize_catalog(server: &str, tools: &[Tool]) -> NormalizedCatalog {
    let Ok(source) = server_source(server) else {
        return NormalizedCatalog {
            source: None,
            tools: Vec::new(),
            schemas: Vec::new(),
            rejected: tools
                .iter()
                .map(|tool| RejectedTool {
                    name: tool.name.to_string(),
                    rejection: McpToolRejection::ServerNameInvalid,
                })
                .collect(),
        };
    };
    let mut accepted = Vec::with_capacity(tools.len().min(MAX_MCP_TOOLS));
    let mut rejected = Vec::new();
    for (index, tool) in tools.iter().enumerate() {
        if index >= MAX_MCP_TOOLS {
            rejected.push(RejectedTool {
                name: tool.name.to_string(),
                rejection: McpToolRejection::TooManyTools,
            });
            continue;
        }
        match normalize_tool(&source, tool) {
            Ok(pair) => accepted.push(pair),
            Err(rejection) => rejected.push(RejectedTool {
                name: tool.name.to_string(),
                rejection,
            }),
        }
    }
    // Sorted as **pairs**, then split, so the definitions and their schemas cannot drift out of
    // alignment. Sorting two parallel vectors separately would leave the invariant held by convention,
    // which is the class of ordering mistake this repository has recorded more than once.
    accepted.sort_by(|left, right| left.0.identity.cmp(&right.0.identity));
    let (tools, schemas) = accepted.into_iter().unzip();
    NormalizedCatalog {
        source: Some(source),
        tools,
        schemas,
        rejected,
    }
}

/// Normalizes one MCP tool into a canonical definition **and the validated schema it came from**.
///
/// Returns the pair rather than the definition alone, because the schema text is needed downstream and
/// recovering it from the parsed `serde_json::Map` would produce a *different* string than the one that
/// was fingerprinted — see [`NormalizedCatalog::schemas`].
///
/// # Errors
///
/// Returns a [`McpToolRejection`] when the name is unusable, the schema is one this workspace does not
/// implement, the annotations contradict each other, or the assembled definition fails the domain's
/// cross-field rules.
pub fn normalize_tool(
    source: &ToolSource,
    tool: &Tool,
) -> Result<(ToolDefinition, String), McpToolRejection> {
    let capability = capability_for(tool.name.as_ref())?;
    let purpose = purpose_for(tool)?;
    let schema_text = serde_json::Value::Object((*tool.input_schema).clone()).to_string();
    let schema = ToolSchema::parse(&schema_text).map_err(|_| McpToolRejection::SchemaRejected)?;
    // Taken from the **parsed** value rather than from `schema_text`, so the document returned is the
    // one the fingerprint was computed over. They are the same bytes today; taking it here is what keeps
    // them the same if the parser ever normalizes, which is the whole reason `ToolSchema::text` exists.
    let validated_schema = schema.text().to_owned();
    let effects = effects_for(tool.annotations.as_ref())?;
    let risk = risk_of(&effects);
    let identity = ToolIdentity {
        capability,
        source: source.clone(),
        schema_fingerprint: *schema.fingerprint(),
    };
    let definition = ToolDefinition::new(
        identity,
        tool.name.as_ref(),
        purpose,
        effects.clone(),
        risk,
        // **No scopes.** An MCP server cannot vouch for a JARVIS scope; scopes come from grants and
        // from reviewed configuration, never from the thing being governed.
        Vec::new(),
        // **Never `Allow`.** This is the line that makes a lying `readOnlyHint` harmless: the worst
        // it can do is label a tool read-only while the tool still waits for an explicit decision.
        ApprovalHint::Ask,
        idempotency_of(
            tool.annotations.as_ref(),
            effects.as_slice() == [Effect::ReadOnly],
        ),
        DataClasses::new(
            jarvis_domain::model::policy::Sensitivity::Internal,
            jarvis_domain::model::policy::Sensitivity::Internal,
        )
        .map_err(|_| McpToolRejection::DefinitionInvalid {
            field: "data_classes",
        })?,
        ExecutionDefaults::new(DEFAULT_MCP_TIMEOUT_MS, DEFAULT_MCP_MAX_ATTEMPTS)
            .map_err(|_| McpToolRejection::DefinitionInvalid { field: "execution" })?,
    )
    .map_err(|error| match error {
        DomainError::ToolIdentifierNotCanonical => McpToolRejection::ToolNameInvalid,
        DomainError::ToolDefinitionInvalid { field } => {
            McpToolRejection::DefinitionInvalid { field }
        }
        _ => McpToolRejection::DefinitionInvalid {
            field: "definition",
        },
    })?;
    Ok((definition, validated_schema))
}

/// Builds the canonical capability for a tool name, refusing rather than transforming.
///
/// A name is refused when it is not a usable segment, and it is **not** repaired by trimming,
/// lowercasing, or replacing punctuation. The canonical form exists so two spellings never denote
/// two identities; a repair that mapped two distinct offered names onto one capability would defeat
/// it, and the name reaches an approval prompt.
///
/// There is deliberately **no separate display-name check**: the segment rule is strictly narrower
/// than the display-name rule (64 bytes of lowercase letters, digits, and underscores against 128
/// bytes of anything control-character-free), so every name that reaches here is already a usable
/// display name. A second check would be unreachable code that reads like a guarantee.
fn capability_for(name: &str) -> Result<ToolCapability, McpToolRejection> {
    ToolCapability::new(CONTRACT_NAMESPACE, name, INITIAL_CONTRACT_MAJOR)
        .map_err(|_| McpToolRejection::ToolNameInvalid)
}

/// Returns the purpose text for a tool, from its description.
///
/// Falls back to the name rather than to an empty string, because the domain refuses a blank purpose
/// and a tool with no description would otherwise be dropped for a reason its author cannot act on.
/// The description is server-controlled text that reaches a prompt, so it is *displayed* and never
/// parsed as instruction, and it is bounded where it is accepted.
///
/// # Errors
///
/// Returns [`McpToolRejection::DescriptionTooLong`] when the description exceeds
/// [`MAX_MCP_PURPOSE_BYTES`]. Refused rather than truncated: a truncated purpose is a description
/// nobody wrote, and the bound is a limit an operator can see being hit.
fn purpose_for(tool: &Tool) -> Result<&str, McpToolRejection> {
    let description = tool
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty());
    match description {
        Some(description) if description.len() <= MAX_MCP_PURPOSE_BYTES => Ok(description),
        Some(_) => Err(McpToolRejection::DescriptionTooLong),
        None => Ok(tool.name.as_ref()),
    }
}

/// Returns whether a tool's annotations claim it does not modify its environment.
fn claims_read_only(annotations: Option<&ToolAnnotations>) -> bool {
    annotations.and_then(|annotations| annotations.read_only_hint) == Some(true)
}

/// Maps untrusted annotations onto the effects this model can defend.
///
/// # Errors
///
/// Returns [`McpToolRejection::AnnotationsConflict`] when a tool claims to be both read-only and
/// open-world, which this model cannot represent because `Effect::ReadOnly` may not be combined with
/// another effect. Refusing keeps the contradiction visible; silently dropping one of the two
/// effects would record a claim nobody made.
fn effects_for(annotations: Option<&ToolAnnotations>) -> Result<Vec<Effect>, McpToolRejection> {
    let open_world = annotations.and_then(|annotations| annotations.open_world_hint) == Some(true);
    let destructive =
        annotations.and_then(|annotations| annotations.destructive_hint) == Some(true);
    if claims_read_only(annotations) {
        if open_world {
            return Err(McpToolRejection::AnnotationsConflict);
        }
        return Ok(vec![Effect::ReadOnly]);
    }
    // A tool that does not claim read-only is recorded as writing, because the absence of a claim is
    // not a claim of safety. `openWorldHint` is mapped to external communication because that is what
    // reaching an open set of entities means here, and `destructiveHint` is only believed in the
    // direction that raises risk.
    let mut effects = vec![Effect::Write];
    if destructive {
        effects.push(Effect::Destructive);
    }
    if open_world {
        effects.push(Effect::ExternalCommunication);
    }
    Ok(effects)
}

/// Derives risk from the effects that were accepted, rather than from any hint about risk.
///
/// MCP has no risk annotation, so this is the only honest source: a read-only tool is low, a
/// destructive one is high, and everything else sits between. Deriving it means the label cannot
/// disagree with the effects, which is the same reason the domain computes `is_consequential`.
fn risk_of(effects: &[Effect]) -> Risk {
    if effects == [Effect::ReadOnly] {
        Risk::Low
    } else if effects.contains(&Effect::Destructive) {
        Risk::High
    } else {
        Risk::Moderate
    }
}

/// Maps the idempotency hint onto a retry posture.
///
/// Only the two safe directions are taken. A read-only tool, or one that claims repeated identical
/// calls have no additional effect, is treated as naturally idempotent and may be retried; anything
/// else is [`Idempotency::None`], which means non-retryable. A server claiming its tool *is*
/// idempotent only widens retrying for a tool that already claimed no effect, and no claim here can
/// make a side-effecting call retryable.
fn idempotency_of(annotations: Option<&ToolAnnotations>, read_only: bool) -> Idempotency {
    let claims_idempotent =
        annotations.and_then(|annotations| annotations.idempotent_hint) == Some(true);
    if read_only || claims_idempotent {
        Idempotency::NaturallyIdempotent
    } else {
        Idempotency::None
    }
}
