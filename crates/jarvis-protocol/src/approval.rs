//! Approval wire types.
//!
//! `docs/contracts/approval-contract.md` defines the approval record a client reads from
//! `GET /api/v1/approvals` and decides through `POST /api/v1/approvals/{id}/decide`. These are the
//! versioned shapes, and they live in `jarvis-protocol` rather than in the HTTP adapter so the
//! contract has one serialization definition a client, the daemon, and a fixture test share.
//!
//! Three decisions are deliberate, and all three follow the same rule the policy surface set: **the
//! wire value is the contract's own spelling, not the domain's `Display`.**
//!
//! - **`state`, `risk`, `effects`, `channels`, and `scope` are the contract's lowercase spellings.**
//!   The domain names a variant `ApprovalState::Pending` and the contract writes `pending`; the same
//!   value must not have two spellings depending on which layer serialized it. A test asserts every
//!   variant's wire form, which is what caught the two-spellings defect on the ledger's state enum.
//! - **The preview is a list of key/value rows, and the fingerprint is a string.** The contract
//!   requires detail to return "the immutable action fingerprint inputs needed for informed review as
//!   a bounded, schema-defined, redacted preview", so the preview travels structurally rather than as
//!   rendered prose — a client that received only a sentence could not redact a field or lay it out.
//! - **The tool's source and schema identity travel beside the capability, in their own fields.** The
//!   contract's detail requirement names "tool source/**schema identity**", and `ACC-024` is the rule
//!   behind it: an approval binds to the implementation rather than to a name that can be re-pointed, so
//!   a client shown only `mail.send@1` cannot tell that the tool behind it was replaced. Four fields
//!   rather than one joined string so a reviewer can see *which* dimension changed.
//! - **A decision body carries no principal, channel, assurance, or time.** The contract says the
//!   server derives them and "a client cannot assert them in the body", so the type has no field for
//!   them: a caller-supplied assurance would let whoever filled in the body choose the step-up rule,
//!   which is the defect `BRN-024` fixed for policy grants.
//!
//! **No field names a hidden argument.** The contract forbids detail from returning "hidden tool
//! arguments, credentials, or content excluded from the fingerprint", and the way to guarantee that
//! is for this module to have no field through which one could arrive — the same structural argument
//! the model-gateway types make about provider accounts.

use serde::{Deserialize, Serialize};

/// The largest page a list request may ask for.
///
/// A bound rather than a constant a caller trusts, because the contract requires "a bounded page
/// size" and an unbounded list over a workspace's approvals is the shape that turns one request into
/// a full table scan. The handler clamps to it rather than refusing, so a client asking for more
/// receives a full page and the server's own limit — the same choice the run event page makes.
///
/// **⚠ The same bound is declared again in `jarvis_application::repository::approval` as
/// `MAX_PENDING_PAGE`, and *nothing compares them*.** This crate may not depend on
/// `jarvis-application` (the documented flow is `Protocol --> Domain`, and application is above
/// protocol), and application may not depend on this crate, so neither can see the other's value —
/// editing one alone would leave the store clamping a page to one number while this module reports
/// `max_page` as another, so a client would be told to page by a number the daemon would not honour.
/// The comparison is therefore asserted in `jarvis-infrastructure`, the one crate that depends on
/// both: `the_reported_page_bound_is_the_one_the_store_enforces` in `http`'s tests. That is the same
/// arrangement `MAX_RUN_INPUT_BYTES`/`MAX_OBJECTIVE_BYTES` uses, and for the same reason.
pub const MAX_APPROVAL_PAGE: u32 = 200;

/// One approval as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalView {
    /// The approval's own identifier.
    pub approval_id: String,
    /// The workspace the action happens in.
    pub workspace_id: String,
    /// Who asked.
    pub requesting_principal_id: String,
    /// The run that asked.
    pub run_id: String,
    /// The tool call that asked.
    pub tool_call_id: String,
    /// The canonical tool identity, `namespace.name@major`.
    ///
    /// **This is the capability only, and the source and schema travel separately.** The contract's
    /// detail requirement names "tool source/**schema identity**", and the rule it protects is
    /// `ACC-024`: a grant or approval binds to the *implementation*, not to a name that can be
    /// re-pointed. A client shown only `mail.send@1` could not tell that the tool behind it was
    /// replaced — which is exactly the review decision the contract says detail exists to support.
    /// Three fields rather than one pre-joined string so a client can compare the dimension that
    /// changed instead of rendering a tuple it cannot take apart.
    pub tool_id: String,
    /// The tool's source kind, `native`/`connector`/`mcp_server`/`runtime`/`plugin`.
    pub tool_source_kind: String,
    /// The tool's publisher, for example `google.gmail` or an MCP server name.
    pub tool_source_owner: String,
    /// The source's own version, `major.minor.patch`.
    pub tool_source_version: String,
    /// The SHA-256 fingerprint over the tool's input schema, `sha256:<hex>`.
    ///
    /// The contract requires approvals to "bind the actual schema fingerprint", and a reviewer who
    /// cannot see it cannot tell that the input schema changed since the approval was requested —
    /// which is the change that makes a re-approval necessary rather than a repeat decision.
    pub schema_fingerprint: String,
    /// A digest over the exact action.
    pub action_fingerprint: String,
    /// The tool's risk at the time of asking.
    pub risk: String,
    /// The tool's effects at the time of asking.
    pub effects: Vec<String>,
    /// A one-line summary of the action.
    pub summary: String,
    /// The structured, bounded, redacted preview.
    pub preview: Vec<PreviewRowView>,
    /// Which channels may decide it.
    pub allowed_channels: Vec<String>,
    /// When the request stops being decidable.
    pub expires_at: String,
    /// Whether it authorizes one call or a pattern.
    pub scope: String,
    /// The current state.
    pub state: String,
    /// The version a write must state to succeed.
    pub version: u64,
    /// Who decided it, once a decision exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// Which channel the decision came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_via: Option<String>,
    /// The assurance the decider **proved**, from the server-resolved context.
    ///
    /// The contract's audit section requires "assurance" recorded, and it is the fact that distinguishes a
    /// decision by a stepped-up caller from one by an ordinary session — the channel says *where* the
    /// decision was made while this says how strongly the caller was authenticated. `None` means **not
    /// recorded**, which is what a decision taken before the column existed says; it is deliberately not
    /// reported as `standard`, because that would assert a credential strength nobody established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_assurance: Option<String>,
    /// When the decision was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    /// The operator's own note on the decision, when one was given.
    ///
    /// **Present on a detail read and absent from a listing row, and that is the contract's own split
    /// rather than an inconsistency.** The list is the queue an operator works through; detail is where
    /// "prior decision metadata" belongs. The value lives on the decision's *transition* rather than as a
    /// column, so a listing would need one trail read per row to include it — which is the N+1 the split
    /// exists to avoid, and the reason the two routes legitimately differ here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_note: Option<String>,
    /// Whether the request has lapsed against the daemon's clock.
    ///
    /// **Separate from `state` on purpose.** The contract requires expiry to be "evaluated on every
    /// read", and a record that lapsed while nobody was looking still has `state: "pending"` until a
    /// transition records the expiry. A client that trusted only the state would offer a prompt that
    /// can no longer be decided, so the computed fact travels beside the stored one rather than
    /// replacing it.
    pub lapsed: bool,
}

/// One preview row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRowView {
    /// The field being shown.
    pub key: String,
    /// The value to show, already redacted by the producer.
    pub value: String,
}

/// The reply to `GET /api/v1/approvals`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalListView {
    /// The approvals in this page, in the daemon's own order (soonest deadline first).
    pub approvals: Vec<ApprovalView>,
    /// The daemon's own page bound, so a client can page without guessing it.
    pub max_page: u32,
    /// Whether the daemon stopped at its page bound with more approvals unread.
    ///
    /// **Present because a short list and a complete one are different answers**, and a client that
    /// cannot tell them apart concludes there is nothing left to decide and stops.
    pub has_more: bool,
    /// The opaque cursor to pass back as `?cursor=` for the next page, when more remain.
    ///
    /// **`has_more` without this was a dead end**: the daemon told a client that more prompts awaited a
    /// decision and gave it no way to fetch one — the worst of both, because the client knows work
    /// remains and cannot do it. Omitted when nothing follows, so "no cursor" and "nothing after this"
    /// are one fact rather than a cursor that points at nothing.
    ///
    /// **Opaque is a promise about use, not a secrecy claim.** A client may pass it back and nothing
    /// else; a cursor it constructs would name a position the listing never produced, which is how one
    /// skips approvals rather than merely reading them oddly. Everything it carries is data the client
    /// already received, and the store re-applies its own workspace and channel predicates regardless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The reply to a decision or cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecisionResponse {
    /// The approval in its state after the write.
    pub approval: ApprovalView,
    /// Whether this call performed the transition, as opposed to finding it already applied.
    ///
    /// The contract requires that "same-key/same-request retry returns the original decision" and
    /// that "idempotent repeats return current state", so a caller must be able to tell the two
    /// apart. A repeat is not an error — a user double-tapping "approve" sends two requests — but
    /// reporting a repeat as a fresh decision would claim an event that did not happen.
    pub applied: bool,
}

/// A decision body.
///
/// `decision` is `approve` or `reject`; the two are a closed set because the contract says "allowed
/// values are `approve` and `reject`", so an unknown verb is a parse failure rather than a value
/// passed along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideApprovalRequest {
    /// `approve` or `reject`.
    pub decision: String,
    /// The version the caller believes is current.
    pub expected_version: u64,
    /// The fingerprint the caller reviewed.
    pub action_fingerprint: String,
    /// An optional operator comment. Stored with the decision, bounded, and never interpreted.
    ///
    /// **This doc said "stored with the decision" while nothing stored it**, which made it a claim about a
    /// capability that did not exist: the field was deserialized and dropped. It now travels to
    /// `ApprovalActor::Decided`'s note, so the sentence is true and the contract's "changed ... comment is
    /// `idempotency.conflict`" rule has a value to compare rather than nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// A cancellation body.
///
/// **`expected_version` and a reason rather than only the version**, because the contract says the
/// requesting principal cancels "using `expected_version`, reason code, and `Idempotency-Key`" — and
/// a reason is what makes the audit trail say why a prompt was withdrawn rather than only that it
/// was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelApprovalRequest {
    /// The version the caller believes is current.
    pub expected_version: u64,
    /// Why it was withdrawn. Bounded; a caller's own text, never interpreted.
    ///
    /// Stored on the cancellation transition, so the audit trail says *why* a prompt was withdrawn and not
    /// only that it was — which is what this type's own doc claimed while the value was dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
