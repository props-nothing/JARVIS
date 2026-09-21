//! Adapter for the domain [`IdGenerator`](jarvis_domain::ids::IdGenerator) port.
//!
//! Generates `UUIDv7` identifiers, which are time-ordered for a given process.
//! The ordering property is used only for index locality and never as
//! authorization context.

use jarvis_domain::ids::IdGenerator;
use uuid::Uuid;

/// Generates `UUIDv7` identifiers.
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidV7Generator;

impl UuidV7Generator {
    /// Creates a `UUIDv7` generator.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Generates a daemon instance identifier.
    ///
    /// `Uuid::now_v7` cannot fail, so this returns a value rather than a
    /// `Result` that could never be `Err`.
    #[must_use]
    pub fn next_instance_id(&self) -> String {
        self.next_uuid("instance").to_string()
    }
}

impl IdGenerator for UuidV7Generator {
    fn next_uuid(&self, _kind: &'static str) -> Uuid {
        Uuid::now_v7()
    }
}

#[cfg(test)]
mod tests {
    use jarvis_domain::ids::IdGenerator;

    use super::UuidV7Generator;

    #[test]
    fn generated_identifiers_are_version_7_and_distinct() {
        let generator = UuidV7Generator::new();
        let first = generator.next_uuid("workspace");
        let second = generator.next_uuid("workspace");

        assert_eq!(first.get_version_num(), 7);
        assert_ne!(first, second);
    }
}
