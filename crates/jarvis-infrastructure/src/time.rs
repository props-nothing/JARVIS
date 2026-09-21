//! Adapter for the domain [`Clock`](jarvis_domain::clock::Clock) port.
//!
//! Reads the operating-system clock fallibly. It never panics on a clock that
//! reports an out-of-range value, because a panic in a daemon is worse than an
//! explicit error the caller can retry or report.

use std::time::SystemTime;

use jarvis_domain::clock::Clock;
use jarvis_domain::error::DomainError;
use jarvis_domain::time::UtcTimestamp;
use jiff::Timestamp;

/// A clock backed by the operating system.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl SystemClock {
    /// Creates a system clock.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns the current instant, as the [`Clock`] port defines it.
    ///
    /// This is an inherent forwarder so callers do not need the trait in scope
    /// for the common case.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::SystemClockUnavailable`] when the operating-system
    /// clock reports an unusable value.
    pub fn now(&self) -> Result<UtcTimestamp, DomainError> {
        <Self as Clock>::now(self)
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Result<UtcTimestamp, DomainError> {
        // Use the fallible conversion instead of `Timestamp::now()`, which
        // panics when the system clock is set to an unreasonable value.
        let system_time = SystemTime::now();
        let timestamp =
            Timestamp::try_from(system_time).map_err(|_| DomainError::SystemClockUnavailable)?;
        Ok(UtcTimestamp::from_timestamp(timestamp))
    }
}

#[cfg(test)]
mod tests {
    use super::SystemClock;

    #[test]
    fn system_clock_returns_a_plausible_instant() {
        let now = SystemClock::new()
            .now()
            .expect("the test host clock must be usable"); // Sanity bound: later than 2020-01-01 but far from the 9999 limit.
        assert!(now.to_string().starts_with("20"));
    }
}
