//! The systemd user-unit controller.
//!
//! A per-user unit under the XDG user configuration path, controlled with
//! `systemctl --user`. Enablement and starting are deliberately separate: the
//! plan that installs enables the unit, and the plan that starts starts it, so an
//! operator preview shows exactly which of the two a command performs.

use std::path::PathBuf;

use super::{
    ServiceBackend, ServiceController, ServiceError, ServicePlan, ServiceSpec, ServiceState,
    systemd_unit,
};

/// Controls a systemd user unit.
#[derive(Debug, Clone)]
pub struct SystemdController {
    /// The directory holding user unit files.
    pub unit_dir: PathBuf,
    /// The `systemctl` executable name.
    pub systemctl: String,
}

impl SystemdController {
    /// Creates a controller using the standard XDG user unit directory.
    #[must_use]
    pub fn new() -> Self {
        // `%t`-style specifier expansion belongs to systemd; here the directory
        // is resolved from the environment with the documented XDG default.
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        Self {
            unit_dir: config_home.join("systemd").join("user"),
            systemctl: "systemctl".to_owned(),
        }
    }

    /// Creates a controller with explicit paths, for tests and previews.
    #[must_use]
    pub fn with_paths(unit_dir: impl Into<PathBuf>, systemctl: impl Into<String>) -> Self {
        Self {
            unit_dir: unit_dir.into(),
            systemctl: systemctl.into(),
        }
    }

    /// Returns the unit file path for `spec`.
    #[must_use]
    pub fn unit_path(&self, spec: &ServiceSpec) -> PathBuf {
        self.unit_dir.join(format!("{}.service", spec.name))
    }

    /// Returns the systemd unit name for `spec`.
    #[must_use]
    pub fn unit_name(&self, spec: &ServiceSpec) -> String {
        format!("{}.service", spec.name)
    }

    fn systemctl_args(&self, spec: &ServiceSpec, verb: &str) -> Vec<String> {
        vec!["--user".to_owned(), verb.to_owned(), self.unit_name(spec)]
    }
}

impl Default for SystemdController {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceController for SystemdController {
    fn backend(&self) -> ServiceBackend {
        ServiceBackend::SystemdUser
    }

    fn status(&self, spec: &ServiceSpec) -> Result<ServiceState, ServiceError> {
        if !self.unit_path(spec).exists() {
            return Ok(ServiceState::NotInstalled);
        }
        // The unit file exists; whether the unit is active is a systemd question
        // that the doctor path answers through `systemctl --user is-active`.
        Ok(ServiceState::Installed)
    }

    fn plan_install(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        let definition = systemd_unit(spec);
        let unit_path = self.unit_path(spec);
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.systemctl),
            args: self.systemctl_args(spec, "enable"),
            definition: Some(definition),
            definition_path: Some(unit_path.clone()),
            summary: format!(
                "Write {} and enable the per-user unit (no elevation).",
                unit_path.display(),
            ),
        })
    }

    fn plan_uninstall(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.systemctl),
            args: self.systemctl_args(spec, "disable"),
            definition: None,
            definition_path: Some(self.unit_path(spec)),
            summary: format!(
                "Stop and disable {}, then remove its unit file.",
                self.unit_name(spec),
            ),
        })
    }

    fn plan_start(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.systemctl),
            args: self.systemctl_args(spec, "start"),
            definition: None,
            definition_path: None,
            summary: format!("Start the per-user unit {}.", self.unit_name(spec)),
        })
    }

    fn plan_stop(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError> {
        spec.validate()?;
        Ok(ServicePlan {
            backend: self.backend(),
            program: PathBuf::from(&self.systemctl),
            args: self.systemctl_args(spec, "stop"),
            definition: None,
            definition_path: None,
            summary: format!("Stop the per-user unit {}.", self.unit_name(spec)),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SystemdController;
    use crate::service::{ServiceBackend, ServiceController, ServiceSpec};

    fn spec() -> ServiceSpec {
        ServiceSpec::new(
            "jarvisd",
            crate::service::tests_support::host_absolute("/usr/local/bin/jarvisd"),
            "JARVIS daemon",
        )
        .expect("valid spec")
    }

    fn controller() -> SystemdController {
        SystemdController::with_paths("/home/user/.config/systemd/user", "systemctl")
    }

    #[test]
    fn the_backend_is_systemd_user() {
        assert_eq!(controller().backend(), ServiceBackend::SystemdUser);
        assert_eq!(ServiceBackend::SystemdUser.name(), "systemd-user");
    }

    #[test]
    fn the_install_plan_writes_the_unit_and_uses_no_elevation() {
        let controller = controller();
        let plan = controller.plan_install(&spec()).expect("plan");

        assert_eq!(plan.program, PathBuf::from("systemctl"));
        assert_eq!(plan.args, vec!["--user", "enable", "jarvisd.service"]);
        assert_eq!(
            plan.definition_path,
            Some(PathBuf::from(
                "/home/user/.config/systemd/user/jarvisd.service"
            )),
        );
        let definition = plan.definition.clone().expect("a definition");
        assert!(definition.contains("[Unit]"));
        assert!(definition.contains("NoNewPrivileges=true"));

        // No absolute path and no elevation appear in the command.
        assert!(
            !plan.args.iter().any(|arg| arg.contains("sudo")),
            "{plan:?}"
        );
        assert!(
            !plan.args.iter().any(|arg| arg.contains("--system")),
            "{plan:?}"
        );
    }

    #[test]
    fn start_and_stop_do_not_write_a_definition() {
        let controller = controller();
        for (plan, verb) in [
            (controller.plan_start(&spec()).expect("start"), "start"),
            (controller.plan_stop(&spec()).expect("stop"), "stop"),
        ] {
            assert_eq!(plan.args, vec!["--user", verb, "jarvisd.service"]);
            assert!(plan.definition.is_none(), "{plan:?}");
            assert!(plan.definition_path.is_none(), "{plan:?}");
        }
    }

    #[test]
    fn uninstall_disable_names_the_unit_file_for_removal() {
        let controller = controller();
        let plan = controller.plan_uninstall(&spec()).expect("plan");
        assert_eq!(plan.args, vec!["--user", "disable", "jarvisd.service"]);
        assert_eq!(
            plan.definition_path,
            Some(PathBuf::from(
                "/home/user/.config/systemd/user/jarvisd.service"
            )),
        );
    }

    #[test]
    fn status_reports_not_installed_for_a_missing_unit_file() {
        let controller = controller();
        assert_eq!(
            controller.status(&spec()).expect("status"),
            crate::service::ServiceState::NotInstalled,
        );
    }

    #[test]
    fn the_unit_directory_defaults_to_the_xdg_user_path() {
        // The controller resolves a real directory; the exact value depends on
        // the environment, so only the shape is asserted.
        let controller = SystemdController::new();
        assert!(
            controller.unit_dir.ends_with("systemd/user"),
            "{}",
            controller.unit_dir.display(),
        );
        assert_eq!(controller.unit_name(&spec()), "jarvisd.service");
    }

    #[test]
    fn the_preview_renders_the_effect_and_the_definition() {
        let plan = controller().plan_install(&spec()).expect("plan");
        let rendered = plan.render();
        assert!(rendered.contains("backend:  systemd-user"), "{rendered}");
        assert!(rendered.contains("effect:   Write "), "{rendered}");
        assert!(rendered.contains("--- definition ---"), "{rendered}");
        assert!(rendered.contains("ExecStart="), "{rendered}");
        assert!(rendered.contains("--- end ---"), "{rendered}");
    }
}
