//! Tests for previewed repair plans, postconditions, and rollback.
//!
//! The properties that matter are negative ones — a repair must refuse to run,
//! must refuse to remove user data, and must not report success without a change
//! — so most of these tests assert a refusal rather than an effect.

use std::path::PathBuf;

use super::*;
use crate::diagnostics::CheckReport;
use crate::paths::ProfilePaths;

fn profile(tag: &str) -> ProfilePaths {
    let root = std::env::temp_dir().join(format!("jarvis-fnd014-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    ProfilePaths::portable(root)
}

fn missing_directories_finding() -> Finding {
    Finding::error(
        "profile directories",
        "jarvis.directory_create",
        "check ownership",
    )
}

fn unsafe_permissions_finding() -> Finding {
    Finding::error("profile directories", "jarvis.unsafe_permissions", "check")
}

fn daemon_finding() -> Finding {
    Finding::warning("daemon", "jarvis.daemon_not_running", "start it")
}

/// The authoritative stale-daemon finding.
///
/// The repair is keyed on this check rather than on daemon reachability, because
/// the discovery file can outlive the daemon and would then report a stale state as
/// a running daemon.
fn stale_state_finding() -> Finding {
    Finding::warning("daemon state", "jarvis.stale_discovery", "run repair")
}

/// A finding with no automated repair.
fn schema_finding() -> Finding {
    Finding::error("schema", "jarvis.db_schema_too_new", "do not downgrade")
}

#[test]
fn a_plan_creates_exactly_the_missing_directories() {
    let paths = profile("plan-missing");
    let missing = paths.missing_directories();
    assert_eq!(
        missing.len(),
        5,
        "a fresh profile has no managed directories"
    );

    let plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    assert_eq!(plan.actions().len(), missing.len());
    // The actions must name the same paths the profile reported, in the same
    // order, or the preview would describe a different repair than the one that
    // runs.
    let planned: Vec<PathBuf> = plan.actions().iter().map(|a| a.path.clone()).collect();
    assert_eq!(planned, missing);
    assert!(plan.actions().iter().all(|action| {
        action.kind == RepairKind::CreateProfileDirectories && action.reversible
    }));
    assert!(!plan.is_destructive());
    assert!(plan.is_fully_reversible());
}

#[test]
fn building_a_plan_changes_nothing() {
    let paths = profile("plan-pure");
    let before = paths.missing_directories();
    let _plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    // Planning is a read-only operation; if it created directories, the preview
    // would be the repair.
    assert_eq!(paths.missing_directories(), before);
    assert!(!paths.config_dir().exists());
}

#[test]
fn an_unconfirmed_plan_is_refused() {
    let paths = profile("unconfirmed");
    let plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    let error = apply(&paths, &plan, false).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_not_confirmed");
    assert!(!error.retryable());
    // Nothing ran.
    assert_eq!(paths.missing_directories().len(), 5);
}

#[test]
fn a_confirmed_plan_reaches_its_postcondition() {
    let paths = profile("apply");
    let plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    let outcome = apply(&paths, &plan, true).expect("applies");
    assert!(outcome.verified);
    assert_eq!(outcome.applied, 5);
    assert!(paths.missing_directories().is_empty());
    // The postcondition is the profile's own check, evaluated after the fact.
    assert!(paths.verify_directories().is_ok());
}

#[test]
fn a_second_apply_is_refused_as_already_satisfied() {
    // This is the property that stops a repair from reporting success without
    // having changed anything. The plan cannot be reused once it has worked.
    let paths = profile("idempotent");
    let plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    apply(&paths, &plan, true).expect("first apply");
    let error = apply(&paths, &plan, true).expect_err("second apply must refuse");
    assert_eq!(error.code(), "jarvis.repair_already_satisfied");
}

#[test]
fn a_plan_is_refused_when_the_directories_already_exist() {
    // A stale plan must not be offered. Building it after the problem is gone
    // must fail rather than produce a no-op.
    let paths = profile("stale-plan");
    paths.ensure_directories().expect("create");
    let error = plan_for(&paths, &missing_directories_finding()).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_not_repairable");
}

#[test]
fn findings_without_a_safe_repair_are_reported_as_unrepairable() {
    let paths = profile("unrepairable");
    for finding in [
        schema_finding(),
        Finding::error("database", "jarvis.db_integrity", "restore a backup"),
        Finding::error("configuration", "jarvis.config_parse", "fix it"),
        Finding::warning(
            "credential",
            "jarvis.credential_missing",
            "start the daemon",
        ),
    ] {
        let error = plan_for(&paths, &finding).expect_err("must not offer a repair");
        assert_eq!(error.code(), "jarvis.repair_not_repairable");
        // The refusal names the thing it refused, so the operator can act.
        assert!(error.to_string().contains(&finding.message));
    }
}

#[test]
fn a_surviving_discovery_file_is_repairable_and_removes_only_that() {
    let paths = profile("stale-discovery");
    paths.ensure_directories().expect("create");
    let lock = paths.runtime_dir().join("jarvis.lock");
    let discovery = paths.runtime_dir().join("discovery.json");
    std::fs::write(&lock, b"").expect("write lock");
    std::fs::write(&discovery, b"{}").expect("write discovery");

    let plan = plan_for(&paths, &stale_state_finding()).expect("plan");
    assert!(
        plan.is_destructive(),
        "removing a file is shown as destructive"
    );
    assert_eq!(plan.actions().len(), 1);
    assert_eq!(plan.actions()[0].kind, RepairKind::RemoveStaleDiscovery);
    assert!(plan.render().contains("REMOVES"));

    let outcome = apply(&paths, &plan, true).expect("applies");
    assert!(outcome.verified);
    assert!(!discovery.exists());
    // The lock file is the daemon's normal resting state and is left alone.
    assert!(lock.exists(), "the benign lock file must not be removed");
}

#[test]
fn the_stale_state_repair_is_not_offered_for_daemon_reachability() {
    // Keying the repair on the wrong finding is a bug that hides itself: the
    // discovery file survives an unclean kill, so a "daemon not running" finding
    // would never appear and the stale state would never be repairable.
    let paths = profile("wrong-finding");
    paths.ensure_directories().expect("create");
    std::fs::write(paths.runtime_dir().join("jarvis.lock"), b"").expect("write lock");
    std::fs::write(paths.runtime_dir().join("discovery.json"), b"{}").expect("write discovery");
    assert!(plan_for(&paths, &stale_state_finding()).is_ok());
    assert!(
        plan_for(&paths, &daemon_finding()).is_err(),
        "reachability must not be the trigger for a stale-state repair"
    );
}

#[test]
fn a_missing_discovery_file_is_not_repairable() {
    // Removing a file that is not there cannot change anything, so no plan is
    // offered. This is also the healthy idle case, which must never reach repair.
    let paths = profile("no-discovery");
    paths.ensure_directories().expect("create");
    let error = plan_for(&paths, &stale_state_finding()).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_not_repairable");
}

#[test]
fn a_stale_discovery_file_is_not_removed_while_the_lock_is_held() {
    // A live daemon owns its discovery file; removing it would break a running
    // daemon's published address for no reason.
    let paths = profile("live-daemon-discovery");
    paths.ensure_directories().expect("create");
    let lock = paths.runtime_dir().join("jarvis.lock");
    let guard = crate::lifecycle::InstanceGuard::acquire(&lock).expect("acquire");
    let discovery = paths.runtime_dir().join("discovery.json");
    std::fs::write(&discovery, b"{}").expect("write discovery");

    let error = plan_for(&paths, &stale_state_finding()).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_daemon_running");
    assert!(discovery.exists());
    drop(guard);
}

#[test]
fn the_database_is_protected_from_repair_removal() {
    // The invariant "repair never deletes user data" is checked on the one path
    // that holds it, and on the boundary around it.
    let paths = profile("protected");
    assert!(is_protected(&paths, &protected_database_path(&paths)));
    assert!(is_protected(&paths, &paths.data_dir().join("anything")));
    assert!(is_protected(
        &paths,
        &paths.database_dir().join("jarvis.sqlite")
    ));
    // The data root itself is a managed directory a repair must be able to
    // CREATE, so it cannot be protected; its contents are protected instead.
    assert!(
        !is_protected(&paths, paths.data_dir()),
        "the data root must be creatable, or a fresh profile could never be repaired"
    );
    // The lock is not user data, so it is not protected.
    assert!(!is_protected(
        &paths,
        &paths.runtime_dir().join("jarvis.lock")
    ));
    // Neither is a log.
    assert!(!is_protected(&paths, &paths.log_dir().join("jarvis.log")));
}

#[test]
fn a_plan_that_only_creates_directories_touches_nothing_protected() {
    // The interaction that broke a naive protection rule: the plan creates the
    // data root, so a rule that protected the whole data subtree would refuse
    // every fresh-profile repair.
    let paths = profile("create-not-protected");
    let plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    assert!(
        plan.actions()
            .iter()
            .any(|action| action.path == paths.data_dir()),
        "the plan must create the data root"
    );
    for action in plan.actions() {
        assert!(
            !is_protected(&paths, &action.path),
            "{} would be refused by the protection guard",
            action.path.display()
        );
    }
    apply(&paths, &plan, true).expect("a directory-creating plan must be allowed");
    assert!(paths.data_dir().is_dir());
}

#[test]
fn a_plan_action_outside_the_profile_is_refused_at_apply_time() {
    // A plan is data; it could have been built by other code. Applying one whose
    // action escapes the profile must fail rather than touch the escape.
    let paths = profile("escape");
    let outside = std::env::temp_dir().join("jarvis-fnd014-escape-outside");
    let plan = RepairPlan {
        diagnosis: RepairDiagnosis {
            check: "profile directories",
            code: "test".to_owned(),
            summary: "hand-built".to_owned(),
        },
        summary: "created by a test".to_owned(),
        actions: vec![RepairAction {
            kind: RepairKind::CreateProfileDirectories,
            path: outside.clone(),
            summary: "escape".to_owned(),
            reversible: true,
        }],
        destructive: false,
    };
    let error = apply(&paths, &plan, true).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_path_outside_profile");
    assert!(
        !outside.exists(),
        "nothing outside the profile may be created"
    );
}

#[test]
fn an_action_that_would_remove_user_data_is_refused() {
    let paths = profile("protect-apply");
    paths.ensure_directories().expect("create");
    let data_file = paths.data_dir().join("precious");
    std::fs::write(&data_file, b"user data").expect("write");

    // A hand-built plan that tries to delete under the data root. The
    // postcondition is satisfiable (the file does not exist afterwards), so the
    // protected-path guard is the only thing standing between the plan and the
    // file — which is exactly what is being tested.
    let plan = RepairPlan {
        diagnosis: RepairDiagnosis {
            check: "daemon",
            code: "test".to_owned(),
            summary: "attempts to delete user data".to_owned(),
        },
        summary: "must be refused".to_owned(),
        actions: vec![RepairAction {
            kind: RepairKind::RemoveStaleLock,
            path: data_file.clone(),
            summary: "delete user data".to_owned(),
            reversible: true,
        }],
        destructive: true,
    };
    let error = apply(&paths, &plan, true).expect_err("must refuse");
    assert_eq!(
        error.code(),
        "jarvis.repair_protected_path",
        "a plan property must be checked before any state reasoning"
    );
    assert!(data_file.exists(), "user data must survive");
}

#[test]
fn a_plan_with_no_postcondition_is_refused_without_changing_anything() {
    // A plan with no actions has no postcondition this module can evaluate, so it
    // must be refused rather than "succeed" at doing nothing.
    let paths = profile("unverifiable");
    let plan = RepairPlan {
        diagnosis: RepairDiagnosis {
            check: "mystery",
            code: "test".to_owned(),
            summary: "no postcondition".to_owned(),
        },
        summary: "unverifiable".to_owned(),
        actions: Vec::new(),
        destructive: false,
    };
    let error = apply(&paths, &plan, true).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_unverifiable");
    assert!(!paths.config_dir().exists());
    assert!(!paths.data_dir().exists());
}

#[test]
fn the_plan_render_names_every_action_and_the_risk() {
    let paths = profile("render");
    let plan = plan_for(&paths, &missing_directories_finding()).expect("plan");
    let text = plan.render();
    assert!(text.contains("diagnosis:"));
    assert!(text.contains("check:     profile directories"));
    assert!(text.contains("risk:      additive; every action is reversible"));
    for action in plan.actions() {
        assert!(
            text.contains(action.kind.token()),
            "{}",
            action.kind.token()
        );
        assert!(text.contains(&action.path.display().to_string()));
    }
}

#[test]
fn plans_for_offers_only_non_ok_findings_and_unrepairable_names_the_rest() {
    let paths = profile("report-drive");
    let mut report = CheckReport::new();
    report.push(Finding::ok("database", "fine"));
    report.push(missing_directories_finding());
    report.push(schema_finding());

    let plans = plans_for(&report, &paths);
    assert_eq!(plans.len(), 1, "only the directory problem is repairable");
    assert_eq!(plans[0].diagnosis().check, "profile directories");

    let refused = unrepairable(&report, &paths);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].0, "schema");
    // The healthy check appears in neither list.
    assert!(
        !plans
            .iter()
            .any(|plan| plan.diagnosis().check == "database")
    );
    assert!(!refused.iter().any(|(check, _)| check == "database"));
}

#[test]
fn an_unsafe_permission_finding_produces_a_permission_plan_not_a_create_plan() {
    // The two problems behind one check must not be collapsed: creating a
    // directory that exists cannot fix its mode. The directories must exist for
    // a permission plan to be possible.
    let paths = profile("permissions");
    paths.ensure_directories().expect("create");
    let plan = plan_for(&paths, &unsafe_permissions_finding()).expect("plan");
    assert!(
        plan.actions()
            .iter()
            .all(|action| action.kind == RepairKind::RestrictDirectoryPermissions),
        "{:?}",
        plan.actions()
    );
    assert!(plan.render().contains("restrict_directory_permissions"));
    // A mode change is not reversible, and the preview must say so.
    assert!(!plan.is_fully_reversible());
    assert!(plan.render().contains("cannot be rolled back"));
    assert!(
        plan.actions().iter().all(|action| !action.reversible),
        "the preview must not promise an undo it cannot perform"
    );
}

#[test]
fn an_unsafe_permission_finding_with_no_directory_present_is_unrepairable() {
    let paths = profile("permissions-absent");
    let error = plan_for(&paths, &unsafe_permissions_finding()).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.repair_not_repairable");
}

#[test]
fn every_repair_error_has_a_unique_namespaced_code() {
    let errors = [
        RepairError::NotRepairable {
            code: "x".to_owned(),
        },
        RepairError::NotConfirmed,
        RepairError::Unverifiable,
        RepairError::PathOutsideProfile { path: "a".into() },
        RepairError::ProtectedPath { path: "b".into() },
        RepairError::DaemonRunning,
        RepairError::PostconditionFailed {
            detail: "c".to_owned(),
        },
        RepairError::AlreadySatisfied,
        RepairError::ActionFailed {
            action: RepairKind::RemoveStaleLock,
        },
        RepairError::PostconditionUnavailable,
        RepairError::RollbackFailed {
            action: RepairKind::RemoveStaleLock,
        },
    ];
    let mut codes: Vec<&str> = errors.iter().map(RepairError::code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), errors.len(), "codes must be unique");
    for code in codes {
        assert!(code.starts_with("jarvis.repair_"), "{code}");
    }
    // A held lock may clear on its own; a refusal will not.
    assert!(RepairError::DaemonRunning.retryable());
    assert!(!RepairError::ProtectedPath { path: "b".into() }.retryable());
    assert!(!RepairError::AlreadySatisfied.retryable());
}

#[test]
fn repair_kind_tokens_are_unique() {
    let kinds = [
        RepairKind::CreateProfileDirectories,
        RepairKind::RestrictDirectoryPermissions,
        RepairKind::RemoveStaleLock,
    ];
    let mut tokens: Vec<&str> = kinds.iter().map(|kind| kind.token()).collect();
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(tokens.len(), kinds.len());
}

#[cfg(unix)]
#[test]
fn a_permissive_directory_is_reported_and_then_repaired() {
    // Unix-only because Windows cannot report an unsafe mode without an ACL
    // query. Typechecked on Windows, executed in the native CI lane.
    use std::os::unix::fs::PermissionsExt as _;

    let paths = profile("unix-mode");
    paths.ensure_directories().expect("create");
    let relaxed = paths.config_dir();
    std::fs::set_permissions(relaxed, std::fs::Permissions::from_mode(0o755))
        .expect("relax the mode");
    assert!(
        paths.verify_directories().is_err(),
        "the pre-state must be detected as unsafe, or the repair would be unverifiable"
    );

    let plan = plan_for(&paths, &unsafe_permissions_finding()).expect("plan");
    let outcome = apply(&paths, &plan, true).expect("applies");
    assert!(outcome.verified);
    assert!(paths.verify_directories().is_ok());
    let mode = std::fs::metadata(relaxed)
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "the mode must be owner-only");
}
