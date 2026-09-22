//! Tests for the run budget's arithmetic.
//!
//! The value here is in the boundary and the refusals, not the happy path: an
//! off-by-one at the deadline instant, a zero timeout read as "no bound", and an
//! overflowing millisecond count are the cases that would silently let a run exceed its
//! budget or fail for the wrong reason.

use super::{BudgetError, BudgetStatus, DEFAULT_RUN_DEADLINE_MS, MAX_STEP_TIMEOUT_MS, RunBudget};
use crate::time::UtcTimestamp;

/// A budget whose deadline is 60 seconds after `at`.
fn budget_at(at: &str) -> (UtcTimestamp, RunBudget) {
    let now = UtcTimestamp::parse(at).expect("valid");
    let deadline = UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid");
    (now, RunBudget::with_deadline(deadline))
}

#[test]
fn a_budget_with_no_deadline_is_unbounded_rather_than_expired() {
    // "No cap" and "cap reached" are different facts. Collapsing them would make every
    // unbounded run look out of time and refuse work it is allowed to do.
    let now = UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid");
    let budget = RunBudget::default();
    assert_eq!(budget.status_at(now), BudgetStatus::Unbounded);
    assert!(budget.permits_step_at(now));
    assert!(budget.status_at(now).is_permitted());
}

#[test]
fn time_remaining_is_measured_against_the_deadline() {
    let (now, budget) = budget_at("2026-09-22T12:00:00Z");
    let status = budget.status_at(now);
    assert_eq!(
        status,
        BudgetStatus::Remaining {
            deadline: UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid"),
            millis: 60_000,
        },
    );
    assert!(budget.permits_step_at(now));
}

#[test]
fn the_deadline_instant_itself_is_expired() {
    // The boundary belongs to the expired side. A budget with zero time left cannot
    // fund another step, and treating the instant as still-valid would make the answer
    // depend on the clock's sub-nanosecond reading.
    let (_, budget) = budget_at("2026-09-22T12:00:00Z");
    let exactly = UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid");
    assert_eq!(
        budget.status_at(exactly),
        BudgetStatus::Expired { deadline: exactly },
    );
    assert!(!budget.permits_step_at(exactly));
}

#[test]
fn a_passed_deadline_is_expired_by_exactly_one_nanosecond_later() {
    // The other side of the same boundary, so the test states both rather than one.
    let (_, budget) = budget_at("2026-09-22T12:00:00Z");
    let just_after = UtcTimestamp::parse("2026-09-22T12:01:00.000000001Z").expect("valid");
    assert!(!budget.permits_step_at(just_after));
    let just_before = UtcTimestamp::parse("2026-09-22T12:00:59.999999999Z").expect("valid");
    assert!(budget.permits_step_at(just_before));
}

#[test]
fn a_zero_step_timeout_is_refused_rather_than_read_as_unlimited() {
    // Zero is the dangerous value in both directions: read as "no bound" it removes the
    // bound the caller believed it set, and read literally it fails every step
    // immediately so the operator blames the provider.
    assert_eq!(
        RunBudget::default().with_step_timeout(0),
        Err(BudgetError::StepTimeoutOutOfRange),
    );
}

#[test]
fn a_step_timeout_above_the_ceiling_is_refused() {
    // An unbounded timeout is an unbounded wait, so the ceiling is enforced rather than
    // documented.
    assert_eq!(
        RunBudget::default().with_step_timeout(MAX_STEP_TIMEOUT_MS + 1),
        Err(BudgetError::StepTimeoutOutOfRange),
    );
    let accepted = RunBudget::default()
        .with_step_timeout(MAX_STEP_TIMEOUT_MS)
        .expect("the ceiling itself is allowed");
    assert_eq!(accepted.step_timeout_ms, Some(MAX_STEP_TIMEOUT_MS));
}

#[test]
fn a_step_timeout_inside_the_range_is_recorded() {
    let budget = RunBudget::default()
        .with_step_timeout(30_000)
        .expect("30s is in range");
    assert_eq!(budget.step_timeout_ms, Some(30_000));
}

#[test]
fn a_run_deadline_is_carried_onto_the_model_call() {
    // The defect this closes: a run with a deadline sent `limits.deadline: null`, so a
    // provider was free to wait forever and the run's own budget bounded nothing.
    let (_, budget) = budget_at("2026-09-22T12:00:00Z");
    let limits = budget.call_limits();
    assert_eq!(
        limits.deadline,
        Some(UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid")),
    );
}

#[test]
fn token_and_cost_ceilings_are_carried_onto_the_model_call() {
    // The same mapping for the other two budgets: a cap the run holds but the call does
    // not send is a cap that bounds nothing.
    let mut budget =
        RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid"));
    budget.max_output_tokens = Some(4096);
    budget.max_cost_microunits = Some(50_000);
    let limits = budget.call_limits();
    assert_eq!(limits.max_output_tokens, Some(4096));
    assert_eq!(limits.max_cost_microunits, Some(50_000));
}

#[test]
fn an_unset_call_ceiling_stays_absent_rather_than_becoming_zero() {
    // A zero cap would refuse the call outright, so "unset" must not be serialized or
    // mapped as zero — the same rule `Usage` follows for an unreported counter.
    let (_, budget) = budget_at("2026-09-22T12:00:00Z");
    let limits = budget.call_limits();
    assert_eq!(limits.max_output_tokens, None);
    assert_eq!(limits.max_cost_microunits, None);
}

#[test]
fn the_widest_possible_span_fits_a_u64_so_the_conversion_cannot_lose_time() {
    // The millisecond conversion saturates, but that branch is unreachable — and this
    // test is what establishes that rather than asserting it in a comment. The widest
    // span any two `UtcTimestamp` values can express is bounded by `jiff`'s own range,
    // which is far below `u64::MAX`. If a future dependency change widened the range,
    // this fails and the saturating branch becomes a real case to test.
    let now = UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid");
    let widest = now
        .as_timestamp()
        .duration_since(jiff::Timestamp::MIN)
        .as_millis();
    assert!(
        u64::try_from(widest).is_ok(),
        "the widest span in milliseconds must fit a u64, got {widest}",
    );
    assert!(u64::try_from(widest).expect("fits") < u64::MAX);
}

#[test]
fn a_far_future_deadline_is_reported_with_its_real_remaining_time() {
    // The largest deadline the clock can actually express, so the boundary of the
    // supported range is covered by a value that parses. (A literal `9999-12-31` does
    // not: `jiff` refuses dates past its `MAX`, which is exactly the bound above.)
    let now = UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid");
    let deadline = UtcTimestamp::parse("9999-12-30T22:00:00Z").expect("within jiff's range");
    let status = RunBudget::with_deadline(deadline).status_at(now);
    match status {
        BudgetStatus::Remaining { millis, .. } => {
            assert!(millis > 0, "{millis}");
            // Roughly 7973 years, which is the point: the value is genuinely large and
            // still exact rather than saturated.
            assert!(millis > 2_000_000_000_000, "{millis}");
            assert!(millis < u64::MAX, "{millis}");
        }
        other => unreachable!("a far-future deadline must report remaining time: {other:?}"),
    }
}

#[test]
fn the_defaults_are_bounded_and_finite() {
    // A constant default is only useful if it is inside the range the setter enforces;
    // otherwise the documented default is one the API itself would refuse.
    let default = DEFAULT_RUN_DEADLINE_MS;
    let ceiling = MAX_STEP_TIMEOUT_MS;
    assert!(default > 0, "{default} must be positive");
    assert!(default < ceiling, "{default} must be under {ceiling}");
    assert!(
        RunBudget::default().with_step_timeout(default).is_ok(),
        "the default must be an accepted value",
    );
}

#[test]
fn budget_codes_are_namespaced() {
    // A code without the `jarvis.` namespace is rewritten by the error-code type, so an
    // unnamespaced code would surface to an operator as `jarvis.internal` and name
    // nothing.
    for error in [BudgetError::StepTimeoutOutOfRange, BudgetError::Malformed] {
        assert!(error.code().starts_with("run."), "{}", error.code());
        assert!(!error.to_string().is_empty());
    }
}

#[test]
fn a_budget_round_trips_through_json_with_absent_fields_omitted() {
    // The stored `agent_runs.budget_json` column holds this shape, so an absent limit
    // must come back absent rather than as null-turned-zero.
    let budget =
        RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid"))
            .with_step_timeout(30_000)
            .expect("valid");
    let encoded = serde_json::to_string(&budget).expect("serializes");
    assert!(!encoded.contains("max_output_tokens"), "{encoded}");
    let decoded: RunBudget = serde_json::from_str(&encoded).expect("deserializes");
    assert_eq!(decoded, budget);
}

#[test]
fn an_unknown_budget_field_is_refused() {
    // `deny_unknown_fields`, so a stray or mistyped limit is a failure rather than an
    // ignored cap — silently dropping a limit is how a budget stops bounding anything.
    let error = serde_json::from_str::<RunBudget>(r#"{"max_output_token":10}"#)
        .expect_err("a misspelled limit must not parse");
    assert!(error.to_string().contains("max_output_token"), "{error}");
}
