//! The restart schedule, which is a pure function of the policy. The behaviour that needs a live child is in
//! `tests/mcp_supervisor_process.rs`.

use std::time::Duration;

use super::McpRestartPolicy;

fn policy() -> McpRestartPolicy {
    McpRestartPolicy {
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(10),
        max_consecutive_failures: 5,
        stable_window: Duration::from_secs(60),
    }
}

#[test]
fn the_delay_doubles_from_the_base() {
    let policy = policy();
    assert_eq!(policy.delay_after(1), Duration::from_secs(1));
    assert_eq!(policy.delay_after(2), Duration::from_secs(2));
    assert_eq!(policy.delay_after(3), Duration::from_secs(4));
    assert_eq!(policy.delay_after(4), Duration::from_secs(8));
}

#[test]
fn the_delay_is_capped_and_a_huge_attempt_count_cannot_overflow_into_a_short_one() {
    let policy = policy();
    assert_eq!(policy.delay_after(5), Duration::from_secs(10));
    assert_eq!(policy.delay_after(31), Duration::from_secs(10));
    assert_eq!(policy.delay_after(u32::MAX), Duration::from_secs(10));
}

#[test]
fn attempt_zero_is_treated_as_the_first() {
    assert_eq!(policy().delay_after(0), Duration::from_secs(1));
}

#[test]
fn the_default_policy_is_bounded() {
    let policy = McpRestartPolicy::default();
    assert!(policy.max_consecutive_failures > 0);
    assert!(policy.base_delay <= policy.max_delay);
}
