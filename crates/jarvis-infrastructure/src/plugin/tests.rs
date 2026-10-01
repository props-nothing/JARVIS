//! Tests for the process plugin manifest contract.
//!
//! The first test parses the **contract's own example** — read from the document, not checked in as a
//! copy — so the type and the contract cannot drift. The rest are the field rules, each exercised by the
//! value it exists to refuse, because a rule whose only test uses a value it accepts is a rule no test
//! covers.

use super::{
    MAX_PLUGIN_MANIFEST_BYTES, MAX_PLUGIN_STARTUP_TIMEOUT_MS, PluginManifest, PluginManifestError,
    is_capability_token, is_environment_name, is_package_relative_path, is_reserved_device_name,
};
// The two rules that moved to the domain: the manifest consults them rather than holding its own copy,
// so these tests exercise the **domain** helpers the manifest delegates to. That the manifest uses them
// is asserted by the `PluginManifest::parse` tests below, not by the helper tests here.
use jarvis_domain::plugin::{is_canonical_package_digest, is_plugin_identifier};

/// The repository root, from this crate's manifest directory.
fn repository_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the crate lives two levels under the repository root")
        .to_path_buf()
}

/// The contract's manifest example, as the document states it, verbatim.
///
/// **Extracted from the document rather than copied**, so editing the example without editing the type
/// fails the build — the property "no drift" means. The fence is the first `json` block in the
/// document, which is the manifest example; its brace-balanced body is taken verbatim.
///
/// It is returned **raw**, placeholders included, because one of its values is a documented placeholder
/// and a test that asserts the placeholder is what keeps the contract and this fixture from drifting
/// apart silently. Use [`valid_example`] when a value that must parse is needed.
fn contract_example() -> String {
    let text = std::fs::read_to_string(repository_root().join("docs/contracts/plugin-manifest.md"))
        .expect("the plugin manifest contract reads");
    let start = text
        .find("```json")
        .expect("the contract carries a JSON example");
    let body = &text[start + "```json".len()..];
    let end = body.find("```").expect("the example fence closes");
    body[..end].trim().to_owned()
}

/// The contract's example with its **documented placeholder** filled, so it parses.
///
/// The example writes `"digest": "sha256:..."`, and the contract says the placeholder `$schema` URL "is
/// replaced by a generated repository schema before plugin implementation" — the digest placeholder is
/// the same kind of thing: a value the document cannot spell because a real one is produced per package.
/// Substituting it is asserted to have happened, so if the contract stops using that placeholder this
/// fails rather than silently parsing a different document. Only this one value is substituted; every
/// other byte is the contract's.
fn valid_example() -> String {
    let raw = contract_example();
    let placeholder = r#""digest": "sha256:...""#;
    assert!(
        raw.contains(placeholder),
        "the contract's example must carry the documented digest placeholder",
    );
    raw.replace(
        placeholder,
        &format!(r#""digest": "sha256:{}""#, "ab".repeat(32)),
    )
}

#[test]
fn the_contracts_own_example_parses_and_validates() {
    // **The document is the source of truth, so its example must be a value this type accepts.** This is
    // the golden-fixture half of the contract tests, applied to the one document this module owns: a
    // field the example carries that the type does not model fails here rather than at a plugin's
    // install.
    let example = valid_example();
    let manifest =
        PluginManifest::parse(example.as_bytes()).expect("the contract's example must parse");
    assert_eq!(manifest.id, "example.research-runtime");
    assert_eq!(manifest.publisher, "example.org");
    assert_eq!(manifest.schema_version, "1.0.0");
    assert_eq!(manifest.compatibility.protocol.kind, "jarvis-runtime");
    assert_eq!(manifest.entrypoint.executable, "bin/example-runtime");
    assert_eq!(manifest.resource_limits.startup_timeout_ms, 10_000);
    // The `$schema` URL the example carries is modelled rather than refused as an unknown field — the
    // regression this guards is a `deny_unknown_fields` that rejects the contract's own document.
    assert!(
        manifest
            .schema_url
            .as_deref()
            .is_some_and(|url| url.ends_with("plugin-manifest-v1.schema.json")),
        "the example's $schema URL must be carried: {:?}",
        manifest.schema_url,
    );
}

#[test]
fn an_unknown_top_level_field_is_refused() {
    // The contract: "Unknown top-level fields fail until a compatible schema version defines extension
    // behavior." A field silently ignored is a manifest claiming something JARVIS did not read.
    let example = valid_example();
    let with_extra = example.replacen('{', r#"{"unexpected": true,"#, 1);
    assert_eq!(
        PluginManifest::parse(with_extra.as_bytes()),
        Err(PluginManifestError::Malformed),
    );
}

#[test]
fn an_unsupported_schema_version_is_refused_by_name() {
    let example = valid_example().replace(
        r#""schema_version": "1.0.0""#,
        r#""schema_version": "2.0.0""#,
    );
    assert_eq!(
        PluginManifest::parse(example.as_bytes()),
        Err(PluginManifestError::UnsupportedSchemaVersion {
            found: "2.0.0".to_owned(),
        }),
        "a future schema is refused rather than read optimistically",
    );
    assert_eq!(
        PluginManifestError::UnsupportedSchemaVersion {
            found: "2.0.0".to_owned(),
        }
        .code(),
        "jarvis.plugin_schema_unsupported",
    );
}

#[test]
fn an_oversized_document_is_refused_before_it_is_parsed() {
    // The bound is checked before serde, so a hostile document cannot exhaust memory first.
    let huge = vec![b'{'; usize::try_from(MAX_PLUGIN_MANIFEST_BYTES).expect("fits") + 1];
    assert_eq!(
        PluginManifest::parse(&huge),
        Err(PluginManifestError::TooLarge),
    );
}

#[test]
fn an_executable_that_escapes_the_package_is_refused() {
    // **The rule the contract's entrypoint section turns on.** Each case is a distinct way two strings
    // name one file, or one string leaves the package. A manifest that named any of these would have the
    // extractor or the launcher escape the verified installation root.
    let example = valid_example();
    for (executable, why) in [
        ("../outside/runtime", "a parent traversal"),
        ("/usr/bin/runtime", "an absolute path"),
        ("C:runtime", "a Windows drive colon"),
        ("bin/rt.exe:stream", "an alternate data stream"),
        ("bin/con", "a reserved device name"),
        ("bin/rt.", "a trailing dot Windows strips"),
        ("bin/rt ", "a trailing space Windows strips"),
        ("bin//rt", "an empty segment"),
        ("bin/./rt", "a current-directory segment"),
        // A backslash traversal, written so the **JSON** carries the escape: the JSON text is
        // `bin\\..\\escape`, which decodes to `bin\..\escape`, and the rule splits on both separators.
        (r"bin\\..\\escape", "a backslash traversal"),
    ] {
        let document = example.replace(
            r#""executable": "bin/example-runtime""#,
            &format!(r#""executable": "{executable}""#),
        );
        assert_ne!(
            document, example,
            "the fixture must actually replace the executable",
        );
        assert_eq!(
            PluginManifest::parse(document.as_bytes()),
            Err(PluginManifestError::UnsafePath {
                field: "entrypoint.executable",
            }),
            "{executable} must be refused: {why}",
        );
    }
}

#[test]
fn a_legitimate_package_relative_path_is_accepted() {
    // The positive control. Without it the refusals above would pass against a rule that refused every
    // path — and a nested package-relative executable is the ordinary case the rule must allow.
    for path in [
        "bin/runtime",
        "bin/sub/runtime.exe",
        "rt",
        "a.b/c-d_1/runtime",
    ] {
        assert!(
            is_package_relative_path(path),
            "{path} is a legitimate package-relative path",
        );
    }
}

#[test]
fn a_digest_without_the_algorithm_prefix_is_refused() {
    // A bare digest could be any algorithm; the prefix is what makes two digests comparable, the same
    // rule `SchemaFingerprint` records.
    assert!(is_canonical_package_digest(&format!(
        "sha256:{}",
        "ab".repeat(32)
    )));
    for (digest, why) in [
        ("ab".repeat(32), "no algorithm prefix"),
        (format!("sha512:{}", "ab".repeat(32)), "the wrong algorithm"),
        (format!("sha256:{}", "AB".repeat(32)), "uppercase hex"),
        (format!("sha256:{}", "ab".repeat(31)), "too short"),
        ("sha256:".to_owned(), "empty digest"),
        (
            "sha256:".to_owned() + &"zz".repeat(32),
            "non-hex characters",
        ),
    ] {
        assert!(
            !is_canonical_package_digest(&digest),
            "must be refused: {why}"
        );
        // Built from the **raw** example, because this test is about the digest itself and the raw
        // document carries the documented placeholder for exactly this substitution.
        let example = contract_example().replace(
            r#""digest": "sha256:...""#,
            &format!(r#""digest": "{digest}""#),
        );
        assert_eq!(
            PluginManifest::parse(example.as_bytes()),
            Err(PluginManifestError::InvalidDigest),
            "a manifest carrying {why} must be refused",
        );
    }
}

#[test]
fn an_unsupported_protocol_kind_is_refused_by_name() {
    // The contract supports exactly the JARVIS runtime protocol and MCP stdio; anything else "requires
    // an accepted contract and evidence note", so it is refused rather than carried as an opaque string.
    // The contract writes this block compactly (`{"kind":"jarvis-runtime",`), so the search uses the
    // exact text rather than a spaced form — and the assertion below fails loudly if it ever moves.
    let example = valid_example().replace(
        r#""kind":"jarvis-runtime""#,
        r#""kind":"some-other-protocol""#,
    );
    assert_ne!(
        example,
        valid_example(),
        "the fixture must replace the protocol kind"
    );
    assert_eq!(
        PluginManifest::parse(example.as_bytes()),
        Err(PluginManifestError::UnsupportedProtocol {
            kind: "some-other-protocol".to_owned(),
        }),
    );
}

#[test]
fn a_resource_limit_of_zero_or_above_its_ceiling_is_refused() {
    // A zero limit is a refusal wearing a limit's name, and an unbounded one is the hang the limit
    // exists to prevent.
    for (value, label) in [
        ("0", "zero startup timeout"),
        (
            &(MAX_PLUGIN_STARTUP_TIMEOUT_MS + 1).to_string(),
            "an over-ceiling startup timeout",
        ),
    ] {
        let example = valid_example().replace(
            r#""startup_timeout_ms": 10000"#,
            &format!(r#""startup_timeout_ms": {value}"#),
        );
        assert_eq!(
            PluginManifest::parse(example.as_bytes()),
            Err(PluginManifestError::LimitOutOfRange {
                field: "resource_limits.startup_timeout_ms",
            }),
            "{label} must be refused",
        );
    }
    // And a zero captured-stream bound is refused the same way.
    let example = valid_example().replace(r#""stdout_bytes": 1048576"#, r#""stdout_bytes": 0"#);
    assert_eq!(
        PluginManifest::parse(example.as_bytes()),
        Err(PluginManifestError::LimitOutOfRange {
            field: "resource_limits.stdout_bytes",
        }),
    );
}

#[test]
fn a_configuration_schema_that_is_not_an_object_is_refused() {
    // A schema is an object; an array or scalar would let a manifest claim a configuration shape it
    // cannot have, and a reader that walked it would find `[]` where it expected fields.
    for value in ["[]", "\"schema\"", "42", "null"] {
        let example = valid_example().replace(
            r#""configuration_schema": {}"#,
            &format!(r#""configuration_schema": {value}"#),
        );
        assert_eq!(
            PluginManifest::parse(example.as_bytes()),
            Err(PluginManifestError::ConfigurationSchemaNotAnObject),
            "a {value} configuration schema must be refused",
        );
    }
}

#[test]
fn an_identity_field_that_is_not_a_slug_is_refused() {
    // The id and publisher are the source identity — the display name never identifies a plugin — so they
    // are held to a slug rule rather than accepting free text.
    assert!(is_plugin_identifier("example.research-runtime"));
    for (value, why) in [
        ("Example.Runtime", "uppercase"),
        ("example", "no namespace dot"),
        ("example..runtime", "an empty segment"),
        (".example", "a leading dot"),
        ("example.", "a trailing dot"),
        ("example runtime", "a space"),
        ("example\truntime", "a control character"),
    ] {
        assert!(!is_plugin_identifier(value), "must be refused: {why}");
    }
    // And the publisher is held to the same rule, so a manifest cannot smuggle free text through it.
    let example = valid_example().replace(
        r#""publisher": "example.org""#,
        r#""publisher": "example\u0000org""#,
    );
    assert_eq!(
        PluginManifest::parse(example.as_bytes()),
        Err(PluginManifestError::InvalidIdentity { field: "publisher" }),
    );
}

#[test]
fn an_environment_entry_that_is_not_a_variable_name_is_refused() {
    // Names, never values — and an entry that is not a name is a value or a shell fragment that would
    // reach a spawn.
    assert!(is_environment_name("PATH"));
    assert!(is_environment_name("JARVIS_HOME"));
    assert!(is_environment_name("_X1"));
    for (name, why) in [
        ("path", "lowercase"),
        ("1PATH", "starts with a digit"),
        ("PATH=evil", "an assignment"),
        ("PA TH", "a space"),
        ("PATH\nevil", "a newline"),
    ] {
        assert!(!is_environment_name(name), "must be refused: {why}");
    }
}

#[test]
fn reserved_device_names_are_matched_on_the_stem() {
    // The stem check is the whole point: `con.txt` is the device on Windows while `console.log` is an
    // ordinary file that merely begins with the same letters.
    for device in [
        "con",
        "CON",
        "con.txt",
        "AUX.tar.gz",
        "nul",
        "com1.log",
        "LPT9",
    ] {
        assert!(is_reserved_device_name(device), "{device} is a device name");
    }
    for ordinary in [
        "console.log",
        "auxiliary.txt",
        "connection",
        "null",
        "com10",
    ] {
        assert!(
            !is_reserved_device_name(ordinary),
            "{ordinary} is an ordinary file name",
        );
    }
}

#[test]
fn a_control_character_in_a_display_name_is_refused() {
    // The name is text an operator reads, so a newline would corrupt a terminal or a log line — the same
    // reason preview items refuse control characters.
    let example = valid_example().replace(
        r#""name": "Example Research Runtime""#,
        r#""name": "Example\nRuntime""#,
    );
    assert_eq!(
        PluginManifest::parse(example.as_bytes()),
        Err(PluginManifestError::EmptyField { field: "name" }),
    );
}

#[test]
fn a_capability_token_is_bounded_and_namespaced() {
    // Capabilities are compared against a grant, so a value carrying control characters or free text is
    // refused rather than stored beside one.
    for good in [
        "runtime.text",
        "jarvis.tools.read:selected",
        "network:api.example.org",
        "artifacts.write",
    ] {
        assert!(is_capability_token(good), "{good} is a capability token");
    }
    for bad in ["", "Runtime.Text", "runtime text", "runtime\ntext"] {
        assert!(!is_capability_token(bad), "{bad} must be refused");
    }
}

#[test]
fn every_error_variant_has_a_distinct_non_empty_code() {
    // **A code is what an operator acts on**, so two variants sharing one would make two different
    // failures indistinguishable — the defect the tool contract's closed set exists to prevent. The list
    // is written by hand so a new variant without a code fails here rather than reaching a surface.
    let codes = [
        PluginManifestError::TooLarge.code(),
        PluginManifestError::Malformed.code(),
        PluginManifestError::UnsupportedSchemaVersion {
            found: String::new(),
        }
        .code(),
        PluginManifestError::InvalidIdentity { field: "id" }.code(),
        PluginManifestError::EmptyField { field: "name" }.code(),
        PluginManifestError::InvalidDigest.code(),
        PluginManifestError::UnsafePath {
            field: "entrypoint.executable",
        }
        .code(),
        PluginManifestError::InvalidVersionRange.code(),
        PluginManifestError::UnsupportedProtocol {
            kind: String::new(),
        }
        .code(),
        PluginManifestError::InvalidPlatform {
            platform: String::new(),
        }
        .code(),
        PluginManifestError::TooManyArguments.code(),
        PluginManifestError::UnsafeArgument.code(),
        PluginManifestError::TooManyEnvironmentNames.code(),
        PluginManifestError::InvalidEnvironmentName {
            name: String::new(),
        }
        .code(),
        PluginManifestError::TooManyCapabilities.code(),
        PluginManifestError::InvalidCapability {
            capability: String::new(),
        }
        .code(),
        PluginManifestError::LimitOutOfRange {
            field: "resource_limits.message_bytes",
        }
        .code(),
        PluginManifestError::ConfigurationSchemaNotAnObject.code(),
    ];
    assert_eq!(codes.len(), 18, "every variant is listed");
    let mut sorted: Vec<&str> = codes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        codes.len(),
        "no two variants may share a code: {sorted:?}",
    );
    for code in codes {
        assert!(
            code.starts_with("jarvis.") && code.len() > "jarvis.".len(),
            "{code} must be a namespaced, non-empty code",
        );
    }
}
