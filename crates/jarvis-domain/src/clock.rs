//! The injected authoritative clock.
//!
//! Expiry, deadlines, and lease comparisons must use an injected clock so tests
//! are deterministic and so a single authority defines "now". The domain owns
//! the port; an infrastructure adapter reads the operating-system clock.

use std::sync::Mutex;

use crate::error::DomainError;
use crate::time::UtcTimestamp;

/// Supplies the current absolute time.
///
/// The trait is object safe so a concrete clock can be injected behind a
/// reference or an `Arc`.
pub trait Clock: Send + Sync {
    /// Returns the current instant.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::SystemClockUnavailable`] when the operating-system
    /// clock reports a value that cannot be represented. The error is returned
    /// rather than a guessed instant so callers never act on a fabricated time.
    fn now(&self) -> Result<UtcTimestamp, DomainError>;
}

/// A clock whose value is set explicitly, for deterministic tests.
///
/// It is public so tests in other crates can share one implementation rather
/// than each defining a subtly different double.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<UtcTimestamp>,
}

impl ManualClock {
    /// Creates a clock fixed at `now`.
    #[must_use]
    pub fn new(now: UtcTimestamp) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }

    /// Replaces the current instant.
    ///
    /// The update is best effort: a poisoned lock can only result from a
    /// previous panic in a test, in which case the whole test has already
    /// failed and the new value is not observable.
    pub fn set(&self, now: UtcTimestamp) {
        if let Ok(mut guard) = self.now.lock() {
            *guard = now;
        }
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Result<UtcTimestamp, DomainError> {
        // A poisoned lock is surfaced as an unavailable clock rather than
        // panicking, keeping the clock port fallible like the real adapter.
        self.now
            .lock()
            .map(|guard| *guard)
            .map_err(|_| DomainError::SystemClockUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, ManualClock};
    use crate::time::UtcTimestamp;

    #[test]
    fn manual_clock_returns_and_updates_its_value() {
        let first = UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid");
        let second = UtcTimestamp::parse("2026-09-20T12:00:01Z").expect("valid");
        let clock = ManualClock::new(first);

        assert_eq!(clock.now().expect("clock is available"), first);
        clock.set(second);
        assert_eq!(clock.now().expect("clock is available"), second);
    }

    #[test]
    fn clock_trait_is_object_safe() {
        let clock: Box<dyn Clock> = Box::new(ManualClock::new(
            UtcTimestamp::parse("2026-09-20T12:00:00Z").expect("valid"),
        ));
        assert!(clock.now().is_ok());
    }
}
