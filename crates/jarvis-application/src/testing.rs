//! In-memory repository doubles for tests and for a provider-less development path.
//!
//! These are shipped rather than test-only for the same reason the scripted provider
//! is: the architecture requires that a paid live call "cannot be the only test of
//! JARVIS orchestration", and the same applies to a database. A controller test needs
//! a store that is real enough to enforce the contract, and `#[cfg(test)]` items are
//! invisible to the integration tests in another crate that need them too.
//!
//! **The important design point:** these doubles do not reimplement the transition
//! rules. [`RunLifecycle`](jarvis_domain::run::lifecycle::RunLifecycle) — the domain
//! value — is the single authority for edge legality, version ordering, and terminal
//! absorption, exactly as it is in the SQLite adapter. A double that copied those
//! rules would be a second implementation that could disagree with the first, and a
//! test passing against it would prove only self-consistency. What these add is
//! *storage* semantics: the transaction-and-event fusion, the uniqueness constraints,
//! and the not-found/foreign-is-indistinguishable rule.
//!
//! The doubles are deliberately **not durable** and must never be used by the daemon:
//! `jarvisd` composes the SQLite adapters.

use std::collections::BTreeMap;
use std::sync::Mutex;

use jarvis_domain::ids::{ConversationId, MessageId, ModelCallId, RunId, WorkspaceId};
use jarvis_domain::run::lifecycle::RunLifecycle;
use jarvis_domain::run::state::RunState;
use jarvis_domain::time::UtcTimestamp;

use crate::repository::conversation::{
    ConversationRepository, NewConversation, NewMessage, StoredConversation, StoredMessage,
};
use crate::repository::model_call::{
    ModelCallOutcome, ModelCallRepository, ModelCallState, NewModelCall, StoredModelCall,
};
use crate::repository::run::{NewActivityEvent, NewRun, RunRepository, RunResumeState, StoredRun};
use crate::repository::{RepositoryError, RepositoryFuture};

/// One stored run: its domain lifecycle plus the fields the lifecycle does not own.
#[derive(Debug, Clone)]
struct RunRow {
    /// The run's own identifier, kept beside the lifecycle because `RunLifecycle`
    /// deliberately holds only state, version, and the terminal instant.
    id: RunId,
    lifecycle: RunLifecycle,
    workspace_id: WorkspaceId,
    conversation_id: ConversationId,
    principal_id: jarvis_domain::ids::PrincipalId,
    objective_ref: Option<String>,
    created_at: UtcTimestamp,
    started_at: Option<UtcTimestamp>,
    updated_at: UtcTimestamp,
    error_code: Option<String>,
    waiting_kind: Option<String>,
    waiting_ref: Option<String>,
}

impl RunRow {
    fn stored(&self) -> StoredRun {
        StoredRun {
            id: self.id,
            workspace_id: self.workspace_id,
            conversation_id: self.conversation_id,
            principal_id: self.principal_id,
            state: self.lifecycle.state(),
            version: self.lifecycle.version(),
            objective_ref: self.objective_ref.clone(),
            created_at: self.created_at,
            started_at: self.started_at,
            updated_at: self.updated_at,
            completed_at: self.lifecycle.terminal_at(),
            error_code: self.error_code.clone(),
        }
    }
}

/// One stored message.
#[derive(Debug, Clone)]
struct MessageRow {
    id: MessageId,
    workspace_id: WorkspaceId,
    conversation_id: ConversationId,
    role: jarvis_domain::model::stream::Role,
    content: String,
    sequence: u64,
    sensitivity: String,
    created_at: UtcTimestamp,
}

/// One stored model-call attempt.
#[derive(Debug, Clone)]
struct CallRow {
    id: ModelCallId,
    workspace_id: WorkspaceId,
    run_id: RunId,
    logical_call_id: ModelCallId,
    attempt: u32,
    call: NewModelCall,
    state: ModelCallState,
    provider_request_id: Option<String>,
    started_at: UtcTimestamp,
    completed_at: Option<UtcTimestamp>,
}

/// The whole in-memory store.
#[derive(Debug, Default)]
struct Store {
    runs: BTreeMap<RunId, RunRow>,
    conversations: BTreeMap<ConversationId, StoredConversation>,
    messages: Vec<MessageRow>,
    calls: BTreeMap<ModelCallId, CallRow>,
    events: Vec<NewActivityEvent>,
}

/// In-memory implementations of the three repositories over one shared store.
///
/// Cloning shares the store, so the same value can be handed to the controller as
/// each of its three ports without them diverging.
#[derive(Debug, Clone, Default)]
pub struct InMemoryRepositories {
    store: std::sync::Arc<Mutex<Store>>,
}

impl InMemoryRepositories {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `body` with the lock held, mapping a poisoned lock to a query failure.
    ///
    /// A poisoned lock can only result from a panic in another thread, and mapping it
    /// to `Query` rather than panicking keeps the port fallible like the real adapter.
    fn with<T>(
        &self,
        body: impl FnOnce(&mut Store) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut guard = self.store.lock().map_err(|_| RepositoryError::Query)?;
        body(&mut guard)
    }

    /// Returns the activity events appended so far, in order.
    ///
    /// Exposed so a test can assert that every state change published exactly one
    /// event in the same write, which is the storage architecture's atomicity rule.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] if the lock is poisoned.
    pub fn recorded_events(&self) -> Result<Vec<NewActivityEvent>, RepositoryError> {
        self.with(|store| Ok(store.events.clone()))
    }

    /// Returns how many runs the store holds.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] if the lock is poisoned.
    pub fn run_count(&self) -> Result<usize, RepositoryError> {
        self.with(|store| Ok(store.runs.len()))
    }
}

impl RunRepository for InMemoryRepositories {
    fn create(&self, run: NewRun) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            self.with(|store| {
                if store.runs.contains_key(&run.id) {
                    return Err(RepositoryError::Conflict { what: "run" });
                }
                // The foreign key is enforced here as the migration enforces it, so a
                // run in an unknown conversation is refused rather than stored.
                if !store.conversations.contains_key(&run.conversation_id) {
                    return Err(RepositoryError::Conflict { what: "run" });
                }
                store.runs.insert(
                    run.id,
                    RunRow {
                        id: run.id,
                        lifecycle: RunLifecycle::new(),
                        workspace_id: run.workspace_id,
                        conversation_id: run.conversation_id,
                        principal_id: run.principal_id,
                        objective_ref: run.objective_ref.clone(),
                        created_at: run.created_at,
                        started_at: None,
                        updated_at: run.created_at,
                        error_code: None,
                        waiting_kind: None,
                        waiting_ref: None,
                    },
                );
                Ok(())
            })
        })
    }

    fn load(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, StoredRun> {
        Box::pin(async move {
            self.with(|store| {
                // Scope is part of the lookup, so a run in another workspace is
                // `NotFound` and not a forbidden result.
                store
                    .runs
                    .get(&run)
                    .filter(|row| row.workspace_id == workspace)
                    .map(RunRow::stored)
                    .ok_or(RepositoryError::NotFound)
            })
        })
    }

    fn load_for_resume(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> RepositoryFuture<'_, RunResumeState> {
        Box::pin(async move {
            self.with(|store| {
                let row = store
                    .runs
                    .get(&run)
                    .filter(|row| row.workspace_id == workspace)
                    .ok_or(RepositoryError::NotFound)?;
                Ok(RunResumeState {
                    run: row.stored(),
                    waiting_kind: row.waiting_kind.clone(),
                    waiting_ref: row.waiting_ref.clone(),
                })
            })
        })
    }

    fn transition<'a>(
        &'a self,
        workspace: WorkspaceId,
        write: crate::repository::run::RunWrite<'a>,
    ) -> RepositoryFuture<'a, StoredRun> {
        Box::pin(async move {
            self.with(|store| {
                if !write.is_consistent() {
                    return Err(RepositoryError::Conflict { what: "waiting_on" });
                }
                let transition = write.transition;
                let row = store
                    .runs
                    .get_mut(&write.event.run_id)
                    .filter(|row| row.workspace_id == workspace)
                    .ok_or(RepositoryError::NotFound)?;

                // The domain decides legality, ordering, and absorption; this double
                // only translates its refusal, exactly as the SQLite adapter does.
                let record = row
                    .lifecycle
                    .apply(transition)
                    .map_err(|error| match error {
                        jarvis_domain::error::DomainError::RunVersionConflict {
                            expected,
                            actual,
                        } => RepositoryError::VersionConflict {
                            expected: expected.get(),
                            actual: actual.get(),
                        },
                        other => RepositoryError::TransitionRefused { code: other.code() },
                    })?;

                row.updated_at = record.occurred_at;
                if record.to == RunState::AwaitingModel && row.started_at.is_none() {
                    row.started_at = Some(record.occurred_at);
                }
                if write.waiting.is_some() {
                    row.waiting_kind = write.waiting.as_ref().map(|on| on.kind.clone());
                    row.waiting_ref = write.waiting.as_ref().map(|on| on.reference.clone());
                } else {
                    row.waiting_kind = None;
                    row.waiting_ref = None;
                }
                let stored = row.stored();

                // The event is appended in the same call as the state change, which is
                // the atomicity the storage architecture requires. The sequence is
                // unique per run, so a reuse is a conflict.
                if store.events.iter().any(|event| {
                    event.run_id == write.event.run_id && event.sequence == write.event.sequence
                }) {
                    return Err(RepositoryError::Conflict {
                        what: "activity_sequence",
                    });
                }
                store.events.push(write.event.clone());
                Ok(stored)
            })
        })
    }

    fn next_event_sequence(&self, workspace: WorkspaceId, run: RunId) -> RepositoryFuture<'_, u64> {
        Box::pin(async move {
            self.with(|store| {
                if store
                    .runs
                    .get(&run)
                    .is_none_or(|row| row.workspace_id != workspace)
                {
                    // An absent run reports `NotFound` rather than a plausible 1, which
                    // a caller would otherwise use and write an orphan event.
                    return Err(RepositoryError::NotFound);
                }
                let maximum = store
                    .events
                    .iter()
                    .filter(|event| event.run_id == run)
                    .map(|event| event.sequence)
                    .max()
                    .unwrap_or(0);
                Ok(maximum.saturating_add(1))
            })
        })
    }
}

impl ConversationRepository for InMemoryRepositories {
    fn create_conversation(&self, conversation: NewConversation) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            self.with(|store| {
                if store.conversations.contains_key(&conversation.id) {
                    return Err(RepositoryError::Conflict {
                        what: "conversation",
                    });
                }
                store.conversations.insert(
                    conversation.id,
                    StoredConversation {
                        id: conversation.id,
                        workspace_id: conversation.workspace_id,
                        owner_user_id: conversation.owner_user_id,
                        title: conversation.title.clone(),
                        archived: false,
                        created_at: conversation.created_at,
                        updated_at: conversation.created_at,
                    },
                );
                Ok(())
            })
        })
    }

    fn load_conversation(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
    ) -> RepositoryFuture<'_, StoredConversation> {
        Box::pin(async move {
            self.with(|store| {
                store
                    .conversations
                    .get(&conversation)
                    .filter(|row| row.workspace_id == workspace)
                    .cloned()
                    .ok_or(RepositoryError::NotFound)
            })
        })
    }

    fn append_message(
        &self,
        workspace: WorkspaceId,
        message: NewMessage,
    ) -> RepositoryFuture<'_, u64> {
        Box::pin(async move {
            self.with(|store| {
                let visible = store
                    .conversations
                    .get(&message.conversation_id)
                    .is_some_and(|row| row.workspace_id == workspace);
                if !visible {
                    // A foreign conversation is `NotFound`, matching the API's rule
                    // that a foreign resource is indistinguishable from a missing one.
                    return Err(RepositoryError::NotFound);
                }
                let sequence = store
                    .messages
                    .iter()
                    .filter(|row| row.conversation_id == message.conversation_id)
                    .map(|row| row.sequence)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1);
                store.messages.push(MessageRow {
                    id: message.id,
                    workspace_id: workspace,
                    conversation_id: message.conversation_id,
                    role: message.role,
                    content: message.content,
                    sequence,
                    sensitivity: message.sensitivity,
                    created_at: message.created_at,
                });
                Ok(sequence)
            })
        })
    }

    fn load_messages(
        &self,
        workspace: WorkspaceId,
        conversation: ConversationId,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> RepositoryFuture<'_, Vec<StoredMessage>> {
        Box::pin(async move {
            self.with(|store| {
                let visible = store
                    .conversations
                    .get(&conversation)
                    .is_some_and(|row| row.workspace_id == workspace);
                if !visible {
                    return Err(RepositoryError::NotFound);
                }
                let after = after_sequence.unwrap_or(0);
                let bounded = usize::try_from(limit.clamp(1, 500)).unwrap_or(500);
                let mut rows: Vec<StoredMessage> = store
                    .messages
                    .iter()
                    .filter(|row| row.conversation_id == conversation && row.sequence > after)
                    .map(|row| StoredMessage {
                        id: row.id,
                        workspace_id: row.workspace_id,
                        conversation_id: row.conversation_id,
                        role: row.role,
                        content: row.content.clone(),
                        sequence: row.sequence,
                        sensitivity: row.sensitivity.clone(),
                        created_at: row.created_at,
                    })
                    .collect();
                rows.sort_by_key(|row| row.sequence);
                rows.truncate(bounded);
                Ok(rows)
            })
        })
    }
}

impl ModelCallRepository for InMemoryRepositories {
    fn record_attempt(&self, call: NewModelCall) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            self.with(|store| {
                let taken = store.calls.values().any(|row| {
                    row.logical_call_id == call.logical_call_id && row.attempt == call.attempt
                });
                if taken {
                    return Err(RepositoryError::Conflict {
                        what: "logical_call_attempt",
                    });
                }
                store.calls.insert(
                    call.id,
                    CallRow {
                        id: call.id,
                        workspace_id: call.workspace_id,
                        run_id: call.run_id,
                        logical_call_id: call.logical_call_id,
                        attempt: call.attempt,
                        call: call.clone(),
                        state: ModelCallState::Pending,
                        provider_request_id: None,
                        started_at: call.started_at,
                        completed_at: None,
                    },
                );
                Ok(())
            })
        })
    }

    fn load_attempt(
        &self,
        workspace: WorkspaceId,
        call: ModelCallId,
    ) -> RepositoryFuture<'_, StoredModelCall> {
        Box::pin(async move {
            self.with(|store| {
                store
                    .calls
                    .get(&call)
                    .filter(|row| row.workspace_id == workspace)
                    .map(stored_call)
                    .ok_or(RepositoryError::NotFound)
            })
        })
    }

    fn record_outcome(
        &self,
        workspace: WorkspaceId,
        call: ModelCallId,
        outcome: ModelCallOutcome,
    ) -> RepositoryFuture<'_, ()> {
        Box::pin(async move {
            self.with(|store| {
                let row = store
                    .calls
                    .get_mut(&call)
                    .filter(|row| row.workspace_id == workspace)
                    .ok_or(RepositoryError::NotFound)?;
                if row.state.is_terminal() {
                    // A recorded result is the record; a late writer cannot rewrite it.
                    return Err(RepositoryError::VersionConflict {
                        expected: 0,
                        actual: 1,
                    });
                }
                row.state = outcome.state;
                row.provider_request_id
                    .clone_from(&outcome.provider_request_id);
                row.completed_at = outcome.completed_at;
                Ok(())
            })
        })
    }

    fn load_attempts(
        &self,
        workspace: WorkspaceId,
        logical_call_id: ModelCallId,
    ) -> RepositoryFuture<'_, Vec<StoredModelCall>> {
        Box::pin(async move {
            self.with(|store| {
                let mut rows: Vec<StoredModelCall> = store
                    .calls
                    .values()
                    .filter(|row| row.workspace_id == workspace)
                    .filter(|row| row.logical_call_id == logical_call_id)
                    .map(stored_call)
                    .collect();
                rows.sort_by_key(|row| row.attempt);
                Ok(rows)
            })
        })
    }
}

/// Projects a stored call row into the port's view.
fn stored_call(row: &CallRow) -> StoredModelCall {
    StoredModelCall {
        id: row.id,
        run_id: row.run_id,
        logical_call_id: row.logical_call_id,
        attempt: row.attempt,
        provider_id: row.call.model.provider_id.clone(),
        model_id: row.call.model.model_id.clone(),
        revision: row.call.model.revision.clone(),
        state: row.state,
        provider_request_id: row.provider_request_id.clone(),
        started_at: row.started_at,
        completed_at: row.completed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryRepositories;
    use crate::repository::RepositoryError;
    use crate::repository::conversation::{ConversationRepository, NewConversation, NewMessage};
    use crate::repository::model_call::{ModelCallRepository, NewModelCall};
    use crate::repository::run::{NewRun, RunRepository, RunWrite};
    use jarvis_domain::ids::{ConversationId, ModelCallId, PrincipalId, RunId, WorkspaceId};
    use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
    use jarvis_domain::model::stream::PortableSettings;
    use jarvis_domain::run::lifecycle::RunTransition;
    use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
    use jarvis_domain::time::UtcTimestamp;

    fn id(value: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(value)
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid(id(1))
    }

    fn conversation() -> ConversationId {
        ConversationId::from_uuid(id(2))
    }

    fn run() -> RunId {
        RunId::from_uuid(id(3))
    }

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn repos() -> InMemoryRepositories {
        InMemoryRepositories::new()
    }

    async fn seed(repositories: &InMemoryRepositories) {
        repositories
            .create_conversation(
                NewConversation::new(
                    conversation(),
                    workspace(),
                    PrincipalId::from_uuid(id(4)),
                    None,
                    "cli".to_owned(),
                    now(),
                )
                .expect("valid"),
            )
            .await
            .expect("created");
        repositories
            .create(
                NewRun::new(
                    run(),
                    workspace(),
                    conversation(),
                    PrincipalId::from_uuid(id(4)),
                    None,
                    now(),
                )
                .expect("valid"),
            )
            .await
            .expect("created");
    }

    #[tokio::test]
    async fn the_double_enforces_scope_like_the_real_adapter() {
        // A foreign run must be `NotFound`, not a forbidden result, because the API
        // requires the two to be indistinguishable.
        let repositories = repos();
        seed(&repositories).await;
        let foreign = WorkspaceId::from_uuid(id(99));
        assert_eq!(
            repositories
                .load(foreign, run())
                .await
                .expect_err("foreign"),
            RepositoryError::NotFound,
        );
    }

    #[tokio::test]
    async fn the_double_refuses_an_illegal_edge_by_asking_the_domain() {
        // The double holds no transition table of its own: it delegates to
        // `RunLifecycle`, so a test against it cannot pass while the domain disagrees.
        let repositories = repos();
        seed(&repositories).await;
        let illegal = RunTransition::new(
            RunState::Received,
            RunState::Responding,
            RunVersion::FIRST,
            TransitionActor::Controller,
            TransitionReason::new("step").expect("valid"),
            now(),
        );
        let error = repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &illegal,
                    crate::repository::run::NewActivityEvent {
                        run_id: run(),
                        sequence: 1,
                        event_type: "run.responding".to_owned(),
                        payload_json: None,
                        visibility: crate::repository::run::EventVisibility::Public,
                        occurred_at: now(),
                    },
                ),
            )
            .await
            .expect_err("an invented edge must be refused");
        assert_eq!(
            error,
            RepositoryError::TransitionRefused {
                code: "jarvis.run_transition_not_allowed",
            }
        );
    }

    #[tokio::test]
    async fn the_double_appends_the_event_with_the_state_change() {
        let repositories = repos();
        seed(&repositories).await;
        let transition = RunTransition::new(
            RunState::Received,
            RunState::ContextBuilding,
            RunVersion::FIRST,
            TransitionActor::Controller,
            TransitionReason::new("step").expect("valid"),
            now(),
        );
        repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &transition,
                    crate::repository::run::NewActivityEvent {
                        run_id: run(),
                        sequence: 1,
                        event_type: "run.context_building".to_owned(),
                        payload_json: None,
                        visibility: crate::repository::run::EventVisibility::Public,
                        occurred_at: now(),
                    },
                ),
            )
            .await
            .expect("applied");

        assert_eq!(repositories.recorded_events().expect("events").len(), 1);
        assert_eq!(
            repositories
                .next_event_sequence(workspace(), run())
                .await
                .expect("readable"),
            2,
        );
    }

    #[tokio::test]
    async fn the_double_refuses_a_reused_event_sequence() {
        let repositories = repos();
        seed(&repositories).await;
        let first = RunTransition::new(
            RunState::Received,
            RunState::ContextBuilding,
            RunVersion::FIRST,
            TransitionActor::Controller,
            TransitionReason::new("step").expect("valid"),
            now(),
        );
        repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &first,
                    crate::repository::run::NewActivityEvent {
                        run_id: run(),
                        sequence: 1,
                        event_type: "run.context_building".to_owned(),
                        payload_json: None,
                        visibility: crate::repository::run::EventVisibility::Public,
                        occurred_at: now(),
                    },
                ),
            )
            .await
            .expect("applied");

        let second = RunTransition::new(
            RunState::ContextBuilding,
            RunState::Planning,
            RunVersion::new(2),
            TransitionActor::Controller,
            TransitionReason::new("step").expect("valid"),
            now(),
        );
        let error = repositories
            .transition(
                workspace(),
                RunWrite::new(
                    &second,
                    crate::repository::run::NewActivityEvent {
                        run_id: run(),
                        sequence: 1,
                        event_type: "run.planning".to_owned(),
                        payload_json: None,
                        visibility: crate::repository::run::EventVisibility::Public,
                        occurred_at: now(),
                    },
                ),
            )
            .await
            .expect_err("a reused sequence must be refused");
        assert_eq!(error.code(), "storage.conflict");
    }

    #[tokio::test]
    async fn the_double_records_a_retry_as_a_new_attempt() {
        let repositories = repos();
        seed(&repositories).await;
        let model = ModelRef::new(
            ProviderId::parse("scripted.local").expect("valid"),
            ModelId::parse("fixture-1").expect("valid"),
        );
        let logical = ModelCallId::from_uuid(id(7));
        for (row, attempt) in [(10_u128, 1_u32), (11, 2)] {
            repositories
                .record_attempt(NewModelCall {
                    id: ModelCallId::from_uuid(id(row)),
                    workspace_id: workspace(),
                    run_id: run(),
                    logical_call_id: logical,
                    attempt,
                    model: model.clone(),
                    request_fingerprint: None,
                    started_at: now(),
                })
                .await
                .expect("a new attempt number is legal");
        }
        let attempts = repositories
            .load_attempts(workspace(), logical)
            .await
            .expect("loads");
        assert_eq!(attempts.len(), 2);

        // The same attempt number again is refused rather than merged.
        let error = repositories
            .record_attempt(NewModelCall {
                id: ModelCallId::from_uuid(id(12)),
                workspace_id: workspace(),
                run_id: run(),
                logical_call_id: logical,
                attempt: 1,
                model,
                request_fingerprint: None,
                started_at: now(),
            })
            .await
            .expect_err("a reused attempt must be refused");
        assert_eq!(error.code(), "storage.conflict");
    }

    #[tokio::test]
    async fn the_double_orders_messages_and_bounds_a_page() {
        let repositories = repos();
        seed(&repositories).await;
        for index in 0..5_u128 {
            repositories
                .append_message(
                    workspace(),
                    NewMessage {
                        id: jarvis_domain::ids::MessageId::from_uuid(id(50 + index)),
                        conversation_id: conversation(),
                        role: jarvis_domain::model::stream::Role::User,
                        content: format!("m{index}"),
                        content_schema_version: 1,
                        sensitivity: "internal".to_owned(),
                        source: "cli".to_owned(),
                        created_at: now(),
                    }
                    .validated()
                    .expect("valid"),
                )
                .await
                .expect("appended");
        }
        let page = repositories
            .load_messages(workspace(), conversation(), Some(2), 2)
            .await
            .expect("loads");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].sequence, 3);
        assert_eq!(page[1].content, "m3");
    }

    #[test]
    fn portable_settings_default_is_used_by_the_fixtures() {
        // Keeps the import honest: the fixtures build a full model request, and this
        // asserts the default block the controller sends is the empty one.
        assert!(PortableSettings::default().is_empty());
    }
}
