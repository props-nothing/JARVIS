//! Tests for rooted filesystem grants and the lexical path rules.
//!
//! Most of these exist because of **one plausible mistake**: comparing containment as a string prefix.
//! `/data/notsecret` starts with `/data/note` and is a sibling directory, so a prefix test authorizes a
//! path the grant never covered. Every containment test therefore uses a fixture where the prefix and
//! the segment comparison disagree, and a test that only used `/data/notes/file` under a `/data/notes`
//! root would pass against the broken implementation.
//!
//! The shape rules are asserted one rule at a time rather than through a single "bad path" case,
//! because each rule defends a different platform behaviour and a combined case would let one rule's
//! removal go unnoticed.

use super::path_grant::{
    MAX_PATH_BYTES, MAX_PATH_SEGMENTS, PathComparison, PathDenial, PathGrant, PathMode,
    WorkspaceRelativePath, authorize_path,
};

// ---------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------

fn path(value: &str) -> WorkspaceRelativePath {
    WorkspaceRelativePath::parse(value).expect("the fixture path is normalized")
}

fn grant(root: &str, modes: &[PathMode]) -> PathGrant {
    PathGrant::new(path(root), modes.to_vec()).expect("the fixture grant is valid")
}

fn read_only(root: &str) -> PathGrant {
    grant(root, &[PathMode::Read])
}

// ---------------------------------------------------------------------------------------
// Containment: the segment-wise rule.
// ---------------------------------------------------------------------------------------

#[test]
fn a_path_under_a_root_is_contained_and_a_sibling_that_shares_a_string_prefix_is_not() {
    // **The test the module is written around.** `data/notsecret/file.md` and `data/notes/file.md`
    // share the string prefix `data/note`, and they are different directories. A prefix test allows
    // both; the segment-wise comparison allows one. The fixture is chosen so the two rules disagree —
    // a test using `data/notes/deeper/file.md` would pass against the broken implementation.
    let root = path("data/notes");
    assert!(
        path("data/notes/file.md").is_under(&root, PathComparison::CaseSensitive),
        "a file directly in the root is contained",
    );
    assert!(
        path("data/notes/deeper/file.md").is_under(&root, PathComparison::CaseSensitive),
        "a descendant is contained",
    );
    assert!(
        !path("data/notsecret/file.md").is_under(&root, PathComparison::CaseSensitive),
        "a sibling sharing the string prefix `data/note` must NOT be contained",
    );
    // And a **true** descendant one level up IS contained, so the refusal above is about the segment
    // boundary rather than about the fixture being wrong. The first version of this assertion used
    // `data/notesecret` under a `data/note` root, which is a *sibling* — so the check whose whole
    // purpose was to prove the fixture meaningful was itself a prefix match, and it failed.
    assert!(
        path("data/note/deeper/file.md")
            .is_under(&path("data/note"), PathComparison::CaseSensitive),
        "a genuine descendant must be contained, so the sibling refusal is not a broken fixture",
    );
}

#[test]
fn a_root_is_contained_in_itself() {
    // A root names a directory, and the directory is inside itself. Refusing this would make the root
    // itself unreadable, which is never what a grant means.
    let root = path("data/notes");
    assert!(root.is_under(&root, PathComparison::CaseSensitive));
}

#[test]
fn a_path_shorter_than_the_root_is_not_contained() {
    // The boundary in the other direction: `data` is not under `data/notes`. A loop that ran out of
    // path segments before root segments would otherwise fall through and answer `true`.
    assert!(!path("data").is_under(&path("data/notes"), PathComparison::CaseSensitive));
    assert!(!path("other").is_under(&path("data/notes"), PathComparison::CaseSensitive));
}

#[test]
fn containment_follows_the_comparisons_case_rule_in_both_directions() {
    // **The comparison is an input because the filesystem decides it.** Asserted both ways on the same
    // pair, because a check that hardcoded either answer would be wrong on one platform — and on
    // Windows, assuming case-sensitive lets a path evade a grant by changing one letter, which is the
    // "case differences" case the architecture names.
    let root = path("Data/Notes");
    let candidate = path("data/notes/file.md");
    assert!(
        candidate.is_under(&root, PathComparison::CaseInsensitive),
        "on a case-insensitive filesystem these are the same directory",
    );
    assert!(
        !candidate.is_under(&root, PathComparison::CaseSensitive),
        "on a case-sensitive filesystem they are not",
    );
}

#[test]
fn a_case_only_difference_is_distinguishable_from_being_outside_every_root() {
    // The two facts need different user actions: a case typo means correct a letter, while being
    // outside every root means request access. The predicate is what makes the distinction possible.
    let root = path("data/notes");
    assert!(path("Data/Notes/file.md").differs_only_by_case_from(&root));
    // A path that differs by more than case is not a case mismatch, or every refusal would be reported
    // as one and the distinction would carry no information.
    assert!(!path("other/notes/file.md").differs_only_by_case_from(&root));
    assert!(!path("data/other/file.md").differs_only_by_case_from(&root));
    // And an exact prefix is not a case difference.
    assert!(!path("data/notes/file.md").differs_only_by_case_from(&root));
}

// ---------------------------------------------------------------------------------------
// Shape rules, one at a time.
// ---------------------------------------------------------------------------------------

#[test]
fn a_parent_directory_segment_is_refused_rather_than_resolved() {
    // Resolving `..` lexically is possible and would give the right answer for a purely lexical path,
    // but it discards the fact that the caller asked to leave the directory — and once symlinks exist
    // the lexical answer and the filesystem's answer differ. Refusing keeps the decision independent of
    // the filesystem, which is what makes it testable at all.
    for value in [
        "../etc/passwd",
        "data/../../etc/passwd",
        "data/notes/../../secret",
        "..",
        "data/..",
    ] {
        assert_eq!(
            WorkspaceRelativePath::denial_for(value),
            Some(PathDenial::Traversal),
            "{value:?} must be refused as traversal",
        );
    }
    // The refusal is a **shape** refusal, so a new grant could never make it allowed and a caller must
    // not offer "request access" for it.
    assert!(PathDenial::Traversal.is_shape());
}

#[test]
fn an_absolute_path_is_refused_and_reported_as_absolute() {
    // Reported specifically rather than as a malformed segment, because a user who supplied `/etc/passwd`
    // needs to learn that absolute paths are not accepted rather than that something was malformed.
    for value in [
        "/etc/passwd",
        "/",
        "//server/share",
        "C:notes.md",
        r"C:\notes.md",
    ] {
        let denial = WorkspaceRelativePath::denial_for(value);
        assert!(
            matches!(
                denial,
                Some(PathDenial::Absolute | PathDenial::AmbiguousSeparator)
            ),
            "{value:?} must be refused as absolute or ambiguous, got {denial:?}",
        );
    }
    assert_eq!(
        WorkspaceRelativePath::denial_for("/etc/passwd"),
        Some(PathDenial::Absolute),
    );
    assert_eq!(
        WorkspaceRelativePath::denial_for("C:notes.md"),
        Some(PathDenial::Absolute),
    );
}

#[test]
fn a_unnormalized_path_is_refused_so_one_file_has_one_spelling() {
    // `a//b`, `./a`, and `a/./b` are spellings of one file, and accepting them means two strings denote
    // one resource — so the containment check could be satisfied by a spelling the root does not cover.
    for value in [
        "data//notes",
        "./data",
        "data/./notes",
        "data/notes/",
        "data/",
        ".",
    ] {
        assert_eq!(
            WorkspaceRelativePath::denial_for(value),
            Some(PathDenial::NotNormalized),
            "{value:?} must be refused as un-normalized",
        );
    }
}

#[test]
fn a_backslash_is_refused_because_it_is_a_separator_on_only_one_platform() {
    // On Windows `a\b` is a directory; on Unix it is one filename containing a backslash. Accepting it
    // would mean one string names two different resources depending on the platform.
    assert_eq!(
        WorkspaceRelativePath::denial_for(r"data\notes"),
        Some(PathDenial::AmbiguousSeparator),
    );
    assert_eq!(
        WorkspaceRelativePath::denial_for(r"..\..\etc"),
        Some(PathDenial::AmbiguousSeparator),
        "the separator rule is checked before traversal, so the reason names the ambiguity",
    );
}

#[test]
fn a_reserved_device_name_is_refused_whatever_its_extension_or_case() {
    // Windows resolves these in any directory and with any extension, so a check comparing the whole
    // segment or the exact case would let `con.txt` and `CON` through on the one platform where they
    // matter.
    for value in [
        "CON",
        "con",
        "data/CON",
        "data/con.txt",
        "data/AUX.tar.gz",
        "data/nul",
        "data/COM1",
        "data/lpt9.log",
    ] {
        assert_eq!(
            WorkspaceRelativePath::denial_for(value),
            Some(PathDenial::ReservedName),
            "{value:?} must be refused as a reserved device name",
        );
    }
    // And a name that merely *contains* a reserved word is fine, so the rule is about the stem rather
    // than about a substring — otherwise ordinary files like `console.log` would be unusable.
    assert!(WorkspaceRelativePath::denial_for("data/console.log").is_none());
    assert!(WorkspaceRelativePath::denial_for("data/conartist.md").is_none());
    assert!(WorkspaceRelativePath::denial_for("data/nullable.md").is_none());
}

#[test]
fn a_colon_in_a_segment_is_refused_as_an_alternate_data_stream() {
    // `notes.md:secret` is a second file behind one name on Windows, so accepting it would let a path
    // read content the grant's root never covered.
    assert_eq!(
        WorkspaceRelativePath::denial_for("data/notes.md:secret"),
        Some(PathDenial::AlternateDataStream),
    );
    assert_eq!(
        WorkspaceRelativePath::denial_for("data/a:b/c"),
        Some(PathDenial::AlternateDataStream),
    );
}

#[test]
fn a_trailing_dot_or_space_is_refused_because_windows_strips_it() {
    // Windows silently removes a trailing dot or space, so `notes.md ` and `notes.md` are one file with
    // two spellings — and the stripped form can differ from the grant's root.
    for value in [
        "data/notes.md.",
        "data/notes.md ",
        "data/notes. ",
        "data/.hidden ",
    ] {
        assert_eq!(
            WorkspaceRelativePath::denial_for(value),
            Some(PathDenial::TrailingDotOrSpace),
            "{value:?} must be refused: Windows would strip the trailing character",
        );
    }
    // A dot *inside* a name is ordinary, so a rule matching any dot would refuse `notes.md`.
    assert!(WorkspaceRelativePath::denial_for("data/notes.md").is_none());
    // And a leading dot is a hidden file, which is a legitimate name on every platform.
    assert!(WorkspaceRelativePath::denial_for("data/.hidden").is_none());
}

#[test]
fn a_malformed_segment_is_refused() {
    assert_eq!(
        WorkspaceRelativePath::denial_for("data/notes\u{0}"),
        Some(PathDenial::MalformedSegment),
    );
    assert_eq!(
        WorkspaceRelativePath::denial_for("data/notes\nevil"),
        Some(PathDenial::MalformedSegment),
    );
    assert_eq!(
        WorkspaceRelativePath::denial_for(""),
        Some(PathDenial::MalformedSegment)
    );
    assert_eq!(
        WorkspaceRelativePath::denial_for(&"a".repeat(MAX_PATH_BYTES + 1)),
        Some(PathDenial::MalformedSegment),
    );
    // The segment count is bounded separately from the byte length, since many short segments reach the
    // count limit before the byte limit.
    let deep: Vec<&str> = std::iter::repeat_n("a", MAX_PATH_SEGMENTS + 1).collect();
    assert_eq!(
        WorkspaceRelativePath::denial_for(&deep.join("/")),
        Some(PathDenial::MalformedSegment),
    );
    // And a name longer than a filesystem permits is refused, so the failure is a typed refusal rather
    // than an OS error from an operation the caller believed was authorized.
    assert_eq!(
        WorkspaceRelativePath::denial_for(&format!("data/{}", "n".repeat(256))),
        Some(PathDenial::MalformedSegment),
    );
}

#[test]
fn a_well_formed_path_is_accepted_and_reports_no_denial() {
    // The positive control for every rule above: if the shape check refused everything, each of those
    // tests would pass for the wrong reason.
    for value in [
        "data/notes/file.md",
        "notes.md",
        "a/b/c/d/e.txt",
        "data/2026-09-27 report.md",
        "data/.hidden",
        "data/file.tar.gz",
    ] {
        assert_eq!(
            WorkspaceRelativePath::denial_for(value),
            None,
            "{value:?} must be accepted",
        );
        assert!(WorkspaceRelativePath::parse(value).is_ok());
    }
}

#[test]
fn a_shape_refusal_is_distinguishable_from_a_permission_refusal() {
    // The distinction a caller needs: a shape refusal can never be resolved by a new grant, so offering
    // "request access" for one would offer to grant something ungrantable.
    for shape in [
        PathDenial::Absolute,
        PathDenial::Traversal,
        PathDenial::NotNormalized,
        PathDenial::AmbiguousSeparator,
        PathDenial::ReservedName,
        PathDenial::AlternateDataStream,
        PathDenial::TrailingDotOrSpace,
        PathDenial::MalformedSegment,
    ] {
        assert!(shape.is_shape(), "{shape} must classify as a shape refusal");
    }
    for permission in [
        PathDenial::OutsideEveryRoot,
        PathDenial::ModeNotGranted,
        PathDenial::CaseMismatch,
    ] {
        assert!(
            !permission.is_shape(),
            "{permission} is about a grant rather than about the path's shape",
        );
    }
}

// ---------------------------------------------------------------------------------------
// Authorization: containment, mode, and the ordering.
// ---------------------------------------------------------------------------------------

#[test]
fn a_read_grant_authorizes_a_read_inside_its_root() {
    // The positive control for the authorization tests below.
    let grants = vec![read_only("data/notes")];
    let decision = authorize_path(
        "data/notes/file.md",
        PathMode::Read,
        &grants,
        PathComparison::CaseSensitive,
    );
    assert!(decision.is_allowed(), "{decision:?}");
    assert_eq!(decision.denial(), None);
}

#[test]
fn a_path_outside_every_root_is_refused_with_the_permission_reason() {
    // Distinct from a shape refusal, because the user's action differs: this one *can* be resolved by a
    // grant.
    let grants = vec![read_only("data/notes")];
    let decision = authorize_path(
        "data/other/file.md",
        PathMode::Read,
        &grants,
        PathComparison::CaseSensitive,
    );
    assert_eq!(decision.denial(), Some(PathDenial::OutsideEveryRoot));
    assert!(!decision.denial().expect("denied").is_shape());
}

#[test]
fn a_shape_refusal_is_reported_before_containment_is_considered() {
    // **The ordering.** A traversal is refused as a traversal even by a grant whose root would
    // *lexically* contain the resolved path, because resolving it is the filesystem's decision and not
    // this layer's. Reporting `OutsideEveryRoot` would invite the user to grant access, which is not
    // the missing thing.
    let grants = vec![read_only("data")];
    let decision = authorize_path(
        "data/../secret",
        PathMode::Read,
        &grants,
        PathComparison::CaseSensitive,
    );
    assert_eq!(decision.denial(), Some(PathDenial::Traversal));
    assert!(
        decision.denial().expect("denied").is_shape(),
        "a traversal is a shape refusal, not a missing grant",
    );
}

#[test]
fn a_grant_that_confers_only_read_refuses_a_write_mode() {
    // Mode-specific grants are the architecture's rule — "read, create, modify, delete" — and a grant
    // checked only for containment would let a read grant authorize a delete.
    let grants = vec![read_only("data/notes")];
    for mode in [PathMode::Create, PathMode::Modify, PathMode::Delete] {
        let decision = authorize_path(
            "data/notes/file.md",
            mode,
            &grants,
            PathComparison::CaseSensitive,
        );
        assert_eq!(
            decision.denial(),
            Some(PathDenial::ModeNotGranted),
            "a read-only grant must refuse {mode}",
        );
    }
    // And read still succeeds, so the refusal is about the mode rather than about the grant being
    // unusable.
    assert!(
        authorize_path(
            "data/notes/file.md",
            PathMode::Read,
            &grants,
            PathComparison::CaseSensitive,
        )
        .is_allowed(),
    );
}

#[test]
fn the_longest_matching_root_decides() {
    // **A legitimate configuration**: read the whole tree, write only one directory inside it. With
    // first-match the broad root's modes would apply inside the narrow one, so a write into
    // `data/notes` would be authorized by the read-everything grant's *containment* even though that
    // grant confers no write — or, if the broad grant conferred write everywhere, the narrow root's
    // restriction would be silently ignored.
    let grants = vec![
        grant("data", &[PathMode::Read]),
        grant("data/notes", &[PathMode::Read, PathMode::Modify]),
    ];
    // Inside the narrow root: modify is permitted by the narrow grant.
    assert!(
        authorize_path(
            "data/notes/file.md",
            PathMode::Modify,
            &grants,
            PathComparison::CaseSensitive,
        )
        .is_allowed(),
        "the narrow root confers modify",
    );
    // Inside the broad root but outside the narrow one: modify is refused, because the broad grant does
    // not confer it and the narrow one does not cover the path.
    assert_eq!(
        authorize_path(
            "data/other/file.md",
            PathMode::Modify,
            &grants,
            PathComparison::CaseSensitive,
        )
        .denial(),
        Some(PathDenial::ModeNotGranted),
    );
    // And the order of the grants does not matter, so the answer does not depend on how a repository
    // happened to return them — the same property `TLS-004` asserts for policy.
    let reversed: Vec<PathGrant> = grants.iter().rev().cloned().collect();
    assert_eq!(
        authorize_path(
            "data/notes/file.md",
            PathMode::Modify,
            &grants,
            PathComparison::CaseSensitive,
        ),
        authorize_path(
            "data/notes/file.md",
            PathMode::Modify,
            &reversed,
            PathComparison::CaseSensitive,
        ),
        "the decision must not depend on the grant order",
    );
}

#[test]
fn a_case_only_difference_is_reported_as_a_case_mismatch_on_a_case_sensitive_filesystem() {
    // The user's fix is to correct a letter rather than to request access, so the reason differs from
    // `OutsideEveryRoot`.
    let grants = vec![read_only("data/notes")];
    assert_eq!(
        authorize_path(
            "Data/Notes/file.md",
            PathMode::Read,
            &grants,
            PathComparison::CaseSensitive,
        )
        .denial(),
        Some(PathDenial::CaseMismatch),
    );
    // On a case-insensitive filesystem the same path is simply allowed, so the distinction is about the
    // comparison rather than about the path.
    assert!(
        authorize_path(
            "Data/Notes/file.md",
            PathMode::Read,
            &grants,
            PathComparison::CaseInsensitive,
        )
        .is_allowed(),
    );
    // And a path that differs by more than case is `OutsideEveryRoot`, so the case reason carries
    // information rather than being reported for every refusal.
    assert_eq!(
        authorize_path(
            "data/other/file.md",
            PathMode::Read,
            &grants,
            PathComparison::CaseSensitive,
        )
        .denial(),
        Some(PathDenial::OutsideEveryRoot),
    );
}

#[test]
fn a_sibling_sharing_a_string_prefix_is_refused_by_authorization() {
    // **The prefix defect at the authorization layer**, which is where it produces a disclosure rather
    // than a wrong boolean: a read grant for `data/notes` must not read `data/notsecret`.
    let grants = vec![read_only("data/notes")];
    assert_eq!(
        authorize_path(
            "data/notsecret/file.md",
            PathMode::Read,
            &grants,
            PathComparison::CaseSensitive,
        )
        .denial(),
        Some(PathDenial::OutsideEveryRoot),
        "a sibling directory must not be readable through a string-prefix match",
    );
}

#[test]
fn no_grants_refuses_every_path_that_is_well_formed() {
    // Nothing is allowed by omission, the same rule `TLS-004` enforces for policy. A check that treated
    // an empty grant list as "unrestricted" would be a filesystem with no permissions at all.
    assert_eq!(
        authorize_path(
            "data/notes/file.md",
            PathMode::Read,
            &[],
            PathComparison::CaseSensitive,
        )
        .denial(),
        Some(PathDenial::OutsideEveryRoot),
    );
    // But a malformed path is still refused for its shape even with no grants, so the reason names the
    // rule a caller can act on.
    assert_eq!(
        authorize_path(
            "../etc/passwd",
            PathMode::Read,
            &[],
            PathComparison::CaseSensitive
        )
        .denial(),
        Some(PathDenial::Traversal),
    );
}

// ---------------------------------------------------------------------------------------
// Grant construction and the mode enumeration.
// ---------------------------------------------------------------------------------------

#[test]
fn a_grant_conferring_no_mode_is_refused() {
    // A grant that confers nothing is not a grant: it would be recorded as access the user gave while
    // authorizing nothing, which is the state most easily mistaken for a working permission — the same
    // "exists but enforced nowhere" shape this project keeps finding.
    let error = PathGrant::new(path("data/notes"), Vec::new());
    assert!(error.is_err());
}

#[test]
fn a_duplicate_mode_is_refused_rather_than_collapsed() {
    // The list is what a reviewer read, so silently narrowing it accepts content the reviewer never saw
    // — the rule `TLS-001` established for effects and scopes.
    let error = PathGrant::new(path("data/notes"), vec![PathMode::Read, PathMode::Read]);
    assert!(error.is_err());
    // And a legitimately distinct pair is accepted, so the rule refuses duplicates rather than lists.
    assert!(PathGrant::new(path("data/notes"), vec![PathMode::Read, PathMode::Modify],).is_ok(),);
}

#[test]
fn every_mode_round_trips_and_the_set_refuses_an_unknown_spelling() {
    for mode in PathMode::ALL {
        assert_eq!(PathMode::parse(mode.as_contract_str()).ok(), Some(*mode));
    }
    assert_eq!(PathMode::ALL.len(), 4, "the architecture names four modes");
    // An unrecognised stored mode must not become Read, the fail-open direction.
    for value in ["write", "READ", "", "execute"] {
        assert!(PathMode::parse(value).is_err(), "{value:?} must be refused");
    }
}

#[test]
fn only_read_is_classified_as_not_writing() {
    // The predicate an approval preview uses: a prompt that did not distinguish reading from deleting
    // would not be "sufficient for informed consent".
    assert!(!PathMode::Read.writes());
    assert!(PathMode::Create.writes());
    assert!(PathMode::Modify.writes());
    assert!(PathMode::Delete.writes());
}

#[test]
fn a_grant_reports_whether_it_writes_at_all() {
    assert!(!read_only("data/notes").writes());
    assert!(grant("data/notes", &[PathMode::Read, PathMode::Delete]).writes());
    assert!(grant("data/notes", &[PathMode::Create]).writes());
}

#[test]
fn a_path_survives_a_json_round_trip_and_a_non_canonical_one_is_refused_on_the_wire() {
    // Paths are persisted in grants and shown in approval previews, so the wire form is contract
    // surface. The round trip matters because a stored grant must authorize the same paths after a
    // restart.
    let original = path("data/notes/file.md");
    let json = serde_json::to_string(&original).expect("a path serializes");
    assert_eq!(json, r#""data/notes/file.md""#);
    let restored: WorkspaceRelativePath = serde_json::from_str(&json).expect("a path deserializes");
    assert_eq!(restored, original);
    // And the deserialization path enforces the same rule, so a stored value cannot introduce an
    // un-normalized path that the containment comparison would then mis-handle.
    assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""data//notes""#).is_err());
    assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""../escape""#).is_err());
}

#[test]
fn a_grant_survives_a_json_round_trip() {
    let original = grant("data/notes", &[PathMode::Read, PathMode::Modify]);
    let json = serde_json::to_string(&original).expect("a grant serializes");
    let restored: PathGrant = serde_json::from_str(&json).expect("a grant deserializes");
    assert_eq!(restored, original);
    assert!(json.contains(r#""read""#), "{json}");
    assert!(json.contains(r#""data/notes""#), "{json}");
}

#[test]
fn the_comparison_itself_compares_the_way_it_says() {
    // Asserted directly rather than only through containment, because the two branches are what the
    // whole case-handling rule rests on.
    assert!(PathComparison::CaseSensitive.segments_equal("Notes", "Notes"));
    assert!(!PathComparison::CaseSensitive.segments_equal("Notes", "notes"));
    assert!(PathComparison::CaseInsensitive.segments_equal("Notes", "notes"));
    assert!(PathComparison::CaseInsensitive.segments_equal("NOTES", "notes"));
    assert!(!PathComparison::CaseInsensitive.segments_equal("Notes", "Notess"));
}
