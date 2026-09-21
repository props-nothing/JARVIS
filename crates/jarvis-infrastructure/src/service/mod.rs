//! Per-user service lifecycle.
//!
//! JARVIS normally runs as a per-user background service with **no elevation**:
//! a systemd user unit, a macOS `LaunchAgent`, or a Windows per-user scheduled
//! task. System/server services are a separate, explicit administrator workflow
//! and are deliberately not implemented here.
//!
//! ## Plan and execute are separate
//!
//! [`ServicePlan`] is the exact command line and the exact unit/plist/task
//! definition that would be installed. It can be produced on any platform and
//! asserted in a test without touching the host. [`execute`] runs it. This is
//! what makes a "preview before change" possible and keeps the security-relevant
//! decisions reviewable.
//!
//! ## Secrets
//!
//! A service definition contains executable and configuration paths and
//! non-secret switches only. Provider keys and local bearer tokens are never
//! embedded in a unit file, plist, task command line, or service-manager
//! environment block.

pub mod drift;
pub mod exec;
mod launchd;
mod systemd;
mod windows;

pub use drift::{launchd_executable, systemd_executable};
pub use exec::{CommandOutcome, execute, run_program};
pub use launchd::LaunchdController;
pub use systemd::SystemdController;
pub use windows::WindowsTaskController;

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// The maximum length of a JARVIS service identifier.
pub const MAX_SERVICE_NAME_LEN: usize = 64;

/// The service identifier used for a standard per-user install.
pub const DEFAULT_SERVICE_NAME: &str = "jarvisd";

/// Which service facility a plan targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceBackend {
    /// A `systemd` user unit under the XDG user configuration path.
    SystemdUser,
    /// A macOS `LaunchAgent` under `~/Library/LaunchAgents`.
    LaunchAgent,
    /// A Windows per-user scheduled task created with `schtasks`.
    WindowsScheduledTask,
}

impl ServiceBackend {
    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SystemdUser => "systemd-user",
            Self::LaunchAgent => "launch-agent",
            Self::WindowsScheduledTask => "windows-scheduled-task",
        }
    }
}

/// A validated, concrete service definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    /// The service identifier. Restricted to a safe character set.
    pub name: String,
    /// The absolute path to the daemon executable.
    pub executable: PathBuf,
    /// A short human description.
    pub description: String,
    /// Extra non-secret arguments passed to the daemon.
    pub arguments: Vec<String>,
}

impl ServiceSpec {
    /// Builds a spec, validating every field.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InvalidName`] for an unusable identifier,
    /// [`ServiceError::RelativeExecutable`] when the executable is not absolute,
    /// and [`ServiceError::UnsafeCharacters`] when a field contains a control
    /// character or newline. Those characters would let a value forge an extra
    /// directive in a unit file or a second command in a task definition, so
    /// they are refused rather than escaped.
    pub fn new(
        name: impl Into<String>,
        executable: impl Into<PathBuf>,
        description: impl Into<String>,
    ) -> Result<Self, ServiceError> {
        let spec = Self {
            name: name.into(),
            executable: executable.into(),
            description: description.into(),
            arguments: Vec::new(),
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Adds a non-secret argument.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::UnsafeCharacters`] when the argument contains a
    /// control character.
    pub fn with_argument(mut self, argument: impl Into<String>) -> Result<Self, ServiceError> {
        let argument = argument.into();
        if has_unsafe_characters(&argument) {
            return Err(ServiceError::UnsafeCharacters);
        }
        self.arguments.push(argument);
        Ok(self)
    }

    /// Validates the whole spec.
    ///
    /// # Errors
    ///
    /// Returns the first problem found.
    pub fn validate(&self) -> Result<(), ServiceError> {
        if !is_safe_name(&self.name) {
            return Err(ServiceError::InvalidName);
        }
        if !self.executable.is_absolute() {
            return Err(ServiceError::RelativeExecutable);
        }
        if has_unsafe_characters(&self.description)
            || has_unsafe_characters(&self.executable.to_string_lossy())
            || self
                .arguments
                .iter()
                .any(|value| has_unsafe_characters(value))
        {
            return Err(ServiceError::UnsafeCharacters);
        }
        Ok(())
    }
}

/// The observed state of a service registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// No registration exists.
    NotInstalled,
    /// Registered but not currently running.
    Installed,
    /// Registered and running.
    Running,
}

impl ServiceState {
    /// Returns the stable name used in diagnostics.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Installed => "installed",
            Self::Running => "running",
        }
    }
}

/// A fully described change, previewable before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePlan {
    /// The facility this plan targets.
    pub backend: ServiceBackend,
    /// The program to run.
    pub program: PathBuf,
    /// The exact arguments passed to the program.
    pub args: Vec<String>,
    /// The exact definition file content, for facilities that use a file.
    ///
    /// `None` for a facility driven entirely by a command, such as `schtasks`.
    /// The content is generated here and contains no secret.
    pub definition: Option<String>,
    /// The path the definition is written to, when there is one.
    pub definition_path: Option<PathBuf>,
    /// A one-line human summary of the effect.
    pub summary: String,
}

impl ServicePlan {
    /// Renders the plan for an operator preview.
    ///
    /// The rendering is what a user approves, so it shows the program, the
    /// argument list, and the definition content verbatim.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let _ = writeln!(out, "backend:  {}", self.backend.name());
        let _ = writeln!(out, "effect:   {}", self.summary);
        let _ = writeln!(out, "program:  {}", self.program.display());
        let display_args: Vec<String> = self
            .args
            .iter()
            .map(|argument| {
                if argument.contains(' ') {
                    format!("\"{argument}\"")
                } else {
                    argument.clone()
                }
            })
            .collect();
        let _ = writeln!(out, "args:     {}", display_args.join(" "));
        if let Some(path) = &self.definition_path {
            let _ = writeln!(out, "writes:   {}", path.display());
        }
        if let Some(definition) = &self.definition {
            out.push_str("--- definition ---\n");
            out.push_str(definition);
            if !definition.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("--- end ---\n");
        }
        out
    }
}

/// An error raised by service management.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ServiceError {
    /// The service name is not usable.
    #[error("the service name is not usable")]
    InvalidName,
    /// The executable path is not absolute.
    #[error("the executable path must be absolute")]
    RelativeExecutable,
    /// A field contains a control character or newline.
    #[error("a service field contains a control character")]
    UnsafeCharacters,
    /// The service facility is not available on this system.
    #[error("the service facility is not available on this system")]
    FacilityUnavailable,
    /// A service-manager command failed.
    #[error("the service-manager command failed")]
    CommandFailed {
        /// The machine code for the failure.
        code: &'static str,
    },
    /// A required program could not be run.
    #[error("the service-manager program could not be run")]
    ProgramUnavailable,
    /// A definition file could not be written.
    #[error("the service definition could not be written")]
    DefinitionWrite,
}

impl ServiceError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidName => "jarvis.service_invalid_name",
            Self::RelativeExecutable => "jarvis.service_relative_executable",
            Self::UnsafeCharacters => "jarvis.service_unsafe_characters",
            Self::FacilityUnavailable => "jarvis.service_facility_unavailable",
            Self::CommandFailed { code } => code,
            Self::ProgramUnavailable => "jarvis.service_program_unavailable",
            Self::DefinitionWrite => "jarvis.service_definition_write",
        }
    }

    /// Returns whether the failed operation is safe to retry unchanged.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        match self {
            Self::FacilityUnavailable
            | Self::CommandFailed { .. }
            | Self::ProgramUnavailable
            | Self::DefinitionWrite => true,
            Self::InvalidName | Self::RelativeExecutable | Self::UnsafeCharacters => false,
        }
    }

    /// Returns actionable, non-secret operator advice.
    #[must_use]
    pub const fn advice(&self) -> &'static str {
        match self {
            Self::InvalidName => {
                "Use 1-64 characters from letters, digits, dot, underscore, or hyphen."
            }
            Self::RelativeExecutable => "Provide the absolute path to the installed `jarvisd`.",
            Self::UnsafeCharacters => {
                "Remove control characters or newlines from the service fields."
            }
            Self::FacilityUnavailable => {
                "This platform has no supported service facility. Use foreground or portable mode."
            }
            Self::CommandFailed { .. } => "Re-run with `jarvis doctor` to see the service state.",
            Self::ProgramUnavailable => {
                "The service-manager tool is missing. Use foreground or portable mode."
            }
            Self::DefinitionWrite => "Check ownership and disk space for the service path.",
        }
    }
}

/// Controls a per-user service facility.
pub trait ServiceController: fmt::Debug + Send + Sync {
    /// Returns the facility this controller manages.
    fn backend(&self) -> ServiceBackend;

    /// Returns the observed state of `spec`.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::ProgramUnavailable`] when the facility tool is
    /// missing and [`ServiceError::FacilityUnavailable`] when this platform has
    /// no supported facility.
    fn status(&self, spec: &ServiceSpec) -> Result<ServiceState, ServiceError>;

    /// Builds the plan that installs `spec`. Does not execute anything.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::FacilityUnavailable`] when this platform has no
    /// supported facility.
    fn plan_install(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError>;

    /// Builds the plan that removes `spec`. Does not execute anything.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::FacilityUnavailable`] when this platform has no
    /// supported facility.
    fn plan_uninstall(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError>;

    /// Builds the plan that starts `spec`. Does not execute anything.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::FacilityUnavailable`] when this platform has no
    /// supported facility.
    fn plan_start(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError>;

    /// Builds the plan that stops `spec`. Does not execute anything.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::FacilityUnavailable`] when this platform has no
    /// supported facility.
    fn plan_stop(&self, spec: &ServiceSpec) -> Result<ServicePlan, ServiceError>;

    /// Returns the executable the **installed** definition names, if one can be read.
    ///
    /// This answers `ACC-003`'s service-path fault: a registered service can point
    /// at a path that is no longer the installed daemon, in which case it reports
    /// `installed` and starts nothing. Reading what is on disk is the only way to
    /// see that, because the intended definition always agrees with itself.
    ///
    /// The three answers are deliberately distinct:
    ///
    /// - `Ok(Some(path))` — a definition was read and it names `path`;
    /// - `Ok(None)` — there is no definition to read, so there is nothing to
    ///   compare and no drift can be claimed;
    /// - `Err(_)` — a definition exists but could not be read, which is itself a
    ///   fault and must not be reported as "no drift".
    ///
    /// The default implementation reads no definition and therefore reports
    /// `Ok(None)`, which is the honest answer for a backend whose definition
    /// cannot be read with the current dependency set.
    ///
    /// # Errors
    ///
    /// Returns the backend's error when a definition exists but could not be read.
    fn installed_executable(&self, spec: &ServiceSpec) -> Result<Option<PathBuf>, ServiceError> {
        let _ = spec;
        Ok(None)
    }
}

/// Whether a registered service still names the daemon executable it should.
///
/// `ACC-003` seeds a service-path fault: an update that moved the version
/// directory, or a partial uninstall, leaves a registered service pointing at a
/// binary that is gone. The service reports `installed` and starts nothing, so
/// without this check the fault is invisible.
///
/// `Unreadable` is a separate answer from `NoDefinition`, and the distinction is
/// the point: "there is nothing to compare" and "a definition exists that this
/// build cannot read" need different operator actions, and collapsing them would
/// report an unreadable definition as a healthy one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServicePathDrift {
    /// No definition is installed, so there is nothing to compare.
    NoDefinition,
    /// The installed definition names the expected executable.
    Matches,
    /// The installed definition names a different executable.
    Drifted {
        /// The executable the installed definition names.
        registered: PathBuf,
        /// The executable this build would register.
        expected: PathBuf,
    },
    /// A definition is installed but could not be read.
    Unreadable {
        /// The stable code from the read failure.
        code: &'static str,
    },
}

impl ServicePathDrift {
    /// Returns the stable code used in diagnostics.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoDefinition => "jarvis.service_not_installed",
            Self::Matches => "jarvis.service_path_ok",
            Self::Drifted { .. } => "jarvis.service_path_drift",
            Self::Unreadable { code } => code,
        }
    }

    /// Returns whether the service points at the wrong executable.
    #[must_use]
    pub const fn is_drifted(&self) -> bool {
        matches!(self, Self::Drifted { .. })
    }
}

/// Compares the installed service definition against `spec`.
///
/// This is the whole `ACC-003` service-path check: read what is on disk, compare it
/// to the executable this build would register, and report a drift with both paths
/// so an operator can see which one is wrong.
///
/// The comparison is on the **rendered installed path**, not on the file existing.
/// A definition file that exists is what `status` already reports, and it is
/// precisely the case that hides this fault.
///
/// # Errors
///
/// Returns the backend's [`ServiceError`] only when a definition exists but could
/// not be read; a missing definition is `Ok(NoDefinition)`.
pub fn path_drift(
    controller: &dyn ServiceController,
    spec: &ServiceSpec,
) -> Result<ServicePathDrift, ServiceError> {
    spec.validate()?;
    let Some(registered) = controller.installed_executable(spec)? else {
        return Ok(ServicePathDrift::NoDefinition);
    };
    if paths_match(&registered, &spec.executable) {
        Ok(ServicePathDrift::Matches)
    } else {
        Ok(ServicePathDrift::Drifted {
            registered,
            expected: spec.executable.clone(),
        })
    }
}

/// Returns whether two executable paths denote the same file.
///
/// Compared as canonical paths when both exist, because the same executable is
/// routinely reached through a symlink or a different spelling — and a drift
/// reported for an equivalent path would be a false positive, which is worse than
/// no check at all. When canonicalization cannot be done (the registered path is
/// gone, which is one of the faults being detected), the comparison falls back to
/// the literal paths, so a *missing* registered binary is still reported as drift
/// rather than silently matching.
fn paths_match(registered: &std::path::Path, expected: &std::path::Path) -> bool {
    if registered == expected {
        return true;
    }
    match (
        std::fs::canonicalize(registered),
        std::fs::canonicalize(expected),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Returns the controller for the current platform.
///
/// # Errors
///
/// Returns [`ServiceError::FacilityUnavailable`] when the platform has no
/// supported per-user facility.
pub fn current_controller() -> Result<Box<dyn ServiceController>, ServiceError> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(SystemdController::new()))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(LaunchdController::new()))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(WindowsTaskController::new()))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err(ServiceError::FacilityUnavailable)
    }
}

/// Returns whether `value` contains a character that could forge a directive.
#[must_use]
fn has_unsafe_characters(value: &str) -> bool {
    value.chars().any(char::is_control)
}

/// Quotes a value for a systemd unit executable line.
///
/// systemd parses the value after `=` according to its own quoting rules, where
/// a double quote and a backslash are significant. A path containing whitespace
/// must be quoted or systemd splits it into separate words, so the value is
/// quoted and the two significant characters are escaped whenever either is
/// present.
#[must_use]
fn systemd_escape(value: &str) -> String {
    let needs_quoting = value
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '"' | '\\'));
    if needs_quoting {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_owned()
    }
}

/// Escapes a value for XML text content.
///
/// The three characters that can end a text node early are replaced with their
/// entity references. This is the structured transformation for the format, not
/// a heuristic: no other character is significant inside an XML text node.
#[must_use]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Returns whether `name` is a safe service identifier.
#[must_use]
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SERVICE_NAME_LEN
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod drift_tests {
    //! `ACC-003`'s service-path fault, at the level the check actually runs.
    //!
    //! These assert the installed definition is read **from disk**, because the
    //! fault is a definition that disagrees with what this build would write. A
    //! test that compared the intended spec to itself would agree and prove
    //! nothing, which is why the fixture writes a real unit file naming a
    //! different executable.

    use std::path::PathBuf;

    use super::{ServiceController, ServicePathDrift, ServiceSpec, SystemdController, path_drift};

    fn spec() -> ServiceSpec {
        let executable = std::env::temp_dir()
            .join("jarvis-drift-expected")
            .join("jarvisd");
        ServiceSpec::new("jarvisd", executable, "JARVIS local daemon (jarvisd)").expect("spec")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-drift-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn no_installed_definition_is_not_a_drift() {
        // "There is nothing to compare" and "it points at the wrong binary" need
        // different operator actions, so they must not share an answer.
        let controller = SystemdController::with_paths(temp_dir("absent"), "systemctl");
        assert_eq!(
            path_drift(&controller, &spec()).expect("no error"),
            ServicePathDrift::NoDefinition,
        );
    }

    #[test]
    fn a_definition_naming_the_expected_executable_is_a_match() {
        let dir = temp_dir("match");
        let controller = SystemdController::with_paths(&dir, "systemctl");
        let spec = spec();
        let unit_path = controller.unit_path(&spec);
        std::fs::write(
            &unit_path,
            format!(
                "[Service]\nExecStart={}\n",
                spec.executable.to_string_lossy()
            ),
        )
        .expect("write unit");

        assert_eq!(
            path_drift(&controller, &spec).expect("no error"),
            ServicePathDrift::Matches,
        );
    }

    #[test]
    fn a_definition_naming_a_moved_executable_is_a_drift() {
        // The exact `ACC-003` fault: the registered service survived an update that
        // moved the version directory, so it names a path that no longer is the
        // installed daemon. The file still exists, so `status` reports `installed`
        // and the fault is invisible without reading the definition.
        let dir = temp_dir("drifted");
        let controller = SystemdController::with_paths(&dir, "systemctl");
        let spec = spec();
        let unit_path = controller.unit_path(&spec);
        std::fs::write(
            &unit_path,
            "[Service]\nExecStart=/old/versions/v0.1.0/bin/jarvisd\n",
        )
        .expect("write unit");

        let drift = path_drift(&controller, &spec).expect("no error");
        // A `match` with an `expect` rather than a `panic!`: the workspace lint
        // policy denies `panic!` even in a test, because a panic in production
        // code is the thing the rule exists to prevent and a test is not exempt.
        let ServicePathDrift::Drifted {
            registered,
            expected,
        } = &drift
        else {
            unreachable!("expected a drift, got {drift:?}")
        };
        // Both paths are reported: "it drifted" without saying from what to what
        // is not actionable.
        assert_eq!(
            registered,
            &PathBuf::from("/old/versions/v0.1.0/bin/jarvisd")
        );
        assert_eq!(expected, &spec.executable);
        assert!(drift.is_drifted());
        assert_eq!(drift.code(), "jarvis.service_path_drift");
        // And the service still reports as installed, which is why this check has
        // to exist: the existing state is not the same as the working state.
        assert_eq!(
            controller.status(&spec).expect("status"),
            super::ServiceState::Installed,
        );
    }

    #[test]
    fn an_unreadable_definition_is_not_reported_as_absent() {
        // A definition that exists but cannot be read must not be answered with
        // "no definition": that would report an unknown service as a healthy one.
        let dir = temp_dir("unreadable");
        let controller = SystemdController::with_paths(&dir, "systemctl");
        let spec = spec();
        // A directory at the unit path exists but cannot be read as a file.
        std::fs::create_dir_all(controller.unit_path(&spec)).expect("create dir");

        let error = path_drift(&controller, &spec).expect_err("must not be Ok");
        assert_eq!(error.code(), "jarvis.service_definition_write");
    }

    #[test]
    fn every_drift_answer_has_a_distinct_code() {
        let answers = [
            ServicePathDrift::NoDefinition.code(),
            ServicePathDrift::Matches.code(),
            ServicePathDrift::Drifted {
                registered: PathBuf::from("/a"),
                expected: PathBuf::from("/b"),
            }
            .code(),
        ];
        let mut unique = answers.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), answers.len(), "{answers:?}");
    }
}

/// Exposes the systemd renderer to the drift tests.
///
/// The drift parser must read back what the renderer writes, and that property is
/// only testable if both are reachable from one place. It is not `pub` outside the
/// crate: a renderer is an implementation detail of the controller.
#[cfg(test)]
fn systemd_unit_for_test(spec: &ServiceSpec) -> String {
    systemd_unit(spec)
}

/// Exposes the launchd renderer to the drift tests, for the same reason.
#[cfg(test)]
fn launchd_plist_for_test(spec: &ServiceSpec) -> String {
    launchd_plist(spec)
}

/// Renders one systemd unit file for `spec`.
fn systemd_unit(spec: &ServiceSpec) -> String {
    // No `Environment=` line: a unit file must never carry a secret. The daemon
    // reads configuration and secret references from its own profile.
    let mut arguments = systemd_escape(&spec.executable.to_string_lossy());
    for argument in &spec.arguments {
        arguments.push(' ');
        arguments.push_str(&systemd_escape(argument));
    }
    format!(
        "[Unit]\n\
         Description={description}\n\
         Documentation=file:{executable}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={arguments}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         # Bounded stop so a wedged task cannot block shutdown indefinitely.\n\
         TimeoutStopSec=15\n\
         # Never elevate: this is a per-user unit.\n\
         NoNewPrivileges=true\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        description = systemd_escape(&spec.description),
        executable = systemd_escape(&spec.executable.to_string_lossy()),
        arguments = arguments,
    )
}

/// Renders one macOS `LaunchAgent` property list for `spec`.
///
/// The plist is hand-rendered rather than produced by a serializer because it is
/// a short fixed shape. Every interpolated value is passed through [`xml_escape`]
/// so the three characters that are significant in an XML text node cannot end a
/// node early.
fn launchd_plist(spec: &ServiceSpec) -> String {
    use std::fmt::Write as _;

    let mut program_arguments = String::new();
    let _ = writeln!(
        program_arguments,
        "        <string>{}</string>",
        xml_escape(&spec.executable.to_string_lossy()),
    );
    for argument in &spec.arguments {
        let _ = writeln!(
            program_arguments,
            "        <string>{}</string>",
            xml_escape(argument),
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20   <key>Label</key>\n\
         \x20   <string>{label}</string>\n\
         \x20   <key>ProgramArguments</key>\n\
         \x20   <array>\n\
         {program_arguments}\
         \x20   </array>\n\
         \x20   <key>RunAtLoad</key>\n\
         \x20   <true/>\n\
         \x20   <key>KeepAlive</key>\n\
         \x20   <dict>\n\
         \x20       <key>SuccessfulExit</key>\n\
         \x20       <false/>\n\
         \x20   </dict>\n\
         \x20   <key>ProcessType</key>\n\
         \x20   <string>Background</string>\n\
         </dict>\n\
         </plist>\n",
        label = xml_escape(&launchd_label(spec)),
    )
}

/// Returns the `LaunchAgent` label for `spec`.
fn launchd_label(spec: &ServiceSpec) -> String {
    format!("com.jarvis.{}", spec.name)
}

/// Small helpers shared by the service test modules.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::path::PathBuf;

    /// Returns a path that `Path::is_absolute` accepts on the host running the
    /// test.
    ///
    /// `Path::is_absolute` is host-specific: on Windows `/bin/jarvisd` has no
    /// drive or UNC prefix and is therefore relative, while on Unix it is
    /// absolute. A test that hard-codes a Unix path silently exercises the
    /// "relative executable" rejection on Windows instead of the behavior it
    /// claims to check. Every service fixture goes through this function so the
    /// assertions hold on every host.
    /// The rendered text is native to the host: `PathBuf::from` normalises
    /// separators, so on Windows this yields a backslash path. Assertions on a
    /// rendered definition therefore compare against the renderer's own escaping
    /// rather than the raw path, and the escaping rules have focused tests of
    /// their own.
    #[must_use]
    pub fn host_absolute(unix: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(
                "C:\\{}",
                unix.trim_start_matches('/').replace('/', "\\"),
            ))
        } else {
            PathBuf::from(unix)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        MAX_SERVICE_NAME_LEN, ServiceError, ServiceSpec, ServiceState, is_safe_name, launchd_label,
        launchd_plist, systemd_escape, systemd_unit,
    };

    fn spec() -> ServiceSpec {
        ServiceSpec::new(
            "jarvisd",
            host_absolute("/usr/local/bin/jarvisd"),
            "JARVIS daemon",
        )
        .expect("a valid spec")
    }

    /// Returns a path that `is_absolute` accepts on the host running the test.
    ///
    /// `Path::is_absolute` is host-specific: on Windows `/bin/jarvisd` has no
    /// drive or UNC prefix and is relative, while on Unix it is absolute. The
    /// fixture follows the host so one set of assertions holds everywhere.
    fn host_absolute(unix: &str) -> PathBuf {
        crate::service::tests_support::host_absolute(unix)
    }

    /// Returns the escaped form the systemd renderer produces for `value`.
    fn escaped(value: &str) -> String {
        systemd_escape(value)
    }

    #[test]
    fn a_valid_spec_is_accepted() {
        let spec = spec();
        assert_eq!(spec.name, "jarvisd");
        assert!(spec.arguments.is_empty());
        spec.validate().expect("valid");
    }

    #[test]
    fn unusable_names_are_rejected() {
        for name in [
            "",
            "has space",
            "has/slash",
            "has:colon",
            "emoji🙂",
            &"a".repeat(65),
        ] {
            let error = ServiceSpec::new(name, host_absolute("/bin/jarvisd"), "d")
                .expect_err("must be rejected");
            assert_eq!(error, ServiceError::InvalidName, "{name:?}");
            assert_eq!(error.code(), "jarvis.service_invalid_name");
        }
        assert!(is_safe_name("jarvisd"));
        assert!(is_safe_name("jarvis.d-1_2"));
        assert_eq!(MAX_SERVICE_NAME_LEN, 64);
    }

    #[test]
    fn a_relative_executable_is_rejected() {
        let error = ServiceSpec::new("jarvisd", PathBuf::from("jarvisd"), "d")
            .expect_err("must be rejected");
        assert_eq!(error, ServiceError::RelativeExecutable);
    }

    #[test]
    fn control_characters_are_rejected_rather_than_escaped() {
        // A newline could forge an extra unit directive, so it must never reach
        // a definition file.
        let error = ServiceSpec::new(
            "jarvisd",
            host_absolute("/bin/jarvisd"),
            "description\nExecStartPre=/bin/evil",
        )
        .expect_err("must be rejected");
        assert_eq!(error, ServiceError::UnsafeCharacters);

        let error = spec()
            .with_argument("--ok\nEnvironment=SECRET=leak")
            .expect_err("must be rejected");
        assert_eq!(error, ServiceError::UnsafeCharacters);

        let error = ServiceSpec::new("jarvisd", host_absolute("/bin/\rjarvisd"), "d")
            .expect_err("must be rejected");
        assert_eq!(error, ServiceError::UnsafeCharacters);
    }

    #[test]
    fn a_valid_argument_is_accepted() {
        let spec = spec()
            .with_argument("--foreground")
            .expect("valid argument");
        assert_eq!(spec.arguments, vec!["--foreground".to_owned()]);
    }

    #[test]
    fn the_systemd_unit_has_the_required_properties() {
        let spec = spec();
        let unit = systemd_unit(&spec);
        let exe = spec.executable.to_string_lossy().into_owned();
        assert!(unit.contains("[Unit]"), "{unit}");
        assert!(
            unit.contains(&format!("ExecStart={}", escaped(&exe))),
            "{unit}",
        );
        assert!(unit.contains("Restart=on-failure"), "{unit}");
        assert!(unit.contains("TimeoutStopSec=15"), "bounded stop: {unit}");
        assert!(
            unit.contains("NoNewPrivileges=true"),
            "no elevation: {unit}"
        );
        assert!(unit.contains("WantedBy=default.target"), "per-user: {unit}");
        // A unit file must never carry a secret or an environment block.
        assert!(
            !unit.contains("Environment="),
            "no secrets in the unit: {unit}"
        );
        assert!(!unit.to_ascii_lowercase().contains("token"), "{unit}");
    }

    #[test]
    fn a_path_with_whitespace_is_quoted_in_the_systemd_unit() {
        // A path with a space is legal on every platform. Unquoted, systemd would
        // split it into separate words and start the wrong program.
        let spaced = "/opt/JARVIS Suite/jarvisd";
        assert_eq!(escaped(spaced), format!("\"{spaced}\""));

        let spec = ServiceSpec::new("jarvisd", host_absolute(spaced), "d").expect("a spec");
        let unit = systemd_unit(&spec);
        let expected = format!("ExecStart={}", escaped(&spec.executable.to_string_lossy()));
        assert!(unit.contains(&expected), "{unit}");
    }

    #[test]
    fn a_quote_in_a_value_is_escaped_in_the_systemd_unit() {
        // The quote is escaped, so it cannot terminate the quoted value early.
        assert_eq!(escaped(r#"/opt/odd"/jarvisd"#), r#""/opt/odd\"/jarvisd""#);
    }

    #[test]
    fn xml_significant_characters_are_escaped_in_the_plist() {
        // `&` and `<` in an XML text node would otherwise make the plist
        // unparseable, which launchd rejects without a useful message.
        let spec = ServiceSpec::new("jarvisd", host_absolute("/opt/a&b<jarvisd"), "d")
            .expect("a valid spec");
        let plist = launchd_plist(&spec);
        assert!(plist.contains("&amp;"), "{plist}");
        assert!(plist.contains("&lt;"), "{plist}");
        assert!(
            !plist.contains("/opt/a&b<jarvisd"),
            "the raw value must not survive: {plist}",
        );
    }

    #[test]
    fn a_backslash_in_a_path_is_escaped_in_the_systemd_unit() {
        // A Windows-shaped path reaching the systemd renderer must not produce an
        // unescaped backslash, which systemd would read as an escape introducer.
        let escaped = systemd_escape("C:\\Program Files\\jarvisd.exe");
        assert_eq!(
            escaped, "\"C:\\\\Program Files\\\\jarvisd.exe\"",
            "{escaped}"
        );
    }

    #[test]
    fn a_plain_value_is_not_quoted_in_the_systemd_unit() {
        assert_eq!(systemd_escape("jarvisd"), "jarvisd");
        assert_eq!(systemd_escape("--foreground"), "--foreground");
    }

    #[test]
    fn the_plist_label_matches_the_plist_filename() {
        // launchd matches a bootstrapped service to its definition by the Label,
        // so a Label that differs from the file name makes the agent impossible
        // to load or remove.
        let spec = spec();
        let plist = launchd_plist(&spec);
        let expected = format!("<string>{}</string>", launchd_label(&spec));
        assert!(plist.contains(&expected), "{plist}");
        assert!(
            launchd_label(&spec) != spec.name,
            "the label must be namespaced, not the bare service name",
        );
    }

    #[test]
    fn the_launchd_plist_is_well_formed_and_labelled() {
        let spec = spec();
        let plist = launchd_plist(&spec);
        let executable = spec.executable.to_string_lossy().into_owned();
        assert!(plist.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("com.jarvis.jarvisd"));
        assert!(
            plist.contains(&format!("<string>{executable}</string>")),
            "{plist}"
        );
        assert!(plist.trim_end().ends_with("</plist>"));
        assert_eq!(launchd_label(&spec), "com.jarvis.jarvisd");
        // No secret and no environment block.
        assert!(!plist.contains("EnvironmentVariables"), "{plist}");
    }

    #[test]
    fn arguments_appear_in_both_definition_formats() {
        let spec = spec().with_argument("--foreground").expect("argument");
        let exe = escaped(&spec.executable.to_string_lossy());
        assert!(
            systemd_unit(&spec).contains(&format!("ExecStart={exe} --foreground")),
            "{}",
            systemd_unit(&spec),
        );
        let plist = launchd_plist(&spec);
        assert!(plist.contains("<string>--foreground</string>"), "{plist}");
    }

    #[test]
    fn service_states_have_stable_names() {
        assert_eq!(ServiceState::NotInstalled.name(), "not_installed");
        assert_eq!(ServiceState::Installed.name(), "installed");
        assert_eq!(ServiceState::Running.name(), "running");
    }

    #[test]
    fn every_error_has_a_code_and_advice() {
        let errors = [
            ServiceError::InvalidName,
            ServiceError::RelativeExecutable,
            ServiceError::UnsafeCharacters,
            ServiceError::FacilityUnavailable,
            ServiceError::CommandFailed {
                code: "jarvis.service_command_failed",
            },
            ServiceError::ProgramUnavailable,
            ServiceError::DefinitionWrite,
        ];
        for error in errors {
            assert!(error.code().starts_with("jarvis."), "{:?}", error.code());
            assert!(!error.advice().is_empty(), "{error:?}");
        }
        assert!(!ServiceError::InvalidName.retryable());
        assert!(ServiceError::FacilityUnavailable.retryable());
    }
}
