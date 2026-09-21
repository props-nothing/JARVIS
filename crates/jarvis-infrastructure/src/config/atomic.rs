//! Bounded reads and atomic replacement for configuration files.
//!
//! A config write must never leave a truncated or partially written active file.
//! The sequence is: create a fresh owner-only temp file in the destination
//! directory, write and flush it, replace the destination, then flush the
//! directory entry. `std::fs::rename` is the atomic replace primitive on both
//! Unix and Windows (it maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`).

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::ConfigError;

/// The maximum accepted configuration file size, in bytes.
///
/// A config file is small operator input, not bulk data. Bounding it before
/// parsing keeps a hostile or accidental huge file from consuming memory.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Distinguishes temp files created within one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Reads a file with a hard byte bound.
///
/// # Errors
///
/// Returns [`ConfigError::TooLarge`] when the file exceeds
/// [`MAX_CONFIG_BYTES`], and [`ConfigError::Read`] for any other I/O failure.
pub fn read_bounded(path: &Path) -> Result<Vec<u8>, ConfigError> {
    let metadata = fs::metadata(path).map_err(|_| ConfigError::Read {
        path: path.to_path_buf(),
    })?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge {
            limit: MAX_CONFIG_BYTES,
        });
    }

    let file = fs::File::open(path).map_err(|_| ConfigError::Read {
        path: path.to_path_buf(),
    })?;
    // Bound the read itself, not only the metadata check: the file could grow
    // between the two calls.
    let mut contents = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| ConfigError::Read {
            path: path.to_path_buf(),
        })?;
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge {
            limit: MAX_CONFIG_BYTES,
        });
    }
    Ok(contents)
}

/// Atomically replaces `path` with `bytes`.
///
/// The previous file is left intact when any step fails, and no partially
/// written file ever becomes `path`.
///
/// # Errors
///
/// Returns [`ConfigError::Write`] when the parent directory, temp file,
/// replacement, or directory flush fails.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let parent = path.parent().ok_or_else(|| ConfigError::Write {
        path: path.to_path_buf(),
    })?;
    if !ensure_parent_dir(parent) {
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
        });
    }

    let temp = temp_path(path);
    // `create_new` refuses to follow or reuse an existing entry, so a symlink
    // planted at the temp name cannot redirect the write.
    if !write_temp_file(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
        });
    }

    if fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(&temp);
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
        });
    }

    // Flushing the directory makes the rename durable, not just visible.
    if !sync_directory(parent) {
        return Err(ConfigError::Write {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Builds a unique sibling temp path so the replace stays on one filesystem.
fn temp_path(path: &Path) -> PathBuf {
    let stem = path.file_name().map_or_else(
        || "config".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(".{stem}.tmp.{}.{unique}", std::process::id());
    path.with_file_name(name)
}

/// Creates parent directories, owner-only on Unix.
fn ensure_parent_dir(parent: &Path) -> bool {
    parent.is_dir() || create_dir_private(parent)
}

#[cfg(unix)]
fn create_dir_private(path: &Path) -> bool {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(path).is_ok()
}

#[cfg(not(unix))]
fn create_dir_private(path: &Path) -> bool {
    fs::create_dir_all(path).is_ok()
}

#[cfg(unix)]
fn write_temp_file(path: &Path, bytes: &[u8]) -> bool {
    use std::os::unix::fs::OpenOptionsExt as _;

    let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    else {
        return false;
    };
    file.write_all(bytes).is_ok() && file.sync_all().is_ok()
}

#[cfg(not(unix))]
fn write_temp_file(path: &Path, bytes: &[u8]) -> bool {
    let Ok(mut file) = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return false;
    };
    file.write_all(bytes).is_ok() && file.sync_all().is_ok()
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> bool {
    fs::File::open(dir).is_ok_and(|handle| handle.sync_all().is_ok())
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> bool {
    // Windows does not support flushing a directory handle through this API.
    true
}
