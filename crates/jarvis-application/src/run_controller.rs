//! The native run controller: the loop that drives one run to a terminal state.
//!
//! This is the piece `first-vertical-slice.md` calls the native minimal run state
//! machine and that `BRN-005` recorded as *not done*: the state machine existed, but
//! nothing drove it. It is also where the preceding slices meet — it budgets context
//! through [`crate::context`](jarvis_domain::context), calls a provider through
//! [`ModelProvider`], and persists every transition with
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
use jarvis_domain::ids::{
    ApprovalId, ConversationId, MessageId, ModelCallId, RunId, ToolCallId, WorkspaceId,
};
use jarvis_domain::model::identity::ModelRef;
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::model::stream::{
    FinishReason, InputItem, InputItems, ModelCallRequest, ModelStreamEvent, ModelStreamEventKind,
    ModelStreamState, PortableSettings, Role, RouteRequirements, StreamAdmission, StreamOutcome,
    Usage,
};
use jarvis_domain::run::budget::{BudgetLimit, BudgetStatus, RunBudget, RunRoute};
use jarvis_domain::run::lifecycle::RunTransition;
use jarvis_domain::run::retry::{FailureClass, FailureSite, RetryDecision};
use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;
use jarvis_domain::tool::call::{ContentBlock, ToolArguments, ToolCallIntent, ToolResultBody};
use jarvis_domain::tool::error_class::ToolErrorClass;

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

/// The greatest number of model turns one run may take.
///
/// A turn is one model call plus the tool calls it proposed, so this bounds a run's **loop** rather
/// than its size: without it a model that keeps proposing tool calls would run until the deadline,
/// and a deadline failure reports "too slow" for a run that was in fact looping. Ten is generous
/// against a real task and small enough that a runaway loop is caught in seconds, not minutes.
///
/// A constant rather than a stored budget field, deliberately: adding a persisted `max_turns` to
/// `RunBudget` changes a serialized shape every stored run carries, and a migration for a bound that
/// no operator has asked to configure would be ceremony. When a caller *does* need to set it, it
/// becomes a budget field with a migration — and this constant is the default that field would take.
pub const MAX_MODEL_TURNS: u32 = 10;

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
    /// The provider's own terminal frame reported the call as failed.
    ///
    /// Distinct from [`Provider`](Self::Provider), which is a *port* failure — an error from
    /// `open`, before any stream existed. This is a well-formed `call.failed` frame the provider
    /// chose to send **inside** a stream it had already opened, so the request is ambiguous
    /// (post-acceptance) and is never retried.
    ///
    /// **Before this existed the controller treated every terminal as success.** `finish_attempt`
    /// recorded `ModelCallState::Completed` and drove the run to `Completed` without ever reading
    /// the finish reason, so a call the provider reported as failed produced a **successful** run
    /// whose answer was the partial output the provider had abandoned.
    ModelCallFailed,
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
    /// The run took more model turns than it is allowed.
    ///
    /// Distinct from [`DeadlineExceeded`](Self::DeadlineExceeded) because the remedies differ: a
    /// deadline breach means the run was too slow, while this means it was **looping** — a model
    /// proposing tool call after tool call — and an operator reading `run.turn_budget_exhausted`
    /// knows to look at the model's behaviour rather than at the budget's numbers.
    TurnBudgetExceeded,
    /// The tool pipeline could not be reached at all.
    ///
    /// Distinct from a tool that refused the call, which is an **observation** the model receives:
    /// this is a fault in JARVIS's own composition — an unreachable store, an unavailable clock —
    /// and it ends the run because no observation could be produced.
    ToolRefused {
        /// The stable, namespaced code from the tool service.
        code: &'static str,
    },
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
            Self::ModelCallFailed => "The model call did not complete.",
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
            Self::TurnBudgetExceeded => "The run took more turns than it is allowed.",
            Self::ToolRefused { .. } => "A tool call could not be dispatched.",
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
            | Self::ModelCallFailed
            | Self::StreamRejected { .. }
            | Self::OutputNotPersisted
            | Self::ClockUnavailable
            | Self::DeadlineExceeded
            | Self::BudgetExceeded { .. }
            | Self::ContextUnassembled { .. }
            | Self::Cancelled
            | Self::TurnBudgetExceeded
            // A dispatch fault is not retryable: the same composition fails the same way, and a
            // repeat would spend a budget on a certain failure.
            | Self::ToolRefused { .. } => false,
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
            Self::ModelCallFailed => "run.model_call_failed",
            Self::StreamRejected { .. } => "run.stream_rejected",
            Self::OutputNotPersisted => "run.output_not_persisted",
            Self::ClockUnavailable => "run.clock_unavailable",
            Self::DeadlineExceeded => "run.deadline_exceeded",
            Self::BudgetExceeded { limit } => limit.code(),
            // Both carry a code from an inner value rather than naming a constant of their own, so
            // they share an arm. Merging them is safe here for the reason it must be checked
            // elsewhere: neither has a variant-specific meaning to preserve.
            Self::ContextUnassembled { code } | Self::ToolRefused { code } => code,
            Self::Cancelled => "run.cancelled",
            Self::TurnBudgetExceeded => "run.turn_budget_exhausted",
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
            | Self::ModelCallFailed
            | Self::StreamRejected { .. }
            | Self::OutputNotPersisted
            | Self::ClockUnavailable
            | Self::DeadlineExceeded
            | Self::BudgetExceeded { .. }
            | Self::ContextUnassembled { .. }
            | Self::TurnBudgetExceeded
            // A dispatch fault leaves the run `Failed` for the same reason a store fault does: it is
            // a fault rather than a decision, and a run whose tool pipeline is unreachable cannot
            // produce an answer.
            | Self::ToolRefused { .. } => RunState::Failed,
        }
    }

    /// Returns whether the outcome is a known, deliberate gap rather than a fault.
    ///
    /// Exposed so a caller can report "this capability is not built yet" differently
    /// from "something went wrong", which is the difference between an honest roadmap
    /// and an apparent bug.
    #[must_use]
    pub const fn is_unimplemented(&self) -> bool {
        // The one value that survives is the tool-fabric gap **reached through a controller with no
        // pipeline composed**. It is deliberately still classified here: a deployment without a
        // tool pipeline is reporting an absent capability rather than a fault, and `TLS-012`'s
        // pipeline is what removes the code from a fully composed daemon.
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
            Self::ModelCallFailed => "the model call did not complete",
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
            Self::TurnBudgetExceeded => "the run exceeded its turn budget",
            Self::ToolRefused { .. } => "a tool call could not be dispatched",
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
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    from: RunState,
    to: RunState,
    event_type: &'static str,
    reason: &'static str,
    outcome: Option<TerminalOutcome>,
    /// The **requester's** reason for a cancellation, when one was recorded.
    ///
    /// Distinct from [`reason`](Self::reason), and the distinction is the whole point of this field:
    /// `reason` is the controller's own closed-set label saying *which code path* cancelled the run,
    /// while this is the caller's text saying *why they asked*. The contract is explicit that both
    /// reach the terminal event — "The reason travels to the terminal event" — and until this the
    /// caller's reason reached nothing at all: it lived on the `CancellationScope` and was never
    /// persisted, so a cancelled run's durable record could not answer the question the caller had
    /// already been asked. It is `Option` because an internal cancellation (the provider reporting
    /// its own cancellation, the supervisor ending a run) has no requester and must not invent one,
    /// and it is **owned** because the caller's text comes from a scope rather than a literal — which
    /// is also why this is the one field on `Step` that can contain text needing escaping.
    requester_reason: Option<String>,
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
            requester_reason: None,
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
            requester_reason: None,
        }
    }

    /// Builds the public payload for a cancellation.
    ///
    /// **This is a closed-set payload, and it is the same shape the failure payload has.** The reason
    /// it was withheld for several rounds is worth recording, because the reasoning was wrong and the
    /// wrongness was expensive: the note said the cancellation's reason "is caller-supplied text, so a
    /// public event needs escaping; hand-rolling a JSON escaper is the ad-hoc string manipulation the
    /// architecture forbids, and adding `serde_json` as a real dependency is a research-gate change."
    ///
    /// Every part of that is false here. The reason a cancellation actually publishes is
    /// [`Step::reason`], which is a **`&'static str` literal from a closed set** — `cancelled_before_start`,
    /// `cancelled_during_step`, `cancelled_after_output`, `cancelled_before_acceptance`,
    /// `provider_reported_call_cancelled`, `cancelled_during_delivery` — so there is nothing to escape,
    /// exactly as with the failure code. The genuinely caller-supplied cancel reason lives on the
    /// `CancellationScope` in the HTTP layer and never reaches this layer at all; it is not the value
    /// being published. So no serializer is needed, no dependency gate applies, and the payload is
    /// built the same way as the failure's.
    ///
    /// That distinction — a **closed-set operator label** versus **caller-supplied text** — is what
    /// should have been checked before the gap was written down. The generalisation worth keeping: a
    /// "this needs escaping" claim is a claim about *which value* is on the wire, and it has to name
    /// that value rather than assume it.
    const fn cancelled_from(from: RunState, reason: &'static str) -> Self {
        Self {
            from,
            to: RunState::Cancelled,
            event_type: "run.cancelled",
            reason,
            // No `outcome`: `outcome` is a *failure* code, and the contract maps the three terminal
            // states one-to-one — `run.cancelled` is not a failure. The published payload is derived
            // from `reason` and `requester_reason` in `payload` instead, so this stays what it is.
            outcome: None,
            requester_reason: None,
        }
    }

    /// Attaches the requester's reason, which the terminal event publishes alongside the label.
    ///
    /// A builder rather than an argument on every constructor, because only the cancellation paths
    /// have a requester: a failure has nobody who asked for it, and threading an `Option` through
    /// every `Step` construction would make three constructors carry a value that is structurally
    /// absent for all of them.
    #[must_use]
    fn with_requester_reason(mut self, reason: Option<String>) -> Self {
        self.requester_reason = reason;
        self
    }

    /// Returns the public payload this step's event carries, when it carries one.
    ///
    /// A **failed** run's event carries the code it settled with, because a client that only
    /// follows the event stream otherwise learns that a run stopped without learning why: the code
    /// is on the run's own row, so a streaming client would have to make a separate read to see it.
    /// The terminal event is the last thing such a client receives, which makes it the wrong place
    /// to omit the answer.
    ///
    /// `retryable` is always `false`, and that is a fact about the outcome rather than a default:
    /// reaching this code means the run is terminal, and a terminal run is not going to be retried
    /// by resending anything — the retry policy is consulted *before* a failure is written, so a
    /// run that arrives here has already had its retry decision made.
    ///
    /// A **cancellation** carries two reasons, and the pair is the point:
    ///
    /// - **`reason`** is the controller's own closed-set label saying *which code path* cancelled the
    ///   run — `cancelled_before_start`, `cancelled_during_step`, `provider_reported_call_cancelled`,
    ///   and so on. It needs no escaping and cannot be forged, so it is what a client branches on.
    /// - **`requester_reason`** is the *caller's* text, present only when a caller actually asked for
    ///   the cancellation. The contract requires it ("The reason travels to the terminal event"), and
    ///   it was reaching **nothing** before this: it lived on the `CancellationScope` and was never
    ///   persisted, so the durable record could not answer the question the caller had already been
    ///   asked. This is the value that genuinely needs escaping.
    ///
    /// The `label` field keeps both discoverable: a client that switches on a known key is unaffected,
    /// and the human-readable reason is there for an operator. An internal cancellation (the provider
    /// reporting its own, the supervisor ending a run) has no requester and publishes only the label,
    /// because inventing a reason for a cancellation nobody requested would attribute a decision to
    /// somebody who did not make it.
    ///
    /// Built by hand rather than through a serializer, following
    /// [`crate::recovery::recovery_payload`] — the application layer has `serde_json` only as a
    /// dev-dependency. That is safe here for the same reason it is safe there, with **one value held
    /// to a higher standard**: the code and the label come from closed sets, and the requester's text
    /// is escaped by [`escape_json_string`], which is proven against `serde_json` by a cross-check
    /// test over adversarial inputs rather than trusted because it looks right.
    ///
    /// The generalisable lesson, recorded because it was learned twice: "this needs escaping" is a
    /// claim about *which value* is on the wire, and it has to name that value. This payload contains
    /// both a value that does not need escaping and one that does.
    fn payload(&self) -> Option<String> {
        if let Some(TerminalOutcome { code }) = self.outcome {
            return Some(format!("{{\"code\":\"{code}\",\"retryable\":false}}"));
        }
        // Only the cancellation's own event type gets a cancellation payload: a client that saw a
        // `cancelled` reason on any other event would be reading a reason for something that did not
        // happen.
        if self.to == RunState::Cancelled && self.event_type == "run.cancelled" {
            return Some(match &self.requester_reason {
                Some(requester) => format!(
                    "{{\"reason\":\"{}\",\"label\":\"{}\"}}",
                    escape_json_string(requester),
                    self.reason,
                ),
                None => format!("{{\"reason\":\"{}\"}}", self.reason),
            });
        }
        None
    }
}

/// Renders `value` as the **contents** of a JSON string, escaped.
///
/// A hand-written escaper because the application layer has no JSON dependency, and it is the same
/// function shape the CLI and the diagnostics manifest already use for the same reason. It is
/// **verified against `serde_json` by a cross-check test over adversarial inputs** rather than
/// trusted: a value that merely *looks* escaped and is not is exactly the defect that produces an
/// event a client cannot parse, and the test is what makes the claim about this function a
/// measurement instead of an opinion.
///
/// The escapes are the ones JSON requires: `"` and `\`, the five short forms for the characters that
/// have them, and `\uXXXX` for every other control character. A control character is escaped rather
/// than dropped, so the document stays valid without silently altering what the caller wrote — an
/// operator reading a cancellation reason needs the bytes the caller sent, not a cleaned-up version.
fn escape_json_string(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if control < '\u{20}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out
}

/// What one model-call attempt produced.
///
/// Three outcomes rather than a `Result` with an error, because "this attempt failed and
/// another is worth making" is not a failure of the *run*: the run is still live and
/// non-terminal when this is returned, and a caller that treated it as an error would fail
/// a run that is about to succeed. The third arm is a tool call, which is not a failure of anything.
#[derive(Debug)]
enum AttemptOutcome {
    /// The attempt produced a final answer, or tool results for a further turn.
    Completed(TurnOutcome),
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

/// What a provider reported about one attempt, as recorded on its outcome row.
///
/// Three fields rather than three parameters, for the reason the outcome struct itself is
/// named rather than positional: the two `Option` values here are of different types and
/// would be transposable in a call — and a transposition would record a usage block as a
/// finish reason, which reads downstream as a corrupted row rather than as a mistake.
///
/// `Default` is the honest value for an attempt that failed before the provider reported
/// anything, so a failure path says "nothing was reported" by omission rather than by naming
/// three `None`s at each site.
#[derive(Debug, Clone, Default)]
struct RecordedOutcome {
    /// The usage the provider reported, when it reported any.
    usage: Option<Usage>,
    /// Why the provider stopped, when it reached a terminal.
    finish_reason: Option<FinishReason>,
    /// The delivery timing observed for the attempt.
    ///
    /// Grouped rather than three sibling fields so no call site can pass a last-output instant
    /// where a first-output one belongs. The two are the same type, so that transposition would
    /// compile and would compute the chunk-spread interval backwards — a plausible wrong number
    /// in a measurement whose entire purpose is being trustworthy. Grouping also gives the failure
    /// paths one argument to carry instead of three, which is what keeps `finish_expired` and
    /// `fail_after_acceptance` inside clippy's argument budget without a suppression.
    delivery: DeliveryTiming,
    /// The provider's own request identifier, when it reported one.
    ///
    /// Recorded with the outcome rather than in a second write, for the same reason the usage
    /// and the finish reason are: a call whose provider id is stored separately from its
    /// terminal would have a window in which a finished call has no correlatable provider
    /// reference. **The controller wrote `None` here on every path while the schema document
    /// claimed the port that stores the column reads it back** — the provider id reached the
    /// start frame's metadata and then stopped, exactly the shape `finish_reason` had before
    /// `BRN-017`.
    provider_request_id: Option<String>,
}

/// When an attempt's output arrived, and how many pieces it arrived in.
///
/// This is the input an incremental-delivery measurement is computed from, and it is a value
/// rather than three parameters because the three belong together: a last-output instant without
/// a first is half an interval, and a delta count without an instant describes output nothing can
/// place in time. The grouping is also what keeps a failure path's signature short enough to read.
#[derive(Debug, Clone, Copy, Default)]
struct DeliveryTiming {
    /// The instant the first output delta arrived, when any did.
    first_output_at: Option<UtcTimestamp>,
    /// The instant the last output delta arrived, when any did.
    last_output_at: Option<UtcTimestamp>,
    /// How many output deltas were observed.
    output_delta_count: u32,
}

impl DeliveryTiming {
    /// Returns the timing observed while draining `drained`.
    ///
    /// One reader for the three fields, so a call site cannot carry two of them and silently drop
    /// the third — which is how `first_output_at` reached some paths and not others before.
    fn of(drained: &DrainedTurn) -> Self {
        Self {
            first_output_at: drained.first_output_at,
            last_output_at: drained.last_output_at,
            output_delta_count: drained.output_delta_count,
        }
    }
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
    /// The tool observations produced by the **previous** turn, in call order.
    ///
    /// Empty on the first turn. Carried on the turn rather than appended to `items`, because the
    /// budgeted items are a **durable** decision about the transcript and an observation is
    /// in-flight context: folding one into the other would make the manifest describe content the
    /// budget never selected, and the observation would then be re-sent on a resumed turn as though
    /// it were part of the conversation.
    observations: &'a [ToolObservation],
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

/// What one model turn produced once its stream reached a terminal.
///
/// Three arms rather than a `Result` with an error, because **a tool call is not a failure of the
/// run**: the model proposed work, the fabric performed or refused it, and the run continues with
/// the observation. An implementation that returned `Err` for a tool call would end a run the model
/// was still working through — which is exactly what the previous version did with
/// `run.tools_not_implemented`, deliberately and with a label saying so.
#[derive(Debug)]
enum TurnOutcome {
    /// The turn produced the run's final answer.
    Answer(RunOutcome),
    /// The turn's tool calls were dispatched and produced observations for a further turn.
    ToolResults {
        /// The observations, in the order the calls completed.
        observations: Vec<ToolObservation>,
    },
    /// A call is blocked on a human decision, so the run waits.
    WaitingApproval {
        /// The approval the caller must list and decide.
        approval: ApprovalId,
    },
}

/// What dispatching one turn's tool calls produced.
///
/// Two arms, and the difference is whether the run can continue: a set of observations lets the
/// model take another turn, while a pending approval stops the batch because a later call's
/// observation depends on a decision nobody has made yet.
#[derive(Debug)]
enum DispatchOutcome {
    /// Every call settled and produced an observation.
    Observations(Vec<ToolObservation>),
    /// A call is waiting on a decision, so the run cannot continue this turn.
    WaitingApproval {
        /// The approval to list and decide.
        approval: ApprovalId,
    },
}

/// One tool call the model completed, with the canonical id its result must pair with.
///
/// **The pair is the point.** `InputItems::new` refuses a tool result whose call is absent, so an
/// observation must carry the *same* call id the provider stamped — a fresh id would produce an
/// orphaned pair the domain refuses, and the run would fail with a stream error for a tool that ran
/// perfectly. The capability is the name the call was announced under, which is what the catalog
/// resolves; the identity it resolves to is what policy authorizes and the ledger keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedToolCall {
    /// The provider's canonical call id, shared by the call and its result.
    call_id: String,
    /// The capability name the model used.
    capability: String,
    /// The complete argument document, as the provider sent it.
    arguments: String,
}

/// What one dispatched tool call produced, as the next model turn sees it.
///
/// Carries **both halves of the pair**: the assistant's call and the tool's result. A request built
/// from the result alone is refused by the domain's `InputItems::new` as an orphaned tool result —
/// the invariant the contract states for context compaction is that a result cannot survive its
/// call — so the call has to travel with it rather than being re-derived, because re-deriving it
/// would mean re-parsing the argument document the provider already sent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolObservation {
    /// The provider's canonical call id, shared by the call and its result.
    call_id: String,
    /// The capability name the model used, which is the call item's `tool_name`.
    capability: String,
    /// The complete argument document, as the provider sent it.
    arguments: String,
    /// Whether the tool reported an error, so the model can tell a result from a refusal.
    is_error: bool,
    /// The bounded text the model reads.
    content: String,
}

/// Renders a call outcome as the observation the model receives.
///
/// A refusal is rendered as **an observation with `is_error` true rather than as a run failure**,
/// and that is the whole design: the model is a participant that must be told "you are not allowed
/// to do that" or "that tool does not exist", because a model that receives a run-level fault
/// cannot reason about it and will propose the same call again. The contract's rule is that a
/// refusal is information; only a fault ends the run.
///
/// The rendering names the **error class**, never the provider's own text and never the arguments:
/// both are untrusted, and an observation is placed in the model's context as content the model
/// reasons over. A class is a closed-set token the model can act on; a provider's raw message would
/// be an injection channel with a helpful tone.
fn observation_of(
    call: &CapturedToolCall,
    outcome: &crate::tool_call::ToolCallOutcome,
) -> ToolObservation {
    use crate::tool_call::ToolCallOutcome;
    let (is_error, content) = match outcome {
        ToolCallOutcome::Completed { result } => (false, render_result(result)),
        ToolCallOutcome::Refused { class, .. } => (
            true,
            format!(
                "{{\"refused\":\"{}\",\"detail\":\"{}\"}}",
                // **Through the class's own spelling, never a literal.** A hand-written `tool.*`
                // string here would be a second spelling of a value the domain already owns, and it
                // would put a *tool* code inside the run controller's source where the run
                // error-code scan reads dotted literals — the scan would then report a code this
                // file does not use as a run outcome. The observation is content, not a code, and
                // sourcing it from the type is what keeps that distinction visible.
                class.as_contract_str(),
                refusal_detail(*class),
            ),
        ),
        // A waiting approval is **not** an observation to reason from: there is no result and
        // nothing for the model to decide, because a human has to. The run is moved to a waiting
        // state by the caller and resumes when the approval is decided, so this arm is never reached
        // for a run that continues; the text names the state and contains no code, because the
        // pending approval is already a durable record a client lists rather than a value to parse
        // out of prose.
        ToolCallOutcome::WaitingApproval { .. } => (
            true,
            "{\"pending\":\"a human decision is required before this call can run\"}".to_owned(),
        ),
        // A duplicate whose result this build cannot return is reported as a conflict rather than
        // as a success with no content — returning an empty result would tell the model the tool
        // ran and produced nothing, which is a claim JARVIS cannot make.
        ToolCallOutcome::Duplicate { state, .. } => (
            true,
            format!(
                "{{\"refused\":\"{}\",\"detail\":\"an equivalent call is already {}\"}}",
                ToolErrorClass::Conflict.as_contract_str(),
                state.as_contract_str(),
            ),
        ),
    };
    ToolObservation {
        call_id: call.call_id.clone(),
        capability: call.capability.clone(),
        arguments: call.arguments.clone(),
        is_error,
        content,
    }
}

/// Returns a short, fixed explanation for a refusal class.
///
/// **A closed set with fixed text**, so an observation cannot carry anything but a sentence JARVIS
/// wrote. This is the same rule the run's own error messages follow: a message that interpolated a
/// provider's value or a caller's argument would put untrusted text into a model's context through
/// the one channel the model is guaranteed to trust.
fn refusal_detail(class: ToolErrorClass) -> &'static str {
    match class {
        ToolErrorClass::NotFound => "no such tool is registered",
        ToolErrorClass::Unavailable => "the tool is unavailable",
        ToolErrorClass::SchemaInvalid => "the arguments did not match the tool's schema",
        ToolErrorClass::PermissionDenied => "this action is not permitted",
        ToolErrorClass::ApprovalRequired => "this action needs approval",
        ToolErrorClass::ApprovalRejected => "approval was refused",
        ToolErrorClass::ApprovalExpired => "approval expired",
        ToolErrorClass::Conflict => "the call conflicts with an existing one",
        ToolErrorClass::RateLimited => "the tool is rate limited",
        ToolErrorClass::Timeout => "the call timed out",
        ToolErrorClass::Cancelled => "the call was cancelled",
        ToolErrorClass::ProviderAuth => "the tool's credentials were rejected",
        ToolErrorClass::ProviderError => "the tool failed",
        ToolErrorClass::OutputInvalid => "the result did not validate",
        ToolErrorClass::OutcomeAmbiguous => "the outcome is unknown and must be reconciled",
        ToolErrorClass::LimitExceeded => "a bound was exceeded",
    }
}

/// Renders a result's content blocks as the text a model reads.
///
/// A tool result is bounded (the domain refuses an over-total one), so this cannot produce an
/// unbounded string; the blocks are joined in the order the provider returned them.
fn render_result(result: &ToolResultBody) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(result.content.len());
    for block in &result.content {
        match block {
            ContentBlock::Json { value } => parts.push(value.as_str().to_owned()),
            ContentBlock::Text { text } => parts.push(text.as_str().to_owned()),
            // A reference the model cannot read is rendered as an explicit note. Placing the
            // artifact *id* alone would look like content, and a model that treated an identifier
            // as the answer would hallucinate around it.
            ContentBlock::Artifact {
                id,
                media_type,
                size,
            } => parts.push(format!(
                "{{\"artifact\":\"{id}\",\"media_type\":\"{media_type}\",\"size\":{size},\"note\":\"content is stored outside this result\"}}"
            )),
        }
    }
    parts.join("\n")
}

/// What draining a stream produced once it reached its terminal.
#[derive(Debug, Default)]
struct DrainedTurn {
    answer: String,
    tool_intent: Option<String>,
    /// The tool calls the model **completed**, with their final arguments.
    ///
    /// **Separate from [`Self::tool_intent`], and the separation was a real gap.** `tool_intent`
    /// records only the *name* of the first `tool.call.added` frame, which was all a controller that
    /// refused every tool call needed. Executing one needs the **complete argument document**, which
    /// arrives on a later `tool.call.completed` frame — so the name alone could not dispatch
    /// anything, and a controller that had tried would have run a tool on half-received arguments.
    /// The domain's `ToolArguments::executable_raw` is the single door that answers "are these
    /// arguments complete?", and this list is built from frames that passed through it.
    tool_calls: Vec<CapturedToolCall>,
    /// The tool name each open call id was announced under.
    ///
    /// A map rather than a field on the vector, because `tool.call.added` and `tool.call.completed`
    /// are **separate frames** and a provider may interleave several calls: pairing them by position
    /// would attribute one call's arguments to another's name. The domain's stream state machine
    /// already keys its own open-call table the same way, and this mirrors it rather than
    /// re-deriving the pairing.
    tool_names: std::collections::BTreeMap<String, String>,
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
    /// Why the provider stopped producing output, from the terminal frame.
    ///
    /// Captured because the finish reason is a fact about the request that only the provider can
    /// supply, and the schema and the port both carry it: a run that stopped on `length` is not
    /// the same as one that stopped on `stop`, and without this a reconciliation pass or an
    /// operator could not tell "the model was cut off by its own token limit" from "the model
    /// finished". The domain retains an unmodelled provider reason as `FinishReason::Other`
    /// precisely so a new reason stays visible instead of being flattened into a clean one.
    finish_reason: Option<FinishReason>,
    /// The provider's own request identifier, from any frame that carried provider metadata.
    ///
    /// A fact about the request that only the provider can supply, and the one value a support
    /// enquiry needs: the schema and the port both carry `provider_request_id`, and the contract
    /// lists "provider request/continuation references" as part of a completed call. Without
    /// this the column stayed `NULL` on every run row while the document claimed the port that
    /// stores it reads it back — the same "a writer nothing feeds" shape `finish_reason` had.
    provider_request_id: Option<String>,
    /// When the first output delta of this call arrived, observed from the controller's own clock.
    ///
    /// The instant is JARVIS's, not the provider's: it is the moment the first output was **seen
    /// here**, which is what a time-to-first-token interval is measured against — a provider's own
    /// stamp would describe its side of a network JARVIS does not control, so the two would not be
    /// comparable across providers.
    ///
    /// **This was the last "wired but with no producer" column.**
    /// [`ModelCallOutcome::first_output_at`](crate::repository::model_call::ModelCallOutcome)
    /// carried it, the adapter bound it, and the read returned it, while every controller path
    /// passed `None` — so the column was absent on every row and the roadmap's "measured time to
    /// first token" had nothing to measure. Set once, on the first `output.text.delta`, so a later
    /// delta cannot move it: the first output is what "time to first token" means.
    first_output_at: Option<UtcTimestamp>,
    /// When the **last** output delta of this call arrived, from the controller's own clock.
    ///
    /// Written unconditionally on every delta, unlike `first_output_at`.
    ///
    /// **This is the other half of `BRN-011`'s measurement.** `first_output_at` was produced and
    /// nothing produced `last_output_at`, so `IncrementalDelivery` — the type the model gateway
    /// architecture requires routing to consult, and which the routing layer already enforces —
    /// had **no producer anywhere in the workspace**: every `CapabilityDescriptor` was built with
    /// `incremental_delivery: None`, which meant "nobody measured this" for every model in every
    /// deployment. A first token and a burst are indistinguishable without the interval's end.
    last_output_at: Option<UtcTimestamp>,
    /// How many output deltas this call produced.
    ///
    /// The measurement's sample size. Counted rather than inferred from the text length, because
    /// a provider may deliver one large delta or many small ones for the same answer, and the
    /// profile's `observed_deltas` is a statement about the *delivery*, not about the content.
    output_delta_count: u32,
}

/// Everything a turn needs that does not change between turns.
///
/// Grouped so [`RunController::drive_turns`] takes one argument rather than seven, following
/// `BRN-008`'s own rule that several same-shaped identifiers in a positional call invite a
/// transposition — and here two of them are identifiers.
struct TurnInputs<'a> {
    context: &'a RequestContext,
    conversation_id: ConversationId,
    items: &'a [RetainedItem],
    model: &'a ModelRef,
    budget: &'a RunBudget,
    cancel: &'a CancellationScope,
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
    /// Tells a live follower that this run's durable stream has grown.
    ///
    /// The controller is where this belongs rather than the repository, because it is the only
    /// place that knows **when a step is published**: every event this run appends is written
    /// through one of three methods here (`advance`, `append_delta`, `publish_usage`), so a
    /// notification at each of them is complete by construction. A notifying repository decorator
    /// would be a second implementation of the storage contract that could drift from the first,
    /// and it could not see a delta published through [`StreamDeltaSink`], which is a separate
    /// port.
    live: crate::live_events::RunStreamNotifier,
    /// Dispatches the model's tool calls through the governed pipeline.
    ///
    /// **Optional, and `None` is a real state rather than a permissive default.** A controller
    /// composed without this port **refuses** every tool call with `run.tools_not_implemented` — the
    /// behaviour the whole fabric had before this existed — rather than executing one ungoverned. A
    /// default that ran tools directly would be the single most dangerous fail-open in the product,
    /// and making the port optional is what lets every existing controller test keep asserting the
    /// refusal without constructing a full tool stack.
    tools: Option<Arc<crate::tool_call::ToolCallService>>,
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
            // A private notifier nobody subscribes to, so a controller built directly is identical
            // to one built through `RunService`: notification is a no-op with no followers rather
            // than a missing behaviour. That is what keeps a controller test and a daemon run on one
            // code path.
            live: crate::live_events::RunStreamNotifier::new(),
            // No tool pipeline, so every tool call is refused by name. The safe default: a
            // controller that ran tools without a policy stage would be the one bypass `TLS-012`
            // exists to make impossible.
            tools: None,
        }
    }

    /// Shares a tool-call service, so this run's tool intents are governed.
    ///
    /// Consuming rather than taking `&mut self`, so the composition cannot attach the pipeline
    /// after a run has begun — the same reason [`Self::with_notifier`] consumes.
    #[must_use]
    pub fn with_tools(mut self, tools: Arc<crate::tool_call::ToolCallService>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Shares `live` so a follower and the controller that feeds it hold one notifier.
    ///
    /// Consuming rather than taking `&mut self`, so this cannot be called twice and leave a
    /// controller notifying a channel no follower reads — which would be a silently absent feature.
    #[must_use]
    pub fn with_notifier(mut self, live: crate::live_events::RunStreamNotifier) -> Self {
        self.live = live;
        self
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
        objective_message: Option<MessageId>,
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
                    Step::cancelled_from(stored.state, "cancelled_before_start")
                        .with_requester_reason(cancel.cancel_reason()),
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

        let (budget, assembled) = self
            .build_context(run, conversation_id, objective, objective_message)
            .await?;

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

        // The model comes from the run's **stored route**, when a policy selected one, and only
        // falls back to the provider's own first served model when no policy was in force.
        //
        // This is the point at which a data policy constrains the call a run actually makes. The
        // route was selected and recorded at creation, where a policy that admitted no compliant
        // model refuses the request; here the selection is read back, so the locality,
        // allow-list, and ceiling a policy expressed are what the daemon calls with rather than
        // what a diagnostic endpoint reports. Taking `provider.models().first()` instead — which
        // is what this did — let a local-only policy select a cloud model for the real call while
        // the probe answered correctly.
        //
        // A stored route naming a model the provider no longer serves fails the run rather than
        // falling back: the policy authorized *that* model, and calling a different one would
        // perform an action nobody permitted. The provider's own selection is used only when no
        // policy governed the run, which is the unconstrained case the budget records by carrying
        // no route.
        let model = match self.resolve_model(run).await {
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

        // Planning -> AwaitingModel. This is the **first** turn's move into the model call; a later
        // turn makes the same move from `Observing`, which is the state the tool work leaves the run
        // in. `step_unless_cancelled` is used for both so the cancellation check is identical.
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
        // in a helper that owns the `AwaitingModel -> <terminal>` transition. The turn loop is a
        // second helper, so every argument it needs is named once rather than closed over.
        self.drive_turns(
            run,
            &TurnInputs {
                context,
                conversation_id,
                items: &assembled.items,
                model: &model,
                budget: &budget,
                cancel,
            },
        )
        .await
    }

    /// Runs model turns until one answers or the turn budget is exhausted.
    ///
    /// **The loop carries its observations forward**, which is why it is a loop over turns rather than
    /// over model calls: turn *n+1*'s input is turn *n*'s tool results, and a helper that re-read the
    /// stored transcript would re-send the objective as a fresh question and lose the pairing between
    /// a call and its result.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError`] when a turn could not be driven, or
    /// [`ControllerError::TurnBudgetExceeded`] when the model kept proposing tool calls past
    /// [`MAX_MODEL_TURNS`].
    async fn drive_turns(
        &self,
        run: RunRef,
        turn: &TurnInputs<'_>,
    ) -> Result<RunOutcome, ControllerError> {
        let mut observations: Vec<ToolObservation> = Vec::new();
        let mut turn_index: u32 = 1;
        loop {
            let outcome = self
                .ask_model(
                    run,
                    &ModelTurn {
                        context: turn.context,
                        conversation_id: turn.conversation_id,
                        items: turn.items,
                        observations: &observations,
                        model: turn.model,
                        cancel: turn.cancel,
                        budget: turn.budget,
                    },
                )
                .await?;

            match outcome {
                TurnOutcome::Answer(outcome) => return Ok(outcome),
                TurnOutcome::ToolResults {
                    observations: produced,
                } => {
                    // A further model turn is permitted only while the run has turns left. The bound
                    // is checked **before** the loop continues, so an exhausted budget fails the run
                    // with a named reason rather than looping until the deadline does it — which
                    // would report a slow run for one that asked for too many turns.
                    turn_index = turn_index.saturating_add(1);
                    if turn_index > MAX_MODEL_TURNS {
                        self.finish(
                            run,
                            Step::failed(
                                RunState::Observing,
                                "run.failed",
                                "turn_budget_exhausted",
                                ControllerError::TurnBudgetExceeded.code(),
                            ),
                        )
                        .await?;
                        return Err(ControllerError::TurnBudgetExceeded);
                    }
                    // **The run returns to `Planning` and then to `AwaitingModel`.** The architecture's
                    // diagram routes a settled tool observation through `Observing -> Planning`, so the
                    // next turn is a fresh decision about what to do with the result rather than a
                    // continuation of the same model call — and the two transitions are the domain's own
                    // legal edges rather than a shortcut, which is what keeps the run's recorded state
                    // path a path the table permits.
                    self.step_unless_cancelled(
                        run,
                        Step::new(
                            RunState::Observing,
                            RunState::Planning,
                            "run.planning",
                            "observation_ready",
                        ),
                        turn.cancel,
                    )
                    .await?;
                    self.step_unless_cancelled(
                        run,
                        Step::new(
                            RunState::Planning,
                            RunState::AwaitingModel,
                            "run.model_started",
                            "model_call_requested",
                        ),
                        turn.cancel,
                    )
                    .await?;
                    observations = produced;
                }
                // A tool call is blocked on a human. The run is left **waiting** rather than
                // terminal, which is the whole point of a durable approval: the prompt is recorded
                // and the run resumes when it is decided. A terminal state here would make the
                // approval useless.
                TurnOutcome::WaitingApproval { approval } => {
                    self.finish(
                        run,
                        Step::new(
                            RunState::AwaitingModel,
                            RunState::AwaitingApproval,
                            "run.approval_requested",
                            "tool_approval_pending",
                        ),
                    )
                    .await?;
                    // **No `error_code` is fabricated, and the approval is not encoded into one.**
                    // A run waiting on a decision has not failed: putting a code on its row would
                    // make a successful request read as a failure, and appending the approval id to
                    // a fixed code would make `error_code` a dynamic value the client-visible
                    // vocabulary cannot enumerate — the defect the vocabulary test caught. A caller
                    // learns which approval to decide from the run's `run.approval_requested` event
                    // and from `GET /api/v1/approvals`, which is the surface that lists it.
                    //
                    // The approval identity is deliberately **not** logged here either: this layer
                    // has no tracing dependency, and the value is already durable in the approval
                    // store and named by the transition's reason.
                    let _ = approval;
                    return Ok(RunOutcome {
                        run_id: run.run_id,
                        state: RunState::AwaitingApproval,
                        answer: None,
                        error_code: None,
                    });
                }
            }
        }
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
        objective_message: Option<MessageId>,
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
            // The objective is also a stored message, so the assembler drops that one by identity
            // and keeps the objective candidate below. Without this the model received the question
            // twice: once as a transcript turn and once as the run's task.
            objective_message,
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

    /// Resolves the model this run is authorized to call.
    ///
    /// The stored route wins when one exists, because it is what a policy selected and recorded;
    /// the provider's own first served model is used only when **no policy was in force**, which
    /// the run's budget records by carrying no route. Falling back when a route exists would
    /// substitute a model nobody authorized for one that was, which is the leak this exists to
    /// prevent.
    ///
    /// A stored route naming a model the provider no longer serves is
    /// [`ControllerError::NoModelServed`] rather than a silent fallback: the policy authorized that
    /// model, so calling a different one would perform an action no rule permitted. The answer is
    /// the same code the no-model case uses, because the operator's remedy is the same — the route
    /// cannot be honoured, so the configuration must change.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError::Repository`] when the run cannot be read and
    /// [`ControllerError::NoModelServed`] when neither a stored route nor a served model exists.
    async fn resolve_model(&self, run: RunRef) -> Result<ModelRef, ControllerError> {
        let budget = self.load(run).await?.budget;
        if let Some(route) = budget.route.as_ref() {
            // A routed model must still be served. The provider is the only thing that knows, and
            // a route naming a model it does not serve is configuration drift rather than a policy
            // decision, so it is reported instead of being quietly replaced.
            if self
                .provider
                .models()
                .iter()
                .any(|served| served == &route.model)
            {
                return Ok(route.model.clone());
            }
            return Err(ControllerError::NoModelServed);
        }
        selected_model(self.provider.as_ref())
    }

    /// Advances a run one state, and refuses to continue if a cancellation arrived.
    async fn step_unless_cancelled(
        &self,
        run: RunRef,
        step: Step,
        cancel: &CancellationScope,
    ) -> Result<(), ControllerError> {
        // Captured **before** the step is consumed, because `self.step` moves it and the cancellation
        // path below still needs to know where the run landed. This was a compile error rather than a
        // design choice, and it is worth noting that it was the *compiler* that made the ordering
        // explicit: a value needed after a move cannot be reached by accident.
        let landed_in = step.to;
        self.step(run, step).await?;
        if !cancel.is_cancelled() {
            return Ok(());
        }
        // `landed_in` is where the run now is, and every working state has a `Cancelled`
        // edge. `Waiting` is the one exception a caller could reach here, and it has one
        // too, so this is total over the states a step can land in.
        //
        // The **requester's** reason is read off the scope and published, because the scope is where
        // the caller's text lives and this is the last point that holds it: the transition itself can
        // only carry a closed-set label, so a reason that is not attached here is a reason the durable
        // record loses.
        let requester_reason = cancel.cancel_reason();
        self.finish(
            run,
            Step::cancelled_from(landed_in, "cancelled_during_step")
                .with_requester_reason(requester_reason),
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
    ) -> Result<TurnOutcome, ControllerError> {
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

        // The stored route is read once and its decision identifier carried into every attempt, so
        // a retry's second `model_calls` row names the same decision as its first. Re-deriving it
        // per attempt would let one logical call's attempts claim different routes, which is
        // exactly what the shared `logical_call_id` exists to prevent.
        let route = self.load(run).await?.budget.route;

        // One logical call spans every attempt, so the chain is auditable as one operation
        // rather than as unrelated calls that happen to be adjacent.
        let logical_call_id = ModelCallId::from_uuid(uuid::Uuid::now_v7());
        let mut attempt = 1_u32;

        loop {
            match self
                .attempt_call(run, turn, logical_call_id, attempt, route.as_ref())
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
                            // No stream was drained on this path — the backoff elapsed before a
                            // retry began — so no output was produced and the timing is the
                            // default: no first instant, no sample, which is what "nothing was
                            // measured" looks like rather than a zeroed profile. The usage is
                            // absent for the same reason, and the previous attempt's report
                            // belongs to its own row rather than to this no-call.
                            DeliveryTiming::default(),
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
    ///
    /// `route` is the run's stored selection, and its decision identifier is stamped on the
    /// attempt's own row. The column answers "which authorization permitted this call", which is
    /// the question an operator asks about a single call rather than about the run, and it is why
    /// every attempt of one logical call names the same decision.
    async fn attempt_call(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
        logical_call_id: ModelCallId,
        attempt: u32,
        route: Option<&RunRoute>,
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
                // The route decision that authorized this call. `None` when no policy was in
                // force, which is the same distinction the run's budget draws — a call recorded
                // under no decision and one permitted by a decision that selected the same model
                // are different facts, and only the second can be explained after the fact.
                route_decision: route.map(|route| route.decision),
                request_fingerprint: None,
                started_at,
            })
            .await
            .map_err(ControllerError::Repository)?;

        let request = build_request(
            run.run_id,
            call_id,
            turn.model,
            turn.items,
            turn.observations,
            &self.tool_names(),
            turn.budget,
        )?;

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
                // Nothing was drained, so no output was produced: the default timing says "no
                // measurement" rather than a zeroed profile. No usage is passed either, because the
                // provider never opened a stream and therefore reported none — an absent usage is
                // honest here, where a `usage_of` over the empty drain would invent one.
                self.finish_deadline_exceeded(run, Some(call_id), DeliveryTiming::default())
                    .await?;
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
                Step::cancelled_from(RunState::AwaitingModel, "cancelled_after_output")
                    .with_requester_reason(turn.cancel.cancel_reason()),
            )
            .await?;
            // The attempt is closed as cancelled rather than completed: the output was
            // produced, but recording it as a successful call would say the call's result
            // was the run's outcome, and the run's outcome is that the caller stopped it.
            // The provider id is carried through: the call happened, and a support enquiry
            // about a cancelled run needs exactly that correlation.
            self.record_call_outcome_with(
                run,
                call_id,
                ModelCallState::Cancelled,
                RecordedOutcome {
                    usage: usage_of(&drained),
                    provider_request_id: drained.provider_request_id.clone(),
                    // Carried through: the output happened, so the delivery measurement this call
                    // produced is real even though the caller stopped it.
                    delivery: DeliveryTiming::of(&drained),
                    ..RecordedOutcome::default()
                },
            )
            .await?;
            return Err(ControllerError::Cancelled);
        }

        self.finish_attempt(run, turn, call_id, drained).await
    }

    /// Settles a run whose provider ended its stream with a terminal other than `call.completed`.
    ///
    /// The contract says adapters emit exactly one of `call.completed`, `call.failed`, or
    /// `call.cancelled`; the first is an answer and the other two are not. This maps the two
    /// non-answer terminals onto their matching run states rather than treating "the stream ended"
    /// as "the model answered", which is what the controller did before this existed.
    ///
    /// The two are settled differently because they are different facts. A `call.failed` terminal is
    /// the provider abandoning an accepted call: the request is ambiguous, so the run **fails** and
    /// is never retried, the same rule [`fail_after_acceptance`] encodes. A `call.cancelled` terminal
    /// is the provider saying *it* stopped the call — a content policy, a provider-side timeout, an
    /// operator action on the provider's side — so the run is **cancelled**, not failed: reporting a
    /// provider's cancellation as a fault would make it indistinguishable from a genuine failure.
    ///
    /// Returns `Ok(())` for a completion (or an absent terminal), so the caller can carry on to the
    /// completion path; returns the matching typed error once the run and its call are settled.
    async fn settle_provider_terminal(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        usage: Option<&Usage>,
        drained: &DrainedTurn,
    ) -> Result<(), ControllerError> {
        let (outcome, transition, error) = match drained.finish_reason {
            Some(FinishReason::ProviderError) => (
                ModelCallState::Failed,
                Step::failed(
                    RunState::AwaitingModel,
                    "run.failed",
                    "provider_reported_call_failed",
                    ControllerError::ModelCallFailed.code(),
                ),
                ControllerError::ModelCallFailed,
            ),
            Some(FinishReason::Cancelled) => (
                ModelCallState::Cancelled,
                Step::cancelled_from(RunState::AwaitingModel, "provider_reported_call_cancelled"),
                ControllerError::Cancelled,
            ),
            _ => return Ok(()),
        };

        self.finish(run, transition).await?;
        // The call is closed with the same terminal the run was: a row reading `Completed` beside a
        // run reading `Cancelled` would disagree about the one fact both records exist to state.
        self.record_call_outcome_with(
            run,
            call_id,
            outcome,
            RecordedOutcome {
                usage: usage.cloned(),
                finish_reason: drained.finish_reason.clone(),
                delivery: DeliveryTiming::of(drained),
                provider_request_id: drained.provider_request_id.clone(),
            },
        )
        .await?;
        Err(error)
    }

    /// Judges a drained stream: a tool intent, a breached ceiling, or a completion.
    async fn finish_attempt(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
        call_id: ModelCallId,
        drained: DrainedTurn,
    ) -> Result<AttemptOutcome, ControllerError> {
        // Captured before anything is moved out of `drained`, because several paths below
        // need them and a later borrow would be a borrow of a partially moved value. The
        // delivery timing is captured here rather than at each use for exactly that reason:
        // the tool calls are moved out below, and reading the timing after that move does not
        // compile.
        let usage = usage_of(&drained);
        let delivery = DeliveryTiming::of(&drained);

        // **A completed tool call is dispatched through the governed pipeline**, and the run
        // continues with the observation rather than ending. This replaces the refusal that carried
        // `run.tools_not_implemented` for every tool intent.
        //
        // A call whose arguments the provider never completed (`tool.call.added` with no matching
        // `tool.call.completed`) is **not** dispatched: the domain's stream state machine refuses to
        // finish a stream with an open call, so reaching here with an uncompleted call means the
        // `finish()` below would have refused the stream — which it does, before this point.
        if !drained.tool_calls.is_empty() {
            return self
                .settle_tool_calls(run, turn, call_id, drained, usage, delivery)
                .await;
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
                DeliveryTiming::of(&drained),
                // **The report that breached the ceiling travels with the failure.** This path
                // discards the run's answer, so the usage is the only evidence of what the provider
                // produced — and the code a client receives names the breached ceiling. Without the
                // usage the model call was recorded as having consumed *nothing*, and the reader
                // sent to `run.budget_output_tokens_exceeded` had no number to compare against it.
                Some(reported),
                "consumption_budget_exceeded",
                ControllerError::BudgetExceeded { limit }.code(),
            )
            .await?;
            return Err(ControllerError::BudgetExceeded { limit });
        }

        // The provider's own terminal said the call did not succeed, so neither did the run. This
        // check must come **before** the completion below, because everything after it treats the
        // stream's terminal as an answer: the call is recorded `Completed`, the usage is published,
        // and the partial output is stored as the delivered answer. Before this existed, none of
        // that looked at the finish reason at all, so a provider that ended its stream with
        // `call.failed` produced a **successful** run whose answer was the text it had abandoned —
        // and a caller reading `state: "completed"` had no way to learn the call failed.
        self.settle_provider_terminal(run, call_id, usage.as_ref(), &drained)
            .await?;

        // The call's outcome is recorded before the run moves on, so a completed run
        // never leaves a model call open. The finish reason travels with it, because "why the
        // provider stopped" is part of the recorded outcome rather than a detail: a run cut off
        // by its own token limit must not be indistinguishable from one the model finished.
        let completed_at = self.now()?;
        let finish_reason = drained.finish_reason.clone();
        // Captured before `drained` is moved into `complete_run` below, and from the same
        // value the completion records, so the row and the answer cannot name different
        // provider calls. The delivery timing is captured here for the same reason: it is part
        // of the recorded outcome, and reading it after the move is not possible.
        let provider_request_id = drained.provider_request_id.clone();
        let delivery = DeliveryTiming::of(&drained);
        self.record_call_outcome_with(
            run,
            call_id,
            ModelCallState::Completed,
            RecordedOutcome {
                usage: usage.clone(),
                finish_reason,
                delivery,
                provider_request_id,
            },
        )
        .await?;

        // And the usage is published as its own event, which the contract lists as a **required**
        // first-slice event type and which nothing published before this: the only way to write an
        // activity event was to write a transition with it, so an informational event was
        // unwritable and a client following the stream could not see what the call consumed.
        //
        // Published only when the provider reported **at least one counter**, because unknown is
        // not zero: an event is a durable public statement, and publishing `0, 0` for a provider
        // that said nothing would record a measurement nobody made.
        if let Some(reported) = usage.as_ref()
            && reported.has_any_counter()
        {
            self.publish_usage(run, call_id, reported).await?;
        }

        // AwaitingModel -> Responding -> Completed, then the answer is stored. The turn's outcome is
        // a `RunOutcome` rather than a further tool round-trip, because a stream that reached its
        // terminal without tool calls is the model answering.
        self.complete_run(
            run,
            drained,
            turn.conversation_id,
            completed_at,
            turn.cancel,
        )
        .await
        .map(|outcome| AttemptOutcome::Completed(TurnOutcome::Answer(outcome)))
    }

    /// Dispatches one turn's tool calls and closes the model call that proposed them.
    ///
    /// **Extracted from [`Self::finish_attempt`] so that method stays about judging a drained
    /// stream.** The tool lifecycle has its own states (`ExecutingTool`, `Observing`), its own
    /// events, and its own two outcomes — observations for a further turn, or a pending approval —
    /// and folding it into the stream judgement is what pushed both past clippy's line budget.
    ///
    /// A call whose arguments the provider never completed (`tool.call.added` with no matching
    /// `tool.call.completed`) is **not** dispatched: the domain's stream state machine refuses to
    /// finish a stream with an open call, so by the time this runs every captured call is complete —
    /// and `CapturedToolCall` is only built from a `ToolCallCompleted` frame.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError`] only for a fault, or [`ControllerError::TurnBudgetExceeded`] is
    /// **not** raised here — the turn bound belongs to [`Self::drive_turns`]. A tool that refused a
    /// call is an observation, not an error.
    async fn settle_tool_calls(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
        call_id: ModelCallId,
        drained: DrainedTurn,
        usage: Option<Usage>,
        delivery: DeliveryTiming,
    ) -> Result<AttemptOutcome, ControllerError> {
        // The run moves through the states the architecture's diagram names, so a client following
        // the stream sees the tool work rather than only its result. `AwaitingModel -> ExecutingTool
        // -> Observing` is the documented path, and each step is one durable transition with its
        // event.
        self.step(
            run,
            Step::new(
                RunState::AwaitingModel,
                RunState::ExecutingTool,
                "run.tool_executing",
                "tool_calls_proposed",
            ),
        )
        .await?;

        let dispatched = self.dispatch_tools(run, turn, &drained.tool_calls).await?;

        // The model call is closed **before** the observation is acted on, so a call that proposed
        // tools is not left open while the run moves on. The finish reason travels with it, so an
        // operator reading the row sees `tool_calls` rather than a bare completion.
        self.record_call_outcome_with(
            run,
            call_id,
            ModelCallState::Completed,
            RecordedOutcome {
                usage: usage.clone(),
                finish_reason: drained.finish_reason.clone(),
                delivery,
                provider_request_id: drained.provider_request_id.clone(),
            },
        )
        .await?;

        // The usage is published before the observation for the same reason it is published on the
        // answer path: it is a durable statement about what a completed call consumed.
        if let Some(reported) = usage.as_ref()
            && reported.has_any_counter()
        {
            self.publish_usage(run, call_id, reported).await?;
        }

        self.step(
            run,
            Step::new(
                RunState::ExecutingTool,
                RunState::Observing,
                "run.observing",
                "tool_calls_settled",
            ),
        )
        .await?;

        Ok(match dispatched {
            DispatchOutcome::Observations(observations) => {
                // The observation is carried to the next turn rather than stored: it is in-flight
                // context, and the durable record of what a tool did is the ledger row and the model
                // call, not a transcript entry that a resumed run would re-send.
                AttemptOutcome::Completed(TurnOutcome::ToolResults { observations })
            }
            DispatchOutcome::WaitingApproval { approval } => {
                AttemptOutcome::Completed(TurnOutcome::WaitingApproval { approval })
            }
        })
    }

    /// Dispatches every captured tool call through the governed pipeline.
    ///
    /// **One call may wait for an approval, and that stops the batch.** A run whose second tool call
    /// is blocked on a human cannot proceed with a third, because the model's next turn needs an
    /// observation this call has not produced — so the batch returns `WaitingApproval` and the run
    /// moves to `AwaitingApproval`. The alternative — dispatching the rest and returning a partial
    /// set — would send the model a transcript with a call that has no result, which
    /// `InputItems::new` refuses and which would be a lie about what happened.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError`] only for a **fault**: a store failure, a clock failure, or the
    /// absence of the tool pipeline. A tool that refused the call is an **observation**, not an
    /// error — the model must be told, and a run-level fault would deny it the chance to react.
    async fn dispatch_tools(
        &self,
        run: RunRef,
        turn: &ModelTurn<'_>,
        calls: &[CapturedToolCall],
    ) -> Result<DispatchOutcome, ControllerError> {
        // **No pipeline means the capability is absent, and that is refused by name rather than
        // approximated.** This is the state every controller test that composes no tool stack is in,
        // and it keeps `run.tools_not_implemented` honest: it now means "this deployment has no tool
        // pipeline", not "the fabric was never built".
        let Some(service) = self.tools.as_ref() else {
            let name = calls
                .first()
                .map(|call| call.capability.clone())
                .unwrap_or_default();
            self.finish(
                run,
                Step::failed(
                    RunState::ExecutingTool,
                    "run.failed",
                    "tool_fabric_unavailable",
                    ControllerError::ToolsNotImplemented {
                        tool_name: name.clone(),
                    }
                    .code(),
                ),
            )
            .await?;
            return Err(ControllerError::ToolsNotImplemented { tool_name: name });
        };

        let mut observations: Vec<ToolObservation> = Vec::with_capacity(calls.len());
        for call in calls {
            // The intent is built from the captured pair: the name the model emitted becomes a
            // capability the catalog resolves, and the argument document is carried as the
            // **unvalidated** text the domain's `ToolArguments` wraps. The domain deliberately does
            // not parse it — validating against a schema is the validator's job — so a malformed
            // document is refused by `tool.schema_invalid` rather than at construction, which is the
            // diagnosis a model can act on.
            let Ok(arguments) = ToolArguments::new(&call.arguments) else {
                // An argument document that cannot even be wrapped — empty, over the byte bound, or
                // carrying a NUL — is refused as a schema failure. Building an intent would be
                // impossible, so the model is told its arguments were unusable, and the remaining
                // calls in the batch are still dispatched.
                observations.push(ToolObservation {
                    call_id: call.call_id.clone(),
                    capability: call.capability.clone(),
                    arguments: call.arguments.clone(),
                    is_error: true,
                    content: format!(
                        "{{\"refused\":\"{}\",\"detail\":\"the arguments were empty or \
                         exceeded the accepted size\"}}",
                        ToolErrorClass::SchemaInvalid.as_contract_str(),
                    ),
                });
                continue;
            };
            let intent = ToolCallIntent::new(
                ToolCallId::from_uuid(uuid::Uuid::now_v7()),
                &call.capability,
                arguments,
                None,
            )
            .map_err(|error| ControllerError::StreamRejected { code: error.code() })?;

            // The idempotency key is the provider's own call id, scoped by the service's
            // `ReservationKey` to identity, workspace, and principal. That is deliberately the
            // **provider's** identifier rather than a fresh one: a provider that re-sends the same
            // call id within one turn is making the same logical call, and the reservation's whole
            // purpose is to recognise it. A fresh key per dispatch would make every repeat look like
            // a new call and duplicate the effect.
            let outcome = service
                .invoke(
                    turn.context,
                    run.run_id,
                    &intent,
                    &call.call_id,
                    turn.cancel,
                )
                .await
                .map_err(|error| ControllerError::ToolRefused { code: error.code() })?;

            if let crate::tool_call::ToolCallOutcome::WaitingApproval { approval } = outcome {
                return Ok(DispatchOutcome::WaitingApproval { approval });
            }
            observations.push(observation_of(call, &outcome));
        }
        Ok(DispatchOutcome::Observations(observations))
    }

    /// Drives a successful run through `Responding` to `Completed`, storing the answer first.
    ///
    /// The order is the guarantee: the answer is persisted **before** the run reaches `Completed`,
    /// so a storage failure leaves a run that never claims to have delivered anything. See the body
    /// for why the reverse order was a defect.
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
                Step::cancelled_from(RunState::Responding, "cancelled_during_delivery")
                    .with_requester_reason(cancel.cancel_reason()),
            )
            .await?;
            // The answer is **not** stored: a cancelled run's output is not its outcome, and
            // persisting it as the assistant's message would put text into the transcript
            // that the run never delivered.
            return Err(ControllerError::Cancelled);
        }

        // The answer is stored **before** the run reaches `Completed`, and this ordering is the
        // guarantee rather than a detail. It used to be the other way round, which meant a run whose
        // answer storage failed was already durably `Completed`: the transcript was never written
        // and the run's own `run.completed` event claimed a delivered answer that existed nowhere.
        // `store_answer` can fail on a real input — a provider that emits more than
        // `MAX_CONTENT_BYTES` produces a content value the message port refuses — so the failure is
        // reachable, not theoretical. Persisting first makes "the run is completed" mean "its answer
        // is durable", because the durable transition is what a client reads.
        let answer = drained.answer;
        if let Err(error) = self.store_answer(run, conversation_id, &answer, at).await {
            // `Responding -> Failed` is the edge the diagram already carries (added for the
            // provider-failure-during-the-answer case), so a run that cannot deliver its answer has
            // a legal terminal exit rather than being abandoned mid-state.
            self.finish(
                run,
                Step::failed(
                    RunState::Responding,
                    "run.failed",
                    "answer_not_stored",
                    error.code(),
                ),
            )
            .await?;
            return Err(error);
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
                // No usage is passed, and that is deliberate rather than an omission: no usage frame
                // has been folded into `drained` on this path — the wait *is* what failed — so a
                // `usage_of(&drained)` here would fabricate a report for a call that made none. The
                // deadline path therefore records an absent usage, which is honest, while the
                // ceiling path records the report it breached.
                self.finish_deadline_exceeded(run, Some(call_id), DeliveryTiming::of(&drained))
                    .await?;
                return Ok(Err(ControllerError::DeadlineExceeded));
            };
            let event = match frame {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(error) => {
                    return self
                        .fail_after_acceptance(run, call_id, DeliveryTiming::of(&drained), error)
                        .await;
                }
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
                    // The third publication point, and the one a notifying repository could not
                    // cover: a delta goes through `StreamDeltaSink` rather than `RunRepository`, so a
                    // decorator around the repository would leave a live follower blind to exactly
                    // the events a stream exists to deliver. Notified after the sink returns, which
                    // its contract makes durable-before-visible.
                    self.live.notify();
                    // Recorded once, from the **first** delta, because the instant of the first
                    // output is what a time-to-first-token interval measures. `get_or_insert`
                    // rather than an assignment so a later delta cannot move it, and the same
                    // `occurred_at` the durable event carries so the column and the event cannot
                    // name different instants for the same token.
                    drained.first_output_at.get_or_insert(occurred_at);
                    // And the last one unconditionally, because it is the interval's other end:
                    // `get_or_insert` here would freeze the first instant and report a chunk
                    // spread of zero for every call, which is the exact reading that makes a
                    // genuine stream look like a burst.
                    drained.last_output_at = Some(occurred_at);
                    drained.output_delta_count = drained.output_delta_count.saturating_add(1);
                    drained.answer.push_str(delta);
                }
                // Everything else is recorded by folding the frame into the turn, which keeps
                // this loop about *draining a stream* rather than about which frame carries
                // which field. `capture` is where the pattern-ordering rule lives.
                _ => capture(&mut drained, &event),
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
        // cancellation uses `cancelled_from`, whose outcome stays absent while its **reason** is
        // published as the event's payload. The contract maps the three terminal states one-to-one,
        // and `run.cancelled` is not a failure code.
        if cancelled {
            // `reason` here is the *failure* label (`deadline_exceeded`/`provider_refused`), which is
            // wrong for a cancellation and was previously published on the event as the reason even
            // though no payload carried it. The cancellation's own closed-set reason is used instead,
            // so the event's reason and its payload agree about why the run stopped.
            self.finish(
                run,
                // No requester reason: this cancellation arrives as a `ProviderError` rather than
                // from a scope, so nothing here knows who asked — and the reason a provider reports
                // its own cancellation is the provider's, not a caller's. Publishing a label with no
                // requester is the accurate answer, and the alternative would be attributing a
                // decision to somebody who did not make it.
                Step::cancelled_from(RunState::AwaitingModel, "cancelled_before_acceptance"),
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
        delivery: DeliveryTiming,
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
        // The delivery timing is carried through: the provider accepted the call and may have
        // emitted tokens before it failed, so the measurement it produced is real rather than
        // nothing. Passing a default would have recorded "no output" for a call that had sent
        // three tokens.
        self.record_call_outcome_with(
            run,
            call_id,
            ModelCallState::Failed,
            RecordedOutcome {
                delivery,
                ..RecordedOutcome::default()
            },
        )
        .await?;
        // The inner `Err` is the *stream's* failure, which is what the caller reports; the
        // outer `Ok` says draining itself worked. Collapsing them would make "the store
        // broke" and "the provider failed mid-stream" the same value.
        Ok(Err(ControllerError::Provider(error)))
    }

    /// Ends a run whose own deadline expired, recording what the caller will be told.
    ///
    /// A wrapper over [`finish_expired`](Self::finish_expired) so the deadline's reason and code
    /// are one decision. They were passed separately at four call sites, which is four chances for
    /// a run to record a code that disagrees with the error its caller receives — exactly the
    /// divergence the parameter exists to prevent.
    ///
    /// **It takes no usage, because both of its call sites are waits that produced none.** One is a
    /// provider that never opened its stream and the other is a frame wait that elapsed; neither
    /// folded a usage report into its drain. Offering the parameter anyway would invite a caller to
    /// pass a `usage_of(&drained)` over an empty drain, which *fabricates* a report for a call that
    /// made none — so the type makes the honest value the only one expressible.
    async fn finish_deadline_exceeded(
        &self,
        run: RunRef,
        call_id: Option<ModelCallId>,
        delivery: DeliveryTiming,
    ) -> Result<(), ControllerError> {
        self.finish_expired(
            run,
            call_id,
            delivery,
            None,
            "deadline_exceeded",
            ControllerError::DeadlineExceeded.code(),
        )
        .await
    }

    /// Ends a run because its own budget expired.
    ///
    /// One method rather than a transition at each timeout site, so the state, the event, the
    /// reason, and the recorded call outcome cannot disagree about why the run stopped.
    ///
    /// `usage` is the provider's own report for the attempt, when it made one, and carrying it here
    /// is the difference between an explicable termination and a silent one: **this path discards the
    /// run's output**, so the usage is the only evidence of what the provider actually produced. It
    /// was previously written as `RecordedOutcome { delivery, ..default() }` on every path, so a run
    /// that failed for exceeding its output-token budget recorded a model call that consumed
    /// **nothing** — and the reader sent to `run.budget_output_tokens_exceeded` could see no number to
    /// compare against the ceiling. The same shape as `BRN-053`: a client-visible code whose cause the
    /// client could not inspect.
    async fn finish_expired(
        &self,
        run: RunRef,
        call_id: Option<ModelCallId>,
        delivery: DeliveryTiming,
        usage: Option<&Usage>,
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
            // The delivery timing is carried through, because a call abandoned *after*
            // emitting tokens still produced them — and the usage for the same reason, since
            // a call that was ended over its output is the one whose output matters most.
            self.record_call_outcome_with(
                run,
                call_id,
                ModelCallState::Failed,
                RecordedOutcome {
                    usage: usage.cloned(),
                    delivery,
                    ..RecordedOutcome::default()
                },
            )
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
        self.record_call_outcome_with(run, call_id, state, RecordedOutcome::default())
            .await
    }

    /// Records a model call's terminal outcome along with what the provider reported.
    ///
    /// The usage, the finish reason, and the first-output instant are written here rather than in
    /// separate calls so a completed attempt and what it consumed are **one write**: an attempt
    /// recorded as completed with no usage, then updated with usage, leaves a window in which a
    /// reconciliation pass reads a finished call as having consumed nothing — which is exactly the
    /// state a cost ceiling cannot check.
    async fn record_call_outcome_with(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        state: ModelCallState,
        recorded: RecordedOutcome,
    ) -> Result<(), ControllerError> {
        let completed_at = self.now()?;
        // The cost is lifted out of the usage block into its own column, because that is
        // where a cost query reads it. Both are written from one source, so they cannot
        // disagree.
        let estimated_cost_microunits = recorded
            .usage
            .as_ref()
            .and_then(|reported| reported.estimated_cost_microunits);
        self.model_calls
            .record_outcome(
                run.workspace,
                call_id,
                ModelCallOutcome {
                    state,
                    provider_request_id: recorded.provider_request_id,
                    continuation_ref: None,
                    usage: recorded.usage,
                    estimated_cost_microunits,
                    finish_reason: recorded.finish_reason,
                    error_code: None,
                    first_output_at: recorded.delivery.first_output_at,
                    last_output_at: recorded.delivery.last_output_at,
                    // `None` when the attempt produced no output, because `Some(0)` would claim a
                    // measurement of a call that delivered nothing — and a reader could not tell
                    // that from a call genuinely observed to produce zero deltas. The column
                    // documents `NULL` as "not measured", and a delta is what increments the
                    // count, so a count without a first-output instant is a shape that cannot
                    // occur; deriving the `Option` from the instant is what makes that true here
                    // rather than merely intended.
                    output_delta_count: recorded
                        .delivery
                        .first_output_at
                        .map(|_| recorded.delivery.output_delta_count),
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
            // Always absent: nothing in this layer can build the payload, because
            // `serde_json` is a dev-dependency here and this crate deliberately does not depend
            // on `jarvis-protocol`. Recorded as a named gap rather than hand-writing JSON, which
            // is the ad-hoc string manipulation the architecture forbids.
            payload_json: step.payload(),
            visibility: EventVisibility::Public,
            occurred_at: now,
        };
        // The outcome travels on the write rather than being scraped back out of the event, so
        // the transition, its event, and the outcome a client reads are one durable fact.
        let write = match step.outcome {
            Some(outcome) => RunWrite::new(&transition, event).failed_with(outcome),
            None => RunWrite::new(&transition, event),
        };
        let stored = self
            .runs
            .transition(run.workspace, write)
            .await
            .map_err(ControllerError::Repository)?;
        // **After** the write, never before. The contract requires the server to persist an event
        // before making it visible on the stream, and a notification is what makes it visible: a
        // follower woken before the row committed would read a stream that does not yet contain the
        // state change it was told about. Notifying after the write is what makes "persist, then
        // publish" true rather than aspirational.
        self.live.notify();
        Ok(stored)
    }

    /// Publishes a `run.usage` event reporting what a call consumed.
    ///
    /// A **separate event** rather than a field on the terminal frame, because the contract lists
    /// `run.usage` as one of the minimum first-slice event types and a client following the stream
    /// switches on it. It is appended without a transition — the run's state is unchanged — which
    /// is why the repository grew an append that is not a state write.
    ///
    /// Both counters are optional in `Usage` and absent means **unreported**, so each is omitted
    /// rather than sent as `0` when the provider did not report it. The caller has already checked
    /// that at least one counter is present, so this never publishes an event that says nothing.
    async fn publish_usage(
        &self,
        run: RunRef,
        call_id: ModelCallId,
        usage: &Usage,
    ) -> Result<(), ControllerError> {
        let payload = usage_payload(usage, Some(call_id));
        let sequence = self
            .runs
            .next_event_sequence(run.workspace, run.run_id)
            .await
            .map_err(ControllerError::Repository)?;
        self.runs
            .append_event(
                run.workspace,
                NewActivityEvent {
                    run_id: run.run_id,
                    sequence,
                    event_type: USAGE_EVENT.to_owned(),
                    payload_json: Some(payload),
                    visibility: EventVisibility::Public,
                    occurred_at: self.now()?,
                },
            )
            .await
            .map_err(ControllerError::Repository)?;
        // After the append, so a follower reads the usage rather than learning about an event that
        // is not there yet.
        self.live.notify();
        Ok(())
    }

    /// Loads a run, mapping a storage failure.
    async fn load(&self, run: RunRef) -> Result<StoredRun, ControllerError> {
        self.runs
            .load(run.workspace, run.run_id)
            .await
            .map_err(ControllerError::Repository)
    }

    /// Returns the canonical tool names this run may propose.
    ///
    /// An empty vector when no pipeline is composed, which is the honest answer: the model is told it
    /// has no tools rather than being offered tools nothing can run. Sourced from the service rather
    /// than from a second list, so what the model is offered and what policy can authorize are the
    /// same set — a catalog the model saw but the service could not resolve would produce
    /// `tool.not_found` for a tool the model was told about, which reads as a JARVIS bug.
    fn tool_names(&self) -> Vec<String> {
        self.tools
            .as_ref()
            .map(|tools| tools.capabilities())
            .unwrap_or_default()
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
/// Retained for the no-policy case and for tests that assert the *provider's own* selection. The
/// run path uses `RunController::resolve_model`, which prefers a stored route (see
/// `ask_model`, which is where that preference is applied).
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

/// The event type a usage report is published as.
///
/// A local literal rather than `jarvis_protocol::event_type::USAGE`, because this crate cannot
/// depend on `jarvis-protocol`. The two are asserted equal by a cross-check test, which is the
/// technique the workspace uses for every such duplicated contract string — a divergence would be
/// invisible here and would break a client that switches on the event type.
///
/// Public so that cross-check can name it rather than restating the literal, which would make the
/// test agree with a second copy of the value instead of with the value itself.
pub const USAGE_EVENT_TYPE: &str = "run.usage";

/// The event type a usage report is published as, inside this module.
const USAGE_EVENT: &str = USAGE_EVENT_TYPE;

/// Builds the public `run.usage` payload.
///
/// Mirrors `jarvis_protocol::run::usage_payload`'s field names, and the two are asserted equal by
/// a cross-check test. Built by hand for the same reason the failed-run payload is: the application
/// layer has no JSON *dependency*, and every value here comes from a closed set — optional `u64`
/// counters, which can be no other character — so no escaping is required and no caller text can
/// reach the document.
///
/// Both counters are omitted when absent rather than written as `0`, because the contract's own
/// rule is that **unknown is not zero**: a provider that did not report a count has not reported
/// a measured zero, and a client must be able to tell the two apart. `jarvis_protocol`'s builder
/// takes plain integers and therefore cannot express that absence at all.
///
/// Public so the cross-check can call it rather than a second copy of the shape.
#[must_use]
pub fn usage_payload_for_wire(usage: &Usage) -> String {
    usage_payload(usage, None)
}

/// Builds the payload, naming the call when the caller knows it.
///
/// The call identifier is optional rather than mandatory so the cross-check can compare the
/// counter fields against `jarvis_protocol`'s builder, which has no call to name. A published
/// event always supplies one — two calls in a run would otherwise be indistinguishable.
///
/// **Every counter the provider reported is published, and two of them were not.** The payload
/// carried `input_tokens` and `output_tokens` while the adapter parses `cached_input_tokens` and
/// `reasoning_tokens` off the same usage block and the call row stores all four — so a client
/// following a run saw a provider's report **truncated**, and specifically could not see the
/// reasoning spend that a reasoning model charges for. The contract's own usage example lists all
/// four, and the local control API states this event carries "the counters the provider
/// **reported**": a counter JARVIS parsed, stored, and withheld from a public event is the reader
/// half of the same defect this project has recorded four times.
///
/// Each is omitted when the provider did not report it, which is the contract's "unknown is not
/// zero" applied to a durable statement — writing `0` for an unreported count would publish a
/// measurement nobody made.
fn usage_payload(usage: &Usage, call_id: Option<ModelCallId>) -> String {
    let mut fields: Vec<String> = Vec::new();
    if let Some(call_id) = call_id {
        fields.push(format!("\"call_id\":\"{call_id}\""));
    }
    // A table of name/value pairs rather than four `if let` blocks, so the set of counters is one
    // readable list and adding a counter to [`Usage`] has an obvious home. The list is guarded by
    // `usage_payload_publishes_every_counter_the_provider_reported`, because a table still lets an
    // entry be dropped — which is exactly how two counters came to be missing here.
    for (name, value) in [
        ("input_tokens", usage.input_tokens),
        ("output_tokens", usage.output_tokens),
        ("cached_input_tokens", usage.cached_input_tokens),
        ("reasoning_tokens", usage.reasoning_tokens),
    ] {
        if let Some(value) = value {
            fields.push(format!("\"{name}\":{value}"));
        }
    }
    format!("{{{}}}", fields.join(","))
}

/// Folds one non-output frame into the turn being drained.
///
/// One function rather than a match inside the drain loop, so "which frame carries which field"
/// has a single answer and the loop stays about reading a stream. It is also where a
/// **pattern-ordering** rule lives, and that rule is not incidental:
///
/// A terminal frame carries **both** the final usage and the reason the provider stopped. Writing
/// this as two arms — one matching `usage: Some(..)` and one matching `finish_reason` — makes the
/// first match win, so a terminal with usage would record its usage and silently drop its reason.
/// Both fields are therefore captured in one arm, and a test asserts both on the same frame.
///
/// `ToolCallAdded` records the **first** intent and keeps draining, because abandoning a stream
/// early would leave the provider's view and JARVIS's disagreeing about whether the call finished.
///
/// A `call.completed` frame carries **both** the reason the provider stopped and a **safety flag**,
/// and the two are not interchangeable: a provider reports a declined request as an ordinary
/// completion whose reason is `stop`, so reading the reason alone records "the model finished its
/// answer" for a request the model refused. [`completed_reason`] is where the flag is folded in.
///
/// The whole [`ModelStreamEvent`] is taken rather than only its `kind`, because a provider's
/// request id lives in the frame's `provider_metadata` beside the payload rather than on a
/// particular kind. Reading only the kind is exactly what dropped that value: the one frame a real
/// adapter attaches it to is the start frame, whose kind this turn otherwise does not need.
fn capture(drained: &mut DrainedTurn, event: &ModelStreamEvent) {
    if let Some(request_id) = event
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.request_id.as_ref())
    {
        drained.provider_request_id = Some(request_id.clone());
    }
    match &event.kind {
        ModelStreamEventKind::ToolCallAdded {
            call_id, tool_name, ..
        } => {
            // Both records: the first name is what the *refusal* path reports (a tool the fabric
            // cannot resolve at all), and the map is what pairs a later completed frame's arguments
            // with the name they belong to.
            drained.tool_intent.get_or_insert_with(|| tool_name.clone());
            drained
                .tool_names
                .insert(call_id.clone(), tool_name.clone());
        }
        // **A completed tool call is captured with its canonical call id and its argument
        // document.** The domain guarantees the document is whole: `ToolCallCompleted` is the frame
        // a provider sends once, after the deltas, and the stream state machine refuses a completed
        // frame whose arguments disagree with what the deltas assembled. So this is the only frame a
        // dispatch may be built from, which is why the capture happens here rather than on
        // `ToolCallAdded`.
        //
        // The pair is recorded even when the argument document is later refused by validation: the
        // call id is what the observation must pair with, and a tool result that answered nothing
        // would be an orphaned pair the domain's `InputItems::new` refuses.
        ModelStreamEventKind::ToolCallCompleted { call_id, arguments } => {
            drained.tool_calls.push(CapturedToolCall {
                call_id: call_id.clone(),
                capability: drained.tool_names.get(call_id).cloned().unwrap_or_default(),
                arguments: arguments.clone(),
            });
        }
        // Usage arrives on its own frame or with the terminal, and a provider may send both. The
        // last one wins rather than the first, because a later frame is a revision and the
        // terminal's block is the final one — taking the first would under-count a provider that
        // updates as it goes.
        ModelStreamEventKind::UsageUpdated { usage } => {
            drained.usage = Some(usage.clone());
            drained.usage_reported = true;
        }
        ModelStreamEventKind::CallCompleted {
            finish_reason,
            usage,
            refused,
        } => {
            drained.finish_reason = Some(completed_reason(finish_reason, *refused));
            if let Some(usage) = usage {
                drained.usage = Some(usage.clone());
                drained.usage_reported = true;
            }
        }
        // A `call.failed` terminal means the provider reported the call as failed, and this arm is
        // what makes that fact reachable: without it the finish reason stayed `None`, so
        // `finish_attempt` could not tell a failed call from a completed one and drove the run to
        // `Completed`. The reason is `FinishReason::ProviderError` — the domain's own name for
        // "the provider reported an error after the stream opened" — so the run's failure and the
        // recorded finish reason are one fact rather than two spellings.
        //
        // The provider's `code` is deliberately **not** carried on `drained`: it is
        // provider-supplied text, and the run's own error code is a JARVIS vocabulary. The
        // provider's value stays in the frame, where an adapter can log it, rather than becoming
        // a client-visible code.
        ModelStreamEventKind::CallFailed { .. } => {
            drained.finish_reason = Some(FinishReason::ProviderError);
        }
        // A `call.cancelled` terminal means **the provider** stopped the call — a content policy, a
        // provider-side timeout, an operator action — which is a different fact from the caller
        // cancelling, and the controller detects the latter on its own signal. Recording the reason
        // lets `finish_attempt` settle the run `Cancelled` rather than treating the terminal as a
        // delivered answer, which is what it did while this arm was absent.
        ModelStreamEventKind::CallCancelled { .. } => {
            drained.finish_reason = Some(FinishReason::Cancelled);
        }
        // Output text is published as it arrives, and every other frame is either metadata this
        // turn does not need or a kind the state machine has already refused.
        _ => {}
    }
}

/// Resolves the finish reason for a `call.completed` frame from the provider's own reason and its
/// safety flag.
///
/// The contract says `call.completed` carries "safety/refusal metadata", and the flag exists
/// precisely because a provider can report a **refusal** while naming an ordinary completion: a
/// declined request comes back with the safety flag set and the stop named `stop`. Reading the
/// reason alone therefore recorded "the model finished its answer" for a request the model
/// declined — which is why `FinishReason::Refusal` had **no producer anywhere in the workspace**,
/// and why the refusal count the observability contract requires under *Models and Runtimes*
/// could not be computed from any stored value: the fact was on the wire and folded away.
///
/// The flag **upgrades a plain `Stop` and nothing else**. Any other reason is already a more
/// specific statement from the provider — `ContentFilter` says the provider's own filter stopped
/// the output, which is a different fact from the model choosing to refuse, and `Length` says the
/// output was truncated rather than declined — so overwriting those would discard what the provider
/// did report.
fn completed_reason(provider_reason: &FinishReason, refused: bool) -> FinishReason {
    if refused && matches!(provider_reason, FinishReason::Stop) {
        FinishReason::Refusal
    } else {
        provider_reason.clone()
    }
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
    model: &ModelRef,
    items: &[RetainedItem],
    observations: &[ToolObservation],
    tools: &[String],
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
    // **The previous turn's tool calls and their results, as pairs.** Both halves are appended for
    // every call, in the order the calls completed, because a `ToolResult` must follow the
    // `ToolCall` it answers — the domain's `InputItems::new` is order-sensitive and refuses an
    // orphaned result, so appending only results would make every tool-using run fail with an
    // invariant error for a tool that ran perfectly.
    input.extend(observations.iter().flat_map(|observation| {
        let call = InputItem::ToolCall {
            call_id: observation.call_id.clone(),
            tool_name: observation.capability.clone(),
            // **Through the domain's own type**, so the arguments are wrapped rather than
            // reconstructed: `ToolArguments::Complete` is what the executor saw, and building a
            // differently-typed value here would let the transcript and the executed call disagree.
            arguments: jarvis_domain::model::stream::ToolArguments::Complete {
                raw: observation.arguments.clone(),
            },
        };
        let result = InputItem::ToolResult {
            call_id: observation.call_id.clone(),
            is_error: observation.is_error,
            content: observation.content.clone(),
        };
        [call, result]
    }));
    Ok(ModelCallRequest {
        call_id,
        run_id,
        // The routed model, which is the same value the run's own route selected and the
        // `model_calls` row records. Sending a request that named no model left the policy
        // constraining the record rather than the call: an adapter was free to answer with
        // whatever it defaulted to while the row said the routed model served it.
        model: model.clone(),
        route_requirements: RouteRequirements::text(),
        input: InputItems::new(input)
            .map_err(|error| ControllerError::StreamRejected { code: error.code() })?,
        // **The tools the model may propose, from the run's own catalog.** An empty vector is a real
        // state (no pipeline composed, or a catalog with nothing in it) and tells the model it has
        // no tools — which is honest, where omitting the field entirely would look like a provider
        // that does not support the feature.
        tools: tools.to_vec(),
        output_schema: None,
        settings: PortableSettings::default(),
        // The run's budget, not an empty set of limits. Sending `None` here was the
        // defect: a run could carry a deadline that never reached the provider, so an
        // adapter honouring the contract's `limits.deadline` had nothing to honour and
        // a provider was free to wait indefinitely.
        limits: budget.call_limits(),
    })
}
