//! Tool arguments and results: bounded, sensitivity-labelled, and untrusted all the way through.
//!
//! Two rules from the architecture govern everything here, and both are about what a **bound** is
//! for:
//!
//! > Results are validated, size-bounded, sensitivity-labelled, and treated as untrusted when
//! > returned to a model or UI.
//!
//! So a result is never a `String` that happens to be short. It is a [`ToolResultBody`], which
//! cannot be constructed without passing its bound, and it carries the [`Sensitivity`] it is
//! classified at — because the classification is what decides whether it may be placed in a model's
//! context, and a result that reached that decision unlabelled would have to be guessed at.
//!
//! **The bound is on bytes rather than on characters, and that is deliberate.** A bound on
//! characters is a bound on neither: a single character may be four bytes, so a character-counted
//! limit admits four times the memory and four times the wire cost for the same number. Every other
//! bound in this project is in bytes for the same reason.
//!
//! These types are **not** a JSON implementation. The domain carries a document as text and checks
//! its shape; validating arguments *against a schema* is a separate concern (`TLS-003`'s second
//! half) that needs a schema subset and a validator, and neither belongs in the domain layer.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::ToolCallId;
use crate::model::policy::Sensitivity;

/// The largest tool argument document accepted.
///
/// 64 KiB, matching the model gateway's `MAX_JSON_TEXT_BYTES`. The two are the same bound for the
/// same reason — an argument document is produced by a model in both cases — and a separate,
/// larger number here would mean a tool could receive what the gateway would refuse to carry.
pub const MAX_ARGUMENT_BYTES: usize = 64 * 1024;

/// The largest tool result accepted from an adapter.
///
/// Larger than the argument bound because results legitimately carry file contents and search
/// hits, but still bounded: an unbounded result is an unbounded write to a durable record and an
/// unbounded entry in a model's context, and both are limits a provider must not be able to raise
/// by returning more.
pub const MAX_RESULT_BYTES: usize = 256 * 1024;

/// The largest number of content blocks one result may carry.
pub const MAX_RESULT_BLOCKS: usize = 64;

/// The largest number of artifacts one result may reference.
pub const MAX_RESULT_ARTIFACTS: usize = 32;

/// The longest accepted provider reference.
pub const MAX_PROVIDER_REFERENCE_BYTES: usize = 256;

/// Validates the shared text rules: non-empty, NUL-free, within a byte bound.
///
/// One function rather than the check repeated at each constructor, because the three rules travel
/// together and a constructor that remembered two of them would be a bound that is almost right.
fn usable_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains('\0')
}

/// The arguments a call was dispatched with.
///
/// Wraps the text exactly as [`crate::model::stream::JsonText`] does, and for the same reason: the
/// domain carries the document and bounds it, while validating it against a schema is the schema
/// owner's job. The name differs because the meaning does — these are arguments rather than a
/// requested output schema — and one type serving both would let a caller pass a schema where
/// arguments are required.
///
/// **`Deserialize` goes through [`Self::new`]** so the byte bound and the NUL rule hold for values that
/// arrive over the wire as well as for values this crate builds. A derived impl would have accepted an
/// oversized argument document from a client, which is the bound's whole purpose defeated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ToolArguments(String);

impl ToolArguments {
    /// Validates and wraps an argument document.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `arguments` when the document is empty,
    /// over [`MAX_ARGUMENT_BYTES`], or contains a NUL byte.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if !usable_text(value, MAX_ARGUMENT_BYTES) {
            return Err(DomainError::ToolDefinitionInvalid { field: "arguments" });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the document text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the document's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always `false`: a constructed value is non-empty. Present because a `len` without an
    /// `is_empty` is a lint, and answering `true` here would contradict the constructor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<'de> Deserialize<'de> for ToolArguments {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

/// One block of tool result content.
///
/// The kinds are the contract's: structured JSON, plain text, or an artifact reference. A **typed**
/// set rather than a free-form `type` string, because a consumer that does not recognize a kind must
/// refuse the block rather than pass it through, and the contract says as much: "unknown content
/// blocks are bounded and preserved safely or marked unsupported".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// A structured JSON value carried as text.
    Json {
        /// The document.
        value: ResultPayload,
    },
    /// Plain text.
    Text {
        /// The text.
        text: ResultPayload,
    },
    /// A reference to an artifact stored outside the result.
    ///
    /// **A reference rather than the bytes.** A result that inlined a file would put the file in
    /// every durable record and every model context that mentions it, which is the same "the
    /// record owns content it does not need" problem the artifact indirection exists to solve.
    Artifact {
        /// An opaque identifier JARVIS can resolve.
        id: String,
        /// The artifact's media type, for a consumer that must decide how to render it.
        media_type: String,
        /// The artifact's size in bytes, so a consumer can refuse one too large to handle without
        /// fetching it first.
        size: u64,
    },
}

/// One block's payload, bounded by the **result** bound rather than the argument bound.
///
/// **A separate type from [`ToolArguments`], and the separation is a fix rather than a nicety.** A
/// first version reused `ToolArguments` here, which bounded every result block at 64 KiB — the
/// *argument* limit. The two bounds differ on purpose and for a stated reason: an argument document
/// is produced by a model, while a result legitimately carries file contents and search hits, which
/// is why [`MAX_RESULT_BYTES`] is four times larger. Reusing one type for both made the larger bound
/// unreachable: no single block could exceed 64 KiB, so a result that legitimately returned a 100 KiB
/// file would have been refused with `tool.limit_exceeded` and the operator's bound would not have
/// explained why. Two bounds need two types, or the stricter one silently governs both.
///
/// **`Deserialize` goes through [`Self::new`]** for the same reason as [`ToolArguments`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ResultPayload(String);

impl ResultPayload {
    /// Validates and wraps a block payload.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `content` when the payload is empty,
    /// over [`MAX_RESULT_BYTES`], or contains a NUL byte. The field is `content` rather than
    /// `payload` because that is the name the contract gives the envelope this appears in, and an
    /// operator looking at a refusal needs the contract's word for it.
    pub fn new(value: &str) -> Result<Self, DomainError> {
        if !usable_text(value, MAX_RESULT_BYTES) {
            return Err(DomainError::ToolDefinitionInvalid { field: "content" });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the payload text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the payload's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always `false`: a constructed value is non-empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<'de> Deserialize<'de> for ResultPayload {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(serde::de::Error::custom)
    }
}

impl ContentBlock {
    /// Builds a structured-content block.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `content` for an unusable document.
    pub fn json(value: &str) -> Result<Self, DomainError> {
        Ok(Self::Json {
            value: ResultPayload::new(value)?,
        })
    }

    /// Builds a text block.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `content` for unusable text.
    pub fn text(value: &str) -> Result<Self, DomainError> {
        Ok(Self::Text {
            text: ResultPayload::new(value)?,
        })
    }

    /// Builds an artifact reference.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `artifact` when the identifier or media
    /// type is unusable, or `artifact.size` when it is zero — a zero-byte artifact is what an
    /// unpopulated field produces, and a consumer that trusted it would fetch an empty document.
    pub fn artifact(id: &str, media_type: &str, size: u64) -> Result<Self, DomainError> {
        if !usable_text(id, MAX_PROVIDER_REFERENCE_BYTES)
            || !usable_text(media_type, MAX_PROVIDER_REFERENCE_BYTES)
        {
            return Err(DomainError::ToolDefinitionInvalid { field: "artifact" });
        }
        if size == 0 {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "artifact.size",
            });
        }
        Ok(Self::Artifact {
            id: id.to_owned(),
            media_type: media_type.to_owned(),
            size,
        })
    }

    /// Returns the bytes this block occupies, by whatever measure its kind uses.
    ///
    /// A block's "size" differs by kind — a document's byte length, an artifact's declared size —
    /// and the caller needs one number to enforce a total against. Computing it here keeps the
    /// definition beside the kinds rather than in the caller, where a new kind would be silently
    /// counted as zero.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        match self {
            Self::Json { value } | Self::Text { text: value } => value.len() as u64,
            Self::Artifact { size, .. } => *size,
        }
    }
}

/// A tool's result, validated and bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolResultBody {
    /// The content blocks, in the order the provider returned them.
    pub content: Vec<ContentBlock>,
    /// An opaque provider reference, for linking to protected diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reference: Option<String>,
    /// The classification of this result.
    pub sensitivity: Sensitivity,
}

impl ToolResultBody {
    /// Validates and builds a result.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `content`, `provider_reference`, or
    /// `result` — the last when the **total** exceeds [`MAX_RESULT_BYTES`], which is refused rather
    /// than truncated. **Refused rather than truncated on purpose**: a truncated JSON document is
    /// invalid and a truncated text result silently misleads, so the safe outcome is a typed refusal
    /// a caller can act on (`tool.limit_exceeded`) rather than a plausible-looking partial answer.
    ///
    /// The total is checked **here** as well as being answerable via
    /// [`Self::is_within_bound`](Self::is_within_bound), and both are needed. The per-block bound
    /// cannot substitute for it: [`MAX_RESULT_BLOCKS`] blocks each at [`MAX_RESULT_BYTES`] is 64 times
    /// the intended total, so the constructor has to add them up. Leaving this to the caller is what
    /// the first version did, and a result of sixty-four legal blocks then constructed successfully
    /// while the whole point of a bound is that the invalid value cannot be built.
    pub fn new(
        content: Vec<ContentBlock>,
        provider_reference: Option<&str>,
        sensitivity: Sensitivity,
    ) -> Result<Self, DomainError> {
        if content.is_empty() {
            return Err(DomainError::ToolDefinitionInvalid { field: "content" });
        }
        if content.len() > MAX_RESULT_BLOCKS {
            return Err(DomainError::ToolDefinitionInvalid { field: "content" });
        }
        if let Some(reference) = provider_reference
            && !usable_text(reference, MAX_PROVIDER_REFERENCE_BYTES)
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "provider_reference",
            });
        }
        let body = Self {
            content,
            provider_reference: provider_reference.map(str::to_owned),
            sensitivity,
        };
        if !body.is_within_bound() {
            return Err(DomainError::ToolDefinitionInvalid { field: "result" });
        }
        Ok(body)
    }

    /// Returns the total bytes this result occupies.
    ///
    /// A **total**, so it is checked once here and once at construction rather than per block: a
    /// per-block bound alone admits a result of `MAX_RESULT_BLOCKS` blocks each at the block limit,
    /// which is 64 times the intended bound. The two bounds are not redundant — the block bound stops
    /// one enormous block, and this one stops many legal ones.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.content.iter().map(ContentBlock::size_bytes).sum()
    }

    /// Returns whether this result is within the total byte bound.
    #[must_use]
    pub fn is_within_bound(&self) -> bool {
        self.size_bytes() <= MAX_RESULT_BYTES as u64
    }

    /// Returns whether the result carries an artifact reference.
    ///
    /// An artifact means the result is **incomplete without a second read**, which a consumer must
    /// know because placing an artifact *reference* in a model's context is not the same as placing
    /// the artifact in it: the model sees an identifier it cannot read.
    #[must_use]
    pub fn has_artifacts(&self) -> bool {
        self.content
            .iter()
            .any(|block| matches!(block, ContentBlock::Artifact { .. }))
    }
}

impl<'de> Deserialize<'de> for ToolResultBody {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            content: Vec<ContentBlock>,
            #[serde(default)]
            provider_reference: Option<String>,
            sensitivity: Sensitivity,
        }
        let body = Wire::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived.** The total byte bound and the non-empty block
        // list are enforced in the constructor — the round's own note calls the total "the bound with
        // teeth" — and a derived impl rebuilt an empty or over-total result for any document that
        // carried one. A tool result is an unbounded write to a durable record and to a model's context,
        // which is the bound the constructor exists to keep.
        Self::new(
            body.content,
            body.provider_reference.as_deref(),
            body.sensitivity,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// What a caller asked for, before it was validated or authorized.
///
/// **Holds no principal, workspace, or grant.** The contract is explicit: "Principal/workspace/
/// grants are trusted context, not accepted from model arguments." So an intent that carried them
/// would invite a caller to take them from here, and the trust boundary would move to whatever
/// produced the intent — which is commonly a model. The identity a call is authorized under is
/// resolved server-side and passed *alongside* an intent rather than inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCallIntent {
    /// The call's own identity.
    pub call_id: ToolCallId,
    /// The canonical capability being invoked, as the model named it.
    ///
    /// A **string** rather than a [`super::identity::ToolCapability`], and the difference is the
    /// trust boundary: a model emits a tool name, and a name is resolved against the registry rather
    /// than believed. Parsing it into a capability here would make an unresolvable name
    /// unrepresentable, and "the model named a tool that does not exist" is a case a caller has to
    /// report as `tool.not_found` rather than refuse at parse time.
    pub capability: String,
    /// The arguments the model produced.
    pub arguments: ToolArguments,
    /// A display and audit summary of the reason, never authorization.
    ///
    /// The contract says this explicitly: "`reason_summary` is display/audit context and never
    /// authorization."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_summary: Option<String>,
}

impl ToolCallIntent {
    /// Builds an intent, validating the fields that are JARVIS's to bound.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `capability` or `reason_summary`. The
    /// capability is bounded but deliberately **not** parsed as a canonical form here; see the field
    /// comment for why.
    pub fn new(
        call_id: ToolCallId,
        capability: &str,
        arguments: ToolArguments,
        reason_summary: Option<&str>,
    ) -> Result<Self, DomainError> {
        if !usable_text(capability, super::identity::MAX_TOOL_SEGMENT_BYTES * 3) {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "capability",
            });
        }
        if let Some(summary) = reason_summary
            && !usable_text(summary, MAX_PROVIDER_REFERENCE_BYTES)
        {
            return Err(DomainError::ToolDefinitionInvalid {
                field: "reason_summary",
            });
        }
        Ok(Self {
            call_id,
            capability: capability.to_owned(),
            arguments,
            reason_summary: reason_summary.map(str::to_owned),
        })
    }
}

impl<'de> Deserialize<'de> for ToolCallIntent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            call_id: ToolCallId,
            capability: String,
            arguments: ToolArguments,
            #[serde(default)]
            reason_summary: Option<String>,
        }
        let intent = Wire::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived.** The capability bound and the reason-summary
        // bound are enforced only in the constructor — an unbounded capability is an unbounded lookup
        // key and an unbounded log field, and `reason_summary` reaches a log line and an audit record.
        // The derived impl accepted both unchecked.
        Self::new(
            intent.call_id,
            &intent.capability,
            intent.arguments,
            intent.reason_summary.as_deref(),
        )
        .map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ToolCallIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The arguments are **not** rendered. They come from a model, may embed untrusted document
        // content, and this type's `Display` reaches logs: the same rule that keeps entire
        // untrusted payloads out of a log line.
        write!(
            formatter,
            "call {} invoking {} ({} argument byte(s))",
            self.call_id,
            self.capability,
            self.arguments.len()
        )
    }
}
