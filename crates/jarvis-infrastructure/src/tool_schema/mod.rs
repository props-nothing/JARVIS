//! Bounded JSON Schema 2020-12 validation for tool arguments: `TLS-003`'s remaining half.
//!
//! [`ToolArguments`] carries a document and bounds its size. That is *shape* checking; it is not
//! *schema* checking. This module is the missing half, so that `tool.schema_invalid` has a producer
//! and an argument document a tool cannot accept is refused before any executor sees it.
//!
//! # Where this lives, and why not in the domain
//!
//! The domain layer cannot host it. `jarvis-domain` carries no JSON dependency at all — a parsed
//! schema and a parsed instance are both [`serde_json::Value`]s — and the supported-subset decision
//! is an integration decision rather than a domain one. `TLS-003` records exactly that: "a schema
//! validator needs a supported-subset decision (which dialect keywords JARVIS honours) and belongs
//! wherever that decision is owned, which is not the domain layer." So this module sits beside
//! `tool_fingerprint`, which owns the other computation the domain can only describe.
//!
//! # A schema is checked once, and refused rather than trimmed
//!
//! [`ToolSchema::parse`] walks the whole document and refuses it unless **every** keyword in it is
//! one this module implements. That is stricter than the specification, which says unrecognised
//! keywords "SHOULD be treated as annotations" (Core §6.5) — and the strictness is deliberate. The
//! annotation is discarded, so a schema relying on a keyword JARVIS ignored would validate **less**
//! than its author asked for while reporting success. That is the fail-open direction, and it is the
//! one direction a validation boundary must not have. A refusal at parse time is a configuration
//! defect reported to an operator; a silently-ignored assertion is a malformed tool call admitted.
//!
//! The same reasoning covers the keywords this module recognises and deliberately does not
//! implement: `pattern`, `patternProperties`, `format`, the `content*` family, `multipleOf`, the
//! `contains` family, `dependentSchemas`, `dependentRequired`, `unevaluatedProperties`,
//! `unevaluatedItems`, `$id`, `$anchor`, `$dynamicAnchor`, `$dynamicRef`, and `$vocabulary`. Each is
//! refused by name so the reason is visible, rather than ignored so the omission is not.
//!
//! Two of those refusals are worth stating, because a reader will otherwise assume an oversight:
//!
//! - **`pattern` and `patternProperties`** are refused because evaluating them needs a regular
//!   expression engine, and choosing one is a dependency decision with its own review — not a
//!   keyword to hand to a hand-rolled matcher. The Validation specification's own Security
//!   Considerations section names the reason: "Regular expressions can often also be crafted to be
//!   extremely expensive to compute (with so-called 'catastrophic backtracking'), resulting in a
//!   denial-of-service attack."
//! - **`multipleOf`** is refused because its semantics are exact — "division by this keyword's
//!   value results in an integer" — over the specification's arbitrary-precision numeric data model
//!   (Core §4.2.1), and the only number representation available here is `f64`. Approximating it
//!   would make `0.3` fail a `multipleOf: 0.1` schema, which is a wrong refusal of a valid call.
//!
//! # The bound is a step budget, from two specifications
//!
//! A bounded *input* does not bound the *work*: `uniqueItems` over a long array is quadratic, and a
//! `$ref` cycle consumes no instance at all. Both specifications require the guard — Core §13:
//! "Validators should take care that the parsing and validating against schemas does not consume
//! excessive system resources. Validators MUST NOT fall into an infinite loop"; and Core §9.4.1,
//! "A schema MUST NOT be run into an infinite loop against an instance" — so evaluation spends from
//! [`MAX_SCHEMA_STEPS`] and descends no further than [`MAX_SCHEMA_DEPTH`], and exhausting either is
//! reported as its own refusal rather than as a violation of the schema. Reporting a budget refusal
//! as "your arguments are invalid" would blame the caller for the validator's own limit.
//!
//! # What a violation may contain
//!
//! A violation names the keyword and the **instance path** (a JSON Pointer, which is what the
//! specification's own output format uses), and never the instance value. The argument document is
//! untrusted model output, so echoing it into a record that reaches an operator, an audit, or a
//! client would move untrusted bytes across a trust boundary for no diagnostic gain.
//! `a_violation_names_the_path_and_never_the_instance_value` asserts that.

use std::fmt;

use serde_json::{Number, Value};

use jarvis_domain::tool::call::ToolArguments;
use jarvis_domain::tool::identity::SchemaFingerprint;

use crate::tool_fingerprint::schema_fingerprint_of;

/// The dialect this module implements.
///
/// Recorded rather than assumed: a schema naming a different dialect is refused instead of being
/// interpreted as 2020-12, because the two can disagree about what a keyword means (2020-12's
/// `items` is not draft-7's) and a wrong interpretation is indistinguishable from a right one.
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The largest schema document accepted.
///
/// Bounded where it is *accepted* rather than where it is used, and deliberately smaller than the
/// 64 KiB argument bound: a tool's input schema is reviewed configuration, and a schema larger than
/// this is a defect before it is a limit. The schema is parsed and walked on every call, so this is
/// also the per-call cost bound.
pub const MAX_SCHEMA_BYTES: usize = 32 * 1024;

/// The deepest schema or instance descent allowed.
///
/// One counter covers schema descent, instance descent, and `$ref` hops, because all three recurse
/// in this module and a bound missing any of them is not a bound. A `$ref` cycle in particular
/// consumes no instance, so this is the only thing that stops it.
///
/// **It is deliberately far below the JSON parser's own limit, and that is the point.** Measured:
/// `serde_json` refuses a document nested more than 127 containers deep, and a schema needs roughly
/// two containers per level (`{"allOf":[` is an object and an array), so a bound set near 128 could
/// never fire — a schema deep enough to reach it is already refused as "not JSON", and the bound
/// would be a declaration nothing enforces. At 32 the load-time check fires first and reports a
/// *schema* problem, which is a diagnosable one, rather than a parse error that reads as a corrupt
/// document.
pub const MAX_SCHEMA_DEPTH: usize = 32;

/// The number of evaluation steps one call may spend.
///
/// This is the resource bound the two specifications require. It is charged per keyword applied and
/// per `uniqueItems` comparison, so a quadratic uniqueness scan over a long array is bounded by the
/// same counter as a `$ref` cycle — one bound, covering both shapes of exhaustion.
pub const MAX_SCHEMA_STEPS: usize = 200_000;

/// The longest keyword name echoed into a refusal.
///
/// A schema is untrusted when it comes from a server, so a keyword name is untrusted text. A name
/// longer than this cannot be a keyword any implementation defines, so it is truncated rather than
/// reproduced in full.
const MAX_ECHOED_BYTES: usize = 64;

/// The longest rendered instance path.
///
/// The path is built from instance property names, so it is an echo of untrusted input even though
/// it carries no values. Bounded so a document cannot make one violation's path arbitrarily large.
pub const MAX_VIOLATION_PATH_BYTES: usize = 256;

/// The `$defs` prefix a local reference must use.
const DEFS_PREFIX: &str = "#/$defs/";

// ---------------------------------------------------------------------------------------
// Keyword classification.
// ---------------------------------------------------------------------------------------

/// The shape a keyword's value must have.
///
/// A table rather than a `match` per keyword, so adding a keyword cannot add a rule to the parser
/// and leave the loader without one: [`shape_of`] is the single place a keyword is named, and both
/// the shape check and the recursion read the same answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Any JSON value.
    Any,
    /// A string.
    String,
    /// A boolean.
    Boolean,
    /// A number.
    Number,
    /// A non-negative integer.
    NonNegativeInteger,
    /// A non-empty array of unique strings.
    NonEmptyStringArray,
    /// The `type` keyword: a type name, or a non-empty array of unique type names.
    TypeSpec,
    /// A non-empty array of any values.
    NonEmptyArray,
    /// A schema: an object or a boolean.
    Schema,
    /// A non-empty array of schemas.
    NonEmptySchemaArray,
    /// An object whose every member value is a schema.
    SchemaMap,
}

/// Every keyword this module supports, with the shape its value must have.
///
/// **One table, and it is the single place a keyword is named.** A violation carries a
/// `&'static str` so an attacker-influenced keyword string can never reach a durable record, and
/// the only way a name can become `&'static` here is by being a row of this table — so "the names
/// this build honours" and "the names it may report" cannot drift apart. Adding a keyword is one
/// row, and forgetting a shape check is not possible, because the shape is the row.
const KEYWORDS: &[(&str, Shape)] = &[
    // Core.
    ("$schema", Shape::String),
    ("$ref", Shape::String),
    ("$defs", Shape::SchemaMap),
    ("$comment", Shape::String),
    // Validation, any instance type (Validation §6.1).
    ("type", Shape::TypeSpec),
    ("enum", Shape::NonEmptyArray),
    ("const", Shape::Any),
    // Numeric (§6.2).
    ("minimum", Shape::Number),
    ("maximum", Shape::Number),
    ("exclusiveMinimum", Shape::Number),
    ("exclusiveMaximum", Shape::Number),
    // Strings (§6.3).
    ("minLength", Shape::NonNegativeInteger),
    ("maxLength", Shape::NonNegativeInteger),
    // Arrays (§6.4).
    ("minItems", Shape::NonNegativeInteger),
    ("maxItems", Shape::NonNegativeInteger),
    ("uniqueItems", Shape::Boolean),
    ("prefixItems", Shape::NonEmptySchemaArray),
    ("items", Shape::Schema),
    // Objects (§6.5).
    ("minProperties", Shape::NonNegativeInteger),
    ("maxProperties", Shape::NonNegativeInteger),
    ("required", Shape::NonEmptyStringArray),
    ("properties", Shape::SchemaMap),
    ("additionalProperties", Shape::Schema),
    ("propertyNames", Shape::Schema),
    // Applicators (Core §10).
    ("allOf", Shape::NonEmptySchemaArray),
    ("anyOf", Shape::NonEmptySchemaArray),
    ("oneOf", Shape::NonEmptySchemaArray),
    ("not", Shape::Schema),
    ("if", Shape::Schema),
    ("then", Shape::Schema),
    ("else", Shape::Schema),
    // Annotations (§9), collected and ignored for the verdict.
    ("title", Shape::String),
    ("description", Shape::String),
    ("default", Shape::Any),
    ("examples", Shape::Any),
    ("deprecated", Shape::Boolean),
    ("readOnly", Shape::Boolean),
    ("writeOnly", Shape::Boolean),
];

/// The row a supported keyword names, or `None` when the keyword is not supported.
///
/// Returns `None` for both "recognised and deliberately unimplemented" and "not a keyword at all";
/// [`is_refused_keyword`] separates them so a refusal can name which of the two reasons applies.
fn keyword_row(keyword: &str) -> Option<&'static (&'static str, Shape)> {
    KEYWORDS.iter().find(|(name, _)| *name == keyword)
}

/// The shape a supported keyword's value must have.
fn shape_of(keyword: &str) -> Option<Shape> {
    keyword_row(keyword).map(|(_, shape)| *shape)
}

/// The `&'static` name of a supported keyword.
///
/// This is what a violation reports, so the name in a durable record is always a literal from
/// [`KEYWORDS`] rather than a string the schema supplied.
fn static_name(keyword: &str) -> Option<&'static str> {
    keyword_row(keyword).map(|(name, _)| *name)
}

/// The keywords this module recognises and deliberately does not implement.
///
/// Named individually rather than lumped into "unknown", because the two refusals mean different
/// things: an unsupported keyword is a capability this build does not have, while an unknown one is
/// a schema this build cannot interpret at all. Both are refused, and the operator is told which.
fn is_refused_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        // Regular expressions, which need a reviewed engine (see the module docs).
        "pattern"
            | "patternProperties"
            // Exact decimal arithmetic, which `f64` cannot provide.
            | "multipleOf"
            // Format assertion is OPTIONAL and "MUST be disabled by default" (Validation §7.2.1).
            | "format"
            // The content vocabulary is annotation-only and needs media-type handling.
            | "contentEncoding"
            | "contentMediaType"
            | "contentSchema"
            // Annotation-dependent applicators, which need annotation collection.
            | "contains"
            | "minContains"
            | "maxContains"
            | "unevaluatedProperties"
            | "unevaluatedItems"
            // Conditional requirements, refused with the rest of the dependent family.
            | "dependentSchemas"
            | "dependentRequired"
            // Base-URI and dialect machinery this module does not implement, so ignoring them
            // would change how a reference resolves.
            | "$id"
            | "$anchor"
            | "$dynamicAnchor"
            | "$dynamicRef"
            | "$vocabulary"
            // Pre-2020-12 aliases. Core Appendix A keeps `definitions` in the default meta-schema
            // for a transitional period and says implementations SHOULD treat it as `$defs`; this
            // module refuses it instead, so a schema that meant something under an older draft is
            // never silently reinterpreted under 2020-12.
            | "definitions"
            | "dependencies"
            | "additionalItems"
    )
}

/// The type names `type` accepts, in the specification's own order (Validation §6.1.1).
const TYPE_NAMES: [&str; 7] = [
    "null", "boolean", "object", "array", "number", "string", "integer",
];

// ---------------------------------------------------------------------------------------
// Refusals.
// ---------------------------------------------------------------------------------------

/// A schema this module refuses to use.
///
/// Every variant is a **load-time** refusal: a schema that produces one is never used to validate
/// anything, so a caller that holds a [`ToolSchema`] holds one that was fully walked. These never
/// reach a client — the client-facing code for an unacceptable argument document is
/// `tool.schema_invalid`, which the caller chooses — so they carry operator diagnostics rather than
/// stable contract codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaRejection {
    /// The document is larger than [`MAX_SCHEMA_BYTES`].
    TooLarge {
        /// The size given.
        bytes: usize,
        /// The bound.
        max: usize,
    },
    /// The document is not JSON.
    NotJson,
    /// The document is JSON but not a schema (a schema must be an object or a boolean).
    NotASchema {
        /// The schema location at fault.
        at: String,
    },
    /// The document declares a dialect this module does not implement.
    UnsupportedDialect {
        /// The dialect it declared.
        found: String,
    },
    /// The keyword is recognised and deliberately not implemented.
    UnsupportedKeyword {
        /// The keyword.
        keyword: String,
        /// The schema location it appeared at.
        at: String,
    },
    /// The keyword is not one this module knows, so it cannot be honoured.
    UnknownKeyword {
        /// The keyword.
        keyword: String,
        /// The schema location it appeared at.
        at: String,
    },
    /// A supported keyword's value has the wrong shape.
    MalformedKeyword {
        /// The keyword.
        keyword: String,
        /// The schema location it appeared at.
        at: String,
    },
    /// A `$ref` does not name a definition in this schema's own `$defs`.
    UnresolvedReference {
        /// The reference.
        reference: String,
        /// The schema location it appeared at.
        at: String,
    },
    /// The schema nests deeper than [`MAX_SCHEMA_DEPTH`].
    TooDeep {
        /// The schema location at fault.
        at: String,
    },
}

impl fmt::Display for SchemaRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, max } => {
                write!(
                    formatter,
                    "schema is {bytes} bytes, over the {max}-byte bound"
                )
            }
            Self::NotJson => formatter.write_str("schema is not valid JSON"),
            Self::NotASchema { at } => {
                write!(
                    formatter,
                    "schema at {at} is neither an object nor a boolean"
                )
            }
            Self::UnsupportedDialect { found } => write!(
                formatter,
                "schema declares dialect {found}, and only {DIALECT} is implemented"
            ),
            Self::UnsupportedKeyword { keyword, at } => {
                write!(formatter, "keyword {keyword} at {at} is not implemented")
            }
            Self::UnknownKeyword { keyword, at } => write!(
                formatter,
                "unknown keyword {keyword} at {at} cannot be honoured"
            ),
            Self::MalformedKeyword { keyword, at } => {
                write!(formatter, "keyword {keyword} at {at} has the wrong shape")
            }
            Self::UnresolvedReference { reference, at } => write!(
                formatter,
                "reference {reference} at {at} does not name a definition in $defs"
            ),
            Self::TooDeep { at } => write!(
                formatter,
                "schema nests deeper than {MAX_SCHEMA_DEPTH} levels at {at}"
            ),
        }
    }
}

impl std::error::Error for SchemaRejection {}

/// Why an argument document was refused.
///
/// Three variants rather than one, because the three call for different responses from an operator:
/// an unparseable document is a malformed tool call, a keyword violation is a tool call the schema
/// refused, and a budget refusal is this validator's own limit — blaming the caller for the last one
/// would be a lie about the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentViolation {
    /// The argument document is not parseable as JSON.
    ///
    /// The parser's own nesting limit lands here too: a document nested more deeply than the parser
    /// admits is a resource-exhaustion attempt, and it is unparseable from this module's side
    /// whatever the reason.
    Unparseable,
    /// The document violated a keyword.
    Keyword {
        /// The keyword, always from this module's closed set.
        keyword: &'static str,
        /// The instance path, as a JSON Pointer. Empty at the document root.
        path: String,
    },
    /// Evaluation exhausted [`MAX_SCHEMA_STEPS`].
    StepsExhausted {
        /// The bound that was exhausted.
        max: usize,
    },
    /// Evaluation descended deeper than [`MAX_SCHEMA_DEPTH`].
    TooDeep,
}

impl ArgumentViolation {
    /// The keyword at fault, or `None` when the refusal was not a keyword violation.
    #[must_use]
    pub fn keyword(&self) -> Option<&'static str> {
        match self {
            Self::Keyword { keyword, .. } => Some(keyword),
            Self::Unparseable | Self::StepsExhausted { .. } | Self::TooDeep => None,
        }
    }
}

impl fmt::Display for ArgumentViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unparseable => formatter.write_str("arguments are not valid JSON"),
            Self::Keyword { keyword, path } if path.is_empty() => {
                write!(
                    formatter,
                    "arguments violate {keyword} at the document root"
                )
            }
            Self::Keyword { keyword, path } => {
                write!(formatter, "arguments violate {keyword} at {path}")
            }
            Self::StepsExhausted { max } => {
                write!(formatter, "validation exceeded its {max}-step budget")
            }
            Self::TooDeep => write!(
                formatter,
                "validation descended deeper than {MAX_SCHEMA_DEPTH} levels"
            ),
        }
    }
}

impl std::error::Error for ArgumentViolation {}

/// A definition's stated schema fingerprint is not the one its schema produces.
///
/// An operator-facing refusal rather than a contract code: a definition is reviewed configuration, so
/// a mismatch is a review or packaging defect that no client could have caused and no client can fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintMismatch {
    /// The fingerprint the definition stated.
    pub stated: String,
    /// The fingerprint computed from the schema text.
    pub computed: String,
}

impl fmt::Display for FingerprintMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the definition states {} and the schema hashes to {}",
            self.stated, self.computed
        )
    }
}

impl std::error::Error for FingerprintMismatch {}

// ---------------------------------------------------------------------------------------
// The schema.
// ---------------------------------------------------------------------------------------

/// A schema that has been walked, refused if unusable, and fingerprinted.
///
/// The text, the parsed document, and the fingerprint are produced together by [`Self::parse`] and
/// never separately, because the three must agree: the fingerprint is what a tool's identity is
/// bound to (`ACC-024`), so a fingerprint computed over text other than the text that was validated
/// would let a schema change move the identity while validation kept using the old rules, or the
/// reverse. One constructor is what makes that unrepresentable.
#[derive(Clone, PartialEq)]
pub struct ToolSchema {
    text: String,
    document: Value,
    fingerprint: SchemaFingerprint,
}

/// Prints the size and the fingerprint, never the document.
///
/// Hand-written rather than derived for the reason every other port-bearing type in this workspace
/// hand-writes it: a schema can come from a server, so its bytes are untrusted text, and a derived
/// `Debug` would put them into any log line that formats a value holding one.
///
/// `finish_non_exhaustive` is what marks the omission: it renders the struct without `..`, so a
/// reader sees that fields exist and were left out, rather than a struct that looks complete because
/// its `document` field was renamed in the output.
impl fmt::Debug for ToolSchema {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolSchema")
            .field("bytes", &self.text.len())
            .field("fingerprint", &self.fingerprint.to_string())
            .finish_non_exhaustive()
    }
}

impl ToolSchema {
    /// Parses, walks, and fingerprints a schema document.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaRejection`] when the document is over [`MAX_SCHEMA_BYTES`], is not JSON, is
    /// not a schema, declares another dialect, contains a keyword this module does not implement,
    /// contains a keyword it cannot interpret, contains a malformed keyword value, contains a
    /// `$ref` that does not resolve within its own `$defs`, or nests past [`MAX_SCHEMA_DEPTH`].
    pub fn parse(text: &str) -> Result<Self, SchemaRejection> {
        if text.len() > MAX_SCHEMA_BYTES {
            return Err(SchemaRejection::TooLarge {
                bytes: text.len(),
                max: MAX_SCHEMA_BYTES,
            });
        }
        let document: Value = serde_json::from_str(text).map_err(|_| SchemaRejection::NotJson)?;
        check_root(&document)?;
        check_schema(&document, &document, "", 0)?;
        Ok(Self {
            fingerprint: schema_fingerprint_of(text),
            text: text.to_owned(),
            document,
        })
    }

    /// Returns the schema's canonical text, exactly as it was parsed.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the schema's fingerprint, computed over [`Self::text`].
    #[must_use]
    pub fn fingerprint(&self) -> &SchemaFingerprint {
        &self.fingerprint
    }

    /// Returns the schema text's size in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Refuses a definition review whose stated fingerprint is not this schema's.
    ///
    /// **This is what stops the validator from being a component nothing consults.** A tool's
    /// identity is bound to a [`SchemaFingerprint`] and an approval is recorded against that
    /// identity, so the fingerprint a definition carries must describe the schema that calls are
    /// actually validated against. Without this check a definition could carry one schema's
    /// fingerprint while another schema — a looser one — was the document in force:
    /// `ToolIdentity::authorizes` would then authorize every call, because the identity never
    /// changed, while the rules that decide acceptance had. That is `ACC-024`'s failure mode
    /// (replacing a tool behind the identity an approval was recorded against) reached through a
    /// pair of values that disagree instead of through a display name.
    ///
    /// Comparing here rather than at each call site is deliberate: there is one place that decides
    /// whether a fingerprint and a schema belong together, so a caller cannot check it by a weaker
    /// rule — a length comparison, say — and pass.
    ///
    /// # Errors
    ///
    /// Returns [`FingerprintMismatch`] naming both fingerprints when they differ.
    pub fn confirms(&self, stated: &SchemaFingerprint) -> Result<(), FingerprintMismatch> {
        if self.fingerprint == *stated {
            return Ok(());
        }
        Err(FingerprintMismatch {
            stated: stated.to_string(),
            computed: self.fingerprint.to_string(),
        })
    }

    /// Always `false`: a constructed schema is non-empty, because an empty document is not JSON.
    ///
    /// Present because a `len` without an `is_empty` is a lint, and answering `true` would
    /// contradict the constructor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Decides whether an argument document satisfies this schema.
    ///
    /// Takes the domain's [`ToolArguments`] rather than a `&str` so the argument byte bound is
    /// enforced by the type: a caller cannot reach this function with an unbounded document, and
    /// cannot pass a schema where arguments belong.
    ///
    /// # Errors
    ///
    /// Returns [`ArgumentViolation`] naming the first keyword the document violates, or reporting
    /// that the document is unparseable, that the step budget was exhausted, or that evaluation
    /// descended too far.
    pub fn validate(&self, arguments: &ToolArguments) -> Result<(), ArgumentViolation> {
        let instance: Value =
            serde_json::from_str(arguments.as_str()).map_err(|_| ArgumentViolation::Unparseable)?;
        let mut walk = Walk {
            budget: Budget {
                steps: MAX_SCHEMA_STEPS,
            },
            path: Vec::new(),
            root: &self.document,
        };
        evaluate(&mut walk, &self.document, &instance, 0)
    }
}

// ---------------------------------------------------------------------------------------
// Loading: walking a schema and refusing what cannot be honoured.
// ---------------------------------------------------------------------------------------

/// Refuses a document that is neither an object nor a boolean (Core §4.3: a schema MUST be one).
fn check_root(document: &Value) -> Result<(), SchemaRejection> {
    if document.is_object() || document.is_boolean() {
        Ok(())
    } else {
        Err(SchemaRejection::NotASchema { at: String::new() })
    }
}

/// Walks a schema at load time, refusing anything that could not be honoured at call time.
///
/// Recursive over the schema's own structure, so it needs its own depth bound: 32 KiB of nested
/// `{"allOf":[` is several thousand levels, and a walk without one would exhaust the stack before
/// any call-time budget applied. The dialect check lives here because `$schema` is only meaningful
/// at the document root, and a nested `$schema` would be a different schema resource's dialect
/// declaration, which this module does not implement.
///
/// `root` is threaded through so a `$ref` can be checked against the schema's own `$defs` — the
/// reference is resolved here, at load, so a dangling one is an operator-visible configuration
/// defect rather than a call-time failure the model's arguments get blamed for.
fn check_schema(
    root: &Value,
    schema: &Value,
    at: &str,
    depth: usize,
) -> Result<(), SchemaRejection> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(SchemaRejection::TooDeep { at: at.to_owned() });
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or_else(|| SchemaRejection::NotASchema { at: at.to_owned() })?;
    for (keyword, value) in object {
        check_keyword(root, keyword, value, at, depth)?;
    }
    Ok(())
}

/// Checks one keyword's value and recurses into any subschemas it holds.
fn check_keyword(
    root: &Value,
    keyword: &str,
    value: &Value,
    at: &str,
    depth: usize,
) -> Result<(), SchemaRejection> {
    let echoed = bounded_echo(keyword);
    let Some(shape) = shape_of(keyword) else {
        return Err(if is_refused_keyword(keyword) {
            SchemaRejection::UnsupportedKeyword {
                keyword: echoed,
                at: at.to_owned(),
            }
        } else {
            SchemaRejection::UnknownKeyword {
                keyword: echoed,
                at: at.to_owned(),
            }
        });
    };
    if !shape_matches(shape, value) {
        return Err(SchemaRejection::MalformedKeyword {
            keyword: echoed,
            at: at.to_owned(),
        });
    }
    if keyword == "$schema" && value.as_str() != Some(DIALECT) {
        return Err(SchemaRejection::UnsupportedDialect {
            found: bounded_echo(value.as_str().unwrap_or_default()),
        });
    }
    if keyword == "$ref" {
        return check_reference(root, value.as_str().unwrap_or_default(), at);
    }
    if keyword == "type" {
        return check_type_spec(value, at);
    }
    let child = schema_path(at, keyword);
    match shape {
        Shape::Schema => check_schema(root, value, &child, depth + 1),
        Shape::SchemaMap => match value.as_object() {
            Some(members) => {
                for (name, nested) in members {
                    check_schema(root, nested, &schema_path(&child, name), depth + 1)?;
                }
                Ok(())
            }
            None => Ok(()),
        },
        Shape::NonEmptySchemaArray => match value.as_array() {
            Some(items) => {
                for (index, nested) in items.iter().enumerate() {
                    check_schema(
                        root,
                        nested,
                        &schema_path(&child, &index.to_string()),
                        depth + 1,
                    )?;
                }
                Ok(())
            }
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Checks that a `$ref` is a `$defs` pointer naming a definition that exists.
///
/// Resolution happens at **load** time rather than at call time. A reference is also existence-only
/// checked — the target is not walked again here, because it is walked anyway as a member of
/// `$defs` — so a `$ref` cycle terminates during the load walk and is instead bounded by
/// [`MAX_SCHEMA_DEPTH`] during evaluation.
fn check_reference(root: &Value, reference: &str, at: &str) -> Result<(), SchemaRejection> {
    let refusal = || SchemaRejection::UnresolvedReference {
        reference: bounded_echo(reference),
        at: at.to_owned(),
    };
    let name = reference
        .strip_prefix(DEFS_PREFIX)
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .ok_or_else(refusal)?;
    match root.get("$defs").and_then(Value::as_object) {
        Some(definitions) if definitions.contains_key(name) => Ok(()),
        _ => Err(refusal()),
    }
}

/// Checks the `type` keyword: a name from the closed set, or a non-empty array of unique names.
fn check_type_spec(value: &Value, at: &str) -> Result<(), SchemaRejection> {
    let malformed = || SchemaRejection::MalformedKeyword {
        keyword: String::from("type"),
        at: at.to_owned(),
    };
    match value {
        Value::String(name) => {
            if TYPE_NAMES.contains(&name.as_str()) {
                Ok(())
            } else {
                Err(malformed())
            }
        }
        Value::Array(names) => {
            if names.is_empty() {
                return Err(malformed());
            }
            let mut seen: Vec<&str> = Vec::with_capacity(names.len());
            for name in names {
                let name = name.as_str().ok_or_else(malformed)?;
                if !TYPE_NAMES.contains(&name) || seen.contains(&name) {
                    return Err(malformed());
                }
                seen.push(name);
            }
            Ok(())
        }
        _ => Err(malformed()),
    }
}

/// Returns whether a value has the shape a keyword requires.
fn shape_matches(shape: Shape, value: &Value) -> bool {
    match shape {
        // No constraint on the value's shape: an annotation may hold anything, and `type` is
        // validated by `check_type_spec` rather than here.
        Shape::Any | Shape::TypeSpec => true,
        Shape::String => value.is_string(),
        Shape::Boolean => value.is_boolean(),
        Shape::Number => value.is_number(),
        Shape::NonNegativeInteger => value
            .as_u64()
            .is_some_and(|number| u32::try_from(number).is_ok()),
        Shape::NonEmptyStringArray => value
            .as_array()
            .is_some_and(|items| non_empty_unique_strings(items)),
        Shape::NonEmptyArray => value.as_array().is_some_and(|items| !items.is_empty()),
        Shape::Schema => value.is_object() || value.is_boolean(),
        Shape::NonEmptySchemaArray => value.as_array().is_some_and(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.is_object() || item.is_boolean())
        }),
        Shape::SchemaMap => value.is_object(),
    }
}

/// Returns whether an array is non-empty and holds unique strings.
fn non_empty_unique_strings(items: &[Value]) -> bool {
    if items.is_empty() {
        return false;
    }
    let mut seen: Vec<&str> = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(name) if !seen.contains(&name) => seen.push(name),
            _ => return false,
        }
    }
    true
}

// ---------------------------------------------------------------------------------------
// Evaluation.
// ---------------------------------------------------------------------------------------

/// The budget, the instance path, and the root document for one evaluation.
///
/// Grouped because the recursive walk threads all three through every call — and `clippy`'s
/// argument-count limit is what turned that into a design signal rather than noise: a function
/// taking a schema, an instance, a depth, a path, a budget, and a root is carrying six things whose
/// relationships are not visible from its signature. The path is also the one piece of state with a
/// strict bracket discipline (push before, pop after), and `below_name`/`below_index` make that
/// discipline impossible to get wrong at a call site.
struct Walk<'a> {
    /// The budget being spent.
    budget: Budget,
    /// The instance path, as JSON Pointer segments.
    path: Vec<String>,
    /// The whole schema document, for `$ref` resolution.
    root: &'a Value,
}

impl Walk<'_> {
    /// Builds a violation at the current instance path.
    fn violation(&self, keyword: &'static str) -> ArgumentViolation {
        ArgumentViolation::Keyword {
            keyword,
            path: render_path(&self.path),
        }
    }

    /// Runs `body` with one property name appended to the path, then removes it.
    fn below_name<T>(&mut self, name: &str, body: impl FnOnce(&mut Walk<'_>) -> T) -> T {
        self.path.push(name.to_owned());
        let outcome = body(self);
        self.path.pop();
        outcome
    }

    /// Runs `body` with one array index appended to the path, then removes it.
    fn below_index<T>(&mut self, index: usize, body: impl FnOnce(&mut Walk<'_>) -> T) -> T {
        self.path.push(index.to_string());
        let outcome = body(self);
        self.path.pop();
        outcome
    }
}

/// The step budget for one call.
struct Budget {
    steps: usize,
}

impl Budget {
    /// Spends one step, or reports the budget exhausted.
    fn spend(&mut self) -> Result<(), ArgumentViolation> {
        if self.steps == 0 {
            return Err(ArgumentViolation::StepsExhausted {
                max: MAX_SCHEMA_STEPS,
            });
        }
        self.steps -= 1;
        Ok(())
    }
}

/// Evaluates an instance against a schema, returning the first violation.
///
/// Recurses on three axes — into subschemas, into child instances, and across `$ref` — so all three
/// increment `depth`: a `$ref` cycle consumes no instance and would otherwise recurse without bound,
/// and the two specifications forbid that by name (Core §13, §9.4.1).
fn evaluate(
    walk: &mut Walk<'_>,
    schema: &Value,
    instance: &Value,
    depth: usize,
) -> Result<(), ArgumentViolation> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(ArgumentViolation::TooDeep);
    }
    walk.budget.spend()?;
    if let Some(allowed) = schema.as_bool() {
        // A boolean schema: `true` is the empty schema and `false` fails everything (Core §4.3.2).
        return if allowed {
            Ok(())
        } else {
            Err(walk.violation("false"))
        };
    }
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    let node = Node { object };
    for (keyword, value) in object {
        // A keyword the table does not name cannot be present: `check_schema` refused the whole
        // document at load, so this is unreachable rather than lenient — and it asserts that rather
        // than silently validating nothing, which is the fail-open direction.
        let Some(named) = static_name(keyword) else {
            debug_assert!(false, "an unsupported keyword survived the load-time walk");
            continue;
        };
        apply(walk, &node, named, value, instance, depth)?;
    }
    Ok(())
}

/// Applies one keyword to one instance.
///
/// `named` is the `&'static` literal from [`KEYWORDS`] rather than a name borrowed from the schema
/// object, so a violation can only ever report a keyword this build defines. That is why the
/// parameter exists and why it is not simply the key the walk is iterating: the schema's own text is
/// untrusted input, and a violation reaches a durable record.
fn apply(
    walk: &mut Walk<'_>,
    node: &Node<'_>,
    named: &'static str,
    value: &Value,
    instance: &Value,
    depth: usize,
) -> Result<(), ArgumentViolation> {
    match named {
        // Two groups decide nothing about the verdict, for different reasons:
        //
        // - annotations, which carry no assertion (Validation §9), and `$defs`, which is a reserved
        //   location whose members apply only through a `$ref`;
        // - `then` and `else`, which `if` reads — "when 'if' is not present, both 'then' and 'else'
        //   MUST be entirely ignored" (Core §10.2.2).
        //
        // `$schema` belongs here too: its only effect is the dialect check `check_schema` performed.
        "title" | "description" | "default" | "examples" | "deprecated" | "readOnly"
        | "writeOnly" | "$comment" | "$schema" | "$defs" | "then" | "else" => Ok(()),
        "$ref" => {
            // Load-time checking resolved every reference, so a failure here is unreachable for a
            // schema built through `parse`; it is reported as a `$ref` violation rather than
            // unwrapped because this is a validation path and a panic here would be a daemon fault
            // caused by model input.
            let target = resolve(walk.root, value.as_str().unwrap_or_default())
                .ok_or_else(|| walk.violation(named))?;
            evaluate(walk, target, instance, depth + 1)
        }
        "type" => check_type(value, instance, walk),
        "enum" => check_enum(value, instance, walk),
        "const" => check_const(value, instance, walk),
        "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
            check_numeric(named, value, instance, walk)
        }
        "minLength" | "maxLength" => check_string_length(named, value, instance, walk),
        "minItems" | "maxItems" | "uniqueItems" | "prefixItems" | "items" => {
            check_array(walk, node, named, value, instance, depth)
        }
        "minProperties"
        | "maxProperties"
        | "required"
        | "properties"
        | "additionalProperties"
        | "propertyNames" => check_object(walk, node, named, value, instance, depth),
        "allOf" | "anyOf" | "oneOf" => check_logic(walk, named, value, instance, depth),
        "not" => invert(evaluate(walk, value, instance, depth + 1), named, walk),
        "if" => check_conditional(walk, node, value, instance, depth),
        _ => {
            debug_assert!(false, "every table row must have an arm");
            Ok(())
        }
    }
}

/// Resolves a local reference against the root document.
///
/// Load-time checking has already refused anything that is not a `#/$defs/<name>` pointer naming an
/// existing definition, so this cannot fail for a schema that was constructed through
/// [`ToolSchema::parse`] — but it returns an `Option` anyway rather than unwrapping, because the
/// alternative is a panic on a validation path and this module has no panic-free way to assert that
/// the two checks agree without duplicating one of them.
fn resolve<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let name = reference
        .strip_prefix(DEFS_PREFIX)
        .filter(|name| !name.is_empty() && !name.contains('/'))?;
    root.get("$defs")?.get(name)
}

// ---------------------------------------------------------------------------------------
// Keyword implementations.
// ---------------------------------------------------------------------------------------

/// Returns the primitive type name of an instance, per the specification's data model.
fn type_name(instance: &Value) -> &'static str {
    match instance {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Returns whether an instance is an integer: a number with a zero fractional part.
fn is_integer(instance: &Value) -> bool {
    match instance {
        Value::Number(number) => {
            number.as_i64().is_some()
                || number.as_u64().is_some()
                || number.as_f64().is_some_and(|value| value.fract() == 0.0)
        }
        _ => false,
    }
}

/// Checks the `type` keyword (Validation §6.1.1).
fn check_type(value: &Value, instance: &Value, walk: &Walk<'_>) -> Result<(), ArgumentViolation> {
    let matched = match value {
        Value::String(name) => matches_one(name, instance),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| matches_one(name, instance)),
        _ => true,
    };
    if matched {
        Ok(())
    } else {
        Err(walk.violation("type"))
    }
}

/// Returns whether an instance matches one type name.
///
/// `integer` is the one name no single comparison covers, because the specification's data model has
/// no integer type: an integer is "any number with a zero fractional part", so `3.0` is one and
/// `3.5` is not.
fn matches_one(name: &str, instance: &Value) -> bool {
    if name == "integer" {
        return is_integer(instance);
    }
    type_name(instance) == name
}

/// Checks `enum`: the instance must equal one of the listed values.
fn check_enum(value: &Value, instance: &Value, walk: &Walk<'_>) -> Result<(), ArgumentViolation> {
    let candidates = value.as_array().map(Vec::as_slice).unwrap_or_default();
    if candidates
        .iter()
        .any(|candidate| json_equal(candidate, instance))
    {
        Ok(())
    } else {
        Err(walk.violation("enum"))
    }
}

/// Checks `const`: the instance must equal the listed value.
fn check_const(value: &Value, instance: &Value, walk: &Walk<'_>) -> Result<(), ArgumentViolation> {
    if json_equal(value, instance) {
        Ok(())
    } else {
        Err(walk.violation("const"))
    }
}

/// Checks a numeric bound.
///
/// Passes when the instance is not a number, which the specification states as a rule rather than
/// an omission: "Most assertions only constrain values within a certain primitive type. When the
/// type of the instance is not of the type targeted by the keyword, the instance is considered to
/// conform to the assertion" (Core §7.6.1). Refusing a string here would make `maxLength` and
/// `maximum` unusable together with a multi-typed schema.
fn check_numeric(
    keyword: &'static str,
    value: &Value,
    instance: &Value,
    walk: &Walk<'_>,
) -> Result<(), ArgumentViolation> {
    // The comparison is exact rather than approximate: a bound is a bound, and `maximum: 3` must
    // refuse `3.000000001`. A tolerance is right for comparing two *measurements* and wrong for
    // enforcing a limit the schema stated.
    let satisfied = {
        let (Some(bound), Value::Number(number)) = (value.as_f64(), instance) else {
            return Ok(());
        };
        let Some(actual) = number.as_f64() else {
            return Ok(());
        };
        match keyword {
            "minimum" => actual >= bound,
            "maximum" => actual <= bound,
            "exclusiveMinimum" => actual > bound,
            "exclusiveMaximum" => actual < bound,
            _ => true,
        }
    };
    if satisfied {
        Ok(())
    } else {
        Err(walk.violation(keyword))
    }
}

/// Checks a string length bound, counted in characters as the specification requires.
fn check_string_length(
    keyword: &'static str,
    value: &Value,
    instance: &Value,
    walk: &Walk<'_>,
) -> Result<(), ArgumentViolation> {
    let (Some(bound), Value::String(text)) = (value.as_u64(), instance) else {
        return Ok(());
    };
    // Characters, not bytes: "The length of a string instance is defined as the number of its
    // characters" (Validation §6.3.1). A byte count refuses a legitimate four-character string that
    // happens to be eight bytes, which is a wrong refusal of a valid call.
    let length = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
    let satisfied = match keyword {
        "minLength" => length >= bound,
        "maxLength" => length <= bound,
        _ => true,
    };
    if satisfied {
        Ok(())
    } else {
        Err(walk.violation(keyword))
    }
}

/// A schema node with the keywords that decide how its applicators behave.
///
/// `additionalProperties` is defined in terms of a **sibling** `properties`
/// (Core §10.3.2.3: it applies only to names that "do not appear in the annotation results of
/// either 'properties' or 'patternProperties'"), and `items` in terms of a sibling `prefixItems`
/// (Core §10.3.1.2: it applies to indexes "greater than the length of the 'prefixItems' array in
/// the same schema object"). So both need the schema OBJECT, not the keyword's own value — and
/// reading those siblings here, in one place, is what stops the two keywords from disagreeing about
/// the boundary they share.
struct Node<'a> {
    object: &'a serde_json::Map<String, Value>,
}

impl<'a> Node<'a> {
    /// The names a sibling `properties` declares, if any.
    fn declared_names(&self) -> Option<&'a serde_json::Map<String, Value>> {
        self.object.get("properties").and_then(Value::as_object)
    }

    /// How many leading items a sibling `prefixItems` already evaluated.
    fn prefix_len(&self) -> usize {
        self.object
            .get("prefixItems")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
}

/// Checks every array keyword.
fn check_array(
    walk: &mut Walk<'_>,
    node: &Node<'_>,
    keyword: &'static str,
    value: &Value,
    instance: &Value,
    depth: usize,
) -> Result<(), ArgumentViolation> {
    let Some(items) = instance.as_array() else {
        return Ok(());
    };
    let length = u64::try_from(items.len()).unwrap_or(u64::MAX);
    match keyword {
        "minItems" if length < value.as_u64().unwrap_or(0) => {
            return Err(walk.violation("minItems"));
        }
        "maxItems" if length > value.as_u64().unwrap_or(0) => {
            return Err(walk.violation("maxItems"));
        }
        "uniqueItems" if value.as_bool() == Some(true) => {
            return check_unique(items, walk);
        }
        "prefixItems" => {
            let schemas = value.as_array().map(Vec::as_slice).unwrap_or_default();
            for (index, schema) in schemas.iter().enumerate() {
                let Some(item) = items.get(index) else {
                    break;
                };
                walk.below_index(index, |walk| evaluate(walk, schema, item, depth + 1))?;
            }
        }
        "items" => {
            // The sibling decides the start index, so `items` covers exactly the items
            // `prefixItems` did not.
            for (index, item) in items.iter().enumerate().skip(node.prefix_len()) {
                walk.below_index(index, |walk| evaluate(walk, value, item, depth + 1))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Checks `uniqueItems` by pairwise comparison, charging each comparison to the budget.
///
/// The comparison is where a long array becomes quadratic, so this is the second place the step
/// budget is charged — "a bounded input does not bound the work" made concrete.
fn check_unique(items: &[Value], walk: &mut Walk<'_>) -> Result<(), ArgumentViolation> {
    for (index, item) in items.iter().enumerate() {
        for other in items.iter().skip(index + 1) {
            walk.budget.spend()?;
            if json_equal(item, other) {
                return Err(walk.violation("uniqueItems"));
            }
        }
    }
    Ok(())
}

/// Checks every object keyword.
fn check_object(
    walk: &mut Walk<'_>,
    node: &Node<'_>,
    keyword: &'static str,
    value: &Value,
    instance: &Value,
    depth: usize,
) -> Result<(), ArgumentViolation> {
    let Some(members) = instance.as_object() else {
        return Ok(());
    };
    let count = u64::try_from(members.len()).unwrap_or(u64::MAX);
    match keyword {
        "minProperties" if count < value.as_u64().unwrap_or(0) => {
            return Err(walk.violation("minProperties"));
        }
        "maxProperties" if count > value.as_u64().unwrap_or(0) => {
            return Err(walk.violation("maxProperties"));
        }
        "required" => {
            let names = value.as_array().map(Vec::as_slice).unwrap_or_default();
            for name in names.iter().filter_map(Value::as_str) {
                if !members.contains_key(name) {
                    return Err(walk.violation("required"));
                }
            }
        }
        "properties" => {
            if let Some(declared) = value.as_object() {
                for (name, schema) in declared {
                    if let Some(child) = members.get(name) {
                        walk.below_name(name, |walk| evaluate(walk, schema, child, depth + 1))?;
                    }
                }
            }
        }
        "additionalProperties" => {
            // The sibling decides which names were already covered; a name it declared is not
            // "additional", even when the schema it declared happens to be `true`.
            let declared = node.declared_names();
            for (name, child) in members {
                if declared.is_some_and(|declared| declared.contains_key(name)) {
                    continue;
                }
                walk.below_name(name, |walk| evaluate(walk, value, child, depth + 1))?;
            }
        }
        "propertyNames" => {
            for name in members.keys() {
                let as_instance = Value::String(name.clone());
                walk.below_name(name, |walk| evaluate(walk, value, &as_instance, depth + 1))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Checks `allOf`, `anyOf`, and `oneOf`.
///
/// A failing subschema is **not** returned early, even under `allOf`, because returning it would
/// report the deeper keyword rather than the combinator — and the combinator is the one an operator
/// can act on. `anyOf` and `oneOf` also need every branch evaluated, so short-circuiting would make
/// the two cases disagree about which branches ran.
fn check_logic(
    walk: &mut Walk<'_>,
    keyword: &'static str,
    value: &Value,
    instance: &Value,
    depth: usize,
) -> Result<(), ArgumentViolation> {
    let schemas = value.as_array().map(Vec::as_slice).unwrap_or_default();
    let branches = schemas
        .iter()
        .filter(|schema| evaluate(walk, schema, instance, depth + 1).is_ok())
        .count();
    let satisfied = u32::try_from(branches).unwrap_or(u32::MAX);
    let total = u32::try_from(schemas.len()).unwrap_or(u32::MAX);
    let okay = match keyword {
        // Every branch must hold. `allOf` with an empty array is refused at load, so `0 == 0` cannot
        // make this pass vacuously.
        "allOf" => satisfied == total,
        "anyOf" => satisfied >= 1,
        "oneOf" => satisfied == 1,
        _ => true,
    };
    if okay {
        Ok(())
    } else {
        Err(walk.violation(keyword))
    }
}

/// Checks `if`, applying `then` or `else` according to the outcome.
///
/// `then` and `else` return `Ok` on their own, because the specification is explicit: "when 'if' is
/// not present, both 'then' and 'else' MUST be entirely ignored" (Core §10.2.2). Their sibling `if`
/// is read here so that rule has one home.
fn check_conditional(
    walk: &mut Walk<'_>,
    node: &Node<'_>,
    value: &Value,
    instance: &Value,
    depth: usize,
) -> Result<(), ArgumentViolation> {
    let outcome = evaluate(walk, value, instance, depth + 1).is_ok();
    let branch = if outcome { "then" } else { "else" };
    let Some(schema) = node.object.get(branch) else {
        return Ok(());
    };
    evaluate(walk, schema, instance, depth + 1)
}

/// Turns a nested outcome into the negation `not` requires.
///
/// Only a **keyword** violation counts as "the subschema failed", so the negation succeeds there.
/// A budget or depth refusal is propagated unchanged: both report a limit of this validator rather
/// than a property of the schema, and letting `not` invert one would turn "we could not finish
/// checking" into "the schema accepts this" — the fail-open direction.
fn invert(
    outcome: Result<(), ArgumentViolation>,
    keyword: &'static str,
    walk: &Walk<'_>,
) -> Result<(), ArgumentViolation> {
    match outcome {
        Err(ArgumentViolation::Keyword { .. }) => Ok(()),
        Err(other) => Err(other),
        Ok(()) => Err(walk.violation(keyword)),
    }
}

// ---------------------------------------------------------------------------------------
// Equality, paths, and small helpers.
// ---------------------------------------------------------------------------------------

/// Compares two instances for equality as the specification defines it (Core §4.2.2).
///
/// Not `Value`'s own `PartialEq`, and the difference is not cosmetic: the specification says two
/// numbers are equal when they "have the same mathematical value" and that "mere formatting
/// differences (indentation, placement of commas, **trailing zeros**) are insignificant", while
/// `serde_json`'s comparison distinguishes the lexical forms it happens to keep. So `1` and `1.0`
/// must compare equal here — a schema with `enum: [1.0]` accepts an argument of `1` — and this is
/// the one function that decides it, for `const`, `enum`, and `uniqueItems` alike.
fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => numbers_equal(a, b),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| json_equal(value, other)))
        }
        _ => false,
    }
}

/// Compares two numbers by mathematical value, as the specification's data model defines equality.
///
/// Integers are compared as integers first so two values above `f64`'s exact range are not collapsed
/// into one another; only a pair that is not both integral falls back to `f64`. That fallback is
/// documented rather than hidden: a value beyond `2^53` compared against a non-integral bound is
/// approximated, which is the one place this module's numeric handling is not exact — and it is why
/// `multipleOf`, whose semantics need exactness throughout, is refused at load.
///
/// The final comparison is exact rather than approximate, and `clippy` suggests a tolerance: a
/// tolerance is right for comparing two measurements and wrong for the equality rule, where `1` and
/// `1.5` are different numbers and no margin makes them the same.
#[expect(
    clippy::float_cmp,
    reason = "the equality rule is exact; a tolerance would call two different numbers equal"
)]
fn numbers_equal(left: &Number, right: &Number) -> bool {
    if let (Some(a), Some(b)) = (left.as_i64(), right.as_i64()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (left.as_u64(), right.as_u64()) {
        return a == b;
    }
    match (left.as_f64(), right.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Renders an instance path as a JSON Pointer, bounded.
///
/// The segments are instance property names, so this is an echo of untrusted input even though it
/// carries no values — which is why it is bounded as well as escaped. `~` and `/` are escaped in
/// that order, as RFC 6901 requires: escaping the slash first would escape the tilde it had just
/// introduced, so a name containing both would render to a pointer that decodes to a third string.
fn render_path(segments: &[String]) -> String {
    let mut rendered = String::new();
    for segment in segments {
        rendered.push('/');
        rendered.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        if rendered.len() > MAX_VIOLATION_PATH_BYTES {
            rendered.truncate(MAX_VIOLATION_PATH_BYTES);
            rendered.push_str("<truncated>");
            return rendered;
        }
    }
    rendered
}

/// Builds a schema path by appending one member.
fn schema_path(at: &str, member: &str) -> String {
    let mut rendered = String::with_capacity(at.len() + member.len() + 1);
    rendered.push_str(at);
    rendered.push('/');
    rendered.push_str(member);
    if rendered.len() > MAX_VIOLATION_PATH_BYTES {
        rendered.truncate(MAX_VIOLATION_PATH_BYTES);
        rendered.push_str("<truncated>");
    }
    rendered
}

/// Bounds a keyword name or reference before it is echoed into a refusal.
fn bounded_echo(text: &str) -> String {
    if text.len() <= MAX_ECHOED_BYTES {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(MAX_ECHOED_BYTES).collect();
    truncated.push_str("<truncated>");
    truncated
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "composition_tests.rs"]
mod composition_tests;
