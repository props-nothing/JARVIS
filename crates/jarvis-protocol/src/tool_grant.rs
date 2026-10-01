//! The tool-authorization wire vocabulary.
//!
//! **These views are the contract a control plane binds to**, and they are deliberately separate from the
//! stored shapes: a grant row carries things a client has no business seeing (the serialized identity the
//! evaluator compares), while a client needs things a row does not carry (whether more pages remain). The
//! projection is one-way, and it happens in `http::tool_grant` so a handler cannot leak a field by
//! serializing a domain value.
//!
//! ## Why the shapes are split the way they are
//!
//! **A write request carries a target principal but never an operator.** A grant is *for* somebody, so the
//! principal is a legitimate field; who configured it is not — that comes from the authenticated context, and
//! a body field for it would let a client attribute a widening to a principal who never wrote it.
//!
//! **`expected_version` is absent from [`WriteToolGrantRequest`] and required by the replace route's own
//! body.** The two verbs must not be interchangeable: a create that silently replaced an existing grant would
//! discard ceilings an operator configured and nobody asked to change, so `PUT` creates and `PATCH` replaces,
//! and the version is what distinguishes them rather than a field's incidental presence.
//!
//! **`workspace_wide` is required on a deny rule.** A refusal that names no workspace applies everywhere,
//! which is the broadest form and must stay expressible — but it must not be the *default*, because a
//! defaulted field would turn an ordinary workspace refusal into a profile-wide one. A required boolean makes
//! the choice explicit at every call site.
//!
//! **`deny_unknown_fields` on every request**, so a client sending `principal` where the shape says
//! `principal_id` is told its body is wrong rather than silently having the field ignored. That is the same
//! choice the approval write shapes make, and it is the difference between a typo being reported and a typo
//! becoming a grant that authorizes less than the operator intended.

use serde::{Deserialize, Serialize};

/// A grant as a client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantView {
    /// The grant's own identifier, which is what a revoke or a read addresses.
    pub grant_id: String,
    /// The capability granted, e.g. `clock.now@1`.
    pub capability: String,
    /// The principal the grant is for.
    pub principal_id: String,
    /// The workspace it applies in.
    pub workspace_id: String,
    /// The scopes conferred, as contract strings.
    pub scopes: Vec<String>,
    /// The effects permitted, as contract strings.
    pub effects: Vec<String>,
    /// The greatest risk permitted.
    pub risk_ceiling: String,
    /// The most sensitive argument permitted.
    pub sensitivity_ceiling: String,
    /// When it stops applying, or `None` for a standing grant.
    ///
    /// **`None` and absent are the same statement to a client**, which is why the field is optional rather
    /// than sentinel-valued: a far-future instant would make "never expires" and "expires in the year 9999"
    /// indistinguishable in a response, and the sentinel would eventually be reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Whether the grant is currently in force. A revoked grant is returned by a read and carries `false`.
    pub active: bool,
    /// The optimistic version, which a replace or a revoke must echo.
    pub version: u32,
    /// The operator who configured it, from the authenticated context.
    pub granted_by: String,
    /// When it was written.
    pub created_at: String,
    /// When it last changed.
    pub updated_at: String,
}

/// One page of grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrantListView {
    /// The grants in this page, in capability order.
    pub grants: Vec<GrantView>,
    /// Whether the store stopped at its bound with rows still unread.
    ///
    /// **Required rather than inferred from the length**, because a client that cannot tell a full page from
    /// an exhausted one concludes there is nothing more to configure — the short-page defect the approval
    /// listing records, arriving on a surface where "nothing more" means an operator stops looking.
    pub has_more: bool,
    /// The largest page this daemon serves.
    pub max_page: u32,
}

/// A request to create or replace a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteToolGrantRequest {
    /// The capability to grant.
    pub capability: String,
    /// The principal the grant is for.
    pub principal_id: String,
    /// The scopes to confer, as contract strings. Empty confers none.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The effects to permit, as contract strings.
    #[serde(default)]
    pub effects: Vec<String>,
    /// The greatest risk to permit.
    pub risk_ceiling: String,
    /// The most sensitive argument to permit.
    pub sensitivity_ceiling: String,
    /// When the grant stops applying, or absent for a standing grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// A deny rule as a client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenyRuleView {
    /// The rule's identifier, which is what a removal addresses.
    pub deny_rule_id: String,
    /// The capability it refuses, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// The principal it refuses, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    /// The workspace it applies in, when it is scoped to one.
    ///
    /// **Absent means every workspace**, which is the broadest and therefore most important value for a
    /// client to be able to read — a listing that hid it would make a profile-wide refusal look like a
    /// workspace-scoped one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// The effects it refuses, as contract strings.
    pub effects: Vec<String>,
    /// The reason shown to a refused principal.
    pub reason: String,
    /// When it was created.
    pub created_at: String,
}

/// A listing of deny rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDenyRuleListView {
    /// The rules, in insertion order.
    pub rules: Vec<DenyRuleView>,
}

/// A request to store a deny rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteDenyRuleRequest {
    /// The capability to refuse, or absent for a rule that names only effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// The principal to refuse, or absent for every principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    /// The effects to refuse, as contract strings.
    #[serde(default)]
    pub effects: Vec<String>,
    /// The reason shown to a principal whose call was refused.
    pub reason: String,
    /// Whether the refusal is scoped to the caller's workspace rather than applying everywhere.
    pub workspace_wide: bool,
}

/// The acknowledgement of a stored deny rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenyRuleCreatedView {
    /// The identifier the rule can be removed by.
    pub deny_rule_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully populated view, so the serialization test exercises every field rather than the ones a
    /// minimal value happens to set.
    fn grant() -> GrantView {
        GrantView {
            grant_id: "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d".to_owned(),
            capability: "clock.now@1".to_owned(),
            principal_id: "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5e".to_owned(),
            workspace_id: "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5f".to_owned(),
            scopes: vec!["clock.read".to_owned()],
            effects: vec!["read_only".to_owned()],
            risk_ceiling: "low".to_owned(),
            sensitivity_ceiling: "public".to_owned(),
            expires_at: Some("2026-10-02T12:00:00Z".to_owned()),
            active: true,
            version: 3,
            granted_by: "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c60".to_owned(),
            created_at: "2026-10-01T12:00:00Z".to_owned(),
            updated_at: "2026-10-01T12:01:00Z".to_owned(),
        }
    }

    #[test]
    fn a_grant_view_round_trips_through_json() {
        let view = grant();
        let encoded = serde_json::to_string(&view).expect("serializes");
        let decoded: GrantView = serde_json::from_str(&encoded).expect("parses");
        assert_eq!(view, decoded);
    }

    #[test]
    fn a_standing_grant_omits_the_expiry_rather_than_sending_a_sentinel() {
        // The absence is the statement, so the field must not appear at all — a `null` would be a third
        // spelling of "never", and a sentinel instant would be a fourth.
        let mut view = grant();
        view.expires_at = None;
        let encoded = serde_json::to_string(&view).expect("serializes");
        assert!(
            !encoded.contains("expires_at"),
            "a standing grant must omit the field: {encoded}",
        );
        let decoded: GrantView = serde_json::from_str(&encoded).expect("parses without the field");
        assert_eq!(decoded.expires_at, None);
    }

    #[test]
    fn a_write_request_refuses_a_field_it_does_not_define() {
        // `deny_unknown_fields` is the difference between a typo being reported and a typo becoming a grant
        // that authorizes less than the operator intended — a misspelled `principle_id` would otherwise be
        // ignored and the request would fail on the *missing* one with a message about a field the operator
        // believes they sent.
        let body = r#"{"capability":"clock.now@1","principal_id":"018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5e","principle_id":"018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c61","risk_ceiling":"low","sensitivity_ceiling":"public"}"#;
        assert!(
            serde_json::from_str::<WriteToolGrantRequest>(body).is_err(),
            "an unknown field must be refused rather than ignored",
        );
    }

    #[test]
    fn a_write_request_needs_no_operator_and_no_version() {
        // The two absences are the point: the operator comes from the authenticated context, and the version
        // belongs to the replace route's own body — so a create cannot set a version and a replace cannot
        // omit one.
        let body = r#"{"capability":"clock.now@1","principal_id":"018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5e","scopes":[],"effects":["read_only"],"risk_ceiling":"low","sensitivity_ceiling":"public"}"#;
        let parsed: WriteToolGrantRequest = serde_json::from_str(body).expect("parses");
        assert!(parsed.expires_at.is_none());
        let encoded = serde_json::to_string(&parsed).expect("serializes");
        assert!(!encoded.contains("granted_by"), "{encoded}");
        assert!(!encoded.contains("expected_version"), "{encoded}");
    }

    #[test]
    fn a_deny_rule_requires_the_workspace_choice() {
        // A required boolean, because a defaulted one would turn an ordinary workspace refusal into a
        // profile-wide one — the direction that widens a restriction's scope without anyone saying so.
        let without = r#"{"capability":"email.send@1","effects":["external_communication"],"reason":"needs review"}"#;
        assert!(serde_json::from_str::<WriteDenyRuleRequest>(without).is_err());
        let with = r#"{"capability":"email.send@1","effects":["external_communication"],"reason":"needs review","workspace_wide":true}"#;
        let parsed: WriteDenyRuleRequest = serde_json::from_str(with).expect("parses");
        assert!(parsed.workspace_wide);
    }
}
