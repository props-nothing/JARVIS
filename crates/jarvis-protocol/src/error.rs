//! The error envelope contract.
//!
//! Every JARVIS API error uses one stable shape (see
//! `docs/contracts/common-conventions.md`): a namespaced machine `code`, a
//! message safe to show the requesting principal, an optional request id, an
//! operation-specific `retryable` flag, and a schema-defined `details` object.
//!
//! The envelope never carries an internal stack trace, provider error text, or
//! secret material. Internal detail belongs in protected diagnostics.

use serde::{Deserialize, Serialize};

/// The namespaces JARVIS owns for error codes, as `docs/contracts/common-conventions.md`
/// declares them.
///
/// The first `.`-separated segment of a code must be one of these. The list is the wire
/// vocabulary rather than a domain concern, which is why it lives here: `jarvis-protocol`
/// depends on no other JARVIS crate, so this is the one place a boundary can consult
/// without depending inward. `jarvis_domain::ErrorCode::NAMESPACES` mirrors it for
/// domain-side code, and a cross-crate test asserts the two agree.
pub const CODE_NAMESPACES: &[&str] = &[
    "jarvis",
    "tool",
    "approval",
    "model",
    "run",
    "stream",
    "storage",
    "event",
    "session",
    "request",
    "api",
    "auth",
    "resource",
    "idempotency",
    "service",
    "internal",
];

/// The code an unrecognized namespace is replaced with at the emission boundary.
pub const INTERNAL_CODE: &str = "jarvis.internal";

/// Returns `true` when `code`'s first `.`-separated segment is a namespace JARVIS owns
/// and is followed by a non-empty name.
///
/// A bare namespace (`tool.`) is not a code — a family name names no error — and neither
/// is a code with no `.` at all.
#[must_use]
pub fn is_owned_code(code: &str) -> bool {
    let Some((namespace, rest)) = code.split_once('.') else {
        return false;
    };
    !rest.is_empty() && CODE_NAMESPACES.contains(&namespace)
}

/// The error object inside [`ErrorEnvelope`].
///
/// Build it through [`ErrorEnvelope::new`], which enforces the namespace rule; constructing
/// this struct by literal skips that check. The fields are public because this shape is the
/// wire contract and a client parses it, so a struct literal is reachable — no JARVIS
/// production path uses one, and a source scan in `jarvis-infrastructure` asserts that the
/// only construction site in this crate is the constructor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    /// A stable, namespaced machine code, for example `tool.permission_denied`.
    pub code: String,
    /// A message safe for the requesting principal.
    pub message: String,
    /// The request the error answers, when one was established.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Whether retrying this operation unchanged could succeed.
    pub retryable: bool,
    /// A suggested delay before retrying, when the operation is retryable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// Schema-defined, non-secret detail for this code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// The top-level error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    /// The error object.
    pub error: ErrorResponse,
}

impl ErrorEnvelope {
    /// Builds an envelope from a code, a safe message, and a retryability flag.
    ///
    /// A code whose namespace JARVIS does not own — or that names a namespace with no
    /// error after it — is **replaced by [`INTERNAL_CODE`]** rather than forwarded, as
    /// `docs/contracts/common-conventions.md` requires. This is the fail-closed direction:
    /// a provider, plugin, or runtime cannot name a code a client would read as JARVIS's
    /// own, and a typo cannot invent one. The replacement is silent because the envelope
    /// has nowhere to record why; callers that need to know should log at the point they
    /// build the code.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        let code = code.into();
        let code = if is_owned_code(&code) {
            code
        } else {
            INTERNAL_CODE.to_owned()
        };
        Self {
            error: ErrorResponse {
                code,
                message: message.into(),
                request_id: None,
                retryable,
                retry_after_ms: None,
                details: None,
            },
        }
    }

    /// Attaches the request id.
    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.error.request_id = Some(request_id.into());
        self
    }

    /// Returns the machine code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.error.code
    }

    /// Serializes the envelope to JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns an error only if serialization fails, which cannot happen for
    /// this shape.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{CODE_NAMESPACES, ErrorEnvelope, INTERNAL_CODE};

    #[test]
    fn the_envelope_shape_matches_the_contract() {
        let envelope = ErrorEnvelope::new(
            "tool.permission_denied",
            "This capability is not allowed in the active workspace.",
            false,
        )
        .with_request_id("0195f4ed-624a-77c2-b3c9-f695867e9ec0");

        let json = String::from_utf8(envelope.to_bytes().expect("serializes")).expect("utf8");
        assert!(json.contains(r#""code":"tool.permission_denied""#));
        assert!(json.contains(r#""retryable":false"#));
        assert!(json.contains(r#""request_id":"0195f4ed-624a-77c2-b3c9-f695867e9ec0""#));
        // Absent optional fields are omitted rather than serialized as null.
        assert!(!json.contains("retry_after_ms"), "{json}");
        assert!(!json.contains("details"), "{json}");
    }

    #[test]
    fn unknown_envelope_fields_are_rejected() {
        let with_extra =
            br#"{"error":{"code":"a","message":"b","retryable":false,"stack":"boom"}}"#;
        assert!(serde_json::from_slice::<ErrorEnvelope>(with_extra).is_err());
    }

    #[test]
    fn a_serialized_envelope_round_trips() {
        let envelope =
            ErrorEnvelope::new("jarvis.db_open", "The database could not be opened.", true);
        let bytes = envelope.to_bytes().expect("serializes");
        let parsed: ErrorEnvelope = serde_json::from_slice(&bytes).expect("parses");
        assert_eq!(parsed, envelope);
        assert_eq!(parsed.code(), "jarvis.db_open");
    }

    #[test]
    fn a_code_outside_the_owned_namespaces_is_replaced() {
        // The fail-closed direction the contract requires. A code a provider or a caller
        // sent must not reach a client as though JARVIS had produced it.
        for foreign in [
            "acme.thing",
            "Tool.permission_denied",
            "toolbox.x",
            "tool.",
            "tool",
            "",
            "internal", // a bare namespace names no error
        ] {
            assert_eq!(
                ErrorEnvelope::new(foreign, "m", false).code(),
                INTERNAL_CODE,
                "{foreign} must not be forwarded as a JARVIS code",
            );
        }
    }

    #[test]
    fn every_owned_namespace_survives_the_constructor() {
        // The other direction, and the one the earlier boundary got wrong: a boundary that
        // recognized only `jarvis.` would rewrite all of these to `jarvis.internal`,
        // collapsing codes a client is told to branch on. The table is compared against
        // `CODE_NAMESPACES` as a set so a namespace added to the const without a sample
        // here — or a sample for a namespace the const does not carry — is a failure.
        let samples: [(&str, &str); 16] = [
            ("jarvis", "jarvis.db_open"),
            ("tool", "tool.permission_denied"),
            ("approval", "approval.required"),
            ("model", "model.policy_not_found"),
            ("run", "run.failed"),
            ("stream", "stream.overrun"),
            ("storage", "storage.transition_refused"),
            ("event", "event.envelope_invalid"),
            ("session", "session.expired"),
            ("request", "request.invalid"),
            ("api", "api.version_unsupported"),
            ("auth", "auth.credential_rejected"),
            ("resource", "resource.not_found"),
            ("idempotency", "idempotency.conflict"),
            ("service", "service.not_ready"),
            ("internal", "internal.failure"),
        ];
        let mut claimed: Vec<&str> = samples.iter().map(|(namespace, _)| *namespace).collect();
        claimed.sort_unstable();
        let mut declared: Vec<&str> = CODE_NAMESPACES.to_vec();
        declared.sort_unstable();
        assert_eq!(
            claimed, declared,
            "every declared namespace needs a sample here and every sample needs a declared \
             namespace",
        );
        for (namespace, sample) in samples {
            assert_eq!(
                ErrorEnvelope::new(sample, "m", false).code(),
                sample,
                "the {namespace} namespace must survive the constructor",
            );
        }
    }
}
