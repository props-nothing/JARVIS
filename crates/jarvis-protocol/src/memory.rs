//! The memory wire vocabulary.
//!
//! The views are projections, not stored shapes: the handler builds them in `http::memory`, so a handler
//! cannot leak a field by serializing a domain value.
//!
//! **A write request carries the user's words and two labels, and nothing about who or where.** The workspace
//! and the principal come from the authenticated context, so a client cannot write a memory into another
//! workspace or attribute one to another principal — there is no field in which to say so.
//!
//! **`deny_unknown_fields` on the request**, so a client that sends `confidence` or `source` — fields a model
//! might invent — is told its body is wrong rather than having them ignored. This slice has no inferred
//! memories, and the request shape says so.

use serde::{Deserialize, Serialize};

/// A memory as a client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryView {
    /// The memory's identifier, which a read or a forget addresses.
    pub memory_id: String,
    /// The workspace that owns it.
    pub workspace_id: String,
    /// Its class: `preference` or `semantic`.
    pub class: String,
    /// The user's words.
    pub text: String,
    /// Its sensitivity label.
    pub sensitivity: String,
    /// How it came to exist. Always `user_request` in this build.
    pub source_kind: String,
    /// The principal who asked JARVIS to remember it.
    pub source_principal_id: String,
    /// When it was first stored.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

/// The answer to a request to remember.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedView {
    /// The memory, whether newly stored or already known.
    pub memory: MemoryView,
    /// `false` when the workspace already held this exact claim and nothing was written.
    pub created: bool,
}

/// One page of memories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryListView {
    /// The memories, newest first.
    pub memories: Vec<MemoryView>,
    /// Whether the store stopped at the limit with more remaining.
    pub has_more: bool,
    /// The largest page this daemon serves.
    pub max_page: u32,
}

/// One ranked search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryHitView {
    /// The matching memory.
    pub memory: MemoryView,
    /// Lexical relevance in thousandths of the query's tokens found.
    pub relevance: u32,
}

/// The result of a search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySearchView {
    /// The hits, most relevant first.
    pub hits: Vec<MemoryHitView>,
    /// Whether the workspace held more memories than one search considers, so the oldest were not ranked.
    pub scan_bounded: bool,
}

/// A request to remember something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RememberRequest {
    /// What to remember, in the user's words.
    pub text: String,
    /// The class; defaults to `preference`.
    #[serde(default = "default_class")]
    pub class: String,
    /// The sensitivity label; defaults to `internal`.
    #[serde(default = "default_sensitivity")]
    pub sensitivity: String,
}

fn default_class() -> String {
    "preference".to_owned()
}

fn default_sensitivity() -> String {
    "internal".to_owned()
}

#[cfg(test)]
mod tests {
    use super::RememberRequest;

    #[test]
    fn the_request_defaults_are_the_documented_ones() {
        let request: RememberRequest =
            serde_json::from_str(r#"{"text":"be concise"}"#).expect("parses");
        assert_eq!(request.class, "preference");
        assert_eq!(request.sensitivity, "internal");
    }

    #[test]
    fn a_request_naming_a_source_or_a_confidence_is_refused() {
        // A model-invented provenance has no field to land in.
        for body in [
            r#"{"text":"x","source":"model"}"#,
            r#"{"text":"x","confidence":1.0}"#,
            r#"{"text":"x","workspace_id":"018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d"}"#,
        ] {
            assert!(
                serde_json::from_str::<RememberRequest>(body).is_err(),
                "{body}"
            );
        }
    }
}
