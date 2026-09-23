//! Absolute UTC instants and calendar dates.
//!
//! JARVIS stores and transmits absolute time as RFC 3339 UTC strings with a
//! trailing `Z` (see the common contract conventions). [`UtcTimestamp`] is the
//! single value type for that representation.
//!
//! [`IsoDate`] is the calendar-date value used where a day, not an instant, is
//! the fact: provider capability evidence records when it was `last_verified`
//! and when it must be `revalidate_by`, and an evidence label is only usable
//! while the current date has not passed that day.

use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::DomainError;

/// An absolute instant in UTC with nanosecond precision.
///
/// The value is normalized to UTC on construction, so it has exactly one canonical
/// form: RFC 3339 with a `Z` offset. Offset-bearing input is accepted and
/// converted, which makes it impossible for two different-looking strings to
/// denote two different stored values for the same instant.
///
/// **One canonical string per instant — but a variable-width column, and a string that does not
/// sort chronologically.** Zero fractional digits are omitted, so `2026-09-20T12:00:00.000Z` and
/// `2026-09-20T12:00:00Z` render identically (measured), while
/// `2026-09-20T12:00:00.123456789Z` renders at 30 characters. That variability breaks lexical
/// ordering, which is easy to assume and wrong: `.` is `0x2E` and `Z` is `0x5A`, so
/// `12:00:00.1Z` sorts **before** `12:00:00Z` even though it is later. **A `TEXT` `<`, `<=`, or
/// `ORDER BY` over these strings is therefore not chronological**, and a column compared that way
/// can name the wrong row. Ordering and range predicates on the *values* are correct
/// (`jiff::Timestamp` is `Ord`); see
/// `the_canonical_forms_of_one_instant_are_equal_while_their_text_order_is_not` for the assertion
/// and `docs/contracts/common-conventions.md` for the rule a SQL predicate must follow.
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

    /// Returns this instant's calendar date in UTC.
    ///
    /// Lives here rather than at each caller because deriving a date needs `jiff`'s time-zone
    /// conversion, and the layers above the domain deliberately do not depend on it: a caller that
    /// reached for `to_zoned` would have to add a dependency the evidence note does not cover,
    /// which is how a clock concern leaks into a crate that only needed a day. The conversion is
    /// UTC-only, because evidence freshness is evaluated against the same instant for every reader
    /// and a local-zone date would make it depend on where the daemon happens to run.
    ///
    /// Infallible: `Timestamp` is already an absolute instant, so projecting it into UTC cannot
    /// fail. It returns a value rather than an `Option` for that reason — an `Option` here would
    /// push an unreachable branch onto every caller.
    #[must_use]
    pub fn utc_date(&self) -> IsoDate {
        IsoDate(self.0.to_zoned(jiff::tz::TimeZone::UTC).date())
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

/// A calendar date with no time and no zone, as `YYYY-MM-DD`.
///
/// Provider capability evidence is dated by day, so a date is the honest unit: a
/// `revalidate_by` day is inclusive, and comparing instants would make freshness
/// depend on the hour of an unrelated clock. Only the 4-2-2 zero-padded form is
/// accepted, because accepting `2026-1-5` would create a second spelling of the
/// same day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IsoDate(Date);

impl IsoDate {
    /// Wraps a calendar date.
    #[must_use]
    pub const fn from_date(date: Date) -> Self {
        Self(date)
    }

    /// Returns the underlying calendar date.
    #[must_use]
    pub const fn as_date(&self) -> Date {
        self.0
    }

    /// Parses a `YYYY-MM-DD` calendar date.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::InvalidTimestamp`] when `value` is not a complete
    /// zero-padded date, including an out-of-range month or day and any trailing
    /// data. A missing component is not defaulted, because a defaulted month or
    /// day would silently date evidence to a day nobody recorded.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        let date: Date = value.parse().map_err(|_| DomainError::InvalidTimestamp)?;
        if date.strftime("%Y-%m-%d").to_string() != value {
            return Err(DomainError::InvalidTimestamp);
        }
        Ok(Self(date))
    }

    /// Returns whether `self` is on or after `other`.
    ///
    /// Used for freshness, where an evidence label is valid through its
    /// `revalidate_by` day inclusive.
    #[must_use]
    pub fn is_no_earlier_than(self, other: Self) -> bool {
        self.0 >= other.0
    }
}

impl fmt::Display for IsoDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.strftime("%Y-%m-%d"))
    }
}

impl FromStr for IsoDate {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for IsoDate {
    type Error = DomainError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl Serialize for IsoDate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IsoDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(DateVisitor)
    }
}

/// Accepts a borrowed or owned string form of a calendar date.
struct DateVisitor;

impl serde::de::Visitor<'_> for DateVisitor {
    type Value = IsoDate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YYYY-MM-DD calendar date string")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        IsoDate::parse(value).map_err(E::custom)
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
    fn the_canonical_forms_of_one_instant_are_equal_while_their_text_order_is_not() {
        // **Two assumptions about this type were wrong, and this test pins the measured facts.**
        //
        // The first was mine, while correcting the doc: I wrote that a zero fractional part is
        // *padded to a different width*, so `…:00Z` and `…:00.000Z` were two strings for one
        // instant. `jiff` **omits** zero fraction digits, so they render identically and the type
        // has one canonical string per instant exactly as its doc always said. The test that
        // asserted the opposite failed, which is the only reason the claim did not reach a
        // document.
        //
        // The second is the one that matters, and it was asserted nowhere while a convention
        // stated its opposite: **the canonical strings do not sort chronologically.** `.` (0x2E)
        // sorts before `Z` (0x5A), so a fractional instant sorts *before* the whole second it
        // follows. `docs/data/migrations.md`'s maintenance-lock lease compared `expires_at <= ?`
        // as TEXT, and `docs/contracts/common-conventions.md` said text comparison "agrees with
        // chronological order for instants that are not equal" — which this measurement refutes.
        let whole = UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid instant");
        let padded = UtcTimestamp::parse("2026-09-20T12:00:00.000Z").expect("valid instant");
        let fraction = UtcTimestamp::parse("2026-09-20T12:00:00.1Z").expect("valid instant");

        // One canonical string per instant: a zero fraction is dropped, not padded. So a SQL `=`
        // between two spellings of one instant *does* match, and the earlier worry was unfounded.
        assert_eq!(whole.to_string(), padded.to_string());
        assert_eq!(whole.to_string(), "2026-09-20T12:00:00Z");
        assert_eq!(fraction.to_string(), "2026-09-20T12:00:00.1Z");

        // …**and the fraction is the instant that follows**, as a value.
        assert!(whole < fraction);

        // …**but its string sorts first.** This is the direction that makes a TEXT predicate
        // unsound, and it is asserted rather than described so that a future change to the display
        // form cannot quietly make the convention's warning stale.
        assert!(
            fraction.to_string() < whole.to_string(),
            "{fraction} must sort before {whole} under byte comparison, which is why a TEXT \
             range predicate is not chronological",
        );

        // The carry case, which a "shorter string is smaller" rule would also get wrong in the
        // other direction: the largest sub-second fraction still sorts before the next whole second
        // **only because `0` < `1`**, so the failure above is not universal — it is exactly the
        // zero-fraction boundary. Pinned so the convention can state the boundary rather than a
        // blanket rule.
        let last_fraction =
            UtcTimestamp::parse("2026-09-20T12:00:00.999999999Z").expect("valid instant");
        let next_whole = UtcTimestamp::parse("2026-09-20T12:00:01Z").expect("valid instant");
        assert!(last_fraction.to_string() < next_whole.to_string());
        assert!(last_fraction < next_whole);

        // And the value comparison is correct in both cases, which is what a caller who needs
        // chronological order must use.
        assert!(fraction < next_whole && whole < next_whole);
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

    #[test]
    fn a_calendar_date_round_trips_and_is_zero_padded() {
        let date = super::IsoDate::parse("2026-09-21").expect("valid date");
        assert_eq!(date.to_string(), "2026-09-21");
        let reparsed: super::IsoDate = date.to_string().parse().expect("display must parse");
        assert_eq!(reparsed, date);

        let json = serde_json::to_string(&date).expect("serializes");
        assert_eq!(json, "\"2026-09-21\"");
    }

    #[test]
    fn a_non_padded_or_partial_date_is_rejected() {
        for value in [
            "",
            "2026-9-21",            // unpadded month
            "2026-09-1",            // unpadded day
            "2026-09",              // missing day
            "2026",                 // missing month and day
            "2026-13-01",           // out-of-range month
            "2026-02-30",           // out-of-range day
            "2026-09-21T00:00:00Z", // a timestamp is not a date
            "2026-09-21 ",          // trailing space
            " 2026-09-21",          // leading space
        ] {
            let error = super::IsoDate::parse(value).expect_err("must be rejected");
            assert_eq!(error.code(), "jarvis.invalid_timestamp", "{value}");
        }
    }

    #[test]
    fn freshness_comparison_includes_the_day_itself() {
        let revalidate_by = super::IsoDate::parse("2026-09-21").expect("valid");
        assert!(revalidate_by.is_no_earlier_than(revalidate_by));
        assert!(
            super::IsoDate::parse("2026-09-22")
                .expect("valid")
                .is_no_earlier_than(revalidate_by)
        );
        assert!(
            !super::IsoDate::parse("2026-09-20")
                .expect("valid")
                .is_no_earlier_than(revalidate_by)
        );
    }
}
