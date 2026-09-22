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

use jarvis_domain::model::policy::{
    ModelDataPolicyStatus, ModelRouteDecision, PolicyRules, PolicyVersionRef, Sensitivity,
};
use jarvis_domain::model::routing::{
    RouteCandidate, RouteRequest, RouteSelectionFailure, select_route_explained,
};
use jarvis_domain::model::stream::RouteRequirements;
use jarvis_domain::time::{IsoDate, UtcTimestamp};

use crate::repository::RepositoryError;
use crate::repository::policy::{ModelDataPolicyRepository, NewPolicyVersion, StoredPolicyVersion};
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
    /// The caller's `expected_version` did not match what the workspace holds.
    ///
    /// Carries both numbers because a caller cannot recover from a precondition failure without
    /// knowing what it was wrong about. They are safe to report: a policy version count is not
    /// policy content, and the caller already holds read access to the policy it is updating —
    /// the scope is resolved server-side, so this cannot disclose another workspace's state.
    VersionConflict {
        /// The version the caller believed was current.
        expected: u32,
        /// The version the workspace actually holds.
        actual: u32,
    },
    /// The submitted rules contradict an earlier layer, so the merged policy does not exist.
    ///
    /// Reported rather than stored-and-refused-later. The contract requires a contradictory
    /// policy to be reported rather than permitted, and the merge is the only place that can
    /// detect it: two disjoint allow-lists are unsatisfiable, so a stored version would sit there
    /// refusing every call with no hint that its author had made a mistake.
    ///
    /// The code is carried as a string rather than as a [`RepositoryError`], because that type's
    /// own `code()` reports its envelope (`storage.transition_refused`) and would hide the
    /// domain's code. A caller told "a storage transition was refused" would go looking at
    /// storage for what is really a mistake in the rules it submitted.
    Contradictory {
        /// The stable, namespaced domain code (`jarvis.invalid_policy_layer`).
        code: &'static str,
    },
    /// The submitted policy is unusable: an out-of-range value, an unbounded name, or a field
    /// this build does not support.
    Invalid {
        /// The stable, namespaced code for what was wrong.
        code: &'static str,
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
            Self::VersionConflict { .. } => "resource.version_conflict",
            // Both carry a code rather than a variant name, because the caller needs the specific
            // reason: `Invalid` says what was out of range, and a contradiction names the merge's
            // own `jarvis.invalid_policy_layer`. Collapsed to one arm because they are the same
            // decision — report the code the layer that refused produced.
            Self::Invalid { code } | Self::Contradictory { code } => code,
            Self::Storage(error) => error.code(),
        }
    }

    /// Returns whether retrying the same request unchanged could succeed.
    ///
    /// Only a storage fault and a stale precondition can. The same policy and the same
    /// candidates will reach the same verdict, so reporting a refusal as retryable would spend
    /// a caller's budget on a certain failure. A `VersionConflict` is retryable because the
    /// remedy is a re-read and a recomputation, which is precisely what makes a precondition
    /// worth stating; a `Contradictory` is not, because resubmitting the same unsatisfiable
    /// rules reaches the same refusal forever.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::NoActivePolicy
            | Self::PolicyNotFound
            | Self::Unsatisfied(_)
            | Self::CandidatesUnbounded { .. }
            | Self::Contradictory { .. }
            | Self::Invalid { .. } => false,
            Self::VersionConflict { .. } => true,
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

    /// Creates the next immutable version of the workspace policy.
    ///
    /// The version is **assigned here** rather than accepted from the caller, because the
    /// contract makes a change of rules create a new version and version identity is what keeps
    /// a past decision explainable. A caller that named the version could skip numbers, reuse one
    /// for different rules, or collide with an existing version and be told its view was stale
    /// when it never held a view at all. `expected_version` is therefore a precondition, not an
    /// instruction: it says what the caller believes is current, and a mismatch is a conflict.
    ///
    /// The rules are merged against the workspace's current policy **before** the write, so a
    /// submitted layer can only narrow what is already in force. That is the contract's layer 4
    /// beneath layer 2, and the direction matters: a request body cannot widen a workspace
    /// policy, which is the rule that makes an approval-free write safe. A contradiction is
    /// refused here rather than stored, because an unsatisfiable policy would otherwise sit in
    /// force refusing every call with nothing to indicate its author had made a mistake.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyServiceError::VersionConflict`] when `expected_version` does not match,
    /// [`PolicyServiceError::Contradictory`] when the merged rules are unsatisfiable,
    /// [`PolicyServiceError::Invalid`] when the rules are out of range, and
    /// [`PolicyServiceError::Storage`] when the write fails.
    pub async fn put(
        &self,
        context: &RequestContext,
        expected_version: u32,
        name: &str,
        submitted: PolicyRules,
        created_at: UtcTimestamp,
        policy_id: jarvis_domain::ids::ModelDataPolicyId,
    ) -> Result<StoredPolicyVersion, PolicyServiceError> {
        if name.trim().is_empty() {
            return Err(PolicyServiceError::Invalid {
                code: "request.invalid",
            });
        }

        // Read the current policy once, and use that same read for both the precondition and
        // the merge. Two reads would let a concurrent writer land between them, so the version
        // the merge applied and the version the precondition checked could differ — which is
        // exactly the silent-narrowing-against-the-wrong-base failure the merge exists to make
        // impossible.
        let current = match self.policies.load_active(context.workspace_id).await {
            Ok(stored) => Some(stored),
            Err(RepositoryError::NotFound) => None,
            Err(other) => return Err(PolicyServiceError::Storage(other)),
        };

        let current_version = current.as_ref().map_or(0, |stored| stored.version);
        if current_version != expected_version {
            return Err(PolicyServiceError::VersionConflict {
                expected: expected_version,
                actual: current_version,
            });
        }

        // Layer 2 (the workspace policy already in force) merged with layer 4 (this submission).
        // `merge_stricter` only narrows, so the result can never permit more than either input.
        let rules = match current.as_ref() {
            Some(stored) => stored
                .rules
                .merge_stricter(&submitted)
                .map_err(|error| PolicyServiceError::Contradictory { code: error.code() })?,
            None => submitted,
        };

        let next_version = current_version.saturating_add(1);
        let candidate = NewPolicyVersion {
            policy_id,
            version: next_version,
            workspace_id: context.workspace_id,
            name: name.to_owned(),
            // A write puts rules in force; archiving is a separate lifecycle action the contract
            // keeps apart, so a new version is always `active` and the previous one is left
            // alone. `load_active` orders by version descending, which is why the older version
            // does not need archiving for the newer one to take effect.
            status: ModelDataPolicyStatus::Active,
            rules,
            created_at,
        }
        .validated()
        .map_err(|_| PolicyServiceError::Invalid {
            code: "request.semantic_invalid",
        })?;

        self.policies
            .insert_version(candidate)
            .await
            .map_err(|error| match error {
                // A duplicate means another writer created this version between the read above
                // and this write. Reported as a version conflict rather than as a storage
                // fault, because it is the same fact the precondition names: the caller's view
                // was stale. `actual` is the version it expected plus one, which is the version
                // the other writer took — the store cannot see the true current version without
                // a second read, and inventing a different number would be a fabricated fact.
                RepositoryError::VersionConflict { .. } => PolicyServiceError::VersionConflict {
                    expected: expected_version,
                    actual: next_version,
                },
                other => PolicyServiceError::Storage(other),
            })?;

        // Read back rather than returning the candidate, so the response describes what is
        // stored. A returned in-memory value would report a version that a failed write never
        // created, and it is the same reason the store re-validates rules on the way out.
        self.version(
            context,
            PolicyVersionRef {
                policy_id,
                version: next_version,
            },
        )
        .await
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use jarvis_domain::ids::{
        CorrelationId, ModelDataPolicyId, PrincipalId, RequestId, WorkspaceId,
    };
    use jarvis_domain::model::capability::CapabilityDescriptor;
    use jarvis_domain::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId};
    use jarvis_domain::model::policy::{
        FallbackPermission, Locality, ModelDataPolicyStatus, PolicyRules, ProviderRetention,
        Sensitivity, Telemetry, TrainingUse,
    };
    use jarvis_domain::model::routing::RouteCandidate;
    use jarvis_domain::model::stream::{Modality, RouteRequirements};
    use jarvis_domain::time::{IsoDate, UtcTimestamp};

    use super::{EvaluationRequest, PolicyService, PolicyServiceError};
    use crate::request_context::{AuthenticationAssurance, RequestChannel, RequestContext};
    use crate::testing::InMemoryRepositories;

    fn context() -> RequestContext {
        RequestContext::new(
            RequestId::from_uuid(uuid::Uuid::from_u128(1)),
            CorrelationId::from_uuid(uuid::Uuid::from_u128(2)),
            PrincipalId::from_uuid(uuid::Uuid::from_u128(3)),
            AuthenticationAssurance::Standard,
            WorkspaceId::from_uuid(uuid::Uuid::from_u128(4)),
            RequestChannel::Api,
        )
    }

    fn at() -> UtcTimestamp {
        UtcTimestamp::parse("2026-09-22T00:00:00Z").expect("valid")
    }

    fn policy_id() -> ModelDataPolicyId {
        ModelDataPolicyId::from_uuid(uuid::Uuid::from_u128(0x99))
    }

    /// A ruleset permitting a cloud route, so a narrowing submission can be observed.
    fn cloud_allowed() -> PolicyRules {
        PolicyRules {
            locality: Locality::ApprovedCloudAllowed,
            maximum_provider_retention: ProviderRetention::ProviderDefaultAllowed,
            provider_training_use: TrainingUse::ProviderDefaultAllowed,
            telemetry: Telemetry::LocalOnly,
            maximum_sensitivity: Sensitivity::Confidential,
            allow_fallback: FallbackPermission::Denied,
            ..PolicyRules::permissive()
        }
    }

    /// A ruleset that narrows locality to local-only.
    fn local_only() -> PolicyRules {
        PolicyRules {
            locality: Locality::LocalOnly,
            ..cloud_allowed()
        }
    }

    fn service() -> (PolicyService, Arc<InMemoryRepositories>) {
        let repositories = Arc::new(InMemoryRepositories::default());
        (
            PolicyService::new(Arc::clone(&repositories)
                as Arc<dyn crate::repository::policy::ModelDataPolicyRepository>),
            repositories,
        )
    }

    async fn put(
        service: &PolicyService,
        expected: u32,
        rules: PolicyRules,
    ) -> Result<crate::repository::policy::StoredPolicyVersion, PolicyServiceError> {
        service
            .put(&context(), expected, "policy", rules, at(), policy_id())
            .await
    }

    fn candidate(class: EndpointClass) -> RouteCandidate {
        let model = ModelRef::new(
            ProviderId::parse("local.ollama").expect("valid"),
            ModelId::parse("llama3.1").expect("valid"),
        );
        RouteCandidate {
            descriptor: CapabilityDescriptor::new(model.clone(), class),
            model,
            endpoint_class: class,
            region: None,
            retention: None,
            training_use: None,
        }
    }

    fn request(candidates: &[RouteCandidate], locality: Locality) -> EvaluationRequest<'_> {
        EvaluationRequest {
            candidates,
            sensitivity: Sensitivity::Internal,
            requirements: RouteRequirements {
                modalities: [Modality::Text].into_iter().collect(),
                required_capabilities: std::collections::BTreeSet::new(),
                locality,
            },
            today: IsoDate::parse("2026-09-22").expect("valid"),
            decided_at: at(),
        }
    }

    #[tokio::test]
    async fn a_first_write_creates_version_one_when_none_exists() {
        let (service, repositories) = service();
        assert_eq!(policies(&repositories), 0);

        let stored = put(&service, 0, cloud_allowed()).await.expect("creates");

        assert_eq!(stored.version, 1);
        assert_eq!(stored.status, ModelDataPolicyStatus::Active);
        assert_eq!(policies(&repositories), 1);
        // The read is what the fixture proves: a write that reported a version it never stored
        // would leave the next read failing, so the count matters as much as the return value.
        let read = service.active(&context()).await.expect("is in force");
        assert_eq!(read.version, 1);
    }

    /// Counts stored policy versions, panicking on a storage fault so an assertion reads plainly.
    fn policies(repositories: &InMemoryRepositories) -> usize {
        repositories.policy_count().expect("reads")
    }

    #[tokio::test]
    async fn a_stale_precondition_is_refused_and_changes_nothing() {
        let (service, repositories) = service();
        put(&service, 0, cloud_allowed()).await.expect("creates");

        // The caller believes nothing exists, but version 1 does. A conflict, not a silently
        // overwritten policy.
        let error = put(&service, 0, local_only()).await.expect_err("conflicts");
        assert_eq!(error.code(), "resource.version_conflict");
        match error {
            PolicyServiceError::VersionConflict { expected, actual } => {
                assert_eq!((expected, actual), (0, 1));
            }
            other => unreachable!("expected a version conflict, got {other:?}"),
        }

        // And the stored policy is untouched — not narrowed to the submitted rules and not
        // replaced. Asserting the error alone would pass against an implementation that wrote
        // the row and then reported a conflict.
        let read = service.active(&context()).await.expect("still in force");
        assert_eq!(read.version, 1);
        assert_eq!(read.rules.locality, Locality::ApprovedCloudAllowed);
        assert_eq!(policies(&repositories), 1);
    }

    #[tokio::test]
    async fn a_later_write_can_only_narrow_what_is_already_in_force() {
        let (service, _) = service();
        put(&service, 0, cloud_allowed()).await.expect("creates");

        // The submission says local-only, which is stricter, so it takes effect.
        let stored = put(&service, 1, local_only()).await.expect("narrows");
        assert_eq!(stored.version, 2);
        assert_eq!(stored.rules.locality, Locality::LocalOnly);

        // The submission now says cloud is allowed, which is LOOSER than what is in force. The
        // merge must keep local-only: a request body cannot widen a workspace policy, and if it
        // could then the write endpoint would be the mechanism by which a caller relaxed its own
        // rules — the one thing the precedence order exists to prevent.
        let widened = put(&service, 2, cloud_allowed())
            .await
            .expect("is accepted");
        assert_eq!(widened.version, 3);
        assert_eq!(
            widened.rules.locality,
            Locality::LocalOnly,
            "a submission must not widen the policy already in force",
        );
    }

    #[tokio::test]
    async fn a_contradictory_submission_is_refused_rather_than_stored() {
        let (service, repositories) = service();

        // An allow-list naming only one provider, then a submission naming only a different one.
        // The intersection is empty, which the domain reports as a contradiction. Storing it
        // would put an unsatisfiable policy permanently in force with nothing to indicate its
        // author had made a mistake.
        let mut first = cloud_allowed();
        first.allowed_providers = ["alpha".to_owned()].into_iter().collect();
        put(&service, 0, first).await.expect("creates");

        let mut second = cloud_allowed();
        second.allowed_providers = ["beta".to_owned()].into_iter().collect();
        let error = put(&service, 1, second).await.expect_err("contradicts");

        assert_eq!(error.code(), "jarvis.invalid_policy_layer");
        assert_eq!(policies(&repositories), 1, "nothing was stored");
    }

    #[tokio::test]
    async fn a_contradiction_is_not_reported_as_a_retryable_conflict() {
        // Both are conflicts in the HTTP sense, and the retry verdict is the difference that
        // matters to a client: a stale view is fixed by re-reading, while resubmitting the same
        // unsatisfiable rules reaches the same refusal forever. A client that retried the second
        // would loop.
        let (service, _) = service();
        let mut first = cloud_allowed();
        first.allowed_providers = ["alpha".to_owned()].into_iter().collect();
        put(&service, 0, first).await.expect("creates");

        let stale = put(&service, 0, cloud_allowed()).await.expect_err("stale");
        assert!(stale.retryable(), "a stale view is fixed by re-reading");

        let mut second = cloud_allowed();
        second.allowed_providers = ["beta".to_owned()].into_iter().collect();
        let contradictory = put(&service, 1, second).await.expect_err("contradicts");
        assert!(
            !contradictory.retryable(),
            "retrying the same unsatisfiable rules cannot succeed",
        );
    }

    #[tokio::test]
    async fn a_narrowed_policy_takes_effect_on_the_next_evaluation() {
        // The write is only meaningful if the evaluation reads it, so this drives both through
        // one service: the whole point of the round is that the stored policy reaches selection.
        let (service, _) = service();
        put(&service, 0, cloud_allowed()).await.expect("creates");

        let cloud = [candidate(EndpointClass::ApprovedCloud)];
        let reference = jarvis_domain::model::policy::PolicyVersionRef {
            policy_id: policy_id(),
            version: 1,
        };
        let decision = service
            .evaluate(
                &context(),
                reference,
                &request(&cloud, Locality::ApprovedCloudAllowed),
            )
            .await
            .expect("a cloud route is permitted");
        assert_eq!(
            decision.effective.endpoint_class,
            EndpointClass::ApprovedCloud
        );

        // Narrow it, then evaluate again. The same candidates must now be refused, which is what
        // proves the evaluation reads the stored rules rather than anything caller-supplied.
        put(&service, 1, local_only()).await.expect("narrows");
        let reference = jarvis_domain::model::policy::PolicyVersionRef {
            policy_id: policy_id(),
            version: 2,
        };
        let refusal = service
            .evaluate(
                &context(),
                reference,
                &request(&cloud, Locality::ApprovedCloudAllowed),
            )
            .await
            .expect_err("the narrower policy refuses a cloud route");
        assert_eq!(refusal.code(), "model.policy_unsatisfied");
        match refusal {
            PolicyServiceError::Unsatisfied(refusal) => {
                assert_eq!(refusal.rejected.len(), 1);
                assert_eq!(
                    refusal.rejected[0].reason,
                    jarvis_domain::model::policy::RejectionReason::LocalityViolated,
                );
            }
            other => unreachable!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unnamed_policy_version_is_not_found_rather_than_evaluated() {
        // A version that does not exist must not fall back to the active one: a caller naming a
        // version is asking to be evaluated under *those* rules, and answering with the current
        // policy would apply rules it never named.
        let (service, _) = service();
        put(&service, 0, cloud_allowed()).await.expect("creates");

        let cloud = [candidate(EndpointClass::ApprovedCloud)];
        let error = service
            .evaluate(
                &context(),
                jarvis_domain::model::policy::PolicyVersionRef {
                    policy_id: policy_id(),
                    version: 9,
                },
                &request(&cloud, Locality::ApprovedCloudAllowed),
            )
            .await
            .expect_err("absent");
        assert_eq!(error.code(), "model.policy_not_found");
    }

    #[tokio::test]
    async fn a_write_the_validation_refuses_stores_nothing() {
        let (service, repositories) = service();
        // A zero version cannot be created: the contract numbers versions from 1, and a stored
        // version 0 would make "no policy exists yet" indistinguishable from a real version.
        let error = service
            .put(&context(), 0, "policy", cloud_allowed(), at(), policy_id())
            .await;
        // This one succeeds — the service assigns version 1 — which is the point of the
        // assertion below: the *validation* path is exercised by an empty name.
        assert!(error.is_ok());
        assert_eq!(policies(&repositories), 1);

        let error = service
            .put(&context(), 1, "   ", cloud_allowed(), at(), policy_id())
            .await
            .expect_err("a blank name is unusable");
        assert_eq!(error.code(), "request.invalid");
        assert_eq!(policies(&repositories), 1, "nothing was stored");
    }
}
