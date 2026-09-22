//! Durable repository ports.
//!
//! The ports live in the application layer because `docs/architecture/domain-boundaries.md`
//! gives it "repository, clock, ID, model, runtime, tool, event, secret, and audit
//! ports", and `docs/architecture/storage-data.md` requires that "core domain code
//! depends on repository traits, not `SQLx` types". An adapter in
//! `jarvis-infrastructure` implements these; nothing here names a database.
//!
//! The method set is deliberately purpose-specific rather than generic CRUD, as
//! `storage-data.md` requires. [`RunRepository::transition`] is the important one:
//! it enforces the expected version and state and reports a conflict instead of
//! silently overwriting concurrent work, and it appends the run's durable activity
//! event **in the same transaction** — the first of that document's required atomic
//! use cases, "transition run state and append its durable activity event".
//!
//! Scope is a parameter, not an implicit filter. Every read takes a
//! [`WorkspaceId`], and a run from another workspace is reported as absent rather
//! than as forbidden, because the local control API requires that "a run from
//! another scope is indistinguishable from a missing run".

pub mod conversation;
pub mod model_call;
pub mod run;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// Why a repository operation failed.
///
/// These are storage-level outcomes and are deliberately **not** the domain's
/// errors: a missing row and an illegal transition are different facts, and
/// collapsing them would make a caller unable to tell "the run is gone" from "the
/// run cannot go there". The domain's own refusal is carried through unchanged in
/// [`RepositoryError::TransitionRefused`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    /// A row the caller named does not exist in the requested scope.
    ///
    /// Carries no distinction between "never existed" and "belongs to another
    /// workspace", because the API requires those to be indistinguishable.
    NotFound,
    /// The stored version did not match the version the caller expected.
    VersionConflict {
        /// The version the caller expected.
        expected: u64,
        /// The version the row actually held.
        actual: u64,
    },
    /// The domain refused the transition.
    ///
    /// The refusal is preserved as its code rather than flattened, so a caller can
    /// distinguish "the edge does not exist" from "your view is stale" — the same
    /// distinction the domain's own ordering is designed to preserve.
    TransitionRefused {
        /// The stable, namespaced domain error code.
        code: &'static str,
    },
    /// A stored value could not be interpreted.
    ///
    /// This is a distinct variant on purpose: a row whose state string the domain
    /// does not recognize is **corruption**, not a row to skip. Silently treating
    /// it as absent would turn a migration problem into an apparent data loss.
    Corrupted {
        /// The column that could not be interpreted.
        column: &'static str,
    },
    /// A uniqueness constraint was violated.
    Conflict {
        /// A stable identifier for what conflicted.
        what: &'static str,
    },
    /// The driver or connection failed.
    Query,
}

impl RepositoryError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "storage.not_found",
            Self::VersionConflict { .. } => "storage.version_conflict",
            Self::TransitionRefused { .. } => "storage.transition_refused",
            Self::Corrupted { .. } => "storage.row_corrupted",
            Self::Conflict { .. } => "storage.conflict",
            Self::Query => "storage.query_failed",
        }
    }

    /// Returns whether retrying the same operation unchanged could succeed.
    ///
    /// A version conflict is retryable in the sense that a caller re-reading and
    /// recomputing can succeed — but the retry is only meaningful with new
    /// information, so it is reported as **not** blindly retryable. The transport
    /// failure is the one outcome a blind retry may fix.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Query)
    }
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Every message is fixed text naming no row contents, no bound parameter,
        // and no path, because a repository error reaches logs and support bundles.
        let text = match self {
            Self::NotFound => "the record was not found in this workspace",
            Self::VersionConflict { .. } => "the record was modified concurrently",
            Self::TransitionRefused { .. } => "the state transition was refused",
            Self::Corrupted { .. } => "a stored value could not be interpreted",
            Self::Conflict { .. } => "a uniqueness constraint was violated",
            Self::Query => "a database operation failed",
        };
        formatter.write_str(text)
    }
}

/// A boxed future returned by an async repository method.
///
/// The ports are object safe — an adapter is injected as `Arc<dyn RunRepository>`
/// — so their async methods return a boxed future rather than using `async fn` in
/// a trait, which is not dyn compatible.
pub type RepositoryFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RepositoryError>> + Send + 'a>>;

#[cfg(test)]
mod tests {
    use super::RepositoryError;

    #[test]
    fn every_code_is_namespaced_and_unique() {
        let errors = [
            RepositoryError::NotFound,
            RepositoryError::VersionConflict {
                expected: 1,
                actual: 2,
            },
            RepositoryError::TransitionRefused {
                code: "jarvis.run_transition_not_allowed",
            },
            RepositoryError::Corrupted { column: "state" },
            RepositoryError::Conflict {
                what: "step_sequence",
            },
            RepositoryError::Query,
        ];
        let mut codes: Vec<&str> = errors.iter().map(RepositoryError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len(), "codes must be unique");
        for code in codes {
            assert!(
                code.starts_with("storage."),
                "{code} must be namespaced for storage",
            );
        }
    }

    #[test]
    fn only_a_transport_failure_is_blindly_retryable() {
        // A version conflict is resolvable, but only by re-reading and
        // recomputing, so it is not reported as retryable-unchanged. The one
        // outcome a blind retry may fix is the transport failure.
        assert!(RepositoryError::Query.retryable());
        for error in [
            RepositoryError::NotFound,
            RepositoryError::VersionConflict {
                expected: 1,
                actual: 2,
            },
            RepositoryError::TransitionRefused {
                code: "jarvis.run_transition_not_allowed",
            },
            RepositoryError::Corrupted { column: "state" },
            RepositoryError::Conflict { what: "run" },
        ] {
            assert!(
                !error.retryable(),
                "{} must not be blindly retryable",
                error.code()
            );
        }
    }

    #[test]
    fn a_missing_record_and_a_foreign_record_are_the_same_error() {
        // The API requires a foreign record to be indistinguishable from a missing
        // one, so there is deliberately no variant that could leak the distinction.
        let missing = RepositoryError::NotFound;
        let foreign = RepositoryError::NotFound;
        assert_eq!(missing, foreign);
        assert_eq!(missing.code(), "storage.not_found");
    }

    #[test]
    fn a_display_message_never_carries_a_value() {
        // The error reaches logs and support bundles, so the rendered text is
        // fixed rather than interpolated with a bound parameter or a path.
        let error = RepositoryError::VersionConflict {
            expected: 7,
            actual: 9,
        };
        let rendered = error.to_string();
        assert!(!rendered.contains('7'), "{rendered}");
        assert!(!rendered.contains('9'), "{rendered}");
        assert_eq!(rendered, "the record was modified concurrently");
    }
}
