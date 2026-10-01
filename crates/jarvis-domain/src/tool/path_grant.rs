//! Rooted filesystem grants and the lexical path rules that make them meaningful.
//!
//! `TLS-007` asks for "safe reference filesystem read and write-plan tools", and the security
//! architecture states the rule this module implements:
//!
//! > Canonicalize paths and defend against traversal, symlink/junction/reparse-point races,
//! > alternate data streams, reserved names, and case differences.
//! > File grants are rooted and mode-specific: read, create, modify, delete.
//!
//! **What is here, and what is deliberately not.** This module owns the *decision*: given a path a
//! model produced and the grants a principal holds, may this path be read? It is a pure function of
//! values, so the rules are exhaustively testable without a filesystem, and the adapter that performs
//! the read has no policy of its own to get wrong. The **enforcement** — opening a file relative to a
//! directory handle with no-follow semantics — is *not* here, and cannot be: this workspace denies
//! `unsafe-code`, so the `openat`-family primitives are unreachable, and `cap-std` is not a dependency.
//! That gap is named in `TLS-007` rather than papered over, because a check that canonicalizes and then
//! opens has a TOCTOU race the architecture explicitly calls out, and shipping that while calling it
//! safe would be worse than shipping nothing.
//!
//! **Three rules carry most of the weight**, and each is a mistake a plausible implementation makes:
//!
//! 1. **Containment is compared segment by segment, never as a string prefix.** `/data/notsecret`
//!    starts with `/data/note` as a string and is a different directory, so a prefix test authorizes a
//!    path the grant never covered. This is the defect the module's tests are written around.
//! 2. **A path must already be in normalized form.** `a//b`, `./a`, and `a/./b` are all spellings of
//!    one file, and accepting them means two strings denote one resource — so the containment check
//!    could be satisfied by a spelling the grant's root does not cover.
//! - **A `..` segment is refused outright rather than resolved.** Resolving it lexically is possible
//!   but it discards the fact that the caller asked to leave the directory, and once symlinks are in
//!   play the lexical answer and the filesystem's answer differ. Refusing keeps the decision
//!   independent of the filesystem, which is what makes it testable at all.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// The longest accepted path.
///
/// Bounded because a path reaches a grant record, an approval preview, and a log line, and because a
/// path is model output here rather than something JARVIS constructed.
pub const MAX_PATH_BYTES: usize = 4096;

/// The largest number of segments one path may contain.
pub const MAX_PATH_SEGMENTS: usize = 128;

/// Names that are device files on Windows, in the stem form that Windows reserves.
///
/// Windows resolves these **whatever directory and whatever extension** they carry, so `CON`, `con.txt`,
/// and `AUX.tar.gz` all name a device. Case is irrelevant there, which is why the check
/// case-folds — a rule that compared exactly would let `con` through on the one platform where it
/// matters.
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// How two paths are compared, which the filesystem decides rather than the domain.
///
/// **An input rather than an assumption.** Whether `/Data` and `/data` are one directory is a fact
/// about the mounted filesystem, and this layer cannot see the filesystem. Assuming case-insensitive
/// would refuse legitimate paths on Linux; assuming case-sensitive would let a Windows path evade a
/// grant by changing one letter, which is the "case differences" case the architecture names. So the
/// adapter states which it is, and the rule that depends on it is here where it can be tested both
/// ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathComparison {
    /// `Data` and `data` are different names.
    CaseSensitive,
    /// `Data` and `data` are the same name.
    CaseInsensitive,
}

impl PathComparison {
    /// Returns whether two segments are the same name under this comparison.
    #[must_use]
    pub fn segments_equal(self, left: &str, right: &str) -> bool {
        match self {
            Self::CaseSensitive => left == right,
            Self::CaseInsensitive => left.eq_ignore_ascii_case(right),
        }
    }
}

/// One of the four access modes a grant can confer.
///
/// The architecture names exactly these: "read, create, modify, delete". A closed set because they are
/// a permission surface, and an unknown mode must not be read as read-only — the fail-open direction —
/// nor as the strongest mode, which would refuse legitimate work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathMode {
    /// Read existing content.
    Read,
    /// Create a file that does not exist.
    Create,
    /// Change an existing file's content.
    Modify,
    /// Remove an entry.
    Delete,
}

impl PathMode {
    /// Every mode, in the architecture's order.
    pub const ALL: &'static [Self] = &[Self::Read, Self::Create, Self::Modify, Self::Delete];

    /// Returns the spelling the architecture uses.
    #[must_use]
    pub const fn as_contract_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Create => "create",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }

    /// Parses the architecture's spelling.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `mode`. An unrecognised stored mode must
    /// not become `Read`.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.as_contract_str() == value)
            .ok_or(DomainError::ToolDefinitionInvalid { field: "mode" })
    }

    /// Returns whether the mode changes anything on disk.
    ///
    /// Used where an approval's preview must say whether the action is a write — the contract requires
    /// the preview to be "sufficient for informed consent", and a prompt that did not distinguish
    /// reading from deleting would not be.
    #[must_use]
    pub const fn writes(self) -> bool {
        !matches!(self, Self::Read)
    }
}

impl fmt::Display for PathMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_contract_str())
    }
}

/// Why a path was refused.
///
/// One variant per *rule*, because these are shown to a user and each needs a different action from
/// them. "Outside the granted roots" means ask for access; "reserved name" means choose another name;
/// "case differs" means the path refers to a different resource than the one granted — three different
/// conversations that a single `Denied` would collapse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathDenial {
    /// The path is absolute, so it is not rooted in any grant.
    Absolute,
    /// The path contains a `..` segment.
    Traversal,
    /// The path contains a `.` or empty segment, so it is not normalized.
    NotNormalized,
    /// The path contains a backslash, which is a separator on one platform and a filename character on
    /// another, so accepting it would mean two spellings denote one file.
    AmbiguousSeparator,
    /// A segment names a device file.
    ReservedName,
    /// A segment contains a colon, which on Windows introduces an alternate data stream.
    AlternateDataStream,
    /// A segment ends with a dot or a space, which Windows silently strips.
    TrailingDotOrSpace,
    /// A segment is empty, over-long, or contains a control character.
    MalformedSegment,
    /// The path is not inside any granted root.
    OutsideEveryRoot,
    /// The path is inside a granted root but the grant does not confer this mode.
    ModeNotGranted,
    /// A segment's case differs from the granted root's, on a case-sensitive filesystem.
    CaseMismatch,
}

impl PathDenial {
    /// Returns whether the refusal is about the path's *shape* rather than about permissions.
    ///
    /// A shape refusal is the model's fault and never becomes allowed by a new grant, while a
    /// permission refusal is the user's to resolve. A caller that offered "request access" for a
    /// traversal attempt would be offering to grant something that cannot be granted.
    #[must_use]
    pub const fn is_shape(self) -> bool {
        matches!(
            self,
            Self::Absolute
                | Self::Traversal
                | Self::NotNormalized
                | Self::AmbiguousSeparator
                | Self::ReservedName
                | Self::AlternateDataStream
                | Self::TrailingDotOrSpace
                | Self::MalformedSegment
        )
    }
}

impl fmt::Display for PathDenial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Absolute => "the path is absolute",
            Self::Traversal => "the path contains a parent-directory segment",
            Self::NotNormalized => "the path is not in normalized form",
            Self::AmbiguousSeparator => "the path contains a backslash",
            Self::ReservedName => "a segment names a reserved device",
            Self::AlternateDataStream => "a segment names an alternate data stream",
            Self::TrailingDotOrSpace => "a segment ends with a dot or a space",
            Self::MalformedSegment => {
                "a segment is empty, over-long, or contains a control character"
            }
            Self::OutsideEveryRoot => "the path is outside every granted root",
            Self::ModeNotGranted => "the grant does not confer this mode",
            Self::CaseMismatch => "a segment's case differs from the granted root",
        };
        formatter.write_str(text)
    }
}

/// A workspace-relative path in normalized form.
///
/// Constructed only through [`Self::parse`] or a validating deserializer, so every instance satisfies
/// the lexical rules and the containment comparison can rely on them. The segments are stored rather
/// than re-split at each use, which is also what makes the segment-wise comparison cheap enough to be
/// obviously correct.
///
/// **⚠ The `Deserialize` impl is hand-written to go through [`Self::parse`], and that is
/// load-bearing.** `#[serde(transparent)]` on a newtype derives a deserializer that calls
/// `String::deserialize` and wraps the result directly, so a stored or received `"data//notes"` — or a
/// `"../escape"` — would have produced a `WorkspaceRelativePath` that violates the type's own
/// invariant. Everything downstream trusts that invariant: `is_under` compares segment by segment and
/// `authorize_path` skips the shape check for a value it believes is already normalized. Deriving the
/// impl would therefore have made the guarantee "true for values this crate constructed and false for
/// values that arrived over the wire", which is the direction an attacker chooses.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    /// Parses and validates a path.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `path` for a shape refusal. The specific
    /// rule is reported separately through [`Self::denial_for`], because a caller building a grant and
    /// a caller authorizing a request both need the reason rather than a boolean.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        if Self::denial_for(value).is_some() {
            return Err(DomainError::ToolDefinitionInvalid { field: "path" });
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns which lexical rule, if any, refuses `value`.
    ///
    /// **A separate function from the constructor rather than the constructor's internals**, because
    /// the authorization path needs to distinguish the rules while the constructor needs only to
    /// refuse, and deriving the reason by re-parsing a refused value would mean two implementations of
    /// one rule.
    #[must_use]
    pub fn denial_for(value: &str) -> Option<PathDenial> {
        if value.is_empty() || value.len() > MAX_PATH_BYTES {
            return Some(PathDenial::MalformedSegment);
        }
        // A backslash is checked before the separator split, because it is not a separator here: on
        // Windows it is, on Unix it is a legal filename character, so accepting it would make `a\b` two
        // different files depending on the platform — the ambiguity this refusal removes.
        if value.contains('\\') {
            return Some(PathDenial::AmbiguousSeparator);
        }
        // An absolute path is refused before splitting, so `/etc/passwd` reports `Absolute` rather than
        // the empty first segment it would otherwise produce.
        if value.starts_with('/') {
            return Some(PathDenial::Absolute);
        }
        // A drive letter or a UNC prefix. `C:x` is drive-relative on Windows and still escapes any
        // root, so the colon rule below would catch it — but the explicit check names the rule, since
        // `C:notes.md` is a path a model plausibly produces.
        if value.len() >= 2 && value.as_bytes()[1] == b':' {
            return Some(PathDenial::Absolute);
        }
        let segments: Vec<&str> = value.split('/').collect();
        if segments.len() > MAX_PATH_SEGMENTS {
            return Some(PathDenial::MalformedSegment);
        }
        for segment in segments {
            if segment.is_empty() {
                // An empty segment is a doubled or trailing slash: not normalized, and a second spelling
                // of a path that would otherwise be checked once.
                return Some(PathDenial::NotNormalized);
            }
            if segment == ".." {
                return Some(PathDenial::Traversal);
            }
            if segment == "." {
                return Some(PathDenial::NotNormalized);
            }
            if segment.contains('\0') || segment.chars().any(char::is_control) {
                return Some(PathDenial::MalformedSegment);
            }
            // A colon anywhere in a segment introduces an alternate data stream on Windows
            // (`notes.md:secret`), which is a second file behind one name.
            if segment.contains(':') {
                return Some(PathDenial::AlternateDataStream);
            }
            // Windows strips a trailing dot or space, so `notes.md ` and `notes.md` are one file with
            // two spellings — and the stripped form can differ from the granted root's.
            if segment.ends_with('.') || segment.ends_with(' ') {
                return Some(PathDenial::TrailingDotOrSpace);
            }
            // The stem, so `CON`, `con.txt`, and `AUX.tar.gz` are all refused.
            let stem = segment.split('.').next().unwrap_or(segment);
            if RESERVED_NAMES
                .iter()
                .any(|reserved| reserved.eq_ignore_ascii_case(stem))
            {
                return Some(PathDenial::ReservedName);
            }
            if segment.len() > 255 {
                return Some(PathDenial::MalformedSegment);
            }
        }
        None
    }

    /// Returns the normalized text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the segments.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Returns whether this path is inside `root`, compared segment by segment.
    ///
    /// **Not a string prefix test, and that is the point.** `/data/notsecret` begins with `/data/note`
    /// as a string and is a sibling directory, so a prefix test authorizes a path the root never
    /// covered. Segment-wise comparison requires the root to be a whole-number-of-segments prefix —
    /// which is only sound because both values are normalized, and a caller cannot construct an
    /// un-normalized one.
    #[must_use]
    pub fn is_under(&self, root: &Self, comparison: PathComparison) -> bool {
        let mut mine = self.segments();
        for root_segment in root.segments() {
            let Some(segment) = mine.next() else {
                // The root is longer than the path, so the path cannot be under it.
                return false;
            };
            if !comparison.segments_equal(segment, root_segment) {
                return false;
            }
        }
        // The root is a prefix of the path **by segments**, so it may be the path itself or an
        // ancestor. Both are inside the root, which is the intended reading of "rooted at".
        true
    }

    /// Returns whether the path names the same resource as `root` except for letter case.
    ///
    /// **Two conditions, and the first version had only one.** It compared the root's segments
    /// case-insensitively against the path's and returned `true` when they matched — which is also true
    /// for a path that is *inside* the root, so `data/notes/file.md` was reported as a case mismatch
    /// against a `data/notes` root. That would have told a user to fix the letter case of a path that
    /// was not case-wrong at all, and (against a case-sensitive filesystem) it would have reported the
    /// wrong reason for a perfectly ordinary descendant.
    ///
    /// The correct reading needs both halves: the prefix must match **ignoring** case, and it must
    /// **not** match exactly — because an exact match means the path is under the root rather than a
    /// different spelling of it.
    #[must_use]
    pub fn differs_only_by_case_from(&self, root: &Self) -> bool {
        let mut mine = self.segments();
        let mut differs = false;
        for root_segment in root.segments() {
            let Some(segment) = mine.next() else {
                return false;
            };
            if segment == root_segment {
                continue;
            }
            if !segment.eq_ignore_ascii_case(root_segment) {
                return false;
            }
            differs = true;
        }
        differs
    }
}

impl fmt::Display for WorkspaceRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceRelativePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// One rooted, mode-specific grant.
///
/// The root is a [`WorkspaceRelativePath`], so a grant cannot be rooted outside the workspace; the
/// workspace itself is the outermost boundary and is enforced by resolution, not by a path string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PathGrant {
    /// The directory the grant is rooted at, inclusive.
    pub root: WorkspaceRelativePath,
    /// Which modes it confers.
    pub modes: Vec<PathMode>,
}

impl PathGrant {
    /// Builds a grant.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::ToolDefinitionInvalid`] naming `modes` when the list is empty or carries
    /// a duplicate, and `mode` when a mode appears twice. **An empty mode list is refused** because a
    /// grant that confers nothing is not a grant: it would be recorded as access the user gave and
    /// would authorize nothing, which is the state most easily mistaken for a working permission.
    pub fn new(root: WorkspaceRelativePath, modes: Vec<PathMode>) -> Result<Self, DomainError> {
        if modes.is_empty() {
            return Err(DomainError::ToolDefinitionInvalid { field: "modes" });
        }
        let mut sorted = modes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != modes.len() {
            // Duplicates are refused rather than collapsed, for the reason `TLS-001` records: the list
            // is what a reviewer read, and silently narrowing it accepts content the reviewer never saw.
            return Err(DomainError::ToolDefinitionInvalid { field: "modes" });
        }
        Ok(Self { root, modes })
    }

    /// Returns whether the grant confers `mode`.
    #[must_use]
    pub fn permits(&self, mode: PathMode) -> bool {
        self.modes.contains(&mode)
    }

    /// Returns whether the grant confers anything that writes.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.modes.iter().any(|mode| mode.writes())
    }
}

impl<'de> Deserialize<'de> for PathGrant {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            root: WorkspaceRelativePath,
            modes: Vec<PathMode>,
        }
        let grant = Wire::deserialize(deserializer)?;
        // **Through [`Self::new`] rather than derived.** A grant that confers nothing, or that lists a
        // mode twice, is a stored authorization the user never gave: an empty list is recorded as access
        // and authorizes nothing, and a duplicate is a list that has been narrowed from what a reviewer
        // read. A derived impl rebuilt both for any grant read from a document, and a grant store is the
        // next consumer.
        Self::new(grant.root, grant.modes).map_err(serde::de::Error::custom)
    }
}

/// What authorizing a path found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathDecision {
    /// The path is allowed, and this is the grant that allowed it.
    Allowed {
        /// The normalized path that was authorized.
        path: WorkspaceRelativePath,
        /// The grant's root, so a caller can log which root covered it.
        root: WorkspaceRelativePath,
    },
    /// The path is refused, with the rule that refused it.
    Denied {
        /// The rule.
        reason: PathDenial,
    },
}

impl PathDecision {
    /// Returns whether the path is allowed.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }

    /// Returns the refusal reason, if it was refused.
    #[must_use]
    pub fn denial(&self) -> Option<PathDenial> {
        match self {
            Self::Allowed { .. } => None,
            Self::Denied { reason } => Some(*reason),
        }
    }
}

/// Authorizes one path against the grants a principal holds.
///
/// The checks are ordered so the caller receives the **most specific true** answer, following the
/// ordering `RunLifecycle::apply` established:
///
/// 1. **Shape first.** A path that is not in normalized form or contains a traversal is refused before
///    any grant is consulted, because such a path cannot be authorized by *any* grant — resolving it
///    would mean deciding what it refers to, and that decision belongs to the filesystem rather than
///    here. Reporting `OutsideEveryRoot` for a traversal would invite the user to grant access, which
///    is not the missing thing.
/// 2. **Containment, segment-wise.** The longest matching root wins, so a narrow grant can coexist with
///    a broad one without the broad one's modes leaking into the narrow root — a real configuration, and
///    one a "first match" rule would get wrong.
/// 3. **Mode last**, because it is the only check that needs the specific grant.
#[must_use]
pub fn authorize_path(
    candidate: &str,
    mode: PathMode,
    grants: &[PathGrant],
    comparison: PathComparison,
) -> PathDecision {
    // Step 1: the shape. Refused before any grant is considered, and the reason is reported so the
    // user learns which rule rather than only that it failed.
    if let Some(reason) = WorkspaceRelativePath::denial_for(candidate) {
        return PathDecision::Denied { reason };
    }
    let path = WorkspaceRelativePath(candidate.to_owned());

    // Step 2: containment, choosing the **longest** matching root. Longest rather than first, because a
    // configuration with a broad root and a narrow sub-root is legitimate (read the tree, write only one
    // directory), and first-match would let the broad root's modes apply inside the narrow one.
    let mut matched: Option<&PathGrant> = None;
    for grant in grants {
        if !path.is_under(&grant.root, comparison) {
            continue;
        }
        let better = matched
            .is_none_or(|current| grant.root.segments().count() > current.root.segments().count());
        if better {
            matched = Some(grant);
        }
    }
    let Some(grant) = matched else {
        // The path is outside every root. **A case-difference is reported separately**, because on a
        // case-sensitive filesystem it names a different directory and the user's fix is to correct a
        // letter rather than to request access.
        let case_only = grants.iter().any(|candidate_grant| {
            path.differs_only_by_case_from(&candidate_grant.root)
                && comparison == PathComparison::CaseSensitive
        });
        return PathDecision::Denied {
            reason: if case_only {
                PathDenial::CaseMismatch
            } else {
                PathDenial::OutsideEveryRoot
            },
        };
    };

    // Step 3: the mode, against the *chosen* grant rather than any grant.
    if !grant.permits(mode) {
        return PathDecision::Denied {
            reason: PathDenial::ModeNotGranted,
        };
    }
    PathDecision::Allowed {
        path,
        root: grant.root.clone(),
    }
}
