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
    ContextManifestId, ConversationId, CorrelationId, MessageId, PrincipalId, RequestId, RunId,
    WorkspaceId,
};
use jarvis_domain::model::policy::{PolicyRules, PolicyVersionRef};
use jarvis_domain::model::stream::Role;
use jarvis_domain::run::budget::RunBudget;
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
/// crate is not a dependency of the application layer, and a test in the daemon
/// asserts the two agree.
pub const MAX_OBJECTIVE_BYTES: usize = 32 * 1024;

/// The default wall-clock budget given to a created run, in milliseconds.
///
/// Fifteen minutes. Every run needs *some* deadline, because the budget is what bounds a
/// provider that never answers: without one the run has no deadline at all, so a hang
/// leaves it non-terminal until a daemon restart recovers it. This is a default rather
/// than a ceiling, and it is deliberately generous — a bound tight enough to interrupt
/// real work would trade a hang for false failures.
pub const DEFAULT_RUN_BUDGET_MS: u64 = 900_000;

/// The scoped operation name for run creation, used in the idempotency key.
pub const CREATE_OPERATION: &str = "runs.create";

/// The scoped operation name for cancellation, used in the idempotency key.
pub const CANCEL_OPERATION: &str = "runs.cancel";

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
            Self::Invalid { code, .. } => code,
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
            | Self::PolicyNotFound => false,
            Self::Storage(error) => error.retryable(),
            Self::Controller(error) => error.retryable(),
        }
    }

    /// Returns a message safe for the requesting principal.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::Invalid { message, .. } => message,
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
        let scope = self
            .scopes
            .lock()
            .ok()
            .and_then(|map| map.get(&run).cloned());
        match scope {
            Some(scope) => {
                scope.cancel();
                true
            }
            None => false,
        }
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
        let budget = budget_for(created_at, resolved)?;

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
                    sensitivity: "internal".to_owned(),
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

        let digest = format!("cancel:{}", reason.len());
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
        self.cancellations.cancel(run);
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
        Some((reference, rules)) => budget.with_policy(reference, rules.maximum_sensitivity),
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

/// The context manifest identifier a run's answer was assembled under.
///
/// Reserved for the context slice to populate; a caller that needs a manifest id now
/// uses this so the field's absence is explicit rather than a hardcoded string
/// appearing in two places later.
#[must_use]
pub fn no_context_manifest() -> Option<ContextManifestId> {
    None
}

#[cfg(test)]
mod tests;
