//! Tests for the tool-call ledger and its execution state machine.
//!
//! The subject that matters most is `ACC-025`: *"Crash after the fake provider accepts an effect but
//! before result persistence. Recovery enters reconciliation and proves no duplicate effect."* The
//! tests below state that scenario directly, because the failure it guards against is a second
//! effect — a second email, a second payment — and every rule in the module exists to make that
//! unrepresentable rather than merely unlikely.
//!
//! Two test-writing decisions are deliberate. The transition table is asserted **cell by cell**
//! against a set written out by hand, because a test that derived its expectation from the
//! implementation could not catch a widened table (which is how `TLS-005` found its own grouping
//! mistake). And each retry decision is asserted with the **other** answer named, because "retryable"
//! is what an implementation that always said `true` would also return.

use super::error_class::ToolErrorClass;
use super::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use super::ledger::{
    InterruptedCallAction, LedgerEntry, LedgerOperation, MAX_IDEMPOTENCY_KEY_BYTES, ReservationKey,
    ReservationOutcome, ToolCallLedger, ToolCallState, ToolCallVersion, classify_interrupted,
};
use crate::error::DomainError;
use crate::ids::{PrincipalId, RunId, ToolCallId, WorkspaceId};
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

fn workspace(value: u128) -> WorkspaceId {
    WorkspaceId::from_uuid(uuid::Uuid::from_u128(value))
}

fn call_id(value: u128) -> ToolCallId {
    ToolCallId::from_uuid(uuid::Uuid::from_u128(value))
}

fn run_id() -> RunId {
    RunId::from_uuid(uuid::Uuid::from_u128(9))
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
        schema_fingerprint: SchemaFingerprint::from_bytes([5; 32]),
    }
}

fn key(idempotency_key: &str) -> ReservationKey {
    ReservationKey::new(tool_identity(), workspace(1), principal(1), idempotency_key)
        .expect("the fixture key is valid")
}

fn entry(idempotency_key: &str) -> LedgerEntry {
    LedgerEntry::reserve(
        call_id(1),
        1,
        run_id(),
        key(idempotency_key),
        LedgerOperation::Execute,
        now(),
    )
}

/// Drives a fresh ledger entry to `to` through the legal intermediate states.
///
/// A helper rather than a direct `apply`, because the states before the target must be reached
/// legally — the machine refuses a jump, which is the point — so a test that wants to be in
/// `EXECUTING` has to walk there. The path is `REQUESTED -> VALIDATED -> APPROVED -> RESERVED ->
/// EXECUTING`, which is the contract's own sequence for a call that needed an approval.
fn drive_to(entry: &mut LedgerEntry, to: ToolCallState) {
    let path: &[ToolCallState] = match to {
        ToolCallState::Requested => &[],
        ToolCallState::Validated => &[ToolCallState::Validated],
        ToolCallState::WaitingApproval => {
            &[ToolCallState::Validated, ToolCallState::WaitingApproval]
        }
        ToolCallState::Denied | ToolCallState::Cancelled => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Denied,
        ],
        ToolCallState::Approved => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Approved,
        ],
        ToolCallState::Reserved => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Approved,
            ToolCallState::Reserved,
        ],
        ToolCallState::Executing => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Approved,
            ToolCallState::Reserved,
            ToolCallState::Executing,
        ],
        ToolCallState::Reconciling => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Approved,
            ToolCallState::Reserved,
            ToolCallState::Executing,
            ToolCallState::Reconciling,
        ],
        ToolCallState::Succeeded => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Approved,
            ToolCallState::Reserved,
            ToolCallState::Executing,
            ToolCallState::Succeeded,
        ],
        ToolCallState::Failed => &[
            ToolCallState::Validated,
            ToolCallState::WaitingApproval,
            ToolCallState::Approved,
            ToolCallState::Reserved,
            ToolCallState::Executing,
            ToolCallState::Failed,
        ],
    };
    for step in path {
        let version = entry.version();
        let result = entry.apply(*step, version, None, now());
        assert!(
            result.is_ok(),
            "the path to {to} must be legal; failed at {step}: {:?}",
            result.err().map(|error| error.code()),
        );
    }
    assert_eq!(
        entry.state(),
        to,
        "the helper must land in the requested state"
    );
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
fn every_state_round_trips_through_its_contract_spelling_and_the_set_is_eleven() {
    for state in ToolCallState::ALL {
        assert_eq!(
            ToolCallState::parse(state.as_contract_str()).ok(),
            Some(*state),
        );
    }
    assert_eq!(
        ToolCallState::ALL.len(),
        11,
        "the contract lists eleven states"
    );
    // An unrecognised stored state must not be read as `Requested`, which is the fail-open direction:
    // a corrupted state column would look like a fresh request rather than as something to inspect.
    assert_eq!(
        field_of(&ToolCallState::parse("no_such_state")),
        Some("state")
    );
    assert_eq!(field_of(&ToolCallState::parse("")), Some("state"));
    // The uppercase prose the contract writes is NOT a stored spelling, and refusing it is deliberate:
    // accepting both would restore the second spelling this method exists to eliminate. The contract's
    // own JSON examples are lowercase, so a stored value is the lowercase form.
    assert_eq!(
        field_of(&ToolCallState::parse("EXECUTING")),
        Some("state"),
        "the contract's prose spelling is not a stored value",
    );
}

#[test]
fn every_state_serializes_to_its_own_spelling() {
    // **The regression test for two spellings of one value.** `as_contract_str` returned the
    // contract's uppercase prose while the serde derive produced `snake_case`, so a state had one
    // spelling to parse and another to store — the "two spellings denote one value" defect this
    // project refuses for identifiers, in the enum that records whether a side effect happened.
    //
    // The assertion compares the two forms for **every** variant, so neither can drift from the other;
    // a single-variant check would have passed for the states whose names need no underscore.
    for state in ToolCallState::ALL {
        let serialized = serde_json::to_string(state).expect("a state serializes");
        let expected = format!("\"{}\"", state.as_contract_str());
        assert_eq!(
            serialized, expected,
            "{state} must serialize to the same spelling it parses from",
        );
        let restored: ToolCallState =
            serde_json::from_str(&serialized).expect("a state deserializes");
        assert_eq!(restored, *state);
    }
    // Named explicitly for the two variants where the forms would have differed most.
    assert_eq!(ToolCallState::Executing.as_contract_str(), "executing");
    assert_eq!(
        ToolCallState::WaitingApproval.as_contract_str(),
        "waiting_approval",
    );
}

#[test]
fn the_transition_table_permits_exactly_the_contracts_edges() {
    // **Written out by hand rather than derived**, for the reason `TLS-005` established: a test that
    // derives its expectation from the implementation cannot catch a widened table. Thirteen edges
    // against a hundred and twenty-one pairs.
    let legal = [
        (ToolCallState::Requested, ToolCallState::Validated),
        (ToolCallState::Requested, ToolCallState::Denied),
        (ToolCallState::Requested, ToolCallState::WaitingApproval),
        (ToolCallState::Requested, ToolCallState::Cancelled),
        (ToolCallState::Validated, ToolCallState::Denied),
        (ToolCallState::Validated, ToolCallState::WaitingApproval),
        (ToolCallState::Validated, ToolCallState::Approved),
        (ToolCallState::Validated, ToolCallState::Reserved),
        (ToolCallState::Validated, ToolCallState::Cancelled),
        (ToolCallState::WaitingApproval, ToolCallState::Approved),
        (ToolCallState::WaitingApproval, ToolCallState::Denied),
        (ToolCallState::WaitingApproval, ToolCallState::Cancelled),
        (ToolCallState::Approved, ToolCallState::Reserved),
        (ToolCallState::Approved, ToolCallState::Cancelled),
        (ToolCallState::Reserved, ToolCallState::Executing),
        (ToolCallState::Reserved, ToolCallState::Cancelled),
        (ToolCallState::Executing, ToolCallState::Succeeded),
        (ToolCallState::Executing, ToolCallState::Failed),
        (ToolCallState::Executing, ToolCallState::Reconciling),
        (ToolCallState::Executing, ToolCallState::Cancelled),
        (ToolCallState::Reconciling, ToolCallState::Succeeded),
        (ToolCallState::Reconciling, ToolCallState::Failed),
    ];
    for from in ToolCallState::ALL {
        for to in ToolCallState::ALL {
            let expected = legal.contains(&(*from, *to));
            assert_eq!(
                from.can_transition_to(*to),
                expected,
                "{from} -> {to} must be {}",
                if expected { "legal" } else { "illegal" },
            );
        }
    }
    assert_eq!(legal.len(), 22, "the table has twenty-two edges");
}

#[test]
fn every_state_either_absorbs_or_can_be_left_and_the_classification_agrees() {
    // The property that keeps a call from sticking with no way out — the shape this project has found
    // seven times. Asserted in both directions so neither a terminal state with an exit nor a
    // non-terminal state without one passes.
    for state in ToolCallState::ALL {
        let has_some_edge = ToolCallState::ALL
            .iter()
            .any(|to| state.can_transition_to(*to));
        assert_eq!(
            has_some_edge,
            !state.is_terminal(),
            "{state}: `is_terminal` and the table must agree",
        );
        assert_eq!(state.has_any_transition(), has_some_edge);
    }
}

#[test]
fn reconciliation_is_not_terminal_and_is_its_own_state() {
    // **The distinction the module exists for.** A call whose outcome is unknown is not finished:
    // treating it as terminal would either lose a successful effect from the record or leave a
    // failed one looking outstanding. And `RECONCILING` is a *separate* state from `EXECUTING`,
    // because "is running" and "may or may not have happened" call for different actions.
    assert!(!ToolCallState::Reconciling.is_terminal());
    assert!(ToolCallState::Reconciling.is_unsettled());
    assert!(ToolCallState::Reconciling.was_dispatched());
    assert!(!ToolCallState::Executing.is_unsettled());
    assert!(ToolCallState::Executing.was_dispatched());
}

#[test]
fn a_call_can_only_be_dispatched_after_a_reservation() {
    // `Approved -> Executing` is refused, so the reservation cannot be skipped. A caller holding an
    // approval must still reserve, which is what makes the reservation the serialization point: two
    // callers holding one approval both reach `Reserved` and only one is granted.
    assert!(!ToolCallState::Approved.can_transition_to(ToolCallState::Executing));
    assert!(ToolCallState::Approved.can_transition_to(ToolCallState::Reserved));
    assert!(ToolCallState::Reserved.can_transition_to(ToolCallState::Executing));
    assert!(
        !ToolCallState::Validated.can_transition_to(ToolCallState::Executing),
        "a validated call has no approval and must not dispatch",
    );
}

#[test]
fn re_entering_execution_is_refused() {
    // A second dispatch is precisely what the reservation prevents, so `EXECUTING -> EXECUTING` must
    // not be a legal edge — accepting it would let an executor record a second dispatch while the
    // reservation still looked held, so the audit trail would claim two effects for one reservation.
    assert!(!ToolCallState::Executing.can_transition_to(ToolCallState::Executing));
    assert!(!ToolCallState::Reconciling.can_transition_to(ToolCallState::Executing));
}

#[test]
fn an_unknown_outcome_must_go_through_reconciliation_rather_than_straight_to_failure() {
    // **The contract's rule**: "an unknown outcome uses `RECONCILING`, not automatic retry". Recording
    // it as failed instead would be the mistake the state machine exists to prevent, because a failed
    // call looks retryable and the retry would duplicate the effect that may already have landed.
    // Asserted by requiring reconciliation to be reachable and by the retry classification below.
    assert!(ToolCallState::Executing.can_transition_to(ToolCallState::Reconciling));
    // `Failed` is reachable too — a *known* failure must be recordable — so the rule is not "failure is
    // forbidden from EXECUTING" but "the unknown case has a state of its own to use".
    assert!(ToolCallState::Executing.can_transition_to(ToolCallState::Failed));
}

// ---------------------------------------------------------------------------------------
// Applying transitions.
// ---------------------------------------------------------------------------------------

#[test]
fn a_reserved_row_starts_requested_at_version_one() {
    let entry = entry("key-1");
    assert_eq!(entry.state(), ToolCallState::Requested);
    assert_eq!(entry.version(), ToolCallVersion::FIRST);
    assert!(entry.outcome().is_none());
    assert!(!entry.may_have_effected(), "nothing has been dispatched");
    assert_eq!(entry.attempt(), 1);
}

#[test]
fn a_stale_version_is_refused_before_the_edge_is_considered() {
    let mut entry = entry("key-1");
    drive_to(&mut entry, ToolCallState::Validated);
    // Reaching `Requested` is illegal from `Validated`, and the version given is stale. The reported
    // reason must be the version, because a stale view must be corrected before an edge is judged.
    let refusal = entry.apply(
        ToolCallState::Requested,
        ToolCallVersion::FIRST,
        None,
        now(),
    );
    assert_eq!(code_of(&refusal), Some("tool.version_conflict"));
}

#[test]
fn an_illegal_edge_is_refused_and_writes_nothing() {
    let mut entry = entry("key-1");
    let refusal = entry.apply(
        ToolCallState::Succeeded,
        ToolCallVersion::FIRST,
        Some(ToolErrorClass::ProviderError),
        now(),
    );
    assert_eq!(code_of(&refusal), Some("tool.state_conflict"));
    assert_eq!(entry.state(), ToolCallState::Requested);
    assert_eq!(entry.version(), ToolCallVersion::FIRST);
}

#[test]
fn a_terminal_transition_records_the_outcome_and_stops_the_call_being_an_effect_risk() {
    let mut entry = entry("key-1");
    drive_to(&mut entry, ToolCallState::Executing);
    assert!(entry.may_have_effected(), "it has been dispatched");
    entry
        .apply(
            ToolCallState::Failed,
            entry.version(),
            Some(ToolErrorClass::ProviderError),
            now(),
        )
        .expect("a known failure is recordable");
    assert_eq!(entry.outcome(), Some(ToolErrorClass::ProviderError));
    assert!(
        !entry.no_effect_confirmed(),
        "a dispatched call that failed has NOT been proven to have had no effect, so a retry would \
         need reconciliation first",
    );
    assert!(
        entry.may_have_effected(),
        "a failed dispatched call may still have taken effect; only reconciliation can say otherwise",
    );
}

#[test]
fn a_refusal_before_dispatch_confirms_no_effect_but_a_failure_after_it_does_not() {
    // **The distinction the whole retry story rests on**, asserted as a pair over two calls whose only
    // difference is where they were refused. A call refused before dispatch cannot have had an effect,
    // so a retry is safe; one that failed after dispatch may have, so it is not. A single-assertion
    // test could not show the difference, and the failure of this pair would be a duplicate effect.
    let mut before = entry("key-before");
    drive_to(&mut before, ToolCallState::Validated);
    before
        .apply(
            ToolCallState::Denied,
            before.version(),
            Some(ToolErrorClass::PermissionDenied),
            now(),
        )
        .expect("a denial is recordable");
    assert!(
        before.no_effect_confirmed(),
        "a denial before dispatch proves no effect happened",
    );
    assert!(!before.may_have_effected());

    let mut after = entry("key-after");
    drive_to(&mut after, ToolCallState::Executing);
    after
        .apply(
            ToolCallState::Failed,
            after.version(),
            Some(ToolErrorClass::ProviderError),
            now(),
        )
        .expect("a failure is recordable");
    assert!(
        !after.no_effect_confirmed(),
        "a failure AFTER dispatch proves nothing about the effect",
    );
    assert!(after.may_have_effected());
}

#[test]
fn confirming_no_effect_ends_the_call_and_never_un_confirms_a_pre_dispatch_refusal() {
    // Two properties of the corrected reconciliation, both of which the first version got wrong:
    //
    // 1. It is a **transition**, so a reconciled row stops looking unsettled. Leaving it in
    //    `RECONCILING` meant a duplicate was reported as needing reconciliation after it had happened.
    // 2. The claim is **sticky**. `apply` ORs it rather than assigning, so a later terminal transition
    //    cannot clear a proof that an effect never happened. Asserted over a pre-dispatch refusal,
    //    which is the case where the claim was established by the transition rather than by a provider
    //    read — clearing it there would let a retry be refused for a call that provably did nothing.
    let mut pre_dispatch = entry("key-refused");
    drive_to(&mut pre_dispatch, ToolCallState::Validated);
    pre_dispatch
        .apply(
            ToolCallState::Denied,
            pre_dispatch.version(),
            Some(ToolErrorClass::PermissionDenied),
            now(),
        )
        .expect("a denial is recordable");
    assert!(pre_dispatch.no_effect_confirmed());
    assert!(!pre_dispatch.was_dispatched());
    assert!(!pre_dispatch.may_have_effected());

    let mut reconciled = entry("key-reconciled");
    drive_to(&mut reconciled, ToolCallState::Reconciling);
    assert!(
        reconciled.was_dispatched(),
        "reconciliation only applies to a dispatched call"
    );
    assert!(
        reconciled.may_have_effected(),
        "unknown until the provider is read"
    );
    let record = reconciled
        .confirm_no_effect(reconciled.version(), now())
        .expect("RECONCILING -> FAILED is legal");
    assert_eq!(record.to, ToolCallState::Failed);
    assert!(!reconciled.state().is_unsettled(), "the call is settled");
    assert!(reconciled.no_effect_confirmed());
    assert!(!reconciled.may_have_effected());
    // A second confirmation is refused, because the row is terminal — the idempotence of reconciliation
    // comes from it being a one-way transition rather than from a guard in this method.
    assert_eq!(
        code_of(&reconciled.confirm_no_effect(reconciled.version(), now())),
        Some("tool.state_conflict"),
    );
}

#[test]
fn the_transition_record_names_both_states_and_both_versions() {
    let mut entry = entry("key-1");
    let record = entry
        .apply(
            ToolCallState::Validated,
            ToolCallVersion::FIRST,
            None,
            now(),
        )
        .expect("the first edge is legal");
    assert_eq!(record.id, entry.id);
    assert_eq!(record.from, ToolCallState::Requested);
    assert_eq!(record.to, ToolCallState::Validated);
    assert_eq!(record.prior_version, ToolCallVersion::FIRST);
    assert_eq!(record.version, ToolCallVersion::new(2));
    assert_eq!(record.occurred_at, now());
    assert!(
        record.outcome.is_none(),
        "a non-terminal transition has no outcome"
    );
    assert_eq!(entry.updated_at(), now());
}

// ---------------------------------------------------------------------------------------
// The reservation key: five dimensions, all of them required.
// ---------------------------------------------------------------------------------------

#[test]
fn an_empty_or_unbounded_idempotency_key_is_refused() {
    // An empty key is refused rather than treated as "no key": a caller who means "no key" must say so
    // by not asking for a reservation, and a defaulted empty string would make every unkeyed call
    // collide with every other.
    assert_eq!(
        field_of(&ReservationKey::new(
            tool_identity(),
            workspace(1),
            principal(1),
            "",
        )),
        Some("idempotency_key"),
    );
    assert_eq!(
        field_of(&ReservationKey::new(
            tool_identity(),
            workspace(1),
            principal(1),
            &"k".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1),
        )),
        Some("idempotency_key"),
    );
    assert_eq!(
        field_of(&ReservationKey::new(
            tool_identity(),
            workspace(1),
            principal(1),
            "key\u{0}with-nul",
        )),
        Some("idempotency_key"),
    );
    assert!(
        ReservationKey::new(
            tool_identity(),
            workspace(1),
            principal(1),
            &"k".repeat(MAX_IDEMPOTENCY_KEY_BYTES),
        )
        .is_ok(),
    );
}

#[test]
fn the_reservation_key_differs_when_any_of_its_four_components_differs() {
    // **The scoping rule stated as a property**, one component at a time. A key missing a component
    // would make a pair equal — and here the consequence is a second effect rather than a disclosure:
    // a retry by one principal finding another principal's row would be answered with that row's
    // outcome and skip the call the caller actually asked for.
    let base = key("shared-key");
    let other_principal =
        ReservationKey::new(tool_identity(), workspace(1), principal(2), "shared-key")
            .expect("valid");
    let other_workspace =
        ReservationKey::new(tool_identity(), workspace(2), principal(1), "shared-key")
            .expect("valid");
    let other_key = key("a-different-key");
    let mut other_identity_source = tool_identity();
    other_identity_source.source = ToolSource::new(
        SourceKind::McpServer,
        "rogue.mail",
        ToolVersion::parse("1.0.0").expect("valid"),
    )
    .expect("valid");
    let other_source = ReservationKey::new(
        other_identity_source,
        workspace(1),
        principal(1),
        "shared-key",
    )
    .expect("valid");

    for (label, varied) in [
        ("principal", other_principal),
        ("workspace", other_workspace),
        ("idempotency key", other_key),
        ("tool identity", other_source),
    ] {
        assert_ne!(
            base, varied,
            "two keys differing only in the {label} must not be equal, or one caller's retry could \
             be answered with another's result",
        );
    }
}

// ---------------------------------------------------------------------------------------
// The ledger: reservation and the ACC-025 cases.
// ---------------------------------------------------------------------------------------

#[test]
fn a_first_reservation_is_granted_and_a_second_for_the_same_key_is_not() {
    // The positive control for everything below, and the atomicity claim: the lookup and the insert
    // are one operation, so a second identical reservation cannot be granted.
    let mut ledger = ToolCallLedger::new();
    assert!(ledger.is_empty());
    assert_eq!(ledger.reserve(entry("key-1")), ReservationOutcome::Granted);
    assert_eq!(ledger.len(), 1);

    let second = ledger.reserve(entry("key-1"));
    assert_eq!(
        second,
        ReservationOutcome::InFlight {
            state: ToolCallState::Requested
        },
        "a second reservation while the first is live must be refused as in flight",
    );
    assert_eq!(ledger.len(), 1, "a refused reservation must not add a row");
}

#[test]
fn a_different_key_is_granted_because_the_five_dimensions_differ() {
    // The positive control for the key: refusing *every* second reservation would be a ledger that
    // admits one call ever, so the rule must key on the whole tuple rather than on "a row exists".
    let mut ledger = ToolCallLedger::new();
    assert_eq!(ledger.reserve(entry("key-1")), ReservationOutcome::Granted);
    assert_eq!(ledger.reserve(entry("key-2")), ReservationOutcome::Granted);
    assert_eq!(ledger.len(), 2);
}

#[test]
fn a_settled_duplicate_returns_the_recorded_outcome_rather_than_a_failure() {
    // A completed call is not a failure for a second caller: the effect already happened and the
    // honest answer is the outcome. A `Result<(), Error>` shape would report this as "no" and invite a
    // retry — the most dangerous response available here.
    let mut ledger = ToolCallLedger::new();
    ledger.reserve(entry("key-1"));
    {
        let row = ledger.get_mut(&key("key-1")).expect("the row exists");
        drive_to(row, ToolCallState::Succeeded);
    }
    let outcome = ledger.reserve(entry("key-1"));
    assert_eq!(
        outcome,
        ReservationOutcome::AlreadyTerminal {
            state: ToolCallState::Succeeded,
            outcome: None,
            // **`false`, because a success WAS dispatched** — it produced the very effect that made it
            // succeed. Claiming no-effect here would be the worst possible direction: a caller would
            // read "nothing happened, so retrying is safe" about the one call that definitely did
            // something.
            no_effect_confirmed: false,
        },
        "a settled success must be reported with its recorded outcome, not as a failure",
    );
    assert!(outcome.is_settled_duplicate());
    assert!(
        !outcome.permits_dispatch(),
        "a settled SUCCESS must not be re-dispatched: the effect already happened",
    );
}

/// `ACC-025`: *"Crash after the fake provider accepts an effect but before result persistence.
/// Recovery enters reconciliation and proves no duplicate effect."*
#[test]
fn a_crash_after_dispatch_enters_reconciliation_and_never_a_retry() {
    // **The acceptance scenario as one executable sequence.** A call reached `EXECUTING` — the state
    // the contract requires *before* an external effect — and then the process stopped, so no outcome
    // was recorded. Recovery classifies it, and the only permissible action is to reconcile: the
    // provider is the sole authority on whether the effect landed, and a retry that assumed it did not
    // is the second email.
    let mut ledger = ToolCallLedger::new();
    ledger.reserve(entry("urgent-call"));
    {
        let row = ledger.get_mut(&key("urgent-call")).expect("the row exists");
        drive_to(row, ToolCallState::Executing);
    }
    // The scan a startup pass performs finds it, because it was dispatched and has no outcome.
    let unsettled = ledger.possibly_effecting_without_outcome();
    assert_eq!(unsettled.len(), 1, "the dispatched call must be found");
    let found = unsettled[0];
    assert_eq!(found.state(), ToolCallState::Executing);
    assert!(found.may_have_effected());

    let action = classify_interrupted(found.state()).expect("a non-terminal state classifies");
    assert_eq!(
        action,
        InterruptedCallAction::Reconcile {
            was_in: ToolCallState::Executing
        },
    );
    assert!(
        !action.permits_retry(),
        "a dispatched call must NEVER be retried: the effect may already have landed",
    );

    // Recovery moves it to reconciliation, which is the state a startup pass would write.
    {
        let row = ledger.get_mut(&key("urgent-call")).expect("the row exists");
        row.apply(action.target_state(), row.version(), None, now())
            .expect("EXECUTING -> RECONCILING is legal");
    }
    assert_eq!(
        ledger.unsettled().len(),
        1,
        "it is now explicitly unsettled"
    );

    // And a duplicate submission *while it is unsettled* is refused as unsettled — not as in flight,
    // and emphatically not as a granted retry — so the caller is told to wait for reconciliation.
    let duplicate = ledger.reserve(entry("urgent-call"));
    assert!(
        matches!(duplicate, ReservationOutcome::Unsettled { .. }),
        "a duplicate while unsettled must be reported as unsettled, got {duplicate:?}",
    );
    assert!(!duplicate.permits_dispatch());

    // Reconciliation reads the provider and establishes that nothing happened. Now, and only now, is
    // a dispatch permissible — and it is permissible because the *world was checked*, not because time
    // passed or the tool declared itself idempotent.
    //
    // **This is a transition, not a flag.** The first version set the claim and left the row in
    // `RECONCILING`, so the row still looked unsettled to every reader — including this reservation
    // check, which reported the duplicate as needing reconciliation *after the reconciliation had
    // happened*. Reconciliation has an outcome like any other, so it ends the call.
    {
        let row = ledger.get_mut(&key("urgent-call")).expect("the row exists");
        row.confirm_no_effect(row.version(), now())
            .expect("RECONCILING -> FAILED is a legal edge");
    }
    assert_eq!(
        ledger.unsettled().len(),
        0,
        "a reconciled call is no longer unsettled, because the answer is now known",
    );
    let after = ledger.reserve(entry("urgent-call"));
    assert_eq!(
        after,
        ReservationOutcome::AlreadyTerminal {
            state: ToolCallState::Failed,
            outcome: Some(ToolErrorClass::ProviderError),
            no_effect_confirmed: true,
        },
        "after reconciliation the duplicate must be settled, proven harmless, and typed as a \
         provider fault",
    );
    assert!(
        after.permits_dispatch(),
        "a call PROVEN to have had no effect may be retried — and only a provider read can prove that",
    );
}

#[test]
fn a_call_interrupted_before_dispatch_is_safe_to_retry() {
    // The other half of `ACC-025`'s classification, asserted so the rule is not "never retry". A call
    // that had not dispatched cannot have produced an effect, so repeating the request is safe for a
    // reason that is about **dispatch** rather than about the tool's own idempotency declaration.
    for state in [
        ToolCallState::Requested,
        ToolCallState::Validated,
        ToolCallState::WaitingApproval,
        ToolCallState::Approved,
        ToolCallState::Reserved,
    ] {
        let action = classify_interrupted(state).expect("a non-terminal state classifies");
        assert_eq!(
            action,
            InterruptedCallAction::SafeToRetry { was_in: state },
            "{state} was never dispatched, so no effect can exist",
        );
        assert!(action.permits_retry(), "{state} may be retried");
        assert!(!action.reason().is_empty());
    }
}

#[test]
fn a_terminal_call_is_left_exactly_as_recorded() {
    // A finished call is not recovered: rewriting a recorded outcome would mean re-reporting an effect
    // that landed or erasing the record of one that did. Follows `run::recovery::classify`'s rule that
    // a terminal run is left alone.
    for state in [
        ToolCallState::Denied,
        ToolCallState::Succeeded,
        ToolCallState::Failed,
        ToolCallState::Cancelled,
    ] {
        assert_eq!(
            classify_interrupted(state),
            None,
            "{state} is terminal and must be left as recorded",
        );
    }
}

#[test]
fn every_non_terminal_state_classifies_and_every_terminal_state_does_not() {
    // **The exhaustiveness property**, so a state added later cannot silently default. Written as an
    // equivalence in both directions rather than a loop over a hardcoded list, because the list is
    // what would go stale.
    for state in ToolCallState::ALL {
        assert_eq!(
            classify_interrupted(*state).is_some(),
            !state.is_terminal(),
            "{state}: classification must exist exactly for the non-terminal states",
        );
    }
    // And the classification's retry answer follows dispatch, not the state's name.
    for state in ToolCallState::ALL {
        if let Some(action) = classify_interrupted(*state) {
            assert_eq!(
                action.permits_retry(),
                !state.was_dispatched(),
                "{state}: only an undispatched call may be retried",
            );
        }
    }
}

#[test]
fn the_recovery_scan_finds_dispatched_calls_without_an_outcome_and_nothing_else() {
    // The work list a startup pass walks. Narrower than `unsettled`, because a call left in
    // `EXECUTING` by a crash has been dispatched and its outcome is unknown even though its state does
    // not yet say `RECONCILING` — that gap is exactly what a crash leaves, so the scan is what lets
    // recovery find the calls `ACC-025` is about.
    let mut ledger = ToolCallLedger::new();
    // A call that never dispatched: not an effect risk.
    ledger.reserve(entry("never-dispatched"));
    // A call left mid-dispatch by a crash.
    ledger.reserve(entry("crashed-mid-dispatch"));
    {
        let row = ledger.get_mut(&key("crashed-mid-dispatch")).expect("row");
        drive_to(row, ToolCallState::Executing);
    }
    // A call that finished successfully: nothing to reconcile.
    ledger.reserve(entry("finished"));
    {
        let row = ledger.get_mut(&key("finished")).expect("row");
        drive_to(row, ToolCallState::Succeeded);
    }

    let found = ledger.possibly_effecting_without_outcome();
    assert_eq!(
        found.len(),
        1,
        "only the crash-leaving call is an effect risk"
    );
    assert_eq!(found[0].state(), ToolCallState::Executing);
    assert_eq!(found[0].key.idempotency_key, "crashed-mid-dispatch");
}

#[test]
fn the_ledger_reports_which_attempt_a_key_is_on() {
    // A second attempt has a **different key**, because the key identifies the invocation while the
    // attempt number distinguishes retries of it — so the attempt count is a property of the row
    // rather than of the key, which is why this is a lookup and not a scan.
    let mut ledger = ToolCallLedger::new();
    ledger.reserve(LedgerEntry::reserve(
        call_id(1),
        2,
        run_id(),
        key("key-1"),
        LedgerOperation::Execute,
        now(),
    ));
    assert_eq!(ledger.attempt_for(&key("key-1")), Some(2));
    assert_eq!(ledger.attempt_for(&key("absent")), None);
}

#[test]
fn an_attempt_number_is_always_at_least_one() {
    // The property the private field makes structural. `0` is what an uninitialised counter produces,
    // and "attempt zero" has no meaning, so nothing downstream could report it coherently. The field
    // is private and a zero is normalised to one at construction, so there is no way to put a zero
    // attempt into a ledger — the accessor exists because a public field would have allowed exactly
    // that after the normalisation.
    let zero = LedgerEntry::reserve(
        call_id(1),
        0,
        run_id(),
        key("key-1"),
        LedgerOperation::Execute,
        now(),
    );
    assert_eq!(
        zero.attempt(),
        1,
        "a zero attempt must be normalised at construction"
    );

    // And a ledger reports the attempt it stored, so the accessor is what a caller reads.
    let mut ledger = ToolCallLedger::new();
    let second = LedgerEntry::reserve(
        call_id(1),
        2,
        run_id(),
        key("key-2"),
        LedgerOperation::Execute,
        now(),
    );
    ledger.reserve(second);
    assert_eq!(ledger.attempt_for(&key("key-2")), Some(2));
    assert_eq!(ledger.attempt_for(&key("absent")), None);
}
#[test]
fn a_ledger_row_survives_a_json_round_trip_in_its_current_state() {
    // A ledger is durable, so the wire form is contract surface: a re-read row must land in the state
    // and version it was stored in, not in a fresh `REQUESTED` at version one.
    let mut entry = entry("key-1");
    drive_to(&mut entry, ToolCallState::Executing);
    entry
        .apply(
            ToolCallState::Failed,
            entry.version(),
            Some(ToolErrorClass::ProviderError),
            now(),
        )
        .expect("a failure is recordable");
    let json = serde_json::to_string(&entry).expect("a row serializes");
    let restored: LedgerEntry = serde_json::from_str(&json).expect("a row deserializes");
    assert_eq!(restored, entry);
    assert_eq!(restored.state(), ToolCallState::Failed);
    assert_eq!(restored.outcome(), Some(ToolErrorClass::ProviderError));
    assert!(
        restored.may_have_effected(),
        "the effect risk survives the round trip",
    );
    assert!(restored.was_dispatched(), "so does the dispatch fact");
    // Every enum in the row is spelled in the one form `as_contract_str` returns, so a stored row is
    // readable against the contract that owns it. Asserting each is what would catch a serde rename
    // that made one of them disagree with its own method — the drift this test was written after
    // finding in `ToolCallState`.
    assert!(json.contains(r#""state":"failed""#), "{json}");
    assert!(json.contains(r#""operation":"execute""#), "{json}");
    assert!(json.contains(r#""kind":"connector""#), "{json}");
    assert!(json.contains(r#""outcome":"provider_error""#), "{json}");
    assert!(json.contains(r#""capability":"mail.send@1""#), "{json}");
}

#[test]
fn a_finished_call_with_no_effect_permits_dispatch_but_an_in_flight_one_does_not() {
    // The `permits_dispatch` truth table, asserted over the four variants so a caller cannot be given
    // the right answer for one and a wrong one for another. The two dangerous arms are asserted
    // explicitly: in flight must not permit a second dispatch (that is the reservation), and unsettled
    // must not either (that is `ACC-025`).
    assert!(ReservationOutcome::Granted.permits_dispatch());
    assert!(
        ReservationOutcome::AlreadyTerminal {
            state: ToolCallState::Failed,
            outcome: Some(ToolErrorClass::Unavailable),
            no_effect_confirmed: true,
        }
        .permits_dispatch(),
        "a failed call proven to have had no effect may be retried",
    );
    assert!(
        !ReservationOutcome::AlreadyTerminal {
            state: ToolCallState::Failed,
            outcome: Some(ToolErrorClass::ProviderError),
            no_effect_confirmed: false,
        }
        .permits_dispatch(),
        "a failed call that MIGHT have effected must not be retried",
    );
    assert!(
        !ReservationOutcome::AlreadyTerminal {
            state: ToolCallState::Succeeded,
            outcome: None,
            no_effect_confirmed: true,
        }
        .permits_dispatch(),
        "a successful call must never be re-dispatched, whatever the flag says",
    );
    assert!(
        !ReservationOutcome::InFlight {
            state: ToolCallState::Executing
        }
        .permits_dispatch(),
    );
    assert!(
        !ReservationOutcome::Unsettled {
            row: crate::ids::ToolCallRecordId::from_uuid(uuid::Uuid::from_u128(1)),
        }
        .permits_dispatch(),
    );
}

#[test]
fn the_ledger_holds_no_policy_and_answers_no_permission_question() {
    // The same structural rule `TLS-002` recorded for the registry: the ledger records what happened,
    // and the decision to dispatch was taken by policy before a reservation was asked for. A ledger
    // that also decided whether a call was allowed would be a second authorization layer, and the one
    // consulted first would be the one that mattered.
    let mut ledger = ToolCallLedger::new();
    ledger.reserve(entry("key-1"));
    let row = ledger.get(&key("key-1")).expect("the row exists");
    // What it does expose is facts about the attempt: its state, version, timestamps, and outcome.
    assert_eq!(row.state(), ToolCallState::Requested);
    assert!(row.outcome().is_none());
    // And there is no lookup that answers a permission question: the only key-addressed read returns a
    // row, so a caller wanting a decision has to ask for one elsewhere.
    assert!(!row.may_have_effected());
}
