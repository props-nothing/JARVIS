//! The memory service: explicit user requests in, scoped memories out.
//!
//! The service is the only writer. It builds the record from a trusted [`RequestContext`] — the workspace and
//! the principal come from the context, never from a request body — and the source is always
//! [`MemorySource::UserRequest`], so there is no path by which a model-proposed claim becomes a memory here.
//! Automatic extraction is a later slice and will arrive as a different, policy-gated write path.
//!
//! The service holds no rule of its own beyond that: text validation is the domain's, uniqueness is the
//! store's, and ranking is the domain's pure function.

use std::sync::Arc;

use jarvis_domain::ids::{IdGenerator, MemoryId, WorkspaceId};
use jarvis_domain::memory::{
    MAX_MEMORY_PAGE, Memory, MemoryClass, MemoryRefusal, MemorySource, MemoryText,
};
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::time::UtcTimestamp;

use crate::repository::RepositoryError;
use crate::repository::memory::{MemoryPage, MemoryRepository, MemorySearch, RememberOutcome};
use crate::request_context::RequestContext;

/// Why a memory operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryServiceError {
    /// The request named something this build refuses.
    Refused(MemoryRefusal),
    /// No such memory in this workspace.
    NotFound,
    /// The store failed.
    Storage(RepositoryError),
}

impl From<RepositoryError> for MemoryServiceError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound => Self::NotFound,
            other => Self::Storage(other),
        }
    }
}

/// What `remember` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    /// The memory, whether newly stored or already known.
    pub memory: Memory,
    /// Whether it was newly stored.
    pub created: bool,
}

/// The memory use cases.
pub struct MemoryService {
    store: Arc<dyn MemoryRepository>,
    ids: Arc<dyn IdGenerator>,
}

impl MemoryService {
    /// Builds the service over a store and an identifier source.
    #[must_use]
    pub fn new(store: Arc<dyn MemoryRepository>, ids: Arc<dyn IdGenerator>) -> Self {
        Self { store, ids }
    }

    /// Returns the underlying store, for the one consumer that needs ranked recall without the use case
    /// wrapper: context assembly, which already holds a workspace.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn MemoryRepository> {
        &self.store
    }

    /// Stores an explicit request to remember `text`.
    ///
    /// An exact duplicate (same normalized text in the same workspace) returns the existing memory with
    /// `created: false` — which also makes a retried request idempotent without a separate key.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::Refused`] for text or a class this build refuses, and `Storage` for a
    /// store failure.
    pub async fn remember(
        &self,
        context: &RequestContext,
        class: MemoryClass,
        text: &str,
        sensitivity: Sensitivity,
        now: UtcTimestamp,
    ) -> Result<Remembered, MemoryServiceError> {
        let text = MemoryText::new(text).map_err(MemoryServiceError::Refused)?;
        let memory = Memory {
            id: MemoryId::from_uuid(self.ids.next_uuid("memory")),
            workspace: context.workspace_id,
            class,
            text,
            sensitivity,
            source: MemorySource::UserRequest {
                principal: context.principal_id,
            },
            created_at: now,
            updated_at: now,
        };
        match self.store.remember(&memory).await? {
            RememberOutcome::Created => Ok(Remembered {
                memory,
                created: true,
            }),
            RememberOutcome::AlreadyKnown(existing) => Ok(Remembered {
                memory: self.store.load(context.workspace_id, existing).await?,
                created: false,
            }),
        }
    }

    /// Reads one memory.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::NotFound`] for an absent or foreign memory.
    pub async fn read(
        &self,
        workspace: WorkspaceId,
        id: MemoryId,
    ) -> Result<Memory, MemoryServiceError> {
        Ok(self.store.load(workspace, id).await?)
    }

    /// Lists the newest memories.
    ///
    /// # Errors
    ///
    /// Returns `Storage` for a store failure.
    pub async fn list(
        &self,
        workspace: WorkspaceId,
        limit: u32,
    ) -> Result<MemoryPage, MemoryServiceError> {
        Ok(self
            .store
            .list(workspace, limit.clamp(1, MAX_MEMORY_PAGE))
            .await?)
    }

    /// Ranks memories against a query.
    ///
    /// # Errors
    ///
    /// Returns `Storage` for a store failure.
    pub async fn search(
        &self,
        workspace: WorkspaceId,
        query: &str,
        limit: u32,
    ) -> Result<MemorySearch, MemoryServiceError> {
        Ok(self
            .store
            .search(workspace, query, limit.clamp(1, MAX_MEMORY_PAGE))
            .await?)
    }

    /// Permanently deletes a memory.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::NotFound`] for an absent or foreign memory.
    pub async fn forget(
        &self,
        workspace: WorkspaceId,
        id: MemoryId,
    ) -> Result<(), MemoryServiceError> {
        Ok(self.store.forget(workspace, id).await?)
    }
}

impl std::fmt::Debug for MemoryService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryService")
            .finish_non_exhaustive()
    }
}
