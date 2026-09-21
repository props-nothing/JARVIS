//! Single-instance ownership.
//!
//! Exactly one daemon may own a profile. The guard is a held, exclusively locked
//! file handle: the operating system releases it when the process ends, so a
//! crash cannot leave a permanent lock. The file's *contents* are diagnostic
//! only and PID text is never treated as proof of ownership.
//!
//! Platform note: an exclusive lock is whole-file on Windows, so the recorded
//! PID is **not** reliably readable through a second handle while the lock is
//! held. Diagnostics therefore report the lock as held rather than trying to
//! read a holder PID from a live lock.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// An error raised while claiming single-instance ownership.
#[derive(Debug, Error)]
pub enum InstanceError {
    /// The lock file could not be created or opened.
    #[error("the instance lock file could not be created")]
    LockFile {
        /// The lock path.
        path: PathBuf,
    },
    /// Another process already holds the lock.
    #[error("another JARVIS daemon already owns this profile")]
    AlreadyHeld,
    /// The lock could not be established for an unexpected reason.
    #[error("the instance lock could not be established")]
    LockFailed,
}

impl InstanceError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LockFile { .. } => "jarvis.instance_lock_file",
            Self::AlreadyHeld => "jarvis.instance_already_held",
            Self::LockFailed => "jarvis.instance_lock_failed",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    ///
    /// A held lock is deterministic; retrying cannot succeed until the owner
    /// exits.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::LockFile { .. } | Self::LockFailed => true,
            Self::AlreadyHeld => false,
        }
    }
}

/// A held single-instance lock.
///
/// Dropping the guard releases the lock, because the exclusive lock lives on the
/// open handle.
#[derive(Debug)]
pub struct InstanceGuard {
    path: PathBuf,
    file: fs::File,
}

impl InstanceGuard {
    /// Claims exclusive ownership of `path`.
    ///
    /// # Errors
    ///
    /// Returns [`InstanceError::AlreadyHeld`] when another process holds the
    /// lock and [`InstanceError::LockFile`] when the file cannot be used.
    pub fn acquire(path: &Path) -> Result<Self, InstanceError> {
        if let Some(parent) = path.parent() {
            create_private_dir(parent).map_err(|()| InstanceError::LockFile {
                path: path.to_path_buf(),
            })?;
        }

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| InstanceError::LockFile {
                path: path.to_path_buf(),
            })?;

        // A non-blocking exclusive lock is the ownership test. Re-locking the
        // same handle is deliberately avoided because its behavior is platform
        // dependent.
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(InstanceError::AlreadyHeld),
            Err(fs::TryLockError::Error(_)) => return Err(InstanceError::LockFailed),
        }

        let mut guard = Self {
            path: path.to_path_buf(),
            file,
        };
        // The recorded PID is diagnostic only and is written after the lock is
        // held, so a reader never sees a PID claiming an unheld lock.
        guard.record_pid();
        Ok(guard)
    }

    /// Returns the lock file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the holder PID as a diagnostic aid. Failure is not fatal.
    fn record_pid(&mut self) {
        let pid = std::process::id().to_string();
        let _ = self.file.set_len(0);
        let _ = self.file.write_all(pid.as_bytes());
        let _ = self.file.sync_all();
    }
}

/// Creates a directory owner-only on Unix.
fn create_private_dir(path: &Path) -> Result<(), ()> {
    if path.is_dir() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(|_| ())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path).map_err(|_| ())
    }
}

/// Returns whether `path` currently has no holder, for stale-state reporting.
///
/// This never proves ownership; it only distinguishes "no holder" from
/// "possibly held" for an operator message.
#[must_use]
pub fn appears_unheld(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{InstanceGuard, appears_unheld};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jarvis-fnd007-lock-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_free_lock_can_be_acquired_and_is_reported_held() {
        let dir = temp_dir("free");
        let path = dir.join("jarvis.lock");
        let guard = InstanceGuard::acquire(&path).expect("a free lock is acquirable");
        assert_eq!(guard.path(), &path);
        assert!(path.exists(), "the lock file is created");
        assert!(
            !appears_unheld(&path),
            "the lock is reported held while the guard lives",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_attempt_reports_already_held() {
        let dir = temp_dir("contended");
        let path = dir.join("jarvis.lock");
        let _first = InstanceGuard::acquire(&path).expect("first acquire");

        // A second handle in the same process contends with the first.
        let error = InstanceGuard::acquire(&path).expect_err("second acquire must be refused");
        assert_eq!(error.code(), "jarvis.instance_already_held");
        assert!(!error.retryable());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropping_the_guard_releases_the_lock() {
        let dir = temp_dir("release");
        let path = dir.join("jarvis.lock");
        {
            let _guard = InstanceGuard::acquire(&path).expect("first acquire");
        }
        // After the guard drops, a new owner can claim the profile.
        let _reacquired = InstanceGuard::acquire(&path).expect("lock must be reusable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_unlocked_file_is_reported_as_unheld() {
        let dir = temp_dir("stale");
        let path = dir.join("jarvis.lock");
        std::fs::write(&path, b"999999").expect("write a stale pid");
        assert!(appears_unheld(&path), "an unlocked file is repairable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_lock_file_is_reported_as_unheld() {
        let dir = temp_dir("missing");
        assert!(appears_unheld(&dir.join("absent.lock")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_release_is_idempotent_across_guards() {
        let dir = temp_dir("idempotent");
        let path = dir.join("jarvis.lock");
        for _ in 0..3 {
            let guard = InstanceGuard::acquire(&path).expect("acquire after release");
            drop(guard);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
