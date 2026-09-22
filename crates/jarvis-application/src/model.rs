//! The model provider port and the deterministic scripted provider.
//!
//! `docs/planning/first-vertical-slice.md` step 14 is one unit of work: a
//! normalized model request/event port **and** the deterministic scripted
//! provider that exercises it. They live together because the scripted provider
//! is what makes the port's contract assertable without a network, a credential,
//! or a paid call, and because the architecture requires the first provider
//! implementation to include the fake one — a paid live call must never be the
//! only test of JARVIS orchestration.
//!
//! The port is deliberately narrower than a provider SDK. A real adapter maps
//! its own protocol into [`ModelStreamEvent`] values at its boundary and maps
//! transport failures into [`ProviderError`]; nothing provider-specific reaches
//! this layer, which is what lets `ACC-015` swap one adapter for another without
//! touching the run or conversation schema.
//!
//! The stream the port returns is the [`ModelStream`] **trait**, and that is
//! load-bearing rather than stylistic: an adapter lives in another crate, so a
//! port returning a same-crate struct with a private buffer could only be
//! satisfied by the scripted provider in this file. An adapter implements
//! [`ModelStream`] itself or — preferably — sends frames into a channel and hands
//! the receiver to [`AdapterStream`], which implements the cancellation and
//! exactly-one-terminal rules once instead of per provider.
//!
//! Three properties are structural rather than documented:
//! - **The provider numbers frames, and starts at 1.** [`ScriptedProvider`]
//!   stamps each frame's sequence, so a script cannot accidentally produce a
//!   non-monotonic stream. A frame numbered
//!   [`Sequence::FIRST`](jarvis_domain::model::stream::Sequence::FIRST) is refused
//!   by the stream state machine, because that value is the state's initial
//!   "nothing seen yet" cursor — which is why the provider's first frame is 1 and
//!   not 0. [`ScriptStep::Raw`] exists solely so the contract's *negative* cases
//!   (a duplicate, a reorder, a frame for another call) can be produced at all.
//! - **The scripted provider never sleeps.** A "slow provider" that slept on a
//!   wall clock would make every test that uses it slow and non-deterministic, and
//!   a test that passed would only mean the machine was fast enough that day.
//!   Timing is *measured* (`BRN-011`) rather than simulated here: this provider
//!   emits a fixed, prepared sequence in an order that is a function of the script
//!   alone.
//! - **Cancellation is an input, not an afterthought.** A cancelled scope closes
//!   the stream with the normalized `call.cancelled` terminal instead of simply
//!   stopping. A bare stop would be indistinguishable from a provider that died,
//!   and the run would be recorded as interrupted rather than cancelled — the
//!   same "a correct cancellation looks like a fault" defect the domain's
//!   late-frame rule exists to prevent.
//!
//! A fourth property follows from the first three: **identifiers come from the
//! injected port**. Frames are stamped through [`IdGenerator`], never through a
//! raw UUID call, so a test can make a stream reproducible frame-for-frame while
//! production still gets `UUIDv7` ordering.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use jarvis_domain::ids::{IdGenerator, ModelCallId, ModelStreamEventId};
use jarvis_domain::model::identity::ModelRef;
use jarvis_domain::model::stream::{
    ModelCallRequest, ModelStreamEvent, ModelStreamEventKind, ProviderMetadata, Sequence,
};
use uuid::Uuid;

use crate::cancellation::CancellationScope;
use crate::request_context::RequestContext;

/// Why a provider could not open a stream.
///
/// The variants are JARVIS's own, not a provider's. An adapter translates its
/// transport's status codes and error bodies into these at its boundary, so a
/// provider's raw error text never becomes a JARVIS decision and never reaches a
/// log or a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    /// The credential was missing, malformed, or rejected.
    Authentication,
    /// The provider rate-limited this caller.
    RateLimited {
        /// The provider's requested delay, when it named one.
        retry_after_ms: Option<u64>,
    },
    /// The provider is unreachable or returned a server fault.
    Unavailable,
    /// The request is not valid for this provider.
    InvalidRequest,
    /// The provider refused the request for policy or content reasons.
    ///
    /// Kept apart from [`InvalidRequest`](Self::InvalidRequest): a refusal is a
    /// decision the repeat of which cannot change, while a malformed request is a
    /// defect in JARVIS.
    Refused,
    /// The provider's response could not be interpreted as a stream.
    Malformed,
    /// No model this provider serves can satisfy the request.
    NoRoute,
    /// The call was already cancelled when the provider was asked to open.
    ///
    /// An error rather than an empty stream, because an empty stream would be
    /// read downstream as an interrupted call — a fault — when the accurate
    /// answer is that a caller asked to stop.
    Cancelled,
}

impl ProviderError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Authentication => "model.provider_authentication",
            Self::RateLimited { .. } => "model.provider_rate_limited",
            Self::Unavailable => "model.provider_unavailable",
            Self::InvalidRequest => "model.provider_request_invalid",
            Self::Refused => "model.provider_refused",
            Self::Malformed => "model.provider_malformed",
            Self::NoRoute => "model.provider_no_route",
            Self::Cancelled => "model.provider_cancelled",
        }
    }

    /// Returns whether retrying the same call unchanged could succeed.
    ///
    /// This is the **single** place that decides retryability, because the
    /// architecture gives exactly one layer ownership of each retry. A blind
    /// retry of a refusal, a malformed request, or a rejected credential cannot
    /// succeed, and retrying a cancellation would do the opposite of what the
    /// caller asked.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Unavailable)
    }
}

/// Assigns the event identifiers and sequences for one stream.
///
/// A stream-scoped value rather than a free function so the counters a stream
/// needs cannot be shared between two concurrent calls, and so a cancellation
/// frame is stamped from the same source as the frames around it.
#[derive(Debug)]
struct FrameStamper<'a> {
    call_id: ModelCallId,
    ids: &'a dyn IdGenerator,
    next: Sequence,
}

impl<'a> FrameStamper<'a> {
    /// Starts at [`Sequence::FIRST`], whose successor is the provider's first frame.
    fn new(call_id: ModelCallId, ids: &'a dyn IdGenerator) -> Self {
        Self {
            call_id,
            ids,
            next: Sequence::FIRST,
        }
    }

    /// Builds the next frame, advancing the sequence.
    fn stamp(
        &mut self,
        kind: ModelStreamEventKind,
        metadata: Option<ProviderMetadata>,
    ) -> Result<ModelStreamEvent, ProviderError> {
        let sequence = self.next.next().map_err(|_| ProviderError::Malformed)?;
        self.next = sequence;
        Ok(ModelStreamEvent {
            call_id: self.call_id,
            event_id: ModelStreamEventId::from_uuid(self.ids.next_uuid("model_stream_event")),
            sequence,
            kind,
            provider_metadata: metadata,
        })
    }

    /// Builds a frame on an explicitly chosen sequence and call, advancing the
    /// cursor past that sequence.
    ///
    /// The call is taken from `call_id` rather than from the stream, because a raw
    /// frame's whole purpose is to reproduce a frame that does **not** belong to
    /// this call. Rewriting it here would make the contract's "a frame for another
    /// call is refused" case unproducible while appearing to support it.
    fn stamp_at(
        &mut self,
        call_id: ModelCallId,
        sequence: Sequence,
        kind: ModelStreamEventKind,
    ) -> ModelStreamEvent {
        self.next = sequence;
        ModelStreamEvent {
            call_id,
            event_id: ModelStreamEventId::from_uuid(self.ids.next_uuid("model_stream_event")),
            sequence,
            kind,
            provider_metadata: None,
        }
    }
}

/// Whether a stream may still produce a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamPhase {
    /// Frames may still arrive, so a cancellation may still be reported.
    Open,
    /// A terminal frame has been produced, or the stream is spent.
    Closed,
}

/// A boxed stream an adapter returns from [`ModelProvider::open`].
///
/// Aliased so the trait signature states the shape once: a concrete stream type
/// would make the port unimplementable outside this crate, and spelling the nested
/// `Pin<Box<dyn Future<...>>>` at every call site obscures that.
pub type BoxedModelStream<'a> = Box<dyn ModelStream + Send + 'a>;

/// The future [`ModelProvider::open`] returns.
pub type OpenResult<'a> =
    Pin<Box<dyn Future<Output = Result<BoxedModelStream<'a>, ProviderError>> + Send + 'a>>;

/// A stream of normalized events for one model call.
///
/// This is a **trait**, not a concrete type, and that is load-bearing rather than
/// stylistic. An adapter lives in another crate, so a port that returned a
/// same-crate struct with a private buffer could only be satisfied by the
/// scripted provider in this file — the trait's signature would look general
/// while in practice `BRN-003` could not implement it. The stream is therefore an
/// abstraction the adapter owns, and [`AdapterStream`] below is the shared
/// implementation an adapter should use so the cancellation and
/// exactly-one-terminal rules are not re-derived per provider.
pub trait ModelStream: Send {
    /// Returns the next event, or `None` when the stream has ended.
    ///
    /// # Errors
    ///
    /// Returns a [`ProviderError`] when the stream cannot continue. A failure that
    /// the *provider* reported is delivered as a terminal `call.failed` frame
    /// instead, so a caller can tell "the transport broke" from "the provider
    /// answered with an error".
    fn next_event(&mut self) -> NextEventFuture<'_>;
}

/// The future [`ModelStream::next_event`] returns.
pub type NextEventFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ModelStreamEvent>, ProviderError>> + Send + 'a>>;

/// The scripted provider's stream: a fixed, pre-built frame queue.
struct BufferedStream<'a> {
    events: VecDeque<ModelStreamEvent>,
    stamper: FrameStamper<'a>,
    cancel: CancellationScope,
    phase: StreamPhase,
}

impl fmt::Debug for BufferedStream<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The queued payloads are not printed: a stream can carry model output and
        // tool arguments, and a diagnostic line is not a place to duplicate them.
        // The counts are enough to debug a stalled stream.
        formatter
            .debug_struct("BufferedStream")
            .field("queued", &self.events.len())
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl<'a> BufferedStream<'a> {
    /// Builds a stream over already-stamped frames and its continuation stamper.
    fn new(
        events: Vec<ModelStreamEvent>,
        stamper: FrameStamper<'a>,
        cancel: CancellationScope,
    ) -> Self {
        // The phase starts `Open` even when the script already carries a terminal,
        // because "the script contains a terminal" and "the terminal has been
        // delivered" are different facts. Closing on the former would disable
        // cancellation for every fully scripted call — a caller who cancels while
        // the first frame is still queued would receive the script's
        // `call.completed` instead of `call.cancelled`, which is precisely the
        // "cancellation looks like success" defect this module exists to prevent.
        // The phase closes when a terminal is *handed out*.
        Self {
            events: events.into(),
            stamper,
            cancel,
            phase: StreamPhase::Open,
        }
    }
}

impl ModelStream for BufferedStream<'_> {
    fn next_event(&mut self) -> NextEventFuture<'_> {
        Box::pin(async move {
            // Cancellation is reported only while no terminal is queued. A terminal
            // already at the head means the call ended before the cancellation was
            // observed, and injecting a second terminal would break the contract's
            // "exactly one terminal event" rule.
            let terminal_queued = self
                .events
                .front()
                .is_some_and(ModelStreamEvent::is_terminal);
            if self.cancel.is_cancelled() && self.phase == StreamPhase::Open && !terminal_queued {
                self.phase = StreamPhase::Closed;
                let dropped = self.events.len();
                self.events.clear();
                let frame = self.stamper.stamp(
                    ModelStreamEventKind::CallCancelled {
                        late_frames_ignored: u64::try_from(dropped).unwrap_or(u64::MAX),
                    },
                    None,
                )?;
                return Ok(Some(frame));
            }
            let Some(event) = self.events.pop_front() else {
                self.phase = StreamPhase::Closed;
                return Ok(None);
            };
            if event.is_terminal() {
                self.phase = StreamPhase::Closed;
            }
            Ok(Some(event))
        })
    }
}

/// The stream a **real** adapter should use to feed normalized frames.
///
/// An adapter's transport produces frames over time, so it sends them into the
/// channel and hands the receiver here. The value of going through this type
/// rather than implementing [`ModelStream`] directly is that the two rules every
/// provider must get right are implemented **once**:
///
/// - **Cancellation is observed while waiting**, not only between frames, so a
///   cancelled call whose provider has gone quiet still ends promptly and with the
///   normalized `call.cancelled` terminal. Implementing this per adapter is how a
///   provider that stalls turns a cancellation into a hang.
/// - **Exactly one terminal is produced.** The adapter's own terminal passes
///   through; if the channel closes without one and the call was cancelled, the
///   cancellation is synthesized; if it closes without one and the call was *not*
///   cancelled, the stream ends with `None` — which the state machine records as
///   *interrupted*, the accurate answer for a transport that died.
///
/// The channel is bounded by the caller, because an unbounded one would let a fast
/// provider grow memory without limit while a slow consumer drained it.
pub struct AdapterStream {
    receiver: tokio::sync::mpsc::Receiver<ModelStreamEvent>,
    cancel: CancellationScope,
    call_id: ModelCallId,
    ids: Arc<dyn IdGenerator>,
    last_sequence: Sequence,
    phase: StreamPhase,
}

impl fmt::Debug for AdapterStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdapterStream")
            .field("phase", &self.phase)
            .field("last_sequence", &self.last_sequence)
            .finish_non_exhaustive()
    }
}

impl AdapterStream {
    /// Wraps a receiver an adapter will feed.
    #[must_use]
    pub fn new(
        call_id: ModelCallId,
        ids: Arc<dyn IdGenerator>,
        cancel: CancellationScope,
        receiver: tokio::sync::mpsc::Receiver<ModelStreamEvent>,
    ) -> Self {
        Self {
            receiver,
            cancel,
            call_id,
            ids,
            last_sequence: Sequence::FIRST,
            phase: StreamPhase::Open,
        }
    }

    /// Produces the single `call.cancelled` terminal and closes the stream.
    fn finish_cancelled(&mut self) -> Result<ModelStreamEvent, ProviderError> {
        self.phase = StreamPhase::Closed;
        let sequence = self
            .last_sequence
            .next()
            .map_err(|_| ProviderError::Malformed)?;
        Ok(ModelStreamEvent {
            call_id: self.call_id,
            event_id: ModelStreamEventId::from_uuid(self.ids.next_uuid("model_stream_event")),
            sequence,
            kind: ModelStreamEventKind::CallCancelled {
                late_frames_ignored: 0,
            },
            provider_metadata: None,
        })
    }
}

impl ModelStream for AdapterStream {
    fn next_event(&mut self) -> NextEventFuture<'_> {
        Box::pin(async move {
            if self.phase == StreamPhase::Closed {
                return Ok(None);
            }
            if self.cancel.is_cancelled() {
                return Ok(Some(self.finish_cancelled()?));
            }
            // `biased` puts cancellation first, so a call cancelled at the same
            // moment a frame arrives is reported as cancelled rather than as one
            // more frame of output.
            let received = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Ok(Some(self.finish_cancelled()?)),
                event = self.receiver.recv() => event,
            };
            let Some(event) = received else {
                self.phase = StreamPhase::Closed;
                return Ok(None);
            };
            if event.sequence > self.last_sequence {
                self.last_sequence = event.sequence;
            }
            if event.is_terminal() {
                self.phase = StreamPhase::Closed;
            }
            Ok(Some(event))
        })
    }
}

/// A model provider adapter.
///
/// Implementations are adapters: they own credentials, transport, and their
/// protocol's encoding, and they produce only normalized events. A provider
/// **never** decides policy, approves a tool call, or persists anything — it
/// proposes frames, and the deterministic layers above decide what they mean.
pub trait ModelProvider: Send + Sync {
    /// The models this provider serves.
    ///
    /// Returned as a slice so a provider serving many models does not allocate per
    /// call. Discovery here is not authorization: a caller learning that a model
    /// exists is granted nothing by it.
    fn models(&self) -> &[ModelRef];

    /// Opens a normalized stream for `request`.
    ///
    /// `context` carries the server-derived request identity; `cancel` is the scope
    /// the returned stream observes. Both are borrowed for the duration of the
    /// returned future so an adapter cannot outlive the request it serves.
    ///
    /// # Errors
    ///
    /// Returns a [`ProviderError`] when no stream can be opened. A failure that
    /// happens after a stream is open is delivered as a terminal `call.failed`
    /// frame instead, so exactly one place reports each failure shape.
    fn open<'a>(
        &'a self,
        context: &'a RequestContext,
        request: &'a ModelCallRequest,
        cancel: &'a CancellationScope,
    ) -> OpenResult<'a>;
}

/// One step of a scripted stream.
#[derive(Debug, Clone)]
pub enum ScriptStep {
    /// Emit this normalized payload; the provider assigns the sequence.
    Event(ModelStreamEventKind),
    /// Emit this frame exactly as supplied.
    ///
    /// The escape hatch that makes the contract's negative cases scriptable: a
    /// duplicate sequence, a reorder, or a frame addressed to a different call can
    /// otherwise never be produced, because the provider numbers frames
    /// monotonically and stamps them with the request's own call.
    Raw(ModelStreamEvent),
}

/// Generates deterministic event identifiers for a scripted stream.
///
/// The default for [`ScriptedProvider`], because a scripted provider whose frame
/// identifiers changed on every run would not be reproducible even though its
/// payloads were. It is deliberately **not** the production generator: the daemon
/// injects the `UUIDv7` adapter so real events stay time-ordered and unique across
/// processes.
#[derive(Debug, Default)]
pub struct CountingIds {
    next: std::sync::atomic::AtomicU64,
}

impl CountingIds {
    /// Creates a generator whose first value is 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl IdGenerator for CountingIds {
    fn next_uuid(&self, _kind: &'static str) -> Uuid {
        let value = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Uuid::from_u128(u128::from(value))
    }
}

/// A deterministic provider that replays a fixed script.
///
/// Determinism is by construction rather than by configuration: the emitted
/// sequence is a function of the script, the request, and the injected
/// [`IdGenerator`], and the provider reads no clock, socket, or environment
/// variable.
pub struct ScriptedProvider {
    models: Vec<ModelRef>,
    script: Vec<ScriptStep>,
    open_failure: Option<ProviderError>,
    ids: Arc<dyn IdGenerator>,
}

impl fmt::Debug for ScriptedProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The script's payloads are omitted for the same reason a stream's are:
        // they can carry prompt-derived model output.
        formatter
            .debug_struct("ScriptedProvider")
            .field("models", &self.models)
            .field("script_steps", &self.script.len())
            .field("open_failure", &self.open_failure)
            .finish_non_exhaustive()
    }
}

impl ScriptedProvider {
    /// Creates a provider that serves `model` and emits nothing.
    #[must_use]
    pub fn new(model: ModelRef) -> Self {
        Self {
            models: vec![model],
            script: Vec::new(),
            open_failure: None,
            ids: Arc::new(CountingIds::new()),
        }
    }

    /// Injects the identifier generator used to stamp frames.
    #[must_use]
    pub fn with_ids(mut self, ids: Arc<dyn IdGenerator>) -> Self {
        self.ids = ids;
        self
    }

    /// Adds a model this provider also serves.
    #[must_use]
    pub fn also_serving(mut self, model: ModelRef) -> Self {
        self.models.push(model);
        self
    }

    /// Appends a normalized payload to the script.
    #[must_use]
    pub fn emit(mut self, kind: ModelStreamEventKind) -> Self {
        self.script.push(ScriptStep::Event(kind));
        self
    }

    /// Appends a text delta for `item_id` to the script.
    #[must_use]
    pub fn emit_text(self, item_id: &str, delta: &str) -> Self {
        self.emit(ModelStreamEventKind::OutputTextDelta {
            item_id: item_id.to_owned(),
            delta: delta.to_owned(),
        })
    }

    /// Appends a frame exactly as supplied to the script.
    #[must_use]
    pub fn emit_raw(mut self, event: ModelStreamEvent) -> Self {
        self.script.push(ScriptStep::Raw(event));
        self
    }

    /// Removes every terminal step, so the script ends without a terminal event.
    ///
    /// A filter rather than a clear, so `interrupt()` may be applied to a script
    /// that already has output — the case that models a provider disconnecting
    /// mid-answer, which is the `ACC-012` shape.
    #[must_use]
    pub fn interrupt(mut self) -> Self {
        self.script.retain(|step| match step {
            ScriptStep::Event(kind) => !kind.is_terminal(),
            ScriptStep::Raw(event) => !event.is_terminal(),
        });
        self
    }

    /// Makes the provider fail before any stream is opened.
    #[must_use]
    pub fn fail_on_open(mut self, error: ProviderError) -> Self {
        self.open_failure = Some(error);
        self
    }
}

impl ModelProvider for ScriptedProvider {
    fn models(&self) -> &[ModelRef] {
        &self.models
    }

    fn open<'a>(
        &'a self,
        context: &'a RequestContext,
        request: &'a ModelCallRequest,
        cancel: &'a CancellationScope,
    ) -> OpenResult<'a> {
        Box::pin(async move {
            if let Some(error) = self.open_failure {
                return Err(error);
            }
            if cancel.is_cancelled() {
                // Refused rather than answered with an empty stream: an empty
                // stream would be read downstream as an interrupted call.
                return Err(ProviderError::Cancelled);
            }

            let mut stamper = FrameStamper::new(request.call_id, self.ids.as_ref());

            // The provider reports which model it selected, because that is a fact
            // only the provider knows. The frame is prepended rather than left to
            // the script, so a script cannot omit the start of its own stream and
            // produce output frames with no `call.started` to belong to.
            let mut frames = vec![stamper.stamp(
                ModelStreamEventKind::CallStarted {
                    model: self.models.first().cloned(),
                },
                Some(ProviderMetadata {
                    // A provider request id correlates a provider-side record with
                    // a JARVIS request. The scripted provider echoes the JARVIS
                    // request id, which is what makes "the trusted context reached
                    // the adapter" observable rather than merely claimed.
                    request_id: Some(context.request_id.to_string()),
                    continuation_ref: None,
                    provider_finish_reason: None,
                }),
            )?];

            for step in &self.script {
                let frame = match step {
                    ScriptStep::Event(kind) => stamper.stamp(kind.clone(), None)?,
                    // A raw frame keeps the call and sequence the test chose, but
                    // still receives a fresh identifier: an identifier is JARVIS's
                    // to assign, and a duplicated one would make two frames
                    // indistinguishable in an audit.
                    ScriptStep::Raw(event) => {
                        stamper.stamp_at(event.call_id, event.sequence, event.kind.clone())
                    }
                };
                frames.push(frame);
            }

            let stream: Box<dyn ModelStream + Send + 'a> =
                Box::new(BufferedStream::new(frames, stamper, cancel.clone()));
            Ok(stream)
        })
    }
}

#[cfg(test)]
mod tests;
