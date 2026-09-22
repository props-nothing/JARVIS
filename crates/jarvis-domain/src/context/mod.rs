//! Context assembly: candidates, budgets, and the context manifest.
//!
//! This module is the domain half of `docs/architecture/memory-context.md`'s
//! "Retrieval Pipeline", "Context Manifest", and "Context Source Priority"
//! sections, and the typed form of `FR-CTX-001` ("context selection is budgeted,
//! provenance-aware, and recorded") together with the context half of
//! `FR-RUN-005` ("hidden chain-of-thought is never persisted or exposed").
//!
//! The architecture draws the pipeline as
//! `Query -> Scope -> Candidates -> Rank -> Diversify -> Budget -> Manifest`, and
//! three of its rules are enforced by these types rather than restated in each
//! caller:
//!
//! - **Policy filtering precedes ranking.** [`budget::ContextBudget::assemble`]
//!   refuses a candidate whose sensitivity exceeds the ceiling or whose validity has
//!   passed *before* it is scored, because the architecture is explicit that
//!   post-filtering a ranked result "leaks both recall and potential side
//!   channels".
//! - **A budget is a hard bound.** An item that does not fit is excluded with a
//!   recorded reason, never truncated, because a half-truncated message reads as a
//!   complete one to the model.
//! - **Every decision is recorded.** [`manifest::ContextManifest`] carries the
//!   references, sources, sensitivities, token estimates, inclusion reasons, and an
//!   exclusion summary counted by reason — so "why did JARVIS send this" and "why was
//!   that omitted" are both answerable without storing duplicate prompt text.
//!
//! Hidden reasoning is made **inexpressible** rather than filtered:
//! [`source::CandidateSource`] has no variant for model-internal reasoning, so a
//! caller cannot offer it, and a serialized-shape test asserts the manifest has no
//! field that could carry it.

pub mod budget;
pub mod manifest;
pub mod source;

pub use budget::{
    AssembledContext, ContextBudget, ExclusionReason, IncludedItem, MAX_CANDIDATES,
    MAX_CONTEXT_BUDGET_TOKENS,
};
pub use manifest::{ContextManifest, ContextManifestItem, ExclusionSummary};
pub use source::{CandidateSource, ContextCandidate, InclusionReason, SourcePriority};
