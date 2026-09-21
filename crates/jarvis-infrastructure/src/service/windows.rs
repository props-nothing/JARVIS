//! The Windows per-user scheduled-task controller.
//!
//! JARVIS does **not** use the Service Control Manager on Windows. A per-user
//! task created with `schtasks` runs as the current user with an interactive
//! token at the lowest run level, and it never stores a password. An elevated
//! service or `LocalSystem` task would place JARVIS outside the user security
//! model and is deliberately not supported here.
//!
//! Verified against the official `schtasks /create` reference: `/sc ONLOGON`
//! (the task runs when the user logs on), `/rl LIMITED` (the lowest run level,
//! and the default), and `/it` (run only while the run-as user is logged on).
//! `/ru` and `/rp` are not used, so no credential is ever recorded in the task.

use std::path::PathBuf;

use super::{
    ServiceBackend, ServiceController, ServiceError, ServicePlan, ServiceSpec, ServiceState,
};

/// Controls a per-user Windows scheduled task.
#[derive(Debug, Clone)]
pub struct WindowsTaskController {
    /// The `schtasks` executable name.
    pub schtasks: String,
    /// The task folder/name prefix.
    pub task_prefix: String,
}

impl WindowsTaskController {
    /// Creates a controller using the standard task name.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schtasks: "schtasks".to_owned(),
            task_prefix: "JARVIS".to_owned(),
        }
    }

    /// Creates a controller with an explicit tool name and prefix.
    #[must_use]
    pub fn with_paths(schtasks: impl Into<String>, task_prefix: impl Into<String>) -> Self {
        Self {
            schtasks: schtasks.into(),
            task_prefix: task_prefix.into(),
        }
    }

    /// Returns the fully qualified task name for `spec`.
    #[must_use]
    pub fn task_name(&self, spec: &ServiceSpec) -> String {
        format!("{}\\{}", self.task_prefix, spec.name)
    }

    /// Returns the program and arguments as one `schtasks` `/tr` value.
    ///
    /// `/tr` takes a single command line, so the executable is quoted and each
    /// argument is appended. Arguments were validated to contain no control
    /// character, so no quoting can be escaped.
    fn task_run(spec: &ServiceSpec) -> String {
        let mut command = format!("\"{}\"", spec.executable.display());
        for argument in &spec.arguments {
            command.push(' ');
            command.push_str(argument);
        }
        command
    }
}

impl Default for WindowsTaskController {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceController for WindowsTaskController {
    fn backend(&self) -> ServiceBackend {
        ServiceBackend::WindowsScheduledTask
    }

    fn status(&self, spec: &ServiceSpec) -> Result<ServiceState, ServiceError> {
        // `/query` is read-only. A missing task reports a non-zero exit, which is
        // the "not installed" answer rather than an error.
        let args = vec!["/query".to_owned(), "/tn".to_owned(), self.task_name(spec)];
        match super::exec::run_program_blocking(&self.schtasks, &args) {
            Ok(true) => Ok(ServiceState::Installed),
            Ok(false) => Ok(ServiceState::NotInstalled),
            // The tool itself is missing: report a facility problem rather than
            // claiming the task is absent.
            Err(error) => Err(error),
        }
    }

    fn plan_install(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        let task_name = self.task_name(spec);
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.schtasks),
            args: vec![
                "/create".to_owned(),
                "/tn".to_owned(),
                task_name.clone(),
                "/tr".to_owned(),
                Self::task_run(spec),
                // Run at logon, in the current user's session.
                "/sc".to_owned(),
                "ONLOGON".to_owned(),
                // Lowest privileges. This is also the documented default, and it
                // is stated explicitly so a preview cannot be misread.
                "/rl".to_owned(),
                "LIMITED".to_owned(),
                // Interactive token only: no stored password, no background
                // session.
                "/it".to_owned(),
                // Replace an existing definition instead of failing on a repeat.
                "/f".to_owned(),
            ],
            // A task is defined entirely by its command line.
            definition: None,
            definition_path: None,
            summary: format!(
                "Register the per-user task {task_name} at logon, run level LIMITED, \
                 interactive token only, no stored password.",
            ),
        })
    }

    fn plan_uninstall(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.schtasks),
            args: vec![
                "/delete".to_owned(),
                "/tn".to_owned(),
                self.task_name(spec),
                "/f".to_owned(),
            ],
            definition: None,
            definition_path: None,
            summary: format!("Delete the scheduled task {}.", self.task_name(spec)),
        })
    }

    fn plan_start(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.schtasks),
            args: vec!["/run".to_owned(), "/tn".to_owned(), self.task_name(spec)],
            definition: None,
            definition_path: None,
            summary: format!("Run the scheduled task {}.", self.task_name(spec)),
        })
    }

    fn plan_stop(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.schtasks),
            args: vec!["/end".to_owned(), "/tn".to_owned(), self.task_name(spec)],
            definition: None,
            definition_path: None,
            summary: format!("End the running task {}.", self.task_name(spec)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsTaskController;
    use crate::service::{ServiceBackend, ServiceController, ServiceSpec};

    fn spec() -> ServiceSpec {
        ServiceSpec::new(
            "jarvisd",
            crate::service::tests_support::host_absolute("/Program Files/JARVIS/jarvisd.exe"),
            "JARVIS daemon",
        )
        .expect("valid spec")
    }

    fn controller() -> WindowsTaskController {
        WindowsTaskController::with_paths("schtasks", "JARVIS")
    }

    #[test]
    fn the_backend_is_a_windows_scheduled_task() {
        assert_eq!(controller().backend(), ServiceBackend::WindowsScheduledTask);
        assert_eq!(
            ServiceBackend::WindowsScheduledTask.name(),
            "windows-scheduled-task",
        );
    }

    #[test]
    fn install_uses_logon_lowest_run_level_and_no_stored_password() {
        let plan = controller().plan_install(&spec()).expect("plan");
        let args = &plan.args;

        assert_eq!(args[0], "/create");
        assert_eq!(plan.args[1], "/tn");
        assert_eq!(plan.args[2], "JARVIS\\jarvisd");
        assert!(args.contains(&"/sc".to_owned()));
        assert!(args.contains(&"ONLOGON".to_owned()), "{args:?}");
        assert!(args.contains(&"/rl".to_owned()));
        assert!(args.contains(&"LIMITED".to_owned()), "{args:?}");
        assert!(
            args.contains(&"/it".to_owned()),
            "interactive only: {args:?}"
        );
        assert!(args.contains(&"/f".to_owned()), "idempotent: {args:?}");

        // The task definition never records a credential.
        assert!(
            !args.contains(&"/ru".to_owned()),
            "no run-as user: {args:?}"
        );
        assert!(!args.contains(&"/rp".to_owned()), "no password: {args:?}");
        assert!(
            !args.iter().any(|arg| arg.contains("System")),
            "never LocalSystem: {args:?}",
        );
        assert!(
            !args.iter().any(|arg| arg.eq_ignore_ascii_case("HIGHEST")),
            "never elevated: {args:?}",
        );
    }

    #[test]
    fn the_task_run_value_quotes_the_executable_path() {
        let plan = controller().plan_install(&spec()).expect("plan");
        let index = plan
            .args
            .iter()
            .position(|arg| arg == "/tr")
            .expect("/tr present");
        let run = &plan.args[index + 1];
        assert!(
            run.starts_with('"') && run.contains("jarvisd.exe"),
            "an executable path with spaces must be quoted: {run}",
        );
    }

    #[test]
    fn arguments_are_appended_to_the_task_run_value() {
        let spec = spec().with_argument("--foreground").expect("argument");
        let plan = controller().plan_install(&spec).expect("plan");
        let index = plan.args.iter().position(|arg| arg == "/tr").expect("/tr");
        assert!(plan.args[index + 1].contains("--foreground"));
    }

    #[test]
    fn uninstall_run_and_end_are_idempotent_commands() {
        let controller = controller();
        let delete = controller.plan_uninstall(&spec()).expect("delete");
        assert_eq!(delete.args, vec!["/delete", "/tn", "JARVIS\\jarvisd", "/f"]);

        let run = controller.plan_start(&spec()).expect("run");
        assert_eq!(run.args, vec!["/run", "/tn", "JARVIS\\jarvisd"]);

        let end = controller.plan_stop(&spec()).expect("end");
        assert_eq!(end.args, vec!["/end", "/tn", "JARVIS\\jarvisd"]);
    }

    #[test]
    fn a_task_plan_has_no_definition_file() {
        let plan = controller().plan_install(&spec()).expect("plan");
        assert!(plan.definition.is_none());
        assert!(plan.definition_path.is_none());
    }

    #[test]
    fn the_preview_states_the_run_level_and_the_logon_trigger() {
        let rendered = controller().plan_install(&spec()).expect("plan").render();
        assert!(
            rendered.contains("backend:  windows-scheduled-task"),
            "{rendered}"
        );
        assert!(rendered.contains("LIMITED"), "{rendered}");
        assert!(rendered.contains("at logon"), "{rendered}");
        assert!(rendered.contains("no stored password"), "{rendered}");
    }

    /// Exercises the read-only query against the real tool where it exists.
    ///
    /// A task name that cannot exist must report `NotInstalled` rather than an
    /// error: `schtasks /query` exits non-zero for an absent task, which is the
    /// "not installed" answer, not a failure of the facility. This is the one part
    /// of the Windows path that is verifiable without creating anything.
    #[cfg(windows)]
    #[test]
    fn querying_an_absent_task_reports_not_installed() {
        let controller = WindowsTaskController::new();
        let absent = ServiceSpec::new(
            "jarvis-absent-probe-0",
            crate::service::tests_support::host_absolute("/Program Files/JARVIS/jarvisd.exe"),
            "probe",
        )
        .expect("a valid spec");
        assert_eq!(
            controller.status(&absent).expect("the tool is present"),
            crate::service::ServiceState::NotInstalled,
        );
    }
}
