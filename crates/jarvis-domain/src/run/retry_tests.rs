//! Tests for the retry policy's decision.
//!
//! The value here is almost entirely in the **refusals**, and especially in the ambiguity
//! rule: the same kind of failure can happen before or after the provider accepted the
//! call, and only one of those may be retried. A test that only checked "a transient error
//! is retried" would pass against a policy that ignored acceptance entirely — which is the
//! defect the contract's retry-ownership section exists to prevent.

use super::{
    FailureClass, FailureSite, MAX_ATTEMPTS, MAX_BACKOFF_MS, RetryDecision, RetryError,
    RetryPolicy, RetryRefusal,
};
use crate::run::budget::RunBudget;
use crate::time::UtcTimestamp;

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
}

/// A budget with 60 seconds left, so a decision is not refused for lack of time.
fn roomy() -> RunBudget {
    RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T12:01:00Z").expect("valid"))
}

/// A policy of three attempts with a 100ms base growing to 1s.
fn three_attempts() -> RetryPolicy {
    RetryPolicy::new(3, 100, 1_000).expect("3 attempts with 100ms base is in range")
}

/// Decides against a roomy budget.
fn decide(
    policy: &RetryPolicy,
    class: FailureClass,
    site: FailureSite,
    attempts: u32,
) -> RetryDecision {
    RetryDecision::decide(policy, class, site, attempts, &roomy(), now())
}

#[test]
fn a_retryable_error_before_acceptance_is_retried() {
    // The case retry exists for: the provider never took the call, so repeating it cannot
    // repeat an effect.
    let decision = decide(
        &three_attempts(),
        FailureClass::Transient,
        FailureSite::BeforeAcceptance,
        1,
    );
    assert_eq!(
        decision,
        RetryDecision::Retry {
            attempt: 2,
            delay_ms: 100,
        },
    );
    assert!(decision.is_retry());
    assert!(!decision.refused_by_budget());
}

#[test]
fn the_same_error_after_acceptance_is_refused_as_ambiguous() {
    // The safety rule, and the reason `FailureSite` is a required input: identical errors
    // must decide differently on either side of acceptance, so a policy that looked only at
    // the error would retry an ambiguous request the contract forbids.
    let error = FailureClass::Transient;
    assert!(decide(&three_attempts(), error, FailureSite::BeforeAcceptance, 1).is_retry());
    assert_eq!(
        decide(&three_attempts(), error, FailureSite::AfterAcceptance, 1),
        RetryDecision::GiveUp {
            reason: RetryRefusal::AmbiguousAfterAcceptance
        },
    );
}

#[test]
fn ambiguity_outranks_attempt_count_and_retryability() {
    // The safety check runs first, so a failure after acceptance is refused even when the
    // error is retryable and attempts remain. If any other rule were checked first, a
    // combination could reach a retry that the rule exists to prevent.
    let decision = decide(
        &three_attempts(),
        FailureClass::Transient,
        FailureSite::AfterAcceptance,
        1,
    );
    assert_eq!(
        decision,
        RetryDecision::GiveUp {
            reason: RetryRefusal::AmbiguousAfterAcceptance
        },
    );
}

#[test]
fn a_permanent_failure_is_not_retried_even_before_acceptance() {
    // Nothing was accepted, so the *ambiguity* rule does not apply — but a refusal, a
    // rejected credential, or a malformed request cannot be fixed by repeating it. The
    // caller classifies the failure, because that needs a provider's error vocabulary which
    // the domain does not own.
    assert_eq!(
        decide(
            &three_attempts(),
            FailureClass::Permanent,
            FailureSite::BeforeAcceptance,
            1,
        ),
        RetryDecision::GiveUp {
            reason: RetryRefusal::NotRetryable
        },
    );
}

#[test]
fn a_transient_failure_is_retried_regardless_of_which_error_it_was() {
    // The complement of the test above, and the reason the classification is a two-valued
    // input rather than a list of provider errors: `Transient` means "a repeat could help",
    // and the policy does not need to know *which* transient failure it was to decide.
    assert!(
        decide(
            &three_attempts(),
            FailureClass::Transient,
            FailureSite::BeforeAcceptance,
            1,
        )
        .is_retry(),
    );
}

#[test]
fn the_attempt_count_is_not_off_by_one() {
    // `max_attempts` is the total including the first, so with 3 the first (1) and second
    // (2) may be followed and the third (3) may not. An off-by-one here either retries once
    // more than the caller allowed or never retries at all.
    let policy = three_attempts();
    assert!(
        policy.permits_attempt(2),
        "the first attempt may be followed"
    );
    assert!(policy.permits_attempt(3), "the second may be followed");
    assert!(!policy.permits_attempt(4), "the third may not");

    assert!(
        decide(
            &policy,
            FailureClass::Transient,
            FailureSite::BeforeAcceptance,
            2
        )
        .is_retry()
    );
    assert_eq!(
        decide(
            &policy,
            FailureClass::Transient,
            FailureSite::BeforeAcceptance,
            3
        ),
        RetryDecision::GiveUp {
            reason: RetryRefusal::AttemptsExhausted
        },
    );
}

#[test]
fn a_policy_of_one_attempt_never_retries() {
    // `none()` is the default, and it must mean "no retry" rather than "retry once": a
    // default that retried would spend a budget the caller never offered.
    let policy = RetryPolicy::none();
    assert_eq!(policy.max_attempts, 1);
    assert!(!policy.permits_attempt(2));
    assert_eq!(
        decide(
            &policy,
            FailureClass::Transient,
            FailureSite::BeforeAcceptance,
            1
        ),
        RetryDecision::GiveUp {
            reason: RetryRefusal::AttemptsExhausted
        },
    );
    assert_eq!(
        policy.backoff_ms(1),
        0,
        "the first attempt waits for nothing"
    );
}

#[test]
fn backoff_doubles_from_the_base_and_saturates_at_the_ceiling() {
    // Deterministic growth, bounded at the ceiling: an unbounded backoff would be an
    // unbounded wait, and the ceiling is what keeps attempt 5 from waiting a minute.
    let policy = RetryPolicy::new(5, 100, 1_000).expect("in range");
    assert_eq!(
        policy.backoff_ms(1),
        0,
        "no delay precedes the first attempt"
    );
    assert_eq!(policy.backoff_ms(2), 100);
    assert_eq!(policy.backoff_ms(3), 200);
    assert_eq!(policy.backoff_ms(4), 400);
    assert_eq!(policy.backoff_ms(5), 800);
    // Saturates rather than overflowing, even for an absurd attempt number.
    assert_eq!(policy.backoff_ms(9), 1_000);
    assert_eq!(policy.backoff_ms(u32::MAX), 1_000);
}

#[test]
fn a_base_above_the_ceiling_is_refused_rather_than_clamped() {
    // A clamped value is one the caller did not choose and cannot detect, which is the same
    // reasoning the portable settings block uses to refuse an out-of-range temperature.
    assert_eq!(
        RetryPolicy::new(2, 500, 100),
        Err(RetryError::BackoffOutOfRange),
    );
}

#[test]
fn attempts_and_backoff_outside_their_bounds_are_refused() {
    assert_eq!(
        RetryPolicy::new(0, 0, 0),
        Err(RetryError::AttemptsOutOfRange)
    );
    assert_eq!(
        RetryPolicy::new(MAX_ATTEMPTS + 1, 0, 0),
        Err(RetryError::AttemptsOutOfRange),
    );
    assert_eq!(
        RetryPolicy::new(2, 0, MAX_BACKOFF_MS + 1),
        Err(RetryError::BackoffOutOfRange),
    );
    assert!(RetryPolicy::new(MAX_ATTEMPTS, 0, MAX_BACKOFF_MS).is_ok());
}

#[test]
fn a_retry_that_cannot_fit_before_the_deadline_is_refused() {
    // A retry whose backoff outlives the deadline is a delay followed by the same failure,
    // with the run's remaining time spent learning nothing. This is what makes "the run
    // failed on time" true rather than approximate.
    let budget =
        RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T12:00:00.050Z").expect("valid"));
    let decision = RetryDecision::decide(
        &RetryPolicy::new(3, 1_000, 1_000).expect("in range"),
        FailureClass::Transient,
        FailureSite::BeforeAcceptance,
        1,
        &budget,
        now(),
    );
    assert_eq!(
        decision,
        RetryDecision::GiveUp {
            reason: RetryRefusal::BackoffOutlivesDeadline
        },
    );
    assert!(
        decision.refused_by_budget(),
        "the budget, not the policy, stopped this retry",
    );
}

#[test]
fn a_retry_at_a_deadline_that_has_passed_is_refused() {
    // The deadline itself counts as expired, so a run with no time left cannot fund another
    // attempt. Checked before the backoff comparison so the two never disagree.
    let budget =
        RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T11:59:59Z").expect("valid"));
    let decision = RetryDecision::decide(
        &three_attempts(),
        FailureClass::Transient,
        FailureSite::BeforeAcceptance,
        1,
        &budget,
        now(),
    );
    assert_eq!(
        decision,
        RetryDecision::GiveUp {
            reason: RetryRefusal::DeadlineExceeded
        },
    );
    assert!(decision.refused_by_budget());
}

#[test]
fn an_unbounded_budget_permits_a_retry_whatever_the_backoff() {
    // No deadline means no time limit, so the backoff cannot outlive it. Reading an unset
    // deadline as "no time left" would refuse every retry for an unbounded run.
    let budget = RunBudget::default();
    let decision = RetryDecision::decide(
        &RetryPolicy::new(3, 60_000, 60_000).expect("in range"),
        FailureClass::Transient,
        FailureSite::BeforeAcceptance,
        1,
        &budget,
        now(),
    );
    assert!(decision.is_retry(), "{decision:?}");
    assert!(!decision.refused_by_budget());
}

#[test]
fn the_remaining_time_is_compared_against_the_backoff_not_ignored() {
    // A budget with more time than the backoff must permit the retry, so the comparison is
    // a comparison rather than a constant refusal.
    let budget =
        RunBudget::with_deadline(UtcTimestamp::parse("2026-09-22T12:00:05Z").expect("valid"));
    let decision = RetryDecision::decide(
        &RetryPolicy::new(3, 100, 1_000).expect("in range"),
        FailureClass::Transient,
        FailureSite::BeforeAcceptance,
        1,
        &budget,
        now(),
    );
    assert!(decision.is_retry(), "{decision:?}");
}

#[test]
fn only_the_two_budget_refusals_report_as_budget_caused() {
    // The distinction a caller needs: "the provider kept failing" and "the run ran out of
    // time" lead to different reports, and the second means the run should be reported as
    // timed out rather than as a provider fault.
    for (decision, budget_caused) in [
        (
            RetryDecision::GiveUp {
                reason: RetryRefusal::DeadlineExceeded,
            },
            true,
        ),
        (
            RetryDecision::GiveUp {
                reason: RetryRefusal::BackoffOutlivesDeadline,
            },
            true,
        ),
        (
            RetryDecision::GiveUp {
                reason: RetryRefusal::AttemptsExhausted,
            },
            false,
        ),
        (
            RetryDecision::GiveUp {
                reason: RetryRefusal::NotRetryable,
            },
            false,
        ),
        (
            RetryDecision::GiveUp {
                reason: RetryRefusal::AmbiguousAfterAcceptance,
            },
            false,
        ),
        (
            RetryDecision::Retry {
                attempt: 2,
                delay_ms: 0,
            },
            false,
        ),
    ] {
        assert_eq!(decision.refused_by_budget(), budget_caused, "{decision:?}");
    }
}

#[test]
fn the_budget_refusal_codes_reuse_the_runs_own_deadline_code() {
    // A budget-caused refusal reports `run.deadline_exceeded` — the same code the run itself
    // uses — so a caller cannot see two names for one condition.
    assert_eq!(
        RetryRefusal::DeadlineExceeded.code(),
        "run.deadline_exceeded"
    );
    for refusal in [
        RetryRefusal::AmbiguousAfterAcceptance,
        RetryRefusal::NotRetryable,
        RetryRefusal::AttemptsExhausted,
        RetryRefusal::BackoffOutlivesDeadline,
    ] {
        assert!(refusal.code().starts_with("run."), "{}", refusal.code());
    }
}

#[test]
fn the_codes_are_namespaced_and_the_errors_explain_themselves() {
    for error in [
        RetryError::AttemptsOutOfRange,
        RetryError::BackoffOutOfRange,
    ] {
        assert!(error.code().starts_with("run."), "{}", error.code());
        assert!(!error.to_string().is_empty(), "{error:?}");
    }
}

#[test]
fn the_default_policy_does_not_retry() {
    // The default is a decision, not an oversight: a policy that retried by default would
    // spend a budget the caller never offered, and `MAX_ATTEMPTS` is the bound that keeps a
    // policy from spending it without limit.
    assert_eq!(RetryPolicy::default(), RetryPolicy::none());
    assert_eq!(RetryPolicy::default().max_attempts, 1);
    let ceiling = MAX_ATTEMPTS;
    assert!(
        ceiling > 1,
        "retry must be reachable by a policy: {ceiling}"
    );
}
