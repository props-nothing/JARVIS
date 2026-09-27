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

/// Notifies live followers that a run's durable stream has grown.
///
/// ## Why a notification and not the events themselves
///
/// The durable event store is the **only** source of truth for what a run has published, and this
/// channel deliberately carries **no payload**. It is a wake-up: a follower that receives one is
/// told "read again from your position", and it reads the events from the store. Three consequences
/// follow, and they are the reason for the shape rather than incidental:
///
/// - **A dropped notification cannot lose output.** The wake-up is not a queue of events, so a slow
///   consumer that lags is *not* missing events — it reads them all, because the store still has
///   them. A channel that carried the events would make a bounded buffer mean lost output, which the
///   contract forbids ("Per-client buffers are bounded. A slow consumer is disconnected; it can
///   replay from its last delivered event").
/// - **A burst of notifications cannot duplicate output.** Several deltas may publish between two
///   reads, so a follower may be woken once for a batch and once for nothing; both cases read from a
///   *sequence*, which makes the read idempotent. Delivering events directly would require
///   de-duplicating them against the resume position, a second implementation of the ordering the
///   store already enforces.
/// - **The store's ordering is the only ordering.** A follower's position is a sequence number read
///   from durable rows, so a live stream and a replayed stream cannot disagree about order.
///
/// ## Why concrete rather than a trait
///
/// There is exactly one mechanism, and a notification with no subscriber is already a no-op — a
/// broadcast `send` with no receivers discards the value. A trait would therefore add a
/// `Noop` implementation that behaves identically to the real one in the only case a test uses it,
/// which is the shape of a double that cannot fail but can drift.
#[derive(Debug, Clone)]
pub struct RunStreamNotifier {
    sender: tokio::sync::broadcast::Sender<RunStreamChanged>,
}

/// The number of wake-ups the channel retains before it reports a follower as lagged.
///
/// Bounded, like every other buffer here, and the bound is **not** a bound on events: a wake-up
/// carries no payload, so a lagged follower has lost no output — it treats the lag as a wake-up and
/// reads from its position. A small number is therefore safe, and the only cost is a follower doing
/// one read it needed anyway.
const WAKEUP_CAPACITY: usize = 64;

impl RunStreamNotifier {
    /// Creates a notifier and the channel it publishes on.
    #[must_use]
    pub fn new() -> Self {
        let (sender, _receiver) = tokio::sync::broadcast::channel(WAKEUP_CAPACITY);
        Self { sender }
    }

    /// Tells every live follower to read again.
    ///
    /// Failure to send is deliberately ignored, and it is not an error: `send` fails only when no
    /// receiver exists, which is the ordinary case of a run nobody is following. A follower that is
    /// *slow* rather than absent is reported by the channel as lagged, and it handles that as a
    /// wake-up — so the one case where ignoring the result would matter is the one the receiver,
    /// rather than the sender, is responsible for.
    pub fn notify(&self) {
        let _sent = self.sender.send(RunStreamChanged);
    }

    /// Subscribes a follower to wake-ups.
    #[must_use]
    pub fn subscribe(&self) -> LiveSubscription {
        LiveSubscription::new(self.sender.subscribe())
    }

    /// Returns the number of live followers, for diagnostics.
    #[must_use]
    pub fn follower_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for RunStreamNotifier {
    fn default() -> Self {
        Self::new()
    }
}

/// A follower's receiver of run-stream wake-ups.
///
/// A newtype rather than a bare `tokio::sync::broadcast::Receiver` so the lag error is normalised
/// here rather than at each call site: to a follower, "you fell behind" and "something changed" are
/// the same instruction, which is "read again from your position".
pub struct LiveSubscription {
    receiver: tokio::sync::broadcast::Receiver<RunStreamChanged>,
}

impl std::fmt::Debug for LiveSubscription {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Nothing about the channel is printed beyond its liveness: the queued count and the sender
        // count are both reachable only from the sender, and a follower's `Debug` has no reason to
        // carry either.
        formatter
            .debug_struct("LiveSubscription")
            .finish_non_exhaustive()
    }
}

impl LiveSubscription {
    /// Creates a subscription over `receiver`.
    #[must_use]
    pub fn new(receiver: tokio::sync::broadcast::Receiver<RunStreamChanged>) -> Self {
        Self { receiver }
    }

    /// Waits for the next wake-up, returning whether the publisher is still live.
    ///
    /// Returns as soon as a wake-up arrives, **or** as soon as the follower has fallen behind — the
    /// two are the same instruction. The alternative would be to surface the lag as an error, which
    /// would either end a stream that still has readable events or force every caller to handle a
    /// case with one sensible response.
    ///
    /// `false` means no further notification can ever arrive, because every publisher is gone. That
    /// is reported rather than hidden: a follower that kept waiting would be holding a connection
    /// open on the promise of an update nothing can send. In the daemon this cannot happen — the
    /// service holds a notifier for the process's lifetime — so it is reachable only by a follower
    /// that outlives its publisher, which is exactly the case worth being explicit about.
    pub async fn wake(&mut self) -> bool {
        match self.receiver.recv().await {
            // `Lagged` is a wake-up too: the follower is behind, so it reads and catches up. Both
            // outcomes mean the publisher is live.
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => false,
        }
    }
}

/// A wake-up that a run's durable event stream has grown.
///
/// Carries no payload by construction: the type has no fields, so a future edit cannot quietly start
/// sending event data through a channel that exists to avoid that. A struct rather than the unit
/// type so the channel's element type is named and self-documenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunStreamChanged;

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
