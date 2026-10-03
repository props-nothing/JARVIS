//! Sub-agent delegation: one run handing a task to another, and waiting for the answer.
//!
//! A single agent is as capable as one context window and one train of thought. A staff is more: a run can hand a
//! bounded task to a **child run**, which has its own conversation and its own model calls, and carry on with the
//! answer. This is the application half of the `agents.delegate@1` tool.
//!
//! Delegation adds no authority. The child is an ordinary run: same workspace, same principal, same tool pipeline,
//! so every tool call it makes is still validated, policy-checked, approved or refused exactly as the parent's
//! would be, and a call that needs a person parks the child like any other run. What delegation *does* add is cost
//! and fan-out, so it is bounded, and each bound is a rule rather than a hope:
//!
//! - **Depth**: a run at [`MAX_DELEGATION_DEPTH`] cannot delegate further. The depth is counted from the stored
//!   parent links, never from anything the model said.
//! - **Breadth**: a run may have at most [`MAX_ACTIVE_CHILDREN`] children active at once.
//! - **Time**: the child's deadline is clamped to the parent's, and the wait is bounded by the tool's own timeout.
//! - **Cancellation**: cancelling the parent while it waits stops the child, whichever way the child is waiting.
//!   The kill switch reaches children directly, because they are runs in the same workspace.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use jarvis_domain::ids::{CorrelationId, RequestId, RunId};
use jarvis_domain::model::stream::Role;
use jarvis_domain::run::state::RunState;

use super::{
    MAX_OBJECTIVE_BYTES, ParentLink, RunOptions, RunService, RunServiceError, RunSpawner,
    api_request_context,
};
use crate::cancellation::CancellationScope;
use crate::repository::RepositoryError;
use crate::tool_call::ToolCaller;

/// The deepest a delegation chain may go. A run at this depth cannot delegate; the root run is depth 0.
pub const MAX_DELEGATION_DEPTH: u32 = 2;

/// The most children one run may have active at once.
pub const MAX_ACTIVE_CHILDREN: usize = 4;

/// The longest answer carried back to the delegating run, in bytes.
pub const MAX_ANSWER_BYTES: usize = 16 * 1024;

/// How often the waiting run looks at its child.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the delegating run asks for.
#[derive(Debug, Clone, Copy)]
pub struct DelegationRequest<'a> {
    /// Who is delegating, from trusted context.
    pub caller: ToolCaller,
    /// The task the child is given as its objective.
    pub task: &'a str,
    /// The longest the delegating run will wait.
    pub timeout: Duration,
}

/// What a finished delegation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationOutcome {
    /// The child run, so the answer can be traced to its activity.
    pub child: RunId,
    /// The state the child ended in.
    pub state: RunState,
    /// The child's final answer, when it completed.
    pub answer: Option<String>,
    /// The child's error code, when it failed.
    pub error_code: Option<String>,
}

/// Why a delegation was refused or did not finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationError {
    /// The task is empty, over the bound, or contains a NUL byte.
    Invalid,
    /// The delegating run is already at [`MAX_DELEGATION_DEPTH`].
    DepthExceeded,
    /// The delegating run already has [`MAX_ACTIVE_CHILDREN`] children active.
    TooManyChildren,
    /// The delegating run no longer exists.
    ParentGone,
    /// The delegating run was cancelled while waiting; the child was stopped.
    Cancelled,
    /// The wait ran out; the child was stopped.
    TimedOut,
    /// No delegator is composed in this daemon.
    Unavailable,
    /// A storage fault.
    Storage,
}

impl DelegationError {
    /// A stable code for a diagnostic or a model-readable refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "delegation.invalid",
            Self::DepthExceeded => "delegation.depth_exceeded",
            Self::TooManyChildren => "delegation.too_many_children",
            Self::ParentGone => "delegation.parent_gone",
            Self::Cancelled => "delegation.cancelled",
            Self::TimedOut => "delegation.timed_out",
            Self::Unavailable => "delegation.unavailable",
            Self::Storage => "delegation.storage",
        }
    }
}

/// The future a delegation returns.
pub type DelegationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DelegationOutcome, DelegationError>> + Send + 'a>>;

/// Hands a task to a sub-agent run and waits for it.
pub trait Delegator: Send + Sync {
    /// Runs `request.task` as a child of `request.caller.run` and waits for it to end.
    ///
    /// # Errors
    ///
    /// See [`DelegationError`]. A child that *ends* failed or cancelled is an `Ok` outcome carrying its state, so
    /// the delegating model can read what happened and adapt.
    fn delegate<'a>(
        &'a self,
        request: DelegationRequest<'a>,
        cancel: &'a CancellationScope,
    ) -> DelegationFuture<'a>;
}

/// A late-bound slot for the delegator.
///
/// The tool executor is composed before the run service it delegates to exists, and the run service is composed
/// over the tool pipeline that holds the executor — a cycle. The slot breaks it: the executor holds the slot, and
/// the composition root fills it once, after the run service is built. An unfilled slot refuses the call
/// ([`DelegationError::Unavailable`]) rather than failing in some other way.
#[derive(Clone, Default)]
pub struct DelegatorSlot(Arc<OnceLock<Arc<dyn Delegator>>>);

impl DelegatorSlot {
    /// Creates an empty slot.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fills the slot, returning `false` when it was already filled.
    pub fn bind(&self, delegator: Arc<dyn Delegator>) -> bool {
        self.0.set(delegator).is_ok()
    }

    /// Returns the delegator, when one is bound.
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn Delegator>> {
        self.0.get().cloned()
    }
}

impl std::fmt::Debug for DelegatorSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DelegatorSlot")
            .field("bound", &self.0.get().is_some())
            .finish()
    }
}

/// The delegator over the real run service.
///
/// Holds the service **weakly**: the service owns the tool pipeline, which owns the executor, which owns the
/// slot this delegator is bound into, so a strong handle here would be a reference cycle. If the service is gone
/// the daemon is shutting down, and the call is refused as [`DelegationError::Unavailable`].
pub struct RunDelegator {
    service: Weak<RunService>,
    spawner: Arc<dyn RunSpawner>,
}

impl RunDelegator {
    /// Builds a delegator that creates child runs through `service` and drives them with `spawner`.
    #[must_use]
    pub fn new(service: &Arc<RunService>, spawner: Arc<dyn RunSpawner>) -> Self {
        Self {
            service: Arc::downgrade(service),
            spawner,
        }
    }
}

/// Counts the stored ancestors of a run, given its parent, which is the run's depth.
///
/// Bounded: it stops once the count passes the limit, so a corrupted or cyclic chain cannot loop.
async fn depth_of(
    service: &RunService,
    workspace: jarvis_domain::ids::WorkspaceId,
    parent_of_run: Option<RunId>,
) -> Result<u32, DelegationError> {
    let mut depth = 0;
    let mut cursor = parent_of_run;
    while let Some(id) = cursor {
        depth += 1;
        if depth > MAX_DELEGATION_DEPTH {
            break;
        }
        cursor = service
            .ports
            .runs
            .load(workspace, id)
            .await
            .map_err(|_| DelegationError::Storage)?
            .parent_run_id;
    }
    Ok(depth)
}

/// Stops a child that is still active.
async fn stop_child(
    service: &RunService,
    context: &crate::request_context::RequestContext,
    child: RunId,
    reason: &str,
) {
    if let Ok(stored) = service.ports.runs.load(context.workspace_id, child).await
        && !stored.state.is_terminal()
    {
        let _ = service.stop_active_run(context, &stored, reason).await;
    }
}

/// The child's last assistant message, bounded.
async fn final_answer(
    service: &RunService,
    context: &crate::request_context::RequestContext,
    conversation: jarvis_domain::ids::ConversationId,
) -> Option<String> {
    let messages = service
        .ports
        .conversations
        .load_messages(context.workspace_id, conversation, None, 500)
        .await
        .ok()?;
    let answer = messages
        .into_iter()
        .rev()
        .find(|message| message.role == Role::Assistant)?
        .content;
    Some(bounded_answer(&answer))
}
/// Cuts `text` at [`MAX_ANSWER_BYTES`] on a character boundary, marking the cut.
fn bounded_answer(text: &str) -> String {
    if text.len() <= MAX_ANSWER_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_ANSWER_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[answer cut at {MAX_ANSWER_BYTES} bytes]", &text[..end])
}

impl Delegator for RunDelegator {
    fn delegate<'a>(
        &'a self,
        request: DelegationRequest<'a>,
        cancel: &'a CancellationScope,
    ) -> DelegationFuture<'a> {
        Box::pin(async move {
            let DelegationRequest {
                caller,
                task,
                timeout,
            } = request;
            if task.trim().is_empty() || task.len() > MAX_OBJECTIVE_BYTES || task.contains('\0') {
                return Err(DelegationError::Invalid);
            }
            let Some(service) = self.service.upgrade() else {
                return Err(DelegationError::Unavailable);
            };
            let service = service.as_ref();
            let context = api_request_context(
                caller.workspace,
                caller.principal,
                RequestId::from_uuid(uuid::Uuid::now_v7()),
                CorrelationId::from_uuid(uuid::Uuid::now_v7()),
            );
            let parent = match service.ports.runs.load(caller.workspace, caller.run).await {
                Ok(parent) => parent,
                Err(RepositoryError::NotFound) => return Err(DelegationError::ParentGone),
                Err(_) => return Err(DelegationError::Storage),
            };
            // The depth is the number of stored ancestors the delegating run has; the child would be one deeper.
            let depth = depth_of(service, caller.workspace, parent.parent_run_id).await?;
            if depth >= MAX_DELEGATION_DEPTH {
                return Err(DelegationError::DepthExceeded);
            }
            let active = service
                .ports
                .runs
                .active_runs(caller.workspace)
                .await
                .map_err(|_| DelegationError::Storage)?;
            let children = active
                .runs
                .iter()
                .filter(|run| run.parent_run_id == Some(caller.run))
                .count();
            if children >= MAX_ACTIVE_CHILDREN {
                return Err(DelegationError::TooManyChildren);
            }

            let created = service
                .create(
                    &context,
                    None,
                    task,
                    &format!("delegate-{}", uuid::Uuid::now_v7()),
                    RunOptions::new().with_parent(ParentLink {
                        run: caller.run,
                        deadline: parent.deadline_at,
                    }),
                    self.spawner.as_ref(),
                )
                .await
                .map_err(|error| match error {
                    RunServiceError::Invalid { .. } => DelegationError::Invalid,
                    _ => DelegationError::Storage,
                })?;

            let started = tokio::time::Instant::now();
            let stored = loop {
                if cancel.is_cancelled() {
                    stop_child(service, &context, created.run_id, "parent_cancelled").await;
                    return Err(DelegationError::Cancelled);
                }
                if started.elapsed() >= timeout {
                    stop_child(service, &context, created.run_id, "delegation_timed_out").await;
                    return Err(DelegationError::TimedOut);
                }
                let stored = service
                    .ports
                    .runs
                    .load(caller.workspace, created.run_id)
                    .await
                    .map_err(|_| DelegationError::Storage)?;
                if stored.state.is_terminal() {
                    break stored;
                }
                tokio::select! {
                    () = cancel.cancelled() => {}
                    () = tokio::time::sleep(POLL_INTERVAL) => {}
                }
            };

            let answer = if stored.state == RunState::Completed {
                final_answer(service, &context, created.conversation_id).await
            } else {
                None
            };
            Ok(DelegationOutcome {
                child: created.run_id,
                state: stored.state,
                answer,
                error_code: stored.error_code,
            })
        })
    }
}
