//! The model-call repository port.
//!
//! `UNIQUE(logical_call_id, attempt)` in the schema is the important constraint:
//! it makes a retry an *attempt* of one logical call rather than a new call, which
//! is the distinction the architecture's retry-ownership rule depends on. The port
//! therefore models a call as an identity plus a numbered attempt rather than as a
//! flat row, and a duplicate attempt is reported as a conflict rather than
//! silently overwriting the recorded outcome of the first one.

use crate::repository::{RepositoryError, RepositoryFuture};
use jarvis_domain::ids::{ModelCallId, RunId, WorkspaceId};
use jarvis_domain::model::identity::{ModelId, ModelRef, ModelRevision, ProviderId};
use jarvis_domain::model::stream::{FinishReason, Usage};
use jarvis_domain::time::UtcTimestamp;

/// The largest accepted provider request or continuation reference.
pub const MAX_PROVIDER_REF_BYTES: usize = 256;

/// How a model call ended.
///
/// A closed set rather than free text, because this is what a retry decision and an
/// operator both read, and an uninterpretable stored value would make "did this
/// call fail or did it refuse" unanswerable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCallState {
    /// The call has been reserved but not started.
    Pending,
    /// The provider has been asked.
    Started,
    /// The call produced output.
    Streaming,
    /// The call completed.
    Completed,
    /// The call failed with a normalized error.
    Failed,
    /// The call was cancelled.
    Cancelled,
}

impl ModelCallState {
    /// Returns the stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Started => "started",
            Self::Streaming => "streaming",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parses the stored spelling.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Corrupted`] for an unrecognized value, because a
    /// call whose outcome cannot be interpreted must not be read as pending or
    /// treated as absent — either would make a failed call look retryable or lost.
    pub fn parse(value: &str) -> Result<Self, RepositoryError> {
        match value {
            "pending" => Ok(Self::Pending),
            "started" => Ok(Self::Started),
            "streaming" => Ok(Self::Streaming),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(RepositoryError::Corrupted { column: "state" }),
        }
    }

    /// Returns whether the call reached an outcome.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// The fields needed to record one model-call attempt.
///
/// Built as a struct literal and then [`validated`](Self::validated), for the same
/// reason as `NewMessage`: every field is independently meaningful and naming them
/// beats a long positional list where two same-typed arguments can be transposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewModelCall {
    /// The attempt's own row identifier.
    pub id: ModelCallId,
    /// The workspace that owns it. Scope is a parameter, never inferred.
    pub workspace_id: WorkspaceId,
    /// The run it belongs to.
    pub run_id: RunId,
    /// The stable identity of the logical call, shared by every attempt.
    pub logical_call_id: ModelCallId,
    /// Which attempt this is, starting at 1.
    pub attempt: u32,
    /// The provider and model that served it.
    pub model: ModelRef,
    /// The requested output schema fingerprint, when structured output was used.
    pub request_fingerprint: Option<String>,
    /// The instant the attempt started.
    pub started_at: UtcTimestamp,
}

impl NewModelCall {
    /// Validates the attempt record.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when `attempt` is zero or the
    /// fingerprint exceeds its bound. Attempt zero is refused rather than treated as
    /// the first attempt, because `1` is the first attempt in the schema and two
    /// spellings of "first" would collide on the uniqueness constraint.
    pub fn validated(self) -> Result<Self, RepositoryError> {
        if self.attempt == 0 {
            return Err(RepositoryError::Conflict { what: "attempt" });
        }
        if self
            .request_fingerprint
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_PROVIDER_REF_BYTES)
        {
            return Err(RepositoryError::Conflict {
                what: "request_fingerprint",
            });
        }
        Ok(self)
    }
}

/// The recorded outcome of a model-call attempt.
///
/// Built as a struct literal and then [`validated`](Self::validated): with nine
/// independently meaningful fields, a positional constructor is where two
/// same-typed `Option<String>` references get transposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallOutcome {
    /// How the attempt ended.
    pub state: ModelCallState,
    /// The provider's request identifier, when it reported one.
    pub provider_request_id: Option<String>,
    /// A continuation reference for resume, when the provider issued one.
    pub continuation_ref: Option<String>,
    /// The provider-reported or estimated usage.
    pub usage: Option<Usage>,
    /// The estimated cost in millionths of the billing currency unit.
    pub estimated_cost_microunits: Option<u64>,
    /// Why the call stopped, when it reached a finish.
    pub finish_reason: Option<FinishReason>,
    /// A stable, namespaced error code when it failed.
    pub error_code: Option<String>,
    /// When the first output arrived, once it has.
    pub first_output_at: Option<UtcTimestamp>,
    /// When the attempt reached its outcome.
    pub completed_at: Option<UtcTimestamp>,
}

impl ModelCallOutcome {
    /// Validates the outcome.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when a provider-supplied reference
    /// exceeds its bound, and when `state` is terminal but no completion instant is
    /// present — a finished attempt with no instant would be indistinguishable from
    /// a live one when read back.
    pub fn validated(self) -> Result<Self, RepositoryError> {
        for reference in [
            self.provider_request_id.as_ref(),
            self.continuation_ref.as_ref(),
        ] {
            if reference
                .is_some_and(|value| value.is_empty() || value.len() > MAX_PROVIDER_REF_BYTES)
            {
                return Err(RepositoryError::Conflict {
                    what: "provider_reference",
                });
            }
        }
        if self.state.is_terminal() && self.completed_at.is_none() {
            return Err(RepositoryError::Conflict {
                what: "completed_at",
            });
        }
        Ok(self)
    }
}

/// A loaded model-call attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredModelCall {
    /// The attempt's row identifier.
    pub id: ModelCallId,
    /// The run it belongs to.
    pub run_id: RunId,
    /// The stable logical-call identity.
    pub logical_call_id: ModelCallId,
    /// Which attempt this is.
    pub attempt: u32,
    /// The provider that served it.
    pub provider_id: ProviderId,
    /// The model that served it.
    pub model_id: ModelId,
    /// The model revision, when pinned.
    pub revision: Option<ModelRevision>,
    /// How the attempt ended.
    pub state: ModelCallState,
    /// The provider's request identifier, when recorded.
    pub provider_request_id: Option<String>,
    /// The instant the attempt started.
    pub started_at: UtcTimestamp,
    /// When it reached its outcome.
    pub completed_at: Option<UtcTimestamp>,
}

/// The durable model-call store.
pub trait ModelCallRepository: Send + Sync {
    /// Records a new attempt.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Conflict`] when `(logical_call_id, attempt)`
    /// already exists — a retry must be a *new attempt number*, not a rewrite of the
    /// recorded outcome of the first.
    fn record_attempt(&self, call: NewModelCall) -> RepositoryFuture<'_, ()>;

    /// Loads an attempt within `workspace`.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] when it is absent or foreign.
    fn load_attempt(
        &self,
        workspace: WorkspaceId,
        call: ModelCallId,
    ) -> RepositoryFuture<'_, StoredModelCall>;

    /// Records the outcome of an existing attempt.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::NotFound`] for an absent or foreign attempt and
    /// [`RepositoryError::VersionConflict`] when the attempt already reached a
    /// terminal outcome, so a late writer cannot rewrite a recorded result.
    fn record_outcome(
        &self,
        workspace: WorkspaceId,
        call: ModelCallId,
        outcome: ModelCallOutcome,
    ) -> RepositoryFuture<'_, ()>;

    /// Returns every attempt of `logical_call_id`, in attempt order.
    ///
    /// This is what makes the retry chain auditable without storing prompt content:
    /// the sequence of attempts and their outcomes is the record.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::Query`] for a driver failure.
    fn load_attempts(
        &self,
        workspace: WorkspaceId,
        logical_call_id: ModelCallId,
    ) -> RepositoryFuture<'_, Vec<StoredModelCall>>;
}

#[cfg(test)]
mod tests {
    use super::{MAX_PROVIDER_REF_BYTES, ModelCallOutcome, ModelCallState, NewModelCall};
    use crate::repository::RepositoryError;
    use jarvis_domain::ids::{ModelCallId, RunId, WorkspaceId};
    use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
    use jarvis_domain::time::UtcTimestamp;

    fn id(value: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(value)
    }

    fn now() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
    }

    fn model() -> ModelRef {
        ModelRef::new(
            ProviderId::parse("scripted.local").expect("valid"),
            ModelId::parse("fixture-1").expect("valid"),
        )
    }

    fn attempt(attempt: u32) -> Result<NewModelCall, RepositoryError> {
        NewModelCall {
            id: ModelCallId::from_uuid(id(1)),
            workspace_id: WorkspaceId::from_uuid(id(2)),
            run_id: RunId::from_uuid(id(3)),
            logical_call_id: ModelCallId::from_uuid(id(4)),
            attempt,
            model: model(),
            request_fingerprint: None,
            started_at: now(),
        }
        .validated()
    }

    #[test]
    fn every_state_round_trips_and_an_unknown_value_is_corruption() {
        for state in [
            ModelCallState::Pending,
            ModelCallState::Started,
            ModelCallState::Streaming,
            ModelCallState::Completed,
            ModelCallState::Failed,
            ModelCallState::Cancelled,
        ] {
            assert_eq!(
                ModelCallState::parse(state.as_str()).expect("round-trips"),
                state,
            );
        }
        // A call whose outcome cannot be interpreted must not be read as pending or
        // as absent: either would make a failed call look retryable or lost.
        let error = ModelCallState::parse("weird").expect_err("unknown must be refused");
        assert_eq!(error.code(), "storage.row_corrupted");
    }

    #[test]
    fn terminality_is_decided_in_one_place() {
        assert!(!ModelCallState::Pending.is_terminal());
        assert!(!ModelCallState::Started.is_terminal());
        assert!(!ModelCallState::Streaming.is_terminal());
        for terminal in [
            ModelCallState::Completed,
            ModelCallState::Failed,
            ModelCallState::Cancelled,
        ] {
            assert!(terminal.is_terminal(), "{terminal:?} must be terminal");
        }
    }

    #[test]
    fn an_attempt_number_starts_at_one() {
        assert!(attempt(1).is_ok());
        assert!(attempt(2).is_ok());
        // Zero is refused rather than treated as the first attempt: the schema's
        // first attempt is 1, and two spellings of "first" would collide on the
        // uniqueness constraint.
        let error = attempt(0).expect_err("attempt zero must be refused");
        assert_eq!(error.code(), "storage.conflict");
    }

    #[test]
    fn a_terminal_outcome_requires_a_completion_instant() {
        // A finished attempt with no instant would read as live, so the pair is
        // enforced here rather than hoped for.
        let error = baseline_outcome(ModelCallState::Completed, None)
            .validated()
            .expect_err("a terminal outcome without an instant must be refused");
        assert_eq!(error.code(), "storage.conflict");

        assert!(
            baseline_outcome(ModelCallState::Completed, Some(now()))
                .validated()
                .is_ok(),
        );
        // A non-terminal outcome needs no instant, so a live attempt is recordable.
        assert!(
            baseline_outcome(ModelCallState::Streaming, None)
                .validated()
                .is_ok(),
        );
    }

    #[test]
    fn a_provider_reference_is_bounded() {
        let over = "a".repeat(MAX_PROVIDER_REF_BYTES + 1);
        let mut outcome = baseline_outcome(ModelCallState::Failed, Some(now()));
        outcome.provider_request_id = Some(over);
        let error = outcome
            .validated()
            .expect_err("an over-long reference must be refused");
        assert_eq!(error.code(), "storage.conflict");
    }

    /// Builds an outcome with only the given state and completion instant set.
    fn baseline_outcome(
        state: ModelCallState,
        completed_at: Option<UtcTimestamp>,
    ) -> ModelCallOutcome {
        ModelCallOutcome {
            state,
            provider_request_id: None,
            continuation_ref: None,
            usage: None,
            estimated_cost_microunits: None,
            finish_reason: None,
            error_code: None,
            first_output_at: None,
            completed_at,
        }
    }
}
