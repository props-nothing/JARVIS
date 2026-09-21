//! The macOS `LaunchAgent` controller.
//!
//! A user-owned `LaunchAgent` under `~/Library/LaunchAgents`, managed with
//! `launchctl` in the `gui/<uid>` domain. A root `LaunchDaemon` is deliberately
//! never installed: that would run JARVIS outside the user's session and outside
//! the per-user security model.

use std::path::PathBuf;

use super::{
    ServiceBackend, ServiceController, ServiceError, ServicePlan, ServiceSpec, ServiceState,
    launchd_label, launchd_plist,
};

/// Controls a per-user `LaunchAgent`.
#[derive(Debug, Clone)]
pub struct LaunchdController {
    /// The directory holding `LaunchAgent` property lists.
    pub agents_dir: PathBuf,
    /// The `launchctl` executable name.
    pub launchctl: String,
    /// The numeric user id whose `gui` domain is targeted.
    pub uid: u32,
}

impl LaunchdController {
    /// Creates a controller using the standard user `LaunchAgents` directory.
    #[must_use]
    pub fn new() -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/Users"), PathBuf::from);
        Self {
            agents_dir: home.join("Library").join("LaunchAgents"),
            launchctl: "launchctl".to_owned(),
            uid: current_uid(),
        }
    }

    /// Creates a controller with explicit paths, for tests and previews.
    #[must_use]
    pub fn with_paths(
        agents_dir: impl Into<PathBuf>,
        launchctl: impl Into<String>,
        uid: u32,
    ) -> Self {
        Self {
            agents_dir: agents_dir.into(),
            launchctl: launchctl.into(),
            uid,
        }
    }

    /// Returns the plist path for `spec`.
    #[must_use]
    pub fn plist_path(&self, spec: &ServiceSpec) -> PathBuf {
        self.agents_dir
            .join(format!("{}.plist", launchd_label(spec)))
    }

    /// Returns the `gui/<uid>` domain target.
    #[must_use]
    pub fn domain(&self) -> String {
        format!("gui/{}", self.uid)
    }
}

impl Default for LaunchdController {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceController for LaunchdController {
    fn backend(&self) -> ServiceBackend {
        ServiceBackend::LaunchAgent
    }

    fn status(&self, spec: &ServiceSpec) -> Result<ServiceState, ServiceError> {
        if !self.plist_path(spec).exists() {
            return Ok(ServiceState::NotInstalled);
        }
        Ok(ServiceState::Installed)
    }

    /// Reads the executable the installed plist names.
    ///
    /// A plist that exists but cannot be read is reported as an error rather than
    /// as "no definition", because the agent is registered and its content is
    /// unknown: reporting nothing to compare would hide the drift this check is
    /// for.
    fn installed_executable(&self, spec: &ServiceSpec) -> Result<Option<PathBuf>, ServiceError> {
        let path = self.plist_path(spec);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|_| ServiceError::DefinitionWrite)?;
        let text = String::from_utf8(bytes).map_err(|_| ServiceError::DefinitionWrite)?;
        Ok(super::drift::launchd_executable(&text))
    }

    fn plan_install(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        let plist = launchd_plist(spec);
        let plist_path = self.plist_path(spec);
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.launchctl),
            // `bootstrap` loads the agent into the user's graphical session. It
            // is the modern replacement for `load` and requires no elevation.
            args: vec![
                "bootstrap".to_owned(),
                self.domain(),
                plist_path.to_string_lossy().into_owned(),
            ],
            definition: Some(plist),
            definition_path: Some(plist_path.clone()),
            summary: format!(
                "Write {} and bootstrap it into {} (no elevation).",
                plist_path.display(),
                self.domain(),
            ),
        })
    }

    fn plan_uninstall(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.launchctl),
            args: vec![
                "bootout".to_owned(),
                self.domain(),
                self.plist_path(spec).to_string_lossy().into_owned(),
            ],
            definition: None,
            definition_path: Some(self.plist_path(spec)),
            summary: format!(
                "Boot the agent out of {} and remove its plist.",
                self.domain()
            ),
        })
    }

    fn plan_start(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.launchctl),
            args: vec![
                "kickstart".to_owned(),
                format!("{}/{}", self.domain(), launchd_label(spec)),
            ],
            definition: None,
            definition_path: None,
            summary: format!("Start the agent {}.", launchd_label(spec)),
        })
    }

    fn plan_stop(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.launchctl),
            args: vec![
                "kill".to_owned(),
                "SIGTERM".to_owned(),
                format!("{}/{}", self.domain(), launchd_label(spec)),
            ],
            definition: None,
            definition_path: None,
            summary: format!("Stop the agent {}.", launchd_label(spec)),
        })
    }
}

/// Returns the current numeric user id where the platform exposes it.
fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        // `id -u` is avoided; the standard library does not expose the uid, so
        // the environment is used and a sane default applies otherwise.
        std::env::var("UID")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(501)
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::LaunchdController;
    use crate::service::{ServiceBackend, ServiceController, ServiceSpec, ServiceState};

    fn spec() -> ServiceSpec {
        ServiceSpec::new(
            "jarvisd",
            crate::service::tests_support::host_absolute("/usr/local/bin/jarvisd"),
            "JARVIS daemon",
        )
        .expect("valid spec")
    }

    fn controller() -> LaunchdController {
        LaunchdController::with_paths("/Users/alice/Library/LaunchAgents", "launchctl", 501)
    }

    #[test]
    fn the_backend_is_launch_agent() {
        assert_eq!(controller().backend(), ServiceBackend::LaunchAgent);
        assert_eq!(ServiceBackend::LaunchAgent.name(), "launch-agent");
    }

    #[test]
    fn install_bootstraps_into_the_user_gui_domain() {
        let controller = controller();
        let plan = controller.plan_install(&spec()).expect("plan");

        assert_eq!(plan.args[0], "bootstrap");
        assert_eq!(plan.args[1], "gui/501", "the user domain, never system");
        assert!(plan.args[2].ends_with("com.jarvis.jarvisd.plist"));
        assert!(
            !plan.args.iter().any(|arg| arg.contains("system")),
            "{plan:?}"
        );

        let plist = plan.definition.expect("a plist");
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("com.jarvis.jarvisd"));
    }

    #[test]
    fn uninstall_boots_the_agent_out() {
        let plan = controller().plan_uninstall(&spec()).expect("plan");
        assert_eq!(plan.args[0], "bootout");
        assert_eq!(plan.args[1], "gui/501");
        assert_eq!(
            plan.definition_path,
            Some(PathBuf::from(
                "/Users/alice/Library/LaunchAgents/com.jarvis.jarvisd.plist"
            )),
        );
    }

    #[test]
    fn start_and_stop_target_the_labelled_service() {
        let controller = controller();
        let start = controller.plan_start(&spec()).expect("start");
        assert_eq!(start.args, vec!["kickstart", "gui/501/com.jarvis.jarvisd"]);

        let stop = controller.plan_stop(&spec()).expect("stop");
        assert_eq!(
            stop.args,
            vec!["kill", "SIGTERM", "gui/501/com.jarvis.jarvisd"]
        );
    }

    #[test]
    fn no_plan_ever_targets_a_system_domain_or_a_daemon_plist() {
        let controller = controller();
        for plan in [
            controller.plan_install(&spec()).expect("install"),
            controller.plan_uninstall(&spec()).expect("uninstall"),
            controller.plan_start(&spec()).expect("start"),
            controller.plan_stop(&spec()).expect("stop"),
        ] {
            for argument in &plan.args {
                assert!(!argument.starts_with("system/"), "{plan:?}");
                assert!(!argument.contains("LaunchDaemons"), "{plan:?}");
                assert!(!argument.contains("sudo"), "{plan:?}");
            }
        }
    }

    #[test]
    fn status_reports_not_installed_for_a_missing_plist() {
        assert_eq!(
            controller().status(&spec()).expect("status"),
            ServiceState::NotInstalled,
        );
    }

    #[test]
    fn the_preview_shows_the_domain_and_the_plist() {
        let rendered = controller().plan_install(&spec()).expect("plan").render();
        assert!(rendered.contains("backend:  launch-agent"), "{rendered}");
        assert!(rendered.contains("gui/501"), "{rendered}");
        assert!(
            rendered.contains("<key>ProgramArguments</key>"),
            "{rendered}"
        );
    }
}
