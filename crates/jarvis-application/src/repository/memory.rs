//! The memory repository port.
//!
//! Every read takes a [`WorkspaceId`], and a memory owned by another workspace is
//! [`RepositoryError::NotFound`](super::RepositoryError::NotFound) — indistinguishable from one that does not
//! exist, the rule every other store here follows. The workspace is a parameter rather than an implicit
//! filter so a query cannot be written without one: `ACC-032` requires that queries "never cross scope", and
//! the only way to make that structural is to have no method that does not name it.

use jarvis_domain::ids::{MemoryId, WorkspaceId};
use jarvis_domain::memory::Memory;

use super::RepositoryFuture;

/// How many memories a search reads before ranking.
///
/// A bound on the **candidate set**, not on the result: ranking is done over the newest `MAX_SEARCH_SCAN`
/// memories of the workspace. The result reports whether the bound was reached, so a caller can tell "nothing
/// matched" from "nothing matched in the newest N". Lexical retrieval over an indexed full-text column is a
/// later increment; until then this keeps the cost of one search bounded however many memories exist.
pub const MAX_SEARCH_SCAN: u32 = 1_000;

/// What storing a memory did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RememberOutcome {
    /// A new memory was stored.
    Created,
    /// The workspace already held this exact claim; nothing was written and this is its identity.
    AlreadyKnown(MemoryId),
}

/// One page of memories, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPage {
    /// The memories.
    pub memories: Vec<Memory>,
    /// Whether the store stopped at the requested limit with more remaining.
    pub bounded: bool,
}

/// One ranked search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHit {
    /// The matching memory.
    pub memory: Memory,
    /// Lexical relevance in thousandths; see `jarvis_domain::memory::lexical_relevance`.
    pub relevance: u32,
}

/// The result of a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearch {
    /// The hits, most relevant first; ties break on recency, then identity, so the order is total.
    pub hits: Vec<MemoryHit>,
    /// Whether the workspace held more memories than [`MAX_SEARCH_SCAN`], so older ones were not considered.
    pub scan_bounded: bool,
}

/// A durable store of memories.
pub trait MemoryRepository: Send + Sync {
    /// Stores `memory` unless the workspace already holds the same claim.
    ///
    /// The uniqueness is decided by the store in one statement, so two concurrent identical requests produce
    /// one memory and one of them is told it already existed.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`](super::RepositoryError::Query) for a driver failure.
    fn remember(&self, memory: &Memory) -> RepositoryFuture<'_, RememberOutcome>;

    /// Loads one memory within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for an absent or foreign
    /// memory and [`RepositoryError::Corrupted`](super::RepositoryError::Corrupted) for a row that cannot be
    /// interpreted.
    fn load(&self, workspace: WorkspaceId, id: MemoryId) -> RepositoryFuture<'_, Memory>;

    /// Lists the newest memories of `workspace`, at most `limit`.
    ///
    /// # Errors
    ///
    /// As [`Self::load`], except that nothing is "not found" for a list.
    fn list(&self, workspace: WorkspaceId, limit: u32) -> RepositoryFuture<'_, MemoryPage>;

    /// Ranks the workspace's memories against `query`, returning at most `limit`.
    ///
    /// # Errors
    ///
    /// As [`Self::list`].
    fn search(
        &self,
        workspace: WorkspaceId,
        query: &str,
        limit: u32,
    ) -> RepositoryFuture<'_, MemorySearch>;

    /// Permanently deletes one memory.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](super::RepositoryError::NotFound) for an absent or foreign
    /// memory, so deleting another workspace's memory is indistinguishable from deleting nothing.
    fn forget(&self, workspace: WorkspaceId, id: MemoryId) -> RepositoryFuture<'_, ()>;
}
