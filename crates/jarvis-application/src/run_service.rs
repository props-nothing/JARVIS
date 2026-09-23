//! The run orchestration service.
//!
//! This is the piece that turns one authenticated request into a durable run: it
//! creates the run and its opening event atomically, claims the idempotency key,
//! spawns the controller that drives it, and records the cancellation intent that
//! stops it. The daemon's HTTP handlers call it and do nothing else, so the boundary
//! stays thin and the orchestration is testable without an HTTP client.
//!
//! Three decisions are deliberate:
//!
//! - **One run is a background task, not a request handler.** A run can outlive the
//!   request that created it, and the contract requires that a client disconnect
//!   never cancels a durable run. Driving it inside the handler would tie its
//!   lifetime to the connection.
//! - **Cancellation is a registered intent before it is a signal.** The contract
//!   requires the intent to be recorded before workers are signalled, so a cancel
//!   that arrives while the run is still starting is not lost. The registry is a
//!   field rather than a global so a test can observe exactly which scopes were
//!   signalled.
//! - **The idempotency key is claimed before the run is spawned.** A retry of a
//!   create must return the original run and must not start a second execution, so
//!   the claim is what decides whether work happens at all.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use jarvis_domain::clock::Clock;
use jarvis_domain::ids::{
    ConversationId, CorrelationId, MessageId, ModelRouteDecisionId, PolicyExceptionId, PrincipalId,
    RequestId, RunId, WorkspaceId,
};
use jarvis_domain::model::capability::CapabilityDescriptor;
use jarvis_domain::model::policy::{PolicyRules, PolicyVersionRef, Sensitivity};
use jarvis_domain::model::routing::{
    RouteCandidate, RouteRequest, RouteSelectionFailure, select_route_explained,
};
use jarvis_domain::model::stream::{Role, RouteRequirements};
use jarvis_domain::run::budget::{RunBudget, RunRoute};
use jarvis_domain::run::state::RunState;

use crate::cancellation::CancellationScope;
use crate::live_events::StreamDeltaSink;
use crate::model::ModelProvider;
use crate::repository::RepositoryError;
use crate::repository::conversation::{
    ConversationRepository, NewConversation, NewMessage, StoredConversation,
};
use crate::repository::model_call::ModelCallRepository;
use crate::repository::policy::ModelDataPolicyRepository;
use crate::repository::run::{
    IdempotencyClaim, NewIdempotencyRecord, NewRun, RunRepository, run_received_event,
};
use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};
use crate::run_controller::{ControllerError, RunController};

/// The largest accepted run objective, in bytes.
///
/// The contract bounds the input text to 32 KiB after UTF-8 decoding, and a
/// different bound here would either accept text the contract refuses or refuse text
/// the contract accepts. It is repeated rather than imported because the protocol
/// crate is not a dependency of the application layer — so this crate **cannot** compare the two,
/// and neither can the protocol crate. The comparison therefore lives in `jarvis-infrastructure`,
/// which depends on both: `the_objective_bound_is_the_one_the_wire_bound_enforces` in `http`'s
/// tests. This comment used to say "a test in the daemon asserts the two agree", which was false —
/// the daemon's only test checks profile source names, and each crate's own test asserted its
/// constant against the same literal, which is the number written twice rather than an agreement.
pub const MAX_OBJECTIVE_BYTES: usize = 32 * 1024;

/// The default wall-clock budget given to a created run, in milliseconds.
///
/// **Re-exported from the domain rather than declared here, because it used to be both.**
/// `DEFAULT_RUN_BUDGET_MS` and `jarvis_domain::run::budget::DEFAULT_RUN_DEADLINE_MS` were separate
/// constants that happened to hold the same number, with nothing comparing them — and the domain's
/// copy had no production caller at all, so it was the *documented* default that the daemon never
/// used. Editing either one alone would have left the default that is written to a run's budget
/// disagreeing with the default the domain publishes, and no test could see it: each constant was
/// asserted against its own literal by its own crate's tests.
///
/// The alias keeps every existing caller spelling the same, so the fix is a deletion rather than a
/// rename. `jarvis-application` may depend on `jarvis-domain`, and does, so there is no boundary
/// being crossed — this is a value with exactly one definition now.
pub use jarvis_domain::run::budget::DEFAULT_RUN_DEADLINE_MS as DEFAULT_RUN_BUDGET_MS;

/// The scoped operation name for run creation, used in the idempotency key.
pub const CREATE_OPERATION: &str = "runs.create";

/// The scoped operation name for cancellation, used in the idempotency key.
pub const CANCEL_OPERATION: &str = "runs.cancel";

/// The data-classification label a run's objective carries.
///
/// One constant for two sites that must agree: the label stored on the objective message and the
/// sensitivity the route decision records. Two literals would let a run's decision say it sent
/// `internal` content while the message it read was labelled `confidential` — the decision and the
/// content would then be judged under different classifications, which is the one thing the
/// ceiling exists to prevent.
const OBJECTIVE_SENSITIVITY: Sensitivity = Sensitivity::Internal;

/// Why a run command could not be carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunServiceError {
    /// The request was invalid before any durable effect.
    Invalid {
        /// The stable, namespaced error code.
        code: &'static str,
        /// A message safe to return to the caller.
        message: &'static str,
    },
    /// The run, conversation, or model call was not found in the caller's scope.
    NotFound,
    /// The idempotency key was reused with different input.
    IdempotencyConflict,
    /// Storage failed.
    Storage(RepositoryError),
    /// The run was created but could not be driven to a terminal state.
    Controller(ControllerError),
    /// The policy named for the run does not exist in the caller's scope.
    ///
    /// A run that named a policy version must not fall back to the workspace's active one: the
    /// caller asked to be governed by *those* rules, and silently applying different ones would
    /// make the run's own record describe a decision nobody made.
    PolicyNotFound,
    /// The policy is in force but no model satisfies it.
    ///
    /// Carries the refusal, including every candidate that was considered and the reason each was
    /// refused, because "no route" alone leaves the caller unable to act: the next step differs
    /// depending on whether the local model was refused for locality or because nobody offered it.
    /// The run is **not created** — a run that existed would either have to be failed immediately
    /// or, worse, executed under a relaxed policy, and neither is what "fails closed" means.
    PolicyUnsatisfied {
        /// The contract's own code for the refusal (`model.policy_unsatisfied`).
        code: &'static str,
        /// The refused candidates, rendered for the caller.
        message: &'static str,
    },
}

impl RunServiceError {
    /// Builds an invalid-request error.
    #[must_use]
    pub const fn invalid(code: &'static str, message: &'static str) -> Self {
        Self::Invalid { code, message }
    }

    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            // One arm for both carried codes: the variant decides *why* the request failed, and
            // the code is a field either way. They are grouped because the body is the same fact
            // rather than by coincidence — a second arm would read as two different answers.
            Self::Invalid { code, .. } | Self::PolicyUnsatisfied { code, .. } => code,
            Self::NotFound => "resource.not_found",
            Self::IdempotencyConflict => "idempotency.conflict",
            Self::Storage(error) => error.code(),
            Self::Controller(error) => error.code(),
            Self::PolicyNotFound => "model.policy_not_found",
        }
    }

    /// Returns whether retrying the same request unchanged could succeed.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            // None of these is fixed by resending the same request. A named policy that does not
            // exist will not exist on a retry either — only a fresh read helps, and that is a
            // different request.
            Self::Invalid { .. }
            | Self::NotFound
            | Self::IdempotencyConflict
            | Self::PolicyNotFound
            | Self::PolicyUnsatisfied { .. } => false,
            Self::Storage(error) => error.retryable(),
            Self::Controller(error) => error.retryable(),
        }
    }

    /// Returns a message safe for the requesting principal.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            // Grouped for the same reason as `code`: both carry the caller-facing message, and
            // the variant is what says which kind of refusal this is.
            Self::Invalid { message, .. } | Self::PolicyUnsatisfied { message, .. } => message,
            Self::NotFound => "No such resource.",
            Self::IdempotencyConflict => {
                "The idempotency key was already used for another request."
            }
            Self::Storage(_) => "The run could not be persisted.",
            Self::Controller(error) => error.message(),
            Self::PolicyNotFound => "No such model data policy.",
        }
    }
}

impl From<RepositoryError> for RunServiceError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound => Self::NotFound,
            other => Self::Storage(other),
        }
    }
}

/// What a create command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedRun {
    /// The run identifier.
    pub run_id: RunId,
    /// The conversation it belongs to.
    pub conversation_id: ConversationId,
    /// Its state when the command returned, which is `Received` for a fresh run and
    /// the original run's current state for a replayed idempotent create.
    pub state: RunState,
    /// When the run was created, so the response reports the persisted instant rather
    /// than one the handler invented.
    pub created_at: jarvis_domain::time::UtcTimestamp,
    /// Whether this command created a new run or replayed an earlier one.
    pub replayed: bool,
}

/// Tracks the cancellation scope of every running run.
///
/// A run is a background task, so the only handle to it is the scope registered at
/// creation. The registry is explicit rather than a global, so a test can assert
/// exactly which scope a cancel signalled, and so two daemons in one process cannot
/// cancel each other's runs.
#[derive(Debug, Default)]
pub struct RunCancellationRegistry {
    scopes: Mutex<HashMap<RunId, CancellationScope>>,
}

impl RunCancellationRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the scope for `run`.
    ///
    /// The scope is stored by value and cloning it shares the signal, so a cancel
    /// arriving after this call cancels the running task.
    pub fn register(&self, run: RunId, scope: CancellationScope) {
        if let Ok(mut map) = self.scopes.lock() {
            map.insert(run, scope);
        }
    }

    /// Signals cancellation for `run`, returning whether it was registered.
    ///
    /// Returns `false` for an unknown run rather than silently succeeding: a cancel
    /// for a run this process is not driving must be reported, so a caller does not
    /// conclude a run was stopped when nothing was signalled.
    pub fn cancel(&self, run: RunId) -> bool {
        self.cancel_with_reason(run, None)
    }

    /// Signals cancellation for `run`, recording `reason` on its scope.
    ///
    /// The reason travels on the **scope** rather than in this registry, because the scope is the
    /// value the controller already holds: a reason stored here would need a second lookup keyed
    /// by run, could disagree with the signal, and would be invisible to any caller holding only
    /// the scope.
    pub fn cancel_with_reason(&self, run: RunId, reason: Option<&str>) -> bool {
        let scope = self
            .scopes
            .lock()
            .ok()
            .and_then(|map| map.get(&run).cloned());
        match scope {
            Some(scope) => {
                scope.cancel_with_reason(reason);
                true
            }
            None => false,
        }
    }

    /// Returns the registered scope for `run`, when this process is driving it.
    ///
    /// Exposed so a test can assert what a cancel actually did to the scope — that it was signalled
    /// *and* what reason it recorded — rather than inferring both from a later terminal state. The
    /// scope is cloned rather than borrowed: a caller holding a reference into the map would keep
    /// the lock held, and the returned scope shares the same signal.
    #[must_use]
    pub fn scope_of(&self, run: RunId) -> Option<CancellationScope> {
        self.scopes.lock().ok()?.get(&run).cloned()
    }

    /// Removes a finished run's scope.
    pub fn forget(&self, run: RunId) {
        if let Ok(mut map) = self.scopes.lock() {
            map.remove(&run);
        }
    }

    /// Returns how many runs are currently cancellable.
    ///
    /// Exposed so a diagnostic line can report a count rather than the scopes
    /// themselves, which would print a run identifier per live run.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.scopes.lock().map_or(0, |map| map.len())
    }
}

/// The run orchestration ports.
///
/// Bundled into one value because every operation needs the same set and passing
/// them separately pushed each constructor past the argument limit.
#[derive(Clone)]
pub struct RunPorts {
    /// The run store.
    pub runs: Arc<dyn RunRepository>,
    /// The conversation and message store.
    pub conversations: Arc<dyn ConversationRepository>,
    /// The model-call store.
    pub model_calls: Arc<dyn ModelCallRepository>,
    /// Publishes live output deltas.
    pub deltas: Arc<dyn StreamDeltaSink>,
    /// Serves models.
    pub provider: Arc<dyn ModelProvider>,
    /// Supplies instants.
    pub clock: Arc<dyn Clock>,
    /// Resolves the policy a run executes under, when a store is configured.
    ///
    /// Optional so the composition can build a run service without a policy store, which is the
    /// state the Foundation surfaces and several tests need. `None` is not a permissive default:
    /// a run created without this port records **no policy**, and the controller treats that as
    /// "hold nothing back but record every label" rather than as "a policy permitted everything".
    pub policies: Option<Arc<dyn ModelDataPolicyRepository>>,
}

/// Orchestrates run creation, execution, and cancellation.
pub struct RunService {
    ports: RunPorts,
    cancellations: Arc<RunCancellationRegistry>,
}

impl std::fmt::Debug for RunService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The ports are not printed: a provider can hold credentials and a repository a
        // connection string, and a debug line reaches a log file.
        formatter
            .debug_struct("RunService")
            .field("live_runs", &self.cancellations.live_count())
            .finish_non_exhaustive()
    }
}

impl RunService {
    /// Builds a service over the given ports.
    #[must_use]
    pub fn new(ports: RunPorts, cancellations: Arc<RunCancellationRegistry>) -> Self {
        Self {
            ports,
            cancellations,
        }
    }

    /// Builds a controller over this service's ports.
    #[must_use]
    pub fn controller(&self) -> RunController {
        RunController::new(
            Arc::clone(&self.ports.runs),
            Arc::clone(&self.ports.conversations),
            Arc::clone(&self.ports.model_calls),
            Arc::clone(&self.ports.deltas),
            Arc::clone(&self.ports.provider),
            Arc::clone(&self.ports.clock),
        )
    }

    /// Creates a run for `context` and returns it without waiting for it to finish.
    ///
    /// The run is created in `Received` with its `run.received` event in one write,
    /// then driven by a background task. The method returns as soon as the run is
    /// durable, which is what lets the API answer `202 Accepted` with a run a client
    /// can immediately stream.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError::Invalid`] when the objective or the idempotency key
    /// is unusable, [`RunServiceError::IdempotencyConflict`] when the key was used
    /// for different input, [`RunServiceError::NotFound`] when an existing
    /// conversation is not in the caller's scope, and [`RunServiceError::Storage`]
    /// when the write fails.
    pub async fn create(
        &self,
        context: &RequestContext,
        conversation_id: Option<ConversationId>,
        objective: &str,
        idempotency_key: &str,
        requested_policy: Option<PolicyVersionRef>,
        spawn: &dyn RunSpawner,
    ) -> Result<CreatedRun, RunServiceError> {
        if objective.is_empty() || objective.len() > MAX_OBJECTIVE_BYTES || objective.contains('\0')
        {
            return Err(RunServiceError::invalid(
                "request.semantic_invalid",
                "The run input is empty or over the bounded limit.",
            ));
        }
        if idempotency_key.is_empty() {
            return Err(RunServiceError::invalid(
                "request.invalid",
                "An idempotency key is required.",
            ));
        }

        let run_id = RunId::from_uuid(uuid::Uuid::now_v7());
        let created_at = self.now()?;

        // The policy is resolved **before** the budget is finished, because the ceiling the run
        // executes under is one of the budget's terms. Resolving it after creating the run would
        // mean a run exists whose rules are not yet known — the state the contract's "the run
        // records the policy it resolved and authorized" rule exists to prevent.
        //
        // Resolution is deliberately *not* an error when no policy exists. A workspace with no
        // policy is a real state, and refusing to create a run in it would make the daemon
        // unusable until an operator wrote a policy — while silently applying a permissive one
        // would attribute a decision to nobody. Instead the run records **no** policy, and the
        // controller holds nothing back while recording every label.
        let resolved = self.resolve_policy(context, requested_policy).await?;

        // The route is selected **before** the budget is finished, because the model the run may
        // call is one of the decision's outputs. It is selected before the run exists, so a policy
        // that admits no compliant model refuses the request rather than creating a run that would
        // have to be governed by something. The sensitivity is the objective's own label, which is
        // the content about to be sent: `internal` is the label `append_objective` stores for a
        // run input, and recording a different one here would make the decision describe a call
        // that never happened.
        let route = self
            .select_route(context, resolved.clone(), OBJECTIVE_SENSITIVITY, created_at)
            .await?;
        let budget = budget_for(created_at, resolved, route)?;

        // The key is checked *before* anything is created, because a replay must not
        // leave an orphan conversation behind. The digest is over the inputs that
        // define the request, not over a serialized body: two requests that differ only
        // in JSON key order are the same request, and two that differ in objective are
        // not. The check below is advisory — `create_run_idempotent` is what actually
        // decides, under one write.
        if let Some((digest, original)) = self
            .ports
            .runs
            .lookup_idempotency(context.workspace_id, CREATE_OPERATION, idempotency_key)
            .await?
        {
            // The digest needs the conversation id, which a replay does not know, so a
            // replay is compared on the objective only. That is the honest scope: the
            // conversation is an output of the first request, and a caller replaying
            // the same command sends the same objective.
            if digest == objective_digest(objective) {
                let stored = self.ports.runs.load(context.workspace_id, original).await?;
                return Ok(CreatedRun {
                    run_id: stored.id,
                    conversation_id: stored.conversation_id,
                    state: stored.state,
                    created_at: stored.created_at,
                    replayed: true,
                });
            }
            return Err(RunServiceError::IdempotencyConflict);
        }

        // The conversation is resolved or created only after the replay check, so a
        // replayed command does no work and leaves nothing behind.
        let conversation = match conversation_id {
            Some(existing) => self.load_conversation(context, existing).await?,
            None => self.create_conversation(context, objective).await?,
        };

        let digest = request_digest(&conversation.id, objective);
        // The run, its opening event, and the idempotency record commit together, so a
        // key cannot be claimed by a command whose run was never written, and a run
        // cannot be created twice by two requests that both passed the check above.
        match self
            .ports
            .runs
            .create_run_idempotent(
                NewRun::with_budget(
                    run_id,
                    context.workspace_id,
                    conversation.id,
                    context.principal_id,
                    // A bounded reference, never the objective text: large content
                    // belongs in artifacts, and the column is bounded.
                    Some(truncate_reference(objective)),
                    created_at,
                    budget,
                )?,
                run_received_event(run_id, created_at),
                NewIdempotencyRecord {
                    key: idempotency_key.to_owned(),
                    workspace_id: context.workspace_id,
                    operation: CREATE_OPERATION.to_owned(),
                    request_digest: digest,
                    run_id,
                    created_at,
                },
            )
            .await?
        {
            IdempotencyClaim::Claimed => {}
            IdempotencyClaim::Replay(original) => {
                // A concurrent request won the claim. The conversation this request
                // created is then an orphan, so it is cleaned up rather than left
                // behind — a replay must leave no trace of the work it did not need.
                if conversation_id.is_none() {
                    self.discard_conversation(context, conversation.id).await;
                }
                let stored = self.ports.runs.load(context.workspace_id, original).await?;
                return Ok(CreatedRun {
                    run_id: stored.id,
                    conversation_id: stored.conversation_id,
                    state: stored.state,
                    created_at: stored.created_at,
                    replayed: true,
                });
            }
            IdempotencyClaim::Conflict => return Err(RunServiceError::IdempotencyConflict),
        }

        self.append_objective(context, conversation.id, objective, created_at)
            .await?;

        // The run is driven in the background, so a client can stream it while the
        // request that created it has already returned.
        self.spawn_run(context, run_id, conversation.id, objective, spawn);

        Ok(CreatedRun {
            run_id,
            conversation_id: conversation.id,
            state: RunState::Received,
            created_at,
            replayed: false,
        })
    }

    /// Stores the objective as the user's message in the conversation.
    ///
    /// The controller reads the transcript to build the model request, so without
    /// this the model would be asked to answer a question it cannot see.
    async fn append_objective(
        &self,
        context: &RequestContext,
        conversation: ConversationId,
        objective: &str,
        created_at: jarvis_domain::time::UtcTimestamp,
    ) -> Result<(), RunServiceError> {
        self.ports
            .conversations
            .append_message(
                context.workspace_id,
                NewMessage {
                    id: MessageId::from_uuid(uuid::Uuid::now_v7()),
                    conversation_id: conversation,
                    role: Role::User,
                    content: objective.to_owned(),
                    content_schema_version: 1,
                    sensitivity: OBJECTIVE_SENSITIVITY.as_str().to_owned(),
                    source: "api".to_owned(),
                    created_at,
                }
                .validated()?,
            )
            .await?;
        Ok(())
    }

    /// Registers the run's cancellation scope and schedules its execution.
    ///
    /// The scope is registered before the task is scheduled, so a cancel arriving
    /// between the two answers is not lost: the registry is what a cancel signals, and
    /// the task observes the signal when it starts.
    fn spawn_run(
        &self,
        context: &RequestContext,
        run_id: RunId,
        conversation: ConversationId,
        objective: &str,
        spawn: &dyn RunSpawner,
    ) {
        let scope = context.cancellation().child();
        self.cancellations.register(run_id, scope.clone());

        let controller = self.controller();
        let registry = Arc::clone(&self.cancellations);
        let objective = objective.to_owned();
        let request_context = RequestContext::new(
            context.request_id,
            context.correlation_id,
            context.principal_id,
            context.assurance,
            context.workspace_id,
            RequestChannel::Api,
        )
        .with_cancellation(scope);

        spawn.spawn(Box::pin(async move {
            // The outcome is deliberately not propagated: the run's terminal state is
            // durable and is what a client reads, so a controller error is already
            // recorded as the run's state rather than needing a channel to nowhere.
            let _outcome = controller
                .execute(
                    &request_context,
                    run_id,
                    conversation,
                    &objective,
                    request_context.cancellation(),
                )
                .await;
            registry.forget(run_id);
        }));
    }

    /// Records cancellation intent for `run` and signals its scope.
    ///
    /// The intent is claimed under the caller's idempotency key first, so a repeated
    /// cancel is idempotent and a cancel for an unknown run is a `NotFound` rather
    /// than a success that stopped nothing.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError::NotFound`] for a foreign or unknown run,
    /// [`RunServiceError::IdempotencyConflict`] when the key was reused with different
    /// input, and [`RunServiceError::Storage`] when the write fails.
    pub async fn cancel(
        &self,
        context: &RequestContext,
        run: RunId,
        reason: &str,
        idempotency_key: &str,
    ) -> Result<RunState, RunServiceError> {
        if idempotency_key.is_empty() {
            return Err(RunServiceError::invalid(
                "request.invalid",
                "An idempotency key is required.",
            ));
        }

        // The run is loaded first so a foreign run is `NotFound` before any record is
        // written for it.
        let stored = self.ports.runs.load(context.workspace_id, run).await?;

        // The digest folds the reason's **content**, not its length. It was `reason.len()`, which
        // made every pair of reasons with the same length one request: a caller that cancelled
        // two runs with `"user_requested"` and `"timeout_reason"` and reused its key would be
        // told its second cancel was a replay of the first and the run would never be signalled.
        // A digest that cannot distinguish the inputs it claims to identify is worse than none,
        // because the caller is given a confident and wrong answer.
        let digest = format!("cancel:{}", reason_digest(reason));
        match self
            .ports
            .runs
            .claim_idempotency(NewIdempotencyRecord {
                key: idempotency_key.to_owned(),
                workspace_id: context.workspace_id,
                operation: CANCEL_OPERATION.to_owned(),
                request_digest: digest,
                run_id: run,
                created_at: self.now()?,
            })
            .await?
        {
            IdempotencyClaim::Conflict => return Err(RunServiceError::IdempotencyConflict),
            // A repeated cancel is idempotent: it reports the run's current state and
            // signals nothing, because the intent is already durable.
            IdempotencyClaim::Replay(_) => return Ok(stored.state),
            IdempotencyClaim::Claimed => {}
        }

        // The contract is explicit: cancellation does not report `cancelled` until
        // bounded cleanup reaches a durable terminal transition. So a terminal run is
        // returned unchanged, and a live run is signalled and still reports its
        // current state — the controller will persist `Cancelled` when it observes the
        // signal, which is what makes the reported state truthful.
        if stored.state.is_terminal() {
            return Ok(stored.state);
        }
        self.cancellations.cancel_with_reason(run, Some(reason));
        Ok(stored.state)
    }

    /// Reads one run in the caller's scope.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError::NotFound`] for an absent or foreign run.
    pub async fn read(
        &self,
        context: &RequestContext,
        run: RunId,
    ) -> Result<crate::repository::run::StoredRun, RunServiceError> {
        Ok(self.ports.runs.load(context.workspace_id, run).await?)
    }

    /// Reads a page of a run's public events.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError::NotFound`] for an absent or foreign run.
    pub async fn events(
        &self,
        context: &RequestContext,
        run: RunId,
        from_sequence: u64,
    ) -> Result<crate::repository::run::RunEventPage, RunServiceError> {
        Ok(self
            .ports
            .runs
            .load_events(
                context.workspace_id,
                run,
                from_sequence,
                crate::repository::run::MAX_EVENT_PAGE,
            )
            .await?)
    }

    /// Reads the sequence a retained public event carries, by identifier.
    ///
    /// Exists so a resume position is resolved against the **stream** rather than against one page
    /// of it. `events` is bounded to [`MAX_EVENT_PAGE`](crate::repository::run::MAX_EVENT_PAGE) and
    /// a run's stream is not, so a caller that searched a single page concluded that an event past
    /// that page was no longer retained — and reported `stream.replay_unavailable`, which the
    /// contract reserves for a position the daemon genuinely no longer has.
    ///
    /// # Errors
    ///
    /// Returns [`RunServiceError::NotFound`] for an absent or foreign run, and
    /// [`RepositoryError::NotFound`](crate::repository::RepositoryError::NotFound) when the event is
    /// not a retained public event of that run.
    pub async fn event_sequence(
        &self,
        context: &RequestContext,
        run: RunId,
        event_id: &str,
    ) -> Result<u64, RunServiceError> {
        Ok(self
            .ports
            .runs
            .load_event_sequence(context.workspace_id, run, event_id)
            .await?)
    }

    /// Resolves the policy this run executes under.
    ///
    /// A named version is honoured exactly, and an absent one means the workspace's active
    /// policy. The difference matters: a caller that named a version is asking to be governed by
    /// *those* rules, so falling back to the active policy when the named one is gone would apply
    /// rules it never named and record a decision nobody made. An absent name is the ordinary
    /// case — the request did not choose — and there the active policy is exactly what is meant.
    ///
    /// Returns `None` when no policy is in force, which is a real state rather than an error. The
    /// distinction between "no policy" and "a permissive policy" is preserved into the stored
    /// budget, because only the first is a configuration an operator should be told about.
    async fn resolve_policy(
        &self,
        context: &RequestContext,
        requested: Option<PolicyVersionRef>,
    ) -> Result<Option<(PolicyVersionRef, PolicyRules)>, RunServiceError> {
        let Some(policies) = self.ports.policies.as_ref() else {
            // No policy store is configured, which is the Foundation composition and several
            // tests. Reported as "no policy in force" rather than as a fault: the run is
            // creatable, and its record says plainly that no policy governed it.
            return Ok(None);
        };
        let stored = match requested {
            Some(reference) => policies
                .load_version(context.workspace_id, reference)
                .await
                // A named version that does not exist is a refusal, not a fallback. Mapped from
                // the store's `NotFound`, which is also what a version owned by another
                // workspace returns — the two are indistinguishable by design.
                .map_err(|error| match error {
                    RepositoryError::NotFound => RunServiceError::PolicyNotFound,
                    other => RunServiceError::Storage(other),
                })?,
            None => match policies.load_active(context.workspace_id).await {
                Ok(stored) => stored,
                // No active policy is the ordinary empty workspace, not a fault.
                Err(RepositoryError::NotFound) => return Ok(None),
                Err(other) => return Err(RunServiceError::Storage(other)),
            },
        };
        Ok(Some((stored.reference(), stored.rules)))
    }

    /// Loads or creates the conversation for a new run.
    async fn load_conversation(
        &self,
        context: &RequestContext,
        conversation: ConversationId,
    ) -> Result<StoredConversation, RunServiceError> {
        Ok(self
            .ports
            .conversations
            .load_conversation(context.workspace_id, conversation)
            .await?)
    }

    /// Creates a conversation titled from the objective.
    async fn create_conversation(
        &self,
        context: &RequestContext,
        objective: &str,
    ) -> Result<StoredConversation, RunServiceError> {
        let id = ConversationId::from_uuid(uuid::Uuid::now_v7());
        let created_at = self.now()?;
        self.ports
            .conversations
            .create_conversation(NewConversation::new(
                id,
                context.workspace_id,
                context.principal_id,
                Some(truncate_reference(objective)),
                "api".to_owned(),
                created_at,
            )?)
            .await?;
        Ok(StoredConversation {
            id,
            workspace_id: context.workspace_id,
            owner_user_id: context.principal_id,
            title: Some(truncate_reference(objective)),
            archived: false,
            created_at,
            updated_at: created_at,
        })
    }

    /// Best-effort removal of a conversation created for a run that was not created.
    ///
    /// A failure is ignored on purpose: the run the caller asked about is unaffected,
    /// and reporting a cleanup failure as the command's outcome would report a fault
    /// that did not affect the answer. An orphan conversation is harmless — it is
    /// empty and scoped to the caller's own workspace — whereas a failed command that
    /// actually succeeded is not.
    async fn discard_conversation(&self, context: &RequestContext, conversation: ConversationId) {
        let _ = self
            .ports
            .conversations
            .discard_if_empty(context.workspace_id, conversation)
            .await;
    }

    /// Returns the current instant.
    fn now(&self) -> Result<jarvis_domain::time::UtcTimestamp, RunServiceError> {
        self.ports
            .clock
            .now()
            .map_err(|_| RunServiceError::Storage(RepositoryError::Query))
    }

    /// Selects and records the route this run is authorized to call.
    ///
    /// **This is the step that makes a data policy govern a real call.** The selector and the
    /// store existed and the diagnostic `GET /model-data-policy/effective` used both, so a probe
    /// answered correctly while the run the daemon actually executed took `provider.models().first()`
    /// — a locality or allow-list rule constrained the *report* and not the call. Resolving here,
    /// at creation, and refusing a run that has no compliant route closes that.
    ///
    /// Four decisions:
    ///
    /// - **A run with no policy in force records no route and is still created.** That is the
    ///   fresh-install case and the contract's "absent means the workspace's active policy"; with
    ///   neither, nothing was decided, and inventing a route would attribute a decision to nobody.
    /// - **A policy in force that admits no candidate refuses the run.** It does not create a run
    ///   that is immediately failed, and it certainly does not relax a rule: the contract's
    ///   `model.policy_unsatisfied` is "not permission to silently relax policy", and a created run
    ///   would have to be governed by something.
    /// - **The candidate inventory is the provider's own served models at its own endpoint class.**
    ///   Built from the same provider the controller will call, so a probe and a run cannot
    ///   disagree about which models exist or where the endpoint sits.
    /// - **The decision is recorded before the run executes**, so a failed call still explains
    ///   which route it was authorized to take. Recording it afterwards would leave a failed run's
    ///   route unanswerable, which is exactly when an operator asks.
    /// - **A single-use grant is spent in the same step.** A selection that relied on one must
    ///   consume it, or an operator's single-use approval would permit an unbounded number of
    ///   runs. The consumption follows the decision and precedes the run, so the ordering is
    ///   itself the guarantee: a failure to record leaves the grant intact, and nothing is spent
    ///   before an audit can name what it permitted.
    async fn select_route(
        &self,
        context: &RequestContext,
        resolved: Option<(PolicyVersionRef, PolicyRules)>,
        sensitivity: Sensitivity,
        created_at: jarvis_domain::time::UtcTimestamp,
    ) -> Result<Option<RunRoute>, RunServiceError> {
        let Some((reference, rules)) = resolved else {
            // No policy in force: nothing to select against, and nothing to record.
            return Ok(None);
        };

        let candidates = route_candidates(self.ports.provider.as_ref());
        // The evidence-revalidation day is derived from the same instant the decision records, so
        // the day and the instant cannot straddle midnight and describe different moments.
        let today = created_at.utc_date();
        let request = RouteRequest {
            rules,
            policy: reference,
            sensitivity,
            // The run's own requirements. A plain text turn with no capability floor, which is
            // what the controller will actually send; a floor naming a capability nothing
            // measures would refuse every candidate on an evidence gap rather than a policy
            // decision (`BRN-011`).
            requirements: RouteRequirements::text(),
            today,
            decided_at: created_at,
            // The workspace's grants are offered to the selector, which decides which apply. Read
            // from the store rather than taken from the caller: an exception is a durable record,
            // and a client-supplied one would be the "editing a request body creates an exception"
            // path the contract forbids outright.
            exceptions: match self.ports.policies.as_ref() {
                Some(policies) => policies
                    .list_exceptions(context.workspace_id)
                    .await
                    .map_err(RunServiceError::Storage)?,
                None => Vec::new(),
            },
        };

        let decision = match select_route_explained(&candidates, &request) {
            Ok(decision) => decision,
            Err(RouteSelectionFailure::Refused(_)) => {
                return Err(RunServiceError::PolicyUnsatisfied {
                    code: "model.policy_unsatisfied",
                    message: "No model satisfies the model data policy in force.",
                });
            }
            Err(RouteSelectionFailure::CandidatesUnbounded { .. }) => {
                return Err(RunServiceError::PolicyUnsatisfied {
                    // The offered set was never examined, so this is NOT a refusal. Reusing the
                    // refusal's code would tell a caller its policy rejected every model when the
                    // truth is that too many were offered — the distinction the selection-failure
                    // enum exists to keep.
                    code: "jarvis.context_candidates_unbounded",
                    message: "Too many model candidates were offered to evaluate.",
                });
            }
        };

        // Only the ports that hold a policy store record decisions. Both are configured together
        // in the daemon, so this is not a second decision: without a store, a decision would have
        // nowhere to live and a grant could not have been read either.
        let Some(policies) = self.ports.policies.as_ref() else {
            return Ok(None);
        };
        let decision_id = ModelRouteDecisionId::from_uuid(uuid::Uuid::now_v7());
        policies
            .record_decision(context.workspace_id, decision_id, decision.clone())
            .await
            .map_err(RunServiceError::Storage)?;

        // A **single-use** grant is spent here, in the same step that records the decision that
        // relied on it. This is the half of the exception lifecycle that had a writer and no
        // production caller: `consume_exception` existed, was tested, and nothing on the run path
        // called it — so a grant an operator marked single-use permitted unlimited runs, which is
        // the permissive direction of "a field nothing enforces".
        //
        // It is placed after the decision is durable and before the run exists, and the order is
        // the point. Consuming before the decision would spend a grant on a selection that might
        // then fail to record; consuming after the run is created would let a run be created
        // whose grant is still unspent if anything between the two failed. Here a failure to
        // record leaves the grant intact and the request refused, and the decision is on disk
        // before anything is spent — so an audit can always show what the grant permitted.
        //
        // The store decides whether the grant is single-use, in the same statement that guards
        // it, so this cannot disagree with the adapter's predicate by re-reading the record.
        if let Some(reference) = decision.exception_ref.as_deref() {
            let exception_id = reference.parse::<PolicyExceptionId>().map_err(|_| {
                RunServiceError::Storage(RepositoryError::Corrupted {
                    column: "exception_ref",
                })
            })?;
            policies
                .consume_exception(context.workspace_id, exception_id, created_at)
                .await
                .map_err(RunServiceError::Storage)?;
        }

        Ok(Some(RunRoute {
            model: decision.effective.model,
            decision: decision_id,
            exception_ref: decision.exception_ref,
        }))
    }
}

/// Builds the candidate list from a provider, applying each served model's own endpoint class.
///
/// A free function so the daemon's `ProviderInventory` and this caller produce the **same**
/// candidates from the same provider. Two builders would let the diagnostic probe and the run
/// disagree about which models exist, which is the failure mode an operator would most struggle
/// to see: the probe would report a route the run never took.
#[must_use]
pub fn route_candidates(provider: &dyn ModelProvider) -> Vec<RouteCandidate> {
    let endpoint_class = provider.endpoint_class();
    provider
        .models()
        .iter()
        .map(|model| RouteCandidate {
            descriptor: CapabilityDescriptor::new(model.clone(), endpoint_class),
            model: model.clone(),
            endpoint_class,
            // The region is unknown because no provider publishes one through this port yet, and
            // an unknown region fails an allow-list rather than passing it — the fail-closed
            // direction, since a provider that does not say where it processes cannot be shown to
            // be inside an allowed region.
            region: None,
            // No retention or training-use evidence: `BRN-011` measures capabilities and no adapter
            // attaches provider terms yet. `None` is the honest value and it makes a policy that
            // demands documentation refuse rather than pass on a guess.
            retention: None,
            training_use: None,
        })
        .collect()
}
/// Builds the budget a newly created run starts with.
///
/// Every run gets a budget, derived rather than left unset. An unset budget is not a neutral
/// default: it means the run has no deadline at all, so a provider that hangs would hold it open
/// indefinitely and the run would never reach a terminal state on its own. The default is bounded
/// and finite, and a caller that needs a different one can supply it once the schema carries typed
/// overrides.
///
/// The resolved policy is folded in here rather than at the call site, so the recorded budget and
/// the policy it came from are set in one place. A later reader can then explain why content was
/// held back without re-deriving it from a policy that may since have been archived; a run created
/// with no policy records none, which is a fact an operator can act on rather than a permissive
/// default they cannot see.
fn budget_for(
    created_at: jarvis_domain::time::UtcTimestamp,
    resolved: Option<(PolicyVersionRef, PolicyRules)>,
    route: Option<RunRoute>,
) -> Result<RunBudget, RunServiceError> {
    let budget = RunBudget::expiring_after(created_at, DEFAULT_RUN_BUDGET_MS).map_err(|error| {
        // A fixed message rather than the budget error's own text: the error is a bound on a
        // constant, so a caller can do nothing about it, and the client-visible message must
        // not carry developer detail.
        RunServiceError::invalid(
            error.code(),
            "The run's time budget could not be established.",
        )
    })?;
    Ok(match resolved {
        Some((reference, rules)) => {
            let budget = budget.with_policy(reference, rules.maximum_sensitivity);
            // The route is attached only when the policy was in force, which is what keeps
            // "a policy selected this model" distinguishable from "no policy governed this run"
            // — the same distinction the reference and the ceiling already draw.
            match route {
                Some(route) => budget.with_route(route),
                None => budget,
            }
        }
        None => budget,
    })
}

/// Starts a background task for a run.
///
/// A trait rather than a direct `tokio::spawn` so a test can drive the future to
/// completion deterministically instead of racing a real task, which is what makes
/// "the run reached a terminal state" an assertion rather than a wait.
pub trait RunSpawner: Send + Sync + std::fmt::Debug {
    /// Spawns `task`.
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>);
}

/// Spawns onto the current Tokio runtime.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSpawner;

impl RunSpawner for TokioSpawner {
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        tokio::spawn(task);
    }
}

/// A deterministic digest of the inputs that define a create request.
///
/// Deliberately not a cryptographic hash: the digest only has to distinguish one
/// request from another within a workspace, and it never reaches a client. A
/// cryptographic digest would imply a tamper-resistance property nothing here needs.
fn request_digest(conversation: &ConversationId, objective: &str) -> String {
    let _ = conversation;
    objective_digest(objective)
}

/// A deterministic digest of a cancellation's defining input.
///
/// Folds the reason's **bytes**, so two reasons that differ are two requests. The fold is shared
/// with the create path's objective digest on purpose: both answer "is this the same request",
/// and a second fold implementation is a second answer to that question.
fn reason_digest(reason: &str) -> String {
    objective_digest(reason)
}

/// A deterministic digest of the request's defining input.
///
/// A deterministic, non-cryptographic fold of the objective text. Deliberately not a
/// cryptographic hash: this only has to distinguish one request from another within a
/// workspace and never reaches a client, and a cryptographic digest would imply a
/// tamper-resistance property nothing here relies on.
///
/// The conversation is **not** folded in. It is a reference the caller may omit, and
/// the contract's own example identifies the input as the text: two commands with one
/// key and the same text are the same request whether or not the caller named a
/// conversation. Including it would make a replay of a create-without-conversation
/// impossible to recognise, because the id is generated on the first call.
fn objective_digest(objective: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in objective.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Truncates text to a bounded reference, respecting character boundaries.
///
/// A byte slice at an arbitrary offset could split a multi-byte character, which is
/// why this walks characters rather than cutting the string.
fn truncate_reference(text: &str) -> String {
    const MAX: usize = 200;
    if text.len() <= MAX {
        return text.to_owned();
    }
    let mut out = String::with_capacity(MAX);
    for character in text.chars() {
        if out.len() + character.len_utf8() > MAX {
            break;
        }
        out.push(character);
    }
    out
}

/// Builds a request context for an authenticated local client.
///
/// Provided here rather than in the HTTP layer so the channel and assurance are set
/// in one place: an API request is `Api`/`Standard`, and a handler that assembled its
/// own context could claim a more privileged channel.
#[must_use]
pub fn api_request_context(
    workspace: WorkspaceId,
    principal: PrincipalId,
    request_id: RequestId,
    correlation: CorrelationId,
) -> RequestContext {
    RequestContext::new(
        request_id,
        correlation,
        principal,
        AuthenticationAssurance::Standard,
        workspace,
        RequestChannel::Api,
    )
}

// **`no_context_manifest()` was here and is deleted.** It returned `None` for a run's
// `agent_runs.context_manifest_id`, and its doc said "a caller that needs a manifest id now uses
// this so the field's absence is explicit". No caller existed — not in production, not in a test,
// and not in a doc — so it was the absence made explicit for nobody, while the column stayed NULL
// for every run because nothing populates it at all (`agent-runs.md` records that the
// `context_manifests` table is `MEM-008`). Its own `must_use` marked it as something a caller
// ought to heed, which is exactly the shape that makes a dead function read as load-bearing.
// When the manifest is persisted, the read of a stored run is where the `Option` belongs — not a
// free function returning a constant.

#[cfg(test)]
mod tests;
