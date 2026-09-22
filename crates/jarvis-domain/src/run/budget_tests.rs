//! Tests for the run budget's arithmetic.
//!
//! The value here is in the boundary and the refusals, not the happy path: an
//! off-by-one at the deadline instant, a zero timeout read as "no bound", and an
//! overflowing millisecond count are the cases that would silently let a run exceed its
//! budget or fail for the wrong reason.

use super::{
    BudgetError, BudgetLimit, BudgetStatus, DEFAULT_RUN_DEADLINE_MS, MAX_STEP_TIMEOUT_MS, RunBudget,
};
use crate::context::budget::MAX_CONTEXT_BUDGET_TOKENS;
use crate::model::stream::Usage;
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
fn a_zero_context_ceiling_is_refused_rather_than_read_as_no_context() {
    // Zero must not be readable as "send nothing": the domain's `ContextBudget` refuses
    // a zero-token budget too, so admitting it here would let a caller set a ceiling the
    // assembler rejects, and the failure would surface at the far boundary as though the
    // conversation were at fault.
    assert_eq!(
        RunBudget::default().with_context_tokens(0),
        Err(BudgetError::ContextTokensOutOfRange),
    );
}

#[test]
fn a_context_ceiling_above_the_domain_maximum_is_refused() {
    // The two ceilings must agree. `ContextBudget::new` refuses anything above
    // `MAX_CONTEXT_BUDGET_TOKENS`, so this type must refuse the same values or a stored
    // budget could hold a ceiling the assembler will not accept.
    assert_eq!(
        RunBudget::default().with_context_tokens(MAX_CONTEXT_BUDGET_TOKENS + 1),
        Err(BudgetError::ContextTokensOutOfRange),
    );
    let accepted = RunBudget::default()
        .with_context_tokens(MAX_CONTEXT_BUDGET_TOKENS)
        .expect("the domain maximum is the boundary the assembler accepts");
    assert_eq!(accepted.max_context_tokens, Some(MAX_CONTEXT_BUDGET_TOKENS));
}

#[test]
fn a_context_ceiling_inside_the_range_is_recorded() {
    let budget = RunBudget::default()
        .with_context_tokens(8_192)
        .expect("8192 is in range");
    assert_eq!(budget.max_context_tokens, Some(8_192));
}

#[test]
fn a_stored_budget_without_a_context_ceiling_still_parses() {
    // The field is `#[serde(default)]` for the same reason every other one is: a budget
    // written by the build that had no context ceiling must still be readable, because
    // runs outlive the process that created them. A parse failure here would make every
    // pre-existing run unrecoverable, which is a migration hazard hiding behind an
    // optional field.
    let budget: RunBudget =
        serde_json::from_str(r#"{"deadline":null,"step_timeout_ms":null}"#).expect("parses");
    assert_eq!(budget.max_context_tokens, None);
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

// ---------------------------------------------------------------------------
// Consumption ceilings
//
// These are what make `max_output_tokens` and `max_cost_microunits` bounds rather than
// numbers carried in a request. The boundary and the "no measurement" cases matter most:
// a boundary on the wrong side either refuses a call that fit or admits one that did not,
// and an absent measurement is the state most easily mistaken for enforcement.
// ---------------------------------------------------------------------------

/// A budget with only an output-token ceiling.
fn token_capped(cap: u64) -> RunBudget {
    RunBudget {
        max_output_tokens: Some(cap),
        ..RunBudget::default()
    }
}

/// Usage reporting `output_tokens`.
fn tokens(count: u64) -> Usage {
    Usage {
        output_tokens: Some(count),
        ..Usage::default()
    }
}

#[test]
fn usage_below_or_equal_to_the_ceiling_is_not_a_breach() {
    // The boundary belongs to the side that can still work: a ceiling of 2048 permits
    // producing token 2048. This is the opposite convention from the deadline (where the
    // boundary instant IS expired) and it is consistent — a deadline at `T` does not permit
    // work at `T`, because that work would finish after `T`.
    let budget = token_capped(2048);
    assert_eq!(budget.exceeded_by(&tokens(0)), None);
    assert_eq!(budget.exceeded_by(&tokens(2047)), None);
    assert_eq!(
        budget.exceeded_by(&tokens(2048)),
        None,
        "exactly the ceiling is inside the budget",
    );
}

#[test]
fn usage_above_the_ceiling_is_a_breach_naming_the_limit() {
    let budget = token_capped(2048);
    assert_eq!(
        budget.exceeded_by(&tokens(2049)),
        Some(BudgetLimit::OutputTokens),
    );
    assert_eq!(
        BudgetLimit::OutputTokens.code(),
        "run.budget_output_tokens_exceeded",
    );
}

#[test]
fn a_cost_breach_is_named_separately_from_a_token_breach() {
    // The two have different remedies — one says the model was asked for too much, the
    // other that the route was too expensive — so reporting either as "a budget breach"
    // would hide which.
    let budget = RunBudget {
        max_cost_microunits: Some(50_000),
        ..RunBudget::default()
    };
    let usage = Usage {
        estimated_cost_microunits: Some(50_001),
        ..Usage::default()
    };
    assert_eq!(budget.exceeded_by(&usage), Some(BudgetLimit::Cost));
    assert_eq!(BudgetLimit::Cost.code(), "run.budget_cost_exceeded");
    assert_eq!(budget.exceeded_by(&tokens(1_000_000)), None);
}

#[test]
fn a_breached_ceiling_is_reported_stably_when_both_are_breached() {
    // Both at once must always report the same one, or the same run would be reported
    // differently on two attempts. The order is fixed: the output ceiling first, because it
    // is the one the run controls directly.
    let budget = RunBudget {
        max_output_tokens: Some(10),
        max_cost_microunits: Some(10),
        ..RunBudget::default()
    };
    let usage = Usage {
        output_tokens: Some(11),
        estimated_cost_microunits: Some(11),
        ..Usage::default()
    };
    assert_eq!(budget.exceeded_by(&usage), Some(BudgetLimit::OutputTokens));
}

#[test]
fn an_unreported_counter_cannot_breach_a_ceiling() {
    // Refusing on an absent value would fail every run against a provider that does not
    // report usage — a false failure, not a safety property. The honest answer is that the
    // ceiling went unchecked, which `budget_is_verifiable` states.
    let budget = RunBudget {
        max_output_tokens: Some(10),
        max_cost_microunits: Some(10),
        ..RunBudget::default()
    };
    assert_eq!(budget.exceeded_by(&Usage::default()), None);
    assert!(
        !budget.budget_is_verifiable(&Usage::default()),
        "a ceiling with no measurement must not be reported as verified",
    );
}

#[test]
fn a_ceiling_with_a_measurement_is_verifiable() {
    let budget = token_capped(10);
    assert!(budget.budget_is_verifiable(&tokens(5)));
    assert!(budget.has_consumption_ceiling());
}

#[test]
fn a_budget_with_no_consumption_ceiling_is_verifiable_and_does_not_breach() {
    // No ceiling is trivially satisfied, and it must not be reported as unverifiable — that
    // would make an unbounded run look like one with an unchecked limit.
    let budget =
        RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T13:00:00Z").expect("valid"));
    assert!(!budget.has_consumption_ceiling());
    assert!(budget.budget_is_verifiable(&Usage::default()));
    assert_eq!(budget.exceeded_by(&tokens(u64::MAX)), None);
}

#[test]
fn a_reported_zero_is_a_measurement_and_does_not_exceed_a_positive_ceiling() {
    // The distinction the `Usage` type exists for: a provider reporting zero is a
    // measurement, so the ceiling IS verifiable and the run is well inside it.
    let budget = token_capped(10);
    let reported_zero = tokens(0);
    assert!(budget.budget_is_verifiable(&reported_zero));
    assert_eq!(budget.exceeded_by(&reported_zero), None);
}
