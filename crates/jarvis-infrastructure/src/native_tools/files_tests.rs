//! Tests for the file tools, against a real temporary directory.
//!
//! The claims that matter are the filesystem's own — a symlink must not lead out of a root, an existing
//! file must not be overwritten by accident — so these run against real files rather than a fake.

use std::path::{Path, PathBuf};

use jarvis_application::tool_call::ToolExecutionError;
use jarvis_domain::tool::call::{ContentBlock, ToolResultBody};
use jarvis_domain::tool::error_class::ToolErrorClass;

use super::{
    FileRoots, MAX_LIST_ENTRIES, MAX_WRITE_BYTES, RootDeclaration, RootError, RootInfo,
    definitions, execute,
};

/// A directory removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("jarvis-files-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&path).expect("the scratch directory is creatable");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn roots(docs: &Path, work: &Path) -> FileRoots {
    FileRoots::open(&[
        RootDeclaration {
            name: "docs".to_owned(),
            path: docs.to_path_buf(),
            writable: false,
        },
        RootDeclaration {
            name: "work".to_owned(),
            path: work.to_path_buf(),
            writable: true,
        },
    ])
    .expect("both roots open")
}

fn run(
    roots: &FileRoots,
    capability: &str,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolErrorClass> {
    match execute(roots, capability, &arguments.to_string()).expect("a file tool") {
        Ok(body) => Ok(json_of(&body)),
        Err(ToolExecutionError::Failed(class)) => Err(class),
        Err(other) => unreachable!("unexpected execution error {other:?}"),
    }
}

fn json_of(body: &ToolResultBody) -> serde_json::Value {
    let ContentBlock::Json { value } = &body.content[0] else {
        unreachable!("the file tools return one json block");
    };
    serde_json::from_str(value.as_str()).expect("the block parses")
}

#[test]
fn a_file_is_listed_read_and_written_inside_its_root() {
    let docs = Scratch::new();
    let work = Scratch::new();
    std::fs::write(docs.path().join("hello.txt"), "hello world").expect("seeds");
    std::fs::create_dir(docs.path().join("sub")).expect("seeds");
    let roots = roots(docs.path(), work.path());

    let listed = run(&roots, "files.list@1", &serde_json::json!({"root": "docs"})).expect("lists");
    let names: Vec<&str> = listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["hello.txt", "sub"]);

    let read = run(
        &roots,
        "files.read@1",
        &serde_json::json!({"root": "docs", "path": "hello.txt"}),
    )
    .expect("reads");
    assert_eq!(read["content"], "hello world");
    assert_eq!(read["truncated"], false);

    let written = run(
        &roots,
        "files.write@1",
        &serde_json::json!({"root": "work", "path": "note.txt", "content": "remember"}),
    )
    .expect("writes");
    assert_eq!(written["bytes_written"], 8);
    assert_eq!(
        std::fs::read_to_string(work.path().join("note.txt")).expect("on disk"),
        "remember"
    );
}

#[test]
fn a_path_that_leaves_the_root_by_shape_is_refused_before_the_filesystem_is_touched() {
    let docs = Scratch::new();
    let work = Scratch::new();
    std::fs::write(work.path().join("secret.txt"), "secret").expect("seeds");
    let roots = roots(docs.path(), work.path());
    for path in [
        "../secret.txt",
        "..",
        "/etc/passwd",
        "C:/Windows/win.ini",
        "a\\b",
        "CON",
        "x:stream",
        "a//b",
    ] {
        let outcome = run(
            &roots,
            "files.read@1",
            &serde_json::json!({"root": "docs", "path": path}),
        );
        assert_eq!(outcome, Err(ToolErrorClass::SchemaInvalid), "{path:?}");
    }
}

#[test]
fn an_unknown_root_and_a_read_only_root_are_permission_failures() {
    let docs = Scratch::new();
    let work = Scratch::new();
    let roots = roots(docs.path(), work.path());
    assert_eq!(
        run(
            &roots,
            "files.list@1",
            &serde_json::json!({"root": "elsewhere"})
        ),
        Err(ToolErrorClass::PermissionDenied),
        "a root the operator never declared"
    );
    assert_eq!(
        run(
            &roots,
            "files.write@1",
            &serde_json::json!({"root": "docs", "path": "x.txt", "content": "x"})
        ),
        Err(ToolErrorClass::PermissionDenied),
        "a read-only root cannot be written"
    );
    assert!(!docs.path().join("x.txt").exists());
}

#[test]
fn a_write_refuses_to_replace_a_file_unless_asked() {
    let docs = Scratch::new();
    let work = Scratch::new();
    std::fs::write(work.path().join("a.txt"), "original").expect("seeds");
    let roots = roots(docs.path(), work.path());
    let create = serde_json::json!({"root": "work", "path": "a.txt", "content": "new"});
    assert_eq!(
        run(&roots, "files.write@1", &create),
        Err(ToolErrorClass::Conflict)
    );
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).expect("kept"),
        "original"
    );

    let replace =
        serde_json::json!({"root": "work", "path": "a.txt", "content": "new", "overwrite": true});
    let done = run(&roots, "files.write@1", &replace).expect("replaces on request");
    assert_eq!(done["overwrote"], true);
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).expect("replaced"),
        "new"
    );
}

#[test]
fn a_write_never_creates_directories_and_is_bounded() {
    let docs = Scratch::new();
    let work = Scratch::new();
    let roots = roots(docs.path(), work.path());
    assert_eq!(
        run(
            &roots,
            "files.write@1",
            &serde_json::json!({"root": "work", "path": "missing/dir/a.txt", "content": "x"})
        ),
        Err(ToolErrorClass::NotFound)
    );
    assert!(!work.path().join("missing").exists());
    let big = "x".repeat(MAX_WRITE_BYTES + 1);
    assert_eq!(
        run(
            &roots,
            "files.write@1",
            &serde_json::json!({"root": "work", "path": "big.txt", "content": big})
        ),
        Err(ToolErrorClass::LimitExceeded)
    );
}

#[test]
fn a_read_is_bounded_and_a_cut_inside_a_character_is_not_a_binary_file() {
    let docs = Scratch::new();
    let work = Scratch::new();
    std::fs::write(docs.path().join("long.txt"), "é".repeat(100)).expect("seeds");
    std::fs::write(docs.path().join("bin.dat"), [0xff, 0xfe, 0x00, 0x01]).expect("seeds");
    let roots = roots(docs.path(), work.path());

    // 51 bytes lands in the middle of the 26th two-byte character.
    let cut = run(
        &roots,
        "files.read@1",
        &serde_json::json!({"root": "docs", "path": "long.txt", "max_bytes": 51}),
    )
    .expect("reads");
    assert_eq!(cut["truncated"], true);
    assert_eq!(cut["content"].as_str().expect("text").chars().count(), 25);

    assert_eq!(
        run(
            &roots,
            "files.read@1",
            &serde_json::json!({"root": "docs", "path": "bin.dat"})
        ),
        Err(ToolErrorClass::OutputInvalid),
        "a binary file is refused rather than returned as garbage"
    );
    assert_eq!(
        run(
            &roots,
            "files.read@1",
            &serde_json::json!({"root": "docs", "path": "nope.txt"})
        ),
        Err(ToolErrorClass::NotFound)
    );
}

#[test]
fn a_listing_is_bounded_and_says_so() {
    let docs = Scratch::new();
    let work = Scratch::new();
    for index in 0..(MAX_LIST_ENTRIES + 5) {
        std::fs::write(docs.path().join(format!("f{index:04}.txt")), "").expect("seeds");
    }
    let roots = roots(docs.path(), work.path());
    let listed = run(&roots, "files.list@1", &serde_json::json!({"root": "docs"})).expect("lists");
    assert_eq!(
        listed["entries"].as_array().expect("entries").len(),
        MAX_LIST_ENTRIES
    );
    assert_eq!(listed["truncated"], true);
}

#[test]
fn a_symlink_that_points_outside_the_root_is_not_followed() {
    // The property a check-then-open cannot give, and the reason `cap-std` is a dependency: the file the
    // link names exists and is readable by this process, and the tool still must not reach it.
    let docs = Scratch::new();
    let work = Scratch::new();
    let outside = Scratch::new();
    std::fs::write(outside.path().join("secret.txt"), "secret").expect("seeds");
    let link = docs.path().join("escape");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(outside.path(), &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(outside.path(), &link).is_ok();
    if !made {
        // Creating a symlink needs a privilege some Windows accounts lack; the claim is then unobservable
        // here and is recorded as such in the evidence note rather than passed silently.
        eprintln!("skipped: this account cannot create symlinks");
        return;
    }
    let roots = roots(docs.path(), work.path());
    let through = run(
        &roots,
        "files.read@1",
        &serde_json::json!({"root": "docs", "path": "escape/secret.txt"}),
    );
    assert!(
        through.is_err(),
        "a link out of the root must not be followed: {through:?}"
    );
    assert_ne!(through, Ok(serde_json::json!("secret")));
    let listed = run(
        &roots,
        "files.list@1",
        &serde_json::json!({"root": "docs", "path": "escape"}),
    );
    assert!(listed.is_err(), "nor listed through: {listed:?}");
}

#[test]
fn opening_refuses_a_missing_directory_and_a_repeated_name() {
    let docs = Scratch::new();
    let missing = docs.path().join("not-there");
    let declared = |name: &str, path: &Path| RootDeclaration {
        name: name.to_owned(),
        path: path.to_path_buf(),
        writable: false,
    };
    assert_eq!(
        FileRoots::open(&[declared("a", &missing)]).err(),
        Some(RootError::Unavailable)
    );
    assert_eq!(
        FileRoots::open(&[declared("a", docs.path()), declared("a", docs.path())]).err(),
        Some(RootError::DuplicateName)
    );
}

#[test]
fn definitions_offer_a_write_tool_only_when_a_root_can_be_written() {
    let read_only = [RootInfo {
        name: "docs".to_owned(),
        writable: false,
    }];
    let both = [
        RootInfo {
            name: "docs".to_owned(),
            writable: false,
        },
        RootInfo {
            name: "work".to_owned(),
            writable: true,
        },
    ];
    let capabilities = |roots: &[RootInfo]| -> Vec<String> {
        definitions(roots)
            .expect("definitions build")
            .iter()
            .map(|tool| tool.definition.identity.capability.to_string())
            .collect()
    };
    assert_eq!(capabilities(&read_only), ["files.list@1", "files.read@1"]);
    assert_eq!(
        capabilities(&both),
        ["files.list@1", "files.read@1", "files.write@1"]
    );

    // The model learns the roots from the purpose, and the identity does not depend on them.
    let first = definitions(&both).expect("builds");
    let second = definitions(&read_only).expect("builds");
    assert!(first[1].definition.purpose.contains("work (read/write)"));
    assert_eq!(
        first[1].definition.identity, second[1].definition.identity,
        "editing roots must not invalidate grants"
    );
}

#[test]
fn the_classification_matches_what_the_tools_do() {
    use jarvis_domain::tool::classification::{ApprovalHint, Effect, Risk};
    let tools = definitions(&[RootInfo {
        name: "w".to_owned(),
        writable: true,
    }])
    .expect("builds");
    let by = |capability: &str| {
        tools
            .iter()
            .find(|tool| tool.definition.identity.capability.to_string() == capability)
            .expect("present")
            .definition
            .clone()
    };
    for read in ["files.list@1", "files.read@1"] {
        let definition = by(read);
        assert_eq!(definition.effects, vec![Effect::ReadOnly]);
        assert_eq!(definition.risk, Risk::Low);
        assert_eq!(definition.default_approval, ApprovalHint::Allow);
    }
    let write = by("files.write@1");
    assert_eq!(write.effects, vec![Effect::Write]);
    assert_eq!(write.risk, Risk::Moderate);
    assert_eq!(
        write.default_approval,
        ApprovalHint::Ask,
        "writing always has a person in the loop unless the operator chose otherwise"
    );
}

#[cfg(windows)]
#[test]
fn a_directory_junction_that_points_outside_the_root_is_not_followed() {
    // Windows junctions need no privilege to create, so unlike a symlink this is observable on every
    // developer account — and a junction is exactly the reparse point the architecture names.
    let docs = Scratch::new();
    let work = Scratch::new();
    let outside = Scratch::new();
    std::fs::write(outside.path().join("secret.txt"), "secret").expect("seeds");
    let link = docs.path().join("escape");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .is_ok_and(|output| output.status.success());
    assert!(made, "mklink /J must be available to run this claim");
    let roots = roots(docs.path(), work.path());
    let through = run(
        &roots,
        "files.read@1",
        &serde_json::json!({"root": "docs", "path": "escape/secret.txt"}),
    );
    assert!(
        through.is_err(),
        "a junction out of the root must not be followed: {through:?}"
    );
    let listed = run(
        &roots,
        "files.list@1",
        &serde_json::json!({"root": "docs", "path": "escape"}),
    );
    assert!(listed.is_err(), "nor listed through: {listed:?}");
    let written = run(
        &roots,
        "files.write@1",
        &serde_json::json!({"root": "work", "path": "ok.txt", "content": "fine"}),
    );
    assert!(
        written.is_ok(),
        "an ordinary write in another root still works"
    );
}
