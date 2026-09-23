//! The model data policy surface: what is in force, and what a route would be.
//!
//! Two endpoints from `docs/contracts/model-data-policy.md` live here:
//! `GET /api/v1/model-data-policy` and `GET /api/v1/model-data-policy/effective`. Each handler
//! resolves the authenticated scope, calls `jarvis_application::policy_service`, and renders
//! the result — nothing more, because the evaluation belongs in the application layer and a
//! handler that reimplemented it would be untestable without an HTTP client.
//!
//! Three contract rules shape this module:
//!
//! - **`effective` is diagnostic, not a way to force a route.** The contract says so
//!   explicitly, so this handler takes no model argument and returns the candidate the policy
//!   selected, never a caller's preference — a route-forcing parameter would be the mechanism
//!   by which a client bypassed its own workspace policy.
//! - **A refusal is a body, not only a status, and the evaluation's status is `200`.** "No
//!   compliant route" is a real answer a client must act on and it carries every rejected
//!   candidate with its reason; a `409` would tell a client the request failed when the answer
//!   is precisely what it asked for.
//! - **Nothing here names a provider account or credential.** The contract forbids provider
//!   account details on the wire, and the response types carry only rule values and a
//!   provider-qualified model, so there is no field through which one could arrive.
//!
//! The rule **values** are rendered as the contract spells them rather than as the domain's
//! `Display` happens to produce. A test asserts every variant's wire form, so a rename inside
//! the domain cannot silently change a client-visible value.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use jarvis_application::policy_service::{EvaluationRequest, PolicyServiceError};
use jarvis_application::repository::policy::StoredPolicyVersion;
use jarvis_application::request_context::{
    AuthenticationAssurance, RequestChannel, RequestContext,
};
use jarvis_domain::ids::{CorrelationId, RequestId};
use jarvis_domain::model::identity::EndpointClass;
use jarvis_domain::model::policy::{
    EffectiveResidency, EffectiveRetention, EffectiveTrainingUse, FallbackPermission, Locality,
    ModelRouteDecision, PolicyRules, PolicyVersionRef, ProviderRetention, RequestedDataPolicy,
    Sensitivity, Telemetry, TrainingUse,
};
use jarvis_domain::model::stream::{Modality, RouteRequirements};
use jarvis_domain::time::{IsoDate, UtcTimestamp};
use jarvis_protocol::{
    ActivePolicyResponse, DataPolicyView, EffectivePolicyResponse, EffectiveRouteView,
    PolicyRulesView, PutPolicyRequest, PutPolicyResponse, RejectedCandidateView,
};

use crate::http::{ApiState, AuthenticatedClient, error_response, runs};

/// The sensitivity an effective-route probe evaluates at.
///
/// `Confidential` rather than `Public`, because the probe asks "would a call carrying this
/// content be permitted", and answering at the most permissive classification would say yes
/// for content the policy forbids. Confidential is the contract's own example ceiling, so it
/// is the value an operator most often needs an answer for.
///
/// It is deliberately not a caller-supplied field: `effective` is diagnostic, and a client
/// that could choose the sensitivity would have a way to enumerate which classifications get
/// through — a probe into the policy rather than a report about it.
const PROBE_SENSITIVITY: Sensitivity = Sensitivity::Confidential;

/// Handles `GET /api/v1/model-data-policy`.
pub async fn read_active_policy(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
) -> Response {
    let Some(service) = state.policies.as_ref() else {
        return not_ready();
    };
    let context = context_for(&client);
    match service.active(&context).await {
        Ok(stored) => json_response(StatusCode::OK, &active_view(&stored)),
        Err(error) => policy_error_response(&error),
    }
}

/// Handles `PUT /api/v1/model-data-policy`.
///
/// The rules are written through a service call that **merges** them against what is already in
/// force, so a request body can only narrow the workspace policy. That is why the handler parses
/// into typed values and refuses an unsupported spelling rather than passing the body along: the
/// merge can only narrow a rule it understands, so a value this build did not parse would be a
/// rule the write silently dropped — the one edit direction that widens a policy.
pub async fn put_active_policy(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    headers: axum::http::HeaderMap,
    body: axum::Json<PutPolicyRequest>,
) -> Response {
    let Some(service) = state.policies.as_ref() else {
        return not_ready();
    };

    // `Idempotency-Key` is required by the contract. Without it a client that retries a timed-out
    // write creates a second version of the same rules, and the workspace's version number
    // advances for a change that was already applied — so a caller can no longer tell how many
    // distinct policies it has made. The key is checked for presence only; the store's
    // `expected_version` precondition is what actually prevents a duplicate, so recording the key
    // would be a second mechanism for the same guarantee.
    if !headers.contains_key("idempotency-key") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "An idempotency key is required.",
            false,
        );
    }

    let rules = match submitted_rules(&body.rules) {
        Ok(rules) => rules,
        Err(code) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                code,
                "The submitted rules contain a value this build does not support.",
                false,
            );
        }
    };

    let context = context_for(&client);
    // The instant comes from the inventory's build time when the daemon recorded one, so the
    // version's `created_at` matches the instant the rest of the surface evaluates against. A
    // clock failure is a 500 rather than a defaulted timestamp: a policy row whose creation
    // instant was invented would be ordered wrongly against every other version.
    let Ok(now) = state
        .inventory
        .as_ref()
        .and_then(|inventory| inventory.built_at())
        .map_or_else(|| crate::time::SystemClock::new().now(), Ok)
    else {
        return internal_failure();
    };

    match service
        .put(
            &context,
            body.expected_version,
            &body.name,
            rules,
            now,
            // A workspace holds one logical policy that accumulates versions, so the identifier
            // is derived from the workspace rather than generated per write. Generating one would
            // make every version its own policy, which would break both the version chain and the
            // contract's "changing rules creates a new version" rule.
            policy_id_for(context.workspace_id),
        )
        .await
    {
        Ok(stored) => json_response(
            StatusCode::OK,
            &PutPolicyResponse {
                policy_id: stored.policy_id.to_string(),
                version: stored.version,
                name: stored.name,
                status: stored.status.as_str().to_owned(),
                rules: full_rules_view(&stored.rules),
            },
        ),
        Err(error) => policy_error_response(&error),
    }
}

/// Returns the policy identifier a workspace's versions belong to.
///
/// Derived from the workspace so every version lands in one chain. This is a stability property,
/// not a privacy one: the identifier is already the workspace's own, and the scope is resolved
/// server-side, so a caller can only ever reach its own chain.
fn policy_id_for(
    workspace: jarvis_domain::ids::WorkspaceId,
) -> jarvis_domain::ids::ModelDataPolicyId {
    jarvis_domain::ids::ModelDataPolicyId::from_uuid(workspace.as_uuid())
}

/// Parses the submitted rules into the domain type, refusing any unsupported spelling.
///
/// Every value goes through a parser rather than being stored as text, because the domain's
/// `PolicyRules` is a typed struct with no free-form field: a rule the domain cannot express
/// cannot be merged, and a stored-but-unmergeable rule would be one this layer wrote and the
/// selector never applied.
fn submitted_rules(view: &PolicyRulesView) -> Result<PolicyRules, &'static str> {
    Ok(PolicyRules {
        locality: parse_locality(&view.locality)?,
        allowed_providers: view
            .allowed_providers
            .as_ref()
            .map(|values| values.iter().cloned().collect())
            .unwrap_or_default(),
        allowed_models: view
            .allowed_models
            .as_ref()
            .map(|values| values.iter().cloned().collect())
            .unwrap_or_default(),
        maximum_provider_retention: parse_retention_requirement(&view.maximum_provider_retention)?,
        provider_training_use: parse_training_requirement(&view.provider_training_use)?,
        telemetry: parse_telemetry(&view.telemetry)?,
        allowed_residency_regions: view.allowed_residency_regions.iter().cloned().collect(),
        maximum_sensitivity: parse_sensitivity(&view.maximum_sensitivity)?,
        allow_fallback: parse_fallback(&view.allow_fallback)?,
    })
}

fn parse_locality(value: &str) -> Result<Locality, &'static str> {
    match value {
        "local_only" => Ok(Locality::LocalOnly),
        "private_network_allowed" => Ok(Locality::PrivateNetworkAllowed),
        "approved_cloud_allowed" => Ok(Locality::ApprovedCloudAllowed),
        _ => Err("request.semantic_invalid"),
    }
}

fn parse_retention_requirement(value: &str) -> Result<ProviderRetention, &'static str> {
    match value {
        "none_documented" => Ok(ProviderRetention::NoneDocumented),
        "bounded_documented" => Ok(ProviderRetention::BoundedDocumented),
        "provider_default_allowed" => Ok(ProviderRetention::ProviderDefaultAllowed),
        _ => Err("request.semantic_invalid"),
    }
}

fn parse_training_requirement(value: &str) -> Result<TrainingUse, &'static str> {
    match value {
        "disallowed_documented" => Ok(TrainingUse::DisallowedDocumented),
        "account_policy_allowed" => Ok(TrainingUse::AccountPolicyAllowed),
        "provider_default_allowed" => Ok(TrainingUse::ProviderDefaultAllowed),
        _ => Err("request.semantic_invalid"),
    }
}

fn parse_telemetry(value: &str) -> Result<Telemetry, &'static str> {
    match value {
        "disabled" => Ok(Telemetry::Disabled),
        "local_only" => Ok(Telemetry::LocalOnly),
        _ => Err("request.semantic_invalid"),
    }
}

fn parse_sensitivity(value: &str) -> Result<Sensitivity, &'static str> {
    match value {
        "public" => Ok(Sensitivity::Public),
        "internal" => Ok(Sensitivity::Internal),
        "confidential" => Ok(Sensitivity::Confidential),
        "restricted" => Ok(Sensitivity::Restricted),
        _ => Err("request.semantic_invalid"),
    }
}

fn parse_fallback(value: &str) -> Result<FallbackPermission, &'static str> {
    match value {
        "denied" => Ok(FallbackPermission::Denied),
        "compliant_only" => Ok(FallbackPermission::CompliantOnly),
        _ => Err("request.semantic_invalid"),
    }
}

/// Renders a stored policy version for either `GET` or `PUT`.
///
/// One function for both, so the read and the write cannot describe the same stored row
/// differently — a client that PUT a policy and immediately GET it must see the same document.
fn active_view(stored: &StoredPolicyVersion) -> ActivePolicyResponse {
    ActivePolicyResponse {
        policy_id: stored.policy_id.to_string(),
        version: stored.version,
        name: stored.name.clone(),
        status: stored.status.as_str().to_owned(),
        rules: full_rules_view(&stored.rules),
    }
}

/// Renders the full nine-field rules of a stored policy.
///
/// Distinct from [`rules_view`], which renders the six-field requested/effective **statement**.
/// The two are different questions: a statement records what a call asked for and what was
/// selected, while a stored policy is the complete rule set a client reads and writes. Rendering
/// a stored policy through the statement shape dropped `allowed_providers`, `allowed_models`, and
/// `allow_fallback` — so a client doing read-modify-write would submit a body that reset two
/// allow-lists to "no restriction" and the fallback rule to a default it never chose.
fn full_rules_view(rules: &PolicyRules) -> PolicyRulesView {
    PolicyRulesView {
        locality: locality_of(rules.locality).to_owned(),
        maximum_provider_retention: retention_requirement_of(rules.maximum_provider_retention),
        provider_training_use: training_requirement_of(rules.provider_training_use),
        telemetry: telemetry_of(rules.telemetry),
        allowed_residency_regions: rules.allowed_residency_regions.iter().cloned().collect(),
        maximum_sensitivity: sensitivity_of(rules.maximum_sensitivity),
        allow_fallback: fallback_of(rules.allow_fallback).to_owned(),
        // The domain keeps the contract's distinction: an empty set means "no restriction at this
        // layer", not "permit nothing". Collapsing both to `[]` would turn the most permissive
        // statement into the most restrictive-looking one, and a read-modify-write would then
        // submit `[]` and narrow the policy to nothing.
        allowed_providers: allow_list(&rules.allowed_providers),
        allowed_models: allow_list(&rules.allowed_models),
    }
}

/// Renders an allow-list, preserving "this layer restricts nothing" as an absent field.
fn allow_list(values: &std::collections::BTreeSet<String>) -> Option<Vec<String>> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().cloned().collect())
    }
}

fn fallback_of(value: FallbackPermission) -> &'static str {
    match value {
        FallbackPermission::Denied => "denied",
        FallbackPermission::CompliantOnly => "compliant_only",
    }
}

/// Handles `GET /api/v1/model-data-policy/effective`.
pub async fn read_effective_route(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
) -> Response {
    let Some(service) = state.policies.as_ref() else {
        return not_ready();
    };
    let context = context_for(&client);

    // The active policy is read once and its reference reused, so the response's policy
    // version and the version the evaluation ran under cannot disagree — a second read could
    // see a policy updated in between and report a version that did not apply.
    let active = match service.active(&context).await {
        Ok(active) => active,
        Err(error) => return policy_error_response(&error),
    };
    let reference = active.reference();

    let Some(inventory) = state.inventory.as_ref() else {
        // A daemon with no provider configured has no candidates to evaluate, which is a
        // readiness fact rather than a policy refusal: reporting it as `model.policy_unsatisfied`
        // would say the policy rejected something when nothing was offered.
        return not_ready();
    };

    // The evaluation instant comes from the inventory's own build time when the daemon
    // recorded one, so two probes through one daemon evaluate evidence freshness against the
    // same moment. Falling back to the system clock keeps the route usable rather than failing
    // it: a probe whose date is slightly newer than the inventory's is still a truthful
    // evaluation, while a 500 would tell the operator nothing.
    let now = match inventory.built_at() {
        Some(built_at) => built_at,
        None => match crate::time::SystemClock::new().now() {
            Ok(now) => now,
            Err(_) => return internal_failure(),
        },
    };
    let (today, decided_at) = evaluation_instant(now);
    let request = EvaluationRequest {
        candidates: inventory.candidates(),
        sensitivity: PROBE_SENSITIVITY,
        requirements: RouteRequirements {
            modalities: [Modality::Text].into_iter().collect(),
            // No required capabilities: nothing measures them yet (`BRN-011`), and requiring one
            // would refuse every candidate on an evidence gap rather than on a policy decision.
            required_capabilities: std::collections::BTreeSet::new(),
            // **No locality floor**, deliberately, even though the policy's own locality is right
            // here. The requirement and the rule are different quantities: `rules.locality` is a
            // *ceiling* over what a route may use, while `requirements.locality` is a *floor* the
            // call must not go below. Setting the floor from the ceiling makes the two the same
            // value and the floor redundant — and it defeats a locality exception, because the
            // selector relaxes the ceiling and cannot relax a floor the caller stated. The policy
            // is what should constrain a diagnostic probe, so the floor is left at the most
            // permissive value and `evaluate` refuses on the rule.
            locality: Locality::ApprovedCloudAllowed,
        },
        today,
        decided_at,
    };

    match service.evaluate(&context, reference, &request).await {
        Ok(decision) => json_response(StatusCode::OK, &compliant_view(&decision)),
        // A refusal is `200` with a body, because the evaluation succeeded and the answer is
        // that nothing complies. The rejected candidates are the actionable half.
        Err(PolicyServiceError::Unsatisfied(refusal)) => json_response(
            StatusCode::OK,
            &refusal_view(reference, &refusal.requested, &refusal.rejected),
        ),
        Err(error) => policy_error_response(&error),
    }
}

/// Renders a compliant decision into the contract's shape.
fn compliant_view(decision: &ModelRouteDecision) -> EffectivePolicyResponse {
    EffectivePolicyResponse {
        policy_id: decision.policy.policy_id.to_string(),
        policy_version: decision.policy.version,
        requested: requested_view(&decision.requested),
        compliant: Some(EffectiveRouteView {
            model: decision.effective.model.to_string(),
            endpoint_class: endpoint_class_of(decision.effective.endpoint_class).to_owned(),
            retention: retention_of(decision.effective.retention).to_owned(),
            training_use: training_use_of(decision.effective.training_use).to_owned(),
            telemetry: telemetry_of(decision.effective.telemetry),
            residency: residency_of(decision.effective.residency).to_owned(),
        }),
        rejected_candidates: decision
            .rejected_candidates
            .iter()
            .map(rejected_view)
            .collect(),
        // No code, so a client that checked only for one cannot read a success as a refusal.
        error_code: None,
    }
}

/// Renders a refusal into the contract's shape.
fn refusal_view(
    reference: PolicyVersionRef,
    requested: &RequestedDataPolicy,
    rejected: &[jarvis_domain::model::policy::RejectedCandidate],
) -> EffectivePolicyResponse {
    EffectivePolicyResponse {
        policy_id: reference.policy_id.to_string(),
        policy_version: reference.version,
        requested: requested_view(requested),
        // Absent rather than a placeholder route: naming a model here would read as a
        // selection, and the whole point of the field's optionality is that a client can tell
        // "nothing complies" from "something does".
        compliant: None,
        rejected_candidates: rejected.iter().map(rejected_view).collect(),
        error_code: Some("model.policy_unsatisfied".to_owned()),
    }
}

/// Renders one rejected candidate.
fn rejected_view(
    rejected: &jarvis_domain::model::policy::RejectedCandidate,
) -> RejectedCandidateView {
    RejectedCandidateView {
        model: rejected.model.to_string(),
        // The **code**, not the domain's `Display`. The contract calls these "candidate
        // rejection reason codes" and requires "safe rejection reason codes", and
        // `RejectionReason`'s `Display` is human prose for an operator log
        // (`"locality violated"`). Rendering that as the wire value ships a UI sentence in a
        // field a client switches on, and it is a different spelling from the
        // `locality_violated` the contract's own response example shows. Every other value in
        // this module goes through a spelling function for the same reason; this one had none.
        reason: rejection_reason_of(rejected.reason).to_owned(),
    }
}

/// The contract's spelling of a rejection reason.
///
/// Written out rather than derived from `serde`, so the wire vocabulary is visible in one place
/// and a variant added to the domain cannot reach a client without someone deciding the code.
fn rejection_reason_of(reason: jarvis_domain::model::policy::RejectionReason) -> &'static str {
    use jarvis_domain::model::policy::RejectionReason;
    match reason {
        RejectionReason::ProviderNotAllowed => "provider_not_allowed",
        RejectionReason::ModelNotAllowed => "model_not_allowed",
        RejectionReason::LocalityViolated => "locality_violated",
        RejectionReason::RetentionUnsatisfied => "retention_unsatisfied",
        RejectionReason::TrainingUseUnsatisfied => "training_use_unsatisfied",
        RejectionReason::ResidencyUnsatisfied => "residency_unsatisfied",
        RejectionReason::CapabilityUnattested => "capability_unattested",
        RejectionReason::EvidenceStale => "evidence_stale",
        RejectionReason::EvidenceMissing => "evidence_missing",
        RejectionReason::SensitivityExceedsPolicy => "sensitivity_exceeds_policy",
    }
}

/// Renders the requested side of a decision.
fn requested_view(requested: &RequestedDataPolicy) -> DataPolicyView {
    DataPolicyView {
        locality: locality_of(requested.locality).to_owned(),
        maximum_provider_retention: retention_requirement_of(requested.maximum_provider_retention),
        provider_training_use: training_requirement_of(requested.provider_training_use),
        telemetry: telemetry_of(requested.telemetry),
        allowed_residency_regions: requested
            .allowed_residency_regions
            .iter()
            .cloned()
            .collect(),
        sensitivity: sensitivity_of(requested.sensitivity),
    }
}

/// The contract's spelling of a locality.
fn locality_of(value: Locality) -> &'static str {
    match value {
        Locality::LocalOnly => "local_only",
        Locality::PrivateNetworkAllowed => "private_network_allowed",
        Locality::ApprovedCloudAllowed => "approved_cloud_allowed",
    }
}

/// The contract's spelling of a required retention behavior.
fn retention_requirement_of(value: ProviderRetention) -> String {
    match value {
        ProviderRetention::NoneDocumented => "none_documented",
        ProviderRetention::BoundedDocumented => "bounded_documented",
        ProviderRetention::ProviderDefaultAllowed => "provider_default_allowed",
    }
    .to_owned()
}

/// The contract's spelling of a required training-use behavior.
fn training_requirement_of(value: TrainingUse) -> String {
    match value {
        TrainingUse::DisallowedDocumented => "disallowed_documented",
        TrainingUse::AccountPolicyAllowed => "account_policy_allowed",
        TrainingUse::ProviderDefaultAllowed => "provider_default_allowed",
    }
    .to_owned()
}

/// The contract's spelling of a telemetry setting.
fn telemetry_of(value: Telemetry) -> String {
    match value {
        Telemetry::Disabled => "disabled",
        Telemetry::LocalOnly => "local_only",
    }
    .to_owned()
}

/// The contract's spelling of a sensitivity classification.
fn sensitivity_of(value: Sensitivity) -> String {
    match value {
        Sensitivity::Public => "public",
        Sensitivity::Internal => "internal",
        Sensitivity::Confidential => "confidential",
        Sensitivity::Restricted => "restricted",
    }
    .to_owned()
}

/// The contract's spelling of an endpoint class.
fn endpoint_class_of(value: EndpointClass) -> &'static str {
    match value {
        EndpointClass::Local => "local",
        EndpointClass::PrivateNetwork => "private_network",
        EndpointClass::ApprovedCloud => "approved_cloud",
    }
}

/// The contract's spelling of an effective retention classification.
fn retention_of(value: EffectiveRetention) -> &'static str {
    match value {
        EffectiveRetention::NotApplicableLocal => "not_applicable_local",
        EffectiveRetention::BoundedDocumented => "bounded_documented",
        EffectiveRetention::NoneDocumented => "none_documented",
        EffectiveRetention::ProviderDefault => "provider_default",
    }
}

/// The contract's spelling of an effective training-use classification.
fn training_use_of(value: EffectiveTrainingUse) -> &'static str {
    match value {
        EffectiveTrainingUse::NotApplicableLocal => "not_applicable_local",
        EffectiveTrainingUse::DisallowedDocumented => "disallowed_documented",
        EffectiveTrainingUse::AccountPolicy => "account_policy",
    }
}

/// The contract's spelling of an effective residency.
fn residency_of(value: EffectiveResidency) -> &'static str {
    match value {
        EffectiveResidency::LocalDevice => "local_device",
        // The class, not the region: the contract's own example shows `region` as the value,
        // and the region the candidate named is a property of the candidate rather than of the
        // classification.
        EffectiveResidency::Region => "region",
    }
}

/// Builds the request context for an authenticated client.
fn context_for(client: &AuthenticatedClient) -> RequestContext {
    let scope = runs::resolve_scope(client);
    RequestContext::new(
        RequestId::from_uuid(uuid::Uuid::now_v7()),
        CorrelationId::from_uuid(uuid::Uuid::now_v7()),
        scope.principal_id,
        AuthenticationAssurance::Standard,
        scope.workspace_id,
        RequestChannel::Api,
    )
}

/// Maps a policy service error to its contract status and envelope.
///
/// A refusal is deliberately absent from this mapping: an unsatisfied policy is a `200` with a
/// body, handled by the caller, and routing it here would give a client a status that says the
/// request failed.
fn policy_error_response(error: &PolicyServiceError) -> Response {
    let status = match error {
        PolicyServiceError::NoActivePolicy | PolicyServiceError::PolicyNotFound => {
            StatusCode::NOT_FOUND
        }
        PolicyServiceError::CandidatesUnbounded { .. } | PolicyServiceError::Invalid { .. } => {
            StatusCode::BAD_REQUEST
        }
        // A stale precondition is a conflict, not a bad request: the submitted policy was
        // well-formed and authorized, and the only thing wrong was that the caller's view had
        // moved on. `409` is the status a client retries after a re-read; `400` would suggest
        // the body was unusable.
        PolicyServiceError::VersionConflict { .. }
        | PolicyServiceError::Contradictory { .. }
        | PolicyServiceError::Unsatisfied(_) => StatusCode::CONFLICT,
        PolicyServiceError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, error.code(), message_for(error), error.retryable())
}

/// Returns a message safe for the requesting principal.
fn message_for(error: &PolicyServiceError) -> &'static str {
    match error {
        PolicyServiceError::NoActivePolicy => "No model data policy is in force.",
        PolicyServiceError::PolicyNotFound => "No such model data policy.",
        PolicyServiceError::Unsatisfied(_) => "No compliant model route satisfies the policy.",
        PolicyServiceError::CandidatesUnbounded { .. } => "Too many model candidates were offered.",
        PolicyServiceError::VersionConflict { .. } => {
            "The policy changed since it was read; re-read and resubmit."
        }
        PolicyServiceError::Contradictory { .. } => {
            "The submitted rules contradict the policy already in force."
        }
        PolicyServiceError::Invalid { .. } => "The submitted policy is not usable.",
        PolicyServiceError::Storage(_) => "The policy could not be read.",
    }
}

/// The refusal for a policy surface that is not configured.
fn not_ready() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "service.not_ready",
        "The model data policy surface is not available yet.",
        true,
    )
}

/// The refusal for a response this daemon could not render.
fn internal_failure() -> Response {
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal.failure",
        "The response could not be rendered.",
        true,
    )
}

/// Serializes a value into an `application/json` response.
fn json_response(status: StatusCode, value: &impl serde::Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Err(_) => internal_failure(),
    }
}

/// Formats the instant and day an evaluation runs at.
///
/// A pair rather than two calls, so the decision instant and the evidence-revalidation day are
/// derived from **one** reading of the clock: two readings could straddle midnight and make the
/// day and the instant describe different moments.
#[must_use]
pub fn evaluation_instant(now: UtcTimestamp) -> (IsoDate, UtcTimestamp) {
    // The conversion lives in the domain, because deriving a date needs `jiff`'s zone handling and
    // this crate's callers should not each reach for it.
    (now.utc_date(), now)
}

#[cfg(test)]
mod tests {
    use super::{
        endpoint_class_of, locality_of, rejection_reason_of, residency_of, retention_of,
        retention_requirement_of, sensitivity_of, telemetry_of, training_requirement_of,
        training_use_of,
    };
    use jarvis_domain::model::identity::EndpointClass;
    use jarvis_domain::model::policy::{
        EffectiveResidency, EffectiveRetention, EffectiveTrainingUse, Locality, ProviderRetention,
        RejectionReason, Sensitivity, Telemetry, TrainingUse,
    };

    /// Every wire value must be the contract's spelling, and none may be the domain's prose.
    ///
    /// The spelling functions exist precisely because the domain's `Display` is a *different*
    /// vocabulary — `RejectionReason::LocalityViolated` displays as `"locality violated"` for an
    /// operator log, while the contract calls these "reason codes" and the response example shows
    /// `locality_violated`. A handler test can only reach the variants a fixture produces, which
    /// is how that mismatch survived until this test: the one code a test did reach was rendered
    /// by the domain and no test asserted the string.
    ///
    /// Asserting **every** variant is the point. A spelling function with a missing arm is the
    /// failure mode, and enumerating the domains here means a new variant cannot be added without
    /// this test failing to compile.
    #[test]
    fn every_rule_value_is_rendered_with_the_contracts_spelling() {
        for (value, expected) in [
            (Locality::LocalOnly, "local_only"),
            (Locality::PrivateNetworkAllowed, "private_network_allowed"),
            (Locality::ApprovedCloudAllowed, "approved_cloud_allowed"),
        ] {
            assert_eq!(locality_of(value), expected);
        }
        for (value, expected) in [
            (ProviderRetention::NoneDocumented, "none_documented"),
            (ProviderRetention::BoundedDocumented, "bounded_documented"),
            (
                ProviderRetention::ProviderDefaultAllowed,
                "provider_default_allowed",
            ),
        ] {
            assert_eq!(retention_requirement_of(value), expected);
        }
        for (value, expected) in [
            (TrainingUse::DisallowedDocumented, "disallowed_documented"),
            (TrainingUse::AccountPolicyAllowed, "account_policy_allowed"),
            (
                TrainingUse::ProviderDefaultAllowed,
                "provider_default_allowed",
            ),
        ] {
            assert_eq!(training_requirement_of(value), expected);
        }
        for (value, expected) in [
            (Telemetry::Disabled, "disabled"),
            (Telemetry::LocalOnly, "local_only"),
        ] {
            assert_eq!(telemetry_of(value), expected);
        }
        for (value, expected) in [
            (Sensitivity::Public, "public"),
            (Sensitivity::Internal, "internal"),
            (Sensitivity::Confidential, "confidential"),
            (Sensitivity::Restricted, "restricted"),
        ] {
            assert_eq!(sensitivity_of(value), expected);
        }
        for (value, expected) in [
            (EndpointClass::Local, "local"),
            (EndpointClass::PrivateNetwork, "private_network"),
            (EndpointClass::ApprovedCloud, "approved_cloud"),
        ] {
            assert_eq!(endpoint_class_of(value), expected);
        }
        for (value, expected) in [
            (
                EffectiveRetention::NotApplicableLocal,
                "not_applicable_local",
            ),
            (EffectiveRetention::BoundedDocumented, "bounded_documented"),
            (EffectiveRetention::NoneDocumented, "none_documented"),
            // Distinct from `none_documented` on purpose: the provider's default terms document
            // nothing, so recording them as documented evidence is the promotion the contract
            // forbids.
            (EffectiveRetention::ProviderDefault, "provider_default"),
        ] {
            assert_eq!(retention_of(value), expected);
        }
        for (value, expected) in [
            (
                EffectiveTrainingUse::NotApplicableLocal,
                "not_applicable_local",
            ),
            (
                EffectiveTrainingUse::DisallowedDocumented,
                "disallowed_documented",
            ),
            (EffectiveTrainingUse::AccountPolicy, "account_policy"),
        ] {
            assert_eq!(training_use_of(value), expected);
        }
        for (value, expected) in [
            (EffectiveResidency::LocalDevice, "local_device"),
            (EffectiveResidency::Region, "region"),
        ] {
            assert_eq!(residency_of(value), expected);
        }
    }

    #[test]
    fn every_rejection_reason_is_a_code_rather_than_a_sentence() {
        // The defect this test was written for: `rejected_view` rendered the domain's `Display`,
        // which put `"locality violated"` on the wire where the contract's response example shows
        // `locality_violated`. The space check is what makes the class fail rather than the
        // instance: prose contains a space and a snake_case code does not.
        for (value, expected) in [
            (RejectionReason::ProviderNotAllowed, "provider_not_allowed"),
            (RejectionReason::ModelNotAllowed, "model_not_allowed"),
            (RejectionReason::LocalityViolated, "locality_violated"),
            (
                RejectionReason::RetentionUnsatisfied,
                "retention_unsatisfied",
            ),
            (
                RejectionReason::TrainingUseUnsatisfied,
                "training_use_unsatisfied",
            ),
            (
                RejectionReason::ResidencyUnsatisfied,
                "residency_unsatisfied",
            ),
            (
                RejectionReason::CapabilityUnattested,
                "capability_unattested",
            ),
            (RejectionReason::EvidenceStale, "evidence_stale"),
            (RejectionReason::EvidenceMissing, "evidence_missing"),
            (
                RejectionReason::SensitivityExceedsPolicy,
                "sensitivity_exceeds_policy",
            ),
        ] {
            let code = rejection_reason_of(value);
            assert_eq!(code, expected);
            assert!(!code.contains(' '), "{code:?} is prose, not a code");
            // And it must differ from what the domain's `Display` produces, so this test fails if
            // a future refactor routes the wire value back through `to_string()`.
            assert_ne!(code, value.to_string());
        }
    }
}
