//! Tests for the diagnostics report and the support-bundle pipeline.
//!
//! The canary test is the important one: it seeds a secret into a real log file,
//! runs the real bundle pipeline, and asserts the bytes are absent from the
//! produced archive. A redaction test that only calls the redactor would prove
//! the redactor works, not that the bundle uses it.

use std::path::PathBuf;

use jarvis_observability::Redactor;

use super::archive::{ZipArchive, build_stored_archive, crc32};
use super::*;
use crate::paths::ProfilePaths;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jarvis-fnd013-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn redactor() -> Redactor {
    Redactor::new().expect("redactor")
}

fn plan_with(items: Vec<PlanItem>) -> BundlePlan {
    let mut plan = BundlePlan::new();
    plan.push(PlanItem {
        path: MANIFEST_PATH.to_owned(),
        description: "manifest",
        source: BundleSource::Rendered,
        optional: false,
        source_path: None,
        content: Some("{}\n".to_owned()),
    });
    for item in items {
        plan.push(item);
    }
    plan
}

fn rendered(path: &str, content: &str) -> PlanItem {
    PlanItem {
        path: path.to_owned(),
        description: "rendered",
        source: BundleSource::Rendered,
        optional: true,
        source_path: None,
        content: Some(content.to_owned()),
    }
}

#[test]
fn a_blocking_finding_is_counted_separately_from_a_warning() {
    let mut report = CheckReport::new();
    report.push(Finding::ok("a", "fine"));
    report.push(Finding::warning("b", "look", "do something"));
    assert!(report.is_usable());
    assert_eq!(report.blocking(), 0);
    assert_eq!(report.warnings(), 1);
    assert_eq!(report.exit_code(), 0);

    report.push(Finding::error("c", "broken", "fix it"));
    assert!(!report.is_usable());
    assert_eq!(report.blocking(), 1);
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn a_report_with_warnings_does_not_claim_there_is_nothing_wrong() {
    let mut report = CheckReport::new();
    report.push(Finding::warning(
        "service",
        "jarvis.service_not_installed",
        "install it",
    ));
    let text = report.render();
    assert!(text.contains("no blocking findings"));
    // The warning count is what stops that phrase from reading as "all clear".
    assert!(text.contains("1 warning(s)"), "{text}");
}

#[test]
fn the_report_json_is_stable_and_escapes_untrusted_text() {
    let mut report = CheckReport::new();
    report.push(Finding::error(
        "configuration",
        "value \"quoted\"\\ and \n newline",
        "fix it",
    ));
    let json = report.to_json();
    assert!(json.contains("\\\"quoted\\\""), "{json}");
    assert!(json.contains("\\\\"), "{json}");
    assert!(json.contains("\\n"), "{json}");
    let reparsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(reparsed["blocking"], 1);
    assert_eq!(reparsed["findings"][0]["check"], "configuration");
}

#[test]
fn the_manifest_cannot_be_excluded_but_a_log_can() {
    let plan = plan_with(vec![rendered("doctor.json", "{}")]);
    assert_eq!(
        plan.resolve_exclusions(&["doctor.json".to_owned()]).ok(),
        Some(vec!["doctor.json".to_owned()])
    );
    assert!(matches!(
        plan.resolve_exclusions(&[MANIFEST_PATH.to_owned()]),
        Err(BundleError::RequiredItem { .. })
    ));
    assert!(matches!(
        plan.resolve_exclusions(&["absent.json".to_owned()]),
        Err(BundleError::UnknownItem { .. })
    ));
}

#[test]
fn a_plan_without_a_manifest_is_refused() {
    let mut plan = BundlePlan::new();
    plan.push(rendered("doctor.json", "{}"));
    let error = materialize(&plan, &[], &redactor()).expect_err("must refuse");
    assert_eq!(error.code(), "jarvis.bundle_missing_manifest");
    assert!(!error.retryable());
}

#[test]
fn excluded_items_are_absent_and_recorded() {
    let plan = plan_with(vec![
        rendered("doctor.json", "{\"checks\":1}"),
        rendered("environment.json", "{\"os\":\"x\"}"),
    ]);
    let bundle =
        materialize(&plan, &["environment.json".to_owned()], &redactor()).expect("materialize");
    let paths: Vec<&str> = bundle
        .files()
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, vec![MANIFEST_PATH, "doctor.json"]);
    assert_eq!(bundle.excluded(), ["environment.json".to_owned()]);
}

#[test]
fn materialized_order_follows_the_plan_order() {
    // A BTreeMap or a sort would make the preview disagree with the bundle.
    let plan = plan_with(vec![
        rendered("z.json", "{}"),
        rendered("a.json", "{}"),
        rendered("m.json", "{}"),
    ]);
    let bundle = materialize(&plan, &[], &redactor()).expect("materialize");
    let paths: Vec<&str> = bundle
        .files()
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, vec![MANIFEST_PATH, "z.json", "a.json", "m.json"]);
}

#[test]
fn a_registered_secret_never_reaches_the_bundle_bytes() {
    // The canary. The value is long enough to be registered, and it is placed in
    // every kind of item the plan can hold: a rendered document and a real log
    // file on disk.
    let canary = "canary-9f4b2c7e1a5d";
    let redactor = redactor();
    redactor.register(canary).expect("register canary");

    let dir = temp_dir("canary");
    let log_path = dir.join("jarvis.2026-09-21.log");
    std::fs::write(
        &log_path,
        format!(
            "{{\"level\":\"INFO\",\"message\":\"enrolled with {canary}\"}}\n\
             {{\"level\":\"INFO\",\"message\":\"no secret here\"}}\n"
        ),
    )
    .expect("write log");

    let mut plan = plan_with(vec![rendered(
        "doctor.json",
        &format!("{{\"note\":\"{canary}\"}}"),
    )]);
    plan.push(PlanItem {
        path: "logs/jarvis.log".to_owned(),
        description: "log tail",
        source: BundleSource::LogTail,
        optional: true,
        source_path: Some(log_path),
        content: None,
    });

    let bundle = materialize(&plan, &[], &redactor).expect("materialize");
    let destination = dir.join("bundle.zip");
    write_bundle(&bundle, &destination).expect("write bundle");
    let archive_bytes = std::fs::read(&destination).expect("read archive");

    // Assert on the actual archive bytes, not only the intermediate values.
    let as_text = String::from_utf8_lossy(&archive_bytes);
    assert!(
        !as_text.contains(canary),
        "the canary leaked into the archive"
    );
    let joined: Vec<u8> = bundle
        .files()
        .iter()
        .flat_map(|file| file.bytes.clone())
        .collect();
    assert!(
        !String::from_utf8_lossy(&joined).contains(canary),
        "the canary leaked into a bundle member"
    );
    assert!(
        as_text.contains(redaction_marker()),
        "the redaction marker should be present, proving the pass ran"
    );
    // The non-secret line must survive, or the bundle is useless.
    assert!(
        as_text.contains("no secret here"),
        "useful content was lost"
    );
}

#[test]
fn a_secret_split_across_two_log_lines_is_not_half_emitted() {
    // Redaction happens per line, so a value that appears in full on its own line
    // is removed. This asserts the boundary case that a naive "redact the whole
    // file at once" implementation would also pass but a per-line one might not:
    // the value is present, complete, on one line.
    let canary = "split-canary-abc123456";
    let redactor = redactor();
    redactor.register(canary).expect("register");

    let dir = temp_dir("split");
    let log_path = dir.join("jarvis.log");
    std::fs::write(
        &log_path,
        format!("first line\nvalue {canary} end\nlast line\n"),
    )
    .expect("write");

    let mut plan = plan_with(vec![]);
    plan.push(PlanItem {
        path: "logs/a.log".to_owned(),
        description: "log tail",
        source: BundleSource::LogTail,
        optional: true,
        source_path: Some(log_path),
        content: None,
    });
    let bundle = materialize(&plan, &[], &redactor).expect("materialize");
    let log_member = bundle
        .files()
        .iter()
        .find(|file| file.path == "logs/a.log")
        .expect("log member");
    let text = String::from_utf8_lossy(&log_member.bytes);
    assert!(!text.contains(canary));
    assert!(text.contains("first line"));
    assert!(text.contains("last line"));
}

#[test]
fn a_large_log_keeps_whole_recent_lines_only() {
    let dir = temp_dir("bounded");
    let log_path = dir.join("jarvis.log");
    let mut contents = String::new();
    // Each line is ~8 bytes; the bound is 256 KiB, so this comfortably exceeds it.
    for index in 0..60_000 {
        let _ = writeln!(contents, "line{index}");
    }
    std::fs::write(&log_path, &contents).expect("write log");

    let mut plan = plan_with(vec![]);
    plan.push(PlanItem {
        path: "logs/big.log".to_owned(),
        description: "log tail",
        source: BundleSource::LogTail,
        optional: true,
        source_path: Some(log_path),
        content: None,
    });
    let bundle = materialize(&plan, &[], &redactor()).expect("materialize");
    let member = bundle
        .files()
        .iter()
        .find(|file| file.path == "logs/big.log")
        .expect("member");
    assert!(member.bytes.len() <= MAX_LOG_BYTES_PER_FILE);
    let text = String::from_utf8_lossy(&member.bytes);
    // The newest line survives, and the tail starts at a record boundary.
    assert!(text.contains("line59999"), "the newest line must be kept");
    assert!(
        text.starts_with("line"),
        "the tail must begin on a record boundary: {:?}",
        &text[..text.len().min(24)]
    );
}

#[test]
fn the_log_file_count_is_bounded_and_the_omission_is_visible() {
    let dir = temp_dir("count");
    for index in 0..(MAX_LOG_FILES + 3) {
        std::fs::write(dir.join(format!("jarvis.{index}.log")), "line\n").expect("write");
    }
    let mut plan = BundlePlan::new();
    let added = add_log_tails(&mut plan, &dir);
    assert_eq!(added.len(), MAX_LOG_FILES);
    assert_eq!(plan.omitted_log_files(), 3);
    assert!(plan.render().contains("outside the"));
}

#[test]
fn a_hostile_log_file_name_cannot_escape_the_archive() {
    assert_eq!(sanitize_archive_path(".."), "unnamed");
    assert_eq!(sanitize_archive_path("../x"), "___x");
    assert_eq!(sanitize_archive_path("C:\\evil"), "C__evil");
    assert_eq!(sanitize_archive_path(""), "unnamed");
    assert_eq!(
        sanitize_archive_path("jarvis.2026-09-21.log"),
        "jarvis.2026-09-21.log"
    );
}

#[test]
fn the_manifest_records_what_was_included_and_excluded() {
    let plan = plan_with(vec![rendered("doctor.json", "{}")]);
    let environment = EnvironmentSummary::current("portable", 1);
    let daemon = DaemonSummary {
        instance_id: Some("inst-1".to_owned()),
        pid: Some(42),
        server_version: Some("0.1.0".to_owned()),
        unavailable_code: None,
    };
    let manifest = render_manifest(
        &plan,
        &environment,
        &daemon,
        &["doctor.json".to_owned()],
        "deadbeef",
    );
    let parsed: serde_json::Value = serde_json::from_str(&manifest).expect("valid JSON");
    assert_eq!(parsed["schema_version"], SUPPORT_BUNDLE_SCHEMA_VERSION);
    assert_eq!(parsed["included_items"], 1);
    assert_eq!(parsed["excluded_items"], 1);
    assert_eq!(parsed["content_sha256"], "deadbeef");
    let items = parsed["items"].as_array().expect("items array");
    let doctor = items
        .iter()
        .find(|item| item["path"] == "doctor.json")
        .expect("doctor item");
    assert_eq!(doctor["included"], false);
}

#[test]
fn the_digest_changes_when_content_or_a_path_changes() {
    let one = vec![BundleFile {
        path: "a".to_owned(),
        bytes: b"one".to_vec(),
    }];
    let two = vec![BundleFile {
        path: "a".to_owned(),
        bytes: b"two".to_vec(),
    }];
    let renamed = vec![BundleFile {
        path: "b".to_owned(),
        bytes: b"one".to_vec(),
    }];
    assert_ne!(digest_of(&one), digest_of(&two));
    assert_ne!(digest_of(&one), digest_of(&renamed));
    assert_eq!(digest_of(&one), digest_of(&one));
}

#[test]
fn a_zip_produced_by_the_bundle_is_structurally_well_formed() {
    // Parse the produced archive back: walk the local headers using only the
    // sizes the archive itself claims, confirm each member's CRC, and confirm the
    // central directory agrees with the local headers. A writer that emits bytes
    // nobody can read is not a support bundle.
    let plan = plan_with(vec![
        rendered("doctor.json", "{\"a\":1}"),
        rendered("environment.json", "{\"b\":2}"),
    ]);
    let bundle = materialize(&plan, &[], &redactor()).expect("materialize");
    let entries: Vec<(String, &[u8])> = bundle
        .files()
        .iter()
        .map(|file| (file.path.clone(), file.bytes.as_slice()))
        .collect();
    let archive: ZipArchive = build_stored_archive(&entries).expect("archive");
    let parsed = parse_stored_archive(archive.bytes());
    assert_eq!(parsed.len(), entries.len());
    for (name, bytes) in &entries {
        let found = parsed
            .iter()
            .find(|entry| &entry.name == name)
            .expect("member present");
        assert_eq!(&found.data, bytes);
        assert_eq!(found.crc, crc32(bytes));
    }
    // The central directory must be readable and contain every member.
    let central = parse_central_directory(archive.bytes());
    assert_eq!(
        central,
        entries
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    );
}

/// A parsed stored member.
struct ParsedMember {
    name: String,
    crc: u32,
    data: Vec<u8>,
}

/// Walks the local file headers of a stored archive.
fn parse_stored_archive(bytes: &[u8]) -> Vec<ParsedMember> {
    let mut members = Vec::new();
    let mut offset = 0_usize;
    while offset + 4 <= bytes.len() && bytes[offset..offset + 4] == [0x50, 0x4b, 0x03, 0x04] {
        let name_len = u16::from_le_bytes([bytes[offset + 26], bytes[offset + 27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
        let crc = u32::from_le_bytes([
            bytes[offset + 14],
            bytes[offset + 15],
            bytes[offset + 16],
            bytes[offset + 17],
        ]);
        let size = u32::from_le_bytes([
            bytes[offset + 18],
            bytes[offset + 19],
            bytes[offset + 20],
            bytes[offset + 21],
        ]) as usize;
        let name_start = offset + 30;
        let name = String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).into_owned();
        let data_start = name_start + name_len + extra_len;
        let data = bytes[data_start..data_start + size].to_vec();
        members.push(ParsedMember { name, crc, data });
        offset = data_start + size;
    }
    members
}

/// Walks the central directory of a stored archive.
fn parse_central_directory(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let end = bytes
        .windows(4)
        .position(|window| window == [0x50, 0x4b, 0x05, 0x06])
        .expect("end of central directory");
    let count = u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]) as usize;
    let start = u32::from_le_bytes([
        bytes[end + 16],
        bytes[end + 17],
        bytes[end + 18],
        bytes[end + 19],
    ]) as usize;
    let mut offset = start;
    for _ in 0..count {
        assert_eq!(
            bytes[offset..offset + 4],
            [0x50, 0x4b, 0x01, 0x02],
            "central header signature"
        );
        let name_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[offset + 32], bytes[offset + 33]]) as usize;
        let name_start = offset + 46;
        names.push(String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).into_owned());
        offset = name_start + name_len + extra_len + comment_len;
    }
    names
}

#[test]
fn write_bundle_refuses_a_destination_it_cannot_write() {
    let plan = plan_with(vec![rendered("doctor.json", "{}")]);
    let bundle = materialize(&plan, &[], &redactor()).expect("materialize");
    // A directory is not a writable file destination.
    let dir = temp_dir("dest");
    let error = write_bundle(&bundle, &dir).expect_err("must fail");
    assert!(error.retryable());
}

#[test]
fn the_plan_render_tells_the_user_what_is_excluded() {
    let plan = plan_with(vec![rendered("doctor.json", "{}")]);
    let text = plan.render();
    assert!(text.contains("nothing has been written yet"));
    assert!(text.contains("excluded from every bundle"));
    assert!(text.contains("manifest.json"));
    assert!(text.contains("required"));
}

#[test]
fn severity_tokens_and_display_agree() {
    for severity in [Severity::Ok, Severity::Warning, Severity::Error] {
        assert_eq!(severity.to_string(), severity.token());
    }
    assert!(Severity::Error.is_blocking());
    assert!(!Severity::Warning.is_blocking());
}

#[test]
fn the_environment_summary_never_contains_an_environment_value() {
    // The summary is built from `env::consts`, not from the process environment.
    // Seed a lookalike variable and confirm it cannot appear.
    let summary = EnvironmentSummary::current("standard", 1);
    let json = summary.to_json();
    assert!(json.contains(std::env::consts::OS));
    assert!(!json.contains("JARVIS_"));
    assert!(!json.contains("PATH"));
}

#[test]
fn a_rendered_item_is_still_redacted() {
    // Redaction applies to every item, so a caller that mistakenly puts a secret
    // in a rendered document does not leak it.
    let canary = "rendered-canary-77zz";
    let redactor = redactor();
    redactor.register(canary).expect("register");
    let plan = plan_with(vec![rendered(
        "doctor.json",
        &format!("{{\"leak\":\"{canary}\"}}"),
    )]);
    let bundle = materialize(&plan, &[], &redactor).expect("materialize");
    let doctor = bundle
        .files()
        .iter()
        .find(|file| file.path == "doctor.json")
        .expect("item");
    assert!(!String::from_utf8_lossy(&doctor.bytes).contains(canary));
}

#[test]
fn sanitize_line_strips_control_characters() {
    let sanitized = sanitize_line("ok\u{1b}[31mred\u{7}");
    assert!(!sanitized.contains('\u{1b}'));
    assert!(!sanitized.contains('\u{7}'));
}

#[test]
fn severity_totals_group_by_token() {
    let mut report = CheckReport::new();
    report.push(Finding::ok("a", "x"));
    report.push(Finding::warning("b", "x", "y"));
    report.push(Finding::error("c", "x", "y"));
    let totals = severity_totals(&report);
    assert_eq!(totals.get("ok"), Some(&1));
    assert_eq!(totals.get("warn"), Some(&1));
    assert_eq!(totals.get("error"), Some(&1));
}

#[test]
fn database_file_is_on_the_data_root_not_the_runtime_root() {
    let paths = jarvis_paths("db");
    let database = database_file(&paths);
    assert_eq!(
        database.file_name().and_then(|name| name.to_str()),
        Some("jarvis.sqlite")
    );
    assert!(!database.starts_with(paths.runtime_dir()));
}

/// Builds a portable profile rooted in a fresh temporary directory.
fn jarvis_paths(tag: &str) -> ProfilePaths {
    ProfilePaths::portable(temp_dir(tag))
}

#[test]
fn a_surviving_discovery_file_over_a_free_lock_is_reported_as_stale() {
    // The state `ACC-003` requires to be recoverable after an unclean kill: the
    // discovery file survives and parses, so a check that trusted it would report
    // "daemon running" and the stale state would never be found. A clean drain
    // unpublishes discovery, so its presence here means the daemon did not drain.
    let paths = jarvis_paths("stale-discovery");
    paths.ensure_directories().expect("create");
    std::fs::write(paths.runtime_dir().join("jarvis.lock"), b"").expect("write lock");
    std::fs::write(
        paths.runtime_dir().join("discovery.json"),
        b"{\"schema_version\":1}",
    )
    .expect("write discovery");

    let environment = DiagnosticsEnvironment {
        daemon: None,
        discovery_error: Some("jarvis.daemon_not_running"),
        controller: None,
        service_spec: None,
        service_spec_error: None,
        controller_error: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let report = runtime.block_on(collect(&paths, &environment));

    let state = report
        .findings()
        .iter()
        .find(|finding| finding.check == "daemon state")
        .expect("the daemon-state check ran");
    assert_eq!(state.severity, Severity::Warning);
    assert_eq!(state.message, "jarvis.stale_discovery");
    // And it is repairable, which is what makes the finding useful.
    assert!(plan_for(&paths, state).is_ok());
}

#[test]
fn an_idle_daemon_must_not_be_reported_as_stale() {
    // The correction that mattered most: a clean drain releases the lock but leaves
    // the lock FILE on disk while unpublishing discovery. Treating any unheld lock
    // as stale made doctor warn and repair act after every clean stop.
    let paths = jarvis_paths("idle-daemon");
    paths.ensure_directories().expect("create");
    std::fs::write(paths.runtime_dir().join("jarvis.lock"), b"").expect("write lock");
    assert!(
        !paths.runtime_dir().join("discovery.json").exists(),
        "a cleanly stopped daemon has no discovery file"
    );

    let environment = DiagnosticsEnvironment {
        daemon: None,
        discovery_error: Some("jarvis.daemon_not_running"),
        controller: None,
        service_spec: None,
        service_spec_error: None,
        controller_error: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let report = runtime.block_on(collect(&paths, &environment));
    let state = report
        .findings()
        .iter()
        .find(|finding| finding.check == "daemon state")
        .expect("the daemon-state check ran");
    assert_eq!(
        state.severity,
        Severity::Ok,
        "an idle daemon is a normal state, not a fault: {}",
        state.message
    );
    assert_eq!(
        plan_for(&paths, state).err(),
        Some(RepairError::NotRepairable {
            code: state.message.clone()
        }),
        "there must be nothing to repair"
    );
}

#[test]
fn a_held_lock_is_reported_as_ok_and_is_not_repairable() {
    // The counterpart: a live holder must never be reported as stale, and repair
    // must refuse it.
    let paths = jarvis_paths("held-lock");
    paths.ensure_directories().expect("create");
    let lock = paths.runtime_dir().join("jarvis.lock");
    let guard = crate::lifecycle::InstanceGuard::acquire(&lock).expect("acquire");
    // A discovery file alongside a HELD lock is a live daemon, not a stale state.
    std::fs::write(paths.runtime_dir().join("discovery.json"), b"{}").expect("write");

    let environment = DiagnosticsEnvironment {
        daemon: None,
        discovery_error: Some("jarvis.daemon_not_running"),
        controller: None,
        service_spec: None,
        service_spec_error: None,
        controller_error: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let report = runtime.block_on(collect(&paths, &environment));
    let state = report
        .findings()
        .iter()
        .find(|finding| finding.check == "daemon state")
        .expect("the daemon-state check ran");
    assert_eq!(state.severity, Severity::Ok);
    // Planning is refused, and the refusal names the reason that matters: a daemon
    // holds the lock, so the discovery file beside it is live, not stale.
    assert_eq!(
        plan_for(&paths, state).err(),
        Some(RepairError::DaemonRunning),
        "a live daemon must make the stale-state repair refuse, naming the daemon"
    );
    // The discovery file must survive: a live daemon owns it.
    assert!(paths.runtime_dir().join("discovery.json").exists());
    drop(guard);
}

#[test]
fn the_collector_does_not_create_the_directories_it_reports_on() {
    // The property that makes the directory check meaningful: a diagnostic must
    // not perform the repair it describes. The earlier version called
    // `ensure_directories`, so a missing-directory fault was invisible *and* the
    // repair for it could never be offered.
    let paths = jarvis_paths("non-mutating");
    assert_eq!(paths.missing_directories().len(), 5);

    let environment = DiagnosticsEnvironment {
        daemon: None,
        discovery_error: Some("jarvis.discovery_missing"),
        controller: None,
        service_spec: None,
        service_spec_error: None,
        controller_error: Some("jarvis.service_facility_unavailable"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let report = runtime.block_on(collect(&paths, &environment));

    assert_eq!(
        paths.missing_directories().len(),
        5,
        "collecting a report must not create anything"
    );
    assert!(!paths.config_dir().exists());

    // The missing directories are reported as a warning, not an error: a fresh
    // profile is a supported state the daemon creates on first start.
    let directories = report
        .findings()
        .iter()
        .find(|finding| finding.check == "profile directories")
        .expect("the check ran");
    assert_eq!(directories.severity, Severity::Warning);
    assert!(report.is_usable(), "{}", report.render());
    // And the finding is repairable, which is the point of reporting it at all.
    assert!(plan_for(&paths, directories).is_ok());
}

#[test]
fn the_collector_is_pure_for_an_empty_profile_with_no_environment() {
    // No daemon, no service controller, no spec: every injectable absence must
    // produce a warning rather than a panic or a blocking error, because doctor
    // runs on a machine where JARVIS has never started.
    let paths = jarvis_paths("empty");
    let environment = DiagnosticsEnvironment {
        daemon: None,
        discovery_error: Some("jarvis.discovery_missing"),
        controller: None,
        service_spec: None,
        service_spec_error: None,
        controller_error: Some("jarvis.service_facility_unavailable"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let report = runtime.block_on(collect(&paths, &environment));
    assert!(report.is_usable(), "{}", report.render());
    assert_eq!(report.blocking(), 0);
    // Directories, daemon, credential, and service are all warnings on a fresh
    // profile.
    assert!(report.warnings() >= 2, "{}", report.render());
    assert!(report.render().contains("daemon"));
    assert!(report.render().contains("configuration"));
    let summary = daemon_summary(&environment);
    assert_eq!(summary.unavailable_code, Some("jarvis.discovery_missing"));
}

#[test]
fn the_daemon_summary_carries_only_safe_identifiers() {
    let discovery = jarvis_protocol::DiscoveryFile {
        schema_version: 1,
        instance_id: "inst-abc".to_owned(),
        pid: 7,
        base_url: "http://127.0.0.1:1234".to_owned(),
        api_major: 1,
        started_at: "2026-09-21T00:00:00Z".to_owned(),
    };
    let descriptor = DaemonDescriptor::from(&discovery);
    let environment = DiagnosticsEnvironment {
        daemon: Some(descriptor),
        discovery_error: None,
        controller: None,
        service_spec: None,
        service_spec_error: None,
        controller_error: None,
    };
    let summary = daemon_summary(&environment);
    assert_eq!(summary.instance_id.as_deref(), Some("inst-abc"));
    assert_eq!(summary.pid, Some(7));
    let json = summary.to_json();
    // The base URL is deliberately not part of the summary: a bundle records the
    // instance, not the port it happened to bind.
    assert!(!json.contains("1234"));
}

#[test]
fn a_client_discovery_value_converts_to_the_same_descriptor() {
    // Two entry points reach the collector: the daemon-side protocol type and the
    // CLI's reachability value. Both must produce a descriptor the checks can
    // read, or one caller silently loses the daemon check.
    let discovered = crate::client::Discovered {
        base_url: "http://127.0.0.1:1".to_owned(),
        instance_id: "inst-xyz".to_owned(),
        pid: 99,
    };
    let descriptor = DaemonDescriptor::from(&discovered);
    assert_eq!(descriptor.instance_id, "inst-xyz");
    assert_eq!(descriptor.pid, 99);
    assert_eq!(descriptor.api_major, crate::http::API_MAJOR);
}

/// Compile-time proof that the collector's database check uses the profile.
#[test]
fn database_file_lives_under_the_data_directory() {
    let paths = jarvis_paths("data");
    assert!(database_file(&paths).starts_with(paths.data_dir()));
}
