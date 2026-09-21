//! Secret redaction and control-character sanitization for log output.
//!
//! Redaction has two layers with different strengths, and they must not be
//! confused:
//!
//! 1. **Registered-value redaction is the guarantee.** A secret whose exact
//!    value is registered is removed wherever it appears, including inside a
//!    larger string. The canary test proves this layer.
//! 2. **Pattern redaction is best-effort defense in depth.** It catches common
//!    credential shapes (an authorization header, a `key=value` pair, URL
//!    userinfo) that a caller logged by accident. It is not a proof, because no
//!    pattern can enumerate every way a secret might appear.
//!
//! Neither layer replaces the rule that secrets are never accepted as ordinary
//! fields in the first place.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::RwLock;

/// The marker substituted for redacted material.
pub const REDACTED: &str = "[REDACTED]";

/// The shortest registered value that is eligible for redaction.
///
/// Redacting very short values would corrupt unrelated output, so a short value
/// is refused at registration instead of silently mangling every log line.
pub const MIN_SECRET_LEN: usize = 8;

/// Key names whose values are treated as credential material by the best-effort
/// pattern pass. Matching is case-insensitive.
const CREDENTIAL_KEYS: &[&str] = &[
    "authorization",
    "api_key",
    "api-key",
    "apikey",
    "access_token",
    "refresh_token",
    "client_secret",
    "token",
    "secret",
    "password",
    "passwd",
];

/// The longest single line the redactor will scan.
///
/// A very long line is truncated before scanning so a hostile payload cannot
/// make redaction itself a denial-of-service vector.
const MAX_SCAN_LEN: usize = 64 * 1024;

/// Replaces registered secret values and credential-shaped substrings.
///
/// The type is cheap to share and is intended to live behind an `Arc` for the
/// lifetime of the process.
#[derive(Default)]
pub struct Redactor {
    /// Registered values, kept sorted so scans are deterministic.
    values: RwLock<BTreeSet<String>>,
    /// Whether the best-effort pattern pass is enabled.
    patterns_enabled: bool,
}

impl Redactor {
    /// Creates a redactor with the best-effort pattern pass enabled.
    ///
    /// # Errors
    ///
    /// Returns [`RedactorError::Unavailable`] if the internal lock cannot be
    /// taken, which only happens if a thread panicked while holding it.
    pub fn new() -> Result<Self, RedactorError> {
        Self::with_patterns(true)
    }

    /// Creates a redactor, choosing whether the pattern pass runs.
    ///
    /// # Errors
    ///
    /// Returns [`RedactorError::Unavailable`] if the internal lock cannot be
    /// taken.
    pub fn with_patterns(patterns_enabled: bool) -> Result<Self, RedactorError> {
        let redactor = Self {
            values: RwLock::new(BTreeSet::new()),
            patterns_enabled,
        };
        // Fail fast rather than deferring a poisoned-lock surprise to first use.
        drop(
            redactor
                .values
                .read()
                .map_err(|_| RedactorError::Unavailable)?,
        );
        Ok(redactor)
    }

    /// Registers a secret value for exact-match redaction.
    ///
    /// # Errors
    ///
    /// Returns [`RedactorError::ValueTooShort`] when `value` is shorter than
    /// [`MIN_SECRET_LEN`], because redacting it would corrupt unrelated output.
    /// Returns [`RedactorError::Unavailable`] if the lock cannot be taken.
    pub fn register(&self, value: &str) -> Result<(), RedactorError> {
        if value.len() < MIN_SECRET_LEN {
            return Err(RedactorError::ValueTooShort);
        }
        self.values
            .write()
            .map_err(|_| RedactorError::Unavailable)?
            .insert(value.to_owned());
        Ok(())
    }

    /// Returns how many values are registered, for diagnostics.
    ///
    /// The count is not sensitive and is useful to answer "did redaction get
    /// configured at all", which is a common production failure.
    ///
    /// # Errors
    ///
    /// Returns [`RedactorError::Unavailable`] if the lock cannot be taken.
    pub fn registered_count(&self) -> Result<usize, RedactorError> {
        self.values
            .read()
            .map(|values| values.len())
            .map_err(|_| RedactorError::Unavailable)
    }

    /// Redacts one line of text that does **not** contain its trailing newline.
    ///
    /// Registered values are replaced first, then credential-shaped substrings,
    /// then control characters are escaped so an untrusted value cannot forge a
    /// second log line or inject a terminal escape sequence.
    #[must_use]
    pub fn redact_line(&self, line: &str) -> String {
        let bounded = if line.len() > MAX_SCAN_LEN {
            // Truncate on a character boundary so the result is valid UTF-8.
            let mut end = MAX_SCAN_LEN;
            while end > 0 && !line.is_char_boundary(end) {
                end -= 1;
            }
            &line[..end]
        } else {
            line
        };

        let mut text = bounded.to_owned();
        if let Ok(values) = self.values.read() {
            for value in values.iter() {
                if text.contains(value.as_str()) {
                    text = text.replace(value.as_str(), REDACTED);
                }
            }
        }
        if self.patterns_enabled {
            text = redact_url_userinfo(&text);
            text = redact_authorization(&text);
            text = redact_credential_pairs(&text);
        }
        sanitize_control_chars(&text)
    }
}

impl fmt::Debug for Redactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render registered values, even in a debug dump of the redactor.
        // The registered set is intentionally omitted rather than printed.
        let count = self.registered_count().unwrap_or(0);
        formatter
            .debug_struct("Redactor")
            .field("registered_values", &count)
            .field("patterns_enabled", &self.patterns_enabled)
            .finish_non_exhaustive()
    }
}

/// An error raised while configuring or using redaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RedactorError {
    /// The value is too short to redact safely.
    #[error("the value is too short to redact without corrupting unrelated output")]
    ValueTooShort,
    /// The redactor's internal state could not be read.
    #[error("the redactor is unavailable")]
    Unavailable,
}

impl RedactorError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ValueTooShort => "jarvis.redactor_value_too_short",
            Self::Unavailable => "jarvis.redactor_unavailable",
        }
    }
}

/// Escapes control characters so a value cannot forge output.
///
/// `\n`, `\r`, and `\t` are rendered as visible escapes; other control
/// characters, including the ANSI escape introducer, are removed. Newline
/// removal is what stops a logged value from impersonating a second log record.
#[must_use]
pub fn sanitize_control_chars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

/// Returns whether `needle` matches `haystack` at `at`, ignoring ASCII case.
///
/// Uses `.get()` so a position that is not a character boundary simply does not
/// match, which keeps multi-byte input safe without a panic.
fn matches_at(haystack: &str, at: usize, needle: &str) -> bool {
    haystack
        .get(at..at + needle.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
}

/// Returns whether `byte` may appear inside a redactable value run.
fn is_value_byte(byte: u8) -> bool {
    !byte.is_ascii_whitespace() && !matches!(byte, b'"' | b'\'' | b',' | b'}' | b')' | b']' | b';')
}

/// Returns whether `byte` is a word byte for boundary checks.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Redacts the password portion of `scheme://user:password@host`.
fn redact_url_userinfo(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut emitted = 0;
    let mut cursor = 0;

    while let Some(relative) = input.get(cursor..).and_then(|rest| rest.find("://")) {
        let scheme_at = cursor + relative;
        let authority_start = scheme_at + 3;
        // Only userinfo when a colon and an `@` appear before any path segment.
        let authority_end = input
            .get(authority_start..)
            .and_then(|rest| rest.find(['@', '/']))
            .map(|at| at + authority_start);
        let Some(at_sign) = authority_end.filter(|at| input.as_bytes()[*at] == b'@') else {
            cursor = authority_start;
            continue;
        };
        let authority = input.get(authority_start..at_sign).unwrap_or_default();
        if !authority.contains(':') {
            cursor = authority_start;
            continue;
        }

        out.push_str(&input[emitted..authority_start]);
        out.push_str(REDACTED);
        out.push('@');
        emitted = at_sign + 1;
        cursor = at_sign + 1;
    }

    out.push_str(&input[emitted..]);
    out
}

/// Redacts a token that follows an authorization scheme keyword.
fn redact_authorization(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut emitted = 0;
    let mut at = 0;

    while at < bytes.len() {
        let scheme_len = ["bearer", "basic"]
            .into_iter()
            .find(|scheme| matches_at(input, at, scheme))
            .map(str::len);
        let Some(scheme_len) = scheme_len else {
            at += 1;
            continue;
        };
        let before_ok = at == 0 || !is_word_byte(bytes[at - 1]);
        let value_start = at + scheme_len;
        if !before_ok || value_start >= bytes.len() || !bytes[value_start].is_ascii_whitespace() {
            at += 1;
            continue;
        }

        let mut start = value_start;
        while start < bytes.len() && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && is_value_byte(bytes[end]) {
            end += 1;
        }

        if end.saturating_sub(start) >= MIN_SECRET_LEN {
            out.push_str(&input[emitted..start]);
            out.push_str(REDACTED);
            emitted = end;
            at = end;
        } else {
            // Keep scanning from after the keyword; nothing is emitted yet.
            at = start.max(value_start);
        }
    }

    out.push_str(&input[emitted..]);
    out
}

/// Redacts the value of a credential-named `key = value` or `key: value` pair.
///
/// The scan tracks two independent positions: `emitted`, how much input has been
/// copied out, and `at`, where the search currently is. Conflating them would
/// drop text whenever a candidate key is rejected.
fn redact_credential_pairs(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut emitted = 0;
    let mut at = 0;

    while at < bytes.len() {
        let key_len = CREDENTIAL_KEYS
            .iter()
            .find(|key| matches_at(input, at, key))
            .map(|key| key.len());
        let Some(key_len) = key_len else {
            at += 1;
            continue;
        };
        let before_ok = at == 0 || !is_word_byte(bytes[at - 1]);
        let after_key = at + key_len;
        let after_ok = after_key >= bytes.len() || !is_word_byte(bytes[after_key]);
        if !before_ok || !after_ok {
            at += 1;
            continue;
        }

        // Skip an opening quote and spacing, then require a separator.
        let mut index = after_key;
        while index < bytes.len() && matches!(bytes[index], b'"' | b'\'' | b' ') {
            index += 1;
        }
        if index >= bytes.len() || !matches!(bytes[index], b'=' | b':') {
            at = after_key;
            continue;
        }
        index += 1;
        while index < bytes.len() && matches!(bytes[index], b'"' | b'\'' | b' ') {
            index += 1;
        }

        let mut end = index;
        while end < bytes.len() && is_value_byte(bytes[end]) {
            end += 1;
        }

        if end.saturating_sub(index) >= MIN_SECRET_LEN {
            out.push_str(&input[emitted..index]);
            out.push_str(REDACTED);
            emitted = end;
            at = end;
        } else {
            // Resume inside the value so a later key can still be found.
            at = index;
        }
    }

    out.push_str(&input[emitted..]);
    out
}

#[cfg(test)]
mod tests {
    use super::{REDACTED, Redactor, RedactorError, matches_at, sanitize_control_chars};

    fn redactor() -> Redactor {
        Redactor::new().expect("redactor must construct")
    }

    #[test]
    fn a_registered_value_is_removed_wherever_it_appears() {
        let redactor = redactor();
        let canary = "canary-9f3c1a7d2b";
        redactor.register(canary).expect("canary is long enough");

        for line in [
            format!("token={canary}"),
            format!("prefix {canary} suffix"),
            format!("{{\"api_key\":\"{canary}\"}}"),
            canary.to_owned(),
        ] {
            let redacted = redactor.redact_line(&line);
            assert!(!redacted.contains(canary), "leaked in {line:?}");
            assert!(
                redacted.contains(REDACTED),
                "marker missing in {redacted:?}"
            );
        }
    }

    #[test]
    fn a_short_value_is_refused_rather_than_over_redacting() {
        let redactor = redactor();
        let error = redactor
            .register("abc")
            .expect_err("a 3-byte value must be refused");
        assert_eq!(error, RedactorError::ValueTooShort);
        assert_eq!(error.code(), "jarvis.redactor_value_too_short");
        assert_eq!(redactor.registered_count().expect("countable"), 0);

        // The refused value must not have been used to mangle output.
        assert_eq!(redactor.redact_line("abcdef"), "abcdef");
    }

    #[test]
    fn debug_of_the_redactor_never_prints_a_value() {
        let redactor = redactor();
        let canary = "canary-11aa22bb33";
        redactor.register(canary).expect("canary is long enough");

        let rendered = format!("{redactor:?}");
        assert!(!rendered.contains(canary));
        assert!(rendered.contains("registered_values"), "{rendered}");
    }

    #[test]
    fn an_authorization_header_is_redacted() {
        let redactor = redactor();
        for line in [
            "Authorization: Bearer abcdefghijklmnop",
            "authorization: bearer abcdefghijklmnop",
            "header=Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==",
        ] {
            let redacted = redactor.redact_line(line);
            assert!(redacted.contains(REDACTED), "{line} -> {redacted}");
            assert!(
                !redacted.contains("abcdefghijklmnop") && !redacted.contains("QWxhZGRpbjpvcGVu"),
                "{line} -> {redacted}",
            );
        }
    }

    #[test]
    fn a_credential_pair_is_redacted() {
        let redactor = redactor();
        for line in [
            "api_key=abcdef123456",
            "api-key: abcdef123456",
            "password=\"hunter2hunter2\"",
            "{\"client_secret\":\"abcdef123456\"}",
            "access_token=abcdef123456&other=1",
        ] {
            let redacted = redactor.redact_line(line);
            assert!(
                !redacted.contains("abcdef123456") && !redacted.contains("hunter2hunter2"),
                "{line} -> {redacted}",
            );
        }
    }

    #[test]
    fn url_userinfo_password_is_redacted() {
        let redactor = redactor();
        let redacted =
            redactor.redact_line("connecting to postgres://jarvis:s3cretpw@db.local:5432/jarvis");
        assert!(redacted.contains(REDACTED), "{redacted}");
        assert!(!redacted.contains("s3cretpw"), "{redacted}");
        assert!(
            redacted.contains("db.local:5432"),
            "host must survive: {redacted}"
        );
    }

    #[test]
    fn a_credential_name_without_a_value_shape_is_left_alone() {
        let redactor = redactor();
        // Nothing is redacted, but the text must not be corrupted either.
        for line in [
            "token budget exhausted",
            "the secret is unavailable",
            "password policy requires 12 characters",
        ] {
            assert_eq!(redactor.redact_line(line), line);
        }
    }

    #[test]
    fn newlines_are_escaped_so_a_value_cannot_forge_a_log_record() {
        let redactor = redactor();
        let redacted = redactor.redact_line("first\nsecond\r\ttabbed");
        assert!(!redacted.contains('\n'), "{redacted:?}");
        assert!(!redacted.contains('\r'), "{redacted:?}");
        assert!(redacted.contains("\\n"), "{redacted:?}");
        assert!(redacted.contains("\\r"), "{redacted:?}");
        assert!(redacted.contains("\\t"), "{redacted:?}");
    }

    #[test]
    fn control_characters_and_terminal_escapes_are_removed() {
        assert_eq!(sanitize_control_chars("a\u{1b}[31mred\u{7}b"), "a[31mredb");
        assert_eq!(sanitize_control_chars("bell\u{7}"), "bell");
        assert_eq!(sanitize_control_chars("plain text"), "plain text");
        // Unicode text beyond ASCII must survive untouched.
        assert_eq!(sanitize_control_chars("café — 日本語"), "café — 日本語");
    }

    #[test]
    fn a_pattern_cannot_be_disabled_accidentally_when_configured_on() {
        let redactor = redactor();
        // With patterns on, the header shape is caught even when the exact value
        // was never registered.
        assert!(
            redactor
                .redact_line("Bearer zzzzyyyyxxxx")
                .contains(REDACTED)
        );
    }

    #[test]
    fn a_very_long_line_is_truncated_before_scanning() {
        let redactor = redactor();
        let huge = "x".repeat(200_000);
        let redacted = redactor.redact_line(&huge);
        assert!(redacted.len() <= 64 * 1024 + 64, "was {}", redacted.len());
    }

    #[test]
    fn a_non_matching_keyword_does_not_delete_text() {
        // Regression: an earlier implementation tracked one cursor for both the
        // search position and the copy frontier, so a rejected candidate
        // silently dropped the skipped text.
        let redactor = redactor();
        for line in [
            "token budget exhausted",
            "a token and a secret and a password walk into a bar",
            "Bearer",
            "basic",
            "https://example.com/path with token inside",
        ] {
            assert_eq!(redactor.redact_line(line), line, "text must be preserved");
        }
    }

    #[test]
    fn a_multi_byte_character_is_never_split() {
        let redactor = redactor();
        // A non-ASCII run before a match forces boundary handling.
        let line = "café — 日本語 token=abcdef123456 done";
        let redacted = redactor.redact_line(line);
        assert!(redacted.contains("café — 日本語"), "{redacted}");
        assert!(redacted.contains("done"), "{redacted}");
        assert!(!redacted.contains("abcdef123456"), "{redacted}");
    }

    #[test]
    fn case_insensitive_search_finds_and_misses_correctly() {
        assert!(matches_at("Hello World", 6, "world"));
        assert!(matches_at("Hello World", 6, "WORLD"));
        assert!(!matches_at("Hello", 0, "absent"));
        assert!(!matches_at("ab", 1, "abc"), "past the end is not a match");
        // A position that is not a character boundary must not match or panic.
        assert!(!matches_at("café — 日本語", 4, "x"));
    }
}
