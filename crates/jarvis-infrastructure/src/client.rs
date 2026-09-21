//! Reading a running daemon's discovery file and authenticated status.
//!
//! The CLI is a client: it discovers the daemon through the published file and
//! authenticates with the enrolled local credential. It never reads the database
//! directly for normal product commands, because the daemon is the authority.

use std::path::Path;
use std::time::Duration;

use jarvis_protocol::{DiscoveryFile, DiscoveryReject};

/// The bounded time a CLI waits for a daemon response.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The maximum response body the CLI will read.
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// A discovered, running daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The base URL to call.
    pub base_url: String,
    /// The daemon instance identifier.
    pub instance_id: String,
    /// The daemon process id, for diagnostics only.
    pub pid: u32,
}

/// A reason a daemon could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// No discovery file exists, so no daemon is running for this profile.
    NotRunning,
    /// The discovery file is present but not usable.
    Invalid(DiscoveryReject),
    /// The credential file is missing or unusable.
    NoCredential,
    /// The daemon did not answer within the bound.
    Timeout,
    /// The daemon answered with an error.
    Rejected {
        /// The machine code the daemon returned.
        code: String,
    },
    /// The response could not be read or parsed.
    MalformedResponse,
    /// A transport-level failure.
    Transport,
}

impl ClientError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotRunning => "jarvis.daemon_not_running",
            Self::Invalid(_) => "jarvis.discovery_invalid",
            Self::NoCredential => "jarvis.credential_missing",
            Self::Timeout => "jarvis.daemon_timeout",
            Self::Rejected { .. } => "jarvis.daemon_rejected",
            Self::MalformedResponse => "jarvis.response_malformed",
            Self::Transport => "jarvis.transport_failed",
        }
    }

    /// Returns an actionable, non-secret message for the operator.
    #[must_use]
    pub fn advice(&self) -> &'static str {
        match self {
            Self::NotRunning => "Start the daemon with `jarvisd`, then retry.",
            Self::Invalid(_) => {
                "The discovery file is present but unusable. Run `jarvis doctor` for detail."
            }
            Self::NoCredential => {
                "This profile has no enrolled client. Run `jarvisd` once to enroll the owner."
            }
            Self::Timeout => "The daemon did not answer in time. Check `jarvis doctor`.",
            Self::Rejected { .. } => {
                "The daemon refused the request. Check the client credential and API version."
            }
            Self::MalformedResponse => "The daemon response could not be read.",
            Self::Transport => "The daemon is not reachable on its published address.",
        }
    }
}

/// Reads and validates the discovery file.
///
/// # Errors
///
/// Returns [`ClientError::NotRunning`] when the file is absent and
/// [`ClientError::Invalid`] when it is not usable. The two are distinct so the
/// operator gets the right next step.
pub fn discover(discovery_path: &Path) -> Result<Discovered, ClientError> {
    if !discovery_path.exists() {
        return Err(ClientError::NotRunning);
    }
    let bytes = std::fs::read(discovery_path).map_err(|_| ClientError::NotRunning)?;
    let file = DiscoveryFile::parse(&bytes).map_err(ClientError::Invalid)?;
    Ok(Discovered {
        base_url: file.base_url,
        instance_id: file.instance_id,
        pid: file.pid,
    })
}

/// Reads the enrolled client credential for this profile.
///
/// # Errors
///
/// Returns [`ClientError::NoCredential`] when the file is missing or malformed.
pub fn read_credential(credential_path: &Path) -> Result<String, ClientError> {
    let bytes = std::fs::read(credential_path).map_err(|_| ClientError::NoCredential)?;
    let text = String::from_utf8(bytes).map_err(|_| ClientError::NoCredential)?;
    let trimmed = text.trim();
    // The credential is a fixed-length unpadded base64url string.
    if trimmed.len() != 43
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(ClientError::NoCredential);
    }
    Ok(trimmed.to_owned())
}

/// Fetches an authenticated endpoint and returns the body.
///
/// This is a deliberately minimal HTTP/1.0 client: the daemon is a single local
/// loopback peer, so a full client stack would add a dependency without adding
/// capability. The request never puts the credential in the URL, and it is sent
/// only to the numeric loopback authority the discovery file names.
///
/// # Errors
///
/// Returns [`ClientError::Transport`] on a connection failure,
/// [`ClientError::Timeout`] when the bound elapses, and
/// [`ClientError::MalformedResponse`] when the status line cannot be read.
pub async fn get_authenticated(
    discovered: &Discovered,
    credential: &str,
    path: &str,
    api_major: u32,
) -> Result<String, ClientError> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    // Only a numeric loopback authority is ever dialed; the discovery file was
    // validated at parse time, but the address is re-derived here so a future
    // caller cannot bypass that.
    let authority = discovered
        .base_url
        .strip_prefix("http://")
        .ok_or(ClientError::Transport)?;
    let (host, port) = authority.rsplit_once(':').ok_or(ClientError::Transport)?;
    if !host.starts_with("127.0.0.1") && host != "[::1]" {
        return Err(ClientError::Transport);
    }
    let port: u16 = port.parse().map_err(|_| ClientError::Transport)?;

    let attempt = async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|_| ClientError::Transport)?;

        let request = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: {host}:{port}\r\n\
             Authorization: Bearer {credential}\r\n\
             Jarvis-API-Version: {api_major}\r\n\
             Connection: close\r\n\r\n",
        );
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|_| ClientError::Transport)?;

        let mut response = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream
                .read(&mut buffer)
                .await
                .map_err(|_| ClientError::Transport)?;
            if read == 0 {
                break;
            }
            response.extend_from_slice(&buffer[..read]);
            if response.len() > MAX_RESPONSE_BYTES {
                break;
            }
        }
        Ok(response)
    };

    let response = tokio::time::timeout(CLIENT_TIMEOUT, attempt)
        .await
        .map_err(|_| ClientError::Timeout)??;

    let text = String::from_utf8(response).map_err(|_| ClientError::MalformedResponse)?;
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));

    let status_line = head.lines().next().ok_or(ClientError::MalformedResponse)?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or(ClientError::MalformedResponse)?;

    if (200..300).contains(&status) {
        return Ok(body.to_owned());
    }

    // An error body carries the daemon's machine code, which is safe to surface.
    let code = serde_json::from_str::<jarvis_protocol::ErrorEnvelope>(body).map_or_else(
        |_| "jarvis.daemon_rejected".to_owned(),
        |envelope| envelope.code().to_owned(),
    );
    Err(ClientError::Rejected { code })
}

#[cfg(test)]
mod tests {
    use super::{ClientError, discover, read_credential};
    use jarvis_protocol::{DISCOVERY_SCHEMA_VERSION, DiscoveryFile, DiscoveryReject};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-fnd008-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_published_discovery_file_is_read() {
        let dir = temp_dir("discover");
        let path = dir.join("discovery.json");
        let record = DiscoveryFile {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            instance_id: "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            pid: 14240,
            base_url: "http://127.0.0.1:43127".to_owned(),
            api_major: 1,
            started_at: "2026-09-21T00:00:00Z".to_owned(),
        };
        std::fs::write(&path, record.to_bytes().expect("serialize")).expect("write");

        let found = discover(&path).expect("discovery succeeds");
        assert_eq!(found.base_url, "http://127.0.0.1:43127");
        assert_eq!(found.pid, 14240);
        assert_eq!(found.instance_id, record.instance_id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_discovery_file_reports_not_running() {
        let dir = temp_dir("absent");
        let error = discover(&dir.join("discovery.json")).expect_err("must be not running");
        assert_eq!(error, ClientError::NotRunning);
        assert_eq!(error.code(), "jarvis.daemon_not_running");
        assert!(error.advice().contains("jarvisd"), "{}", error.advice());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_invalid_discovery_file_is_distinct_from_not_running() {
        let dir = temp_dir("invalid");
        let path = dir.join("discovery.json");
        std::fs::write(&path, b"{not json").expect("write");

        let error = discover(&path).expect_err("must be invalid");
        assert_eq!(error, ClientError::Invalid(DiscoveryReject::Malformed));
        assert_eq!(error.code(), "jarvis.discovery_invalid");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unsafe_authority_in_the_file_is_refused() {
        let dir = temp_dir("unsafe");
        let path = dir.join("discovery.json");
        let record = DiscoveryFile {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            instance_id: "0195f4e8-7f6a-7c21-8ab5-4f0f80fd7e09".to_owned(),
            pid: 1,
            base_url: "http://evil.example:1".to_owned(),
            api_major: 1,
            started_at: "2026-09-21T00:00:00Z".to_owned(),
        };
        std::fs::write(&path, record.to_bytes().expect("serialize")).expect("write");

        let error = discover(&path).expect_err("must be refused");
        assert_eq!(error.code(), "jarvis.discovery_invalid");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_well_formed_credential_is_read() {
        let dir = temp_dir("credential");
        let path = dir.join("client-credential");
        // 43 characters of the unpadded base64url alphabet.
        let credential = "A".repeat(43);
        std::fs::write(&path, &credential).expect("write");

        assert_eq!(read_credential(&path).expect("read"), credential);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_malformed_credentials_are_reported() {
        let dir = temp_dir("credential-bad");
        let path = dir.join("client-credential");

        assert_eq!(
            read_credential(&path).expect_err("missing"),
            ClientError::NoCredential,
        );

        for bad in ["tooshort", &"A".repeat(44), &format!("{}!", "A".repeat(42))] {
            std::fs::write(&path, bad).expect("write");
            assert_eq!(
                read_credential(&path).expect_err("malformed"),
                ClientError::NoCredential,
                "{bad:?}",
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_error_has_a_code_and_actionable_advice() {
        let errors = [
            ClientError::NotRunning,
            ClientError::Invalid(DiscoveryReject::Malformed),
            ClientError::NoCredential,
            ClientError::Timeout,
            ClientError::Rejected {
                code: "auth.credential_rejected".to_owned(),
            },
            ClientError::MalformedResponse,
            ClientError::Transport,
        ];
        for error in errors {
            assert!(error.code().starts_with("jarvis."), "{:?}", error.code());
            assert!(!error.advice().is_empty(), "{error:?}");
        }
    }
}
