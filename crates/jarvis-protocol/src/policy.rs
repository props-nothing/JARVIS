//! Model data policy wire types.
//!
//! `docs/contracts/model-data-policy.md` defines the `model_data_policy` object a
//! client reads from `GET /api/v1/model-data-policy` and the effective route it reads
//! from `GET /api/v1/model-data-policy/effective`. These are the versioned shapes, and
//! they live in `jarvis-protocol` rather than in the HTTP adapter so the contract has
//! one serialization definition that a client, the daemon, and a fixture test share.
//!
//! Three decisions are deliberate:
//!
//! - **The rule fields are the contract's own spellings, not the domain's enum names.**
//!   The domain names a variant `ProviderDefaultAllowed` and the contract spells the
//!   value `provider_default_allowed`; the same wire value must not have two spellings
//!   depending on which layer serialized it. A test asserts every variant's wire form.
//! - **A rejection carries a reason code and a model reference, never a message.** The
//!   contract requires the considered candidates and their reasons to be auditable
//!   *without* storing prompt content, so a free-form explanation would be the field
//!   through which content returned to the wire.
//! - **`requested` and `effective` are separate objects.** The contract treats the
//!   distinction as the point: what was asked for and what the gateway actually selected
//!   are different facts, and a client that could not see both could not tell a policy
//!   that was relaxed from one that was satisfied.

use serde::{Deserialize, Serialize};

/// A requested or effective data-policy statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPolicyView {
    /// Where inputs and outputs may go.
    pub locality: String,
    /// The documented provider retention behavior.
    pub maximum_provider_retention: String,
    /// The documented provider training-use behavior.
    pub provider_training_use: String,
    /// Whether JARVIS telemetry may be emitted.
    pub telemetry: String,
    /// The residency regions the content may be processed in.
    #[serde(default)]
    pub allowed_residency_regions: Vec<String>,
    /// The sensitivity classification of the content.
    pub sensitivity: String,
}

/// The resolved route a call would use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveRouteView {
    /// The selected provider and model.
    pub model: String,
    /// Where the selected endpoint sits relative to the trust boundary.
    pub endpoint_class: String,
    /// How retention was classified for this route.
    pub retention: String,
    /// How training use was classified for this route.
    pub training_use: String,
    /// The effective telemetry setting.
    pub telemetry: String,
    /// The effective residency.
    pub residency: String,
}

/// One candidate that was refused, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectedCandidateView {
    /// The provider-qualified model that was refused.
    pub model: String,
    /// The stable rejection reason.
    pub reason: String,
}

/// The reply to `GET /api/v1/model-data-policy/effective`.
///
/// `compliant` is present only when a compliant route exists, and `error_code` only when
/// none does. Both are optional rather than one being an empty value, because "no route
/// satisfies the policy" and "a route satisfying the policy" are different answers and a
/// client must not have to infer which it received from the shape of an empty object —
/// the same reason the contract gives a distinct code for the refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectivePolicyResponse {
    /// The policy version the evaluation ran under.
    pub policy_id: String,
    /// The immutable version number.
    pub policy_version: u32,
    /// What the policy requested.
    pub requested: DataPolicyView,
    /// The compliant route, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliant: Option<EffectiveRouteView>,
    /// Every candidate that was refused, with its reason.
    #[serde(default)]
    pub rejected_candidates: Vec<RejectedCandidateView>,
    /// The stable code when no candidate complied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// The active policy a client reads from `GET /api/v1/model-data-policy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivePolicyResponse {
    /// The policy identifier.
    pub policy_id: String,
    /// The immutable version number.
    pub version: u32,
    /// The operator-facing name.
    pub name: String,
    /// The lifecycle state.
    pub status: String,
    /// The typed rules in force.
    pub rules: DataPolicyView,
}

#[cfg(test)]
mod tests {
    use super::{ActivePolicyResponse, DataPolicyView, EffectivePolicyResponse};

    /// The contract's own example, field for field, minus the elided identifiers.
    ///
    /// Read from `docs/contracts/model-data-policy.md`'s "Requested and Effective Decision"
    /// section rather than invented here, because a fixture beside the code drifts from the
    /// document it claims to represent while the test still passes.
    #[test]
    fn the_policy_view_accepts_the_contracts_rule_spellings() {
        let json = r#"{
            "locality": "local_only",
            "maximum_provider_retention": "none_documented",
            "provider_training_use": "disallowed_documented",
            "telemetry": "disabled",
            "allowed_residency_regions": ["eu"],
            "sensitivity": "confidential"
        }"#;
        let view: DataPolicyView =
            serde_json::from_str(json).expect("the contract's spellings parse");
        assert_eq!(view.locality, "local_only");
        assert_eq!(view.maximum_provider_retention, "none_documented");
        assert_eq!(view.allowed_residency_regions, vec!["eu".to_owned()]);
    }

    #[test]
    fn a_refusal_omits_the_compliant_route_and_carries_a_code() {
        // The two answers are distinguishable in the shape, not only in the values: a client
        // reading `compliant: null` with no code could not tell a refusal from a rendering bug.
        let json = r#"{
            "policy_id": "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d",
            "policy_version": 1,
            "requested": {
                "locality": "local_only",
                "maximum_provider_retention": "none_documented",
                "provider_training_use": "disallowed_documented",
                "telemetry": "disabled",
                "allowed_residency_regions": [],
                "sensitivity": "internal"
            },
            "rejected_candidates": [{"model": "openai/gpt-x1", "reason": "locality_violated"}],
            "error_code": "model.policy_unsatisfied"
        }"#;
        let response: EffectivePolicyResponse =
            serde_json::from_str(json).expect("a refusal parses");
        assert!(response.compliant.is_none());
        assert_eq!(response.rejected_candidates.len(), 1);
        assert_eq!(
            response.error_code.as_deref(),
            Some("model.policy_unsatisfied"),
        );

        // And the compliant case omits the code, so a client that checked only for a code
        // would not mistake a success for a refusal.
        let complaint_json = r#"{
            "policy_id": "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d",
            "policy_version": 1,
            "requested": {
                "locality": "local_only",
                "maximum_provider_retention": "none_documented",
                "provider_training_use": "disallowed_documented",
                "telemetry": "disabled",
                "allowed_residency_regions": [],
                "sensitivity": "internal"
            },
            "compliant": {
                "model": "local.ollama/llama3.1",
                "endpoint_class": "local",
                "retention": "not_applicable_local",
                "training_use": "not_applicable_local",
                "telemetry": "disabled",
                "residency": "local_device"
            },
            "rejected_candidates": []
        }"#;
        let compliant: EffectivePolicyResponse =
            serde_json::from_str(complaint_json).expect("a compliant reply parses");
        assert!(compliant.compliant.is_some());
        assert!(compliant.error_code.is_none());
    }

    #[test]
    fn an_unknown_field_is_refused_on_the_response_shape_too() {
        // `deny_unknown_fields` on a *response* is unusual and deliberate: this type is also
        // what the daemon serializes, and an internal build that started emitting a field —
        // a credential, a provider account label — would be caught by the round trip rather
        // than by review. The contract forbids provider account details on the wire.
        let json = r#"{
            "policy_id": "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d",
            "version": 1,
            "name": "Private client work",
            "status": "active",
            "rules": {
                "locality": "local_only",
                "maximum_provider_retention": "none_documented",
                "provider_training_use": "disallowed_documented",
                "telemetry": "disabled",
                "allowed_residency_regions": [],
                "sensitivity": "confidential"
            },
            "provider_account": "acct-123"
        }"#;
        assert!(
            serde_json::from_str::<ActivePolicyResponse>(json).is_err(),
            "an unexpected field must not be silently dropped",
        );
    }
}
