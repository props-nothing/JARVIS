//! Provider-safe tool names.
//!
//! JARVIS's canonical tool names are namespaced and versioned — `clock.now@1`, `mcp.read_file@1` — and
//! the tool fabric owns them. The Chat Completions function schema constrains `function.name` to
//! letters, digits, underscores and dashes, at most 64 characters, and a strict endpoint refuses a request
//! that offers anything else (Ollama and a few others are lenient, which is why this went unnoticed).
//! Sending the canonical name would make every real tool unusable on the providers most users have.
//!
//! So the wire carries a **sanitized** name and this module owns the round trip:
//!
//! - **Per request, not global.** A [`ToolNames`] is derived from one request — the offered tools first,
//!   then any tool a replayed transcript names — and travels with the exchange, so the name a model sends
//!   back is resolved against exactly the table the model was shown. There is no process-wide registry
//!   that two concurrent runs could disagree about.
//! - **Injective within a request.** Two canonical names that sanitize alike (`a.b@1` and `a_b_1`) are
//!   kept apart with a numeric suffix, in a fixed order, so the mapping is reproducible.
//! - **An unknown wire name is passed through unchanged.** The model is untrusted: a name that is in no
//!   table reaches the tool pipeline as written and is refused there (`tool.not_found`), exactly as an
//!   invented name always was. The mapping never *grants* anything — it only spells names.

use std::collections::BTreeMap;

/// The longest `function.name` the Chat Completions schema accepts.
pub const MAX_WIRE_NAME_LEN: usize = 64;

/// The name table of one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolNames {
    to_wire: BTreeMap<String, String>,
    to_canonical: BTreeMap<String, String>,
}

impl ToolNames {
    /// Builds the table for the canonical names in `canonical`, in order.
    ///
    /// The order decides which of two colliding names keeps the plain spelling, so callers pass the
    /// offered tools first: a tool the model is being asked to use is never the one that gets a suffix
    /// because an old transcript mentioned a lookalike.
    #[must_use]
    pub fn new<'a>(canonical: impl IntoIterator<Item = &'a str>) -> Self {
        let mut names = Self::default();
        for name in canonical {
            names.insert(name);
        }
        names
    }

    fn insert(&mut self, canonical: &str) {
        if self.to_wire.contains_key(canonical) {
            return;
        }
        let base = sanitize(canonical);
        let mut candidate = base.clone();
        let mut suffix = 2_u32;
        while self.to_canonical.contains_key(&candidate) {
            let tail = format!("_{suffix}");
            let keep = MAX_WIRE_NAME_LEN.saturating_sub(tail.len());
            candidate = format!("{}{tail}", &base[..base.len().min(keep)]);
            suffix += 1;
        }
        self.to_wire.insert(canonical.to_owned(), candidate.clone());
        self.to_canonical.insert(candidate, canonical.to_owned());
    }

    /// Returns the spelling to send for `canonical`.
    #[must_use]
    pub fn wire(&self, canonical: &str) -> String {
        self.to_wire
            .get(canonical)
            .cloned()
            .unwrap_or_else(|| sanitize(canonical))
    }

    /// Returns the canonical name for a name the model sent, or the name itself when it is in no table.
    #[must_use]
    pub fn canonical(&self, wire: &str) -> String {
        self.to_canonical
            .get(wire)
            .cloned()
            .unwrap_or_else(|| wire.to_owned())
    }
}

/// Reduces a canonical name to the characters and length the wire allows.
///
/// Letters, digits and dashes stay; every other run of characters becomes one underscore, so
/// `mcp.read_file@1` reads `mcp_read_file_1`. Leading and trailing underscores are trimmed, an empty
/// result becomes `tool`, and the result is cut to [`MAX_WIRE_NAME_LEN`].
#[must_use]
pub fn sanitize(canonical: &str) -> String {
    let mut out = String::with_capacity(canonical.len());
    let mut pending_separator = false;
    for character in canonical.chars() {
        if character.is_ascii_alphanumeric() || character == '-' {
            if pending_separator && !out.is_empty() {
                out.push('_');
            }
            pending_separator = false;
            out.push(character);
        } else {
            pending_separator = true;
        }
    }
    if out.is_empty() {
        out.push_str("tool");
    }
    out.truncate(MAX_WIRE_NAME_LEN);
    // Truncation can end on a dangling separator only if one was pushed last, which it never is.
    out
}

#[cfg(test)]
mod tests {
    use super::{MAX_WIRE_NAME_LEN, ToolNames, sanitize};

    fn valid(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_WIRE_NAME_LEN
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn a_canonical_name_becomes_a_schema_valid_readable_one() {
        assert_eq!(sanitize("clock.now@1"), "clock_now_1");
        assert_eq!(sanitize("mcp.read_file@1"), "mcp_read_file_1");
        for name in ["clock.now@1", "a/b c", "..", "", "é.ß@2", &"x".repeat(200)] {
            assert!(valid(&sanitize(name)), "{name:?} -> {:?}", sanitize(name));
        }
    }

    #[test]
    fn the_name_a_model_sends_back_resolves_to_the_canonical_one() {
        let names = ToolNames::new(["clock.now@1", "mcp.read_file@1"]);
        assert_eq!(names.wire("mcp.read_file@1"), "mcp_read_file_1");
        assert_eq!(names.canonical("mcp_read_file_1"), "mcp.read_file@1");
        assert_eq!(names.canonical("clock_now_1"), "clock.now@1");
    }

    #[test]
    fn names_that_sanitize_alike_are_kept_apart() {
        let names = ToolNames::new(["a.b@1", "a_b_1", "a-b@1"]);
        let wires = [
            names.wire("a.b@1"),
            names.wire("a_b_1"),
            names.wire("a-b@1"),
        ];
        assert_eq!(wires[0], "a_b_1", "the first keeps the plain spelling");
        let distinct: std::collections::BTreeSet<_> = wires.iter().collect();
        assert_eq!(distinct.len(), 3, "{wires:?}");
        for (canonical, wire) in ["a.b@1", "a_b_1", "a-b@1"].iter().zip(&wires) {
            assert!(valid(wire));
            assert_eq!(&names.canonical(wire), canonical, "the round trip is exact");
        }
    }

    #[test]
    fn a_long_collision_stays_within_the_length_bound() {
        let long = "x".repeat(MAX_WIRE_NAME_LEN);
        let other = format!("{long}.");
        let names = ToolNames::new([long.as_str(), other.as_str()]);
        assert!(valid(&names.wire(&other)), "{}", names.wire(&other));
        assert_ne!(names.wire(&long), names.wire(&other));
    }

    #[test]
    fn an_unknown_name_is_passed_through_for_the_pipeline_to_refuse() {
        let names = ToolNames::new(["clock.now@1"]);
        assert_eq!(names.canonical("shell_run"), "shell_run");
        assert_eq!(names.canonical("clock.now@1"), "clock.now@1");
    }
}
