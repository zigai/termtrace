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
    terminal::screen::{ScreenFrameContent, ScreenProjectionRequest},
};
use termtrace_sdk::{
    assertions::SnapshotProfile,
    control_endpoint_is_reachable,
    locator::{LocatorSpec, StyleFilter},
    testing::{
        finalize_owned_sessions, run_terminal_test, EvidencePolicy, OwnedSessionFinalization,
        TerminalTest, TerminalTestConfig, TestFailure, TestOutcome,
    },
    LaunchSpec, ScreenFrameQuery, StableCaptureOptions, StopRequest, Terminal,
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
            let capture = test.session().expect("open test session").capture_stable(
                StableCaptureOptions {
                    stable_for: Duration::from_millis(20),
                    timeout: Duration::from_secs(5),
                    projection: ScreenProjectionRequest::default(),
                    allow_incomplete: false,
                    reaction: None,
                },
            )?;

            let ScreenFrameContent::Spans { rows } = &capture.frame.data else {
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
            .expect("observe READY");

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
                .session()
                .expect("open test session")
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
                .session()
                .expect("open test session")
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
            .expect("force-close fixture");

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

    assert_eq!(report.retained, locked.exists());
    assert_eq!(report.evidence_directory.is_some(), locked.exists());
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

#[test]
fn rust_owned_adoption_validation_still_finalizes_the_consumed_session() {
    let root = TempDir::new().expect("evidence root");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), root.path());
    let session = terminal
        .start(LaunchSpec::new(looping_command()))
        .expect("launch");

    session
        .output()
        .wait_for(b"READY", Duration::from_secs(5))
        .expect("ready");
    let path = session.path().to_path_buf();
    let error = TerminalTest::from_owned_session(
        session,
        root.path().join("wrong"),
        EvidencePolicy::Always,
        Duration::from_secs(2),
    )
    .err()
    .expect("validation");
    assert!(error.to_string().contains("outside the test root"));
    assert!(path.join("completion.json").is_file());
    assert!(!control_endpoint_is_reachable(&path));
}

#[test]
fn rust_owned_artifact_preparation_failure_never_prevents_stop() {
    let root = TempDir::new().expect("evidence root");
    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), root.path());
    let session = terminal
        .start(LaunchSpec::new(looping_command()))
        .expect("launch");

    session
        .output()
        .wait_for(b"READY", Duration::from_secs(5))
        .expect("ready");
    let path = session.path().to_path_buf();
    fs::write(
        root.path()
            .join(format!(".test-artifacts-{}", session.session_id())),
        b"collision",
    )
    .expect("artifact collision");

    let report = finalize_owned_sessions(
        vec![OwnedSessionFinalization::Open(Ok(session))],
        root.path(),
        TestOutcome::Passed,
        EvidencePolicy::OnFailure,
        Duration::from_secs(2),
        None,
    );
    assert!(report.retained);
    assert!(report
        .secondary_failures
        .iter()
        .any(|failure| return failure.phase == "artifact_directory"));
    assert!(path.join("completion.json").is_file());
    assert!(!control_endpoint_is_reachable(&path));
    assert!(report
        .verified_artifacts
        .iter()
        .all(|path| return path.exists()));
}

#[test]
fn rust_lifecycle_views_keep_force_authority_during_stalled_control() {
    use nix::{
        sys::signal::{kill, Signal},
        unistd::Pid,
    };
    let root = TempDir::new().expect("evidence root");
    let pid_file = root.path().join("actor");
    let script = "printf '%s' \"$PPID\" > \"$1\"; printf '%s' $$ > \"$1.child\"; printf READY; while IFS= read -r line; do :; done";
    let command = vec![
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(script),
        OsString::from("lifecycle"),
        pid_file.as_os_str().to_owned(),
    ];

    let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), root.path());
    let mut config = TerminalTestConfig::new(terminal, command);
    config.columns = 20;
    config.rows = 3;
    config.finalization_timeout = Duration::from_millis(250);
    let mut test = TerminalTest::launch(config).expect("launch");
    let session = test.session().expect("owned");
    session
        .output()
        .wait_for(b"READY", Duration::from_secs(5))
        .expect("ready");

    let actor: i32 = fs::read_to_string(&pid_file)
        .expect("actor PID")
        .parse()
        .expect("PID");

    let child = fs::read_to_string(pid_file.with_extension("child")).expect("child PID");
    let mut observer = session.lifecycle_observer();
    assert!(observer.is_launched());

    kill(Pid::from_raw(actor), Signal::SIGSTOP).expect("pause owned recorder");

    let worker = thread::spawn(move || {
        return observer.stop(StopRequest::default(), Some(Duration::from_secs(30)));
    });

    let admission = Instant::now() + Duration::from_secs(1);
    while test
        .session()
        .expect("owned")
        .drain_lifecycle_observers(Duration::ZERO)
        .is_ok()
    {
        assert!(Instant::now() < admission, "control observer admission");

        thread::sleep(Duration::from_millis(1));
    }

    let started = Instant::now();
    let report = test.finalize(TestOutcome::Error);

    assert!(started.elapsed() < Duration::from_secs(6));

    let _observed = worker.join().expect("observer returned");

    assert!(report.retained);
    assert!(!PathBuf::from(format!("/proc/{actor}")).exists());
    assert!(!PathBuf::from(format!("/proc/{child}")).exists());
    assert!(test.session().is_none());
}

#[test]
fn rust_passing_body_retains_fatal_recorder_completion_but_not_exit_seven() {
    for (command, fatal) in [
        (
            vec![OsString::from("/termtrace-task04-no-such-child")],
            true,
        ),
        (
            vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("exit 7"),
            ],
            false,
        ),
    ] {
        let root = TempDir::new().expect("evidence root");
        let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), root.path());
        let mut config = TerminalTestConfig::new(terminal, command);
        config.columns = 20;
        config.rows = 3;
        let mut test = TerminalTest::launch(config).expect("launch completion-bearing recorder");
        let completion = test
            .session_mut()
            .expect("owned")
            .complete(Duration::from_secs(5))
            .expect("validated completion");

        if fatal {
            assert_eq!(
                completion.failure.as_ref().expect("fatal fact").code,
                "spawn_failed"
            );
        } else {
            assert!(completion.failure.is_none());
            assert_eq!(completion.exit_code, Some(7));
        }

        let report = test.finalize(TestOutcome::Passed);
        assert_eq!(report.retained, fatal);
        assert_eq!(
            report
                .secondary_failures
                .iter()
                .any(|failure| return failure.phase == "session_failure"),
            fatal
        );

        if !fatal {
            assert!(report.secondary_failures.is_empty());
        }
    }
}

#[test]
fn acknowledged_stop_wait_zero_preserves_applied_and_eventual_completion() {
    use termtrace_model::protocol::mutation::{MutationEffect, Retry};
    use termtrace_sdk::{input::MutationGuard, Error, ErrorKind, Uuid};
    for bounded in [false, true] {
        let root = TempDir::new().expect("evidence root");
        let terminal = Terminal::new(env!("CARGO_BIN_EXE_termtrace"), root.path());
        let command = [
            "/bin/sh",
            "-c",
            "trap 'sleep .5; exit 0' TERM; printf READY; while :; do sleep .05; done",
        ]
        .map(OsString::from)
        .to_vec();

        let mut session = terminal.start(LaunchSpec::new(command)).expect("launch");
        session
            .output()
            .wait_for(b"READY", Duration::from_secs(5))
            .expect("ready");
        // A completion-only query is not a Stop and must not borrow a later acknowledgement.
        let query = session.complete(Duration::ZERO).expect_err("still running");
        assert_eq!(query.kind(), ErrorKind::Engine);
        assert_eq!(query.effect(), MutationEffect::NotApplied);
        assert_eq!(query.retry(), Retry::NewRequest);
        let request_id = Uuid::now_v7();
        let request = StopRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        };
        let error = if bounded {
            session
                .stop_bounded(request, Duration::from_millis(100))
                .expect_err("bounded completion expires after acknowledged Stop")
        } else {
            session
                .stop(request, Some(Duration::ZERO))
                .expect_err("completion wait expires after acknowledged Stop")
        };

        if bounded && error.kind() == ErrorKind::ControlTimeout {
            // No positive acknowledgement arrived within this graceful deadline: do not
            // manufacture Applied merely because later evidence shows the actor stopped.
            assert_eq!(error.effect(), MutationEffect::Unknown);
            assert_eq!(error.retry(), Retry::SameRequestId);
            assert!(matches!(error, Error::Control(_)));
        } else {
            assert_eq!(error.kind(), ErrorKind::Engine);
            assert_eq!(error.code(), "engine");
            assert_eq!(error.effect(), MutationEffect::Applied);
            assert_eq!(error.retry(), Retry::DoNotRetry);
            let Error::MutationContext { source, .. } = &error else {
                panic!("context, not a fabricated actor rejection");
            };
            assert!(matches!(source.as_ref(), Error::Engine(_)));
        }

        let completion = session
            .complete(Duration::from_secs(5))
            .expect("eventual validated completion");
        assert_eq!(completion.session_id, session.session_id());
        assert!(completion.failure.is_none());
        assert!(session.lifecycle_finished());
    }
}
