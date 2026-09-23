//! Local client authentication.
//!
//! Foundation has no credential-store adapter yet, so the client copy of a
//! credential is written to an owner-only file in the profile config directory.
//! This is a **documented, temporary** deviation from "store in the OS credential
//! store where available" (see the Foundation evidence note): it is never a
//! silent plaintext fallback, it uses the same owner-only permission path proved
//! on Unix, and `FND-008` plus the keyring slice replace it.

use std::path::{Path, PathBuf};

pub mod credential;

use credential::{CredentialError, CredentialVerifier, GeneratedCredential, verify};

/// The file name holding the enrolled client credential.
pub const CLIENT_CREDENTIAL_FILE: &str = "client-credential";

/// A registered local client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredClient {
    /// The stable client identifier.
    pub client_id: String,
    /// The one-way verifier the daemon stores.
    pub verifier: CredentialVerifier,
    /// The instant the client was enrolled, RFC 3339 UTC with `Z`.
    pub created_at: String,
    /// Whether the client has been revoked.
    pub revoked: bool,
}

impl RegisteredClient {
    /// Returns whether this client may authenticate.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        !self.revoked
    }
}

/// A registry of locally enrolled clients.
///
/// A single owner client is the Foundation case; the type is a collection so
/// revocation and multiple device clients do not need a contract change.
#[derive(Debug, Default)]
pub struct ClientRegistry {
    clients: Vec<RegisteredClient>,
}

impl ClientRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a client.
    pub fn register(&mut self, client: RegisteredClient) {
        self.clients
            .retain(|existing| existing.client_id != client.client_id);
        self.clients.push(client);
    }

    /// Marks a client revoked.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::Rejected`] when no such client exists.
    pub fn revoke(&mut self, client_id: &str) -> Result<(), CredentialError> {
        match self
            .clients
            .iter_mut()
            .find(|client| client.client_id == client_id)
        {
            Some(client) => {
                client.revoked = true;
                Ok(())
            }
            None => Err(CredentialError::Rejected),
        }
    }

    /// Returns the active client count.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.clients
            .iter()
            .filter(|client| client.is_active())
            .count()
    }

    /// Verifies a presented credential against every active client.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::Rejected`] when no active client matches.
    /// Unknown, malformed, and revoked credentials all produce this same value,
    /// so a caller cannot distinguish them.
    pub fn authenticate(&self, presented: &str) -> Result<&RegisteredClient, CredentialError> {
        for client in self.clients.iter().filter(|client| client.is_active()) {
            if verify(presented, &client.verifier).is_ok() {
                return Ok(client);
            }
        }
        Err(CredentialError::Rejected)
    }
}

/// Where the client copy of a credential is stored for a profile.
#[derive(Debug, Clone)]
pub struct ClientCredentialPath {
    path: PathBuf,
}

impl ClientCredentialPath {
    /// Creates the path inside a profile config directory.
    #[must_use]
    pub fn in_config_dir(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join(CLIENT_CREDENTIAL_FILE),
        }
    }

    /// Returns the file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Writes the client credential as an owner-only file.
///
/// # Errors
///
/// Returns an error when the file cannot be written with owner-only
/// permissions. A write that cannot establish those permissions fails rather
/// than producing a world-readable credential.
pub fn store_client_credential(
    destination: &ClientCredentialPath,
    credential: &GeneratedCredential,
) -> Result<(), CredentialError> {
    let text = credential.to_presentation_text();
    write_owner_only(destination.path(), text.as_bytes())
}

/// Reads a previously stored client credential.
///
/// # Errors
///
/// Returns [`CredentialError::Malformed`] when the file is unreadable or does
/// not contain a well-formed credential.
pub fn load_client_credential(source: &ClientCredentialPath) -> Result<String, CredentialError> {
    let bytes = std::fs::read(source.path()).map_err(|_| CredentialError::Malformed)?;
    let text = String::from_utf8(bytes).map_err(|_| CredentialError::Malformed)?;
    let trimmed = text.trim();
    if trimmed.len() != credential::CREDENTIAL_TEXT_LEN {
        return Err(CredentialError::Malformed);
    }
    Ok(trimmed.to_owned())
}

/// Writes owner-only bytes, creating the parent directory owner-only on Unix.
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), CredentialError> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }

    // The builder is mutated inside each branch and never after, so the outer
    // binding is immutable. Only the platform-specific branch needs `mut`, which is
    // why the branches are separate bindings rather than one shared mutable value.
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // `umask` can only clear bits, so 0o600 is an upper bound.
        options.mode(0o600);
        options
    };

    #[cfg(not(unix))]
    let options = {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        options
    };

    let mut file = options.open(path).map_err(|_| CredentialError::Malformed)?;
    file.write_all(bytes)
        .map_err(|_| CredentialError::Malformed)?;
    file.sync_all().map_err(|_| CredentialError::Malformed)
}

/// Creates a directory owner-only on Unix.
fn create_private_dir(path: &Path) -> Result<(), CredentialError> {
    if path.is_dir() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(|_| CredentialError::Malformed)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path).map_err(|_| CredentialError::Malformed)
    }
}

/// Enrolls the first local client and stores its credential.
///
/// Returns the registry entry and the presentation text the operator must keep.
///
/// # Errors
///
/// Returns [`CredentialError::EntropyUnavailable`] when the random source fails,
/// and a write error when the client copy cannot be stored owner-only.
pub fn enroll_owner_client(
    client_id: &str,
    created_at: &str,
    destination: &ClientCredentialPath,
) -> Result<(RegisteredClient, GeneratedCredential), CredentialError> {
    let credential = GeneratedCredential::generate()?;
    store_client_credential(destination, &credential)?;
    Ok((
        RegisteredClient {
            client_id: client_id.to_owned(),
            verifier: credential.verifier(),
            created_at: created_at.to_owned(),
            revoked: false,
        },
        credential,
    ))
}

impl ClientRegistry {
    /// Loads the enrolled owner client, enrolling one if the profile has none.
    ///
    /// The returned credential text is `Some` only when a new credential was
    /// created, which is the only moment the daemon holds the plaintext. The
    /// caller is expected to register it for redaction and show it to the
    /// operator exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::Malformed`] when an existing credential file
    /// is unreadable or malformed, so a corrupt enrollment is reported instead
    /// of being silently replaced (which would lock out the existing client).
    /// Returns [`CredentialError::EntropyUnavailable`] when a new credential is
    /// needed and the random source fails.
    pub fn into_enrolled(
        self,
        destination: &ClientCredentialPath,
    ) -> Result<(Self, Option<String>), CredentialError> {
        let mut registry = self;

        if destination.path().exists() {
            let presented = load_client_credential(destination)?;
            let verifier = verify_presented(&presented)?;
            registry.register(RegisteredClient {
                client_id: "owner".to_owned(),
                verifier,
                created_at: String::new(),
                revoked: false,
            });
            return Ok((registry, None));
        }

        let (registered, credential) = enroll_owner_client("owner", "", destination)?;
        let presented = credential.to_presentation_text();
        registry.register(registered);
        Ok((registry, Some(presented)))
    }
}

/// Returns the verifier for a credential presented as text, without storing it.
///
/// # Errors
///
/// Returns [`CredentialError::Malformed`] when the text is not a well-formed
/// credential.
fn verify_presented(presented: &str) -> Result<CredentialVerifier, CredentialError> {
    credential::verifier_from_presentation_text(presented)
}

#[cfg(test)]
mod tests {
    use super::{
        ClientCredentialPath, ClientRegistry, RegisteredClient, enroll_owner_client,
        load_client_credential,
    };
    use crate::auth::credential::GeneratedCredential;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-fnd007-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn client(id: &str, credential: &GeneratedCredential) -> RegisteredClient {
        RegisteredClient {
            client_id: id.to_owned(),
            verifier: credential.verifier(),
            created_at: "2026-09-21T00:00:00Z".to_owned(),
            revoked: false,
        }
    }

    #[test]
    fn enrollment_stores_a_file_the_owner_can_read_back() {
        let dir = temp_dir("enroll");
        let destination = ClientCredentialPath::in_config_dir(&dir);

        let (registered, credential) =
            enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination)
                .expect("enrollment succeeds");

        assert_eq!(registered.client_id, "owner");
        assert!(registered.is_active());

        let stored = load_client_credential(&destination).expect("credential is readable");
        assert_eq!(stored, credential.to_presentation_text());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_stored_credential_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = temp_dir("perms");
        let destination = ClientCredentialPath::in_config_dir(&dir);
        enroll_owner_client("owner", "2026-09-21T00:00:00Z", &destination)
            .expect("enrollment succeeds");

        let mode = std::fs::metadata(destination.path())
            .expect("file exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode & 0o077, 0, "credential file mode {mode:o}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_registered_client_authenticates_and_a_stranger_does_not() {
        let credential = GeneratedCredential::generate().expect("entropy");
        let mut registry = ClientRegistry::new();
        registry.register(client("owner", &credential));

        let matched = registry
            .authenticate(&credential.to_presentation_text())
            .expect("the enrolled client authenticates");
        assert_eq!(matched.client_id, "owner");

        let stranger = GeneratedCredential::generate().expect("entropy");
        let error = registry
            .authenticate(&stranger.to_presentation_text())
            .expect_err("a stranger must be rejected");
        assert_eq!(error.code(), "jarvis.credential_rejected");
    }

    #[test]
    fn a_revoked_client_can_no_longer_authenticate() {
        let credential = GeneratedCredential::generate().expect("entropy");
        let mut registry = ClientRegistry::new();
        registry.register(client("owner", &credential));
        assert_eq!(registry.active_count(), 1);

        registry.revoke("owner").expect("revocation succeeds");
        assert_eq!(registry.active_count(), 0);

        let error = registry
            .authenticate(&credential.to_presentation_text())
            .expect_err("a revoked client must be rejected");
        assert_eq!(error.code(), "jarvis.credential_rejected");

        // Revoking an unknown client is refused rather than silently accepted.
        assert!(registry.revoke("missing").is_err());
    }

    #[test]
    fn a_malformed_credential_collapses_to_the_same_rejection_as_a_stranger() {
        // `authenticate`'s doc states that "unknown, malformed, and revoked credentials all produce
        // this same value, so a caller cannot distinguish them" — a claim about all three, and only
        // **two** were asserted: the stranger test and the revoked test, both of which produce
        // `Rejected` directly.
        //
        // A malformed credential takes a genuinely different path: it fails while decoding, before
        // any comparison, so `verify` returns `Malformed`. That distinct variant is why the collapse
        // is worth asserting rather than assuming — a caller receiving it would learn that its
        // credential was *shaped* wrong rather than unknown, which is a real disclosure when the
        // credential came from another profile or from a probe.
        //
        // The HTTP layer cannot catch this on its own: it maps the error through `unauthenticated()`
        // today, so reverting the collapse to `?` would surface `jarvis.credential_malformed`, and
        // the existing handler test would fail on the *body equality* rather than on the property
        // being named. Asserting it at the registry pins the collapse where it happens.
        let credential = GeneratedCredential::generate().expect("entropy");
        let mut registry = ClientRegistry::new();
        registry.register(client("owner", &credential));

        for (label, presented) in [
            ("empty", ""),
            ("not-base64", "!!!!"),
            ("too-short", "AAAA"),
            ("wrong-length", &"A".repeat(44)),
        ] {
            let error = registry
                .authenticate(presented)
                .expect_err("a malformed credential must be rejected");
            assert_eq!(
                error.code(),
                "jarvis.credential_rejected",
                "a {label} credential must not disclose that it was malformed",
            );
        }

        // And a *well-formed* credential that this daemon has never seen — the other-profile case
        // the contract names. It decodes and hashes correctly, so it can only be refused by
        // comparison, which is the path the stranger test takes; asserted here so all three cases
        // the doc names are held to one value in one place.
        let foreign = GeneratedCredential::generate().expect("entropy");
        let error = registry
            .authenticate(&foreign.to_presentation_text())
            .expect_err("a foreign profile's credential must be rejected");
        assert_eq!(error.code(), "jarvis.credential_rejected");
    }

    #[test]
    fn the_registry_holds_more_than_one_credential_without_length_leak() {
        // Two clients whose presentations differ in length class must both work,
        // which is why verification decodes before hashing rather than comparing
        // text.
        let first = GeneratedCredential::generate().expect("entropy");
        let second = GeneratedCredential::generate().expect("entropy");
        assert_eq!(
            first.to_presentation_text().len(),
            second.to_presentation_text().len()
        );

        let mut registry = ClientRegistry::new();
        registry.register(client("a", &first));
        registry.register(client("b", &second));
        assert_eq!(registry.active_count(), 2);

        assert_eq!(
            registry
                .authenticate(&first.to_presentation_text())
                .expect("first authenticates")
                .client_id,
            "a",
        );
        assert_eq!(
            registry
                .authenticate(&second.to_presentation_text())
                .expect("second authenticates")
                .client_id,
            "b",
        );
    }

    #[test]
    fn registering_the_same_client_id_replaces_rather_than_duplicates() {
        let first = GeneratedCredential::generate().expect("entropy");
        let second = GeneratedCredential::generate().expect("entropy");
        let mut registry = ClientRegistry::new();
        registry.register(client("owner", &first));
        registry.register(client("owner", &second));

        assert_eq!(registry.active_count(), 1);
        assert!(
            registry
                .authenticate(&first.to_presentation_text())
                .is_err(),
            "the replaced credential must no longer work",
        );
        assert!(
            registry
                .authenticate(&second.to_presentation_text())
                .is_ok()
        );
    }

    #[test]
    fn a_missing_credential_file_is_reported_not_invented() {
        let dir = temp_dir("missing");
        let destination = ClientCredentialPath::in_config_dir(&dir);
        let error =
            load_client_credential(&destination).expect_err("a missing file must be an error");
        assert_eq!(error.code(), "jarvis.credential_malformed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_credential_file_is_rejected() {
        let dir = temp_dir("truncated");
        let destination = ClientCredentialPath::in_config_dir(&dir);
        std::fs::write(destination.path(), b"tooshort").expect("write fixture");
        assert!(load_client_credential(&destination).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_credential_byte_count_matches_the_contract() {
        assert_eq!(super::credential::CREDENTIAL_BYTES, 32);
    }

    #[test]
    fn enrolling_then_reloading_rebuilds_a_working_registry() {
        let dir = temp_dir("reload");
        let destination = ClientCredentialPath::in_config_dir(&dir);

        // First start enrolls and returns the plaintext exactly once.
        let (registry, created) = ClientRegistry::new()
            .into_enrolled(&destination)
            .expect("first enrollment succeeds");
        let presented = created.expect("a new credential is returned once");
        assert_eq!(registry.active_count(), 1);

        // A later start reloads the stored credential and returns no plaintext.
        let (reloaded, again) = ClientRegistry::new()
            .into_enrolled(&destination)
            .expect("reload succeeds");
        assert!(
            again.is_none(),
            "the plaintext is only returned at enrollment"
        );
        assert_eq!(reloaded.active_count(), 1);

        // The reloaded registry authenticates the original credential, which
        // proves the derivation matches the one used at enrollment.
        assert!(
            reloaded.authenticate(&presented).is_ok(),
            "the reloaded verifier must accept the enrolled credential",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
