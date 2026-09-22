//! The native agent run state machine.
//!
//! This is the typed form of the state machine in
//! `docs/architecture/agent-runtime.md`, which is the document that owns the
//! run controller's behavior. The rules it states in prose are enforced here:
//!
//! - **One state machine, explicit terminal and waiting states.** Every legal
//!   transition is listed in [`RunState::allowed_targets`], transcribed from the
//!   architecture diagram, so an edge the diagram does not contain is refused
//!   rather than invented at a call site.
//! - **A transition records its provenance.** [`RunTransition`] carries the actor,
//!   the reason, the expected prior version, and the instant, and
//!   [`RunLifecycle::apply`] returns a [`RunTransitionRecord`] describing what
//!   happened — because "the state changed" is not auditable on its own.
//! - **Optimistic concurrency.** A transition whose expected prior version does not
//!   match the current version is refused, so two workers cannot both advance one
//!   run by writing in sequence.
//! - **A terminal state is absorbing.** Nothing leaves `Completed`, `Failed`, or
//!   `Cancelled`, so a late worker cannot resurrect a finished run.
//!
//! The domain owns the *states*; it does not own the wire projection. The local
//! control API exposes a coarser set of states to clients and the finer states
//! here remain internal, which is why [`state::WireRunState`] exists as an explicit
//! mapping rather than as a `serde` rename on [`state::RunState`].

pub mod budget;
pub mod lifecycle;
pub mod recovery;
pub mod state;
