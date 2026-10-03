//! Tests for MCP server process launch.
//!
//! Four carry more weight than the rest, and each exists because a plausible implementation gets it
//! wrong in a way no other test here would notice:
//!
//! - **`the_child_inherits_no_ambient_environment`** is the isolation property. Inheritance is the
//!   *default*, so the failure mode is an implementation that never called `env_clear` — which passes
//!   every other test here while handing an untrusted server the daemon's credentials.
//! - **`a_resolved_program_path_is_passed_verbatim`** pins the opposite of a mangle: the path that was
//!   reviewed is the path exec'd, with no lossy substitution.
//! - **`stderr_is_piped_rather_than_inherited`** is the one deviation from the SDK's own default, and
//!   the default is what a careless builder keeps.
//! - **`the_diagnostic_tail_keeps_the_last_bytes_not_the_first`** is the direction that makes
//!   diagnostics useful: a server's startup banner explains nothing about why it later died.

// See `tool_schema/tests.rs`: the workspace denies `clippy::panic`, and `clippy.toml`'s
// `allow-expect-in-tests` classifies Cargo `tests/*.rs` targets rather than a `#[path]` module
// reached from the library. In a test a panic *is* the failure report.
#![allow(clippy::panic)]

use std::path::PathBuf;

use super::{
    DEFAULT_MCP_STARTUP_TIMEOUT_MS, MAX_MCP_ARGUMENTS, MAX_MCP_ENV_VALUE_BYTES, MAX_MCP_ENV_VARS,
    MAX_MCP_STDERR_BYTES, MAX_MCP_TOKEN_BYTES, McpLaunchSpec, McpProcessError, StderrTail,
    build_command, spawn_stdio_server,
};
use std::collections::BTreeSet;

/// The environment keys `cmd.exe` **synthesises for its own children**, whatever it was given.
///
/// Measured rather than assumed: a first version of the isolation test asserted that the child saw
/// *only* the listed variables, and it failed listing exactly these three. They are the shell's own
/// bookkeeping — it sets them when they are absent — so they are not evidence of an inherited
/// environment, and treating them as a leak would make the test fail for a reason that is not the
/// property it names.
///
/// The distinction matters because it is *narrow*: if `env_clear()` were removed, the child would
/// inherit the daemon's entire environment and this set would not cover it, so the assertion still
/// detects the failure it exists for.
#[cfg(windows)]
const SHELL_SYNTHESISED_ENV_KEYS: &[&str] = &["COMSPEC", "PATHEXT", "PROMPT"];

/// No shell synthesis on this platform: `sh` passes through what it is given.
#[cfg(not(windows))]
const SHELL_SYNTHESISED_ENV_KEYS: &[&str] = &[];

/// The two variables every isolation assertion here lists, so the sets cannot drift apart.
const LISTED_ENV_KEYS: &[&str] = &["JARVIS_MCP_PROBE_ONE", "JARVIS_MCP_PROBE_TWO"];

/// A specification that must be accepted, with one environment pair.
fn base_spec() -> McpLaunchSpec {
    McpLaunchSpec {
        program: PathBuf::from("/usr/local/bin/mcp-server"),
        args: vec![
            "--serve".to_owned(),
            "--root".to_owned(),
            "/tmp/it".to_owned(),
        ],
        env: vec![("HOME".to_owned(), "/tmp/it".to_owned())],
        working_dir: Some(PathBuf::from("/tmp/it")),
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    }
}

#[test]
fn a_complete_specification_is_accepted() {
    assert_eq!(base_spec().validate(), Ok(()));
}

#[test]
fn the_child_inherits_no_ambient_environment() {
    // The isolation property, and **this test had to be rewritten because its first version could not
    // detect the failure it names.** That version asserted on `build_command(..).as_std().get_envs()`,
    // which reports only the variables *explicitly set on the command* — inheritance happens later, in
    // the OS at spawn time, so the assertion passed identically with and without `env_clear()`. The
    // mutant that deletes `env_clear()` **survived**, which is how the gap was found.
    //
    // So the child is asked to print its own environment instead. The property is two-sided: the
    // reviewed variable must appear (proving the child ran and the output is real), and *every* printed
    // key must be one JARVIS listed (proving nothing ambient leaked). Inheritance is the default, so
    // without `env_clear` the second half fails immediately.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    let spec = McpLaunchSpec {
        program: shell_program(),
        args: shell_args("print environment"),
        env: vec![
            (LISTED_ENV_KEYS[0].to_owned(), "one".to_owned()),
            (LISTED_ENV_KEYS[1].to_owned(), "two".to_owned()),
        ],
        working_dir: None,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    };

    let process = runtime
        .block_on(async { spawn_stdio_server(&spec) })
        .expect("the shell must start");
    let printed = runtime.block_on(async {
        for _ in 0..100 {
            let text = process.diagnostics();
            if text.contains("JARVIS_MCP_PROBE") {
                return text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        process.diagnostics()
    });
    runtime.block_on(async move {
        drop(process);
    });

    // The positive control, so an empty capture (shell absent, command failed) cannot pass.
    assert!(
        printed.contains("JARVIS_MCP_PROBE_ONE=one"),
        "the child must see the reviewed variables, got {printed:?}"
    );
    // The property: no key the operator did not list, other than the ones the shell synthesises for
    // itself. This is what `env_clear` buys, and removing it puts the daemon's whole environment here.
    let allowed: BTreeSet<&str> = LISTED_ENV_KEYS
        .iter()
        .chain(SHELL_SYNTHESISED_ENV_KEYS)
        .copied()
        .collect();
    let unlisted: Vec<String> = printed
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, _)| key.trim().to_owned())
        .filter(|key| {
            // An empty line, and `cmd`'s `=C:`-style per-drive current-directory entries, which are
            // not environment variables in the inherited sense.
            !key.is_empty() && !key.starts_with('=') && !allowed.contains(key.as_str())
        })
        .collect();
    assert!(
        unlisted.is_empty(),
        "the child inherited variables the operator did not list: {unlisted:?}"
    );
}

#[test]
fn an_empty_environment_list_means_the_child_sees_nothing() {
    // The complement, so the assertion above is not satisfied by an implementation that always adds one
    // variable. An operator who configures no environment gets a child with none — asserted at the
    // command level here, since the spawned form is covered by the test above.
    let spec = McpLaunchSpec {
        env: Vec::new(),
        ..base_spec()
    };
    let command = build_command(&spec).expect("the fixture must build");

    assert_eq!(command.as_std().get_envs().count(), 0);
}

#[test]
fn a_resolved_program_path_is_passed_verbatim() {
    // A path is handed back to the operating system, where a substituted character means a *different*
    // file — so the path that was reviewed is the path executed, with no lossy conversion. This is the
    // opposite rule to the one diagnostics follow, and for the opposite reason.
    let spec = McpLaunchSpec {
        program: PathBuf::from("/opt/mcp servers/one"),
        ..base_spec()
    };
    let command = build_command(&spec).expect("the fixture must build");

    assert_eq!(
        command.as_std().get_program().to_string_lossy(),
        "/opt/mcp servers/one"
    );
    // The arguments are verbatim too: the argv form is exactly what makes an argument containing
    // spaces or punctuation inert rather than injectable.
    let args: Vec<String> = command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, vec!["--serve", "--root", "/tmp/it"]);
}

#[test]
fn a_program_path_with_a_control_character_is_refused() {
    // A NUL cannot be passed to an exec call at all, and a newline corrupts a log line. The workspace
    // has one definition of that set (`plugin::contains_control`), reused here rather than restated.
    let spec = McpLaunchSpec {
        program: PathBuf::from("/usr/bin/mcp\nserver"),
        ..base_spec()
    };
    assert_eq!(spec.validate(), Err(McpProcessError::ProgramInvalid));

    let empty = McpLaunchSpec {
        program: PathBuf::new(),
        ..base_spec()
    };
    assert_eq!(empty.validate(), Err(McpProcessError::ProgramInvalid));
}

#[test]
fn an_argument_with_a_control_character_is_refused_rather_than_quoted() {
    let spec = McpLaunchSpec {
        args: vec!["--name=x\0y".to_owned()],
        ..base_spec()
    };
    assert_eq!(spec.validate(), Err(McpProcessError::ArgumentInvalid));

    let too_many = McpLaunchSpec {
        args: vec!["a".to_owned(); MAX_MCP_ARGUMENTS + 1],
        ..base_spec()
    };
    assert_eq!(too_many.validate(), Err(McpProcessError::ArgumentInvalid));
}

#[test]
fn an_environment_entry_that_is_not_one_assignment_is_refused() {
    // `=` is the separator the platform uses, so a key containing one is two assignments rather than a
    // name. Refusing it keeps the list the thing an operator reviewed.
    let spec = McpLaunchSpec {
        env: vec![("HOME=/etc".to_owned(), "x".to_owned())],
        ..base_spec()
    };
    assert_eq!(spec.validate(), Err(McpProcessError::EnvironmentInvalid));

    let empty_key = McpLaunchSpec {
        env: vec![(String::new(), "x".to_owned())],
        ..base_spec()
    };
    assert_eq!(
        empty_key.validate(),
        Err(McpProcessError::EnvironmentInvalid)
    );

    let too_many = McpLaunchSpec {
        env: vec![("K".to_owned(), "v".to_owned()); MAX_MCP_ENV_VARS + 1],
        ..base_spec()
    };
    assert_eq!(
        too_many.validate(),
        Err(McpProcessError::EnvironmentInvalid)
    );
}

#[test]
fn an_empty_environment_value_is_allowed_while_an_over_bound_one_is_not() {
    // Empty is a real configuration — a variable deliberately set to nothing, which is distinguishable
    // from an absent one — so refusing it would make a legitimate declaration unexpressible. This is
    // the control for the validity direction of the two environment rules.
    let deliberately_empty = McpLaunchSpec {
        env: vec![("TOKEN".to_owned(), String::new())],
        ..base_spec()
    };
    assert_eq!(deliberately_empty.validate(), Ok(()));

    let over_bound = McpLaunchSpec {
        env: vec![("TOKEN".to_owned(), "x".repeat(MAX_MCP_ENV_VALUE_BYTES + 1))],
        ..base_spec()
    };
    assert_eq!(
        over_bound.validate(),
        Err(McpProcessError::EnvironmentInvalid)
    );
}

#[test]
fn a_startup_timeout_outside_its_bounds_is_refused_rather_than_clamped() {
    // An unbounded startup timeout is a hang with no clock, and clamping a zero would turn "never
    // started" into "started immediately" — a different configuration than the one written.
    for timeout in [0, 60_001] {
        let spec = McpLaunchSpec {
            startup_timeout_ms: timeout,
            ..base_spec()
        };
        match spec.validate() {
            Err(McpProcessError::StartupTimeoutOutOfRange { timeout_ms, .. }) => {
                assert_eq!(timeout_ms, timeout);
            }
            other => panic!("{timeout} must be out of range, got {other:?}"),
        }
    }
    let at_the_bound = McpLaunchSpec {
        startup_timeout_ms: 60_000,
        ..base_spec()
    };
    assert_eq!(at_the_bound.validate(), Ok(()));
}

#[test]
fn every_process_error_code_is_namespaced_and_distinct() {
    let errors = [
        McpProcessError::ProgramInvalid,
        McpProcessError::ArgumentInvalid,
        McpProcessError::WorkingDirectoryInvalid,
        McpProcessError::EnvironmentInvalid,
        McpProcessError::StartupTimeoutOutOfRange {
            timeout_ms: 0,
            min: 1,
            max: 2,
        },
        McpProcessError::SpawnFailed,
    ];
    let codes: Vec<&str> = errors.iter().map(McpProcessError::code).collect();

    assert!(
        codes.iter().all(|code| code.starts_with("mcp.")),
        "{codes:?}"
    );
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "codes must be distinct: {codes:?}"
    );
}

#[test]
fn a_token_longer_than_the_bound_is_refused() {
    let spec = McpLaunchSpec {
        args: vec!["x".repeat(MAX_MCP_TOKEN_BYTES + 1)],
        ..base_spec()
    };
    assert_eq!(spec.validate(), Err(McpProcessError::ArgumentInvalid));
}

#[test]
fn the_diagnostic_tail_keeps_the_last_bytes_not_the_first() {
    // The direction that makes diagnostics useful. A server that dies mid-session wrote the reason
    // immediately before it died; its startup banner explains nothing about the failure, so a
    // first-bytes buffer would keep exactly the part that does not help.
    let mut tail = StderrTail::default();
    tail.push(b"banner that explains nothing");
    let filler = "f".repeat(MAX_MCP_STDERR_BYTES);
    tail.push(filler.as_bytes());
    tail.push(b"reason it died");

    let text = tail.text();
    assert_eq!(tail.bytes.len(), MAX_MCP_STDERR_BYTES, "bounded");
    assert!(
        text.ends_with("reason it died"),
        "the most recent bytes must be retained"
    );
    assert!(
        !text.contains("banner"),
        "the oldest bytes must be discarded"
    );
}

#[test]
fn an_over_long_write_still_yields_a_bounded_tail() {
    // A single write larger than the whole bound, which is the case a chunk-at-a-time append has to
    // handle without growing past the limit.
    let mut tail = StderrTail::default();
    tail.push(&vec![b'x'; MAX_MCP_STDERR_BYTES * 3]);

    assert_eq!(tail.bytes.len(), MAX_MCP_STDERR_BYTES);
}

#[test]
fn a_working_directory_that_is_not_utf8_is_refused_rather_than_replaced() {
    // A lossy conversion would hand the process table a *different* directory than the one reviewed.
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;
        let spec = McpLaunchSpec {
            working_dir: Some(PathBuf::from(OsStr::from_bytes(b"/tmp/\xff"))),
            ..base_spec()
        };
        assert_eq!(
            spec.validate(),
            Err(McpProcessError::WorkingDirectoryInvalid)
        );
    }
}

#[test]
fn spawning_a_program_that_does_not_exist_is_a_named_error_rather_than_a_panic() {
    // The only test here that launches a process, and it launches one that **cannot** start — so it is
    // deterministic and creates nothing. Its job is to prove the failure path is a typed error and not
    // an unwrap, and that no half-built transport escapes.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    let spec = McpLaunchSpec {
        program: PathBuf::from("/nonexistent/jarvis/mcp-server-that-cannot-exist"),
        ..base_spec()
    };

    let result = runtime.block_on(async { spawn_stdio_server(&spec) });
    match result {
        Err(McpProcessError::SpawnFailed) => {}
        other => panic!("a missing program must be SpawnFailed, got {other:?}"),
    }
}

#[test]
fn stderr_is_captured_into_the_bounded_tail_rather_than_inherited() {
    // The one place this module deviates from the SDK's own default, and the default is what a careless
    // builder keeps: `TokioChildProcessBuilder::new` sets `stderr: Stdio::inherit()`, which would spray
    // an untrusted server's output — unbounded and unredacted — across the daemon's own stderr.
    //
    // Asserted end to end rather than by inspecting a builder field, because the property is "the bytes
    // the child wrote are the bytes we retained". The child is given an **absolute** interpreter path
    // deliberately: `env_clear` removes `PATH`, so a bare `cmd` would not resolve, and this test would
    // then be measuring path lookup instead of stderr capture.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a runtime");
    let spec = McpLaunchSpec {
        program: shell_program(),
        args: shell_args("echo mcp-stderr-probe 1>&2"),
        // No environment, so nothing about this test depends on the daemon's own.
        env: Vec::new(),
        working_dir: None,
        startup_timeout_ms: DEFAULT_MCP_STARTUP_TIMEOUT_MS,
    };

    let process = runtime
        .block_on(async { spawn_stdio_server(&spec) })
        .expect("the shell must start");

    // Polled inside `block_on` because the draining task only progresses while the runtime is driven,
    // and bounded so a failure is a failed assertion rather than a hang.
    let captured = runtime.block_on(async {
        for _ in 0..100 {
            let text = process.diagnostics();
            if text.contains("mcp-stderr-probe") {
                return text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        process.diagnostics()
    });
    let debugged = format!("{process:?}");
    // **Dropped inside the runtime, and that is a requirement rather than tidiness.** `rmcp` kills its
    // child from its `Drop`, and `kill()` awaits a reap — so dropping a transport outside a runtime
    // panics *inside the SDK* with "there is no reactor running" (found exactly here, at
    // `transport/child_process.rs:50`). A daemon is async so it drops servers in-runtime anyway, but a
    // synchronous shutdown path has to enter one deliberately.
    runtime.block_on(async move {
        drop(process);
    });

    assert!(
        captured.contains("mcp-stderr-probe"),
        "the child's stderr must be captured into the tail, got {captured:?}"
    );
    // And the value's `Debug` must not leak it: untrusted text must not reach a log line by accident.
    assert!(!debugged.contains("mcp-stderr-probe"), "{debugged}");
}

/// The absolute path of a shell interpreter for this platform.
///
/// Absolute on purpose: see the test above. `COMSPEC` is read from the *daemon's* environment at test
/// time, which is where resolution belongs.
fn shell_program() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("COMSPEC").map_or_else(
            || PathBuf::from(r"C:\Windows\System32\cmd.exe"),
            PathBuf::from,
        )
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/bin/sh")
    }
}

/// The argv for this platform's shell, as **separate entries**.
///
/// Separate rather than composed on purpose: the script is interpreted by the shell because it is an
/// argument *to* the shell, which is exactly the property `service::exec` means by "never through a
/// shell, so no argument can be reinterpreted as a shell construct" — here the shell is the program,
/// so the script must be its own entry rather than concatenated into one.
fn shell_args(intent: &str) -> Vec<String> {
    let flag = if cfg!(windows) { "/C" } else { "-c" };
    let script = match (cfg!(windows), intent) {
        // `set` with no argument prints the environment; redirected to stderr because that is the
        // stream this module captures.
        (true, "print environment") => "set 1>&2".to_owned(),
        (false, "print environment") => "env 1>&2".to_owned(),
        // The same on both platforms, so one arm covers them rather than two identical ones.
        (_, "echo mcp-stderr-probe 1>&2") => "echo mcp-stderr-probe 1>&2".to_owned(),
        (_, other) => other.to_owned(),
    };
    vec![flag.to_owned(), script]
}
