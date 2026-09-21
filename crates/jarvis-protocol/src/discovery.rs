//! The daemon discovery file contract.
//!
//! The daemon publishes this file after binding and before reporting readiness
//! (see `docs/contracts/local-control-api.md`). A client reads it to learn the
//! loopback authority to call.
//!
//! Two properties are security-relevant and are enforced here rather than left
//! to a convention:
//!
//! * the file contains **no** credential, secret reference, database path, or
//!   provider configuration;
//! * `base_url` must be plain `http` on a numeric loopback host, so a hostile or
//!   corrupted file cannot redirect a client to another host or scheme.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The discovery file schema version this build writes.
pub const DISCOVERY_SCHEMA_VERSION: u32 = 1;

/// The maximum accepted discovery file size.
///
/// A discovery file is a small fixed record. Bounding it means a hostile or
/// corrupted file cannot be used to exhaust memory before it is rejected.
pub const MAX_DISCOVERY_BYTES: usize = 4 * 1024;

/// The published discovery record for one running daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryFile {
    /// The discovery schema version.
    pub schema_version: u32,
    /// A per-process identifier. Shutdown removes only a file whose
    /// `instance_id` still belongs to that daemon.
    pub instance_id: String,
    /// The operating-system process id. Never proof of identity on its own.
    pub pid: u32,
    /// The loopback base URL, for example `http://127.0.0.1:43127`.
    pub base_url: String,
    /// The API major version the daemon serves.
    pub api_major: u32,
    /// The instant the daemon started, RFC 3339 UTC with `Z`.
    pub started_at: String,
}

/// A reason a discovery record was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryReject {
    /// The document is not valid JSON or is over the size bound.
    Malformed,
    /// The schema version is not one this client understands.
    UnsupportedVersion {
        /// The version found in the file.
        found: u32,
    },
    /// `base_url` is not a permitted loopback authority.
    UnsafeAuthority,
    /// A required field is empty or otherwise not usable.
    EmptyField,
}

impl DiscoveryReject {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Malformed => "jarvis.discovery_malformed",
            Self::UnsupportedVersion { .. } => "jarvis.discovery_version_unsupported",
            Self::UnsafeAuthority => "jarvis.discovery_unsafe_authority",
            Self::EmptyField => "jarvis.discovery_empty_field",
        }
    }
}

impl fmt::Display for DiscoveryReject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("the discovery file is malformed or too large"),
            Self::UnsupportedVersion { found } => {
                write!(formatter, "discovery schema version {found} is unsupported")
            }
            Self::UnsafeAuthority => {
                formatter.write_str("the discovery base URL is not a permitted loopback authority")
            }
            Self::EmptyField => formatter.write_str("a required discovery field is empty"),
        }
    }
}

impl DiscoveryFile {
    /// Parses and validates a discovery document.
    ///
    /// # Errors
    ///
    /// Returns the [`DiscoveryReject`] describing the first problem found. A
    /// rejection never echoes file contents back, because the file is untrusted
    /// input and echoing it could inject into a log line.
    pub fn parse(bytes: &[u8]) -> Result<Self, DiscoveryReject> {
        if bytes.len() > MAX_DISCOVERY_BYTES {
            return Err(DiscoveryReject::Malformed);
        }
        let file: Self = serde_json::from_slice(bytes).map_err(|_| DiscoveryReject::Malformed)?;
        file.validate()?;
        Ok(file)
    }

    /// Validates the fields a client depends on.
    ///
    /// # Errors
    ///
    /// Returns the [`DiscoveryReject`] describing the first problem found.
    pub fn validate(&self) -> Result<(), DiscoveryReject> {
        if self.schema_version != DISCOVERY_SCHEMA_VERSION {
            return Err(DiscoveryReject::UnsupportedVersion {
                found: self.schema_version,
            });
        }
        if self.instance_id.is_empty() || self.started_at.is_empty() {
            return Err(DiscoveryReject::EmptyField);
        }
        if !validate_loopback_url(&self.base_url) {
            return Err(DiscoveryReject::UnsafeAuthority);
        }
        Ok(())
    }

    /// Serializes the record to JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns an error only if serialization fails, which is a programming
    /// error for this shape.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Returns whether `authority` is a permitted numeric loopback authority.
///
/// Anything other than plain `http` on `127.0.0.1` or `[::1]` is refused. User
/// info, a path, a query, a fragment, a hostname, or a different scheme could all
/// redirect a client to a host JARVIS does not control.
#[must_use]
pub fn validate_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    if rest.contains('@') || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return false;
    }

    // Split on the *last* colon: an IPv6 authority contains colons inside the
    // brackets, so splitting on the first one would mis-parse `[::1]:43127`.
    let authority = match rest.rsplit_once(':') {
        Some((host, port)) => {
            if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return false;
            }
            let Ok(port) = port.parse::<u32>() else {
                return false;
            };
            if port == 0 || port > u16::MAX.into() {
                return false;
            }
            host
        }
        None => return false,
    };

    authority == "127.0.0.1" || authority == "[::1]" || authority == "::1"
}

#[cfg(test)]
mod tests {
    use super::{DISCOVERY_SCHEMA_VERSION, DiscoveryFile, DiscoveryReject, validate_loopback_url};

    fn sample() -> DiscoveryFile {
        DiscoveryFile {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            instance_id: "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            pid: 14240,
            base_url: "http://127.0.0.1:43127".to_owned(),
            api_major: 1,
            started_at: "2026-09-20T12:34:56Z".to_owned(),
        }
    }

    #[test]
    fn a_valid_record_round_trips() {
        let record = sample();
        let bytes = record.to_bytes().expect("serializes");
        let parsed = DiscoveryFile::parse(&bytes).expect("parses");
        assert_eq!(parsed, record);
    }

    #[test]
    fn the_contract_shape_is_stable() {
        let json = String::from_utf8(sample().to_bytes().expect("serializes")).expect("utf8");
        assert!(json.contains(r#""schema_version":1"#));
        assert!(json.contains(r#""base_url":"http://127.0.0.1:43127""#));
        assert!(json.contains(r#""api_major":1"#));
        // No credential, secret reference, database path, or provider field.
        for forbidden in [
            "credential",
            "token",
            "secret",
            "password",
            "database",
            "provider",
        ] {
            assert!(
                !json.to_ascii_lowercase().contains(forbidden),
                "the discovery file must not carry {forbidden}",
            );
        }
    }

    #[test]
    fn a_non_loopback_or_unsafe_authority_is_rejected() {
        let mut record = sample();
        for unsafe_url in [
            "http://0.0.0.0:43127",
            "http://192.168.1.10:43127",
            "http://localhost:43127",
            "https://127.0.0.1:43127",
            "http://user:pw@127.0.0.1:43127",
            "http://127.0.0.1:43127/api",
            "http://127.0.0.1:43127?x=1",
            "http://127.0.0.1:43127#frag",
            "http://127.0.0.1",
            "http://127.0.0.1:",
            "http://127.0.0.1:notaport",
            "http://127.0.0.1:0",
            "http://127.0.0.1:70000",
            "ws://127.0.0.1:43127",
            "",
        ] {
            record.base_url = unsafe_url.to_owned();
            let error = record.validate().expect_err("must be rejected");
            assert_eq!(error, DiscoveryReject::UnsafeAuthority, "{unsafe_url}");
            assert_eq!(error.code(), "jarvis.discovery_unsafe_authority");
        }
    }

    #[test]
    fn a_valid_ipv6_loopback_authority_is_accepted() {
        assert!(validate_loopback_url("http://[::1]:43127"));
        assert!(validate_loopback_url("http://::1:43127"));
    }

    #[test]
    fn an_unsupported_version_is_rejected_before_other_fields() {
        let mut record = sample();
        record.schema_version = 2;
        let error = record.validate().expect_err("must be rejected");
        assert_eq!(error, DiscoveryReject::UnsupportedVersion { found: 2 });
    }

    #[test]
    fn empty_required_fields_are_rejected() {
        let mut record = sample();
        record.instance_id = String::new();
        assert_eq!(
            record.validate().expect_err("must be rejected"),
            DiscoveryReject::EmptyField,
        );

        let mut record = sample();
        record.started_at = String::new();
        assert_eq!(
            record.validate().expect_err("must be rejected"),
            DiscoveryReject::EmptyField,
        );
    }

    #[test]
    fn malformed_and_oversized_documents_are_rejected() {
        assert_eq!(
            DiscoveryFile::parse(b"not json").expect_err("must be rejected"),
            DiscoveryReject::Malformed,
        );
        // An unknown field is refused rather than ignored.
        let with_extra = br#"{"schema_version":1,"instance_id":"a","pid":1,
            "base_url":"http://127.0.0.1:1","api_major":1,
            "started_at":"2026-09-20T12:34:56Z","extra":"x"}"#;
        assert_eq!(
            DiscoveryFile::parse(with_extra).expect_err("must be rejected"),
            DiscoveryReject::Malformed,
        );

        let huge = vec![b'{'; super::MAX_DISCOVERY_BYTES + 1];
        assert_eq!(
            DiscoveryFile::parse(&huge).expect_err("must be rejected"),
            DiscoveryReject::Malformed,
        );
    }

    #[test]
    fn rejections_do_not_echo_file_contents() {
        let hostile = br#"{"schema_version":1,"instance_id":"a","pid":1,
            "base_url":"http://evil.example:1","api_major":1,
            "started_at":"2026-09-20T12:34:56Z"}"#;
        let error = DiscoveryFile::parse(hostile).expect_err("must be rejected");
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("evil.example"), "no echo: {rendered}");
    }
}
