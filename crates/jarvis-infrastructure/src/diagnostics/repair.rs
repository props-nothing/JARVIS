//! Previewed, confirmed repair plans with rollback and verified postconditions.
//!
//! A repair is the one place in JARVIS that mutates durable local state on an
//! operator's behalf, so it follows the same discipline the service controllers
//! use: a repair is a **plan value first**.
//!
//! 1. A [`RepairPlan`] is built from a [`Finding`]. It lists the exact
//!    [`RepairAction`]s, each with the path it touches and a stable kind.
//!    Building a plan performs no side effects, so it can be printed, asserted,
//!    and tested on any host.
//! 2. Applying a plan requires explicit confirmation. [`apply`] refuses an
//!    unconfirmed plan, so no caller can treat "a plan exists" as "proceed".
//! 3. Before mutating anything, the plan's own postcondition is checked. If it
//!    already holds, the plan is refused as [`RepairError::AlreadySatisfied`]: a
//!    repair that reports success without changing anything is indistinguishable
//!    from one that did nothing useful.
//! 4. Applying is refused while a process holds the single-instance lock, because
//!    a repair that ran while the daemon was live could race it for the same
//!    files.
//! 5. After applying, the postcondition is checked **again**. A failed
//!    postcondition rolls back every action that recorded an undo step, and a
//!    rollback failure is reported rather than swallowed.
//!
//! ## What repair will not do
//!
//! - It never weakens authentication and never invents a credential. A missing
//!   credential is repaired by starting the daemon, not by this module.
//! - It never deletes user data. Anything under the data root — above all the
//!   database — is refused by [`is_protected`]. A data problem is repaired by a
//!   restore, which is its own confirmed flow in the storage module.
//! - It refuses a plan it cannot verify, rather than reporting success on an
//!   unverifiable outcome.
//! - It refuses a path outside the profile, at plan time and again at apply time.
//! - It never touches a *held* single-instance lock: a held lock means a live
//!   daemon, and the correct repair is to stop the daemon, not to delete evidence.
//!
//! ## Residual risk, stated rather than hidden
//!
//! A stale lock cannot be removed *while holding it*: on Windows a locked file
//! cannot be deleted, and on Unix deleting it would leave the lock meaningless.
//! The check is therefore "no process currently holds it", then removal, which
//! leaves a narrow window in which a daemon could start. The postcondition and
//! the rollback exist precisely because that window cannot be closed by a file
//! operation, and the daemon's own startup lock acquisition is what ultimately
//! decides ownership.

use std::path::{Path, PathBuf};

use super::collector::database_file;
use super::{CheckReport, Finding, Severity};

use crate::lifecycle::appears_unheld;
use crate::paths::ProfilePaths;

/// The finding code reported when a managed directory is accessible beyond its
/// owner.
const CODE_UNSAFE_PERMISSIONS: &str = "jarvis.unsafe_permissions";

/// Why a repair cannot be offered or cannot run.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RepairError {
    /// The finding has no safe automated repair.
    #[error("no safe automated repair exists for {code}")]
    NotRepairable {
        /// The finding's stable code or bounded fact.
        code: String,
    },
    /// The plan was not explicitly confirmed.
    #[error("a repair requires explicit confirmation")]
    NotConfirmed,
    /// The plan cannot be verified, so its outcome could not be trusted.
    #[error("the plan has no verifiable postcondition")]
    Unverifiable,
    /// A path in the plan is outside the profile.
    #[error("a repair path is outside the profile")]
    PathOutsideProfile {
        /// The rejected path.
        path: PathBuf,
    },
    /// A path in the plan holds user data and must not be removed by a repair.
    #[error("a repair must not remove user data")]
    ProtectedPath {
        /// The rejected path.
        path: PathBuf,
    },
    /// The single-instance lock is held, so a daemon is running.
    #[error("the daemon is running; a repair would race with it")]
    DaemonRunning,
    /// The postcondition did not hold after applying.
    #[error("the repair did not reach its postcondition")]
    PostconditionFailed {
        /// What the postcondition observed.
        detail: String,
    },
    /// The postcondition already held, so the repair would have done nothing.
    #[error("already repaired; nothing to do")]
    AlreadySatisfied,
    /// An action failed.
    #[error("a repair action failed")]
    ActionFailed {
        /// The action that failed.
        action: RepairKind,
    },
    /// The postcondition could not be evaluated.
    #[error("the postcondition could not be checked")]
    PostconditionUnavailable,
    /// Rolling back an action failed.
    #[error("rollback failed for {action:?}")]
    RollbackFailed {
        /// The action that could not be rolled back.
        action: RepairKind,
    },
}

impl RepairError {
    /// Returns the stable, namespaced code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotRepairable { .. } => "jarvis.repair_not_repairable",
            Self::NotConfirmed => "jarvis.repair_not_confirmed",
            Self::Unverifiable => "jarvis.repair_unverifiable",
            Self::PathOutsideProfile { .. } => "jarvis.repair_path_outside_profile",
            Self::ProtectedPath { .. } => "jarvis.repair_protected_path",
            Self::DaemonRunning => "jarvis.repair_daemon_running",
            Self::PostconditionFailed { .. } => "jarvis.repair_postcondition_failed",
            Self::AlreadySatisfied => "jarvis.repair_already_satisfied",
            Self::ActionFailed { .. } => "jarvis.repair_action_failed",
            Self::PostconditionUnavailable => "jarvis.repair_postcondition_unavailable",
            Self::RollbackFailed { .. } => "jarvis.repair_rollback_failed",
        }
    }

    /// Returns whether retrying unchanged is safe.
    ///
    /// A held lock may clear on its own; a refusal, a protected path, or a failed
    /// postcondition will not be fixed by retrying the same plan.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::DaemonRunning | Self::ActionFailed { .. })
    }
}

/// The kinds of repair JARVIS can perform in this milestone.
///
/// Each variant is a bounded, idempotent, user-data-preserving change. The set is
/// deliberately small: a repair that needs a decision is not automated, and a
/// finding with no variant here is reported as [`RepairError::NotRepairable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepairKind {
    /// Create the managed profile directories that are missing.
    CreateProfileDirectories,
    /// Restrict a directory's mode to the owner on a platform that can do so.
    RestrictDirectoryPermissions,
    /// Remove a single-instance lock file that no process holds.
    RemoveStaleLock,
    /// Remove a discovery file that survived a daemon which did not drain.
    RemoveStaleDiscovery,
}

impl RepairKind {
    /// Returns the token used in a rendered plan.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::CreateProfileDirectories => "create_profile_directories",
            Self::RestrictDirectoryPermissions => "restrict_directory_permissions",
            Self::RemoveStaleLock => "remove_stale_lock",
            Self::RemoveStaleDiscovery => "remove_stale_discovery",
        }
    }
}

/// One bounded change a plan would make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairAction {
    /// What kind of change this is.
    pub kind: RepairKind,
    /// The path the change touches. Always inside the profile.
    pub path: PathBuf,
    /// A one-line description of the effect.
    pub summary: String,
    /// Whether the action can be rolled back.
    ///
    /// Recorded rather than assumed, so the preview can name the action an
    /// operator should scrutinize.
    pub reversible: bool,
}

/// A reproducible diagnosis of a problem, separate from its repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairDiagnosis {
    /// The finding's stable check name.
    pub check: &'static str,
    /// The finding's stable code or bounded fact.
    pub code: String,
    /// A plain description of what is wrong.
    pub summary: String,
}

/// A complete, unexecuted repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairPlan {
    diagnosis: RepairDiagnosis,
    actions: Vec<RepairAction>,
    summary: String,
    destructive: bool,
}

impl RepairPlan {
    /// Returns the diagnosis this plan addresses.
    #[must_use]
    pub fn diagnosis(&self) -> &RepairDiagnosis {
        &self.diagnosis
    }

    /// Returns the actions in execution order.
    #[must_use]
    pub fn actions(&self) -> &[RepairAction] {
        &self.actions
    }

    /// Returns a one-line summary of the whole plan.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns whether the plan removes anything.
    ///
    /// A destructive plan must be visibly different from one that only adds, so
    /// the preview states it and the confirmation is unambiguous.
    #[must_use]
    pub const fn is_destructive(&self) -> bool {
        self.destructive
    }

    /// Returns whether every action in the plan can be rolled back.
    #[must_use]
    pub fn is_fully_reversible(&self) -> bool {
        self.actions.iter().all(|action| action.reversible)
    }

    /// Renders the plan for an operator to review before confirming.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let _ = writeln!(out, "diagnosis: {}", self.diagnosis.summary);
        let _ = writeln!(
            out,
            "check:     {} ({})",
            self.diagnosis.check, self.diagnosis.code
        );
        let _ = writeln!(out, "effect:    {}", self.summary);
        let _ = writeln!(
            out,
            "risk:      {}",
            if self.destructive {
                "REMOVES a file; reversible only by restoring it"
            } else if self.is_fully_reversible() {
                "additive; every action is reversible"
            } else {
                "contains an action that cannot be rolled back"
            }
        );
        out.push_str("actions:\n");
        for action in &self.actions {
            let _ = writeln!(
                out,
                "  {:<30} {:<5} {}",
                action.kind.token(),
                if action.reversible { "undo" } else { "final" },
                action.path.display()
            );
            let _ = writeln!(out, "      {}", action.summary);
        }
        out
    }
}

/// Builds the repair plan for `finding`, or reports why none is safe.
///
/// # Errors
///
/// Returns [`RepairError::NotRepairable`] for a finding with no automated repair,
/// which is the answer for most findings: an unsupported schema, a corrupt
/// database, a missing credential, and a daemon that is not running each require
/// a decision or a restore, not a file operation. Returns
/// [`RepairError::DaemonRunning`] when a stale-state repair would touch state a
/// live daemon owns.
pub fn plan_for(paths: &ProfilePaths, finding: &Finding) -> Result<RepairPlan, RepairError> {
    match finding.check {
        "profile directories" => plan_directories(paths, finding),
        // The daemon-state check is the authoritative one, so the stale-state
        // repair hangs off it. Keying the repair on the *reachability* finding
        // instead would miss every unclearly-terminated daemon whose discovery
        // file is still on disk.
        "daemon state" => plan_stale_discovery(paths, finding),
        _ => Err(RepairError::NotRepairable {
            code: finding.message.clone(),
        }),
    }
}

/// Builds a plan for the profile-directory check.
///
/// Two distinct problems hide behind this one check and they need different
/// repairs: a **missing** directory is created, while an **unsafe mode** is
/// restricted. Creating a directory that already exists cannot fix its
/// permissions, so the two must not be collapsed into a single plan.
fn plan_directories(paths: &ProfilePaths, finding: &Finding) -> Result<RepairPlan, RepairError> {
    // A mode problem is identified by the finding's code, because that is the
    // only part of the collector's error mapping that survives into a Finding.
    if finding.message == CODE_UNSAFE_PERMISSIONS {
        let existing: Vec<PathBuf> = paths
            .all_dirs()
            .iter()
            .filter(|directory| directory.is_dir())
            .map(|directory| (*directory).to_path_buf())
            .collect();
        for directory in &existing {
            guard_inside_profile(paths, directory)?;
        }
        let actions: Vec<RepairAction> = existing
            .into_iter()
            .map(|directory| RepairAction {
                kind: RepairKind::RestrictDirectoryPermissions,
                path: directory,
                // On Windows the ACL is the control and cannot be tightened
                // without `unsafe` FFI, so the summary says so rather than
                // promising a change the action will not make.
                summary: "restrict the mode to the owner (fails where the ACL is the control)"
                    .to_owned(),
                reversible: false,
            })
            .collect();
        if actions.is_empty() {
            return Err(RepairError::NotRepairable {
                code: finding.message.clone(),
            });
        }
        return Ok(RepairPlan {
            diagnosis: RepairDiagnosis {
                check: finding.check,
                code: finding.message.clone(),
                summary: "a managed directory is accessible beyond its owner".to_owned(),
            },
            summary: format!("restrict {} directory mode(s) to the owner", actions.len()),
            actions,
            destructive: false,
        });
    }

    let missing = paths.missing_directories();
    if missing.is_empty() {
        // The check failed for a reason this plan cannot express, so refuse
        // rather than offer a plan that would not change the outcome.
        return Err(RepairError::NotRepairable {
            code: finding.message.clone(),
        });
    }

    // Every path must be inside the profile before it becomes an action. The plan
    // is the last point at which this is cheap to prove.
    for directory in &missing {
        guard_inside_profile(paths, directory)?;
    }

    let actions: Vec<RepairAction> = missing
        .iter()
        .map(|directory| RepairAction {
            kind: RepairKind::CreateProfileDirectories,
            path: directory.clone(),
            summary: "create with owner-only permissions where the platform supports it".to_owned(),
            reversible: true,
        })
        .collect();

    Ok(RepairPlan {
        diagnosis: RepairDiagnosis {
            check: finding.check,
            code: finding.message.clone(),
            summary: format!("{} managed directory(ies) are missing", actions.len()),
        },
        summary: format!("create {} missing profile directory(ies)", actions.len()),
        actions,
        destructive: false,
    })
}

/// Removes a stale discovery file that survived an unclearly-terminated daemon.
///
/// The discovery file — not the lock file — is what is removed, and that\u{2019}s the
/// point. A clean drain releases the lock *and* unpublishes discovery, so a lock
/// file with no holder is the normal resting state and removing it would be a
/// pointless repair. A discovery file that survived a *free* lock means the
/// daemon did not drain, and the stale file is also what makes `jarvis status`
/// claim a dead daemon is running.
fn plan_stale_discovery(
    paths: &ProfilePaths,
    finding: &Finding,
) -> Result<RepairPlan, RepairError> {
    let discovery = paths.runtime_dir().join("discovery.json");
    guard_inside_profile(paths, &discovery)?;

    if !discovery.exists() {
        // Nothing to remove, so no plan could change anything. This is also the
        // healthy case, which is why an idle daemon must never reach here.
        return Err(RepairError::NotRepairable {
            code: finding.message.clone(),
        });
    }
    // Removing a discovery file while a daemon holds the lock would break a live
    // daemon's published address for no reason.
    let lock = paths.runtime_dir().join("jarvis.lock");
    if lock.exists() && !appears_unheld(&lock) {
        return Err(RepairError::DaemonRunning);
    }

    Ok(RepairPlan {
        diagnosis: RepairDiagnosis {
            check: finding.check,
            code: finding.message.clone(),
            summary: "a discovery file survived a daemon that did not drain".to_owned(),
        },
        summary: "remove the stale discovery file".to_owned(),
        actions: vec![RepairAction {
            kind: RepairKind::RemoveStaleDiscovery,
            path: discovery,
            summary: "remove the discovery file; a held instance lock is never touched".to_owned(),
            reversible: true,
        }],
        destructive: true,
    })
}

/// Rejects a path that is not inside the profile.
fn guard_inside_profile(paths: &ProfilePaths, path: &Path) -> Result<(), RepairError> {
    // Containment is checked against every managed root, not only the data root,
    // so a runtime or log path is covered too.
    let inside = paths.all_dirs().iter().any(|root| path.starts_with(root));
    if inside {
        Ok(())
    } else {
        Err(RepairError::PathOutsideProfile {
            path: path.to_path_buf(),
        })
    }
}

/// What applying a plan did, for the operator's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairOutcome {
    /// How many actions ran.
    pub applied: usize,
    /// Whether the postcondition was verified after applying.
    pub verified: bool,
    /// A plain description of the verified state.
    pub detail: String,
}

/// Applies `plan` and verifies its postcondition.
///
/// This is synchronous because every action it performs is a filesystem
/// operation; it becomes asynchronous only if a future action needs the database.
///
/// `confirmed` must be `true`. There is no default, because a repair that runs
/// without a decision is exactly the failure mode this design exists to prevent.
///
/// # Errors
///
/// - [`RepairError::NotConfirmed`] when `confirmed` is false.
/// - [`RepairError::Unverifiable`] when the plan has no postcondition, in which
///   case nothing is changed.
/// - [`RepairError::AlreadySatisfied`] when the postcondition already holds.
/// - [`RepairError::DaemonRunning`] when the lock a plan would remove is held.
/// - [`RepairError::ProtectedPath`] when an action would remove user data.
/// - [`RepairError::PostconditionFailed`] when the state after applying does not
///   satisfy the postcondition; every reversible action is rolled back first.
/// - [`RepairError::RollbackFailed`] when a rollback itself fails, which is
///   reported rather than hidden.
pub fn apply(
    paths: &ProfilePaths,
    plan: &RepairPlan,
    confirmed: bool,
) -> Result<RepairOutcome, RepairError> {
    if !confirmed {
        return Err(RepairError::NotConfirmed);
    }

    let postcondition = postcondition_for(paths, plan)?;

    // A plan is data and could have been constructed elsewhere, so its actions are
    // validated *before* any state reasoning. A plan that escapes the profile or
    // would remove user data is refused regardless of what the profile currently
    // looks like, because those are properties of the plan, not of the state.
    for action in &plan.actions {
        guard_inside_profile(paths, &action.path)?;
        if is_protected(paths, &action.path) {
            return Err(RepairError::ProtectedPath {
                path: action.path.clone(),
            });
        }
    }

    guard_daemon_not_holding(paths, plan)?;

    // Re-check the pre-state. Between building the plan and confirming it, the
    // problem may have resolved. Recording this is what stops a plan from
    // "succeeding" without having changed anything.
    if postcondition.check()? {
        return Err(RepairError::AlreadySatisfied);
    }

    let mut journal = Journal::new();
    for action in &plan.actions {
        let undo = perform(action).map_err(|()| RepairError::ActionFailed {
            action: action.kind,
        })?;
        journal.record(action, undo);
    }

    match postcondition.check() {
        Ok(true) => Ok(RepairOutcome {
            applied: plan.actions.len(),
            verified: true,
            detail: postcondition.describe(),
        }),
        Ok(false) => {
            journal.rollback()?;
            Err(RepairError::PostconditionFailed {
                detail: postcondition.describe(),
            })
        }
        Err(_) => Err(RepairError::PostconditionFailed {
            detail: "the postcondition could not be checked".to_owned(),
        }),
    }
}

/// Refuses to remove runtime state while a process holds the instance lock.
fn guard_daemon_not_holding(paths: &ProfilePaths, plan: &RepairPlan) -> Result<(), RepairError> {
    let touches_runtime_state = plan.actions.iter().any(|action| {
        matches!(
            action.kind,
            RepairKind::RemoveStaleLock | RepairKind::RemoveStaleDiscovery
        )
    });
    if !touches_runtime_state {
        return Ok(());
    }
    if appears_unheld(&paths.runtime_dir().join("jarvis.lock")) {
        Ok(())
    } else {
        Err(RepairError::DaemonRunning)
    }
}

/// Something that must become true for a repair to be considered complete.
///
/// It is checked before applying (so a no-op repair is detected) and after (so a
/// useless repair is detected). It is a live closure rather than a stored boolean
/// because the answer changes as the actions run.
struct Postcondition {
    description: String,
    check: Box<dyn Fn() -> Result<bool, ()>>,
}

impl Postcondition {
    fn check(&self) -> Result<bool, RepairError> {
        (self.check)().map_err(|()| RepairError::PostconditionUnavailable)
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

/// Builds the postcondition for a plan.
///
/// # Errors
///
/// Returns [`RepairError::Unverifiable`] for a plan this module cannot verify.
fn postcondition_for(
    paths: &ProfilePaths,
    plan: &RepairPlan,
) -> Result<Postcondition, RepairError> {
    let kinds: Vec<RepairKind> = plan.actions.iter().map(|action| action.kind).collect();

    if kinds.contains(&RepairKind::RemoveStaleLock) {
        let lock = paths.runtime_dir().join("jarvis.lock");
        return Ok(Postcondition {
            description: "the lock file no longer exists".to_owned(),
            check: Box::new(move || Ok(!lock.exists())),
        });
    }

    if kinds.contains(&RepairKind::RemoveStaleDiscovery) {
        let discovery = paths.runtime_dir().join("discovery.json");
        return Ok(Postcondition {
            description: "the stale discovery file no longer exists".to_owned(),
            check: Box::new(move || Ok(!discovery.exists())),
        });
    }

    let directory_work = kinds.contains(&RepairKind::CreateProfileDirectories)
        || kinds.contains(&RepairKind::RestrictDirectoryPermissions);
    if directory_work {
        let profile = paths.clone();
        return Ok(Postcondition {
            // One postcondition covers both directory problems: every managed
            // directory exists *and* passes the owner-only check. It is evaluated
            // by `verify_directories`, which never creates anything, so the check
            // cannot be what makes itself pass.
            description: "every managed directory exists and is owner-only".to_owned(),
            check: Box::new(move || {
                Ok(
                    profile.missing_directories().is_empty()
                        && profile.verify_directories().is_ok(),
                )
            }),
        });
    }

    Err(RepairError::Unverifiable)
}

/// The undo record for one applied action.
enum Undo {
    /// The directory was created by this action, so it can be removed again.
    RemoveCreatedDirectory(PathBuf),
    /// A file was removed; its bytes were captured so it can be restored.
    RestoreFile(PathBuf, Vec<u8>),
    /// Nothing is needed to reverse the action.
    None,
}

/// Records what was done so a failed postcondition can be rolled back.
struct Journal {
    entries: Vec<(RepairKind, Undo)>,
}

impl Journal {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn record(&mut self, action: &RepairAction, undo: Undo) {
        self.entries.push((action.kind, undo));
    }

    /// Reverses every recorded action, newest first.
    ///
    /// A rollback that fails is reported, not swallowed: an operator must know
    /// that the profile is in a partially repaired state.
    fn rollback(&mut self) -> Result<(), RepairError> {
        while let Some((kind, undo)) = self.entries.pop() {
            match undo {
                Undo::RemoveCreatedDirectory(path) => {
                    // Only remove it when it is still empty. A directory that
                    // gained content after creation is no longer this action's to
                    // delete.
                    let empty =
                        std::fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_none());
                    if empty {
                        std::fs::remove_dir(&path)
                            .map_err(|_| RepairError::RollbackFailed { action: kind })?;
                    }
                }
                Undo::RestoreFile(path, contents) => {
                    std::fs::write(&path, &contents)
                        .map_err(|_| RepairError::RollbackFailed { action: kind })?;
                }
                Undo::None => {}
            }
        }
        Ok(())
    }
}

/// Performs one action and returns its undo record.
fn perform(action: &RepairAction) -> Result<Undo, ()> {
    match action.kind {
        RepairKind::CreateProfileDirectories => {
            if action.path.is_dir() {
                // Already true, so this action did not create it and must not
                // delete it on rollback.
                return Ok(Undo::None);
            }
            create_directory(&action.path)?;
            Ok(Undo::RemoveCreatedDirectory(action.path.clone()))
        }
        RepairKind::RestrictDirectoryPermissions => {
            restrict_directory(&action.path)?;
            // A mode change is not reversed: the old mode is the insecure state
            // being corrected, so restoring it would undo the repair.
            Ok(Undo::None)
        }
        RepairKind::RemoveStaleLock | RepairKind::RemoveStaleDiscovery => {
            let contents = std::fs::read(&action.path).map_err(|_| ())?;
            std::fs::remove_file(&action.path).map_err(|_| ())?;
            Ok(Undo::RestoreFile(action.path.clone(), contents))
        }
    }
}

/// Creates one managed directory using the profile's owner-only rules.
fn create_directory(path: &Path) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(|_| ())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path).map_err(|_| ())
    }
}

/// Restricts an existing directory's mode to the owner.
fn restrict_directory(path: &Path) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|_| ())
    }
    #[cfg(not(unix))]
    {
        // Windows permissions come from the Known Folder ACL, which cannot be
        // tightened without `unsafe` FFI. Reporting success here would claim a
        // control this process did not apply, so the action fails instead. It is
        // unreachable in practice because the collector cannot report an unsafe
        // mode on Windows either.
        let _ = path;
        Err(())
    }
}

/// Builds every repair plan that can be offered for `report`.
///
/// A finding without a safe repair is absent from the result rather than present
/// with an empty plan, so a caller cannot mistake "no repair" for "a repair that
/// does nothing".
#[must_use]
pub fn plans_for(report: &CheckReport, paths: &ProfilePaths) -> Vec<RepairPlan> {
    report
        .findings()
        .iter()
        .filter(|finding| finding.severity != Severity::Ok)
        .filter_map(|finding| plan_for(paths, finding).ok())
        .collect()
}

/// Reports the findings that have no safe automated repair, with the reason.
///
/// This is the honest half of the answer: an operator needs to know which
/// problems repair will not touch.
#[must_use]
pub fn unrepairable(report: &CheckReport, paths: &ProfilePaths) -> Vec<(String, RepairError)> {
    report
        .findings()
        .iter()
        .filter(|finding| finding.severity != Severity::Ok)
        .filter_map(|finding| {
            plan_for(paths, finding)
                .err()
                .map(|error| (finding.check.to_owned(), error))
        })
        .collect()
}

/// Returns the database path a repair must never remove.
///
/// Named as a function so the invariant "repair does not delete user data" has
/// one testable definition rather than only a comment.
#[must_use]
pub fn protected_database_path(paths: &ProfilePaths) -> PathBuf {
    database_file(paths)
}

/// Returns whether a path holds user data that repair must not remove.
///
/// The **data root itself is not protected**, because it is a managed directory
/// that a repair legitimately creates when it is missing. What the data root
/// *holds* is user data, so anything strictly inside it is protected — including
/// the database. Getting this boundary wrong in either direction is a bug the
/// tests pin: too broad and repair cannot create the data directory at all; too
/// narrow and it could delete the database.
#[must_use]
pub fn is_protected(paths: &ProfilePaths, path: &Path) -> bool {
    path != paths.data_dir() && path.starts_with(paths.data_dir())
}

#[cfg(test)]
mod tests;
