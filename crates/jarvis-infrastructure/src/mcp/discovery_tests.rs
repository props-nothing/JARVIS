//! The tests live in a separate file so this module's documentation stays about behavior rather than about
//! assertions. See the parent module for why discovery is shaped the way it is.

use super::{DISCOVERY_TIMEOUT, McpStartupRefusal};
use crate::mcp::process::{MAX_MCP_STARTUP_TIMEOUT_MS, MIN_MCP_STARTUP_TIMEOUT_MS};

#[test]
fn the_discovery_timeout_matches_the_longest_accepted_startup_timeout() {
    // A **coupling** assertion rather than a copy. The two bounds answer the same question — how long may a
    // server take — and a discovery that allowed less than a launch would refuse servers whose launch
    // configuration an operator was explicitly permitted to allow. Asserting the equality means a change to
    // one is a failing test rather than a silent disagreement.
    assert_eq!(
        u64::try_from(DISCOVERY_TIMEOUT.as_millis()).expect("the timeout fits in u64"),
        MAX_MCP_STARTUP_TIMEOUT_MS,
        "the discovery bound must equal the longest accepted startup timeout"
    );
}

#[test]
fn the_discovery_timeout_exceeds_the_shortest_accepted_startup_timeout() {
    // The complement of the assertion above: were the *shortest* accepted startup timeout ever raised past the
    // discovery bound, a launch an operator configured legitimately could not be discovered. This is the
    // direction that would break, and asserting it separately means a mutation of the equality above that
    // happens to still satisfy one of these dies in the other.
    assert!(
        u64::try_from(DISCOVERY_TIMEOUT.as_millis()).expect("the timeout fits in u64")
            > MIN_MCP_STARTUP_TIMEOUT_MS,
        "the discovery bound must exceed the shortest accepted startup timeout"
    );
}

#[test]
fn a_name_that_is_not_a_configuration_identity_is_permanent() {
    // The name is configuration, so a retry re-reads the same configuration. A non-permanent refusal here
    // would have a caller re-launching a process to rediscover a defect that is visible without one.
    let refusal = McpStartupRefusal::ServerNameInvalid;
    assert!(
        refusal.is_permanent(),
        "an unusable configured name cannot be repaired by retrying"
    );
    assert_eq!(refusal.code(), "mcp.server_config_invalid");
}

#[test]
fn a_version_mismatch_is_permanent_and_an_all_refused_catalog_is_too() {
    // Both are permanent for the same reason and it is worth asserting together: neither depends on anything
    // transient. The peer's version set is what the peer implements, and the definitions it sent are the ones
    // this adapter refused.
    assert!(
        McpStartupRefusal::NoCompatibleVersion.is_permanent(),
        "a peer with no shared version answers identically forever"
    );
    assert_eq!(
        McpStartupRefusal::NoCompatibleVersion.code(),
        "mcp.startup_no_compatible_version"
    );

    let refusal = McpStartupRefusal::NoUsableTools {
        first_rejection: crate::mcp::McpToolRejection::ToolNameInvalid,
    };
    assert!(
        refusal.is_permanent(),
        "a catalog whose every tool was refused is the same catalog next time"
    );
}

#[test]
fn a_launch_failure_and_a_timeout_are_retryable() {
    // **The complement of the permanence tests.** A security-shaped predicate is satisfied by an
    // implementation that always says `true`, so the retryable cases must be asserted separately or the
    // permanence claims above prove nothing.
    let launch = McpStartupRefusal::Launch {
        code: "mcp.spawn_failed",
    };
    assert!(
        !launch.is_permanent(),
        "a program may be installed before the next attempt"
    );
    assert_eq!(launch.code(), "mcp.spawn_failed");

    let timeout = McpStartupRefusal::TimedOut {
        diagnostics: String::new(),
    };
    assert!(!timeout.is_permanent(), "a slow start may be transient");
    assert_eq!(timeout.code(), "mcp.discovery_timeout");
}

#[test]
fn the_permanence_of_a_derived_failure_is_the_one_it_was_derived_from() {
    // `Launch` carries a code from another module and `StartupFailed`/`ListingFailed` carry a permanence
    // verdict. **The carried verdict must not be shadowed by a rule here**, or the two modules would disagree
    // about one failure and the one that logs would be the one nobody acted on.
    let permanent = McpStartupRefusal::StartupFailed {
        code: "mcp.startup_no_preferred_version",
        permanent: true,
        diagnostics: String::new(),
    };
    assert!(permanent.is_permanent());

    let transient = McpStartupRefusal::StartupFailed {
        code: "mcp.startup_connection_closed",
        permanent: false,
        diagnostics: String::new(),
    };
    assert!(
        !transient.is_permanent(),
        "the carried verdict must govern, not a rule inferred from the variant"
    );

    let listing = McpStartupRefusal::ListingFailed {
        code: "tool.provider_error",
        permanent: false,
        diagnostics: String::new(),
    };
    assert!(!listing.is_permanent());
    assert_eq!(listing.code(), "tool.provider_error");
}
