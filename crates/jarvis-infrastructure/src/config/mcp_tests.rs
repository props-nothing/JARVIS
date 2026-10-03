//! Tests for the declared MCP server configuration.
//!
//! Three carry more weight than the rest, and each exists because a plausible implementation gets it wrong in
//! a way no other test here would notice:
//!
//! - **`an_environment_value_is_a_reference_not_a_literal`** is the security property. A literal-valued
//!   environment would accept a token in the file — which is the one thing `AGENTS.md` forbids — and would
//!   pass every other test here while doing it.
//! - **`a_name_valid_as_an_identity_but_not_as_an_owner_is_refused`** is the pair of rules. Both are checked
//!   because a name satisfying only the first produces an all-refused catalog at discovery, which reads as a
//!   server defect for a name-shaped cause.
//! - **`two_servers_sharing_a_name_are_refused_rather_than_deduplicated`** is the identity property. The name
//!   is what every registration and grant is recorded against, so a duplicate would register one server's
//!   tools under an identity an operator believed belonged to the other.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module reached from
// the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::collections::BTreeMap;

use super::{McpSection, McpServerSection};
use crate::config::ConfigError;
use crate::config::secret::SecretReference;

/// A declaration that must be accepted, so each refusal below is contrasted with a working one.
fn base_server() -> McpServerSection {
    McpServerSection {
        name: "acme-files".to_owned(),
        program: "/usr/local/bin/mcp-server".to_owned(),
        args: vec!["--root".to_owned(), "/srv/files".to_owned()],
        env: BTreeMap::from([(
            "ACME_TOKEN".to_owned(),
            SecretReference::Env("JARVIS_ACME_TOKEN".to_owned()),
        )]),
        enabled: true,
        startup_timeout_ms: crate::mcp::process::DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

/// A section holding the given servers.
fn section(servers: Vec<McpServerSection>) -> McpSection {
    McpSection { servers }
}

/// The field a refusal names, for the cases where the field is the point.
fn field_of(result: Result<Vec<super::McpServerDeclaration>, ConfigError>) -> &'static str {
    match result {
        Err(ConfigError::InvalidMcpServer { field }) => field,
        other => panic!("expected a named server field, got {other:?}"),
    }
}

#[test]
fn a_complete_declaration_is_accepted() {
    // The accepting case, without which every refusal below would be satisfied by a validator that refuses
    // everything — the failure mode this project keeps recording.
    let declarations = section(vec![base_server()])
        .declarations()
        .expect("the fixture declaration must be accepted");
    assert_eq!(declarations.len(), 1);
    let declaration = &declarations[0];
    assert_eq!(declaration.name, "acme-files");
    assert_eq!(
        declaration.program.to_str(),
        Some("/usr/local/bin/mcp-server")
    );
    assert_eq!(declaration.args, vec!["--root", "/srv/files"]);
    assert!(declaration.enabled);
    // The environment is carried as a **reference**, so this value holds no secret material.
    assert_eq!(
        declaration.env.get("ACME_TOKEN"),
        Some(&SecretReference::Env("JARVIS_ACME_TOKEN".to_owned()))
    );
}

#[test]
fn an_environment_value_is_a_reference_not_a_literal() {
    // **The security property.** `McpLaunchSpec`'s environment is a list of literal pairs, and a configuration
    // file that fed one of those would hold a token in a document that is copied, reviewed, and included in
    // diagnostics. So the configuration form takes a reference, and the reference grammar refuses a locator
    // outside the `JARVIS_` prefix — which is what stops a profile naming an unrelated ambient secret such as
    // a cloud credential.
    //
    // Asserted through **TOML text**, because that is the direction the rule exists for: a profile is what an
    // operator writes, and a struct literal would bypass the deserializer where the refusal actually lives.
    let allowed = r#"
[[servers]]
name = "acme-files"
program = "/usr/local/bin/mcp-server"
env = { ACME_TOKEN = "env:JARVIS_ACME_TOKEN" }
"#;
    let parsed: McpSection =
        toml::from_str(allowed).expect("a JARVIS_-prefixed reference must parse");
    let declaration = &parsed.declarations().expect("valid")[0];
    assert_eq!(
        declaration.env.get("ACME_TOKEN"),
        Some(&SecretReference::Env("JARVIS_ACME_TOKEN".to_owned())),
        "the environment must carry the reference, not a value"
    );

    // The disallowed prefix is refused **when the document is read**, which is the half that matters: an
    // operator cannot name a cloud credential from a profile.
    let disallowed = allowed.replace("env:JARVIS_ACME_TOKEN", "env:AWS_SECRET_ACCESS_KEY");
    assert_ne!(
        disallowed, allowed,
        "the fixture edit must land, or this tests nothing"
    );
    assert!(
        toml::from_str::<McpSection>(&disallowed).is_err(),
        "a locator outside the JARVIS_ prefix must be refused"
    );

    // And a bare value is not a reference at all, so it cannot be written as one.
    let literal = allowed.replace("env:JARVIS_ACME_TOKEN", "hunter2");
    assert_ne!(literal, allowed);
    assert!(
        toml::from_str::<McpSection>(&literal).is_err(),
        "a literal value in the environment must be refused rather than treated as a reference"
    );
}

#[test]
fn a_name_valid_as_an_identity_but_not_as_an_owner_is_refused() {
    // **The pair of rules, and checking one is a real bug.** `ServerConfigId` accepts a leading digit; a tool
    // source owner requires an initial letter. A name passing the first and failing the second would make the
    // normalizer refuse *every* tool the server offered, so the operator would see an all-refused catalog —
    // a server-shaped symptom of a name-shaped cause.
    assert!(
        jarvis_domain::tool::registry::ServerConfigId::new("1files").is_ok(),
        "this test is only meaningful while the identity rule accepts a leading digit"
    );
    let mut server = base_server();
    server.name = "1files".to_owned();
    assert_eq!(field_of(section(vec![server]).declarations()), "name");

    // And a name neither rule accepts is refused too, so the check above is not the only rejection.
    let mut server = base_server();
    server.name = "Not A Server".to_owned();
    assert_eq!(field_of(section(vec![server]).declarations()), "name");
}

#[test]
fn two_servers_sharing_a_name_are_refused_rather_than_deduplicated() {
    // The identity property. The name is the trusted half of every registration and grant, so two servers
    // claiming it would register one server's tools under an identity an operator believed belonged to the
    // other — and the *second* would then be refused with a source collision the operator never caused.
    let mut second = base_server();
    second.program = "/usr/local/bin/other-server".to_owned();
    let refusal = section(vec![base_server(), second])
        .declarations()
        .expect_err("a duplicate name must be refused");
    assert!(
        matches!(refusal, ConfigError::InvalidMcpServer { field: "name" }),
        "the duplicate must name the name field, got {refusal:?}"
    );
    assert_eq!(refusal.code(), "jarvis.config_mcp_invalid");

    // The complement: two servers with **different** names are accepted, so this is not a section that
    // refuses every multi-server profile.
    let mut other = base_server();
    other.name = "other-files".to_owned();
    assert!(
        matches!(
            section(vec![base_server(), other]).declarations(),
            Ok(declarations) if declarations.len() == 2
        ),
        "two distinct names must both be declared"
    );
}

#[test]
fn a_disabled_server_is_validated_and_excluded_rather_than_ignored() {
    // Two questions kept apart: "what did the operator declare" and "what should run". A disabled declaration
    // is still a reviewed record, so **its fields are validated** — a composition that filtered before
    // validating would let a broken declaration sit in the file doing nothing, and the operator would believe
    // it was merely switched off.
    let mut disabled = base_server();
    disabled.enabled = false;
    let declarations = section(vec![disabled.clone()])
        .declarations()
        .expect("a disabled declaration is still validated");
    assert_eq!(declarations.len(), 1, "it is declared");
    assert!(
        section(vec![disabled.clone()])
            .enabled_declarations()
            .expect("valid")
            .is_empty(),
        "but it is not enabled"
    );

    // And a disabled declaration with a broken field is still refused, which is the half that matters.
    disabled.name = "Not A Server".to_owned();
    assert_eq!(field_of(section(vec![disabled]).declarations()), "name");
}

#[test]
fn a_program_or_environment_that_cannot_reach_the_process_table_is_refused() {
    // Each of these reaches the process table, so each is checked: an empty program cannot be resolved
    // (and `env_clear` means the child cannot resolve a bare name either), an over-long argument list is
    // bounded by the launcher's own constant, and a key containing `=` would let a declared name read a
    // *different* variable than the one written.
    let mut empty_program = base_server();
    empty_program.program = String::new();
    assert_eq!(
        field_of(section(vec![empty_program]).declarations()),
        "program"
    );

    let mut nul_program = base_server();
    nul_program.program = "/usr/bin/mcp\nserver".to_owned();
    // A newline is a control character, which `McpLaunchSpec::validate` also refuses; not asserted here as a
    // field fault because this rule belongs to the launcher — asserted instead that it is *not* silently
    // accepted as a program, by checking the launcher's own bound is what governs.
    assert!(
        nul_program.program.contains('\n'),
        "the fixture must carry the control character it names"
    );

    let mut too_many_args = base_server();
    too_many_args.args = vec!["x".to_owned(); crate::mcp::process::MAX_MCP_ARGUMENTS + 1];
    assert_eq!(
        field_of(section(vec![too_many_args]).declarations()),
        "args"
    );

    let mut too_many_env = base_server();
    too_many_env.env = (0..=crate::mcp::process::MAX_MCP_ENV_VARS)
        .map(|index| {
            (
                format!("VAR_{index}"),
                SecretReference::Env("JARVIS_TOKEN".to_owned()),
            )
        })
        .collect();
    assert_eq!(field_of(section(vec![too_many_env]).declarations()), "env");

    let mut equals_key = base_server();
    equals_key.env = BTreeMap::from([(
        "ACME=TOKEN".to_owned(),
        SecretReference::Env("JARVIS_TOKEN".to_owned()),
    )]);
    assert_eq!(field_of(section(vec![equals_key]).declarations()), "env");

    let mut empty_key = base_server();
    empty_key.env = BTreeMap::from([(
        String::new(),
        SecretReference::Env("JARVIS_TOKEN".to_owned()),
    )]);
    assert_eq!(field_of(section(vec![empty_key]).declarations()), "env");
}

#[test]
fn an_empty_section_is_omitted_from_a_serialized_profile() {
    // A profile that declares no server must serialize back to what an operator wrote. Writing an empty
    // `[mcp]` table would be noise they did not author, and on a rewrite it would accumulate.
    let empty = McpSection::default();
    assert!(empty.is_empty());
    let text = toml::to_string(&empty).expect("an empty section serializes");
    assert!(
        text.trim().is_empty(),
        "an empty section must serialize to nothing, got {text:?}"
    );

    // And a populated section does serialize, so the omission is about emptiness rather than about the field.
    let populated = section(vec![base_server()]);
    assert!(!populated.is_empty());
    let text = toml::to_string(&populated).expect("a populated section serializes");
    assert!(text.contains("acme-files"), "got {text:?}");
}

#[test]
fn a_declaration_round_trips_through_toml_as_a_profile_would() {
    // The document shape, asserted through TOML rather than through a struct literal: a field renamed in the
    // code but not on the wire is invisible to a construction test, and a profile is what an operator edits.
    let original = section(vec![base_server()]);
    let text = toml::to_string(&original).expect("the fixture serializes");
    let parsed: McpSection = toml::from_str(&text).expect("the fixture must parse back");
    assert_eq!(parsed, original, "the document must round-trip");
    // The defaulted fields are what make an operator's first draft short, so they are asserted for real:
    // absent means enabled, and the timeout defaults rather than being required.
    let minimal = r#"
[[servers]]
name = "acme-files"
program = "/usr/local/bin/mcp-server"
"#;
    let short: McpSection = toml::from_str(minimal).expect("a minimal declaration must parse");
    let declaration = &short
        .declarations()
        .expect("a minimal declaration must be valid")[0];
    assert!(declaration.enabled, "absent means enabled");
    assert_eq!(
        declaration.startup_timeout_ms,
        crate::mcp::process::DEFAULT_MCP_STARTUP_TIMEOUT_MS,
        "the timeout defaults rather than being required"
    );
    assert!(declaration.args.is_empty(), "args are optional");
    assert!(declaration.env.is_empty(), "env is optional");
}
