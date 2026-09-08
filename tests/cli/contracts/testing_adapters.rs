use std::{
    collections::BTreeMap,
    env,
    error::Error as StdError,
    ffi::OsString,
    fs, panic,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;
use termtrace_media::save::verify_saved_artifact;
use termtrace_model::{
    artifact::save::SaveManifest,
    terminal::screen::{ScreenFrameData, ScreenProjectionRequest},
};
use termtrace_sdk::{
    assertions::SnapshotProfile,
    control_endpoint_is_reachable,
    locator::{LocatorSpec, StyleFilter},
    testing::{
        run_terminal_test, EvidencePolicy, TerminalTest, TerminalTestConfig, TestFailure,
        TestOutcome,
    },
    ScreenFrameQuery, StableCaptureOptions, Terminal,
};

fn looping_command() -> Vec<OsString> {
    return [
        "/bin/sh",
        "-c",
        "trap 'exit 0' TERM; printf READY; while :; do sleep .1; done",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
}

#[test]
fn rust_test_helper_discards_styled_stable_attempt_after_snapshot() {
    let evidence = TempDir::new().expect("temporary evidence directory");
    let snapshots = TempDir::new().expect("temporary snapshot directory");
    let command = [
        "/bin/sh",
        "-c",
        "trap 'exit 0' TERM; printf '\\033[?2026h\\033[44mREADY   X\\033[0m\\033[?2026l'; while :; do sleep .1; done",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<OsString>>();

    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), evidence.path());
    let mut config = TerminalTestConfig::new(terminal, command);
    config.columns = 8;
    config.rows = 3;
    config.snapshot_root = snapshots.path().to_path_buf();
    config.update_snapshots = true;

    let result = run_terminal_test(
        config,
        |test| -> Result<(), Box<dyn StdError + Send + Sync>> {
            test.session()
                .expect("open test session")
                .output()
                .wait_for(b"READY", Duration::from_secs(5))?;

            let capture =
                test.screen()
                    .expect("open test screen")
                    .capture_stable(StableCaptureOptions {
                        stable_for: Duration::from_millis(20),
                        timeout: Duration::from_secs(5),
                        projection: ScreenProjectionRequest::default(),
                        allow_incomplete: false,
                    })?;

            let ScreenFrameData::Spans { rows } = &capture.frame.data else {
                panic!("stable frame did not return span detail");
            };
            assert!(rows[0].soft_wrapped);
            assert!(rows[1].wrap_continuation);
            assert_eq!(capture.frame.cursor.row, Some(1));
            assert_eq!(capture.frame.cursor.column, Some(1));
            let resolved_background = rows[0]
                .spans
                .iter()
                .find(|span| return span.text.contains("READY"))
                .expect("styled READY span")
                .appearance
                .resolved_background;

            let query = LocatorSpec::text("READY")?.with_style(StyleFilter {
                resolved_background: Some(resolved_background),
                ..StyleFilter::default()
            });
            assert_eq!(capture.frame.query(query)?.single()?.text.as_ref(), "READY");
            test.assert_snapshot(
                "styled-ready",
                &capture.frame,
                SnapshotProfile::Spans,
                Default::default(),
            )?;

            return Ok(());
        },
    );

    assert!(result.is_ok(), "passing adapter attempt failed: {result:?}");
    assert_eq!(
        fs::read_dir(evidence.path())
            .expect("evidence root")
            .count(),
        0
    );
    assert!(snapshots.path().join("styled-ready.json").is_file());
}

#[test]
fn rust_test_helper_retains_verified_failure_artifacts() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let mut config = TerminalTestConfig::new(terminal, looping_command());
    config.evidence_policy = EvidencePolicy::OnFailure;

    let result = run_terminal_test(config, |test| -> Result<(), &'static str> {
        let session = test.session().expect("open test session");
        session
            .output()
            .wait_for(b"READY", Duration::from_secs(5))
            .map_err(|_| return "READY was not observed")?;

        return Err("PRIMARY_FAILURE_SENTINEL");
    });

    let TestFailure::Body {
        error,
        finalization,
    } = result.expect_err("body failure must remain primary")
    else {
        panic!("body failure was replaced");
    };
    assert_eq!(error, "PRIMARY_FAILURE_SENTINEL");
    assert!(finalization.retained);
    assert!(
        finalization.secondary_failures.is_empty(),
        "failure artifact finalization failed: {:?}",
        finalization.secondary_failures
    );
    let evidence = finalization
        .evidence_directory
        .expect("retained attempt evidence");
    assert!(evidence.join("semantic-screenshot/screen.png").is_file());
    assert!(evidence.join("debug-bundle/manifest.json").is_file());
    assert!(evidence.join("termtrace-test-evidence.json").is_file());
    assert_eq!(finalization.verified_artifacts.len(), 4);
    let screenshot = evidence.join("semantic-screenshot");
    let manifest: SaveManifest = serde_json::from_slice(
        &fs::read(screenshot.join("save-manifest.json")).expect("screenshot manifest"),
    )
    .expect("valid screenshot manifest");

    fs::write(screenshot.join("screen.png"), b"changed").expect("corrupt screenshot");
    assert!(verify_saved_artifact(&screenshot, &manifest).is_err());
}

#[test]
fn rust_test_helper_creates_and_reports_snapshot_mismatches() {
    let temporary = TempDir::new().expect("temporary directory");
    let snapshot_root = temporary.path().join("snapshots");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let mut update_config = TerminalTestConfig::new(terminal, looping_command());
    update_config.snapshot_root = snapshot_root.clone();
    update_config.update_snapshots = true;
    let updated = run_terminal_test(
        update_config,
        |test| -> Result<(), Box<dyn StdError + Send + Sync>> {
            test.session()
                .expect("open test session")
                .output()
                .wait_for(b"READY", Duration::from_secs(5))?;
            let frame = test
                .screen()
                .expect("open test screen")
                .frame(Default::default())?;
            test.assert_snapshot("ready", &frame, SnapshotProfile::Spans, Default::default())?;

            return Ok(());
        },
    );
    assert!(updated.is_ok(), "snapshot update failed: {updated:?}");

    let snapshot_path = snapshot_root.join("ready.json");
    let snapshot = fs::read_to_string(&snapshot_path).expect("reviewed snapshot");
    fs::write(&snapshot_path, snapshot.replace("READY", "STALE"))
        .expect("make reviewed snapshot stale");

    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let mut compare_config = TerminalTestConfig::new(terminal, looping_command());
    compare_config.snapshot_root = snapshot_root;
    let mismatch = run_terminal_test(
        compare_config,
        |test| -> Result<(), Box<dyn StdError + Send + Sync>> {
            test.session()
                .expect("open test session")
                .output()
                .wait_for(b"READY", Duration::from_secs(5))?;
            let frame = test
                .screen()
                .expect("open test screen")
                .frame(Default::default())?;
            test.assert_snapshot("ready", &frame, SnapshotProfile::Spans, Default::default())?;

            return Ok(());
        },
    );

    let TestFailure::Body {
        error,
        finalization,
    } = mismatch.expect_err("stale reviewed snapshot must fail")
    else {
        panic!("snapshot mismatch was replaced");
    };
    assert!(error.to_string().contains("normalized frame differs"));
    assert!(error.to_string().contains("expected:"));
    assert!(finalization.retained);
    assert!(finalization.secondary_failures.is_empty());
}

#[test]
fn rust_test_helper_timeout_retains_evidence_and_reaps_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let pid_path = temporary.path().join("recorded.pid");
    let command = vec![
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(
            "trap 'exit 0' TERM; printf '%s' $$ > \"$1\"; printf READY; while :; do sleep .1; done",
        ),
        OsString::from("timeout-fixture"),
        pid_path.clone().into_os_string(),
    ];

    let mut config = TerminalTestConfig::new(terminal, command);
    config.evidence_policy = EvidencePolicy::Always;
    let mut test = TerminalTest::launch(config).expect("launch timed test");
    test.session()
        .expect("open test session")
        .output()
        .wait_for(b"READY", Duration::from_secs(5))
        .expect("READY output");

    let session_path = test
        .session()
        .expect("open test session")
        .path()
        .to_path_buf();
    assert!(control_endpoint_is_reachable(&session_path));
    let recorded_pid: u32 = fs::read_to_string(&pid_path)
        .expect("recorded process PID")
        .parse()
        .expect("numeric recorded process PID");

    let report = test.finalize(TestOutcome::TimedOut);
    assert!(report.retained);
    assert!(report.secondary_failures.is_empty());
    let canonical = report
        .verified_artifacts
        .iter()
        .find(|path| {
            return path
                .extension()
                .is_some_and(|extension| return extension == "tt");
        })
        .expect("verified canonical evidence");
    assert!(canonical.join("completion.json").is_file());
    assert!(!control_endpoint_is_reachable(&session_path));
    let process_path = PathBuf::from(format!("/proc/{recorded_pid}"));
    let deadline = Instant::now() + Duration::from_secs(2);

    while process_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !process_path.exists(),
        "recorded child survived finalization"
    );
    let evidence = report
        .evidence_directory
        .as_ref()
        .expect("retained timeout evidence");
    assert!(evidence.join("semantic-screenshot/screen.png").is_file());
    assert!(evidence.join("debug-bundle/manifest.json").is_file());
    let inventory = || {
        let mut pending = vec![evidence.clone()];
        let mut files = BTreeMap::new();

        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    fs::read_dir(path)
                        .expect("retained directory")
                        .map(|entry| return entry.expect("retained directory entry").path()),
                );
            } else {
                files.insert(
                    path.clone(),
                    fs::read(path).expect("retained artifact bytes"),
                );
            }
        }

        return files;
    };
    let before = inventory();
    let repeated = test.finalize(TestOutcome::Passed);
    assert_eq!(repeated.outcome, TestOutcome::TimedOut);
    assert_eq!(
        repeated, report,
        "finalization must preserve the first evidence report"
    );
    assert_eq!(
        inventory(),
        before,
        "repeated finalization must not rewrite, duplicate, or delete artifacts"
    );
}

#[test]
fn rust_test_helper_preserves_body_failure_when_cleanup_fails() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let mut config = TerminalTestConfig::new(terminal, looping_command());
    config.finalization_timeout = Duration::from_millis(20);
    let result = run_terminal_test(config, |test| -> Result<(), &'static str> {
        test.session_mut()
            .expect("open test session")
            .force_close()
            .map_err(|_| return "fixture force-close failed")?;

        return Err("PRIMARY_CLEANUP_FAILURE_SENTINEL");
    });

    let TestFailure::Body {
        error,
        finalization,
    } = result.expect_err("body failure must remain primary")
    else {
        panic!("body failure was replaced");
    };
    assert_eq!(error, "PRIMARY_CLEANUP_FAILURE_SENTINEL");
    assert!(finalization.retained);
    assert!(!finalization.secondary_failures.is_empty());
}

#[test]
fn rust_test_helper_resumes_original_panic_after_finalization() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let config = TerminalTestConfig::new(terminal, looping_command());
    let panic_payload = panic::catch_unwind(|| {
        let _result = run_terminal_test(
            config,
            |test| -> Result<(), Box<dyn StdError + Send + Sync>> {
                test.session()
                    .expect("open test session")
                    .output()
                    .wait_for(b"READY", Duration::from_secs(5))?;
                panic!("PRIMARY_PANIC_SENTINEL");
            },
        );
    })
    .expect_err("test body panic must be resumed");
    assert_eq!(
        panic_payload.downcast_ref::<&str>(),
        Some(&"PRIMARY_PANIC_SENTINEL")
    );
    let evidence = fs::read_dir(temporary.path())
        .expect("panic evidence root")
        .map(|entry| return entry.expect("panic evidence entry").path())
        .find(|path| return path.is_dir())
        .expect("retained panic evidence");
    assert!(evidence.join("semantic-screenshot/screen.png").is_file());
    assert!(evidence.join("debug-bundle/manifest.json").is_file());
}

#[cfg(unix)]
#[test]
fn rust_test_helper_does_not_claim_deleted_evidence_after_discard_failure() {
    use nix::unistd::geteuid;
    use std::os::unix::fs::PermissionsExt as _;
    if geteuid().is_root() {
        return;
    }

    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
    let config = TerminalTestConfig::new(terminal, looping_command());
    let mut test = TerminalTest::launch(config).expect("launch test");
    test.session()
        .expect("open test session")
        .output()
        .wait_for(b"READY", Duration::from_secs(5))
        .expect("READY output");

    let locked = test
        .evidence_directory()
        .expect("attempt evidence")
        .join("locked");

    fs::create_dir_all(&locked).expect("locked directory");
    fs::write(locked.join("entry"), b"retained").expect("locked entry");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
        .expect("remove locked permissions");

    let report = test.finalize(TestOutcome::Passed);

    if locked.exists() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700))
            .expect("restore cleanup permissions");
    }

    assert!(!report.retained);
    assert!(report.evidence_directory.is_none());
    assert!(report.verified_artifacts.is_empty());
    assert!(
        report
            .secondary_failures
            .iter()
            .any(|failure| return failure.phase == "evidence_discard"),
        "discard failure was not reported: {:?}",
        report.secondary_failures
    );
}

#[test]
fn rust_test_helper_reports_cleanup_failure_while_resuming_panic() {
    const CHILD_ENV: &str = "TERMTRACE_RUST_PANIC_CLEANUP_CHILD";
    if env::var_os(CHILD_ENV).is_some() {
        let temporary = TempDir::new().expect("temporary directory");
        let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.path());
        let mut config = TerminalTestConfig::new(terminal, looping_command());
        config.finalization_timeout = Duration::from_millis(20);
        let _result = run_terminal_test(
            config,
            |test| -> Result<(), Box<dyn StdError + Send + Sync>> {
                test.session_mut()
                    .expect("open test session")
                    .force_close()?;
                panic!("PRIMARY_PANIC_CLEANUP_SENTINEL");
            },
        );

        return;
    }

    let output = Command::new(env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "testing_adapters::rust_test_helper_reports_cleanup_failure_while_resuming_panic",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("run nested panic test");
    assert!(
        !output.status.success(),
        "nested panic unexpectedly succeeded"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("PRIMARY_PANIC_CLEANUP_SENTINEL"),
        "{stderr}"
    );
    assert!(
        stderr.contains("secondary failures occurred while finalizing a panicked test"),
        "{stderr}"
    );
    assert!(stderr.contains("session_stop"), "{stderr}");
    assert!(stderr.contains("retained panic evidence"), "{stderr}");
}
