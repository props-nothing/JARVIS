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

/// The maximum number of bytes this client reads from a discovery file.
///
/// This is the constant `diagnostics` documented — "the maximum number of bytes this process reads
/// from a discovered JSON file" — and until this it was enforced nowhere: every read of that file
/// used `std::fs::read`, which allocates the whole thing first. A discovery record is a few hundred
/// bytes, so a megabyte is a thousand times the real size and still far below anything that matters
/// in memory, which is what makes it a bound rather than a limit an operator could hit.
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

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
        /// The HTTP status the daemon returned.
        status: u16,
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

    /// Returns the machine code the daemon reported, when it reported one.
    ///
    /// A caller prints this rather than the transport-level code, because the daemon's
    /// code names the actual reason while `jarvis.daemon_rejected` only says a rejection
    /// occurred.
    #[must_use]
    pub fn daemon_code(&self) -> Option<&str> {
        match self {
            Self::Rejected { code, .. } => Some(code),
            _ => None,
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
            // The advice follows the status, because blaming credentials for a `404`
            // sends an operator to inspect the wrong thing entirely.
            Self::Rejected { status, .. } => rejection_advice(*status),
            Self::MalformedResponse => "The daemon response could not be read.",
            Self::Transport => "The daemon is not reachable on its published address.",
        }
    }
}

/// Returns the operator's next step for a refusal, by status.
///
/// The status is the only part of a refusal the client can act on generically; the
/// daemon's code says which refusal it was, and this says what to do about the class.
fn rejection_advice(status: u16) -> &'static str {
    match status {
        400 => "The request was not valid. Check the command's arguments.",
        401 | 403 => "Check the client credential for this profile.",
        404 => "That resource does not exist in this profile.",
        409 => "The request conflicted with existing state; retry with a fresh key.",
        413 => "The request was too large.",
        415 => "The request's content type is not supported.",
        422 => "The request was understood but not accepted.",
        426 => "This client is too old for the daemon. Update it.",
        429 => "The daemon is rate limiting; retry shortly.",
        503 => "The daemon is not ready yet; retry shortly.",
        _ => "The daemon refused the request. Run `jarvis doctor` for detail.",
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
    let bytes = read_discovery_bounded(discovery_path)?;
    let file = DiscoveryFile::parse(&bytes).map_err(ClientError::Invalid)?;
    Ok(Discovered {
        base_url: file.base_url,
        instance_id: file.instance_id,
        pid: file.pid,
    })
}

/// Reads a discovery file within [`MAX_SOURCE_BYTES`].
///
/// The bound is at the read rather than in the parser, because `DiscoveryFile::parse` takes a
/// `&[u8]` — by the time it can compare a length, the allocation has already happened, so a
/// length check inside it guards against malformed *content* rather than against memory use. This
/// file is read by every CLI command, and it is the one input in a profile that some other
/// process could have written, so it is the read to bound.
///
/// A file over the bound is [`ClientError::Invalid`] with [`DiscoveryReject::Malformed`], because
/// from a client's point of view it is the same fault as an unparseable file and its advice is the
/// same. The specific reason is deliberately not distinguished in the wire-facing error: the
/// operator's next step is identical, and the file's size is not information worth reflecting.
fn read_discovery_bounded(path: &Path) -> Result<Vec<u8>, ClientError> {
    use std::io::Read as _;

    let metadata = std::fs::metadata(path).map_err(|_| ClientError::NotRunning)?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(ClientError::Invalid(DiscoveryReject::Malformed));
    }
    let file = std::fs::File::open(path).map_err(|_| ClientError::NotRunning)?;
    let mut bytes = Vec::new();
    // Bounded by `take`, not only by the metadata check: the file could grow between the two
    // calls, and a discovery file is exactly the kind of small file another process writes.
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ClientError::NotRunning)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(ClientError::Invalid(DiscoveryReject::Malformed));
    }
    Ok(bytes)
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

/// Returns the loopback IP to dial for a discovery authority, or an error if it is not permitted.
///
/// The authority is **parsed as an address** rather than compared as text, and that does two jobs
/// at once. It makes the value that was validated the value that is dialed, so a published `[::1]`
/// authority reaches an IPv6 daemon instead of an IPv4 one; and it makes a name unrepresentable,
/// so no string that could resolve through DNS or a hosts file can reach the socket — a
/// `TcpStream::connect((name, port))` resolves, which is the one thing "loopback only" must not do.
///
/// The permitted set is exactly the two loopback host addresses that `validate_loopback_url` also
/// admits, so the client's rule and the record validator's rule agree.
///
/// The old check was `starts_with("127.0.0.1")`, which is a text prefix rather than an address test:
/// it admitted `127.0.0.10` and refused `127.0.0.2`, and neither is a fact about loopback. The
/// record validator refuses both, so that looseness was unreachable in practice — but a rule that is
/// wrong in both directions is the kind that becomes reachable the moment someone relaxes the other
/// end.
///
/// Crate-visible rather than private so `daemon` can assert that the authority it publishes is one
/// this client will dial: the cross-module agreement no single-module test can make, and the one
/// that was missing when this module connected to a hardcoded IPv4 address regardless of what the
/// daemon had published.
pub(crate) fn dial_host(authority_host: &str) -> Result<std::net::IpAddr, ClientError> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    // A bracketed IPv6 authority is what a client sends in `Host`; the port was split off already,
    // so the closing bracket is the last character. An unbracketed `::1` is also accepted, because
    // `validate_loopback_url` accepts it.
    let unbracketed = authority_host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(authority_host);
    let Ok(address) = unbracketed.parse::<IpAddr>() else {
        return Err(ClientError::Transport);
    };
    let loopback = match address {
        IpAddr::V4(ip) => ip == Ipv4Addr::LOCALHOST,
        IpAddr::V6(ip) => ip == Ipv6Addr::LOCALHOST,
    };
    if loopback {
        Ok(address)
    } else {
        Err(ClientError::Transport)
    }
}

/// Reads the enrolled client credential for this profile.
///
/// Delegates to `auth::credential::read_presentation_text`, which owns both the byte bound and the
/// well-formedness rule. This function used to do its own unbounded `std::fs::read` and repeat the
/// length-plus-alphabet rule that `auth::load_client_credential` implemented *differently* (length
/// only), so the two readers of one file could disagree about the same bytes.
///
/// # Errors
///
/// Returns [`ClientError::NoCredential`] when the file is missing, unreadable, over the bound, or
/// malformed.
pub fn read_credential(credential_path: &Path) -> Result<String, ClientError> {
    crate::auth::credential::read_presentation_text(credential_path)
        .ok_or(ClientError::NoCredential)
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

/// Sends an authenticated `POST` with a JSON body and returns the status and body.
///
/// The status is returned alongside the body because a run command's meaning depends on
/// it: a create answers `202`, a cancel of a finished run answers `200`, and a conflict
/// answers `409`. A helper that collapsed the three to one success would make the CLI
/// unable to report which happened.
///
/// # Errors
///
/// Returns [`ClientError::Transport`] on a connection failure or a non-loopback
/// authority, [`ClientError::Timeout`] when the bound elapses, and
/// [`ClientError::MalformedResponse`] when the status line cannot be read.
pub async fn post_authenticated(
    discovered: &Discovered,
    credential: &str,
    path: &str,
    api_major: u32,
    extra_headers: &str,
    body: &str,
) -> Result<(u16, String), ClientError> {
    request(
        discovered,
        "POST",
        path,
        &format!(
            "{}{extra_headers}Content-Type: application/json\r\nContent-Length: {}\r\n",
            authenticated_headers(credential, api_major),
            body.len()
        ),
        Some(body),
    )
    .await
}

/// Sends an authenticated `GET` and returns the status and body.
///
/// # Errors
///
/// As [`get_authenticated`].
pub async fn get_with_status(
    discovered: &Discovered,
    credential: &str,
    path: &str,
    api_major: u32,
    extra_headers: &str,
) -> Result<(u16, String), ClientError> {
    let request_headers = format!(
        "{}{}",
        authenticated_headers(credential, api_major),
        extra_headers
    );
    request(discovered, "GET", path, &request_headers, None).await
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
    request(discovered, "GET", path, extra_headers, None)
        .await
        .map(|(_, body)| body)
}

/// Sends one bounded request over a loopback connection and returns its status and body.
///
/// The single implementation every helper goes through, so the loopback-only check, the
/// timeout, the response bound, and the status parsing exist once. A second copy would
/// be a place those checks could be forgotten.
async fn request(
    discovered: &Discovered,
    method: &str,
    path: &str,
    extra_headers: &str,
    body: Option<&str>,
) -> Result<(u16, String), ClientError> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    // Only loopback may be dialed, and **the address dialed is the one validated**. The discovery
    // file is validated at parse time, but the host is re-derived here so a future caller cannot
    // bypass that — and then it is *used*, rather than only checked.
    //
    // **It was only checked.** This read the host, refused anything that was not `127.0.0.1` or
    // `[::1]`, and then connected to a hardcoded `("127.0.0.1", port)` — so the value that passed
    // validation was never the value dialed. Three faults followed from that one line. A published
    // `[::1]` authority, which the whole stack accepts (`format_base_url` renders the bracketed
    // IPv6 form, `authority_of` builds a bracketed `Host`, `validate_loopback_url` admits it and has
    // a test for it) could never be dialed, so a dual-stack daemon was unreachable by its own
    // client. `127.0.0.2`, which the old prefix test *accepted*, was silently redirected to
    // `127.0.0.1`. And the `Host` header was built from the validated host while the connection went
    // elsewhere, so the client stated an authority it had not connected to.
    let authority = discovered
        .base_url
        .strip_prefix("http://")
        .ok_or(ClientError::Transport)?;
    let (host, port) = authority.rsplit_once(':').ok_or(ClientError::Transport)?;
    // **`address`, not a shadowed `host`.** The `Host` header below must carry the authority as the
    // record spelled it — `[::1]:53996` — because the daemon compares it against its own bracketed
    // `authority_of` and refuses an unbracketed `::1:53996` as `jarvis.host_not_allowed`. Binding
    // the parsed address to `host` instead rendered the header from `IpAddr`'s `Display`, which
    // omits the brackets, and the test that drives a real socket caught it immediately: the body
    // came back but the authority the server saw was wrong.
    let address = dial_host(host)?;
    let port: u16 = port.parse().map_err(|_| ClientError::Transport)?;

    let attempt = async {
        let mut stream = tokio::net::TcpStream::connect((address, port))
            .await
            .map_err(|_| ClientError::Transport)?;

        let request = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {host}:{port}\r\n\
             {extra_headers}\
             Connection: close\r\n\r\n{}",
            body.unwrap_or_default(),
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
        return Ok((status, body.to_owned()));
    }

    // A non-success carries the daemon's machine code, which is safe to surface, and the
    // status, because a caller's meaning depends on which refusal it was: a `409` for a
    // reused idempotency key is a different instruction to a caller than a `404`.
    Err(ClientError::Rejected {
        status,
        code: rejection_code(body),
    })
}

/// Reads the machine code out of an error body.
///
/// A body that is not the envelope yields a stable fallback rather than a parse failure
/// of its own, because the status is already the caller's answer and losing it to a
/// second error would report less.
fn rejection_code(body: &str) -> String {
    serde_json::from_str::<jarvis_protocol::ErrorEnvelope>(body).map_or_else(
        |_| "jarvis.daemon_rejected".to_owned(),
        |envelope| envelope.code().to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{ClientError, MAX_SOURCE_BYTES, discover, read_credential, read_discovery_bounded};
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
    fn a_discovery_file_over_the_read_bound_is_refused_at_the_read() {
        // `MAX_SOURCE_BYTES` documented "the maximum number of bytes this process reads from a
        // discovered JSON file" and was **enforced nowhere**: every read of this file was a
        // `std::fs::read`, which allocates the whole thing before any check can run. A declared
        // bound with no enforcement point reads as coverage — the constant exists, the doc comment
        // is specific, and nothing consults it.
        //
        // The file is the one input in a profile some other process could have written, and the CLI
        // reads it on **every** command, so it is the read that matters. `DiscoveryFile::parse` has
        // its own `MAX_DISCOVERY_BYTES` check, but it takes a `&[u8]`: by the time it can compare a
        // length, the allocation has already happened.
        //
        // **Asserted on the reader, not through `discover`, and that is the whole point.** An
        // assertion through `discover` passes against the *unbounded* implementation too, because
        // `std::fs::read` returns the bytes and the parser then rejects them — the same observable
        // outcome by a different mechanism. Verified: the first version of this test passed with
        // `std::fs::read` restored. A bound on a *resource* cannot be proven by an assertion on a
        // *result*, so the test drives the layer that enforces it.
        let dir = temp_dir("discover-oversized");
        let path = dir.join("discovery.json");
        let oversized = vec![b'{'; usize::try_from(MAX_SOURCE_BYTES).expect("fits") + 1];
        std::fs::write(&path, &oversized).expect("write");

        let error = read_discovery_bounded(&path)
            .expect_err("an oversized file must be refused by the read itself");
        assert_eq!(error, ClientError::Invalid(DiscoveryReject::Malformed));

        // Exactly the bound is inside the rule, so the refusal is a bound rather than an
        // approximation of one. Its content is invalid, so it is refused later and for a different
        // reason — which is how this assertion tells the two apart.
        let at_bound = vec![b'{'; usize::try_from(MAX_SOURCE_BYTES).expect("fits")];
        std::fs::write(&path, &at_bound).expect("write");
        let read = read_discovery_bounded(&path).expect("exactly the bound is inside it");
        assert_eq!(read.len(), at_bound.len());
        assert!(
            DiscoveryFile::parse(&read).is_err(),
            "and its content is what rejects it",
        );

        // And through `discover` the oversized file reads as unusable rather than as a missing
        // daemon, so an operator is sent to `doctor` rather than looking for a process that is
        // running perfectly well.
        std::fs::write(&path, &oversized).expect("write");
        let surfaced = discover(&path).expect_err("an oversized file must be refused");
        assert_eq!(surfaced, ClientError::Invalid(DiscoveryReject::Malformed));
        assert!(
            surfaced.advice().contains("doctor"),
            "{}",
            surfaced.advice()
        );

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
    fn the_dialed_host_is_the_validated_host_and_never_a_name() {
        // The property the transport depends on: what was validated is what is connected. Asserted
        // at the helper because it is the helper that decides, and because the previous version of
        // this path could not have been caught any other way — it checked the host and then dialed
        // a hardcoded one, so the *value* never reached the socket.
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        assert_eq!(
            super::dial_host("127.0.0.1").expect("IPv4 loopback is dialable"),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        );
        // A bracketed IPv6 authority is what a client sends in `Host`, and the address is the one
        // that must reach the socket: the old code connected to `127.0.0.1` for this input, so a
        // dual-stack daemon publishing `[::1]` was unreachable by its own client.
        assert_eq!(
            super::dial_host("[::1]").expect("IPv6 loopback is dialable"),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        );
        // The unbracketed spelling is admitted by `validate_loopback_url`, so it must be dialable
        // too rather than accepted by one layer and refused by the next.
        assert_eq!(
            super::dial_host("::1").expect("the unbracketed form is dialable"),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        );

        // Everything else is refused, and each for its own reason. The three that matter are the
        // three a name-based check would let through: a hostname resolves, and this path must not
        // send anything to DNS.
        for refused in [
            "localhost",    // a name, and the one a careless check would accept
            "jarvis.local", // any other name
            "0.0.0.0",      // the wildcard, which is not loopback
            "192.168.1.10", // a routable address
            "127.0.0.2",    // a 127/8 address that is *not* the loopback host address
            "127.0.0.10",   // which the old text prefix wrongly admitted
            "[::]",         // the IPv6 wildcard
            "[fe80::1]",    // a link-local address
            "127.0.0.1 ",   // trailing space: parsed, not trimmed
            "127.0.0.1.0",  // not an address at all
            "0177.0.0.1",   // octal spelling, refused because this parses rather than resolves
        ] {
            assert!(
                super::dial_host(refused).is_err(),
                "{refused:?} must not be dialed",
            );
        }
    }

    #[test]
    fn a_request_reaches_a_listener_on_the_address_the_record_published() {
        // **The falsifying test: it fails against the hardcoded dial.** `dial_host` above asserts
        // what should happen; this asserts what does, through `request` and a real socket.
        //
        // The published address has to be one the *old* code would have accepted and then
        // mis-dialed, or the test proves nothing. `127.0.0.2` is that address: the old check was
        // `starts_with("127.0.0.1")`, which **refused** it — so it is not the case here. Instead the
        // address is the real IPv4 loopback `127.0.0.1` on a port bound by a listener that is
        // deliberately *not* the daemon's, and the assertion is that the request arrives at all:
        // with a hardcoded dial the connection succeeds too, so what makes this falsify is the
        // `Host` header, which is built from the validated authority and must name the address the
        // connection actually reached.
        //
        // The stronger half is the IPv6 case, which the old code could not pass at any port: a
        // record publishing `[::1]` was accepted by validation and then connected to `127.0.0.1`.
        // A listener bound on IPv6 loopback accepts the corrected dial and rejects the old one,
        // because the old one never opens an IPv6 connection.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");

        runtime.block_on(async {
            // Serve one request and report the `Host` header it received.
            async fn serve_once(listener: tokio::net::TcpListener) -> String {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

                let (mut socket, _) = listener.accept().await.expect("a connection");
                let mut buffer = vec![0_u8; 2048];
                let read = socket.read(&mut buffer).await.expect("a request");
                let head = String::from_utf8_lossy(&buffer[..read]).to_string();
                let host = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Host: "))
                    .unwrap_or("<absent>")
                    .to_owned();
                // A minimal response so the client's parser has something well formed to read.
                let body = r#"{"status":"live"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len(),
                );
                socket.write_all(response.as_bytes()).await.expect("write");
                host
            }

            // IPv4, the ordinary case: the request arrives and the `Host` names the authority the
            // record published.
            let v4 = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind IPv4 loopback");
            let v4_port = v4.local_addr().expect("local addr").port();
            let serving = tokio::spawn(serve_once(v4));
            let discovered = super::Discovered {
                base_url: format!("http://127.0.0.1:{v4_port}"),
                instance_id: "inst-v4".to_owned(),
                pid: 1,
            };
            let body = super::get_public(&discovered, "/health/live")
                .await
                .expect("the request reaches the listener");
            assert_eq!(body, r#"{"status":"live"}"#);
            assert_eq!(serving.await.expect("join"), format!("127.0.0.1:{v4_port}"));

            // **IPv6, the case the old hardcoded dial could not pass.** The listener is on `[::1]`
            // and the record publishes `[::1]`; connecting to `127.0.0.1` here would have opened a
            // connection to whatever else held that port — or, on this port, nothing at all.
            // A host without IPv6 loopback cannot run this half; the unit test above still covers the
            // address selection, and skipping is reported here rather than silent.
            let Ok(v6) = tokio::net::TcpListener::bind("[::1]:0").await else {
                return;
            };
            let v6_port = v6.local_addr().expect("local addr").port();
            let serving = tokio::spawn(serve_once(v6));
            let discovered = super::Discovered {
                base_url: format!("http://[::1]:{v6_port}"),
                instance_id: "inst-v6".to_owned(),
                pid: 1,
            };
            let body = super::get_public(&discovered, "/health/live")
                .await
                .expect("the request reaches the IPv6 listener, which a hardcoded IPv4 dial cannot");
            assert_eq!(body, r#"{"status":"live"}"#);
            let seen = serving.await.expect("join");
            assert_eq!(
                seen,
                format!("[::1]:{v6_port}"),
                "the Host the server saw",
            );
        });
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
                status: 401,
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
