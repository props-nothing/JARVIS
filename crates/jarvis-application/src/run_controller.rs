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
use std::sync::Arc;

use jarvis_domain::clock::Clock;
use jarvis_domain::ids::{ConversationId, MessageId, ModelCallId, RunId, WorkspaceId};
use jarvis_domain::model::identity::ModelRef;
use jarvis_domain::model::stream::{
    CallLimits, ContentBlock, InputItem, InputItems, ModelCallRequest, ModelStreamEventKind,
    ModelStreamState, PortableSettings, Role, RouteRequirements, StreamAdmission, StreamOutcome,
};
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;

use crate::cancellation::CancellationScope;
use crate::model::{ModelProvider, ModelStream, ProviderError};
use crate::repository::RepositoryError;
use crate::repository::conversation::{ConversationRepository, NewMessage, StoredMessage};
use crate::repository::model_call::{
    ModelCallOutcome, ModelCallRepository, ModelCallState, NewModelCall,
};
use crate::repository::run::{
    EventVisibility, NewActivityEvent, RunRepository, RunWrite, StoredRun,
};
use crate::request_context::RequestContext;

/// How many transcript messages are folded into one model call.
///
/// Bounded for the same reason every other collection here is: an unbounded read
/// would let one long conversation be pulled into memory, and the context budgeter
/// exists precisely because a transcript cannot be sent whole.
pub const MAX_TRANSCRIPT_MESSAGES: u32 = 200;

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
    /// The injected clock reported no usable instant.
    ClockUnavailable,
    /// The caller cancelled the run.
    Cancelled,
}

impl ControllerError {
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
            Self::ClockUnavailable => "run.clock_unavailable",
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
            | Self::ClockUnavailable => RunState::Failed,
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
            Self::ClockUnavailable => "the clock reported no usable instant",
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
#[derive(Debug, Clone, Copy)]
struct Step {
    from: RunState,
    to: RunState,
    event_type: &'static str,
    reason: &'static str,
}

impl Step {
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
        }
    }
}

/// Everything one model turn needs besides the run it belongs to.
struct ModelTurn<'a> {
    context: &'a RequestContext,
    conversation_id: ConversationId,
    objective: &'a str,
    transcript: &'a [StoredMessage],
    model: &'a ModelRef,
    cancel: &'a CancellationScope,
}

/// What draining a stream produced once it reached its terminal.
#[derive(Debug, Default)]
struct DrainedTurn {
    answer: String,
    tool_intent: Option<String>,
}

/// Drives one durable run to a terminal state.
pub struct RunController {
    runs: Arc<dyn RunRepository>,
    conversations: Arc<dyn ConversationRepository>,
    model_calls: Arc<dyn ModelCallRepository>,
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
        provider: Arc<dyn ModelProvider>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            runs,
            conversations,
            model_calls,
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

        // A cancellation that arrived before any work is reported without touching
        // the run's state: there is nothing to clean up, and moving a run to
        // `Cancelled` for a request that never started would record work that did not
        // happen.
        if cancel.is_cancelled() {
            return Err(ControllerError::Cancelled);
        }

        // Received -> ContextBuilding. The transcript is read before the model call so
        // a run whose conversation is unreadable fails before it costs anything.
        self.step(
            run,
            Step::new(
                RunState::Received,
                RunState::ContextBuilding,
                "run.context_building",
                "context_build_requested",
            ),
        )
        .await?;

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

        // ContextBuilding -> Planning. There is no persisted plan artifact yet, which
        // the architecture permits: "Planning is a strategy, not a mandatory extra
        // model call." The state names that a decision was taken; it does not claim a
        // plan document exists.
        self.step(
            run,
            Step::new(
                RunState::ContextBuilding,
                RunState::Planning,
                "run.planning",
                "context_ready",
            ),
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
                    Step::new(
                        RunState::Planning,
                        RunState::Failed,
                        "run.failed",
                        "no_model_served",
                    ),
                )
                .await?;
                return Err(error);
            }
        };

        // Planning -> AwaitingModel.
        self.step(
            run,
            Step::new(
                RunState::Planning,
                RunState::AwaitingModel,
                "run.model_started",
                "model_call_requested",
            ),
        )
        .await?;

        // From here every failure must leave the run terminal, so the remainder runs
        // in a helper that owns the `AwaitingModel -> <terminal>` transition.
        self.ask_model(
            run,
            &ModelTurn {
                context,
                conversation_id,
                objective,
                transcript: &transcript,
                model: &model,
                cancel,
            },
        )
        .await
    }

    /// Performs the model turn and finishes the run.
    async fn ask_model(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
    ) -> Result<RunOutcome, ControllerError> {
        let call_id = ModelCallId::from_uuid(uuid::Uuid::now_v7());
        let started_at = self.now()?;
        self.model_calls
            .record_attempt(NewModelCall {
                id: call_id,
                workspace_id: run.workspace,
                run_id: run.run_id,
                logical_call_id: call_id,
                attempt: 1,
                model: turn.model.clone(),
                request_fingerprint: None,
                started_at,
            })
            .await
            .map_err(ControllerError::Repository)?;

        let request = build_request(run.run_id, call_id, turn.transcript, turn.objective)?;

        let mut stream = match self
            .provider
            .open(turn.context, &request, turn.cancel)
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                // A cancellation is the caller's own action, so the run ends
                // `Cancelled` and the error keeps its identity rather than becoming a
                // generic failure.
                let cancelled = error == ProviderError::Cancelled;
                let finish = if cancelled {
                    Step::new(
                        RunState::AwaitingModel,
                        RunState::Cancelled,
                        "run.failed",
                        "provider_refused",
                    )
                } else {
                    Step::new(
                        RunState::AwaitingModel,
                        RunState::Failed,
                        "run.failed",
                        "provider_refused",
                    )
                };
                self.finish(run, finish).await?;
                // The recorded attempt is closed even though the provider never
                // accepted it: an attempt left `Pending` would make a later
                // reconciliation pass unable to tell whether a call was outstanding.
                let outcome = if cancelled {
                    ModelCallState::Cancelled
                } else {
                    ModelCallState::Failed
                };
                self.record_call_outcome(run, call_id, outcome).await?;
                return Err(ControllerError::Provider(error));
            }
        };

        // The stream is drained first and the run advanced afterwards, so a failure to
        // persist a transition cannot be confused with a failure to read the stream.
        let drained = match self.drain_stream(run, call_id, stream.as_mut()).await? {
            Ok(drained) => drained,
            // The stream was refused or a frame rejected; the run is already terminal.
            Err(error) => return Err(error),
        };

        // A tool intent needs the fabric that does not exist yet. Refused with a
        // terminal, typed outcome rather than a fabricated observation.
        if let Some(tool_name) = drained.tool_intent {
            self.finish(
                run,
                Step::new(
                    RunState::AwaitingModel,
                    RunState::Failed,
                    "run.failed",
                    "tool_fabric_unavailable",
                ),
            )
            .await?;
            self.record_call_outcome(run, call_id, ModelCallState::Failed)
                .await?;
            return Err(ControllerError::ToolsNotImplemented { tool_name });
        }

        // The call's outcome is recorded before the run moves on, so a completed run
        // never leaves a model call open.
        let completed_at = self.now()?;
        self.record_call_outcome(run, call_id, ModelCallState::Completed)
            .await?;

        // AwaitingModel -> Responding -> Completed, then the answer is stored.
        self.complete_run(run, drained, turn.conversation_id, completed_at)
            .await
    }

    /// Drives a successful run through `Responding` to `Completed` and stores the answer.
    async fn complete_run(
        &self,
        run: RunRef,
        drained: DrainedTurn,
        conversation_id: ConversationId,
        at: UtcTimestamp,
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
    ) -> Result<Result<DrainedTurn, ControllerError>, ControllerError> {
        let mut state = ModelStreamState::new(call_id);
        let mut drained = DrainedTurn::default();
        while let Some(event) = stream
            .next_event()
            .await
            .map_err(ControllerError::Provider)?
        {
            match state.accept(&event) {
                Ok(StreamAdmission::Accepted) => {}
                // Late frames are the contract's required behaviour, not a fault.
                Ok(StreamAdmission::IgnoredAfterTerminal) => continue,
                Err(error) => {
                    self.finish(
                        run,
                        Step::new(
                            RunState::AwaitingModel,
                            RunState::Failed,
                            "run.failed",
                            "stream_frame_rejected",
                        ),
                    )
                    .await?;
                    self.record_call_outcome(run, call_id, ModelCallState::Failed)
                        .await?;
                    return Ok(Err(ControllerError::StreamRejected { code: error.code() }));
                }
            }
            match &event.kind {
                ModelStreamEventKind::OutputTextDelta { delta, .. } => {
                    drained.answer.push_str(delta);
                }
                ModelStreamEventKind::ToolCallAdded { tool_name, .. } => {
                    // The first intent is recorded, but the loop keeps draining so the
                    // stream's terminal is still observed: abandoning a stream early
                    // would leave the provider's view and JARVIS's disagreeing.
                    drained.tool_intent.get_or_insert_with(|| tool_name.clone());
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
                    Step::new(
                        RunState::AwaitingModel,
                        RunState::Failed,
                        "run.failed",
                        "stream_interrupted",
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
                    Step::new(
                        RunState::AwaitingModel,
                        RunState::Failed,
                        "run.failed",
                        "stream_unfinished",
                    ),
                )
                .await?;
                self.record_call_outcome(run, call_id, ModelCallState::Failed)
                    .await?;
                Ok(Err(ControllerError::StreamRejected { code: error.code() }))
            }
        }
    }

    /// Records a model call's terminal outcome.
    async fn record_call_outcome(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        state: ModelCallState,
    ) -> Result<(), ControllerError> {
        let completed_at = self.now()?;
        self.model_calls
            .record_outcome(
                run.workspace,
                call_id,
                ModelCallOutcome {
                    state,
                    provider_request_id: None,
                    continuation_ref: None,
                    usage: None,
                    estimated_cost_microunits: None,
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
        self.runs
            .transition(run.workspace, RunWrite::new(&transition, event))
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

/// Builds the normalized model request for one call.
///
/// Deliberately a free function: it reads no port and holds no state, so making it a
/// method would imply a dependency on the controller that does not exist.
fn build_request(
    run_id: RunId,
    call_id: ModelCallId,
    transcript: &[StoredMessage],
    objective: &str,
) -> Result<ModelCallRequest, ControllerError> {
    let mut items: Vec<InputItem> = vec![InputItem::SystemPolicyRef {
        // A reference, never inline policy text: the policy is resolved by JARVIS so a
        // request body cannot rewrite it, which is the same reason the context
        // budgeter carries references rather than content.
        policy_ref: "system/default".to_owned(),
    }];
    for message in transcript {
        items.push(InputItem::Message {
            role: message.role,
            blocks: vec![ContentBlock::Text {
                text: message.content.clone(),
            }],
        });
    }
    items.push(InputItem::Message {
        role: Role::User,
        blocks: vec![ContentBlock::Text {
            text: objective.to_owned(),
        }],
    });
    Ok(ModelCallRequest {
        call_id,
        run_id,
        route_requirements: RouteRequirements::text(),
        input: InputItems::new(items)
            .map_err(|error| ControllerError::StreamRejected { code: error.code() })?,
        tools: Vec::new(),
        output_schema: None,
        settings: PortableSettings::default(),
        limits: CallLimits {
            deadline: None,
            max_output_tokens: None,
            max_cost_microunits: None,
        },
    })
}
