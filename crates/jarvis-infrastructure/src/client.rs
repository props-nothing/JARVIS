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

/// What answered at a published daemon address.
///
/// An unreachable address and an address held by a *different* process are
/// different faults with different operator actions, and collapsing them into one
/// boolean is what makes a port conflict look like a daemon that simply is not
/// running. Both are seedable in `ACC-003`, and the second is the one a naive
/// reachability check reports as a healthy daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonLiveness {
    /// The daemon answered its own liveness route.
    Live,
    /// Nothing is listening on the published address.
    NothingListening,
    /// Something is listening, but it is not this daemon: it did not answer the
    /// liveness route with the contract's token.
    ForeignListener,
}

impl DaemonLiveness {
    /// Returns the stable code for diagnostics.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Live => "jarvis.daemon_live",
            Self::NothingListening => "jarvis.daemon_unreachable",
            Self::ForeignListener => "jarvis.port_conflict",
        }
    }
}

/// Probes a published daemon address for liveness, without authentication.
///
/// This exists because a discovery file is **not** evidence that a daemon is
/// alive. It survives an unclean kill and still parses, so anything that reasons
/// from the file alone reports a dead daemon as running. The lock cannot be used
/// either: it is released by the operating system on process exit, so a free lock
/// plus a published discovery file is exactly the stale state, not a live daemon.
///
/// The probe is `GET /health/live`, which the local control API already defines as
/// unauthenticated and which returns only a status token. That matters here: a
/// diagnostic must not need the credential to answer "is something listening?",
/// and it must not learn anything but that.
///
/// # Errors
///
/// Returns the [`ClientError`] for the failed exchange. A refused connection maps
/// to [`DaemonLiveness::NothingListening`] and a wrong body to
/// [`DaemonLiveness::ForeignListener`]; a transport failure that is neither (a
/// timeout, an unparseable response) is reported as an error because it cannot be
/// attributed to either fault.
pub async fn probe_liveness(discovered: &Discovered) -> Result<DaemonLiveness, ClientError> {
    match get_public(discovered, "/health/live").await {
        Ok(body) => {
            if body.trim() == r#"{"status":"live"}"# {
                Ok(DaemonLiveness::Live)
            } else {
                // A listener that answers with something else is holding the
                // published address without being the daemon.
                Ok(DaemonLiveness::ForeignListener)
            }
        }
        // A refused connection is the ordinary "nothing there" case.
        Err(ClientError::Transport) => Ok(DaemonLiveness::NothingListening),
        Err(error) => Err(error),
    }
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
    let request = authenticated_headers(credential, api_major);
    get_with_headers(discovered, path, &request).await
}

/// Builds the headers for an authenticated request.
///
/// Kept as a named function so the test can assert the credential is present here
/// and absent in [`public_headers`], which is what makes the two paths different
/// by construction rather than by which call site remembers to pass what.
#[must_use]
fn authenticated_headers(credential: &str, api_major: u32) -> String {
    format!(
        "Authorization: Bearer {credential}\r\n\
         Jarvis-API-Version: {api_major}\r\n"
    )
}

/// Builds the headers for an unauthenticated request, which are none.
///
/// The local control API exposes exactly two unauthenticated routes, and both
/// return a status token and nothing else. An empty header block is the structural
/// guarantee that a probe cannot carry a credential.
#[must_use]
const fn public_headers() -> &'static str {
    ""
}

/// Fetches an unauthenticated endpoint and returns the body.
///
/// This is deliberately a separate function from [`get_authenticated`] rather than
/// that function with an empty credential: the absence of an `Authorization`
/// header has to be structural, so a caller cannot accidentally send the owner's
/// credential to a route that does not require it.
///
/// # Errors
///
/// Returns the same [`ClientError`] class as [`get_authenticated`].
pub async fn get_public(discovered: &Discovered, path: &str) -> Result<String, ClientError> {
    get_with_headers(discovered, path, public_headers()).await
}

/// Performs one bounded loopback exchange and returns the response body.
///
/// # Errors
///
/// Returns [`ClientError::Transport`] on a connection failure or when the
/// published authority is not numeric loopback, [`ClientError::Timeout`] when the
/// bound elapses, and [`ClientError::MalformedResponse`] when the status line
/// cannot be read.
async fn get_with_headers(
    discovered: &Discovered,
    path: &str,
    extra_headers: &str,
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
             {extra_headers}\
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

    #[tokio::test]
    async fn a_refused_liveness_probe_reports_nothing_listening() {
        // The property the `daemon` check depends on. A discovery file that parses
        // is not a running daemon, so the probe must report the absence on an
        // address where nothing is listening — and this binds nothing, which is the
        // point: a port nobody holds is the honest way to test "no daemon here".
        let discovered = super::Discovered {
            // Port 1 on loopback is not bound by an unprivileged process.
            base_url: "http://127.0.0.1:1".to_owned(),
            instance_id: "inst".to_owned(),
            pid: 1,
        };
        assert_eq!(
            super::probe_liveness(&discovered).await.expect("probe ran"),
            super::DaemonLiveness::NothingListening
        );
    }

    #[test]
    fn the_public_transport_sends_no_authorization_header() {
        // The liveness route is unauthenticated, and the absence of the header has
        // to be structural rather than a caller's choice: a diagnostic must not
        // hand the owner's credential to a route that does not require it.
        let authenticated = super::authenticated_headers("secret-credential", 1);
        assert!(authenticated.contains("Authorization: Bearer secret-credential"));
        assert!(super::public_headers().is_empty());
        assert!(!super::public_headers().contains("Authorization"));
    }

    #[test]
    fn every_liveness_state_has_a_distinct_code() {
        // A port conflict reported with the same code as "not running" is
        // unsearchable and gives the wrong advice, so the codes must differ.
        let codes = [
            super::DaemonLiveness::Live.code(),
            super::DaemonLiveness::NothingListening.code(),
            super::DaemonLiveness::ForeignListener.code(),
        ];
        let mut unique = codes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len(), "{codes:?}");
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
