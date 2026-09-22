//! The port for publishing a run's live output deltas.
//!
//! A model call is the one place a run produces output *while* it is still working,
//! and the local control API requires each chunk to be a durable public event with
//! its own sequence, so a client can replay what it missed. That makes a delta a
//! publish operation the controller needs but must not implement: persistence
//! belongs to the repository adapter, so the controller takes this port instead of a
//! database handle.
//!
//! Why a port rather than a repository call made directly:
//!
//! - `jarvis-application` cannot name `jarvis-infrastructure`, and the controller
//!   lives in the application layer.
//! - The contract's event type and payload shape are a wire concern. Keeping the
//!   payload assembly in the adapter means the controller never builds JSON, so it
//!   cannot accidentally put a prompt fragment or a secret into a public payload.
//!
//! A delta is deliberately **not** a state transition. The run stays in
//! `AwaitingModel` while the answer is produced, so routing a delta through
//! [`RunWrite`](crate::repository::run::RunWrite) would require inventing a self-edge
//! the domain correctly does not have.

use crate::repository::RepositoryFuture;
use jarvis_domain::ids::{RunId, WorkspaceId};
use jarvis_domain::time::UtcTimestamp;

/// The public event type an output-text delta is published under.
///
/// Owned by this port rather than by the wire crate, because the port is what both
/// an implementation and its callers must agree on. `jarvis-protocol` carries the
/// same literal for fixture and framing use, and a test in the infrastructure crate
/// asserts the two agree, so the duplication cannot drift silently — it caught a
/// mismatch on the first run, where this constant read `run.output_text_delta` and
/// the contract's example says `run.output_text.delta`.
pub const OUTPUT_TEXT_DELTA_EVENT: &str = "run.output_text.delta";

/// Publishes a run's live output deltas as durable public events.
///
/// Implementations must persist before returning, because the contract states that
/// the server persists an event before making it visible on the stream. A sink that
/// buffered in memory would make a reconnecting client miss output it had already
/// been shown.
pub trait StreamDeltaSink: Send + Sync {
    /// Publishes one output-text delta and returns its assigned sequence.
    ///
    /// The sequence is assigned by the implementation, not the caller: two deltas
    /// that both asked for "the next one" would otherwise be able to claim the same
    /// position, which the contract forbids and the store's uniqueness constraint
    /// would refuse.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`](crate::repository::RepositoryError::NotFound)
    /// for an absent or foreign run and
    /// [`RepositoryError::Query`](crate::repository::RepositoryError::Query) for a
    /// storage failure. The controller treats any failure here as a run failure: a
    /// stream that cannot record what it produced is not a stream a client can trust.
    fn output_text_delta(
        &self,
        workspace: WorkspaceId,
        run: RunId,
        item_id: String,
        delta: String,
        occurred_at: UtcTimestamp,
    ) -> RepositoryFuture<'_, u64>;
}

/// A sink that records nothing.
///
/// Valid for a caller that persists the answer at the end and replays it later
/// rather than following the run live, such as a scripted test that asserts final
/// state only. It is **not** the production default: the daemon composes the durable
/// sink, because a run whose stream a client already saw must have that output
/// recorded.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDeltaSink;

impl StreamDeltaSink for NoopDeltaSink {
    fn output_text_delta(
        &self,
        _workspace: WorkspaceId,
        _run: RunId,
        _item_id: String,
        _delta: String,
        _occurred_at: UtcTimestamp,
    ) -> RepositoryFuture<'_, u64> {
        // Sequence 0 is never a valid activity sequence, so a caller that mistook
        // this for the real sink and used the value would fail loudly rather than
        // write at a plausible position.
        Box::pin(async { Ok(0) })
    }
}
