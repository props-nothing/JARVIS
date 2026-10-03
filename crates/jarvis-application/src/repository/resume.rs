//! The store of what a parked run needs in order to continue.
//!
//! A run that waits on a human decision is not running anywhere: the task that drove it has ended, and
//! the daemon may restart before anyone decides. What the run needs to continue — the batch of tool
//! calls the model proposed, which of them already produced results, and which one is waiting — lives
//! here, so resuming reads **durable state** rather than reconstructing a transcript.
//!
//! The record is written **before** the run is parked, so a run that reads as waiting always has the
//! state to continue from; a run whose record could not be written is failed rather than parked.
//!
//! Scoped by workspace like every other store here: a record of another workspace reads as absent.

use serde::{Deserialize, Serialize};

use jarvis_domain::ids::{ApprovalId, ConversationId, MessageId, RunId, WorkspaceId};

use super::RepositoryFuture;

/// The largest serialized record a store accepts, in bytes.
///
/// A bound rather than a courtesy: the record carries model-proposed arguments and tool output, both
/// of which are individually bounded but not jointly. A batch that exceeds this fails the run, which is
/// the honest answer — a run that cannot be resumed must not be parked.
pub const MAX_RESUME_RECORD_BYTES: usize = 1_048_576;

/// The format version of [`ResumeRecord`]. A store refuses a record it does not recognise.
pub const RESUME_RECORD_VERSION: u32 = 1;

/// One tool call the model proposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCall {
    /// The provider's canonical call id.
    pub call_id: String,
    /// The capability name the model used.
    pub capability: String,
    /// The complete argument document, as the provider sent it.
    pub arguments: String,
}

/// What a settled call produced, as the next model turn reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeObservation {
    /// The provider's canonical call id.
    pub call_id: String,
    /// The capability name the model used.
    pub capability: String,
    /// The complete argument document.
    pub arguments: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
    /// The bounded text the model reads.
    pub content: String,
}

/// Everything needed to continue one parked run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeRecord {
    /// The record's format version; see [`RESUME_RECORD_VERSION`].
    pub version: u32,
    /// The run it belongs to.
    pub run: RunId,
    /// The workspace the run is in.
    pub workspace: WorkspaceId,
    /// The conversation the run answers in.
    pub conversation: ConversationId,
    /// The approval the run is waiting on.
    pub approval: ApprovalId,
    /// The one-based model turn whose tool calls are parked.
    pub turn_index: u32,
    /// The run's objective, from which its context is rebuilt.
    pub objective: String,
    /// The stored message carrying the objective, when there is one.
    pub objective_message: Option<MessageId>,
    /// The turn's full batch of proposed calls, in order.
    pub calls: Vec<ResumeCall>,
    /// The index in `calls` of the call waiting on the approval.
    pub waiting_index: u32,
    /// The observations of `calls[..waiting_index]`, which settled before the wait.
    pub settled: Vec<ResumeObservation>,
}

/// A durable store of resume records, at most one per run.
pub trait RunResumeRepository: Send + Sync {
    /// Stores `record`, replacing the run's previous one.
    ///
    /// Replacing is the contract: a run that parks again after resuming has one current record, and a
    /// stale one would resume the wrong call.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`](super::RepositoryError::Conflict) when the serialized
    /// record exceeds [`MAX_RESUME_RECORD_BYTES`], and
    /// [`RepositoryError::Query`](super::RepositoryError::Query) for a driver failure.
    fn save(&self, record: &ResumeRecord) -> RepositoryFuture<'_, ()>;

    /// Reads the run's record, or `None` when it has none.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Corrupted`](super::RepositoryError::Corrupted) for a record that cannot
    /// be interpreted, including one of an unknown version.
    fn load(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> RepositoryFuture<'_, Option<ResumeRecord>>;

    /// Removes the run's record. Removing an absent one is not an error.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`](super::RepositoryError::Query) for a driver failure.
    fn discard(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, ()>;
}
