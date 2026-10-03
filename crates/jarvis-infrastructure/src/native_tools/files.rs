//! The file tools: list, read and write inside operator-declared roots.
//!
//! These are the first native tools that touch the user's own data, so the whole design is about what they
//! *cannot* do. Three rules, each enforced by a different layer so no single mistake opens the filesystem:
//!
//! - **Rooted by configuration, never by the model.** The only places a tool can reach are the roots the
//!   operator declared in `[[tools.files.roots]]`, each with a name and a mode (`read` or `read_write`). The
//!   model names a root and a path *inside* it; it can neither invent a root nor widen one.
//! - **The path is a shape-checked relative path.** [`WorkspaceRelativePath::denial_for`] is the domain's own
//!   rule — no absolute paths, no `..`, no backslashes, no alternate data streams, no Windows reserved
//!   names — and a refusal is reported as `tool.schema_invalid` because the arguments are what is wrong.
//! - **The open is capability-based.** Every file is opened *relative to a [`cap_std::fs::Dir`]*, and
//!   `cap-std` resolves the path inside the directory and refuses to follow a symlink out of it. That is the
//!   piece a check-then-open cannot give: between a `canonicalize` and an `open` the filesystem can change,
//!   and the architecture names that race (`TLS-007`). See
//!   `docs/research/integrations/filesystem-capability.md` for the review.
//!
//! What a write does is deliberately narrow: **create a file, or replace one when asked to**. It never
//! creates directories, never follows a link out of the root, and refuses a read-only root. Everything is
//! bounded — path length, content size, bytes read, entries listed — so one call cannot exhaust memory or the
//! model's context.
//!
//! The tools are not offered at all when no root is declared, so a fresh profile exposes no filesystem
//! capability.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use jarvis_application::tool_call::ToolExecutionError;
use jarvis_domain::error::DomainError;
use jarvis_domain::model::policy::Sensitivity;
use jarvis_domain::tool::call::{ContentBlock, ToolResultBody};
use jarvis_domain::tool::classification::{
    ApprovalHint, DataClasses, Effect, ExecutionDefaults, Idempotency, Risk,
};
use jarvis_domain::tool::definition::ToolDefinition;
use jarvis_domain::tool::error_class::ToolErrorClass;
use jarvis_domain::tool::identity::{
    SchemaFingerprint, SourceKind, ToolCapability, ToolIdentity, ToolSource, ToolVersion,
};
use jarvis_domain::tool::path_grant::WorkspaceRelativePath;

use super::Definition;
use crate::tool_fingerprint::schema_fingerprint_of;

/// The capability of the directory-listing tool.
pub const LIST_CAPABILITY: &str = "files.list@1";
/// The capability of the file-reading tool.
pub const READ_CAPABILITY: &str = "files.read@1";
/// The capability of the file-writing tool.
pub const WRITE_CAPABILITY: &str = "files.write@1";

/// The most bytes one read returns unless the caller asks for fewer.
pub const DEFAULT_READ_BYTES: usize = 32 * 1024;
/// The most bytes one read can ever return, which keeps a result inside the model's context.
pub const MAX_READ_BYTES: usize = 128 * 1024;
/// The most bytes one write accepts. Below the tool-call argument bound, so a write is refused here with a
/// reason the model can act on rather than by the transport with a generic one.
pub const MAX_WRITE_BYTES: usize = 48 * 1024;
/// The most entries one listing returns.
pub const MAX_LIST_ENTRIES: usize = 200;
/// The longest root name.
pub const MAX_ROOT_NAME_LEN: usize = 32;

/// The input schema of `files.list@1`.
pub const LIST_SCHEMA: &str = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":{"root":{"type":"string","minLength":1,"maxLength":32,"description":"A configured root name."},"path":{"type":"string","maxLength":1024,"description":"A directory inside the root, relative to it. Empty lists the root itself."}},"required":["root"]}"#;

/// The input schema of `files.read@1`.
pub const READ_SCHEMA: &str = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":{"root":{"type":"string","minLength":1,"maxLength":32,"description":"A configured root name."},"path":{"type":"string","minLength":1,"maxLength":1024,"description":"A file inside the root, relative to it."},"max_bytes":{"type":"integer","minimum":1,"maximum":131072,"description":"The most bytes to return. Defaults to 32768."}},"required":["root","path"]}"#;

/// The input schema of `files.write@1`.
pub const WRITE_SCHEMA: &str = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"properties":{"root":{"type":"string","minLength":1,"maxLength":32,"description":"A configured root name that allows writing."},"path":{"type":"string","minLength":1,"maxLength":1024,"description":"The file to write inside the root, relative to it. Its folder must already exist."},"content":{"type":"string","maxLength":49152,"description":"The text to write."},"overwrite":{"type":"boolean","description":"Replace the file if it exists. Defaults to false, which refuses to overwrite."}},"required":["root","path","content"]}"#;

/// One declared root, as the daemon was configured with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDeclaration {
    /// The name the model uses.
    pub name: String,
    /// The directory on disk. Absolute.
    pub path: PathBuf,
    /// Whether the root may be written to.
    pub writable: bool,
}

/// What the tool definitions need to say about a root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootInfo {
    /// The name the model uses.
    pub name: String,
    /// Whether the root may be written to.
    pub writable: bool,
}

/// Why a root could not be opened at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootError {
    /// The directory does not exist or cannot be opened.
    Unavailable,
    /// Two roots share a name.
    DuplicateName,
}

struct Root {
    name: String,
    dir: Dir,
    writable: bool,
}

/// The opened roots: one directory capability each.
///
/// Opening is the only place the daemon uses *ambient* filesystem authority, and it does so once, for the
/// operator's own configured paths. Everything after holds a capability to a directory and nothing else.
#[derive(Clone)]
pub struct FileRoots {
    roots: Arc<Vec<Root>>,
}

impl std::fmt::Debug for FileRoots {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names only: a root's path is operator configuration and a debug line is not a place to copy it.
        formatter
            .debug_struct("FileRoots")
            .field(
                "roots",
                &self
                    .roots
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl FileRoots {
    /// Opens every declared root.
    ///
    /// # Errors
    ///
    /// Returns [`RootError::Unavailable`] when a directory cannot be opened and
    /// [`RootError::DuplicateName`] for a repeated name. A root that cannot be opened fails startup rather
    /// than being skipped: a tool offered over a root that is not there would fail on every call, and a
    /// silently missing root is a configuration fault an operator should see.
    pub fn open(declared: &[RootDeclaration]) -> Result<Self, RootError> {
        let mut roots: Vec<Root> = Vec::with_capacity(declared.len());
        for declaration in declared {
            if roots.iter().any(|root| root.name == declaration.name) {
                return Err(RootError::DuplicateName);
            }
            let dir = Dir::open_ambient_dir(&declaration.path, ambient_authority())
                .map_err(|_| RootError::Unavailable)?;
            roots.push(Root {
                name: declaration.name.clone(),
                dir,
                writable: declaration.writable,
            });
        }
        Ok(Self {
            roots: Arc::new(roots),
        })
    }

    /// Returns what the definitions say about each root, in declaration order.
    #[must_use]
    pub fn info(&self) -> Vec<RootInfo> {
        self.roots
            .iter()
            .map(|root| RootInfo {
                name: root.name.clone(),
                writable: root.writable,
            })
            .collect()
    }

    fn root(&self, name: &str) -> Option<&Root> {
        self.roots.iter().find(|root| root.name == name)
    }
}

/// Builds the reviewed definitions for the declared roots.
///
/// **The roots appear in the tool's purpose, not in its schema.** The schema is part of a tool's identity
/// (its fingerprint is what grants and approvals bind to), so putting root names in it would change the
/// identity — and invalidate every stored grant — each time an operator edited a path. The purpose is text
/// the model reads, which is exactly where "these are the roots" belongs.
///
/// # Errors
///
/// Returns a construction refusal from the domain when a reviewed constant is inconsistent.
pub fn definitions(roots: &[RootInfo]) -> Result<Vec<Definition>, DomainError> {
    let listing = roots
        .iter()
        .map(|root| {
            format!(
                "{} ({})",
                root.name,
                if root.writable {
                    "read/write"
                } else {
                    "read-only"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let any_writable = roots.iter().any(|root| root.writable);
    let mut out = vec![
        build(
            LIST_CAPABILITY,
            LIST_SCHEMA,
            "List files",
            &format!(
                "Lists the files and folders in a directory inside a configured root. Roots: {listing}."
            ),
            vec![Effect::ReadOnly],
            Risk::Low,
            ApprovalHint::Allow,
            Idempotency::NaturallyIdempotent,
            (Sensitivity::Internal, Sensitivity::Confidential),
        )?,
        build(
            READ_CAPABILITY,
            READ_SCHEMA,
            "Read a file",
            &format!(
                "Reads a text file inside a configured root and returns up to 128 KiB of it. Roots: {listing}."
            ),
            vec![Effect::ReadOnly],
            Risk::Low,
            ApprovalHint::Allow,
            Idempotency::NaturallyIdempotent,
            (Sensitivity::Internal, Sensitivity::Confidential),
        )?,
    ];
    // A write tool is offered only when some root allows writing: a tool that can never succeed is noise the
    // model would keep trying.
    if any_writable {
        out.push(build(
            WRITE_CAPABILITY,
            WRITE_SCHEMA,
            "Write a file",
            &format!(
                "Creates a text file inside a configured root, or replaces one when overwrite is true. Roots: {listing}."
            ),
            vec![Effect::Write],
            Risk::Moderate,
            ApprovalHint::Ask,
            Idempotency::None,
            (Sensitivity::Internal, Sensitivity::Internal),
        )?);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    capability: &str,
    schema: &str,
    display: &str,
    purpose: &str,
    effects: Vec<Effect>,
    risk: Risk,
    approval: ApprovalHint,
    idempotency: Idempotency,
    classes: (Sensitivity, Sensitivity),
) -> Result<Definition, DomainError> {
    let identity = ToolIdentity {
        capability: ToolCapability::parse(capability)?,
        source: ToolSource::new(
            SourceKind::Native,
            "jarvis.core",
            ToolVersion::parse("1.0.0")?,
        )?,
        // Derived from the schema text, never written by hand, so editing a schema moves the identity.
        schema_fingerprint: SchemaFingerprint::parse(&schema_fingerprint_of(schema).to_string())?,
    };
    let definition = ToolDefinition::new(
        identity,
        display,
        purpose,
        effects,
        risk,
        Vec::new(),
        approval,
        idempotency,
        DataClasses::new(classes.0, classes.1)?,
        ExecutionDefaults::new(10_000, 1)?,
    )?;
    Ok(Definition {
        definition,
        input_schema: schema.to_owned(),
    })
}

type Operation = fn(&FileRoots, &serde_json::Value) -> Result<serde_json::Value, ToolErrorClass>;

/// Runs one file tool.
///
/// Blocking filesystem work, so the caller runs it on the blocking pool. Returns `None` when `capability`
/// is not a file tool, so the dispatcher can refuse it rather than guess.
#[must_use]
pub fn execute(
    roots: &FileRoots,
    capability: &str,
    arguments: &str,
) -> Option<Result<ToolResultBody, ToolExecutionError>> {
    let operation: Operation = match capability {
        LIST_CAPABILITY => list,
        READ_CAPABILITY => read,
        WRITE_CAPABILITY => write,
        _ => return None,
    };
    Some(finish(
        parse(arguments).and_then(|value| operation(roots, &value)),
    ))
}

/// Returns whether `capability` names a file tool.
#[must_use]
pub fn is_file_tool(capability: &str) -> bool {
    matches!(
        capability,
        LIST_CAPABILITY | READ_CAPABILITY | WRITE_CAPABILITY
    )
}

fn finish(
    outcome: Result<serde_json::Value, ToolErrorClass>,
) -> Result<ToolResultBody, ToolExecutionError> {
    let value = outcome.map_err(ToolExecutionError::Failed)?;
    let block = ContentBlock::json(&value.to_string())
        .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))?;
    ToolResultBody::new(vec![block], None, Sensitivity::Confidential)
        .map_err(|_| ToolExecutionError::Failed(ToolErrorClass::OutputInvalid))
}

fn parse(arguments: &str) -> Result<serde_json::Value, ToolErrorClass> {
    serde_json::from_str(arguments).map_err(|_| ToolErrorClass::SchemaInvalid)
}

fn text<'a>(arguments: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(serde_json::Value::as_str)
}

/// Resolves the named root and checks the relative path's shape.
///
/// An empty path (or `.`) means the root itself and is only allowed when `allow_root` says so.
fn resolve<'a>(
    roots: &'a FileRoots,
    arguments: &serde_json::Value,
    allow_root: bool,
) -> Result<(&'a Root, String), ToolErrorClass> {
    let name = text(arguments, "root").ok_or(ToolErrorClass::SchemaInvalid)?;
    // An unknown root is a permission failure, not a missing file: the model asked for a place it was never
    // given, and "not found" would invite it to guess other names.
    let root = roots.root(name).ok_or(ToolErrorClass::PermissionDenied)?;
    let path = text(arguments, "path").unwrap_or("");
    if path.is_empty() || path == "." {
        return if allow_root {
            Ok((root, ".".to_owned()))
        } else {
            Err(ToolErrorClass::SchemaInvalid)
        };
    }
    if WorkspaceRelativePath::denial_for(path).is_some() {
        return Err(ToolErrorClass::SchemaInvalid);
    }
    Ok((root, path.to_owned()))
}

/// Maps an I/O error from a capability-relative operation onto the tool error classes.
///
/// `cap-std` reports a path that would leave the root, or a symlink out of it, as a permission error, so
/// the escape attempt and an ordinary denied file read the same to the model — which is the point.
fn class_of(error: &std::io::Error) -> ToolErrorClass {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => ToolErrorClass::NotFound,
        ErrorKind::AlreadyExists => ToolErrorClass::Conflict,
        ErrorKind::PermissionDenied => ToolErrorClass::PermissionDenied,
        ErrorKind::InvalidInput | ErrorKind::InvalidData => ToolErrorClass::SchemaInvalid,
        _ => ToolErrorClass::ProviderError,
    }
}

fn list(
    roots: &FileRoots,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolErrorClass> {
    let (root, path) = resolve(roots, arguments, true)?;
    let entries = root.dir.read_dir(&path).map_err(|e| class_of(&e))?;
    let mut found: Vec<(String, &'static str, Option<u64>)> = Vec::new();
    let mut truncated = false;
    for entry in entries {
        let entry = entry.map_err(|e| class_of(&e))?;
        // A name that is not UTF-8 cannot be written back by the model, so it is not listed.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let metadata = entry.metadata().map_err(|e| class_of(&e))?;
        let (kind, size) = if metadata.is_dir() {
            ("dir", None)
        } else if metadata.is_file() {
            ("file", Some(metadata.len()))
        } else {
            // Links and devices are listed as what they are not allowed to be followed as.
            ("other", None)
        };
        found.push((name, kind, size));
        if found.len() > MAX_LIST_ENTRIES {
            truncated = true;
            found.pop();
            break;
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    let entries: Vec<serde_json::Value> = found
        .into_iter()
        .map(|(name, kind, size)| serde_json::json!({ "name": name, "kind": kind, "bytes": size }))
        .collect();
    Ok(
        serde_json::json!({ "root": root.name, "path": path, "entries": entries, "truncated": truncated }),
    )
}

fn read(
    roots: &FileRoots,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolErrorClass> {
    let (root, path) = resolve(roots, arguments, false)?;
    let limit = arguments
        .get("max_bytes")
        .and_then(serde_json::Value::as_u64)
        .map_or(DEFAULT_READ_BYTES, |n| {
            usize::try_from(n).unwrap_or(MAX_READ_BYTES)
        })
        .clamp(1, MAX_READ_BYTES);
    let file = root.dir.open(&path).map_err(|e| class_of(&e))?;
    let metadata = file.metadata().map_err(|e| class_of(&e))?;
    if !metadata.is_file() {
        return Err(ToolErrorClass::SchemaInvalid);
    }
    // One byte past the limit says whether the file was cut, without reading the rest of a large file.
    let mut buffer = Vec::with_capacity(limit.min(8 * 1024) + 1);
    file.take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut buffer)
        .map_err(|e| class_of(&e))?;
    let truncated = buffer.len() > limit;
    buffer.truncate(limit);
    let content = match String::from_utf8(buffer) {
        Ok(content) => content,
        Err(error) => {
            // A cut can land inside a multi-byte character; that is a truncation, not a binary file.
            let valid = error.utf8_error().valid_up_to();
            let incomplete_tail = error.utf8_error().error_len().is_none();
            if truncated && incomplete_tail {
                String::from_utf8_lossy(&error.into_bytes()[..valid]).into_owned()
            } else {
                return Err(ToolErrorClass::OutputInvalid);
            }
        }
    };
    Ok(serde_json::json!({
        "root": root.name,
        "path": path,
        "bytes": metadata.len(),
        "truncated": truncated,
        "content": content,
    }))
}

fn write(
    roots: &FileRoots,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolErrorClass> {
    let (root, path) = resolve(roots, arguments, false)?;
    if !root.writable {
        return Err(ToolErrorClass::PermissionDenied);
    }
    let content = text(arguments, "content").ok_or(ToolErrorClass::SchemaInvalid)?;
    if content.len() > MAX_WRITE_BYTES {
        return Err(ToolErrorClass::LimitExceeded);
    }
    let overwrite = arguments
        .get("overwrite")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut options = OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        // `create_new` fails if the file exists, atomically, so "do not overwrite" is the filesystem's
        // decision rather than a check this code could race.
        options.create_new(true);
    }
    let mut file = root
        .dir
        .open_with(&path, &options)
        .map_err(|e| class_of(&e))?;
    file.write_all(content.as_bytes())
        .map_err(|e| class_of(&e))?;
    file.sync_all().map_err(|e| class_of(&e))?;
    Ok(serde_json::json!({
        "root": root.name,
        "path": path,
        "bytes_written": content.len(),
        "overwrote": overwrite,
    }))
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
