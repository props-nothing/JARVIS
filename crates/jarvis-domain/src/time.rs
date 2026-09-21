//! Absolute UTC instants.
//!
//! JARVIS stores and transmits absolute time as RFC 3339 UTC strings with a
//! trailing `Z` (see the common contract conventions). [`UtcTimestamp`] is the
//! single value type for that representation.

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::DomainError;

/// An absolute instant in UTC with nanosecond precision.
///
/// The value is normalized to UTC on construction, so it has exactly one string
/// form: RFC 3339 with a `Z` offset. Offset-bearing input is accepted and
/// converted, which makes it impossible for two different-looking strings to
/// denote two different stored values for the same instant.
///
/// JARVIS does not model leap seconds: `jiff` constrains a parsed second of `60`
/// to `59`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcTimestamp(Timestamp);

impl UtcTimestamp {
    /// Wraps an instant that is already normalized to UTC.
    #[must_use]
    pub const fn from_timestamp(timestamp: Timestamp) -> Self {
        Self(timestamp)
    }

    /// Returns the underlying instant.
    #[must_use]
    pub const fn as_timestamp(&self) -> Timestamp {
        self.0
    }

    /// Parses an RFC 3339 / ISO 8601 instant and normalizes it to UTC.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidTimestamp`] when `value` is not a complete,
    /// valid instant. An offset is required; a civil datetime without an offset
    /// is ambiguous and is rejected rather than guessed. Trailing data after a
    /// valid instant is rejected.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let timestamp: Timestamp = value.parse().map_err(|_| DomainError::InvalidTimestamp)?;
        Ok(Self(timestamp))
    }
}

impl fmt::Display for UtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `Timestamp`'s `Display` is RFC 3339 compliant and always renders `Z`
        // because a timestamp has no offset. Fractional seconds appear only
        // when they are non-zero.
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for UtcTimestamp {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for UtcTimestamp {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl Serialize for UtcTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // A string, never an integer: the contract fixes RFC 3339 text, and an
        // integer form would lose the `Z`/UTC guarantee at the wire boundary.
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for UtcTimestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(TimestampVisitor)
    }
}

/// Accepts a borrowed or owned string form of an instant.
struct TimestampVisitor;

impl serde::de::Visitor<'_> for TimestampVisitor {
    type Value = UtcTimestamp;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an RFC 3339 UTC timestamp string")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        UtcTimestamp::parse(value).map_err(E::custom)
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.visit_str(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::UtcTimestamp;

    #[test]
    fn display_is_rfc3339_utc_with_z() {
        let instant = UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid instant");
        assert_eq!(instant.to_string(), "2026-09-20T12:00:00Z");

        let fractional = UtcTimestamp::parse("2005-08-07T23:19:49.123Z").expect("valid instant");
        assert_eq!(fractional.to_string(), "2005-08-07T23:19:49.123Z");
    }

    #[test]
    fn offsets_are_normalized_to_utc() {
        let instant = UtcTimestamp::parse("2024-06-19 15:22:45-04").expect("valid instant");
        assert_eq!(instant.to_string(), "2024-06-19T19:22:45Z");
    }

    #[test]
    fn parsing_and_display_round_trip() {
        let original = UtcTimestamp::parse("2065-01-24T06:49:59.000000123Z").expect("valid");
        let reparsed: UtcTimestamp = original.to_string().parse().expect("display must parse");
        assert_eq!(reparsed, original);
    }

    #[test]
    fn leap_second_is_constrained_not_rejected() {
        let instant = UtcTimestamp::parse("2016-12-31 23:59:60Z").expect("leap second is accepted");
        assert_eq!(instant.to_string(), "2016-12-31T23:59:59Z");
    }

    #[test]
    fn invalid_instants_are_rejected() {
        for value in [
            "",
            " ",
            "not-a-time",
            "2026-13-40T00:00:00Z",      // out-of-range month/day
            "2026-09-20T12:00:00",       // no offset: ambiguous
            "2026-09-20T12:00:00Zextra", // trailing data
            " 2026-09-20T12:00:00Z",     // leading space
        ] {
            let error = UtcTimestamp::parse(value).expect_err("must be rejected");
            assert_eq!(error.code(), "jarvis.invalid_timestamp");
        }
    }

    #[test]
    fn serde_uses_the_string_form_not_an_integer() {
        #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
        struct Record {
            occurred_at: UtcTimestamp,
        }

        let record = Record {
            occurred_at: UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid"),
        };
        let json = serde_json::to_string(&record).expect("serialization succeeds");
        assert_eq!(json, r#"{"occurred_at":"2026-09-20T12:00:00Z"}"#);

        let parsed: Record = serde_json::from_str(&json).expect("deserialization succeeds");
        assert_eq!(parsed, record);
    }

    #[test]
    fn serde_rejects_a_non_string_and_a_malformed_string() {
        #[derive(Debug, serde::Deserialize)]
        struct Record {
            #[allow(dead_code)]
            occurred_at: UtcTimestamp,
        }

        for json in [
            r#"{"occurred_at":1517644800}"#,
            r#"{"occurred_at":"not-a-time"}"#,
            r#"{"occurred_at":"2026-09-20T12:00:00Zextra"}"#,
        ] {
            assert!(
                serde_json::from_str::<Record>(json).is_err(),
                "{json} must be rejected",
            );
        }
    }
}
