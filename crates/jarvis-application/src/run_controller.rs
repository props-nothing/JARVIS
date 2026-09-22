//! The native run controller: the loop that drives one run to a terminal state.
//!
//! This is the piece `first-vertical-slice.md` calls the native minimal run state
//! machine and that `BRN-005` recorded as *not done*: the state machine existed, but
//! nothing drove it. It is also where the preceding slices meet — it budgets context
//! through [`crate::context`](jarvis_domain::context), calls a provider through
//! [`ModelProvider`](crate::model::ModelProvider), and persists every transition with
//! its public event through [`crate::repository`].
//!
//! The architecture's "Native Runtime" list is the shape of [`RunController::execute`]:
//!
//! 1. receive the objective and bounded context;
//! 2. ask a model for either a final response or a typed tool intent;
//! 3. route tool intent through policy/execution — **not implemented**;
//! 4. append a bounded observation — **not implemented**;
//! 5. repeat within turn, token, cost, time, and tool budgets;
//! 6. produce a final response or an explicit waiting/failure state.
//!
//! Steps 3 and 4 are the tool fabric (Milestone 3). Because a model cannot yet get
//! its tool executed, this controller performs exactly **one** model turn and refuses
//! a tool intent with a typed, terminal outcome carrying
//! `run.tools_not_implemented`. That is deliberate: a controller that answered a tool
//! call with a fabricated observation would be scaffolding presented as a feature,
//! which `AGENTS.md` forbids. There is likewise no turn counter, because a loop that
//! cannot iterate a second time would be a claim with no behaviour behind it; the
//! repeat step arrives with the fabric that makes it meaningful.
//!
//! Four properties are structural:
//!
//! - **A state change is persisted with its event or not at all.** The controller
//!   never sets a state and then decides whether to record it; it calls the
//!   repository's fused `transition`, so the storage architecture's "persist the
//!   transition before publishing an event" holds by construction rather than by
//!   discipline.
//! - **Model frames go through the domain's state machine.** Ordering, duplicate, and
//!   terminal rules are enforced by [`ModelStreamState`], so the controller does not
//!   re-implement frame ordering and cannot disagree with the contract about it.
//! - **A stream without a terminal event is a failure.** The domain's `Interrupted`
//!   outcome maps to `Failed` with `run.stream_interrupted`, because the contract is
//!   explicit that a stream ending without a terminal is not success.
//! - **A cancelled call ends `Cancelled`, not `Failed`.** A provider reports
//!   cancellation as its own error, and mapping it to a failure would record the
//!   caller's own action as a fault.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use jarvis_domain::clock::Clock;
use jarvis_domain::context::manifest::ContextManifest;
use jarvis_domain::ids::{ConversationId, MessageId, ModelCallId, RunId, WorkspaceId};
use jarvis_domain::model::identity::ModelRef;
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::model::stream::{
    InputItem, InputItems, ModelCallRequest, ModelStreamEventKind, ModelStreamState,
    PortableSettings, Role, RouteRequirements, StreamAdmission, StreamOutcome, Usage,
};
use jarvis_domain::run::budget::{BudgetLimit, BudgetStatus, RunBudget};
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::retry::{FailureClass, FailureSite, RetryDecision};
use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;

use crate::cancellation::CancellationScope;
use crate::context_assembly::{self, RetainedItem};
use crate::live_events::StreamDeltaSink;
use crate::model::{ModelProvider, ModelStream, ProviderError};
use crate::repository::RepositoryError;
use crate::repository::conversation::{ConversationRepository, NewMessage};
use crate::repository::model_call::{
    ModelCallOutcome, ModelCallRepository, ModelCallState, NewModelCall,
};
use crate::repository::run::{
    EventVisibility, NewActivityEvent, RunRepository, RunWrite, StoredRun, TerminalOutcome,
};
use crate::request_context::RequestContext;

/// How many transcript messages are folded into one model call.
///
/// Bounded for the same reason every other collection here is: an unbounded read
/// would let one long conversation be pulled into memory, and the context budgeter
/// exists precisely because a transcript cannot be sent whole.
pub const MAX_TRANSCRIPT_MESSAGES: u32 = 200;

/// The context ceiling applied to a run that was created without one.
///
/// A default rather than no ceiling, for the same reason a run gets a default deadline:
/// an unset ceiling is not neutral, it means the prompt is bounded only by
/// [`MAX_TRANSCRIPT_MESSAGES`], and 200 messages can exceed any model's window. This is
/// comfortably above the window the controller reads and far below the domain's maximum,
/// so a default run behaves identically to a capped one while a caller that needs a
/// different bound can say so.
pub const DEFAULT_CONTEXT_TOKENS: u64 = 8_192;

/// Returns the context ceiling a run's budget implies.
///
/// Falls back to [`DEFAULT_CONTEXT_TOKENS`] rather than to a refusal, and the fallback is
/// named rather than written inline so the decision has one site. A stored ceiling the
/// domain would refuse is treated as absent instead of fatal: the value came from a budget
/// JARVIS wrote, so failing the run would report a storage problem as a caller's mistake,
/// while an absent ceiling is a state the default already covers.
fn effective_context_ceiling(budget: &RunBudget) -> u64 {
    budget.max_context_tokens.unwrap_or(DEFAULT_CONTEXT_TOKENS)
}

/// Why a run could not be driven to a terminal state.
///
/// Deliberately distinct from both the domain's transition refusal and the
/// repository's storage outcome: this is "the controller could not continue", and a
/// caller needs to tell that apart from "the store failed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    /// A run could not be read or advanced.
    Repository(RepositoryError),
    /// The provider refused to open a stream.
    Provider(ProviderError),
    /// The provider serves no models, so there is nothing to ask.
    NoModelServed,
    /// The model proposed a tool call and the tool fabric does not exist yet.
    ///
    /// Distinct from a fault because it is a *known* gap: `TLS-001` through `TLS-012`
    /// are what will satisfy it, and an operator should read it as "not implemented",
    /// not as "the run broke".
    ToolsNotImplemented {
        /// The tool the model asked for.
        tool_name: String,
    },
    /// The model stream ended without a terminal event.
    StreamInterrupted,
    /// The model stream carried a frame the state machine refused.
    StreamRejected {
        /// The stable, namespaced domain error code.
        code: &'static str,
    },
    /// Produced output could not be published as a durable public event.
    ///
    /// Distinct from a generic storage failure because it says exactly what was
    /// lost: a client that already saw part of an answer would otherwise be told the
    /// run failed for an unrelated reason, and the events it received would not
    /// reconcile with the run's stored state.
    OutputNotPersisted,
    /// The injected clock reported no usable instant.
    ClockUnavailable,
    /// The run exceeded its own deadline.
    ///
    /// Distinct from [`Provider`](Self::Provider) because the deadline is JARVIS's
    /// budget and the run is responsible for it, not the provider. An operator reading
    /// `run.deadline_exceeded` knows to look at the configured budget; reading a
    /// provider fault would send them to the provider's status page instead.
    DeadlineExceeded,
    /// The run exceeded a token or cost ceiling it was created under.
    ///
    /// Distinct from [`DeadlineExceeded`](Self::DeadlineExceeded) because the two have
    /// different remedies: a deadline breach means the run was too slow, while a
    /// consumption breach means it asked for too much output or took too expensive a
    /// route.
    BudgetExceeded {
        /// Which ceiling was breached.
        limit: BudgetLimit,
    },
    /// The model input could not be assembled from the stored conversation.
    ///
    /// Distinct from [`Repository`](Self::Repository) because the read succeeded — what
    /// failed is turning stored messages into a bounded, labelled request, and the
    /// remedies differ: a repository fault is a store problem, while this is a message
    /// or a budget that JARVIS cannot use.
    ContextUnassembled {
        /// The stable, namespaced error code.
        code: &'static str,
    },
    /// The caller cancelled the run.
    Cancelled,
}

impl ControllerError {
    /// Returns a message safe for the requesting principal.
    ///
    /// The same fixed text [`Display`](fmt::Display) produces, exposed so a caller
    /// that reports an error to a client does not have to format it and risk a
    /// developer-facing string reaching a user.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::Repository(_) => "The run could not be persisted.",
            Self::Provider(_) => "The model provider did not answer.",
            Self::NoModelServed => "No model is available to serve this run.",
            Self::ToolsNotImplemented { .. } => "Tool execution is not available yet.",
            Self::StreamInterrupted => "The model stream ended before it finished.",
            Self::StreamRejected { .. } => "The model stream was refused.",
            Self::OutputNotPersisted => "Produced output could not be recorded.",
            Self::ClockUnavailable => "The clock could not provide an instant.",
            Self::DeadlineExceeded => "The run exceeded its time budget.",
            Self::BudgetExceeded { limit } => match limit {
                BudgetLimit::OutputTokens => "The run exceeded its output-token budget.",
                BudgetLimit::Cost => "The run exceeded its cost budget.",
            },
            Self::ContextUnassembled { .. } => "The run's context could not be assembled.",
            Self::Cancelled => "The run was cancelled.",
        }
    }

    /// Returns whether retrying the same request unchanged could succeed.
    ///
    /// A tool-shaped refusal is explicitly **not** retryable: the capability is
    /// absent, so retrying it changes nothing. A provider error's own retryability is
    /// preserved rather than guessed at here.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::Provider(error) => error.retryable(),
            Self::Repository(error) => error.retryable(),
            Self::NoModelServed
            | Self::ToolsNotImplemented { .. }
            | Self::StreamInterrupted
            | Self::StreamRejected { .. }
            | Self::OutputNotPersisted
            | Self::ClockUnavailable
            | Self::DeadlineExceeded
            | Self::BudgetExceeded { .. }
            | Self::ContextUnassembled { .. }
            | Self::Cancelled => false,
        }
    }

    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Repository(error) => error.code(),
            Self::Provider(error) => error.code(),
            Self::NoModelServed => "run.no_model_served",
            Self::ToolsNotImplemented { .. } => "run.tools_not_implemented",
            Self::StreamInterrupted => "run.stream_interrupted",
            Self::StreamRejected { .. } => "run.stream_rejected",
            Self::OutputNotPersisted => "run.output_not_persisted",
            Self::ClockUnavailable => "run.clock_unavailable",
            Self::DeadlineExceeded => "run.deadline_exceeded",
            Self::BudgetExceeded { limit } => limit.code(),
            Self::ContextUnassembled { code } => code,
            Self::Cancelled => "run.cancelled",
        }
    }

    /// Returns the state a run should be left in for this outcome.
    ///
    /// Here rather than at each call site so that "a cancelled run ends `Cancelled`"
    /// is one decision. Every error maps to a *terminal* state: this controller has no
    /// wait-and-resume path yet, and reporting a non-terminal state it cannot leave
    /// would be worse than failing.
    #[must_use]
    pub const fn terminal_state(&self) -> RunState {
        match self {
            Self::Cancelled | Self::Provider(ProviderError::Cancelled) => RunState::Cancelled,
            Self::Repository(_)
            | Self::Provider(_)
            | Self::NoModelServed
            | Self::ToolsNotImplemented { .. }
            | Self::StreamInterrupted
            | Self::StreamRejected { .. }
            | Self::OutputNotPersisted
            | Self::ClockUnavailable
            | Self::DeadlineExceeded
            | Self::BudgetExceeded { .. }
            | Self::ContextUnassembled { .. } => RunState::Failed,
        }
    }

    /// Returns whether the outcome is a known, deliberate gap rather than a fault.
    ///
    /// Exposed so a caller can report "this capability is not built yet" differently
    /// from "something went wrong", which is the difference between an honest roadmap
    /// and an apparent bug.
    #[must_use]
    pub const fn is_unimplemented(&self) -> bool {
        matches!(self, Self::ToolsNotImplemented { .. })
    }
}

impl fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Fixed text naming no prompt content, no provider payload, and no path.
        let text = match self {
            Self::Repository(_) => "the run could not be persisted",
            Self::Provider(_) => "the model provider did not answer",
            Self::NoModelServed => "the provider serves no model",
            Self::ToolsNotImplemented { .. } => "tool execution is not implemented yet",
            Self::StreamInterrupted => "the model stream ended without a terminal event",
            Self::StreamRejected { .. } => "the model stream was refused",
            Self::OutputNotPersisted => "produced output could not be recorded",
            Self::ClockUnavailable => "the clock reported no usable instant",
            Self::DeadlineExceeded => "the run exceeded its time budget",
            Self::BudgetExceeded { limit } => match limit {
                BudgetLimit::OutputTokens => "the run exceeded its output-token budget",
                BudgetLimit::Cost => "the run exceeded its cost budget",
            },
            Self::ContextUnassembled { .. } => "the run's context could not be assembled",
            Self::Cancelled => "the run was cancelled",
        };
        formatter.write_str(text)
    }
}

/// What a driven run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// The run that finished.
    pub run_id: RunId,
    /// The state it ended in.
    pub state: RunState,
    /// The assembled answer, when the run completed.
    pub answer: Option<String>,
    /// The stable error code, when it failed or was cancelled.
    pub error_code: Option<String>,
}

/// The run a controller action applies to.
///
/// Grouped because every helper needs the same two values, and passing them
/// separately pushed several signatures past the argument limit and invited a
/// workspace/run-id transposition at a call site.
#[derive(Debug, Clone, Copy)]
struct RunRef {
    workspace: WorkspaceId,
    run_id: RunId,
}

/// One state change: its endpoints, its public event type, and its recorded reason.
///
/// The endpoints and their reason are always supplied together, so a mismatched pair
/// — a `Received -> Planning` edge carrying `Responding`'s event type — cannot be
/// written by accident.
///
/// A failure carries its typed outcome, and it is stored here rather than derived from `reason`
/// at the write. The two are different vocabularies on purpose: `reason` is a short operator label
/// that is safe to reword, while the outcome is a namespaced code a client switches on. Deriving
/// one from the other would let a cosmetic edit to a log label silently change a stable identifier.
#[derive(Debug, Clone, Copy)]
struct Step {
    from: RunState,
    to: RunState,
    event_type: &'static str,
    reason: &'static str,
    outcome: Option<TerminalOutcome>,
}

impl Step {
    /// A step that leaves the run in a non-failed state.
    const fn new(
        from: RunState,
        to: RunState,
        event_type: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            from,
            to,
            event_type,
            reason,
            outcome: None,
        }
    }

    /// A step that fails the run, carrying the code the run settles with.
    ///
    /// A separate constructor rather than a flag, so a failure **cannot** be written without its
    /// code: the compiler requires the argument at every call site that ends a run, which is the
    /// difference between this being enforced and being remembered. Before this existed the code
    /// was never written at all and `agent_runs.error_code` stayed `NULL` for every failed run,
    /// so `GET /api/v1/runs/{id}` could not report the "public error summary" the contract's run
    /// read requires.
    const fn failed(
        from: RunState,
        event_type: &'static str,
        reason: &'static str,
        code: &'static str,
    ) -> Self {
        Self {
            from,
            to: RunState::Failed,
            event_type,
            reason,
            outcome: Some(TerminalOutcome::failed(code)),
        }
    }
}

/// What one model-call attempt produced.
///
/// Three outcomes rather than a `Result` with an error, because "this attempt failed and
/// another is worth making" is not a failure of the *run*: the run is still live and
/// non-terminal when this is returned, and a caller that treated it as an error would fail
/// a run that is about to succeed.
#[derive(Debug)]
enum AttemptOutcome {
    /// The attempt produced a completed run.
    Completed(RunOutcome),
    /// The provider refused before accepting the call, and a repeat may help.
    Retryable {
        /// How the caller classified the failure.
        class: FailureClass,
        /// The provider error this attempt ended with.
        ///
        /// Carried so a refused retry reports the *underlying* failure rather than an
        /// invented one: an operator needs to know the provider was unavailable, not merely
        /// that the run gave up.
        error: ProviderError,
    },
}

/// Everything one model turn needs besides the run it belongs to.
struct ModelTurn<'a> {
    context: &'a RequestContext,
    conversation_id: ConversationId,
    /// The items the context budget selected, in the order it selected them.
    ///
    /// Replaces the raw transcript this used to carry: the request is built from what fit
    /// the budget rather than from everything that was read, so the two cannot disagree
    /// about what the model was given.
    items: &'a [RetainedItem],
    model: &'a ModelRef,
    cancel: &'a CancellationScope,
    /// The run's own budget. Carried into the turn because the deadline is a property of
    /// the *run*, not of this call: the same budget must bound every step, and deriving
    /// a fresh deadline per step would let a run outlive its own limit.
    ///
    /// The deadline is read from here rather than passed beside it. `NewRun::with_budget`
    /// derives the stored `deadline_at` column from this value, so carrying both into the
    /// turn would give one fact two sources that could disagree.
    budget: &'a RunBudget,
}

/// What draining a stream produced once it reached its terminal.
#[derive(Debug, Default)]
struct DrainedTurn {
    answer: String,
    tool_intent: Option<String>,
    /// The usage the provider reported, from a `usage.updated` frame or the terminal.
    ///
    /// Captured so the run can be judged against its token and cost ceilings. Before
    /// this, the ceilings reached the provider and nothing compared the result against
    /// them, so they bounded nothing.
    usage: Option<Usage>,
    /// Whether the provider reported any usage at all.
    ///
    /// Kept apart from `usage` being `None`, because "the provider said it used nothing"
    /// and "the provider said nothing" are different facts: the first is a measurement
    /// against which a ceiling can be checked, the second cannot check anything.
    usage_reported: bool,
}

/// Drives one durable run to a terminal state.
pub struct RunController {
    runs: Arc<dyn RunRepository>,
    conversations: Arc<dyn ConversationRepository>,
    model_calls: Arc<dyn ModelCallRepository>,
    /// Publishes live output deltas. A no-op sink is valid for a caller that
    /// replays the persisted events later rather than following them live.
    deltas: Arc<dyn StreamDeltaSink>,
    provider: Arc<dyn ModelProvider>,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for RunController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The ports are not printed: a provider can hold credentials and a repository
        // a connection string, and neither belongs in a diagnostic line.
        formatter
            .debug_struct("RunController")
            .field("served_models", &self.provider.models().len())
            .finish_non_exhaustive()
    }
}

impl RunController {
    /// Builds a controller over the given ports.
    #[must_use]
    pub fn new(
        runs: Arc<dyn RunRepository>,
        conversations: Arc<dyn ConversationRepository>,
        model_calls: Arc<dyn ModelCallRepository>,
        deltas: Arc<dyn StreamDeltaSink>,
        provider: Arc<dyn ModelProvider>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            runs,
            conversations,
            model_calls,
            deltas,
            provider,
            clock,
        }
    }

    /// Drives `run_id` to a terminal state and reports what it produced.
    ///
    /// The run must already exist in `Received`; the caller creates it so that
    /// creation is an auditable request of its own rather than a side effect of
    /// execution.
    ///
    /// # Errors
    ///
    /// Returns a [`ControllerError`] when the loop could not reach a terminal state.
    /// A run that *did* reach one is reported through a [`RunOutcome`] for a
    /// completion and through `Err` otherwise, so a caller cannot mistake "the run
    /// failed" for "the controller broke" — the error's own
    /// [`terminal_state`](ControllerError::terminal_state) says which state the run
    /// was left in.
    pub async fn execute(
        &self,
        context: &RequestContext,
        run_id: RunId,
        conversation_id: ConversationId,
        objective: &str,
        cancel: &CancellationScope,
    ) -> Result<RunOutcome, ControllerError> {
        let run = RunRef {
            workspace: context.workspace_id,
            run_id,
        };

        // A cancellation that arrived before any work is recorded as a terminal
        // transition rather than leaving the run in `Received`.
        //
        // An earlier version of this method returned without touching the run, on the
        // reasoning that recording work that never happened is a falsehood. That
        // reasoning was wrong about what the transition claims: moving to `Cancelled`
        // does not assert that work happened, it records that the request was
        // cancelled — which is exactly what occurred. Leaving the run in `Received`
        // made it permanently non-terminal, so a client polling it would wait forever
        // for an answer that had already been abandoned. The contract is explicit that
        // cancellation reaches a durable terminal transition.
        if cancel.is_cancelled() {
            // The state is read rather than assumed to be `Received`. The first version
            // hardcoded `Received` as the transition's origin, so a cancel arriving after
            // the run had already advanced was **refused** as an illegal edge and the
            // refusal was swallowed by the detached task — leaving the run in
            // `context_building` forever with the caller's cancellation recorded but
            // unexpressible. A live-daemon journey found it.
            let stored = self.load(run).await?;
            if !stored.state.is_terminal() {
                self.finish(
                    run,
                    Step::new(
                        stored.state,
                        RunState::Cancelled,
                        "run.cancelled",
                        "cancelled_before_start",
                    ),
                )
                .await?;
            }
            return Err(ControllerError::Cancelled);
        }

        // Received -> ContextBuilding. The transcript is read before the model call so
        // a run whose conversation is unreadable fails before it costs anything.
        //
        // Every intermediate step goes through `step_unless_cancelled`, so a cancellation
        // that arrives while the run is working is honoured at the next step rather than
        // only at the model-turn boundary. That is where the earlier per-state checks were
        // incomplete: a cancel landing between two steps was never consulted, so the run
        // continued to completion.
        self.step_unless_cancelled(
            run,
            Step::new(
                RunState::Received,
                RunState::ContextBuilding,
                "run.context_building",
                "context_build_requested",
            ),
            cancel,
        )
        .await?;

        let (budget, assembled) = self.build_context(run, conversation_id, objective).await?;

        // ContextBuilding -> Planning. There is no persisted plan artifact yet, which
        // the architecture permits: "Planning is a strategy, not a mandatory extra
        // model call." The state names that a decision was taken; it does not claim a
        // plan document exists.
        self.step_unless_cancelled(
            run,
            Step::new(
                RunState::ContextBuilding,
                RunState::Planning,
                "run.planning",
                "context_ready",
            ),
            cancel,
        )
        .await?;

        // The provider must name a model *before* the run can wait on one. Checking
        // here rather than inside the model turn means a misconfigured provider fails
        // the run from `Planning`; resolving it after `AwaitingModel` was entered left
        // the run waiting for a call that could never be made, in a state with no
        // legal way out.
        let model = match selected_model(self.provider.as_ref()) {
            Ok(model) => model,
            Err(error) => {
                self.finish(
                    run,
                    Step::failed(
                        RunState::Planning,
                        "run.failed",
                        "no_model_served",
                        error.code(),
                    ),
                )
                .await?;
                return Err(error);
            }
        };

        // Planning -> AwaitingModel.
        self.step_unless_cancelled(
            run,
            Step::new(
                RunState::Planning,
                RunState::AwaitingModel,
                "run.model_started",
                "model_call_requested",
            ),
            cancel,
        )
        .await?;

        // From here every failure must leave the run terminal, so the remainder runs
        // in a helper that owns the `AwaitingModel -> <terminal>` transition.
        self.ask_model(
            run,
            &ModelTurn {
                context,
                conversation_id,
                items: &assembled.items,
                model: &model,
                cancel,
                budget: &budget,
            },
        )
        .await
    }

    /// Reads the run's budget, the transcript, and the budgeted prompt.
    ///
    /// One method rather than inline, because the stage has three failure modes with three
    /// different honest answers — an unreadable conversation, an unusable budget, and a
    /// context that dropped the objective — and each has to leave the run **terminal** at
    /// `ContextBuilding`. Failing to do so was a real defect: the first version propagated
    /// the assembly error with `?`, so the run sat in `ContextBuilding` forever with no legal
    /// exit, the same missing-terminal-exit class this project has now found five times.
    ///
    /// The budget is read from the run rather than taken from the caller, because the deadline
    /// belongs to the run that was created: a caller that re-derived it here could disagree
    /// with what was stored, and recovery reads the stored value.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError::Repository`] for an unreadable conversation,
    /// [`ControllerError::ContextUnassembled`] for an unusable budget or an unreadable message
    /// label, and `run.context_objective_dropped` when the objective did not fit the budget.
    async fn build_context(
        &self,
        run: RunRef,
        conversation_id: ConversationId,
        objective: &str,
    ) -> Result<(RunBudget, context_assembly::AssembledInput), ControllerError> {
        let budget = self.load(run).await?.budget;

        // Bounded by *count*, which is why the assembly below is what bounds it by cost: a
        // window of 200 messages can still exceed any model's window.
        let transcript = self
            .conversations
            .load_messages(
                run.workspace,
                conversation_id,
                None,
                MAX_TRANSCRIPT_MESSAGES,
            )
            .await
            .map_err(ControllerError::Repository)?;

        // The prompt is assembled under the run's own context ceiling rather than sent as
        // every message read. This is also where the architecture's "untrusted retrieved
        // content is clearly delimited" rule becomes real: the transcript is content JARVIS
        // did not author, and `RetainedItem::to_input_item` is what delimits it.
        let assembled = match context_assembly::assemble(
            &transcript,
            objective,
            effective_context_ceiling(&budget),
            // The ceiling the run was created under, resolved from the stored policy and carried
            // in the budget. An absent ceiling means **no policy was in force**, and only then is
            // the permissive value used — with the manifest recording each item's label either
            // way, so the gap stays visible. The two cases are kept apart deliberately: a policy
            // that permits everything is a decision an operator made, while no policy at all is
            // not, and collapsing them would report an unconfigured daemon as a permissive one.
            //
            // Read from the budget rather than from the store on purpose. Re-reading per call
            // would let a policy edited mid-run change the ceiling a running run is judged
            // against, so two steps of one run could be held to different rules — and the run's
            // own record would no longer explain its own decision.
            budget
                .context_sensitivity_ceiling()
                .unwrap_or(Sensitivity::Restricted),
            self.now()?,
        ) {
            Ok(assembled) => assembled,
            Err(error) => {
                self.fail_context_building(run, "context_unassembled", error.code())
                    .await?;
                return Err(ControllerError::ContextUnassembled { code: error.code() });
            }
        };

        self.refuse_a_context_without_its_objective(run, &assembled.manifest)
            .await?;
        Ok((budget, assembled))
    }

    /// Moves a run from `ContextBuilding` to `Failed`.
    ///
    /// One helper for the three ways this stage can fail, so "a context failure leaves the run
    /// terminal" is a single decision rather than a transition repeated at each site — which is
    /// how one of them came to be missing.
    async fn fail_context_building(
        &self,
        run: RunRef,
        reason: &'static str,
        code: &'static str,
    ) -> Result<(), ControllerError> {
        self.finish(
            run,
            Step::failed(RunState::ContextBuilding, "run.failed", reason, code),
        )
        .await
    }

    /// Refuses to continue when the context budget dropped the run's own objective.
    ///
    /// This is the one exclusion worth failing for. Everything else that does not fit is
    /// recent conversation, and answering with slightly less context is a legitimate trade;
    /// answering with **no objective** is not, because the model would be asked to answer a
    /// question it was never given — which produces plausible text about the wrong thing, the
    /// failure mode hardest for a caller to notice and the reason a budget is worth enforcing
    /// rather than approximating.
    ///
    /// The manifest is what makes it checkable: it records the decision, so the caller is not
    /// left to infer from an empty prompt that something was dropped. The check runs **before**
    /// the run advances to `Planning`, so a run with no question in it fails before a provider
    /// is contacted and therefore before anything is billed.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError::ContextUnassembled`] with
    /// `run.context_objective_dropped` after moving the run to `Failed`.
    async fn refuse_a_context_without_its_objective(
        &self,
        run: RunRef,
        manifest: &ContextManifest,
    ) -> Result<(), ControllerError> {
        if manifest
            .included
            .iter()
            .any(|item| item.reference == context_assembly::OBJECTIVE_REFERENCE)
        {
            return Ok(());
        }
        self.fail_context_building(
            run,
            "context_objective_dropped",
            ControllerError::ContextUnassembled {
                code: "run.context_objective_dropped",
            }
            .code(),
        )
        .await?;
        Err(ControllerError::ContextUnassembled {
            code: "run.context_objective_dropped",
        })
    }

    /// Advances a run one state, and refuses to continue if a cancellation arrived.
    async fn step_unless_cancelled(
        &self,
        run: RunRef,
        step: Step,
        cancel: &CancellationScope,
    ) -> Result<(), ControllerError> {
        self.step(run, step).await?;
        if !cancel.is_cancelled() {
            return Ok(());
        }
        // `step.to` is where the run now is, and every working state has a `Cancelled`
        // edge. `Waiting` is the one exception a caller could reach here, and it has one
        // too, so this is total over the states a step can land in.
        self.finish(
            run,
            Step::new(
                step.to,
                RunState::Cancelled,
                "run.cancelled",
                "cancelled_during_step",
            ),
        )
        .await?;
        Err(ControllerError::Cancelled)
    }

    /// Performs the model turn, retrying a pre-acceptance failure within the run's budget.
    ///
    /// The split between a *retryable* failure and a *fatal* one is where the contract's
    /// retry-ownership rule is enforced:
    ///
    /// - a provider that refused **before accepting** the call left nothing behind, so the
    ///   run is not transitioned and another attempt can be made;
    /// - a failure **after acceptance** is ambiguous — output may exist, may have been
    ///   billed, and may have been recorded provider-side — so the run is failed and no
    ///   retry is attempted. `model-gateway.md` permits repeating an ambiguous request only
    ///   once provider idempotency is documented and used, which it is not.
    ///
    /// Before this, every attempt was attempt 1 and a transient failure ended the run. The
    /// retry-chain storage (`logical_call_id` plus `attempt`) existed since `BRN-004` and
    /// had never been used.
    async fn ask_model(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
    ) -> Result<RunOutcome, ControllerError> {
        // A run whose deadline had already passed when its turn began is refused before a
        // provider is contacted at all. Recording a model call for a request that was never
        // going to be made would leave an open attempt and spend nothing on purpose.
        if !turn.budget.permits_step_at(self.now()?) {
            self.finish(
                run,
                Step::failed(
                    RunState::AwaitingModel,
                    "run.failed",
                    "deadline_exceeded_before_call",
                    ControllerError::DeadlineExceeded.code(),
                ),
            )
            .await?;
            return Err(ControllerError::DeadlineExceeded);
        }

        // One logical call spans every attempt, so the chain is auditable as one operation
        // rather than as unrelated calls that happen to be adjacent.
        let logical_call_id = ModelCallId::from_uuid(uuid::Uuid::now_v7());
        let mut attempt = 1_u32;

        loop {
            match self
                .attempt_call(run, turn, logical_call_id, attempt)
                .await?
            {
                AttemptOutcome::Completed(outcome) => return Ok(outcome),
                AttemptOutcome::Retryable { class, error } => {
                    // The run is deliberately still non-terminal here: a retry needs a run
                    // it can continue, and failing it first would make the retry impossible.
                    let decision = RetryDecision::decide(
                        &turn.budget.retry,
                        class,
                        FailureSite::BeforeAcceptance,
                        attempt,
                        turn.budget,
                        self.now()?,
                    );
                    let RetryDecision::Retry {
                        attempt: next,
                        delay_ms,
                    } = decision
                    else {
                        // The policy or the budget refused the retry, so the run is failed
                        // now. A budget-caused refusal is reported as the deadline it is,
                        // because "the provider kept failing" and "the run ran out of time"
                        // send an operator to different places.
                        let reason = if decision.refused_by_budget() {
                            "retry_refused_by_budget"
                        } else {
                            "retry_refused"
                        };
                        // The code is the one the caller will be *returned*, so the run's durable
                        // row and the response cannot disagree about why it failed.
                        let returned = if decision.refused_by_budget() {
                            ControllerError::DeadlineExceeded
                        } else {
                            ControllerError::Provider(error)
                        };
                        self.finish(
                            run,
                            Step::failed(
                                RunState::AwaitingModel,
                                "run.failed",
                                reason,
                                returned.code(),
                            ),
                        )
                        .await?;
                        return Err(returned);
                    };
                    // The delay is bounded by the same `wait_bounded` every other wait uses,
                    // so a backoff can never outlive the run's deadline — which the decision
                    // already checked, and which this makes true of the actual wait rather
                    // than only of the arithmetic.
                    if self
                        .wait_bounded(
                            turn.budget,
                            tokio::time::sleep(Duration::from_millis(delay_ms)),
                        )
                        .await?
                        .is_none()
                    {
                        self.finish_expired(
                            run,
                            None,
                            "deadline_exceeded",
                            ControllerError::DeadlineExceeded.code(),
                        )
                        .await?;
                        return Err(ControllerError::DeadlineExceeded);
                    }
                    attempt = next;
                }
            }
        }
    }

    /// Makes one model-call attempt.
    ///
    /// Returns [`AttemptOutcome::Retryable`] only for a failure the provider reported
    /// *before* accepting the call, and only for a failure the caller classified as
    /// transient. Every other path leaves the run terminal and returns `Err`.
    async fn attempt_call(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
        logical_call_id: ModelCallId,
        attempt: u32,
    ) -> Result<AttemptOutcome, ControllerError> {
        // A fresh row per attempt, sharing the logical identity. A retry that reused the
        // row id would overwrite the first attempt's recorded outcome, which the
        // repository's uniqueness constraint exists to refuse.
        let call_id = ModelCallId::from_uuid(uuid::Uuid::now_v7());
        let started_at = self.now()?;
        self.model_calls
            .record_attempt(NewModelCall {
                id: call_id,
                workspace_id: run.workspace,
                run_id: run.run_id,
                logical_call_id,
                attempt,
                model: turn.model.clone(),
                request_fingerprint: None,
                started_at,
            })
            .await
            .map_err(ControllerError::Repository)?;

        let request = build_request(run.run_id, call_id, turn.items, turn.budget)?;

        // Bounded by the run's own budget. A provider that never answers must not hold
        // the run open: the deadline this run declares has to be an actual bound, and an
        // unbounded `await` here is exactly how a deadline stops meaning anything.
        let opened = self
            .wait_bounded(
                turn.budget,
                self.provider.open(turn.context, &request, turn.cancel),
            )
            .await?;

        let mut stream = match opened {
            // The bound elapsed before the provider answered.
            None => {
                self.finish_deadline_exceeded(run, Some(call_id)).await?;
                return Err(ControllerError::DeadlineExceeded);
            }
            Some(Ok(stream)) => stream,
            // The provider refused to open, so nothing can have been produced, billed, or
            // recorded — this is the only failure the run may retry.
            Some(Err(error)) => {
                return self.fail_open(run, call_id, error, logical_call_id).await;
            }
        };

        // The stream is drained first and the run advanced afterwards, so a failure to
        // persist a transition cannot be confused with a failure to read the stream.
        let drained = match self
            .drain_stream(run, call_id, stream.as_mut(), turn.budget)
            .await?
        {
            Ok(drained) => drained,
            // The stream was refused or a frame rejected; the run is already terminal.
            Err(error) => return Err(error),
        };

        // Cancellation is re-checked **after** the stream drains and before the run is
        // allowed to succeed. This is the last window in which a caller can cancel, and
        // without it a cancel that arrived while the final frames were being processed
        // was reported as `202` (cleanup in flight) and then ignored: the run completed
        // and claimed success for a request whose cancellation the daemon had accepted.
        //
        // `BufferedStream` reports a cancellation only while no terminal is queued, which
        // is correct for the *stream* — a terminal at the head means the call genuinely
        // finished — but it means the controller cannot rely on the stream to surface a
        // cancel that raced the terminal. Deciding here, on the run's own signal, is what
        // makes the outcome authoritative rather than the last frame's.
        if turn.cancel.is_cancelled() {
            self.finish(
                run,
                Step::new(
                    RunState::AwaitingModel,
                    RunState::Cancelled,
                    "run.cancelled",
                    "cancelled_after_output",
                ),
            )
            .await?;
            // The attempt is closed as cancelled rather than completed: the output was
            // produced, but recording it as a successful call would say the call's result
            // was the run's outcome, and the run's outcome is that the caller stopped it.
            self.record_call_outcome_with(
                run,
                call_id,
                ModelCallState::Cancelled,
                usage_of(&drained),
            )
            .await?;
            return Err(ControllerError::Cancelled);
        }

        self.finish_attempt(run, turn, call_id, drained).await
    }

    /// Judges a drained stream: a tool intent, a breached ceiling, or a completion.
    async fn finish_attempt(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
        call_id: ModelCallId,
        drained: DrainedTurn,
    ) -> Result<AttemptOutcome, ControllerError> {
        // Captured before anything is moved out of `drained`, because three paths below
        // need it and a later borrow would be a borrow of a partially moved value.
        let usage = usage_of(&drained);

        // A tool intent needs the fabric that does not exist yet. Refused with a
        // terminal, typed outcome rather than a fabricated observation.
        if let Some(tool_name) = drained.tool_intent {
            let _ = tool_name;
            self.finish(
                run,
                Step::failed(
                    RunState::AwaitingModel,
                    "run.failed",
                    "tool_fabric_unavailable",
                    ControllerError::ToolsNotImplemented {
                        tool_name: "unavailable".to_owned(),
                    }
                    .code(),
                ),
            )
            .await?;
            self.record_call_outcome_with(run, call_id, ModelCallState::Failed, usage)
                .await?;
            return Err(ControllerError::ToolsNotImplemented { tool_name });
        }

        // The run's ceilings are checked against what was actually used. This is what makes
        // `max_output_tokens` and `max_cost_microunits` bounds rather than numbers carried
        // in a request: before this, both reached the provider and nothing compared the
        // result against them.
        //
        // The run is failed and the answer discarded. A run that breached a ceiling and
        // still returned its output would make the ceiling advisory, and a caller who set
        // one could not tell an enforced limit from a cosmetic one.
        if let Some(reported) = &usage
            && let Some(limit) = turn.budget.exceeded_by(reported)
        {
            self.finish_expired(
                run,
                Some(call_id),
                "consumption_budget_exceeded",
                ControllerError::BudgetExceeded { limit }.code(),
            )
            .await?;
            return Err(ControllerError::BudgetExceeded { limit });
        }

        // The call's outcome is recorded before the run moves on, so a completed run
        // never leaves a model call open.
        let completed_at = self.now()?;
        self.record_call_outcome_with(run, call_id, ModelCallState::Completed, usage)
            .await?;

        // AwaitingModel -> Responding -> Completed, then the answer is stored.
        self.complete_run(
            run,
            drained,
            turn.conversation_id,
            completed_at,
            turn.cancel,
        )
        .await
        .map(AttemptOutcome::Completed)
    }

    /// Drives a successful run through `Responding` to `Completed` and stores the answer.
    async fn complete_run(
        &self,
        run: RunRef,
        drained: DrainedTurn,
        conversation_id: ConversationId,
        at: UtcTimestamp,
        cancel: &CancellationScope,
    ) -> Result<RunOutcome, ControllerError> {
        self.step(
            run,
            Step::new(
                RunState::AwaitingModel,
                RunState::Responding,
                "run.responding",
                "final_response",
            ),
        )
        .await?;

        // The last window in which a caller can cancel, and the one a live-daemon journey
        // pointed at. The run has produced its answer but has not committed it, so a
        // cancellation arriving here must still be truthful — the contract requires a
        // cancellation to reach a durable terminal transition, and `ACC-016` names
        // cancelling *during* delivery specifically. Without this check the run would move
        // to `Completed` and record success for a request whose cancellation the daemon had
        // already accepted as in-flight.
        if cancel.is_cancelled() {
            self.finish(
                run,
                Step::new(
                    RunState::Responding,
                    RunState::Cancelled,
                    "run.cancelled",
                    "cancelled_during_delivery",
                ),
            )
            .await?;
            // The answer is **not** stored: a cancelled run's output is not its outcome, and
            // persisting it as the assistant's message would put text into the transcript
            // that the run never delivered.
            return Err(ControllerError::Cancelled);
        }

        self.step(
            run,
            Step::new(
                RunState::Responding,
                RunState::Completed,
                "run.completed",
                "answer_delivered",
            ),
        )
        .await?;

        // The answer is persisted as the assistant's message so the transcript
        // survives the run, and its content is stored here rather than in an event
        // payload: the contract bounds public payloads and forbids prompt content in
        // them.
        let answer = drained.answer;
        self.store_answer(run, conversation_id, &answer, at).await?;

        Ok(RunOutcome {
            run_id: run.run_id,
            state: RunState::Completed,
            answer: Some(answer),
            error_code: None,
        })
    }

    /// Persists a completed answer as the assistant's message.
    async fn store_answer(
        &self,
        run: RunRef,
        conversation_id: ConversationId,
        answer: &str,
        at: UtcTimestamp,
    ) -> Result<(), ControllerError> {
        // An empty answer is not stored: an empty assistant message would be an item
        // the next turn's context would carry for no benefit.
        if answer.is_empty() {
            return Ok(());
        }
        self.conversations
            .append_message(
                run.workspace,
                NewMessage {
                    id: MessageId::from_uuid(uuid::Uuid::now_v7()),
                    conversation_id,
                    role: Role::Assistant,
                    content: answer.to_owned(),
                    content_schema_version: 1,
                    sensitivity: "internal".to_owned(),
                    source: "daemon".to_owned(),
                    created_at: at,
                }
                .validated()
                .map_err(ControllerError::Repository)?,
            )
            .await
            .map_err(ControllerError::Repository)
            .map(|_sequence| ())
    }

    /// Drains a stream to its terminal, leaving the run terminal on any refusal.
    ///
    /// Frames are admitted by the domain's state machine, so ordering and terminal
    /// rules are enforced in exactly one place rather than re-derived here. The return
    /// type is doubly nested because this method reports two different things: whether
    /// *draining* failed, and whether the *stream* failed. Collapsing them would make
    /// "the store broke" and "the model stream was refused" the same value.
    async fn drain_stream(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        stream: &mut dyn ModelStream,
        budget: &RunBudget,
    ) -> Result<Result<DrainedTurn, ControllerError>, ControllerError> {
        let mut state = ModelStreamState::new(call_id);
        let mut drained = DrainedTurn::default();
        loop {
            // Each frame wait is bounded by what is left of the budget, so a provider
            // that stalls mid-stream cannot hold the run open past its deadline. The
            // bound is re-derived per frame rather than fixed at the first one: a stream
            // that has already consumed most of its time has little left, and bounding it
            // by the original allowance would let it exceed the run's own limit.
            let next = self.wait_bounded(budget, stream.next_event()).await?;

            // The bound elapsed while waiting for a frame.
            let Some(frame) = next else {
                self.finish_deadline_exceeded(run, Some(call_id)).await?;
                return Ok(Err(ControllerError::DeadlineExceeded));
            };
            let event = match frame {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(error) => return self.fail_after_acceptance(run, call_id, error).await,
            };
            match state.accept(&event) {
                Ok(StreamAdmission::Accepted) => {}
                // Late frames are the contract's required behaviour, not a fault.
                Ok(StreamAdmission::IgnoredAfterTerminal) => continue,
                Err(error) => {
                    let code = ControllerError::StreamRejected { code: error.code() }.code();
                    self.finish(
                        run,
                        Step::failed(
                            RunState::AwaitingModel,
                            "run.failed",
                            "stream_frame_rejected",
                            code,
                        ),
                    )
                    .await?;
                    self.record_call_outcome(run, call_id, ModelCallState::Failed)
                        .await?;
                    return Ok(Err(ControllerError::StreamRejected { code: error.code() }));
                }
            }
            match &event.kind {
                ModelStreamEventKind::OutputTextDelta { item_id, delta } => {
                    // Each chunk is published as its own durable public event, so a
                    // client that reconnects replays the output it missed rather than
                    // only the final answer. A failure to publish fails the run: a
                    // stream whose output cannot be recorded is not one a client can
                    // trust, and completing anyway would claim an answer the audit
                    // trail does not contain.
                    let occurred_at = self.now()?;
                    self.deltas
                        .output_text_delta(
                            run.workspace,
                            run.run_id,
                            item_id.clone(),
                            delta.clone(),
                            occurred_at,
                        )
                        .await
                        .map_err(|_| ControllerError::OutputNotPersisted)?;
                    drained.answer.push_str(delta);
                }
                ModelStreamEventKind::ToolCallAdded { tool_name, .. } => {
                    // The first intent is recorded, but the loop keeps draining so the
                    // stream's terminal is still observed: abandoning a stream early
                    // would leave the provider's view and JARVIS's disagreeing.
                    drained.tool_intent.get_or_insert_with(|| tool_name.clone());
                }
                // Usage arrives on its own frame or with the terminal, and a provider may
                // send both. The last one wins rather than the first, because a later frame
                // is a revision and the terminal's block is the final one — taking the
                // first would under-count a provider that updates as it goes.
                //
                // Both arms assign the same thing, and they are merged rather than
                // duplicated: a divergence between two copies of "capture the usage" is
                // exactly how one arrival path would stop being captured.
                ModelStreamEventKind::UsageUpdated { usage }
                | ModelStreamEventKind::CallCompleted {
                    usage: Some(usage), ..
                } => {
                    drained.usage = Some(usage.clone());
                    drained.usage_reported = true;
                }
                _ => {}
            }
        }

        // A finished tool call with no completion, or a stream with no terminal, is
        // not success: `StreamOutcome::Interrupted` is the contract's word for it.
        match state.finish() {
            Ok(StreamOutcome::Terminal(_)) => Ok(Ok(drained)),
            Ok(StreamOutcome::Interrupted { .. }) => {
                self.finish(
                    run,
                    Step::failed(
                        RunState::AwaitingModel,
                        "run.failed",
                        "stream_interrupted",
                        ControllerError::StreamInterrupted.code(),
                    ),
                )
                .await?;
                self.record_call_outcome(run, call_id, ModelCallState::Failed)
                    .await?;
                Ok(Err(ControllerError::StreamInterrupted))
            }
            Err(error) => {
                self.finish(
                    run,
                    Step::failed(
                        RunState::AwaitingModel,
                        "run.failed",
                        "stream_unfinished",
                        ControllerError::StreamRejected { code: error.code() }.code(),
                    ),
                )
                .await?;
                self.record_call_outcome(run, call_id, ModelCallState::Failed)
                    .await?;
                Ok(Err(ControllerError::StreamRejected { code: error.code() }))
            }
        }
    }

    /// Returns the longest a single wait may take, or `None` when unbounded.
    ///
    /// The bound is the **minimum** of the remaining deadline and the step timeout,
    /// because both are limits the run declared and only the tighter one satisfies
    /// both. Computed fresh at each wait rather than once, so a stream that has already
    /// consumed half its time is bounded by what is left rather than by the original
    /// allowance.
    fn wait_bound(&self, budget: &RunBudget) -> Result<Option<Duration>, ControllerError> {
        let now = self.now()?;
        let deadline_bound = match budget.status_at(now) {
            // An expired deadline yields a zero bound, so the wait fails immediately
            // rather than being treated as "no limit" — the direction that would let a
            // run past its own deadline keep going.
            BudgetStatus::Expired { .. } => Some(Duration::ZERO),
            BudgetStatus::Remaining { millis, .. } => Some(Duration::from_millis(millis)),
            BudgetStatus::Unbounded => None,
        };
        let step_bound = budget.step_timeout_ms.map(Duration::from_millis);
        Ok(match (deadline_bound, step_bound) {
            (Some(deadline), Some(step)) => Some(deadline.min(step)),
            (Some(bound), None) | (None, Some(bound)) => Some(bound),
            (None, None) => None,
        })
    }

    /// Awaits `future`, bounded by the run's budget.
    ///
    /// Returns `None` when the bound elapsed. This is the method that makes a deadline
    /// an actual bound rather than a value carried in a request: without it a provider
    /// that never sends a frame would hold the run open forever, and the run's own
    /// `deadline_at` would describe a limit nothing enforced.
    ///
    /// With no bound configured the future is awaited as-is. That is a real state a
    /// directly-created run can be in, and reporting "timed out" for it would invent a
    /// limit the run never had.
    async fn wait_bounded<F, T>(
        &self,
        budget: &RunBudget,
        future: F,
    ) -> Result<Option<T>, ControllerError>
    where
        F: Future<Output = T>,
    {
        match self.wait_bound(budget)? {
            Some(bound) => Ok(tokio::time::timeout(bound, future).await.ok()),
            None => Ok(Some(future.await)),
        }
    }

    /// Ends a run whose provider refused to open a stream.
    ///
    /// Four outcomes arrive here and they are not the same fact:
    ///
    /// - a **cancellation** is the caller's own action, so the run ends `Cancelled` and
    ///   the caller's request is never recorded as a fault;
    /// - a **timeout** means the *budget* was exhausted, so the run ends `Failed` but is
    ///   reported as `DeadlineExceeded` — an operator reading a provider fault would look
    ///   at the provider's status page when the answer is in the run's budget;
    /// - a **transient** failure means the provider never accepted the call, so the attempt
    ///   is closed but the **run is left `AwaitingModel`** and `Retryable` is returned. The
    ///   run must stay live for a retry to be possible, and failing it here would make the
    ///   retry this path exists to allow impossible;
    /// - anything else is a genuine provider fault and keeps its own code.
    ///
    /// The recorded attempt is closed on every path, including the retryable one: an
    /// attempt left `Pending` would make a later reconciliation pass unable to tell whether
    /// a call was still outstanding.
    async fn fail_open(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        error: ProviderError,
        _logical_call_id: ModelCallId,
    ) -> Result<AttemptOutcome, ControllerError> {
        let cancelled = error == ProviderError::Cancelled;
        let timed_out = error == ProviderError::Timeout;
        let class = classify(error);

        // A transient failure leaves the run live. Nothing was accepted, so the run has not
        // failed — it merely has not succeeded yet, and the loop in `ask_model` decides
        // whether another attempt fits the policy and the budget.
        if !cancelled && !timed_out && class == FailureClass::Transient {
            self.record_call_outcome(run, call_id, ModelCallState::Failed)
                .await?;
            return Ok(AttemptOutcome::Retryable { class, error });
        }

        let reason = if timed_out {
            "deadline_exceeded"
        } else {
            "provider_refused"
        };
        // The returned error is built once, so its code is what the run's terminal row records.
        // Building it twice would let the stored code and the returned code diverge, which is the
        // one thing a durable error summary must not do.
        let returned = if cancelled {
            ControllerError::Cancelled
        } else if timed_out {
            ControllerError::DeadlineExceeded
        } else {
            ControllerError::Provider(error)
        };
        let outcome = if cancelled {
            ModelCallState::Cancelled
        } else {
            ModelCallState::Failed
        };

        // A cancellation is a terminal transition too, and it is deliberately *not* a failure:
        // `Step::failed` would record `run.failed`'s semantics on a cancelled run, so the
        // cancellation keeps the plain constructor and its outcome stays absent. The contract maps
        // the three terminal states one-to-one, and `run.cancelled` is not a failure code.
        if cancelled {
            self.finish(
                run,
                Step::new(
                    RunState::AwaitingModel,
                    RunState::Cancelled,
                    "run.cancelled",
                    reason,
                ),
            )
            .await?;
        } else {
            self.finish(
                run,
                Step::failed(
                    RunState::AwaitingModel,
                    "run.failed",
                    reason,
                    returned.code(),
                ),
            )
            .await?;
        }
        self.record_call_outcome(run, call_id, outcome).await?;

        Err(returned)
    }

    /// Ends a run whose provider failed *after* accepting the call.
    ///
    /// A distinct method from [`fail_open`](Self::fail_open) because the **safety** verdict
    /// differs: a failure after acceptance is an ambiguous request, so it is never retried
    /// whatever the error says about retryability, while a failure before acceptance may be.
    /// Keeping them apart is what stops a future edit from routing this case into the
    /// retryable branch, which is the one change the contract forbids here.
    ///
    /// This was a real defect: the mid-stream error was propagated with no transition at all,
    /// so the run was left in `AwaitingModel` — non-terminal, and indistinguishable from a run
    /// about to retry. A client would poll it forever and only a daemon restart would settle
    /// it, which is the same class as the missing terminal exit `BRN-007` recorded.
    async fn fail_after_acceptance(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        error: ProviderError,
    ) -> Result<Result<DrainedTurn, ControllerError>, ControllerError> {
        self.finish(
            run,
            Step::failed(
                RunState::AwaitingModel,
                "run.failed",
                "provider_failed_after_acceptance",
                ControllerError::Provider(error).code(),
            ),
        )
        .await?;
        self.record_call_outcome(run, call_id, ModelCallState::Failed)
            .await?;
        // The inner `Err` is the *stream's* failure, which is what the caller reports; the
        // outer `Ok` says draining itself worked. Collapsing them would make "the store
        // broke" and "the provider failed mid-stream" the same value.
        Ok(Err(ControllerError::Provider(error)))
    }

    /// Ends a run because its own budget expired.
    ///
    /// One method rather than a transition at each timeout site, so the state, the
    /// event, the reason, and the recorded call outcome cannot disagree about why the
    /// run stopped.
    /// Ends a run whose own deadline expired, recording what the caller will be told.
    ///
    /// A wrapper over [`finish_expired`](Self::finish_expired) so the deadline's reason and code
    /// are one decision. They were passed separately at four call sites, which is four chances for
    /// a run to record a code that disagrees with the error its caller receives — exactly the
    /// divergence the parameter exists to prevent.
    async fn finish_deadline_exceeded(
        &self,
        run: RunRef,
        call_id: Option<ModelCallId>,
    ) -> Result<(), ControllerError> {
        self.finish_expired(
            run,
            call_id,
            "deadline_exceeded",
            ControllerError::DeadlineExceeded.code(),
        )
        .await
    }

    /// Ends a run because its own budget expired.
    ///
    /// One method rather than a transition at each timeout site, so the state, the event, the
    /// reason, and the recorded call outcome cannot disagree about why the run stopped.
    async fn finish_expired(
        &self,
        run: RunRef,
        call_id: Option<ModelCallId>,
        reason: &'static str,
        code: &'static str,
    ) -> Result<(), ControllerError> {
        self.finish(
            run,
            Step::failed(RunState::AwaitingModel, "run.failed", reason, code),
        )
        .await?;
        if let Some(call_id) = call_id {
            // The attempt is closed as failed rather than left pending: a call that was
            // abandoned mid-stream is finished, and a pending row would make a later
            // reconciliation pass read it as still outstanding.
            self.record_call_outcome(run, call_id, ModelCallState::Failed)
                .await?;
        }
        Ok(())
    }

    /// Records a model call's terminal outcome.
    async fn record_call_outcome(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        state: ModelCallState,
    ) -> Result<(), ControllerError> {
        self.record_call_outcome_with(run, call_id, state, None)
            .await
    }

    /// Records a model call's terminal outcome along with the usage it reported.
    ///
    /// The usage is written here rather than in a separate call so a completed attempt and
    /// its consumption are one write: an attempt recorded as completed with no usage, then
    /// updated with usage, leaves a window in which a reconciliation pass reads a finished
    /// call as having consumed nothing — which is exactly the state a cost ceiling cannot
    /// check.
    async fn record_call_outcome_with(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        state: ModelCallState,
        usage: Option<Usage>,
    ) -> Result<(), ControllerError> {
        let completed_at = self.now()?;
        // The cost is lifted out of the usage block into its own column, because that is
        // where a cost query reads it. Both are written from one source, so they cannot
        // disagree.
        let estimated_cost_microunits = usage
            .as_ref()
            .and_then(|reported| reported.estimated_cost_microunits);
        self.model_calls
            .record_outcome(
                run.workspace,
                call_id,
                ModelCallOutcome {
                    state,
                    provider_request_id: None,
                    continuation_ref: None,
                    usage,
                    estimated_cost_microunits,
                    finish_reason: None,
                    error_code: None,
                    first_output_at: None,
                    completed_at: Some(completed_at),
                },
            )
            .await
            .map_err(ControllerError::Repository)
    }

    /// Advances a run one state, loading its current version first.
    async fn step(&self, run: RunRef, step: Step) -> Result<(), ControllerError> {
        let stored = self.load(run).await?;
        self.advance(run, stored.version, step).await.map(|_| ())
    }

    /// Moves a run to a terminal state.
    async fn finish(&self, run: RunRef, step: Step) -> Result<(), ControllerError> {
        self.step(run, step).await
    }

    /// Applies one transition, publishing its event in the same write.
    async fn advance(
        &self,
        run: RunRef,
        version: RunVersion,
        step: Step,
    ) -> Result<StoredRun, ControllerError> {
        let now = self.now()?;
        let transition = RunTransition::new(
            step.from,
            step.to,
            version,
            TransitionActor::Controller,
            TransitionReason::new(step.reason)
                .map_err(|error| ControllerError::StreamRejected { code: error.code() })?,
            now,
        );
        let sequence = self
            .runs
            .next_event_sequence(run.workspace, run.run_id)
            .await
            .map_err(ControllerError::Repository)?;
        let event = NewActivityEvent {
            run_id: run.run_id,
            sequence,
            event_type: step.event_type.to_owned(),
            payload_json: None,
            visibility: EventVisibility::Public,
            occurred_at: now,
        };
        // The outcome travels on the write rather than being scraped back out of the event, so
        // the transition, its event, and the outcome a client reads are one durable fact.
        let write = match step.outcome {
            Some(outcome) => RunWrite::new(&transition, event).failed_with(outcome),
            None => RunWrite::new(&transition, event),
        };
        self.runs
            .transition(run.workspace, write)
            .await
            .map_err(ControllerError::Repository)
    }

    /// Loads a run, mapping a storage failure.
    async fn load(&self, run: RunRef) -> Result<StoredRun, ControllerError> {
        self.runs
            .load(run.workspace, run.run_id)
            .await
            .map_err(ControllerError::Repository)
    }

    /// Returns the current instant from the injected clock.
    fn now(&self) -> Result<UtcTimestamp, ControllerError> {
        self.clock
            .now()
            .map_err(|_| ControllerError::ClockUnavailable)
    }
}

#[cfg(test)]
mod tests;

/// The model a controller would select for `provider`.
///
/// Exposed so a test can assert what the controller will ask without duplicating the
/// selection rule, and so `NoModelServed` has one definition.
///
/// # Errors
///
/// Returns [`ControllerError::NoModelServed`] when the provider names no model.
pub fn selected_model(provider: &dyn ModelProvider) -> Result<ModelRef, ControllerError> {
    provider
        .models()
        .first()
        .cloned()
        .ok_or(ControllerError::NoModelServed)
}

/// Classifies a provider failure for the retry policy.
///
/// This is the *only* place a provider error's retryability is turned into a policy input,
/// and it delegates to [`ProviderError::retryable`] rather than restating the rule: a
/// second copy of "which errors can be repeated" is exactly how the adapter and the
/// controller would come to disagree.
///
/// The classification stops at transient-versus-permanent. Whether a retry is *permitted*
/// also needs to know whether the provider had accepted the call, and that is a separate
/// input to the decision rather than something this function could infer from the error.
fn classify(error: ProviderError) -> FailureClass {
    if error.retryable() {
        FailureClass::Transient
    } else {
        FailureClass::Permanent
    }
}

/// Returns the usage a drained turn reported, if any.
///
/// A free function because it reads no port and holds no state, and because the same
/// expression is needed on three paths (the tool refusal, the ceiling check, and the
/// completion) where a divergence between copies would record a different usage for the
/// same call.
fn usage_of(drained: &DrainedTurn) -> Option<Usage> {
    drained.usage.clone()
}

/// Builds the normalized model request for one call.
///
/// Deliberately a free function: it reads no port and holds no state, so making it a
/// method would imply a dependency on the controller that does not exist.
///
/// The input is the **budgeted** items, not the raw transcript. That is what makes the
/// request bounded: everything in `items` already fit the run's context ceiling, and
/// anything that did not is recorded in the manifest as an exclusion rather than silently
/// dropped here. Placement decisions — including delimiting untrusted content — belong to
/// [`RetainedItem::to_input_item`], so this function cannot place an item the budget
/// refused.
fn build_request(
    run_id: RunId,
    call_id: ModelCallId,
    items: &[RetainedItem],
    budget: &RunBudget,
) -> Result<ModelCallRequest, ControllerError> {
    let mut input: Vec<InputItem> = vec![InputItem::SystemPolicyRef {
        // A reference, never inline policy text: the policy is resolved by JARVIS so a
        // request body cannot rewrite it, which is the same reason the context
        // budgeter carries references rather than content. The run's objective is
        // deliberately *not* placed here — that slot is JARVIS's own policy, and a
        // caller's text occupying it is exactly the confusion the architecture forbids.
        policy_ref: "system/default".to_owned(),
    }];
    input.extend(items.iter().map(RetainedItem::to_input_item));
    Ok(ModelCallRequest {
        call_id,
        run_id,
        route_requirements: RouteRequirements::text(),
        input: InputItems::new(input)
            .map_err(|error| ControllerError::StreamRejected { code: error.code() })?,
        tools: Vec::new(),
        output_schema: None,
        settings: PortableSettings::default(),
        // The run's budget, not an empty set of limits. Sending `None` here was the
        // defect: a run could carry a deadline that never reached the provider, so an
        // adapter honouring the contract's `limits.deadline` had nothing to honour and
        // a provider was free to wait indefinitely.
        limits: budget.call_limits(),
    })
}
