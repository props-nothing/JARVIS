//! Provider, model, and revision identity.
//!
//! The model gateway architecture is explicit that **provider**, **model**,
//! **runtime**, and **route** are different concepts and must never be used
//! interchangeably. Separate newtypes are what make that structural rather than
//! a naming convention: a [`ProviderId`] cannot be passed where a [`ModelId`] is
//! required, so the two cannot be silently transposed at a call site.
//!
//! These identifiers are deliberately **not** canonical UUIDs. A provider and a
//! model are external facts named by their owner, so their identity is owner-
//! scoped text. A model ID is scoped by its provider, which is why an
//! [`ModelRef`] carries both and a bare model ID is never globally unique.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// The longest accepted provider, model, revision, or region name.
///
/// The bound exists because these values reach logs, persisted records, and
/// routing decisions; an unbounded caller-supplied string in any of those places
/// is a resource and rendering hazard.
const MAX_NAME_BYTES: usize = 64;

/// Validates an owner-assigned name: lowercase dotted slug segments.
///
/// A value is one or more `.`-separated segments, each of which begins and ends
/// with a lowercase ASCII alphanumeric and may contain `-` or `_` inside. The
/// segment rule rejects a leading, trailing, or doubled `.` for free, so
/// `local.ollama` is accepted and `.local`, `local.`, and `local..ollama` are
/// not. Uppercase is rejected rather than folded, because accepting both
/// spellings would let two strings denote one identity.
fn validate_name(value: &str, kind: &'static str) -> Result<(), DomainError> {
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        return Err(DomainError::InvalidIdentifier { kind });
    }
    let well_formed = value.split('.').all(|segment| {
        let mut bytes = segment.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return false;
        }
        let mut last = first;
        for byte in bytes {
            let allowed =
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_';
            if !allowed {
                return false;
            }
            last = byte;
        }
        last.is_ascii_alphanumeric()
    });
    if well_formed {
        Ok(())
    } else {
        Err(DomainError::InvalidIdentifier { kind })
    }
}

/// Declares a validated, owner-assigned name newtype.
macro_rules! owner_name {
    ($name:ident, $kind:literal, $doc:literal) => {
        #[doc = $doc]
        ///
        // Not a link: `validate_name` is private to this crate, and rustdoc refuses to send the
        // reader of a **public** item's documentation to a private symbol. As plain code text the
        // name still tells a maintainer where the rule lives without promising a link that cannot
        // be followed. (Rustdoc reported this as `private-intra-doc-links` the moment the lint was
        // denied, which is why it is fixed here rather than left as a warning.)
        /// The value is a lowercase dotted slug (see `validate_name`); it is
        /// stored exactly as validated, so there is only one spelling per
        /// identity and a lookup cannot miss on case or separators.
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Builds a value from a string literal already known to be valid.
            ///
            /// A literal is validated at first use rather than with an `expect` at the
            /// call site, so a mistyped literal degrades to a fallback instead of
            /// panicking on a startup path. `None` means the literal is not a legal
            /// value, which a caller should treat as a programming error and handle by
            /// falling back rather than by unwrapping.
            #[must_use]
            pub fn from_literal(value: &'static str) -> Option<Self> {
                validate_name(value, $kind).ok().map(|()| Self(value.to_owned()))
            }

            /// Parses and validates a value.
            ///
            /// # Errors
            ///
            /// Returns [`DomainError::InvalidIdentifier`] with kind
            #[doc = concat!("`", $kind, "` when `value` is empty, longer than")]
            #[doc = concat!(" ", stringify!(MAX_NAME_BYTES), " bytes, or not a lowercase dotted slug.")]
            pub fn parse(value: &str) -> Result<Self, DomainError> {
                validate_name(value, $kind)?;
                Ok(Self(value.to_owned()))
            }

            /// Returns the value as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Lossless and identical to `Display`: a diagnostic line carries
                // the same value a persistence row or a client would see.
                write!(formatter, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = DomainError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = DomainError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                /// Holds the wire form to the same rule as `from_str`.
                struct NameVisitor;

                impl serde::de::Visitor<'_> for NameVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a lowercase dotted name")
                    }

                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        $name::parse(value).map_err(E::custom)
                    }

                    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                        self.visit_str(&value)
                    }
                }

                deserializer.deserialize_str(NameVisitor)
            }
        }
    };
}

owner_name!(
    ProviderId,
    "provider",
    "Identifies a model provider (an API, a local runtime, or a compatible endpoint)."
);
owner_name!(
    ModelId,
    "model",
    "Identifies a model within one provider's namespace."
);
owner_name!(
    ModelRevision,
    "model_revision",
    "Identifies an immutable model revision where the provider publishes one."
);
owner_name!(
    Region,
    "region",
    "Names a data-residency region as a deployment-defined label."
);

/// A provider-qualified model reference.
///
/// A model ID alone is only unique inside its provider, so routing, persisted
/// records, and evidence all carry the pair. Constructing one requires both, which
/// is what stops a bare model name from being treated as a global identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    /// The owning provider.
    pub provider_id: ProviderId,
    /// The model inside that provider.
    pub model_id: ModelId,
    /// The immutable revision, when the provider publishes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<ModelRevision>,
}

impl ModelRef {
    /// Creates a reference with no pinned revision.
    #[must_use]
    pub fn new(provider_id: ProviderId, model_id: ModelId) -> Self {
        Self {
            provider_id,
            model_id,
            revision: None,
        }
    }

    /// Returns a copy pinned to `revision`.
    #[must_use]
    pub fn with_revision(mut self, revision: ModelRevision) -> Self {
        self.revision = Some(revision);
        self
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.revision {
            Some(revision) => write!(
                formatter,
                "{}/{}@{revision}",
                self.provider_id, self.model_id
            ),
            None => write!(formatter, "{}/{}", self.provider_id, self.model_id),
        }
    }
}

/// Where a provider's endpoint sits relative to the user's trust boundary.
///
/// This is a routing input, not a claim about data handling. It exists so a
/// local-only policy can be evaluated without interpreting a provider name:
/// `local.ollama` *looks* local and a hosted endpoint reached over a private
/// network is not cloud, so the name is not evidence for either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointClass {
    /// On the user's own device, reachable with no network.
    Local,
    /// On a network the operator controls, not the public internet.
    PrivateNetwork,
    /// On an approved cloud provider.
    ApprovedCloud,
}

impl EndpointClass {
    /// Returns whether the endpoint is outside the user's device.
    #[must_use]
    pub const fn requires_network(self) -> bool {
        !matches!(self, Self::Local)
    }
}

#[cfg(test)]
mod tests {
    use super::{EndpointClass, ModelId, ModelRef, ModelRevision, ProviderId, Region};

    #[test]
    fn dotted_and_simple_names_are_accepted() {
        for value in ["local.ollama", "openai", "my-provider.v2", "a_b.c1", "x"] {
            assert!(
                ProviderId::parse(value).is_ok(),
                "{value} must be an accepted provider name",
            );
        }
    }

    #[test]
    fn lookalike_and_malformed_names_are_rejected() {
        for value in [
            "",
            " ",
            "OpenAI",         // uppercase would create a second spelling
            "openai ",        // trailing space
            " openai",        // leading space
            ".openai",        // empty leading segment
            "openai.",        // empty trailing segment
            "local..ollama",  // empty inner segment
            "local/ollama",   // path separator
            "local ollama",   // space inside
            "provider:model", // a colon invites two concepts in one value
            "café",           // non-ASCII
        ] {
            let error = ProviderId::parse(value).expect_err("must be rejected");
            assert_eq!(error.code(), "jarvis.invalid_identifier");
        }
    }

    #[test]
    fn an_over_long_name_is_rejected() {
        let long = "a".repeat(65);
        assert!(
            ModelId::parse(&long).is_err(),
            "the 64-byte bound must hold"
        );
        assert!(
            ModelId::parse(&"a".repeat(64)).is_ok(),
            "exactly 64 bytes is inside the bound",
        );
    }

    #[test]
    fn a_model_reference_requires_its_provider_and_renders_unambiguously() {
        let provider = ProviderId::parse("local.ollama").expect("valid");
        let model = ModelId::parse("llama3.1").expect("valid");
        let bare = ModelRef::new(provider.clone(), model.clone());
        assert_eq!(bare.to_string(), "local.ollama/llama3.1");
        assert!(bare.revision.is_none());

        let pinned =
            bare.with_revision(ModelRevision::parse("2025-07-23").expect("valid revision"));
        assert_eq!(pinned.to_string(), "local.ollama/llama3.1@2025-07-23");
    }

    #[test]
    fn names_round_trip_through_serde_and_reject_lookalikes() {
        let model = ModelId::parse("gpt-x1").expect("valid");
        let json = serde_json::to_string(&model).expect("serializes");
        assert_eq!(json, "\"gpt-x1\"");
        let parsed: ModelId = serde_json::from_str(&json).expect("canonical form parses");
        assert_eq!(parsed, model);
        ModelId::deserialize_from("GPT-X1").expect_err("uppercase must not deserialize");
    }

    #[test]
    fn debug_and_display_agree() {
        let region = Region::parse("eu").expect("valid");
        assert_eq!(format!("{region}"), format!("{region:?}"));
    }

    #[test]
    fn endpoint_class_reports_whether_network_is_required() {
        assert!(!EndpointClass::Local.requires_network());
        assert!(EndpointClass::PrivateNetwork.requires_network());
        assert!(EndpointClass::ApprovedCloud.requires_network());
    }

    /// Helper so the serde test reads as "a lookalike wire value is rejected".
    trait DeserializeFrom: Sized {
        fn deserialize_from(value: &str) -> Result<Self, serde_json::Error>;
    }

    impl DeserializeFrom for ModelId {
        fn deserialize_from(value: &str) -> Result<Self, serde_json::Error> {
            serde_json::from_value(serde_json::Value::String(value.to_owned()))
        }
    }
}
