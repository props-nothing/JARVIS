//! Durable memory: typed, scoped, provenance-carrying claims.
//!
//! This is the first slice of Milestone 4 (`MEM-001`, `MEM-002`, `MEM-003`). The architecture
//! (`docs/architecture/memory-context.md`) is explicit that memory is "a governed knowledge subsystem, not a
//! transcript vector index", and that "the initial implementation stores only explicit user requests such as
//! 'remember that...' and confirmed preferences". That is exactly what this module can express and no more:
//!
//! - **Only a user's explicit request writes a memory.** [`MemorySource`] has one variant, and it carries the
//!   principal who asked. There is deliberately no `Inferred` or `Extracted` source, so a model-proposed claim
//!   has no way to become a memory through this type — the inability is structural rather than a check.
//! - **Every memory is scoped to a workspace**, and the repository takes the workspace as a parameter on every
//!   read, so a query cannot be issued without one.
//! - **Hidden reasoning is inexpressible.** The record holds the user's own words and a provenance, nothing a
//!   model produced.
//! - **Retrieval is deterministic.** [`lexical_relevance`] is a pure function of the query and the text, so a
//!   ranking is reproducible and a test can state the exact order it expects.
//!
//! Not here, and named: embeddings, hybrid ranking, entity resolution, supersession lineage, and the other
//! memory classes. [`MemoryClass`] lists only the two classes the first slice writes.

use std::collections::BTreeSet;
use std::fmt;

use crate::ids::{MemoryId, PrincipalId, WorkspaceId};
use crate::model::policy::Sensitivity;
use crate::time::UtcTimestamp;

/// The longest accepted memory text, in bytes.
///
/// Bounded because the text is written into a prompt: an unbounded memory is an unbounded prompt, and the
/// context budgeter would exclude it whole rather than truncate it, so an over-long memory would simply never
/// be recalled.
pub const MAX_MEMORY_TEXT_BYTES: usize = 2_048;

/// The most memories one search or listing returns.
pub const MAX_MEMORY_PAGE: u32 = 100;

/// Why a memory could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRefusal {
    /// The text is empty after trimming.
    EmptyText,
    /// The text exceeds [`MAX_MEMORY_TEXT_BYTES`].
    TextTooLong,
    /// The text contains a control character other than a newline or tab.
    ControlCharacter,
    /// The class is not one this build writes.
    UnknownClass,
}

impl MemoryRefusal {
    /// Returns a short stable reason, safe to show the requesting principal.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::EmptyText => "the memory text is empty",
            Self::TextTooLong => "the memory text is too long",
            Self::ControlCharacter => "the memory text contains a control character",
            Self::UnknownClass => "the memory class is not supported",
        }
    }
}

/// A validated memory text: trimmed, non-empty, bounded, and free of control characters.
///
/// Validated **on the way in**, because a memory is later rendered into a prompt and an invalid value that
/// reached storage would be re-read by every future run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryText(String);

impl MemoryText {
    /// Validates `value`.
    ///
    /// # Errors
    ///
    /// Returns the [`MemoryRefusal`] naming the first rule the value breaks.
    pub fn new(value: &str) -> Result<Self, MemoryRefusal> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(MemoryRefusal::EmptyText);
        }
        if trimmed.len() > MAX_MEMORY_TEXT_BYTES {
            return Err(MemoryRefusal::TextTooLong);
        }
        if trimmed
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
        {
            return Err(MemoryRefusal::ControlCharacter);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the normalized form used to recognize an exact duplicate: lowercased, with every run of
    /// whitespace collapsed to one space.
    ///
    /// Two texts with the same key are the same claim, so storing both would only make a recall return it
    /// twice and make a correction ambiguous about which copy it meant.
    #[must_use]
    pub fn dedupe_key(&self) -> String {
        self.0
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Estimates the prompt cost in tokens, with the same four-bytes-per-token figure the context assembly
    /// uses for messages, so a memory and a message are budgeted on one scale.
    #[must_use]
    pub fn estimated_tokens(&self) -> u64 {
        (u64::try_from(self.0.len()).unwrap_or(u64::MAX) / 4).max(1)
    }
}

/// The classes of memory this build writes.
///
/// A subset of the seven the architecture names, and deliberately so: working memory is run state and is
/// never durable, and the remaining durable classes need lifecycle rules (episodic expiry, relationship
/// confidence, procedural versioning) that this slice does not implement. Adding a class later is an addition
/// here and a migration-free change, because the column stores the spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryClass {
    /// How the user wants outcomes or interactions to go.
    Preference,
    /// A fact or belief about the user's world.
    Semantic,
}

impl MemoryClass {
    /// Returns the stored and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Semantic => "semantic",
        }
    }

    /// Parses the stored or wire spelling.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryRefusal::UnknownClass`] for any other value, so a class this build cannot interpret is
    /// refused rather than defaulted.
    pub fn parse(value: &str) -> Result<Self, MemoryRefusal> {
        match value {
            "preference" => Ok(Self::Preference),
            "semantic" => Ok(Self::Semantic),
            _ => Err(MemoryRefusal::UnknownClass),
        }
    }
}

impl fmt::Display for MemoryClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a memory came from.
///
/// One variant: an explicit request from a principal. See the module documentation for why there is no
/// second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    /// The principal asked JARVIS to remember this.
    UserRequest {
        /// The principal who asked.
        principal: PrincipalId,
    },
}

impl MemorySource {
    /// Returns the stored spelling of the source kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::UserRequest { .. } => "user_request",
        }
    }

    /// Returns the principal behind the source.
    #[must_use]
    pub const fn principal(&self) -> PrincipalId {
        match self {
            Self::UserRequest { principal } => *principal,
        }
    }
}

/// One durable memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    /// Its identity.
    pub id: MemoryId,
    /// The workspace that owns it.
    pub workspace: WorkspaceId,
    /// Its class.
    pub class: MemoryClass,
    /// The user's words.
    pub text: MemoryText,
    /// How sensitive the claim is, which bounds where it may be sent.
    pub sensitivity: Sensitivity,
    /// Where it came from.
    pub source: MemorySource,
    /// When it was first stored.
    pub created_at: UtcTimestamp,
    /// When it was last changed.
    pub updated_at: UtcTimestamp,
}

/// Splits `text` into lowercase alphanumeric tokens of at least two characters.
///
/// Single characters are dropped because they match nearly everything and rank nothing. Stop-word lists are
/// deliberately absent: they are language-specific, and a wrong list silently deletes a word the user meant.
fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|token| token.chars().count() >= 2)
        .collect()
}

/// Scores how well `text` answers `query`, in thousandths, or `None` when no query token appears.
///
/// The score is the fraction of the **query's** distinct tokens that appear in the text, so a short memory is
/// not penalised for being short and a long one is not rewarded for mentioning everything. A token matches
/// exactly or as a shared prefix of at least four characters, which lets "proposals" find "proposal" without
/// a stemmer whose language rules this slice cannot vouch for.
///
/// Pure and total: the same inputs always give the same score, which is what lets a ranking test state an
/// exact order.
#[must_use]
pub fn lexical_relevance(query: &str, text: &str) -> Option<u32> {
    let wanted = tokens(query);
    if wanted.is_empty() {
        return None;
    }
    let available = tokens(text);
    let matched = wanted
        .iter()
        .filter(|token| {
            available
                .iter()
                .any(|candidate| candidate == *token || shares_prefix(candidate, token))
        })
        .count();
    if matched == 0 {
        return None;
    }
    let matched = u32::try_from(matched).unwrap_or(u32::MAX);
    let total = u32::try_from(wanted.len()).unwrap_or(u32::MAX).max(1);
    Some(matched.saturating_mul(1_000) / total)
}

/// Whether one token is a prefix of the other and the shorter is at least four characters.
fn shares_prefix(left: &str, right: &str) -> bool {
    let (short, long) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    short.chars().count() >= 4 && long.starts_with(short)
}

#[cfg(test)]
mod tests {
    use super::{MAX_MEMORY_TEXT_BYTES, MemoryClass, MemoryRefusal, MemoryText, lexical_relevance};

    #[test]
    fn text_is_trimmed_and_bounded_and_free_of_control_characters() {
        assert_eq!(
            MemoryText::new("  client proposals should be concise \n")
                .expect("valid")
                .as_str(),
            "client proposals should be concise"
        );
        assert_eq!(MemoryText::new("   ").err(), Some(MemoryRefusal::EmptyText));
        assert_eq!(
            MemoryText::new(&"a".repeat(MAX_MEMORY_TEXT_BYTES + 1)).err(),
            Some(MemoryRefusal::TextTooLong)
        );
        assert!(MemoryText::new(&"a".repeat(MAX_MEMORY_TEXT_BYTES)).is_ok());
        assert_eq!(
            MemoryText::new("bad\u{0}text").err(),
            Some(MemoryRefusal::ControlCharacter)
        );
        assert_eq!(
            MemoryText::new("escape\u{1b}[31m").err(),
            Some(MemoryRefusal::ControlCharacter)
        );
        // A newline and a tab are ordinary text.
        assert!(MemoryText::new("line one\nline\ttwo").is_ok());
    }

    #[test]
    fn two_spellings_of_one_claim_share_a_dedupe_key() {
        let one = MemoryText::new("Client   proposals should be CONCISE").expect("valid");
        let two = MemoryText::new("client proposals should be concise").expect("valid");
        assert_eq!(one.dedupe_key(), two.dedupe_key());
        let other = MemoryText::new("client proposals should be long").expect("valid");
        assert_ne!(one.dedupe_key(), other.dedupe_key());
    }

    #[test]
    fn a_class_round_trips_and_an_unknown_one_is_refused() {
        for class in [MemoryClass::Preference, MemoryClass::Semantic] {
            assert_eq!(MemoryClass::parse(class.as_str()), Ok(class));
        }
        assert_eq!(
            MemoryClass::parse("episodic"),
            Err(MemoryRefusal::UnknownClass)
        );
        assert_eq!(MemoryClass::parse(""), Err(MemoryRefusal::UnknownClass));
    }

    #[test]
    fn relevance_is_the_fraction_of_query_tokens_found() {
        let text = "client proposals should be concise";
        assert_eq!(lexical_relevance("client proposals", text), Some(1_000));
        assert_eq!(lexical_relevance("client budget", text), Some(500));
        assert_eq!(lexical_relevance("weather forecast", text), None);
        // Case does not matter.
        assert_eq!(lexical_relevance("CLIENT", text), Some(1_000));
    }

    #[test]
    fn a_plural_finds_its_singular_but_a_short_token_does_not_match_by_prefix() {
        assert_eq!(
            lexical_relevance("proposal", "client proposals are short"),
            Some(1_000)
        );
        // `cat` is a prefix of `category` but is under four characters, so it must not match.
        assert_eq!(lexical_relevance("cat", "category theory"), None);
    }

    #[test]
    fn an_empty_or_single_character_query_matches_nothing() {
        assert_eq!(lexical_relevance("", "anything at all"), None);
        assert_eq!(lexical_relevance("a i", "a i o"), None);
    }

    #[test]
    fn relevance_is_deterministic() {
        let first = lexical_relevance("prefer concise proposals", "Prefers concise proposals");
        for _ in 0..10 {
            assert_eq!(
                lexical_relevance("prefer concise proposals", "Prefers concise proposals"),
                first
            );
        }
        assert_eq!(first, Some(1_000));
    }
}
