//! Tests for the durable approval record and its lifecycle.
//!
//! The subject that carries the most risk is the **state machine**, and specifically the rule the
//! contract states as "terminal decisions are immutable" and "a one-shot approval is consumed
//! atomically when the exact tool call is reserved; reuse or changed fingerprint fails". Every test
//! below that asserts a refusal names the *other* outcome it is ruling out, because "refused" is the
//! answer a state machine that refused everything would also give.

use super::approval::{
    AllowedChannels, ApprovalActor, ApprovalChannel, ApprovalPreview, ApprovalRequestParts,
    ApprovalScopeKind, ApprovalState, ApprovalSummary, ApprovalVersion, DecisionNote,
    DurableApproval, MAX_APPROVAL_CHANNELS, MAX_DECISION_NOTE_BYTES, MAX_PREVIEW_ITEMS,
    MAX_PREVIEW_TEXT_BYTES, MAX_SUMMARY_BYTES, PreviewItem, is_usable_summary,
};
use super::classification::{Effect, Risk};
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use crate::error::DomainError;
use crate::ids::{PrincipalId, RunId, ToolCallId, WorkspaceId};
use crate::model::exception::RequiredAssurance;
use crate::time::UtcTimestamp;

// ---------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------

fn instant(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).expect("the fixture instant parses")
}

fn now() -> UtcTimestamp {
    instant("2026-09-27T12:00:00Z")
}

fn principal(value: u128) -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::from_u128(value))
}

fn tool_identity() -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::parse("mail.send@1").expect("canonical"),
        source: ToolSource::new(
            SourceKind::Connector,
            "acme.mail",
            ToolVersion::parse("1.0.0").expect("valid"),
        )
        .expect("the fixture source is valid"),
        schema_fingerprint: SchemaFingerprint::from_bytes([3; 32]),
    }
}

/// A one-shot approval request for the mail tool, expiring in ten minutes.
fn parts() -> ApprovalRequestParts {
    ApprovalRequestParts {
        workspace: WorkspaceId::from_uuid(uuid::Uuid::from_u128(1)),
        requesting_principal: principal(1),
        run: RunId::from_uuid(uuid::Uuid::from_u128(2)),
        tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(3)),
        identity: tool_identity(),
        action_digest: super::canonical::ActionDigest::from_bytes([11; 32]),
        risk: Risk::High,
        effects: vec![Effect::ExternalCommunication, Effect::Write],
        summary: ApprovalSummary::new("Send one email to peter@example.com").expect("shaped"),
        preview: ApprovalPreview::new(vec![
            PreviewItem::new("To", "peter@example.com").expect("shaped"),
            PreviewItem::new("Subject", "Following up").expect("shaped"),
        ])
        .expect("the preview is within bounds"),
        allowed_channels: AllowedChannels::new(vec![
            ApprovalChannel::Cli,
            ApprovalChannel::Desktop,
        ])
        .expect("two channels"),
        expires_at: instant("2026-09-27T12:10:00Z"),
        scope: ApprovalScopeKind::OneShot,
    }
}

/// A decision actor, at `Standard` assurance unless a test says otherwise.
///
/// `Standard` is the ordinary verified-credential case; a test that is *about* the recorded assurance
/// passes `Elevated` explicitly so the two are distinguishable, which is the whole reason the field exists.
fn decision_at(channel: ApprovalChannel, assurance: RequiredAssurance) -> ApprovalActor {
    ApprovalActor::Decided {
        principal: principal(1),
        channel,
        assurance,
        note: None,
    }
}

fn decision(channel: ApprovalChannel) -> ApprovalActor {
    decision_at(channel, RequiredAssurance::Standard)
}

/// A decision actor carrying an operator note, so a test can assert the note survives.
fn decision_with_note(channel: ApprovalChannel, note: &str) -> ApprovalActor {
    ApprovalActor::Decided {
        principal: principal(1),
        channel,
        assurance: RequiredAssurance::Standard,
        note: Some(DecisionNote::new(note).expect("a usable note")),
    }
}

/// The field a refusal named, or `None`. `DomainError` is deliberately not `PartialEq`.
fn field_of<T>(result: &Result<T, DomainError>) -> Option<&'static str> {
    match result.as_ref().err() {
        Some(DomainError::ToolDefinitionInvalid { field }) => Some(field),
        _ => None,
    }
}

/// The stable code of a refusal, or `None`.
fn code_of<T>(result: &Result<T, DomainError>) -> Option<&'static str> {
    result.as_ref().err().map(DomainError::code)
}

// ---------------------------------------------------------------------------------------
// The transition table.
// ---------------------------------------------------------------------------------------

#[test]
fn a_fresh_approval_is_pending_at_version_one() {
    // Version one rather than zero, so an uninitialised field cannot read as a valid stored version.
    // And `Pending` rather than a chosen state, because the state is not a parameter — an approval
    // constructed already decided is the one thing an approval must never be.
    let approval = DurableApproval::request(parts());
    assert_eq!(approval.state(), ApprovalState::Pending);
    assert_eq!(approval.version(), ApprovalVersion::FIRST);
    assert!(!approval.is_decided());
    assert!(approval.decided_by().is_none());
    assert!(approval.decided_via().is_none());
}

#[test]
fn the_transition_table_permits_exactly_the_contracts_edges() {
    // **The table asserted cell by cell**, because a state machine tested only through its happy path
    // would accept an extra edge and no test would notice. The expected set is written out rather
    // than derived, so adding an edge to the implementation fails here rather than silently widening
    // what an approval lifecycle permits.
    let legal = [
        (ApprovalState::Pending, ApprovalState::Approved),
        (ApprovalState::Pending, ApprovalState::Rejected),
        (ApprovalState::Pending, ApprovalState::Expired),
        (ApprovalState::Pending, ApprovalState::Cancelled),
        (ApprovalState::Approved, ApprovalState::Consumed),
        (ApprovalState::Approved, ApprovalState::Invalidated),
        (ApprovalState::Approved, ApprovalState::Expired),
        // The revocation edge: a granted permission, above all a standing one, can be withdrawn.
        (ApprovalState::Approved, ApprovalState::Cancelled),
    ];
    let all = [
        ApprovalState::Pending,
        ApprovalState::Approved,
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ];
    for from in all {
        for to in all {
            let expected = legal.contains(&(from, to));
            assert_eq!(
                from.can_transition_to(to),
                expected,
                "{from} -> {to} must be {}",
                if expected { "legal" } else { "illegal" },
            );
        }
    }
}

#[test]
fn every_state_either_absorbs_or_can_be_left() {
    // **The property an approval that could never be decided would violate**, and the one six earlier
    // rounds in this project found missing by hitting it live: a state with no legal exit leaves the
    // record stuck forever. Asserted as an equivalence in both directions over the whole set, so
    // neither a state wrongly classified as terminal nor a non-terminal state with no edges passes.
    let all = [
        ApprovalState::Pending,
        ApprovalState::Approved,
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ];
    for state in all {
        let has_some_edge = all.iter().any(|to| state.can_transition_to(*to));
        assert_eq!(
            has_some_edge,
            !state.is_terminal(),
            "{state}: a non-terminal state must have a legal exit and a terminal one must not, so \
             `is_terminal` and the transition table must agree",
        );
        assert_eq!(state.can_advance(), !state.is_terminal());
        assert_eq!(
            state.has_any_transition(),
            has_some_edge,
            "the consultable predicate must agree with the table it answers from",
        );
    }
    // And Pending is reachable to every decision, so no decision is unreachable — an approval that
    // could be requested but never decided is a request no user can answer.
    for decision in [
        ApprovalState::Approved,
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
    ] {
        assert!(
            ApprovalState::Pending.can_transition_to(decision),
            "a pending approval must be able to reach {decision}",
        );
    }
}

#[test]
fn the_absorbing_rule_is_part_of_the_table_rather_than_a_guard_beside_it() {
    // **The regression test for a defect this round found by mutation.** `can_transition_to` used to
    // begin `if self.is_terminal() { return false; }` and then list the legal edges. Disabling that
    // guard **compiled and passed every test**, because no arm in the table has a terminal source —
    // so the branch was unreachable and enforced nothing while reading as enforcement.
    //
    // The table is what must carry the rule, so this test asserts a terminal source is refused **for
    // every target** through the same call the mutation went through, and that the refusal comes from
    // the table: `has_any_transition` is `false` for every terminal state, which a guard that had been
    // deleted would have answered `true` for.
    let terminals = [
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ];
    let all = [
        ApprovalState::Pending,
        ApprovalState::Approved,
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ];
    for terminal in terminals {
        assert!(
            !terminal.has_any_transition(),
            "{terminal} must report that it has no legal exit — this is the assertion a deleted \
             guard fails, because the guard was answering the question and the table was not",
        );
        for to in all {
            assert!(!terminal.can_transition_to(to), "{terminal} -> {to}");
        }
    }
    // And the non-terminal pair reports that it does have exits, so the assertion above is not
    // satisfied by a predicate that answers `false` for everything.
    assert!(ApprovalState::Pending.has_any_transition());
    assert!(ApprovalState::Approved.has_any_transition());
}

#[test]
fn a_terminal_state_is_absorbing_for_every_target_including_itself() {
    // **The rule the one-shot consumption depends on.** "Terminal decisions are immutable" is a
    // statement about the transition table, and it is asserted against **every** terminal state and
    // every target — including the same state, because `Consumed -> Consumed` would let a second
    // reservation write a transition that looks like progress while the effect stayed single, so the
    // audit trail would claim two spends for one call.
    let terminals = [
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ];
    let all = [
        ApprovalState::Pending,
        ApprovalState::Approved,
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ];
    for terminal in terminals {
        assert!(
            terminal.is_terminal(),
            "{terminal} must classify as terminal"
        );
        for to in all {
            assert!(
                !terminal.can_transition_to(to),
                "{terminal} -> {to} must be refused: a terminal decision is immutable",
            );
        }
    }
    // And the non-terminal pair is classified as such, so the classification is not "everything is
    // terminal", which would refuse every legal edge the test above also asserts.
    assert!(!ApprovalState::Pending.is_terminal());
    assert!(
        !ApprovalState::Approved.is_terminal(),
        "APPROVED is a decision waiting to be spent, not a terminal one — the pair is easy to get \
         backwards",
    );
}

#[test]
fn a_consumed_approval_cannot_be_consumed_again() {
    // The one-shot rule stated as the sequence that matters: approve, consume, then try to consume
    // again. Asserted through `apply` rather than through the table, because the table being right
    // and the method consulting it are different claims.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    let spent = approval
        .apply(
            ApprovalState::Consumed,
            ApprovalVersion::new(2),
            ApprovalActor::Consumed {
                tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(3)),
            },
            now(),
        )
        .expect("the one-shot approval may be spent once");
    assert_eq!(spent.to, ApprovalState::Consumed);
    assert_eq!(spent.prior_version, ApprovalVersion::new(2));

    let again = approval.apply(
        ApprovalState::Consumed,
        ApprovalVersion::new(3),
        ApprovalActor::Consumed {
            tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(3)),
        },
        now(),
    );
    assert_eq!(
        code_of(&again),
        Some("approval.state_conflict"),
        "a second consumption must be refused, or one approval would authorize two sends",
    );
}

#[test]
fn a_rejected_approval_cannot_later_be_approved() {
    // The other terminal-absorption case, and the one a user might plausibly want reversed. It is
    // refused because the decision is the record: a rejection that could be changed would make the
    // approval's history a description of its current state rather than of what was decided.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Rejected,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be refused");
    assert_eq!(
        code_of(&approval.apply(
            ApprovalState::Approved,
            ApprovalVersion::new(2),
            decision(ApprovalChannel::Cli),
            now(),
        )),
        Some("approval.state_conflict"),
    );
}

#[test]
fn a_stale_version_is_refused_before_the_edge_is_considered() {
    // The ordering, which follows `RunLifecycle::apply`'s reasoning: a caller working from a stale
    // view must be told its view is stale before it is told the edge is illegal, because an illegal
    // edge computed from a stale state may be legal from the current one. Asserted with a
    // version that is both stale **and** on an illegal edge, so the reported error distinguishes the
    // two orders.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    // `Pending -> Approved` again is illegal from `Approved`, and the version stated is stale.
    let refusal = approval.apply(
        ApprovalState::Approved,
        ApprovalVersion::FIRST,
        decision(ApprovalChannel::Cli),
        now(),
    );
    assert_eq!(
        code_of(&refusal),
        Some("approval.version_conflict"),
        "a stale view must be reported as a stale view",
    );
}

#[test]
fn a_decision_from_an_unpermitted_channel_is_refused() {
    // The contract lists `allowed_channels` in an approval request, which only means something if the
    // decision is checked against it. Voice is not in this fixture's set, so a voice decision must be
    // refused — otherwise recording the channels would be a field with a reader and no effect.
    let mut approval = DurableApproval::request(parts());
    let refusal = approval.apply(
        ApprovalState::Approved,
        ApprovalVersion::FIRST,
        decision(ApprovalChannel::Voice),
        now(),
    );
    assert_eq!(field_of(&refusal), Some("allowed_channels"));
    // The approval is untouched, because a refused transition must not have written anything.
    assert_eq!(approval.state(), ApprovalState::Pending);
    assert_eq!(approval.version(), ApprovalVersion::FIRST);

    // And a permitted channel succeeds, so the rule refuses the wrong channel rather than all of them.
    assert!(
        approval
            .apply(
                ApprovalState::Approved,
                ApprovalVersion::FIRST,
                decision(ApprovalChannel::Desktop),
                now(),
            )
            .is_ok(),
    );
}

#[test]
fn a_non_decision_transition_is_not_channel_checked() {
    // Expiry and invalidation are caused by time and by a changed action, not by a person, so they
    // carry no channel and must not be refused for lacking one. Asserted because a channel check
    // applied to every transition would make expiry unreachable — an approval that could never lapse
    // would sit `Pending` forever, the "no legal way out" shape this project keeps finding.
    let mut approval = DurableApproval::request(parts());
    assert!(
        approval
            .apply(
                ApprovalState::Expired,
                ApprovalVersion::FIRST,
                ApprovalActor::Expired {
                    expires_at: instant("2026-09-27T12:10:00Z"),
                },
                now(),
            )
            .is_ok(),
    );
    assert_eq!(approval.state(), ApprovalState::Expired);
    assert!(
        !approval.is_decided(),
        "time is not a decider: `decided_by` must stay empty, or the audit trail would claim a \
         person expired an approval late at night",
    );
}

#[test]
fn a_decision_records_who_decided_and_on_which_channel() {
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Desktop),
            now(),
        )
        .expect("the approval may be given");
    assert!(approval.is_decided());
    assert_eq!(approval.decided_by(), Some(principal(1)));
    assert_eq!(approval.decided_via(), Some(ApprovalChannel::Desktop));
}

#[test]
fn a_note_bounds_the_text_it_accepts_and_the_bound_holds_on_the_way_in() {
    // **The type `DecisionNote` exists because the wire's `comment` and the cancellation's `reason` were
    // bounded at one layer and stored at none.** Each asserted here against the rule its constructor
    // enforces, in the shape `wire_validation_tests` established for every other validated newtype in
    // this crate: the valid value round-trips, and the value the constructor refuses is refused by the
    // **deserializer** too.
    assert!(DecisionNote::new("asked Bob, he said no").is_ok());
    // A newline and a tab are legitimate in an operator's note, because the value is never interpreted.
    assert!(DecisionNote::new("line one\nline two\ttabbed").is_ok());
    assert!(
        DecisionNote::new("").is_err(),
        "an empty note is not a note — `None` is how a caller says it gave none",
    );
    assert!(DecisionNote::new(&"x".repeat(MAX_DECISION_NOTE_BYTES + 1)).is_err());
    assert!(
        DecisionNote::new("bell\u{7}here").is_err(),
        "a control character is refused because the note reaches an operator display",
    );

    // The deserializer goes through the constructor, so a note that arrived over the wire cannot carry a
    // control character or exceed the bound — the systemic defect this crate recorded for nine types.
    assert!(serde_json::from_str::<DecisionNote>("\"fine\"").is_ok());
    assert!(serde_json::from_str::<DecisionNote>("\"\"").is_err());
    let over = format!("\"{}\"", "x".repeat(MAX_DECISION_NOTE_BYTES + 1));
    assert!(serde_json::from_str::<DecisionNote>(&over).is_err());
}

#[test]
fn a_decision_note_is_carried_by_the_actor_that_recorded_it() {
    // The note explains **this** transition, so it belongs to the actor rather than to the record: a
    // decision's comment and a cancellation's reason are the same kind of value, and both are reachable
    // through one accessor. Asserted through `apply`, because the actor the transition records is what an
    // adapter stores and what a reader reconstructs.
    let mut approval = DurableApproval::request(parts());
    let transition = approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision_with_note(ApprovalChannel::Cli, "checked the recipient"),
            now(),
        )
        .expect("the approval may be given");
    assert_eq!(
        transition.actor.note().map(DecisionNote::as_str),
        Some("checked the recipient"),
        "**the note must travel on the transition the human authored**",
    );

    // A decision with no note reports none, which is distinct from an empty note (refused above).
    let mut quiet = DurableApproval::request(parts());
    let quiet_transition = quiet
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert!(quiet_transition.actor.note().is_none());

    // And a cancellation carries its own reason through the same accessor.
    let mut withdrawn = DurableApproval::request(parts());
    let cancellation = withdrawn
        .apply(
            ApprovalState::Cancelled,
            ApprovalVersion::FIRST,
            ApprovalActor::Cancelled {
                by: principal(1),
                reason: Some(DecisionNote::new("superseded").expect("a usable note")),
            },
            now(),
        )
        .expect("PENDING -> CANCELLED is legal");
    assert_eq!(
        cancellation.actor.note().map(DecisionNote::as_str),
        Some("superseded"),
        "a cancellation's reason is the same kind of value as a decision's comment",
    );
}

#[test]
fn the_state_round_trips_through_its_contract_spelling() {
    for state in [
        ApprovalState::Pending,
        ApprovalState::Approved,
        ApprovalState::Rejected,
        ApprovalState::Expired,
        ApprovalState::Cancelled,
        ApprovalState::Consumed,
        ApprovalState::Invalidated,
    ] {
        assert_eq!(
            ApprovalState::parse(state.as_contract_str()).ok(),
            Some(state),
        );
    }
    // An unknown stored state must not be read as `Pending`, which is the fail-open direction for a
    // decision: a row whose state column was corrupted would come back as an undecided request
    // rather than as a refusal.
    assert_eq!(field_of(&ApprovalState::parse("granted")), Some("state"));
    assert_eq!(field_of(&ApprovalState::parse("")), Some("state"));
}

// ---------------------------------------------------------------------------------------
// The coverage check: the four conditions that must all hold.
// ---------------------------------------------------------------------------------------

#[test]
fn an_approved_unexpired_approval_covers_its_exact_action() {
    // The positive control for the three refusals below. Without it, each of those would pass against
    // a `covers` that always answered `false`.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert!(approval.covers(&approved_digest(), now()));
}

/// The digest the fixture approval was recorded for.
///
/// A real digest's **shape** rather than a placeholder word, because the type now validates one: a value
/// like `"sha256:the-approved-action"` would have been accepted as text and refused as a digest, which is
/// the change this fixture exists to reflect — the fingerprint is a value a computation produced rather
/// than a string a caller asserted.
fn approved_digest() -> super::canonical::ActionDigest {
    super::canonical::ActionDigest::from_bytes([11; 32])
}

/// A digest for an action the fixture approval does **not** cover.
fn other_digest() -> super::canonical::ActionDigest {
    super::canonical::ActionDigest::from_bytes([12; 32])
}

#[test]
fn an_approved_approval_covers_exactly_its_own_action() {
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert!(approval.covers(&approved_digest(), now()));
}

#[test]
fn a_pending_approval_covers_nothing() {
    // An undecided request must not authorize: the state is checked, so "the record exists" is not
    // the same as "the action was approved". A `covers` that checked only the digest would allow a
    // call whose approval had never been given.
    let approval = DurableApproval::request(parts());
    assert!(!approval.covers(&approved_digest(), now()));
}

#[test]
fn an_approved_approval_for_a_different_action_covers_nothing() {
    // The contract: "approving 'send this email' does not approve a rewritten recipient, subject,
    // body, attachment, or account." The digest is the mechanism, and it is compared after the state
    // so the two refusals are driven by different conditions.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert!(!approval.covers(&other_digest(), now()));
}

#[test]
fn a_lapsed_approval_covers_nothing_even_though_its_state_is_still_approved() {
    // **The subtle one, and the reason lapse is a computation rather than only a state.** An approval
    // that lapsed while nobody was looking still has `Approved` in its state column — nothing ran to
    // transition it — so a check that trusted the stored state alone would honour a lapsed approval.
    // The expiry is compared at use, which is the only moment that can be authoritative.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert_eq!(
        approval.state(),
        ApprovalState::Approved,
        "nothing transitioned it, which is exactly the case that has to be caught at use",
    );
    // One second before expiry it still covers; at expiry it does not. The boundary is the same
    // convention as an expiry instant elsewhere in this project: an expiry at `T` does not permit
    // work at `T`.
    assert!(approval.covers(&approved_digest(), instant("2026-09-27T12:09:59Z")));
    assert!(
        !approval.covers(&approved_digest(), instant("2026-09-27T12:10:00Z")),
        "an approval lapsed exactly at its expiry must not cover the action",
    );
    assert!(!approval.covers(&approved_digest(), instant("2026-09-27T13:00:00Z")));
}

#[test]
fn a_consumed_approval_no_longer_covers_its_action() {
    // The one-shot semantics at the coverage check, which is where the executor asks: an approval
    // that has been spent must stop covering immediately, or the reservation's own check would be
    // the only thing standing between one approval and two sends.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert!(approval.covers(&approved_digest(), now()));
    approval
        .apply(
            ApprovalState::Consumed,
            ApprovalVersion::new(2),
            ApprovalActor::Consumed {
                tool_call: ToolCallId::from_uuid(uuid::Uuid::from_u128(3)),
            },
            now(),
        )
        .expect("the approval may be spent");
    assert!(!approval.covers(&approved_digest(), now()));
}

#[test]
fn a_standing_approval_is_not_spent_by_use_but_a_one_shot_is() {
    // The contract's "one exact action **or** a narrowly defined standing rule". The distinction is
    // what the executor consults to decide whether to consume, so it is stated as a predicate rather
    // than left to a caller to infer from the scope name.
    assert!(ApprovalScopeKind::OneShot.is_consumed_on_use());
    assert!(!ApprovalScopeKind::Standing.is_consumed_on_use());
}

// ---------------------------------------------------------------------------------------
// Bounds and shapes.
// ---------------------------------------------------------------------------------------

#[test]
fn an_approval_with_no_permitted_channel_is_unrepresentable() {
    // **An approval with no channel could never be decided**, so it would sit `Pending` until it
    // expired — the "no legal way out" shape. Refusing the empty set at construction makes that
    // state unrepresentable rather than merely unlikely.
    assert_eq!(
        field_of(&AllowedChannels::new(Vec::new())),
        Some("allowed_channels"),
    );
    let too_many = vec![ApprovalChannel::Cli; MAX_APPROVAL_CHANNELS + 1];
    assert_eq!(
        field_of(&AllowedChannels::new(too_many)),
        Some("allowed_channels"),
    );
    let ok = AllowedChannels::new(vec![ApprovalChannel::Api]).expect("one channel is enough");
    assert!(ok.permits(ApprovalChannel::Api));
    assert!(!ok.permits(ApprovalChannel::Cli));
}

#[test]
fn a_preview_item_refuses_control_characters_and_unbounded_text() {
    // A preview is rendered inside a prompt a user reads to decide, so it is the one place where a
    // crafted string could make the prompt misrepresent the action. Refused at construction rather
    // than escaped at render time, because escape-at-render is a rule each renderer must remember.
    assert!(PreviewItem::new("To", "peter@example.com").is_ok());
    for (key, value) in [
        ("", "value"),
        ("key", ""),
        ("Subject", "line one\u{1b}[31mred"),
        ("Subject", "line\none"),
        (&"k".repeat(MAX_PREVIEW_TEXT_BYTES + 1), "value"),
        ("key", &"v".repeat(MAX_PREVIEW_TEXT_BYTES + 1)),
    ] {
        assert_eq!(
            field_of(&PreviewItem::new(key, value)),
            Some("preview"),
            "({key:?}, {value:?}) must be refused",
        );
    }
}

#[test]
fn a_preview_may_be_empty_because_that_is_a_policy_question() {
    // Whether a preview can be empty is about the *tool* rather than a domain invariant: a read-only
    // tool genuinely may have nothing to show. Refusing it here would make a reviewable decision into
    // a compile-time argument.
    let empty = ApprovalPreview::new(Vec::new()).expect("an empty preview is allowed");
    assert!(empty.is_empty());
    assert!(empty.items().is_empty());

    let too_many = (0..=MAX_PREVIEW_ITEMS)
        .map(|index| PreviewItem::new("k", &format!("v{index}")).expect("shaped"))
        .collect();
    assert_eq!(
        field_of(&ApprovalPreview::new(too_many)),
        Some("preview"),
        "the item count is bounded even though emptiness is not",
    );
}

#[test]
fn a_summary_is_bounded_and_control_free() {
    assert!(is_usable_summary("Send one email to peter@example.com"));
    assert!(!is_usable_summary(""));
    assert!(!is_usable_summary("send\u{7}bell"));
    assert!(!is_usable_summary(&"a".repeat(MAX_SUMMARY_BYTES + 1)));
    assert!(is_usable_summary(&"a".repeat(MAX_SUMMARY_BYTES)));

    // The type is now the enforcement point, so the same rules hold through it and the text round-trips.
    let summary = ApprovalSummary::new("Send one email").expect("shaped");
    assert_eq!(summary.as_str(), "Send one email");
    assert_eq!(summary.to_string(), "Send one email");
    assert_eq!(
        field_of(&ApprovalSummary::new("")),
        Some("summary"),
        "an empty summary is refused by the type a record now carries",
    );
}

#[test]
fn a_channel_parses_only_from_its_contract_spelling() {
    for channel in [
        ApprovalChannel::Cli,
        ApprovalChannel::Desktop,
        ApprovalChannel::Mobile,
        ApprovalChannel::Voice,
        ApprovalChannel::Api,
    ] {
        assert_eq!(
            ApprovalChannel::parse(channel.as_contract_str()).ok(),
            Some(channel),
        );
    }
    // A channel JARVIS cannot verify is refused rather than carried, because the whole point of
    // recording the channel is to check where a decision came from.
    for value in ["telegram", "CLI", "", "email"] {
        assert_eq!(
            field_of(&ApprovalChannel::parse(value)),
            Some("allowed_channels"),
            "{value:?} must be refused",
        );
    }
}

#[test]
fn an_approval_survives_a_json_round_trip() {
    // Approvals are durable records, so the wire form is contract surface. The state and version are
    // private but serialized, so the round trip is what proves a stored record re-reads into the same
    // lifecycle position rather than into a fresh pending one.
    let mut approval = DurableApproval::request(parts());
    approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Desktop),
            now(),
        )
        .expect("the approval may be given");
    let json = serde_json::to_string(&approval).expect("an approval serializes");
    let restored: DurableApproval = serde_json::from_str(&json).expect("an approval deserializes");
    assert_eq!(restored, approval);
    assert_eq!(
        restored.state(),
        ApprovalState::Approved,
        "a re-read record must land in the state it was stored in, not in a fresh PENDING",
    );
    assert_eq!(restored.version(), ApprovalVersion::new(2));
    assert!(restored.is_decided());
    // And the actor tag is on the wire, so a stored transition says what kind of actor it had.
    let actor_json = serde_json::to_string(&decision(ApprovalChannel::Cli)).expect("serializes");
    assert!(actor_json.contains(r#""kind":"decided""#), "{actor_json}");
}

#[test]
fn the_transition_record_names_both_states_and_both_versions() {
    // A record is written to the audit trail from this, so it must state the *applied* versions
    // rather than the predicted ones — the caller's expectation and the outcome are different facts.
    let mut approval = DurableApproval::request(parts());
    let record = approval
        .apply(
            ApprovalState::Approved,
            ApprovalVersion::FIRST,
            decision(ApprovalChannel::Cli),
            now(),
        )
        .expect("the approval may be given");
    assert_eq!(record.id, approval.id);
    assert_eq!(record.from, ApprovalState::Pending);
    assert_eq!(record.to, ApprovalState::Approved);
    assert_eq!(record.prior_version, ApprovalVersion::FIRST);
    assert_eq!(record.version, ApprovalVersion::new(2));
    assert_eq!(record.occurred_at, now());
    assert!(record.to_string().contains("pending -> approved"));
}
