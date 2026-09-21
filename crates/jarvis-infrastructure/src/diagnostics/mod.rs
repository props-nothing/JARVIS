//! Deterministic diagnostics: findings, collector, and reviewable support bundle.
//!
//! This module owns two things that `jarvis doctor` and `jarvis support-bundle`
//! both need, and that used to live in the command-line binary:
//!
//! 1. [`CheckReport`] — a deterministic list of named checks with a severity, a
//!    stable message code, and operator advice. A blocking finding is *not*
//!    the same as an unexpected condition: a daemon that is simply not running
//!    is a warning, because foreground mode is a supported way to use JARVIS.
//!    Only findings that make the profile unusable are blocking, so
//!    "no blocking findings" never has to be read as "nothing is wrong".
//!
//! 2. The support-bundle pipeline — [`BundlePlan`], [`PlanItem`], [`SupportBundle`]
//!    and [`write_bundle`]. A bundle is *planned* (a pure value that can be
//!    printed and asserted on any host), then *materialized* in memory, then
//!    written. The plan is what a user reviews; the materialized bundle is what
//!    gets written, so "what was previewed" and "what was exported" cannot
//!    diverge.
//!
//! The redaction guarantee is inherited rather than reimplemented. Bundle text
//! passes through [`jarvis_observability::Redactor`], which removes registered
//! secret values wherever they appear. The canary test in this module proves
//! that a registered value is absent from the produced archive bytes.
//!
//! Log collection does **not** use [`jarvis_observability::RedactingWriter`],
//! because that writer buffers a partial line until `flush` — usable only when
//! the writer is dropped. A bundle must keep reading a live file, so the tail
//! line is redacted explicitly instead and the guarantee is stated in
//! [`BundleSource::LogTail`].

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// `Redactor` is re-exported here so a caller of this module does not need a
// direct `jarvis-observability` dependency just to pass the redactor the bundle
// API requires. The other items are internal to this module.
pub use jarvis_observability::Redactor;
use jarvis_observability::{REDACTED, RedactorError, sanitize_control_chars};
use sha2::{Digest as _, Sha256};

mod archive;
mod collector;
mod repair;
#[cfg(test)]
mod tests;

pub use archive::{ArchiveError, MAX_ARCHIVE_BYTES, ZipArchive};
pub use collector::{
    DaemonDescriptor, DiagnosticsEnvironment, collect, daemon_summary, database_file,
};
pub use repair::{
    RepairAction, RepairDiagnosis, RepairError, RepairKind, RepairOutcome, RepairPlan, apply,
    is_protected, plan_for, plans_for, protected_database_path, unrepairable,
};
/// The bundle manifest schema version, recorded inside every bundle.
pub const SUPPORT_BUNDLE_SCHEMA_VERSION: u32 = 1;

/// The maximum number of bytes a bundle retains from one log file.
///
/// A support bundle is a bounded diagnostic snapshot, not a log archive. Reading
/// a whole log directory would copy days of history into a file the user is
/// about to attach to a message.
pub const MAX_LOG_BYTES_PER_FILE: usize = 256 * 1024;

/// The maximum number of log files a bundle includes.
pub const MAX_LOG_FILES: usize = 5;

/// The maximum number of bytes this process reads from a discovered JSON file.
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

/// How serious a finding is.
///
/// The ordering matters: [`CheckReport::blocking`] counts only
/// [`Severity::Error`], and [`Severity::Warning`] must never be collapsed into
/// it, because a warning is precisely the case where the operator may decide the
/// current state is acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational: the check passed.
    Ok,
    /// The operator should look at this, but the profile is usable.
    Warning,
    /// The profile cannot serve until this is fixed.
    Error,
}

impl Severity {
    /// Returns the lowercase token used in the report and the bundle.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warn",
            Self::Error => "error",
        }
    }

    /// Returns whether this severity is a blocking finding.
    #[must_use]
    pub const fn is_blocking(self) -> bool {
        matches!(self, Self::Error)
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

/// One named diagnostic check and its outcome.
///
/// `message` and `advice` are constrained:
///
/// - `message` is either a fact the check itself produced (a version string, a
///   count, another error's stable `code()`) or a fixed sentence from this
///   module. No raw filesystem content, no log line, and no secret ever reaches
///   it.
/// - `advice` is always a fixed sentence owned by the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable, human-readable check name (for example `database integrity`).
    pub check: &'static str,
    /// The outcome.
    pub severity: Severity,
    /// A bounded fact about the outcome.
    pub message: String,
    /// A fixed, actionable next step.
    pub advice: Option<&'static str>,
}

impl Finding {
    /// Records a passed check.
    #[must_use]
    pub fn ok(check: &'static str, message: impl Into<String>) -> Self {
        Self {
            check,
            severity: Severity::Ok,
            message: message.into(),
            advice: None,
        }
    }

    /// Records a condition the operator should review.
    #[must_use]
    pub fn warning(check: &'static str, message: impl Into<String>, advice: &'static str) -> Self {
        Self {
            check,
            severity: Severity::Warning,
            message: message.into(),
            advice: Some(advice),
        }
    }

    /// Records a condition that blocks the profile.
    #[must_use]
    pub fn error(check: &'static str, message: impl Into<String>, advice: &'static str) -> Self {
        Self {
            check,
            severity: Severity::Error,
            message: message.into(),
            advice: Some(advice),
        }
    }

    /// Renders the single line `jarvis doctor` prints for this finding.
    #[must_use]
    pub fn render(&self) -> String {
        if let Some(advice) = self.advice {
            format!(
                "{:<7} {}: {} -- {}",
                self.severity.token(),
                self.check,
                self.message,
                advice
            )
        } else {
            format!(
                "{:<7} {}: {}",
                self.severity.token(),
                self.check,
                self.message
            )
        }
    }
}

/// The ordered findings of one doctor run.
#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    findings: Vec<Finding>,
}

impl CheckReport {
    /// Creates an empty report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a finding, preserving check order.
    pub fn push(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    /// Returns every finding, in the order the checks ran.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Returns how many findings block the profile.
    #[must_use]
    pub fn blocking(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity.is_blocking())
            .count()
    }

    /// Returns how many findings need operator review but are not blocking.
    #[must_use]
    pub fn warnings(&self) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Warning)
            .count()
    }

    /// Returns whether the profile is usable.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.blocking() == 0
    }

    /// Returns the stable exit code the CLI should return.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        u8::from(!self.is_usable())
    }

    /// Renders the whole report with an explicit summary.
    ///
    /// The summary names warnings separately from blocking findings so a clean
    /// exit cannot be mistaken for "nothing was reported".
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for finding in &self.findings {
            out.push_str(&finding.render());
            out.push('\n');
        }
        let blocking = self.blocking();
        let warnings = self.warnings();
        if blocking == 0 {
            let _ = writeln!(out, "\nno blocking findings ({warnings} warning(s))");
        } else {
            let _ = writeln!(
                out,
                "\n{blocking} blocking finding(s), {warnings} warning(s)"
            );
        }
        out
    }

    /// Renders the findings as the JSON document a bundle embeds.
    ///
    /// The shape is stable and versioned so a downstream reader is not coupled
    /// to the display format.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n  \"schema_version\": 1,\n  \"findings\": [\n");
        for (index, finding) in self.findings.iter().enumerate() {
            out.push_str("    {\"check\": ");
            out.push_str(&json_string(finding.check));
            out.push_str(", \"severity\": ");
            out.push_str(&json_string(finding.severity.token()));
            out.push_str(", \"message\": ");
            out.push_str(&json_string(&finding.message));
            out.push_str(", \"advice\": ");
            match finding.advice {
                Some(advice) => out.push_str(&json_string(advice)),
                None => out.push_str("null"),
            }
            out.push('}');
            if index + 1 != self.findings.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("  ],\n  \"blocking\": ");
        out.push_str(&self.blocking().to_string());
        out.push_str(",\n  \"warnings\": ");
        out.push_str(&self.warnings().to_string());
        out.push_str("\n}\n");
        out
    }
}

/// The environment data a bundle records.
///
/// This is an explicit, small record rather than a dump of the process
/// environment, because the environment is the most common accidental secret
/// carrier. The daemon and client never pass their own environment through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentSummary {
    /// The profile mode, as resolved (`standard` or `portable`).
    pub profile_mode: &'static str,
    /// The platform, as `std::env::consts::OS`.
    pub os: &'static str,
    /// The architecture, as `std::env::consts::ARCH`.
    pub arch: &'static str,
    /// The client version, as recorded by Cargo.
    pub client_version: &'static str,
    /// The API major version this client speaks.
    pub api_major: u32,
}

impl EnvironmentSummary {
    /// Captures this process's platform facts.
    #[must_use]
    pub fn current(profile_mode: &'static str, api_major: u32) -> Self {
        Self {
            profile_mode,
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            client_version: env!("CARGO_PKG_VERSION"),
            api_major,
        }
    }

    /// Renders the environment record as JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\n  \"schema_version\": 1,\n  \"profile_mode\": {},\n  \"os\": {},\n  \
             \"arch\": {},\n  \"client_version\": {},\n  \"api_major\": {}\n}}\n",
            json_string(self.profile_mode),
            json_string(self.os),
            json_string(self.arch),
            json_string(self.client_version),
            self.api_major,
        )
    }
}

/// The daemon facts a bundle records, or the reason they could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonSummary {
    /// The daemon instance id, when a discovery file was readable.
    ///
    /// This is a canonical JARVIS identifier, not a secret: it identifies an
    /// instance, and possession of it grants nothing.
    pub instance_id: Option<String>,
    /// The daemon process id, when readable. Diagnostic only; never authority.
    pub pid: Option<u32>,
    /// The daemon's own reported version, when readable.
    pub server_version: Option<String>,
    /// The stable error code when the environment could not be read.
    pub unavailable_code: Option<&'static str>,
}

impl DaemonSummary {
    /// Renders the daemon record as JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\n  \"schema_version\": 1,\n");
        out.push_str("  \"instance_id\": ");
        push_optional(&mut out, self.instance_id.as_deref());
        out.push_str(",\n  \"pid\": ");
        match self.pid {
            Some(pid) => out.push_str(&pid.to_string()),
            None => out.push_str("null"),
        }
        out.push_str(",\n  \"server_version\": ");
        push_optional(&mut out, self.server_version.as_deref());
        out.push_str(",\n  \"unavailable_code\": ");
        push_optional(&mut out, self.unavailable_code);
        out.push_str("\n}\n");
        out
    }
}

/// Where a bundle item's content comes from.
///
/// The variant is part of the plan the user reviews, so a reader can see that a
/// log tail is bounded and that the raw environment is *not* part of the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleSource {
    /// Rendered in memory by this process from a typed value.
    Rendered,
    /// A bounded, redacted tail of one log file, capped at
    /// [`MAX_LOG_BYTES_PER_FILE`] bytes and read whole so no partial line is
    /// copied.
    LogTail,
}

impl BundleSource {
    /// Returns the token recorded in the manifest.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Rendered => "rendered",
            Self::LogTail => "log_tail_bounded_redacted",
        }
    }
}

/// One item the user can review, and optionally exclude, before export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Path of the item inside the bundle.
    pub path: String,
    /// What the item is, in one sentence.
    pub description: &'static str,
    /// Where the content comes from.
    pub source: BundleSource,
    /// Whether the user may exclude this item.
    ///
    /// The manifest is never optional: a bundle without a manifest cannot be
    /// interpreted. Everything else is the user's choice.
    pub optional: bool,
    /// The absolute path on disk the item is read from, for a log tail.
    pub source_path: Option<PathBuf>,
    /// The rendered content, for a rendered item.
    pub content: Option<String>,
}

/// The reviewable list of what a bundle would contain.
///
/// Creating a plan reads nothing but the log directory listing and renders
/// in-memory records. Reading the log tail happens at materialization, so
/// printing the plan cannot itself copy unredacted file content anywhere.
#[derive(Debug, Clone, Default)]
pub struct BundlePlan {
    items: Vec<PlanItem>,
    /// The log files skipped because the bound was reached, for honesty.
    omitted_log_files: usize,
}

impl BundlePlan {
    /// Creates an empty plan.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an item.
    pub fn push(&mut self, item: PlanItem) {
        self.items.push(item);
    }

    /// Records how many log files were left out by the file-count bound.
    pub fn set_omitted_log_files(&mut self, count: usize) {
        self.omitted_log_files = count;
    }

    /// Returns the items in the order they will appear in the bundle.
    #[must_use]
    pub fn items(&self) -> &[PlanItem] {
        &self.items
    }

    /// Returns how many log files the file-count bound left out.
    #[must_use]
    pub const fn omitted_log_files(&self) -> usize {
        self.omitted_log_files
    }

    /// Renders the plan for the operator, including what is excluded.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from("bundle preview (nothing has been written yet)\n");
        for item in &self.items {
            let optional = if item.optional {
                "optional"
            } else {
                "required"
            };
            let _ = writeln!(
                out,
                "  {:<34} {:<26} {optional}\n      {}",
                item.path,
                item.source.token(),
                item.description
            );
        }
        if self.omitted_log_files > 0 {
            let _ = writeln!(
                out,
                "  note: {} further log file(s) are outside the {} file bound",
                self.omitted_log_files, MAX_LOG_FILES
            );
        }
        out.push_str(
            "excluded from every bundle: secret values, authorization headers, the raw\n\
             process environment, the database file, prompts, messages, documents, tool\n\
             payloads, transcripts, and audio.\n",
        );
        out
    }

    /// Returns the item paths that `--exclude` names resolve to, or an error.
    ///
    /// Excluding an item that is not optional is refused by name, so the user is
    /// told the manifest cannot be dropped instead of silently getting it.
    ///
    /// # Errors
    ///
    /// Returns [`BundleError::UnknownItem`] or [`BundleError::RequiredItem`].
    pub fn resolve_exclusions(&self, names: &[String]) -> Result<Vec<String>, BundleError> {
        let mut resolved = Vec::new();
        for name in names {
            let Some(item) = self.items.iter().find(|item| &item.path == name) else {
                return Err(BundleError::UnknownItem { name: name.clone() });
            };
            if !item.optional {
                return Err(BundleError::RequiredItem { name: name.clone() });
            }
            resolved.push(name.clone());
        }
        Ok(resolved)
    }
}

/// A materialized bundle: the ordered files and their bytes.
#[derive(Debug)]
pub struct SupportBundle {
    files: Vec<BundleFile>,
    excluded: Vec<String>,
}

impl SupportBundle {
    /// Returns the included files, in archive order.
    #[must_use]
    pub fn files(&self) -> &[BundleFile] {
        &self.files
    }

    /// Returns the item paths the user excluded.
    #[must_use]
    pub fn excluded(&self) -> &[String] {
        &self.excluded
    }

    /// Returns the total uncompressed size of the bundle.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes.len() as u64).sum()
    }
}

/// One file inside a materialized bundle.
#[derive(Debug)]
pub struct BundleFile {
    /// Path inside the archive.
    pub path: String,
    /// The redacted content.
    pub bytes: Vec<u8>,
}

/// An error raised while planning or writing a bundle.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// The named item does not exist in the plan.
    #[error("no bundle item is named {name}")]
    UnknownItem {
        /// The name the user supplied.
        name: String,
    },
    /// The named item cannot be excluded.
    #[error("bundle item {name} is required and cannot be excluded")]
    RequiredItem {
        /// The name the user supplied.
        name: String,
    },
    /// The plan has no manifest, so a bundle cannot be interpreted.
    #[error("the bundle plan has no manifest")]
    MissingManifest,
    /// Redaction could not be applied.
    #[error("redaction could not be applied: {0}")]
    Redaction(#[from] RedactorError),
    /// A log file could not be read.
    #[error("a log file could not be read")]
    LogRead {
        /// The file that failed.
        path: PathBuf,
    },
    /// A log line was not valid in the profile's encoding.
    #[error("a log line was not valid UTF-8")]
    LogEncoding,
    /// The archive could not be produced.
    #[error(transparent)]
    Archive(#[from] ArchiveError),
}

impl BundleError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownItem { .. } => "jarvis.bundle_unknown_item",
            Self::RequiredItem { .. } => "jarvis.bundle_required_item",
            Self::MissingManifest => "jarvis.bundle_missing_manifest",
            Self::Redaction(_) => "jarvis.redactor_unavailable",
            Self::LogRead { .. } => "jarvis.bundle_log_read",
            Self::LogEncoding => "jarvis.bundle_log_encoding",
            Self::Archive(_) => "jarvis.bundle_archive",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::LogRead { .. })
    }
}

/// Materializes `plan`, excluding every path in `excluded`.
///
/// All content passes through `redactor` before it enters the bundle. A log file
/// is bounded to [`MAX_LOG_BYTES_PER_FILE`] bytes and read whole, so the tail
/// that is copied is a set of complete lines and never a partial line.
///
/// # Errors
///
/// Returns [`BundleError::MissingManifest`] when the plan has no manifest,
/// [`BundleError::LogRead`] or [`BundleError::LogEncoding`] for a log problem,
/// and [`BundleError::Archive`] when the archive bounds are exceeded.
pub fn materialize(
    plan: &BundlePlan,
    excluded: &[String],
    redactor: &Redactor,
) -> Result<SupportBundle, BundleError> {
    if !plan.items.iter().any(|item| item.path == MANIFEST_PATH) {
        return Err(BundleError::MissingManifest);
    }

    let mut files = Vec::new();
    for item in &plan.items {
        if excluded.contains(&item.path) {
            continue;
        }
        let raw = match item.source {
            BundleSource::Rendered => item.content.clone().unwrap_or_default(),
            BundleSource::LogTail => read_log_tail(item)?,
        };
        // One redaction pass over the finished text. `redact_line` also escapes
        // control characters, so a log record cannot inject an escape sequence
        // into the file that replaces the log it came from.
        let redacted = redact_text(&raw, redactor);
        files.push(BundleFile {
            path: item.path.clone(),
            bytes: redacted.into_bytes(),
        });
    }

    Ok(SupportBundle {
        files,
        excluded: excluded.to_vec(),
    })
}

/// The manifest path inside every bundle.
pub const MANIFEST_PATH: &str = "manifest.json";

/// Writes `bundle` to `destination` as a stored ZIP archive.
///
/// The archive is produced entirely in memory first, so a failure to read a
/// source cannot leave a half-written file at `destination`.
///
/// # Errors
///
/// Returns [`ArchiveError::TooLarge`] when the bundle exceeds
/// [`MAX_ARCHIVE_BYTES`], and [`BundleError::LogRead`]-class I/O errors from the
/// final write as [`ArchiveError`].
pub fn write_bundle(bundle: &SupportBundle, destination: &Path) -> Result<u64, BundleError> {
    let entries: Vec<(String, &[u8])> = bundle
        .files
        .iter()
        .map(|file| (file.path.clone(), file.bytes.as_slice()))
        .collect();
    let bytes = archive::build_stored_archive(&entries)?;
    std::fs::write(destination, bytes.bytes()).map_err(|_| BundleError::LogRead {
        path: destination.to_path_buf(),
    })?;
    Ok(bytes.bytes().len() as u64)
}

/// The result of exporting a bundle, for the operator's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutcome {
    /// The number of bytes written to the destination.
    pub bytes_written: u64,
    /// How many items the bundle contains.
    pub included: usize,
    /// How many items the user excluded.
    pub excluded: usize,
    /// The digest recorded in the manifest.
    pub digest: String,
}

/// Materializes `plan` and writes it to `destination` in one step.
///
/// This is the whole export, in the library, for one reason: the manifest
/// records the digest of the members, so it must be rendered **after** they are
/// materialized and **before** the archive is built. A caller that assembled
/// those steps itself could write a manifest whose digest does not describe the
/// archive, and nothing would notice. Keeping the order here makes the invariant
/// a property of the library, not of every caller.
///
/// # Errors
///
/// Returns any [`BundleError`] from materialization, and
/// [`BundleError::MissingManifest`] when the plan cannot carry a manifest.
pub fn export_bundle(
    plan: &BundlePlan,
    excluded: &[String],
    redactor: &Redactor,
    environment: &EnvironmentSummary,
    daemon: &DaemonSummary,
    destination: &Path,
) -> Result<ExportOutcome, BundleError> {
    let mut bundle = materialize(plan, excluded, redactor)?;

    // The digest covers every member *except* the manifest, because the manifest
    // contains the digest. Including it would be circular.
    let others: Vec<BundleFile> = bundle
        .files
        .iter()
        .filter(|file| file.path != MANIFEST_PATH)
        .map(|file| BundleFile {
            path: file.path.clone(),
            bytes: file.bytes.clone(),
        })
        .collect();
    let digest = digest_of(&others);
    let manifest = render_manifest(plan, environment, daemon, excluded, &digest);

    let Some(slot) = bundle
        .files
        .iter_mut()
        .find(|file| file.path == MANIFEST_PATH)
    else {
        return Err(BundleError::MissingManifest);
    };
    slot.bytes = manifest.into_bytes();

    let bytes_written = write_bundle(&bundle, destination)?;
    Ok(ExportOutcome {
        bytes_written,
        included: bundle.files.len(),
        excluded: bundle.excluded.len(),
        digest,
    })
}

/// Computes the SHA-256 of a bundle's file contents, as recorded in the manifest.
#[must_use]
pub fn digest_of(files: &[BundleFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        // The path is mixed in so that reordering or renaming an item changes
        // the digest; hashing content alone would let two different bundles
        // share a digest.
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(&file.bytes);
        hasher.update([0xff]);
    }
    hex(&hasher.finalize())
}

/// Renders the bundle manifest from a plan, an environment, and a daemon
/// summary.
///
/// The manifest records what was included, what the user excluded, and the
/// digest of the other files, so a reader can verify the archive is internally
/// consistent without trusting the sender's description of it.
#[must_use]
pub fn render_manifest(
    plan: &BundlePlan,
    environment: &EnvironmentSummary,
    daemon: &DaemonSummary,
    excluded: &[String],
    content_digest: &str,
) -> String {
    let mut out = String::from("{\n");
    out.push_str("  \"schema_version\": ");
    out.push_str(&SUPPORT_BUNDLE_SCHEMA_VERSION.to_string());
    out.push_str(",\n  \"environment\": ");
    out.push_str(&compact(&environment.to_json()));
    out.push_str(",\n  \"daemon\": ");
    out.push_str(&compact(&daemon.to_json()));
    out.push_str(",\n  \"items\": [\n");
    let mut included = 0;
    for (index, item) in plan.items().iter().enumerate() {
        let is_excluded = excluded.contains(&item.path);
        out.push_str("    {\"path\": ");
        out.push_str(&json_string(&item.path));
        out.push_str(", \"source\": ");
        out.push_str(&json_string(item.source.token()));
        out.push_str(", \"optional\": ");
        out.push_str(if item.optional { "true" } else { "false" });
        out.push_str(", \"included\": ");
        out.push_str(if is_excluded { "false" } else { "true" });
        out.push('}');
        // The comma is emitted *between* entries only. Appending it after every
        // entry produces a trailing comma, which is not valid JSON, and this is
        // exactly the kind of bug a hand-assembled document hides.
        if index + 1 != plan.items().len() {
            out.push(',');
        }
        out.push('\n');
        if !is_excluded {
            included += 1;
        }
    }
    out.push_str("  ],\n  \"included_items\": ");
    out.push_str(&included.to_string());
    out.push_str(",\n  \"excluded_items\": ");
    out.push_str(&excluded.len().to_string());
    out.push_str(",\n  \"content_sha256\": ");
    out.push_str(&json_string(content_digest));
    out.push_str(
        ",\n  \"retention\": \"Support bundles are diagnostic snapshots. They are not \
         retained by JARVIS, and JARVIS never uploads one. Delete this file when the \
         issue is resolved.\",\n  \"excluded_classes\": [\"secret_values\", \
         \"authorization_headers\", \"raw_environment\", \"database_file\", \
         \"prompts_and_messages\", \"documents\", \"tool_payloads\", \
         \"transcripts_and_audio\"]\n}\n",
    );
    out
}

/// Builds a bundle plan for a resolved profile.
///
/// The plan is deterministic for a given profile and report: the same inputs
/// produce the same item order, which is what makes the preview meaningful.
#[must_use]
pub fn plan_bundle(
    profile_mode: &'static str,
    api_major: u32,
    report: &CheckReport,
    daemon: &DaemonSummary,
) -> BundlePlan {
    let environment = EnvironmentSummary::current(profile_mode, api_major);
    let mut plan = BundlePlan::new();
    plan.push(PlanItem {
        path: MANIFEST_PATH.to_owned(),
        description: "schema, environment, item inventory, and content digest",
        source: BundleSource::Rendered,
        optional: false,
        source_path: None,
        content: Some(String::new()),
    });
    plan.push(PlanItem {
        path: "environment.json".to_owned(),
        description: "platform, arch, versions, and profile mode only",
        source: BundleSource::Rendered,
        optional: true,
        source_path: None,
        content: Some(environment.to_json()),
    });
    plan.push(PlanItem {
        path: "daemon.json".to_owned(),
        description: "daemon instance id, pid, and reported version",
        source: BundleSource::Rendered,
        optional: true,
        source_path: None,
        content: Some(daemon.to_json()),
    });
    plan.push(PlanItem {
        path: "doctor.json".to_owned(),
        description: "stable check codes, severities, and fixed advice",
        source: BundleSource::Rendered,
        optional: true,
        source_path: None,
        content: Some(report.to_json()),
    });
    plan
}

/// Adds bounded, redacted log tails to `plan`, up to the file-count bound.
///
/// Only files this function actually adds are returned, so a caller can state
/// exactly which files were read.
pub fn add_log_tails(plan: &mut BundlePlan, log_directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(log_directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();

    let total = files.len();
    let mut added = Vec::new();
    for path in files.into_iter().take(MAX_LOG_FILES) {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A log file name is sanitized before it becomes an archive path, so a
        // hostile name cannot escape the archive root or forge a directory.
        let safe = sanitize_archive_path(name);
        plan.push(PlanItem {
            path: format!("logs/{safe}"),
            description: "bounded redacted tail of one local log file",
            source: BundleSource::LogTail,
            optional: true,
            source_path: Some(path.clone()),
            content: None,
        });
        added.push(path);
    }
    // Counted from the files actually added, so a name this function could not
    // use is reported as omitted rather than silently disappearing.
    plan.set_omitted_log_files(total.saturating_sub(added.len()));
    added
}

/// Reads a bounded, complete-line tail from a log item's source file.
fn read_log_tail(item: &PlanItem) -> Result<String, BundleError> {
    let path = item.source_path.as_ref().ok_or(BundleError::LogEncoding)?;
    let bytes = std::fs::read(path).map_err(|_| BundleError::LogRead { path: path.clone() })?;
    let text = String::from_utf8_lossy(&bytes).into_owned();

    if text.len() <= MAX_LOG_BYTES_PER_FILE {
        return Ok(text);
    }
    // Keep the *newest* complete lines: a bundle is about the recent failure.
    let start = text.len() - MAX_LOG_BYTES_PER_FILE;
    let start = floor_char_boundary(&text, start);
    // Drop the leading partial line so the tail begins at a record boundary.
    match text[start..].find('\n') {
        Some(offset) => Ok(text[start + offset + 1..].to_owned()),
        None => Ok(text[start..].to_owned()),
    }
}

/// Returns the largest character boundary at or below `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut end = index.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Redacts every line of `text`, preserving line structure.
///
/// The text is split on newlines and each line is passed through
/// [`Redactor::redact_line`], which is the same call the log writer makes, so a
/// bundle cannot apply a weaker redaction than the logs it copies.
fn redact_text(text: &str, redactor: &Redactor) -> String {
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    // `split('\n')` keeps a trailing empty segment for text ending in a newline;
    // rejoining with `\n` restores the exact line structure.
    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(&redactor.redact_line(line));
    }
    out
}

/// Replaces path separators and leading dots in an archive path component.
///
/// A bundle path is data, not a filesystem operation, but a reader that extracts
/// the archive must not be given a component it could interpret as a directory
/// escape, and a name that merely *looks* like `..` is rejected in review as
/// suspicious even when it is harmless. Any character outside a plain name is
/// replaced with `_`, and a leading run of dots is replaced too, so the result
/// can never begin with `..`. A name made only of dots becomes `unnamed`.
#[must_use]
pub fn sanitize_archive_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for character in name.chars().take(96) {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    // A name of only dots is the traversal token itself.
    if out.chars().all(|character| character == '.') {
        return "unnamed".to_owned();
    }
    let leading_dots = out
        .chars()
        .take_while(|character| *character == '.')
        .count();
    if leading_dots > 0 {
        let mut fixed = String::with_capacity(out.len());
        fixed.extend(std::iter::repeat_n('_', leading_dots));
        fixed.push_str(&out[leading_dots..]);
        out = fixed;
    }
    if out.is_empty() {
        out.push_str("unnamed");
    }
    out
}

/// Renders a string as a JSON string literal.
///
/// The escaping is explicit rather than delegated to `serde_json`, because the
/// manifest is assembled as text: delegating only part of a document to a
/// serializer makes the unescaped parts the dangerous ones.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters are escaped rather than removed, so the JSON
            // stays valid without silently altering a value.
            character if u32::from(character) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// Pushes an optional string as a JSON value or `null`.
fn push_optional(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => out.push_str(&json_string(value)),
        None => out.push_str("null"),
    }
}

/// Collapses a pretty-printed JSON document onto one line for embedding.
fn compact(document: &str) -> String {
    document.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Formats bytes as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Collects one log file into a redacted archive member.
///
/// This exists so a caller that already knows its source can reuse the same
/// bounded, redacted read path the plan uses.
///
/// # Errors
///
/// Returns [`BundleError::LogRead`] when the file cannot be read.
pub fn redacted_log_bytes(path: &Path, redactor: &Redactor) -> Result<Vec<u8>, BundleError> {
    let item = PlanItem {
        path: "logs/log".to_owned(),
        description: "bounded redacted tail of one local log file",
        source: BundleSource::LogTail,
        optional: true,
        source_path: Some(path.to_path_buf()),
        content: None,
    };
    let text = read_log_tail(&item)?;
    Ok(redact_text(&text, redactor).into_bytes())
}

/// Returns the marker substitution redaction performs.
///
/// Exposed so a caller can assert on exactly the token the redactor produces
/// instead of hard-coding the constant in two places.
#[must_use]
pub const fn redaction_marker() -> &'static str {
    REDACTED
}

/// Sanitizes an untrusted single-line string for terminal output.
///
/// Re-exported so a caller in this crate does not need to depend on
/// `jarvis-observability` directly for one function.
#[must_use]
pub fn sanitize_line(value: &str) -> String {
    sanitize_control_chars(value)
}

/// Groups findings by severity for a summary table.
#[must_use]
pub fn severity_totals(report: &CheckReport) -> BTreeMap<&'static str, usize> {
    let mut totals: BTreeMap<&'static str, usize> = BTreeMap::new();
    for finding in report.findings() {
        *totals.entry(finding.severity.token()).or_default() += 1;
    }
    totals
}
