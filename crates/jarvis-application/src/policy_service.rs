//! Evaluating the model data policy for a route: the read side of `BRN-010`.
//!
//! `jarvis_domain::model::routing` is the selector and
//! [`crate::repository::policy`] is the store; this module is what joins them to
//! something a client can ask for. It is the layer the contract's
//! `GET /api/v1/model-data-policy/effective` maps onto, and its whole job is assembling a
//! [`RouteRequest`] from **stored** state so the selector has a caller that is not a test.
//!
//! Three decisions are deliberate:
//!
//! - **The candidate inventory is supplied by the caller, not read from a table.** A
//!   capability descriptor's evidence is measured per provider and model, and where that
//!   inventory lives is the provider layer's concern (`BRN-003`, `BRN-011`). This service
//!   therefore takes candidates as an argument and never fabricates one: inventing a
//!   descriptor would let a route be selected on evidence nobody recorded, which is the
//!   promotion `model-data-policy.md` forbids.
//! - **The sensitivity is the content's, and it is a required input.** A caller that does not
//!   know what it is about to send must not have the evaluation assume it is public, because
//!   that is the direction of error which lets confidential content through.
//! - **The rules come from the stored version, never from the caller.** An evaluation with
//!   caller-supplied rules would let a client relax its own workspace policy, which is the
//!   one thing the precedence order exists to prevent.
//!
//! The service returns **domain** types rather than wire ones, because
//! `jarvis-application` must not depend on `jarvis-protocol`. Rendering a decision into the
//! contract's JSON shape is the HTTP layer's job, and keeping the two apart is what stops a
//! wire field from being invented here where no contract test could see it.

use std::sync::Arc;

use jarvis_domain::model::policy::{ModelRouteDecision, PolicyVersionRef, Sensitivity};
use jarvis_domain::model::routing::{
    RouteCandidate, RouteRequest, RouteSelectionFailure, select_route_explained,
};
use jarvis_domain::model::stream::RouteRequirements;
use jarvis_domain::time::{IsoDate, UtcTimestamp};

use crate::repository::RepositoryError;
use crate::repository::policy::{ModelDataPolicyRepository, StoredPolicyVersion};
use crate::request_context::RequestContext;

/// Why a policy evaluation could not be answered.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyServiceError {
    /// No policy is in force for the caller's workspace.
    ///
    /// Reported rather than answered with permissive rules: a call made under no policy
    /// applies nothing, and a caller told "everything is permitted" would proceed on a
    /// decision nobody took. It shares the contract's `model.policy_not_found` code with an
    /// absent named version, because the client's remedy is the same — there is no policy to
    /// evaluate against — and a second code would be a distinction only the server can see.
    NoActivePolicy,
    /// The named policy version does not exist in the caller's scope.
    PolicyNotFound,
    /// Every candidate was evaluated and none satisfied the policy.
    ///
    /// Carries the refusal, including every rejected candidate and its reason, because "no
    /// route" alone leaves a caller unable to act: the next step differs depending on whether
    /// the local model was refused for locality or because nobody offered it.
    Unsatisfied(Box<jarvis_domain::model::routing::RouteRefusal>),
    /// More candidates were offered than the bound allows, so none was evaluated.
    ///
    /// Distinct from [`Unsatisfied`](Self::Unsatisfied) because the candidate set was never
    /// examined: reporting "your policy refused every model" would send the caller to edit a
    /// policy that is not the problem.
    CandidatesUnbounded {
        /// How many were offered, so an operator can see the scale.
        offered: usize,
    },
    /// The store could not be read.
    Storage(RepositoryError),
}

impl PolicyServiceError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoActivePolicy | Self::PolicyNotFound => "model.policy_not_found",
            Self::Unsatisfied(_) => "model.policy_unsatisfied",
            Self::CandidatesUnbounded { .. } => "jarvis.context_candidates_unbounded",
            Self::Storage(error) => error.code(),
        }
    }

    /// Returns whether retrying the same request unchanged could succeed.
    ///
    /// Only a storage fault can: the same policy and the same candidates will reach the same
    /// verdict, so reporting a refusal as retryable would spend a caller's budget on a certain
    /// failure.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::NoActivePolicy
            | Self::PolicyNotFound
            | Self::Unsatisfied(_)
            | Self::CandidatesUnbounded { .. } => false,
            Self::Storage(error) => error.retryable(),
        }
    }
}

impl From<RepositoryError> for PolicyServiceError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound => Self::PolicyNotFound,
            other => Self::Storage(other),
        }
    }
}

/// What one evaluation needs besides the policy the store holds.
#[derive(Debug, Clone)]
pub struct EvaluationRequest<'a> {
    /// The candidates to consider, supplied by the provider layer.
    pub candidates: &'a [RouteCandidate],
    /// The sensitivity of the content about to be sent.
    pub sensitivity: Sensitivity,
    /// The hard capabilities the call requires.
    pub requirements: RouteRequirements,
    /// The day to evaluate evidence freshness against.
    pub today: IsoDate,
    /// The instant the evaluation runs.
    pub decided_at: UtcTimestamp,
}

/// Reads policies and evaluates routes against them.
pub struct PolicyService {
    policies: Arc<dyn ModelDataPolicyRepository>,
}

impl std::fmt::Debug for PolicyService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The port is not printed: an adapter can hold a connection string.
        formatter
            .debug_struct("PolicyService")
            .finish_non_exhaustive()
    }
}

impl PolicyService {
    /// Builds the service over `policies`.
    #[must_use]
    pub fn new(policies: Arc<dyn ModelDataPolicyRepository>) -> Self {
        Self { policies }
    }

    /// Reads the workspace's active policy.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyServiceError::NoActivePolicy`] when none is in force, mapped from the
    /// store's `NotFound` deliberately: "the workspace has no policy" is a different fact from
    /// "the version you named is missing", and only the caller knows which it asked for.
    pub async fn active(
        &self,
        context: &RequestContext,
    ) -> Result<StoredPolicyVersion, PolicyServiceError> {
        match self.policies.load_active(context.workspace_id).await {
            Ok(stored) => Ok(stored),
            Err(RepositoryError::NotFound) => Err(PolicyServiceError::NoActivePolicy),
            Err(other) => Err(PolicyServiceError::Storage(other)),
        }
    }

    /// Reads one policy version in the caller's scope.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyServiceError::PolicyNotFound`] for an absent or foreign version.
    pub async fn version(
        &self,
        context: &RequestContext,
        reference: PolicyVersionRef,
    ) -> Result<StoredPolicyVersion, PolicyServiceError> {
        self.policies
            .load_version(context.workspace_id, reference)
            .await
            .map_err(PolicyServiceError::from)
    }

    /// Evaluates `request` against the named policy version.
    ///
    /// The evaluation never relaxes a rule: the selector returns the first compliant candidate
    /// or a refusal carrying every rejected candidate, which is the contract's "routing
    /// rejection rather than silent relaxation".
    ///
    /// # Errors
    ///
    /// Returns [`PolicyServiceError::Unsatisfied`] when no candidate complies,
    /// [`PolicyServiceError::CandidatesUnbounded`] when the set was too large to examine, and
    /// [`PolicyServiceError::PolicyNotFound`] when the version is absent.
    pub async fn evaluate(
        &self,
        context: &RequestContext,
        reference: PolicyVersionRef,
        request: &EvaluationRequest<'_>,
    ) -> Result<ModelRouteDecision, PolicyServiceError> {
        let stored = self.version(context, reference).await?;
        // The reference is read before the rules are moved: `reference()` borrows `stored`,
        // so extracting it afterwards would be a borrow of a partially moved value.
        let policy = stored.reference();

        let route_request = RouteRequest {
            rules: stored.rules,
            // The stored reference, so a later reader can see which rules applied even after
            // the policy is archived or replaced.
            policy,
            sensitivity: request.sensitivity,
            requirements: request.requirements.clone(),
            today: request.today,
            decided_at: request.decided_at,
        };

        match select_route_explained(request.candidates, &route_request) {
            Ok(decision) => Ok(decision),
            Err(RouteSelectionFailure::Refused(refusal)) => {
                Err(PolicyServiceError::Unsatisfied(Box::new(refusal)))
            }
            Err(RouteSelectionFailure::CandidatesUnbounded { offered }) => {
                Err(PolicyServiceError::CandidatesUnbounded { offered })
            }
        }
    }
}
