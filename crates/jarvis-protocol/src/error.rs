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

/// The error object inside [`ErrorEnvelope`].
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
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            error: ErrorResponse {
                code: code.into(),
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
    use super::ErrorEnvelope;

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
}
