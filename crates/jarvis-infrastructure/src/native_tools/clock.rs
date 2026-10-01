//! The first native tool: read the current time.
//!
//! A model cannot know the date, and "what time is it" is a question an assistant is asked
//! constantly. This is the tool that makes the fabric **do something observable** rather than prove
//! itself against a double: it is read-only, low risk, needs no credential, no network, and no new
//! dependency, and its result is a value a test can assert against the injected clock.
//!
//! ## Why this tool, and not a file read
//!
//! It would be more impressive to ship `fs.read` first, and it would also be dishonest. The
//! architecture requires a filesystem tool to open files through **open-relative, no-follow**
//! primitives, because check-then-open has a TOCTOU race that `TLS-007` names explicitly — and those
//! primitives are `unsafe` FFI this workspace forbids (`unsafe-code = "deny"`) or a `cap-std`-class
//! dependency that needs its own evidence note. Shipping `canonicalize`-then-`open` while the path
//! grants were enforced would put an unreviewed race underneath the one layer designed to prevent
//! it, which is worse than not shipping the tool.
//!
//! So the first tool is the one whose whole effect is reading the daemon's own clock. It exercises
//! every stage of the pipeline — resolve, validate, fingerprint, policy, ledger, execute, record —
//! with an effect that cannot hurt anything, and `TLS-007`'s named gap stays named rather than
//! papered over.
//!
//! ## The definition is built here, and that is deliberate
//!
//! A tool's definition is **reviewed configuration**, and this module is where the review lives for
//! the one native tool: the effects, the risk, the approval hint, the idempotency declaration, and
//! the schema are all written out rather than derived from the executor. An executor that generated
//! its own definition would let a tool declare its own classification, which is the one input the
//! architecture says must come from trusted configuration.

use jarvis_domain::error::DomainError;
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk,
};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};

use crate::tool_fingerprint::schema_fingerprint_of;

/// The canonical capability this tool implements.
///
/// `clock.now@1` rather than `time.now` because the namespace names the *implementation*, and this
/// implementation has no argument and returns a single instant. A future tool that reads a
/// timezone or performs arithmetic is a different capability with its own major.
pub const CAPABILITY: &str = "clock.now@1";

/// The input schema, as reviewed configuration.
///
/// **An explicit empty object, not `{"type":"object"}` alone**, and both halves matter: a schema
/// must be present (a tool with no schema is refused by `SchemaValidator`), and `additionalProperties`
/// is `false` so an argument the model invents is a **violation** rather than something silently
/// ignored. A model that sends `{"timezone":"Europe/Amsterdam"}` to this tool is making a mistake,
/// and the validator's job is to tell it so — accepting the extra key would let it believe the
/// timezone was honoured.
///
/// `"$schema"` is present because a schema naming a dialect is interpreted under it, and this
/// module's parser refuses a document that names another one.
pub const INPUT_SCHEMA: &str = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":{}}"#;

/// The result's media classification.
///
/// `Internal` rather than `Public`: the instant a user asked for is not secret, but it is a fact
/// about the user's session and the tool's own output label is what the context budgeter reads. A
/// tool that labelled its output as broadly shareable would be making a claim about the user's data
/// that the tool's author cannot know.
const RESULT_SENSITIVITY: Sensitivity = Sensitivity::Internal;

/// Builds the reviewed definition for this tool.
///
/// # Errors
///
/// Returns [`DomainError`] if the reviewed constants are internally inconsistent — a typo in the
/// schema fingerprint derivation or a version whose major disagrees with the capability's. This is a
/// **construction** fault in JARVIS's own configuration rather than anything a caller can cause, so
/// it is returned rather than panicked: a daemon that cannot compose its tool catalog must report
/// that at startup instead of aborting, which is the same rule every other composition step follows.
pub fn definition() -> Result<ToolDefinition, DomainError> {
    let capability = ToolCapability::parse(CAPABILITY)?;
    let source = ToolSource::new(
        SourceKind::Native,
        // `jarvis.core` is the daemon's own publisher identity. It is a *publisher* rather than a
        // capability namespace, which is why it is dotted — the rule `is_owner` records.
        "jarvis.core",
        ToolVersion::parse("1.0.0")?,
    )?;
    // The fingerprint is **derived from the schema text**, never written by hand. A hand-written
    // fingerprint is the defect `BRN-063` recorded: an identity that cannot move when the schema
    // changes, so `ACC-024` has nothing behind it. Deriving it here means editing `INPUT_SCHEMA`
    // changes the identity, which is exactly what the contract requires.
    let schema_fingerprint =
        SchemaFingerprint::parse(&schema_fingerprint_of(INPUT_SCHEMA).to_string())?;
    let identity = ToolIdentity {
        capability,
        source,
        schema_fingerprint,
    };
    ToolDefinition::new(
        identity,
        "Current time",
        "Returns the current UTC instant from the daemon's own clock.",
        vec![Effect::ReadOnly],
        // Low, and the label is honest: reading a clock is not consequential. The policy evaluator
        // uses it for the read-only-and-low-risk fast path, which is what lets this tool run without
        // a prompt — the one arm of policy that allows without a grant, and it requires BOTH
        // conditions rather than either.
        Risk::Low,
        Vec::new(),
        // **`Allow`, and it is the one value the fast path accepts.** Policy's read-only fast path
        // requires this hint, so a tool that declared `Ask` would prompt on every clock read — which
        // is a prompt a user learns to dismiss, defeating the prompt's purpose. `ToolDefinition::new`
        // permits `Allow` here precisely because the tool is low-risk, read-only, and asks for no
        // scope; declaring it for a consequential tool is refused by the constructor.
        ApprovalHint::Allow,
        // Naturally idempotent: reading a clock twice converges, and a repeat with a later instant is
        // a *different* answer rather than a duplicated effect. The reservation still scopes it by
        // the caller's key, so a retry of one turn's call is recognised as the same logical call.
        Idempotency::NaturallyIdempotent,
        DataClasses::new(Sensitivity::Public, RESULT_SENSITIVITY)?,
        ExecutionDefaults::new(5_000, 1)?,
    )
}
