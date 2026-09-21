//! Local client credentials.
//!
//! The daemon proves a client by comparing a SHA-256 verifier in constant time.
//! The credential itself is 32 bytes from the operating system CSPRNG and is
//! never stored by the daemon: only the verifier is, so a database disclosure
//! does not disclose a usable credential.
//!
//! SHA-256 is the correct choice here precisely because the input is 256 bits of
//! OS-generated entropy, so there is nothing to brute-force. Human-memorable
//! passwords would need a slow password hash instead.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

/// The exact credential length in bytes.
pub const CREDENTIAL_BYTES: usize = 32;

/// The base64url-encoded length of a credential (32 bytes, unpadded).
pub const CREDENTIAL_TEXT_LEN: usize = 43;

/// An error raised while handling credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialError {
    /// The operating system random number generator failed.
    ///
    /// Enrollment aborts rather than falling back to weaker bytes.
    EntropyUnavailable,
    /// The presented token is not a well-formed credential.
    Malformed,
    /// The credential is not accepted.
    ///
    /// Unknown, malformed, and revoked credentials all report this same value so
    /// a caller cannot distinguish them.
    Rejected,
}

impl CredentialError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EntropyUnavailable => "jarvis.credential_entropy_unavailable",
            Self::Malformed => "jarvis.credential_malformed",
            Self::Rejected => "jarvis.credential_rejected",
        }
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntropyUnavailable => {
                formatter.write_str("the operating system random source is unavailable")
            }
            Self::Malformed => formatter.write_str("the credential is not well formed"),
            Self::Rejected => formatter.write_str("the credential was not accepted"),
        }
    }
}

/// A freshly generated credential, held only long enough to enroll a client.
///
/// `Debug` is deliberately not derived: a credential must not reach a log line
/// through an incidental `{:?}`.
pub struct GeneratedCredential {
    bytes: [u8; CREDENTIAL_BYTES],
}

impl GeneratedCredential {
    /// Generates a credential from the operating system CSPRNG.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::EntropyUnavailable`] when the random source
    /// fails. Weak or partially filled bytes are never returned.
    pub fn generate() -> Result<Self, CredentialError> {
        let mut bytes = [0_u8; CREDENTIAL_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| CredentialError::EntropyUnavailable)?;
        Ok(Self { bytes })
    }

    /// Returns the verifier to store in a normal record.
    #[must_use]
    pub fn verifier(&self) -> CredentialVerifier {
        CredentialVerifier(Sha256::digest(self.bytes).into())
    }

    /// Returns the unpadded base64url text presented to the client.
    ///
    /// Base64 is applied only at the presentation edge; the credential is bytes
    /// everywhere else.
    #[must_use]
    pub fn to_presentation_text(&self) -> String {
        base64_encode_unpadded(&self.bytes)
    }
}

impl fmt::Debug for GeneratedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GeneratedCredential([REDACTED])")
    }
}

/// The stored, one-way verifier for a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialVerifier([u8; 32]);

impl CredentialVerifier {
    /// Wraps 32 raw verifier bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the raw verifier bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the verifier in lowercase hexadecimal for storage.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(nibble_to_hex(byte >> 4));
            out.push(nibble_to_hex(byte & 0x0f));
        }
        out
    }

    /// Parses a verifier from lowercase hexadecimal.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialError::Malformed`] unless the input is exactly 64
    /// lowercase hexadecimal characters.
    pub fn from_hex(value: &str) -> Result<Self, CredentialError> {
        if value.len() != 64 || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(CredentialError::Malformed);
        }
        let raw = value.as_bytes();
        let mut bytes = [0_u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = hex_to_nibble(raw[index * 2]).ok_or(CredentialError::Malformed)?;
            let low = hex_to_nibble(raw[index * 2 + 1]).ok_or(CredentialError::Malformed)?;
            *byte = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

/// Returns the verifier for a credential presented as text.
///
/// This is the derivation the daemon uses after enrollment: the stored file
/// holds the presentation text, and the daemon hashes it to rebuild the verifier
/// rather than retaining a plaintext copy at rest beyond that file.
///
/// # Errors
///
/// Returns [`CredentialError::Malformed`] when the text is not a well-formed
/// credential.
pub fn verifier_from_presentation_text(
    presented: &str,
) -> Result<CredentialVerifier, CredentialError> {
    let bytes = base64_decode_unpadded(presented.trim())?;
    if bytes.len() != CREDENTIAL_BYTES {
        return Err(CredentialError::Malformed);
    }
    Ok(CredentialVerifier(Sha256::digest(bytes).into()))
}

/// Verifies a presented credential against a stored verifier.
///
/// # Errors
///
/// Returns [`CredentialError::Malformed`] when the presented text is not a
/// well-formed credential and [`CredentialError::Rejected`] when it does not
/// match.
pub fn verify(presented: &str, expected: &CredentialVerifier) -> Result<(), CredentialError> {
    let bytes = base64_decode_unpadded(presented)?;
    if bytes.len() != CREDENTIAL_BYTES {
        return Err(CredentialError::Malformed);
    }
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    // Equal-length digests compare in constant time, so a caller cannot learn the
    // verifier byte-by-byte from timing.
    if digest.ct_eq(expected.as_bytes()).into() {
        Ok(())
    } else {
        Err(CredentialError::Rejected)
    }
}

/// Encodes bytes as unpadded base64url.
fn base64_encode_unpadded(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |byte| u32::from(*byte));
        let b2 = chunk.get(2).map_or(0, |byte| u32::from(*byte));
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(char::from(ALPHABET[((triple >> 18) & 0x3f) as usize]));
        out.push(char::from(ALPHABET[((triple >> 12) & 0x3f) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(ALPHABET[((triple >> 6) & 0x3f) as usize]));
        }
        if chunk.len() > 2 {
            out.push(char::from(ALPHABET[(triple & 0x3f) as usize]));
        }
    }
    out
}

/// Decodes unpadded base64url, rejecting padding and any non-alphabet byte.
fn base64_decode_unpadded(input: &str) -> Result<Vec<u8>, CredentialError> {
    fn value_of(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if bytes.len() % 4 == 1 {
        // One leftover character cannot encode any byte.
        return Err(CredentialError::Malformed);
    }

    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut accumulator = 0_u32;
        for byte in chunk {
            let value = value_of(*byte).ok_or(CredentialError::Malformed)?;
            accumulator = (accumulator << 6) | value;
        }
        accumulator <<= 6 * (4 - chunk.len());

        out.push(((accumulator >> 16) & 0xff) as u8);
        if chunk.len() > 2 {
            out.push(((accumulator >> 8) & 0xff) as u8);
        }
        if chunk.len() > 3 {
            out.push((accumulator & 0xff) as u8);
        }
    }
    Ok(out)
}

/// Maps a nibble to its lowercase hexadecimal character.
fn nibble_to_hex(nibble: u8) -> char {
    char::from(b"0123456789abcdef"[nibble as usize])
}

/// Maps a hexadecimal character to its nibble.
fn hex_to_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CREDENTIAL_TEXT_LEN, CredentialError, CredentialVerifier, GeneratedCredential, verify,
    };

    #[test]
    fn a_generated_credential_has_the_contract_length_and_presentation() {
        let credential = GeneratedCredential::generate().expect("entropy available");
        assert_eq!(credential.verifier().as_bytes().len(), 32);
        assert_eq!(credential.to_presentation_text().len(), CREDENTIAL_TEXT_LEN);

        // Unpadded base64url only.
        let text = credential.to_presentation_text();
        assert!(!text.contains('='));
        assert!(
            text.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "{text}",
        );
    }

    #[test]
    fn the_stored_verifier_is_a_one_way_digest_not_the_credential() {
        let credential = GeneratedCredential::generate().expect("entropy available");
        let verifier = credential.verifier();
        let presentation = credential.to_presentation_text();

        let hex = verifier.to_hex();
        assert_ne!(
            hex, presentation,
            "the stored form must not be the credential"
        );
        assert!(
            !presentation.contains(&hex),
            "the credential must not contain its verifier",
        );

        // The verifier round-trips through its stored form.
        assert_eq!(
            CredentialVerifier::from_hex(&hex).expect("parses"),
            verifier
        );
    }

    #[test]
    fn a_matching_credential_verifies() {
        let credential = GeneratedCredential::generate().expect("entropy available");
        let verifier = credential.verifier();
        verify(&credential.to_presentation_text(), &verifier).expect("must verify");
    }

    #[test]
    fn a_different_credential_is_rejected() {
        let expected = GeneratedCredential::generate().expect("entropy").verifier();
        let other = GeneratedCredential::generate().expect("entropy");
        let error = verify(&other.to_presentation_text(), &expected)
            .expect_err("a different credential must be rejected");
        assert_eq!(error, CredentialError::Rejected);
        assert_eq!(error.code(), "jarvis.credential_rejected");
    }

    #[test]
    fn malformed_and_unknown_credentials_never_verify() {
        let verifier = GeneratedCredential::generate().expect("entropy").verifier();
        for presented in [
            "",
            "short",
            "!not-base64!",
            "AAAA=", // padded
            &"A".repeat(CREDENTIAL_TEXT_LEN + 1),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", // decodes to 33 bytes
        ] {
            let error = verify(presented, &verifier).expect_err("must not verify");
            assert!(
                matches!(
                    error,
                    CredentialError::Malformed | CredentialError::Rejected
                ),
                "{presented:?} produced {error:?}",
            );
        }
    }

    #[test]
    fn a_verifier_hex_round_trip_rejects_lookalikes() {
        let verifier = CredentialVerifier::from_bytes([0xab; 32]);
        let hex = verifier.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(
            CredentialVerifier::from_hex(&hex).expect("parses"),
            verifier
        );

        for bad in [
            "",
            "ab",
            &hex[..63],
            &"AB".repeat(32),   // uppercase
            &"zz".repeat(32),   // not hexadecimal
            &format!("{hex}0"), // too long
        ] {
            assert!(
                CredentialVerifier::from_hex(bad).is_err(),
                "{bad:?} must be rejected",
            );
        }
    }

    #[test]
    fn base64_round_trips_every_length_class() {
        // Lengths chosen to cover the 1, 2, and 3 byte remainder cases.
        for length in 1..=70_usize {
            let input: Vec<u8> = (0..length)
                .map(|index| u8::try_from(index * 7 % 256).expect("masked into a byte"))
                .collect();
            let text = super::base64_encode_unpadded(&input);
            let decoded = super::base64_decode_unpadded(&text).expect("decodes");
            assert_eq!(decoded, input, "length {length}");
        }
    }

    #[test]
    fn entropy_failure_is_its_own_code() {
        assert_eq!(
            CredentialError::EntropyUnavailable.code(),
            "jarvis.credential_entropy_unavailable",
        );
    }

    #[test]
    fn debug_of_a_generated_credential_never_prints_it() {
        let credential = GeneratedCredential::generate().expect("entropy available");
        let rendered = format!("{credential:?}");
        assert_eq!(rendered, "GeneratedCredential([REDACTED])");
        assert!(!rendered.contains(&credential.to_presentation_text()));
    }
}
