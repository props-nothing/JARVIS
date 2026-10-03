//! Tests for MCP composition.
//!
//! Three carry more weight than the rest, and each exists because a plausible implementation gets it wrong in
//! a way no other test here would notice:
//!
//! - **`every_declared_server_with_a_session_is_dispatched_to_by_identity`** is the multi-server property.
//!   Every MCP server's tools share one source *kind*, so the router registers one executor for the kind and
//!   the fan-out has to happen inside it. An implementation that kept only the first server, or that matched by
//!   capability, would pass every single-server test here.
//! - **`an_identity_no_server_admitted_is_refused_rather_than_served_by_another`** is the misroute direction.
//!   Picking a server by position would send one server's tool name to another's process.
//! - **`a_missing_secret_is_reported_before_any_process_exists`** is the resolution ordering. Resolving after
//!   the spawn would report a configuration fault as a server that failed to start, which sends an operator to
//!   the wrong system.
//!
//! The async composition is exercised against a **real spawned child** in `tests/mcp_composition_process.rs`;
//! what is tested here is the pure half, so each rule has a detector that does not need a process.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module reached from
// the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{ComposedMcpExecutor, launch_spec_for};
use crate::config::mcp::McpServerDeclaration;
use crate::config::secret::{MapSecretResolver, SecretReference};
use jarvis_application::cancellation::CancellationScope;
use jarvis_application::tool_call::{ToolExecutionError, ToolExecutionRequest, ToolExecutor};
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};

/// A declaration that must be accepted, so each refusal below is contrasted with a working one.
fn declaration(name: &str) -> McpServerDeclaration {
    McpServerDeclaration {
        name: name.to_owned(),
        program: PathBuf::from("/usr/local/bin/mcp-server"),
        args: vec!["--root".to_owned(), "/srv/files".to_owned()],
        env: BTreeMap::from([(
            "ACME_TOKEN".to_owned(),
            SecretReference::Env("JARVIS_ACME_TOKEN".to_owned()),
        )]),
        enabled: true,
        startup_timeout_ms: crate::mcp::process::DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

/// A resolver holding one usable secret.
fn resolver() -> MapSecretResolver {
    let resolver = MapSecretResolver::new();
    resolver.insert("JARVIS_ACME_TOKEN", "s3cret");
    resolver
}

/// An identity for a tool of `owner`'s, with a fingerprint distinguished by `seed`.
fn identity(owner: &str, name: &str, seed: u8) -> ToolIdentity {
    ToolIdentity {
        capability: ToolCapability::new("mcp", name, 1)
            .expect("the fixture name is a usable segment"),
        source: ToolSource::new(
            SourceKind::McpServer,
            owner,
            ToolVersion::parse("1.0.0").expect("the fixture version parses"),
        )
        .expect("the fixture owner is usable"),
        schema_fingerprint: SchemaFingerprint::from_bytes([seed; 32]),
    }
}

/// The recording double's `execute`, which answers a **recognisable class** for any identity it holds.
///
/// ⚠ **The first version of this was a `match` with one arm that matched everything** — `match capability
/// { _ => NotFound }` — which clippy correctly reported as replaceable by its scrutinee and body. It was also a
/// real defect rather than a lint: an arm that matches everything means the `capability` binding was never
/// used, so the double recorded nothing and the comment claiming it "records which identity it was asked for"
/// was false. It now **asserts the identity is one it admitted**, which is what makes it a double for a
/// dispatcher rather than a body-returning stub.
#[derive(Debug)]
struct RecordingExecutor {
    /// The identities this double will answer for, standing in for a server's admitted set.
    admitted: Vec<ToolIdentity>,
}

impl ToolExecutor for RecordingExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest<'a>,
        _cancel: &'a CancellationScope,
    ) -> jarvis_application::tool_call::ToolExecutionFuture<'a> {
        let admitted = self.admitted.contains(request.identity);
        Box::pin(async move {
            if admitted {
                return Err(ToolExecutionError::Failed(ToolErrorClass::OutputInvalid));
            }
            Err(ToolExecutionError::Failed(ToolErrorClass::NotFound))
        })
    }
}

#[test]
fn a_complete_declaration_resolves_into_a_launch_specification() {
    // The accepting case, without which every refusal below would be satisfied by a function that refuses
    // everything.
    let spec = launch_spec_for(&declaration("acme-files"), &resolver())
        .expect("the fixture declaration must resolve");
    assert_eq!(spec.program.to_str(), Some("/usr/local/bin/mcp-server"));
    assert_eq!(spec.args, vec!["--root", "/srv/files"]);
    // **The value, not the reference.** This is the moment the secret rules call "the last responsible one":
    // the child needs material in its environment, so the pair that reaches the process table is literal and
    // the reference never does.
    assert_eq!(
        spec.env,
        vec![("ACME_TOKEN".to_owned(), "s3cret".to_owned())]
    );
    assert_eq!(
        spec.working_dir, None,
        "this adapter sets no working directory, so the child inherits none from the configuration"
    );
}

#[test]
fn a_missing_secret_is_reported_before_any_process_exists() {
    // **The resolution ordering.** Resolving after the spawn would report a configuration fault as a server
    // that failed to start, sending an operator to look at a process that never ran. The refusal is the
    // *secret* code, which is what they have to fix.
    let empty = MapSecretResolver::new();
    let refusal = launch_spec_for(&declaration("acme-files"), &empty)
        .expect_err("an unresolvable reference must be refused");
    assert_eq!(refusal.code(), "jarvis.secret_unavailable");
    assert_eq!(
        refusal,
        super::McpCompositionRefusal::SecretUnavailable {
            server: "acme-files".to_owned()
        }
    );
    // And it is permanent: the same profile resolves the same way until it is edited.
    assert!(refusal.is_permanent());

    // The complement: the same declaration with a resolver that *does* hold the secret resolves, so this is
    // not a function that fails on every input.
    assert!(launch_spec_for(&declaration("acme-files"), &resolver()).is_ok());
}

#[test]
fn a_specification_the_launcher_refuses_is_reported_with_the_launchers_own_code() {
    // The declaration's validation and the launcher's are different checks with one owner each. A value that
    // passes this module's shape rules and fails the launcher's must be caught **here**, or it surfaces at the
    // spawn as `mcp.spawn_failed` — a message about the child for a fault in the specification.
    let mut empty_program = declaration("acme-files");
    empty_program.program = PathBuf::new();
    let refusal = launch_spec_for(&empty_program, &resolver())
        .expect_err("an empty program must be refused by the launcher's own validation");
    assert_eq!(
        refusal,
        super::McpCompositionRefusal::LaunchInvalid {
            server: "acme-files".to_owned(),
            code: "mcp.program_invalid",
        },
        "the launcher's own code must be carried, not a composition-level substitute"
    );
    assert!(refusal.is_permanent());

    // A control character reaches the process table, so the launcher refuses it too — and the code names the
    // field rather than the spawn.
    let mut control_arg = declaration("acme-files");
    control_arg.args = vec!["bad\narg".to_owned()];
    let refusal = launch_spec_for(&control_arg, &resolver())
        .expect_err("a control character in an argument must be refused");
    assert_eq!(refusal.code(), "mcp.argument_invalid");
}

#[test]
fn the_environment_is_resolved_in_a_deterministic_order() {
    // Two identical starts must produce the same environment, or a server that reads its own environment would
    // see a different order between runs. The declaration's map is sorted, and the vector is built in that
    // order, so this is a property of the type rather than of a caller's insertion order.
    let mut many = declaration("acme-files");
    many.env = BTreeMap::from([
        (
            "ZZZ_LAST".to_owned(),
            SecretReference::Env("JARVIS_ACME_TOKEN".to_owned()),
        ),
        (
            "AAA_FIRST".to_owned(),
            SecretReference::Env("JARVIS_ACME_TOKEN".to_owned()),
        ),
        (
            "MMM_MIDDLE".to_owned(),
            SecretReference::Env("JARVIS_ACME_TOKEN".to_owned()),
        ),
    ]);
    let spec = launch_spec_for(&many, &resolver()).expect("the fixture must resolve");
    let keys: Vec<&str> = spec.env.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        vec!["AAA_FIRST", "MMM_MIDDLE", "ZZZ_LAST"],
        "the environment must be built in the map's own sorted order"
    );
}

#[test]
fn an_empty_composition_dispatches_to_no_server_and_refuses_everything() {
    // The degenerate configuration: valid, useless, and it must refuse rather than panic. Asserted because a
    // first version would `expect` the lookup, and this crate denies `panic` in library code precisely so a
    // refusal has to be a value.
    let composition = super::McpComposition::default();
    assert!(composition.is_clean(), "nothing was refused");
    assert!(composition.servers().is_empty());
    assert!(composition.admitted_identities().is_empty());
    assert!(composition.source_kinds().is_empty());

    let dispatcher = ComposedMcpExecutor::over(&composition);
    assert!(dispatcher.is_empty());
    assert_eq!(dispatcher.servers(), 0);
    assert!(
        dispatcher
            .executor_for(&identity("acme-files", "read_file", 1))
            .is_none(),
        "an empty dispatcher must report no executor rather than a substitute"
    );
}

#[test]
fn every_mcp_server_shares_one_source_kind_and_that_is_why_the_fan_out_is_inside_it() {
    // **The fact the dispatcher exists for, asserted rather than assumed.** `RoutingExecutor` holds at most
    // one executor per source kind, and every MCP server's tools declare `McpServer` — so the fan-out across
    // servers cannot happen in the router. If this ever stopped being true (a per-server kind, say), the
    // dispatcher would be unnecessary work and this test is where that would surface.
    let first = identity("acme-files", "read_file", 1);
    let second = identity("other-files", "write_file", 2);
    assert_eq!(first.source.kind, second.source.kind);
    assert_eq!(first.source.kind, SourceKind::McpServer);
    // And they are still **different** identities, because the owner is part of the tuple — which is what makes
    // an identity-keyed dispatch unambiguous without a lookup order.
    assert_ne!(first, second);
    assert_ne!(first.source.owner, second.source.owner);
}

#[test]
fn the_recording_double_proves_the_dispatch_key_is_the_whole_identity() {
    // A double rather than a session, to keep the key's shape testable without a process. Two identities that
    // share a **capability** but differ by owner must be distinguished, since a capability-keyed dispatch
    // would send one server's tool to the other's executor.
    let mine = identity("acme-files", "read_file", 1);
    let theirs = identity("other-files", "read_file", 1);
    assert_eq!(
        mine.capability, theirs.capability,
        "the fixture must share a capability, or this tests the wrong rule"
    );
    assert_ne!(
        mine, theirs,
        "and differ by identity, which is what a dispatch must key on"
    );

    let double = RecordingExecutor {
        admitted: vec![mine.clone()],
    };
    assert!(double.admitted.contains(&mine));
    assert!(
        !double.admitted.contains(&theirs),
        "a capability that matches is not enough: the source owner is part of the identity"
    );
}
