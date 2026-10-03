//! Tests for the approval-prompt argument rows: what a person is shown about a call they are asked to allow.
//!
//! The claim is that the rows are **informative and safe**: the path and the start of the text appear, and
//! nothing the model wrote can break the prompt — control characters, length, nesting and volume are bounded.

use jarvis_domain::tool::call::ToolArguments;

use super::argument_rows;

fn rows(document: &str) -> Vec<(String, String)> {
    argument_rows(&ToolArguments::new(document).expect("arguments"))
}

#[test]
fn scalar_arguments_are_shown_with_a_key_prefix() {
    let shown = rows(r#"{"root":"notes","path":"a/b.txt","overwrite":true,"limit":5}"#);
    assert!(shown.contains(&("arg.root".to_owned(), "notes".to_owned())));
    assert!(shown.contains(&("arg.path".to_owned(), "a/b.txt".to_owned())));
    assert!(shown.contains(&("arg.overwrite".to_owned(), "true".to_owned())));
    assert!(shown.contains(&("arg.limit".to_owned(), "5".to_owned())));
}

#[test]
fn control_characters_and_length_are_bounded() {
    let long = "x".repeat(1000);
    let document = format!(r#"{{"content":"line one\nline\u001b[31m two","long":"{long}"}}"#);
    let shown = rows(&document);
    let content = &shown
        .iter()
        .find(|row| row.0 == "arg.content")
        .expect("content")
        .1;
    assert!(!content.chars().any(char::is_control));
    assert_eq!(content, "line one line [31m two");
    let cut = &shown
        .iter()
        .find(|row| row.0 == "arg.long")
        .expect("long")
        .1;
    assert_eq!(cut.chars().count(), 161);
    assert!(cut.ends_with('…'));
}

#[test]
fn nested_values_are_skipped_and_the_count_is_capped() {
    let shown = rows(r#"{"a":{"deep":1},"b":[1,2],"c":"kept"}"#);
    assert_eq!(shown, vec![("arg.c".to_owned(), "kept".to_owned())]);

    let many: Vec<String> = (0..20)
        .map(|index| format!(r#""k{index:02}":"v""#))
        .collect();
    assert_eq!(rows(&format!("{{{}}}", many.join(","))).len(), 8);
}

#[test]
fn a_non_object_document_shows_nothing() {
    assert!(rows("[1,2,3]").is_empty());
    assert!(rows("not json").is_empty());
}
