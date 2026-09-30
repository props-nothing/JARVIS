//! Tests for the bounded schema validator.
//!
//! Four of these carry more weight than the rest, and each exists because a plausible
//! implementation gets it wrong in a way no other test here would notice:
//!
//! - **`a_schema_using_an_unimplemented_keyword_is_refused_rather_than_ignored`** is the module's
//!   whole reason for refusing at load. The specification says an unrecognised keyword SHOULD be
//!   treated as an annotation, so the compliant-looking implementation is the one that **silently
//!   validates less than the schema asked for** — the fail-open direction.
//! - **`one_and_one_point_zero_are_the_same_number_to_enum_and_const`** pins the specification's
//!   equality rule against `serde_json`'s, which does not follow it. A validator that used the
//!   derived comparison would reject a document the schema accepts.
//! - **`a_reference_cycle_is_refused_as_too_deep_rather_than_recursing_for_ever`** is the property
//!   both specifications require by name ("Validators MUST NOT fall into an infinite loop"), and it
//!   is the one failure mode here that cannot be observed by reading a return value.
//! - **`a_violation_names_the_path_and_never_the_instance_value`** keeps untrusted argument text out
//!   of anything an operator or an audit will read.

// The workspace denies `clippy::panic` because a panic in production code is a runtime hazard, and
// `clippy.toml`'s `allow-expect-in-tests`/`allow-unwrap-in-tests` do **not** cover it here — those
// settings classify a `tests/*.rs` integration crate, while this is a `#[path]` module reached from
// the library, which the lint sees as production. In a test a panic *is* the failure report, and
// asserting a mismatched enum variant with `panic!("... got {other:?}")` reports which variant
// arrived, where `unreachable!` would wrongly claim the branch could not be reached.
#![allow(clippy::panic)]

use super::{
    ArgumentViolation, DIALECT, MAX_SCHEMA_BYTES, MAX_SCHEMA_DEPTH, MAX_SCHEMA_STEPS,
    SchemaRejection, ToolSchema,
};
use jarvis_domain::tool::call::{MAX_ARGUMENT_BYTES, ToolArguments};

/// Parses a schema that must be accepted.
fn schema(text: &str) -> ToolSchema {
    ToolSchema::parse(text).expect("the schema must be usable")
}

/// Decides a document that must be accepted.
fn accepts(text: &str, document: &str) {
    let result = schema(text).validate(&arguments(document));
    assert!(result.is_ok(), "expected acceptance, got {result:?}");
}

/// Returns the violation a document must produce.
fn refuses(text: &str, document: &str) -> ArgumentViolation {
    schema(text)
        .validate(&arguments(document))
        .expect_err("the document must be refused")
}

/// Wraps an argument document, which the domain type has already bounded.
fn arguments(document: &str) -> ToolArguments {
    ToolArguments::new(document).expect("a usable argument document")
}

// ---------------------------------------------------------------------------------------
// Load-time refusals.
// ---------------------------------------------------------------------------------------

#[test]
fn a_schema_using_an_unimplemented_keyword_is_refused_rather_than_ignored() {
    // The compliant-looking implementation ignores this and validates nothing, so the test is
    // written to fail for the *reason* the module exists rather than for any refusal at all.
    for (keyword, value) in [
        ("pattern", "\"^a+$\""),
        ("patternProperties", "{\"^a\":{\"type\":\"string\"}}"),
        ("format", "\"date-time\""),
        ("contentEncoding", "\"base64\""),
        ("contentMediaType", "\"image/png\""),
        ("multipleOf", "2"),
        ("contains", "{\"type\":\"string\"}"),
        ("minContains", "1"),
        ("maxContains", "2"),
        ("unevaluatedProperties", "false"),
        ("unevaluatedItems", "false"),
        ("dependentRequired", "{\"a\":[\"b\"]}"),
        ("dependentSchemas", "{\"a\":{\"required\":[\"b\"]}}"),
        ("$id", "\"https://example.com/x\""),
        ("$anchor", "\"x\""),
        ("$dynamicAnchor", "\"x\""),
        ("$dynamicRef", "\"#x\""),
        ("$vocabulary", "{}"),
        ("definitions", "{}"),
        ("dependencies", "{}"),
        ("additionalItems", "{}"),
    ] {
        let text = format!("{{\"type\":\"object\",\"{keyword}\":{value}}}");
        match ToolSchema::parse(&text) {
            Err(SchemaRejection::UnsupportedKeyword { keyword: named, .. }) => {
                assert_eq!(named, keyword, "the refusal must name the keyword at fault");
            }
            other => panic!("{keyword} must be refused as unsupported, got {other:?}"),
        }
    }
}

#[test]
fn a_keyword_no_implementation_defines_is_refused_as_unknown_rather_than_as_unsupported() {
    // The two refusals mean different things — a capability this build lacks versus a schema it
    // cannot interpret — so the variants must not be interchangeable.
    match ToolSchema::parse("{\"maxLenght\":3}") {
        Err(SchemaRejection::UnknownKeyword { keyword, .. }) => assert_eq!(keyword, "maxLenght"),
        other => panic!("expected an unknown-keyword refusal, got {other:?}"),
    }
}

#[test]
fn an_unresolved_reference_is_refused_at_load_rather_than_at_call() {
    for text in [
        "{\"$ref\":\"#/$defs/absent\"}",
        // The case where `$defs` EXISTS and does not hold the name. Without this one the existence
        // check is unexercised: every other input here is refused because the whole `$defs` object
        // is missing, which a validator that never looked inside would also refuse.
        "{\"$ref\":\"#/$defs/absent\",\"$defs\":{\"present\":{\"type\":\"object\"}}}",
        "{\"$ref\":\"#/absent\"}",
        "{\"$ref\":\"https://example.com/x\"}",
        "{\"$ref\":\"#/$defs/a/b\"}",
        "{\"$ref\":\"\"}",
    ] {
        match ToolSchema::parse(text) {
            Err(SchemaRejection::UnresolvedReference { .. }) => {}
            other => panic!("{text} must be refused as unresolved, got {other:?}"),
        }
    }
    // And the resolving case must be accepted, or the refusals above succeed for the wrong reason.
    let resolving = "{\"$ref\":\"#/$defs/a\",\"$defs\":{\"a\":{\"type\":\"object\"}}}";
    assert!(ToolSchema::parse(resolving).is_ok());
    // A `$defs` member that is not a schema is refused too, since `$defs` members are walked.
    match ToolSchema::parse("{\"$defs\":{\"a\":[]}}") {
        Err(SchemaRejection::NotASchema { .. }) => {}
        other => panic!("a non-schema member must be refused, got {other:?}"),
    }
}

#[test]
fn a_schema_that_is_not_a_schema_is_refused() {
    for text in ["[]", "\"text\"", "7", "null", "true"] {
        let outcome = ToolSchema::parse(text);
        if text == "true" {
            // A boolean schema is a schema: `true` accepts everything and is legal (Core §4.3).
            assert!(outcome.is_ok(), "a boolean schema must be accepted");
        } else {
            assert!(
                matches!(outcome, Err(SchemaRejection::NotASchema { .. })),
                "{text} is not a schema, got {outcome:?}"
            );
        }
    }
}

#[test]
fn a_malformed_keyword_value_is_refused_rather_than_defaulted() {
    for text in [
        "{\"minLength\":-1}",
        "{\"minLength\":\"3\"}",
        "{\"maxItems\":1.5}",
        "{\"type\":\"numberish\"}",
        "{\"type\":[]}",
        "{\"type\":[\"string\",\"string\"]}",
        "{\"required\":[]}",
        "{\"required\":[\"a\",\"a\"]}",
        "{\"enum\":[]}",
        "{\"allOf\":[]}",
        "{\"uniqueItems\":\"true\"}",
        "{\"properties\":[]}",
        "{\"additionalProperties\":7}",
    ] {
        match ToolSchema::parse(text) {
            Err(SchemaRejection::MalformedKeyword { .. }) => {}
            other => panic!("{text} must be refused as malformed, got {other:?}"),
        }
    }
}

#[test]
fn an_oversized_schema_is_refused_before_it_is_parsed() {
    let mut text = String::from("{\"title\":\"");
    text.push_str(&"a".repeat(MAX_SCHEMA_BYTES));
    text.push_str("\"}");
    match ToolSchema::parse(&text) {
        Err(SchemaRejection::TooLarge { max, .. }) => assert_eq!(max, MAX_SCHEMA_BYTES),
        other => panic!("expected an oversized refusal, got {other:?}"),
    }
}

#[test]
fn another_dialect_is_refused_rather_than_interpreted_as_this_one() {
    let other = "{\"$schema\":\"https://json-schema.org/draft-07/schema#\"}";
    match ToolSchema::parse(other) {
        Err(SchemaRejection::UnsupportedDialect { found }) => {
            assert!(
                found.contains("draft-07"),
                "the refusal must name the dialect found"
            );
        }
        outcome => panic!("expected a dialect refusal, got {outcome:?}"),
    }
    assert!(ToolSchema::parse(&format!("{{\"$schema\":\"{DIALECT}\"}}")).is_ok());
}

#[test]
fn a_schema_nested_past_the_depth_bound_is_refused_at_load() {
    // A schema needs roughly two containers per level (`{"allOf":[` is an object and an array), so
    // the depth bound must sit well below the parser's own limit or it could never fire — the
    // document would be refused as "not JSON" first. This test asserts that ordering, so lowering
    // MAX_SCHEMA_DEPTH past the parser's limit fails here instead of silently making the bound dead.
    let levels = MAX_SCHEMA_DEPTH + 4;
    assert!(
        levels * 2 < 128,
        "the depth bound must fire before the JSON parser's container limit is reached",
    );
    let mut text = String::new();
    for _ in 0..levels {
        text.push_str("{\"not\":");
    }
    text.push_str("{}");
    for _ in 0..levels {
        text.push('}');
    }
    match ToolSchema::parse(&text) {
        Err(SchemaRejection::TooDeep { .. }) => {}
        other => panic!("expected a depth refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------
// Keywords: acceptance and refusal, on both sides of each boundary.
// ---------------------------------------------------------------------------------------

#[test]
fn type_accepts_each_name_and_refuses_the_others() {
    let accepted = [
        ("\"null\"", "null"),
        ("\"boolean\"", "true"),
        ("\"object\"", "{}"),
        ("\"array\"", "[]"),
        ("\"number\"", "1.5"),
        ("\"string\"", "\"x\""),
        ("\"integer\"", "3"),
        ("\"integer\"", "3.0"),
    ];
    for (name, document) in accepted {
        accepts(&format!("{{\"type\":{name}}}"), document);
    }
    // The boundary the specification states explicitly: an integer "matches any number with a zero
    // fractional part", so a fractional number is not an integer and a bare integer is a number.
    assert_eq!(
        refuses("{\"type\":\"integer\"}", "1.5").keyword(),
        Some("type")
    );
    assert_eq!(
        refuses("{\"type\":\"string\"}", "3").keyword(),
        Some("type")
    );
    // A union admits what either member admits and refuses what neither does.
    accepts("{\"type\":[\"string\",\"null\"]}", "null");
    accepts("{\"type\":[\"string\",\"null\"]}", "\"x\"");
    assert_eq!(
        refuses("{\"type\":[\"string\",\"null\"]}", "3").keyword(),
        Some("type")
    );
}

#[test]
fn one_and_one_point_zero_are_the_same_number_to_enum_and_const() {
    // Core §4.2.2: numbers are equal when they "have the same mathematical value", and "mere
    // formatting differences ... (trailing zeros) are insignificant". `serde_json`'s own `PartialEq`
    // does not follow that rule, so this fails against a validator that reused it.
    accepts("{\"enum\":[1.0]}", "1");
    accepts("{\"enum\":[1]}", "1.0");
    accepts("{\"const\":2.0}", "2");
    accepts("{\"const\":2}", "2.0");
    // The negative side, so the two assertions above are not satisfied by "always equal".
    assert_eq!(refuses("{\"enum\":[1]}", "2").keyword(), Some("enum"));
    assert_eq!(refuses("{\"const\":1}", "2").keyword(), Some("const"));
    // Objects and arrays compare structurally, not by reference or by key order.
    accepts("{\"const\":{\"b\":1,\"a\":2}}", "{\"a\":2,\"b\":1}");
    accepts("{\"const\":[1,[2]]}", "[1,[2]]");
    assert_eq!(
        refuses("{\"const\":{\"a\":1}}", "{\"a\":1,\"b\":2}").keyword(),
        Some("const")
    );
}

#[test]
fn a_numeric_bound_is_compared_against_its_own_inclusive_or_exclusive_side() {
    accepts("{\"minimum\":3}", "3");
    assert_eq!(refuses("{\"minimum\":3}", "2.9").keyword(), Some("minimum"));
    accepts("{\"exclusiveMinimum\":3}", "3.1");
    assert_eq!(
        refuses("{\"exclusiveMinimum\":3}", "3").keyword(),
        Some("exclusiveMinimum")
    );
    accepts("{\"maximum\":3}", "3");
    assert_eq!(refuses("{\"maximum\":3}", "3.1").keyword(), Some("maximum"));
    accepts("{\"exclusiveMaximum\":3}", "2.9");
    assert_eq!(
        refuses("{\"exclusiveMaximum\":3}", "3").keyword(),
        Some("exclusiveMaximum")
    );
    // A numeric bound does not constrain a non-number (Core §7.6.1).
    accepts("{\"maximum\":3}", "\"not a number\"");
}

#[test]
fn a_length_bound_counts_characters_rather_than_bytes() {
    // "The length of a string instance is defined as the number of its characters" (Validation
    // §6.3.1). A byte-counting implementation refuses this four-character string.
    accepts("{\"maxLength\":4}", "\"\u{e9}\u{e9}\u{e9}\u{e9}\"");
    assert_eq!(
        refuses("{\"maxLength\":3}", "\"\u{e9}\u{e9}\u{e9}\u{e9}\"").keyword(),
        Some("maxLength")
    );
    accepts("{\"minLength\":4}", "\"\u{e9}\u{e9}\u{e9}\u{e9}\"");
    assert_eq!(
        refuses("{\"minLength\":5}", "\"\u{e9}\u{e9}\u{e9}\u{e9}\"").keyword(),
        Some("minLength")
    );
    accepts("{\"maxLength\":3}", "7");
}

#[test]
fn unique_items_is_decided_by_the_specifications_equality_rule() {
    accepts("{\"uniqueItems\":true}", "[1,2,3]");
    accepts("{\"uniqueItems\":false}", "[1,1]");
    // 1 and 1.0 are the same number, so this array is not unique.
    assert_eq!(
        refuses("{\"uniqueItems\":true}", "[1,1.0]").keyword(),
        Some("uniqueItems")
    );
    assert_eq!(
        refuses("{\"uniqueItems\":true}", "[{\"a\":1},{\"a\":1}]").keyword(),
        Some("uniqueItems")
    );
}

#[test]
fn items_covers_exactly_the_items_prefix_items_did_not() {
    // Core §10.3.1.2 makes `items` depend on its SIBLING's length. A validator that applied `items`
    // to every element, or to none, passes a test with only one of the two keywords present.
    let text = "{\"prefixItems\":[{\"type\":\"integer\"}],\"items\":{\"type\":\"string\"}}";
    accepts(text, "[1,\"a\",\"b\"]");
    assert_eq!(refuses(text, "[\"a\",\"b\"]").keyword(), Some("type"));
    assert_eq!(refuses(text, "[1,\"a\",2]").keyword(), Some("type"));
    // With no sibling `prefixItems`, `items` covers everything.
    assert_eq!(
        refuses("{\"items\":{\"type\":\"string\"}}", "[1]").keyword(),
        Some("type")
    );
    // A lone `prefixItems` constrains only the prefix and not the length. The third element is a
    // later *string*, which the prefix schema's `type` would refuse — so this acceptance is what
    // shows `prefixItems` stops at its own length rather than covering the whole array.
    accepts(
        "{\"prefixItems\":[{\"type\":\"integer\"}]}",
        "[1,\"a\",\"b\"]",
    );
    assert_eq!(
        refuses("{\"prefixItems\":[{\"type\":\"integer\"}]}", "[\"a\",1]").keyword(),
        Some("type"),
    );
}

#[test]
fn additional_properties_excludes_what_a_sibling_properties_declared() {
    // Core §10.3.2.3 defines this in terms of a sibling, so a validator that ignored the sibling
    // would refuse a declared property — the wrong refusal of a valid call.
    let text = "{\"properties\":{\"known\":{\"type\":\"integer\"}},\"additionalProperties\":false}";
    accepts(text, "{\"known\":1}");
    assert_eq!(
        refuses(text, "{\"known\":1,\"extra\":2}").keyword(),
        Some("false")
    );
    // The declared property is still validated by `properties`, not by `additionalProperties`.
    assert_eq!(
        refuses(text, "{\"known\":\"not an integer\"}").keyword(),
        Some("type")
    );
    // `properties` alone leaves undeclared names unconstrained.
    accepts(
        "{\"properties\":{\"known\":{\"type\":\"integer\"}}}",
        "{\"other\":\"x\"}",
    );
}

#[test]
fn required_names_every_missing_property_rather_than_only_the_first() {
    let text = "{\"required\":[\"a\",\"b\"]}";
    accepts(text, "{\"a\":1,\"b\":2}");
    assert_eq!(refuses(text, "{\"a\":1}").keyword(), Some("required"));
    assert_eq!(refuses(text, "{}").keyword(), Some("required"));
    // A property count bound is a different question from presence.
    accepts("{\"minProperties\":1}", "{\"a\":1}");
    assert_eq!(
        refuses("{\"minProperties\":2}", "{\"a\":1}").keyword(),
        Some("minProperties")
    );
    assert_eq!(
        refuses("{\"maxProperties\":1}", "{\"a\":1,\"b\":2}").keyword(),
        Some("maxProperties")
    );
}

#[test]
fn property_names_constrains_every_key() {
    // `propertyNames` applies the subschema to the names, so the schema sees a string.
    let text = "{\"propertyNames\":{\"maxLength\":2}}";
    accepts(text, "{\"ab\":1,\"c\":2}");
    assert_eq!(refuses(text, "{\"abc\":1}").keyword(), Some("maxLength"));
}

#[test]
fn the_logic_applicators_combine_their_subschemas_results() {
    let base = "{\"type\":\"integer\"}";
    accepts(&format!("{{\"allOf\":[{base}]}}"), "1");
    assert_eq!(
        refuses(&format!("{{\"allOf\":[{base},{{\"minimum\":5}}]}}"), "1").keyword(),
        Some("allOf")
    );
    accepts(
        &format!("{{\"anyOf\":[{base},{{\"type\":\"string\"}}]}}"),
        "\"x\"",
    );
    // `anyOf` admits a document that satisfies every branch; `oneOf` does not.
    let overlapping = "{\"anyOf\":[true,true]}";
    accepts(overlapping, "1");
    assert_eq!(
        refuses("{\"oneOf\":[true,true]}", "1").keyword(),
        Some("oneOf")
    );
    accepts("{\"oneOf\":[true,false]}", "1");
    assert_eq!(refuses("{\"not\":{}}", "1").keyword(), Some("not"));
    accepts("{\"not\":{\"type\":\"string\"}}", "1");
}

#[test]
fn a_conditional_applies_its_branch_and_ignores_one_without_its_if() {
    let text =
        "{\"if\":{\"type\":\"integer\"},\"then\":{\"minimum\":5},\"else\":{\"type\":\"string\"}}";
    accepts(text, "5");
    assert_eq!(refuses(text, "4").keyword(), Some("minimum"));
    accepts(text, "\"x\"");
    assert_eq!(refuses(text, "true").keyword(), Some("type"));
    // "when 'if' is not present, both 'then' and 'else' MUST be entirely ignored" (Core §10.2.2).
    accepts("{\"then\":{\"minimum\":5}}", "1");
    accepts("{\"else\":{\"minimum\":5}}", "1");
}

#[test]
fn a_reference_applies_the_definition_it_names() {
    let text = "{\"properties\":{\"a\":{\"$ref\":\"#/$defs/positive\"}},\
                \"$defs\":{\"positive\":{\"type\":\"integer\",\"exclusiveMinimum\":0}}}";
    accepts(text, "{\"a\":1}");
    assert_eq!(
        refuses(text, "{\"a\":0}").keyword(),
        Some("exclusiveMinimum")
    );
    assert_eq!(refuses(text, "{\"a\":\"x\"}").keyword(), Some("type"));
}

#[test]
fn a_boolean_schema_accepts_or_refuses_everything() {
    // Core §4.3.2: `true` is the empty schema, `false` always fails.
    accepts("true", "1");
    accepts("true", "{\"anything\":[1,2,3]}");
    assert_eq!(refuses("false", "1").keyword(), Some("false"));
    assert_eq!(refuses("false", "null").keyword(), Some("false"));
    // The empty object is equivalent to `true`, which is the property the equivalence rests on.
    accepts("{}", "1");
}

// ---------------------------------------------------------------------------------------
// The budget, the depth, and the disclosure rule.
// ---------------------------------------------------------------------------------------

#[test]
fn a_reference_cycle_is_refused_as_too_deep_rather_than_recursing_for_ever() {
    // Both specifications require this by name: "Validators MUST NOT fall into an infinite loop"
    // (Core §13) and "A schema MUST NOT be run into an infinite loop against an instance"
    // (Core §9.4.1). A cycle consumes no instance, so only the depth bound stops it — and a
    // validator without one does not fail here, it exhausts the stack.
    let text = "{\"$ref\":\"#/$defs/loop\",\"$defs\":{\"loop\":{\"$ref\":\"#/$defs/loop\"}}}";
    let outcome = schema(text).validate(&arguments("1"));
    assert_eq!(outcome, Err(ArgumentViolation::TooDeep));
    // A deep but finite chain of references that stays inside the bound must still be accepted, or
    // the refusal above would be satisfied by a validator that refused every reference. The chain
    // is half the bound, so it is deep enough to be a real chain and shallow enough not to trip it.
    //
    // Built with `serde_json` rather than by concatenating braces: a hand-built document that is one
    // brace short is refused as `NotJson`, which looks identical to a bound refusing it.
    let chain = MAX_SCHEMA_DEPTH / 2;
    let mut definitions = serde_json::Map::new();
    for step in 0..chain {
        definitions.insert(
            format!("d{step}"),
            serde_json::json!({ "$ref": format!("#/$defs/d{}", step + 1) }),
        );
    }
    definitions.insert(
        format!("d{chain}"),
        serde_json::json!({ "type": "integer" }),
    );
    let document = serde_json::json!({
        "$ref": "#/$defs/d0",
        "$defs": definitions,
    })
    .to_string();
    assert!(
        ToolSchema::parse(&document).is_ok(),
        "the chain must be loadable, so the cycle refusal is about the bound: {:?}",
        ToolSchema::parse(&document).err(),
    );
    assert!(schema(&document).validate(&arguments("1")).is_ok());
}

#[test]
fn unique_items_exhausting_the_step_budget_is_reported_as_a_budget_refusal() {
    // A bounded INPUT does not bound the WORK: this array is well inside the argument byte bound
    // and its uniqueness scan is quadratic. The refusal must name the budget rather than claim the
    // caller's arguments violate the schema — that would blame them for this validator's own limit.
    let count = MAX_SCHEMA_STEPS / 256 + 64;
    let items: Vec<String> = (0..count).map(|index| index.to_string()).collect();
    let document = format!("[{}]", items.join(","));
    assert!(
        document.len() < MAX_ARGUMENT_BYTES,
        "the trigger must be inside the argument bound or it is not testing the step budget",
    );
    assert!(
        count * (count - 1) / 2 > MAX_SCHEMA_STEPS,
        "the fixture must actually exhaust the budget, or the refusal is unreachable",
    );
    let outcome = schema("{\"uniqueItems\":true}").validate(&arguments(&document));
    match outcome {
        Err(ArgumentViolation::StepsExhausted { max }) => assert_eq!(max, MAX_SCHEMA_STEPS),
        other => panic!("expected a step-budget refusal, got {other:?}"),
    }
    // The other half: a small array with duplicates is refused as a KEYWORD violation, so the
    // budget refusal above cannot be satisfied by a validator that refuses everything large.
    assert_eq!(
        refuses("{\"uniqueItems\":true}", "[1,1]").keyword(),
        Some("uniqueItems")
    );
}

#[test]
fn a_violation_names_the_path_and_never_the_instance_value() {
    let text = "{\"properties\":{\"secret\":{\"maxLength\":3}}}";
    let outcome = refuses(text, "{\"secret\":\"hunter2-is-a-credential\"}");
    let ArgumentViolation::Keyword { keyword, path } = outcome else {
        panic!("expected a keyword violation");
    };
    assert_eq!(keyword, "maxLength");
    assert_eq!(path, "/secret");
    // The value must appear in neither the path nor the rendered violation: the arguments are
    // untrusted model output, and echoing them moves them across a trust boundary for no gain.
    assert!(!path.contains("hunter2"));
    let rendered = format!(
        "{}",
        ArgumentViolation::Keyword {
            keyword,
            path: path.clone(),
        }
    );
    assert!(!rendered.contains("hunter2"));
}

#[test]
fn an_instance_nested_past_the_parsers_own_limit_is_refused_as_unparseable() {
    // The INSTANCE bound, which is a different bound from the schema depth: this module never sees a
    // value more deeply nested than `serde_json` admits, because parsing refuses it first. Measured:
    // the parser refuses at 128 containers, so 200 array levels is past it while 100 is inside.
    //
    // The pair matters. Asserting only that the deep document is refused would pass against a
    // validator that refused every array; asserting that the shallower one is ACCEPTED is what shows
    // the boundary belongs to the parser and not to this module's own depth bound.
    let nested = |depth: usize| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
    assert!(
        schema("{}").validate(&arguments(&nested(100))).is_ok(),
        "100 levels is inside the parser's limit, so it must be validated rather than refused",
    );
    match schema("{}").validate(&arguments(&nested(200))) {
        Err(ArgumentViolation::Unparseable) => {}
        other => panic!("expected an unparseable refusal, got {other:?}"),
    }
}

#[test]
fn an_unparseable_document_is_refused_as_such_rather_than_as_a_keyword_violation() {
    let outcome = schema("{\"type\":\"object\"}").validate(&arguments("{not json"));
    assert_eq!(outcome, Err(ArgumentViolation::Unparseable));
    assert_eq!(outcome.map_err(|error| error.keyword()), Err(None));
}

// ---------------------------------------------------------------------------------------
// The schema value itself.
// ---------------------------------------------------------------------------------------

#[test]
fn the_fingerprint_is_computed_over_the_text_that_was_parsed() {
    // The fingerprint is what a tool's identity is bound to, so it must describe the rules that
    // were actually validated against. Two schemas differing only in a bound must differ; the same
    // text must fingerprint the same.
    let first = schema("{\"maxLength\":3}");
    let second = schema("{\"maxLength\":4}");
    assert_ne!(first.fingerprint(), second.fingerprint());
    assert_eq!(
        *first.fingerprint(),
        *schema("{\"maxLength\":3}").fingerprint()
    );
    // And it must be the same value the infrastructure's schema hasher produces for these bytes,
    // so that the identity recorded here is the identity an approval was recorded against.
    assert_eq!(
        *first.fingerprint(),
        crate::tool_fingerprint::schema_fingerprint_of("{\"maxLength\":3}")
    );
}

#[test]
fn the_debug_form_reports_the_size_and_the_fingerprint_rather_than_the_document() {
    // A schema can come from a remote server, so a derived `Debug` would put untrusted bytes into
    // any log line that formats a value holding one.
    let rendered = format!("{:?}", schema("{\"title\":\"do not log me\"}"));
    assert!(rendered.contains("bytes"));
    assert!(rendered.contains("sha256:"));
    assert!(!rendered.contains("do not log me"));
}

#[test]
fn a_definition_whose_stated_fingerprint_is_another_schema_is_refused() {
    // The property that makes this more than an unused accessor: a definition may not carry one
    // schema's fingerprint while a different schema decides what it accepts. `ACC-024` is about an
    // approval not transferring to a replacement, and two disagreeing values are one way to get
    // there without the identity ever changing.
    let stated = schema("{\"maxLength\":3}");
    assert!(stated.confirms(stated.fingerprint()).is_ok());
    let other = schema("{\"maxLength\":4000}");
    let mismatch = other
        .confirms(stated.fingerprint())
        .expect_err("a different schema must not confirm the stated fingerprint");
    assert_ne!(mismatch.stated, mismatch.computed);
    // Both values must appear, or an operator cannot tell which schema was reviewed.
    assert!(mismatch.to_string().contains(&mismatch.stated));
    assert!(mismatch.to_string().contains(&mismatch.computed));
}

#[test]
fn the_accessors_describe_the_parsed_text() {
    let text = "{\"type\":\"object\"}";
    let parsed = schema(text);
    assert_eq!(parsed.text(), text);
    assert_eq!(parsed.len(), text.len());
    assert!(!parsed.is_empty());
}
