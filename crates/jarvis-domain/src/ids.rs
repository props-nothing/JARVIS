//! Canonical JARVIS identifiers.
//!
//! Canonical IDs are typed newtypes over a lowercase, hyphenated `UUIDv7` value.
//! Consumers treat an ID as opaque and case-sensitive; a typed newtype prevents
//! a workspace ID from being used where a principal ID is required and
//! guarantees a single canonical string form.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;

/// Declares a UUIDv7-backed identifier newtype.
macro_rules! identifier {
    ($name:ident, $kind:literal, $doc:literal) => {
        #[doc = $doc]
        ///
        /// The inner [`Uuid`] is always canonical, so the value has exactly one
        /// string representation: lowercase hexadecimal with hyphens. The
        /// identifier kind returned in [`DomainError::InvalidIdentifier`] is
        #[doc = concat!("`", $kind, "`.")]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            /// Wraps an already-canonical UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Returns the inner UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }

            /// Parses a canonical identifier string.
            ///
            /// # Errors
            ///
            /// Returns [`DomainError::InvalidIdentifier`] when `value` is not a
            /// canonical hyphenated, lowercase UUID. Braces, a `urn:uuid:`
            /// prefix, uppercase hex, or trailing data are rejected rather than
            /// normalized, because silently accepting a lookalike form weakens
            /// the guarantee that two spellings never denote two identities.
            pub fn parse(value: &str) -> Result<Self, DomainError> {
                // `Uuid::try_parse` is deliberately lenient: it also accepts
                // uppercase hex, braced, and `urn:uuid:` forms, and the simple
                // form with no hyphens. Accepting those would let two spellings
                // denote the same identity, so a parsed value is re-rendered in
                // the canonical form and compared against the input. Only the
                // exact canonical spelling is accepted.
                let uuid = Uuid::try_parse(value)
                    .map_err(|_| DomainError::InvalidIdentifier { kind: $kind })?;
                if uuid.is_nil() || uuid.is_max() {
                    return Err(DomainError::InvalidIdentifier { kind: $kind });
                }
                if uuid.hyphenated().to_string() != value {
                    return Err(DomainError::InvalidIdentifier { kind: $kind });
                }
                Ok(Self(uuid))
            }

            /// Returns whether the value is the nil identifier.
            #[must_use]
            pub fn is_nil(&self) -> bool {
                self.0.is_nil()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Lossless and identical to `Display` so a diagnostic or log
                // line carries the same opaque value a caller would see.
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

        impl Borrow<Uuid> for $name {
            fn borrow(&self) -> &Uuid {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                /// Rejects non-canonical identifier strings on the wire.
                struct IdentifierVisitor;

                impl serde::de::Visitor<'_> for IdentifierVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a canonical JARVIS identifier string")
                    }

                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        // Go through `parse` so the wire form is held to the
                        // same canonical rule as `from_str`; a lenient derived
                        // deserializer would accept lookalike spellings.
                        $name::parse(value).map_err(E::custom)
                    }
                }

                deserializer.deserialize_str(IdentifierVisitor)
            }
        }
    };
}

identifier!(WorkspaceId, "workspace", "Identifies a workspace.");
identifier!(
    PrincipalId,
    "principal",
    "Identifies a principal (user or service)."
);
identifier!(RequestId, "request", "Identifies a single request.");
identifier!(
    CorrelationId,
    "correlation",
    "Correlates related requests and events."
);
identifier!(RunId, "run", "Identifies one durable agent run.");
identifier!(
    ConversationId,
    "conversation",
    "Identifies a durable conversation in the conversation/run context."
);
identifier!(
    MessageId,
    "message",
    "Identifies one durable message in a conversation."
);
identifier!(
    ModelCallId,
    "model_call",
    "Identifies one logical model call."
);
identifier!(
    ModelStreamEventId,
    "model_stream_event",
    "Identifies one event in a normalized model stream."
);
identifier!(
    ModelDataPolicyId,
    "model_data_policy",
    "Identifies one immutable version of a model data policy."
);
identifier!(
    ContextManifestId,
    "context_manifest",
    "Identifies one recorded context assembly."
);
identifier!(
    ModelRouteDecisionId,
    "model_route_decision",
    "Identifies one persisted route decision. A decision is an auditable record in \
     its own right — the contract requires its considered candidates and rejection \
     reasons to be readable without storing prompt content — so it has its own \
     identity rather than being derived from the call it explains."
);
identifier!(
    PolicyExceptionId,
    "policy_exception",
    "Identifies one durable model-data-policy exception. An exception is separately \
     revocable and, when single-use, consumed once, so it needs an identity of its \
     own rather than being named by the rule it relaxes — the rule is shared by \
     every exception for it, and a decision must name the exact one that permitted \
     the call."
);
identifier!(
    RunActivityEventId,
    "run_activity_event",
    "Identifies one durable run activity event, which is globally unique because \
     a client resumes an event stream by echoing this value as `Last-Event-ID`."
);

/// Generates identifiers for domain records.
///
/// The port is injected so production code uses real `UUIDv7` values while tests
/// inject a deterministic sequence. It lives in the domain layer and so does not
/// depend on any runtime.
pub trait IdGenerator: fmt::Debug + Send + Sync {
    /// Returns the next identifier for `kind`.
    ///
    /// `kind` is the identifier type name and is informational only; it exists
    /// for diagnostics and deterministic test implementations.
    fn next_uuid(&self, kind: &'static str) -> Uuid;
}

#[cfg(test)]
mod tests {
    use super::{IdGenerator, PrincipalId, RequestId, WorkspaceId};
    use crate::error::DomainError;
    use uuid::Uuid;

    const CANONICAL_V7: &str = "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d";

    #[test]
    fn canonical_uuidv7_round_trips_through_strings() {
        let parsed = WorkspaceId::parse(CANONICAL_V7).expect("canonical v7 must parse");
        assert_eq!(parsed.to_string(), CANONICAL_V7);
        assert_eq!(parsed.as_uuid().get_version_num(), 7);

        let reparsed: WorkspaceId = parsed.to_string().parse().expect("display must parse");
        assert_eq!(reparsed, parsed);
    }

    #[test]
    fn debug_and_display_agree_and_are_lossless() {
        let id = RequestId::parse(CANONICAL_V7).expect("canonical v7 must parse");
        assert_eq!(format!("{id}"), format!("{id:?}"));
        assert_eq!(format!("{id}"), CANONICAL_V7);
    }

    #[test]
    fn non_canonical_forms_are_rejected() {
        let rejected = [
            "",
            " ",
            "018f2b3c4d5e7a6b8c9d0e1f2a3b4c5d",       // no hyphens
            "018F2B3C-4D5E-7A6B-8C9D-0E1F2A3B4C5D",   // uppercase hex
            "{018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d}", // braces
            "urn:uuid:018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d", // urn prefix
            "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d ",  // trailing space
            "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5dx",  // trailing data
        ];

        for value in rejected {
            let error = WorkspaceId::parse(value).expect_err("lookalike must be rejected");
            assert_eq!(error.code(), "jarvis.invalid_identifier");
            assert!(matches!(
                error,
                DomainError::InvalidIdentifier { kind: "workspace" }
            ));
        }
    }

    #[test]
    fn nil_and_max_identifiers_are_rejected() {
        for value in [
            "00000000-0000-0000-0000-000000000000",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
        ] {
            assert!(
                PrincipalId::parse(value).is_err(),
                "sentinel {value} must not be a valid identifier",
            );
        }
    }

    #[test]
    fn serde_is_canonical_on_the_wire_and_rejects_lookalikes() {
        let id = WorkspaceId::parse(CANONICAL_V7).expect("canonical v7 must parse");

        // Serialization emits the canonical string, not a derived-struct shape.
        let json = serde_json::to_string(&id).expect("serialization succeeds");
        assert_eq!(json, format!("\"{CANONICAL_V7}\""));

        let parsed: WorkspaceId = serde_json::from_str(&json).expect("canonical wire form parses");
        assert_eq!(parsed, id);

        // A lookalike spelling must not deserialize even though the underlying
        // `uuid` crate would accept it.
        for lookalike in [
            "\"018F2B3C-4D5E-7A6B-8C9D-0E1F2A3B4C5D\"",
            "\"018f2b3c4d5e7a6b8c9d0e1f2a3b4c5d\"",
        ] {
            assert!(
                serde_json::from_str::<WorkspaceId>(lookalike).is_err(),
                "{lookalike} must not deserialize",
            );
        }
    }

    #[test]
    fn identifier_kinds_do_not_mix_at_the_type_level() {
        // This test documents intent: the types are distinct even when built
        // from the same bytes, so a workspace ID cannot flow into a principal
        // parameter without an explicit conversion.
        let workspace = WorkspaceId::parse(CANONICAL_V7).expect("canonical v7 must parse");
        let principal = PrincipalId::from_uuid(workspace.as_uuid());
        assert_eq!(workspace.to_string(), principal.to_string());
        assert_eq!(workspace.as_uuid(), principal.as_uuid());
        assert_ne!(workspace, WorkspaceId::from_uuid(Uuid::nil()));
    }

    /// A deterministic generator used to prove the port is injectable.
    #[derive(Debug)]
    struct CountingGenerator;

    impl IdGenerator for CountingGenerator {
        fn next_uuid(&self, _kind: &'static str) -> Uuid {
            Uuid::from_u128(1)
        }
    }

    #[test]
    fn id_generator_port_is_injectable_and_object_safe() {
        let generator: &dyn IdGenerator = &CountingGenerator;
        let uuid = generator.next_uuid("request");
        assert_eq!(uuid, Uuid::from_u128(1));
    }
}
