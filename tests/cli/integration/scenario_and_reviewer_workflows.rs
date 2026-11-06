//! scenario and reviewer workflows CLI integration tests.

use crate::support::*;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::sync::mpsc;

#[test]
fn scenario_runner_executes_typed_steps_in_fresh_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("ready.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "ready workflow",
            "launch": {
                "command": ["/bin/sh", "-c", "test \"$SCENARIO_ID\" = ready && printf Ready && IFS= read -r line && printf Done && sleep 30"],
                "environment": {
                    "base": "clear",
                    "set": { "SCENARIO_ID": "ready" },
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "never"
                },
                "geometry": { "cols": 90, "rows": 25 },
                "timeoutMs": 3000
            },
            "steps": [
                { "waitScreen": { "contains": "Ready" } },
                { "mark": "ready-visible" },
                { "send": [{ "text": "go" }, { "key": "enter" }] },
                { "waitScreen": { "contains": "Done" } },
                { "mark": "done-visible" },
                { "assertTimeline": {
                    "betweenMarkers": ["ready-visible", "done-visible"],
                    "noPresentedScreenContains": "Missing"
                } },
                { "matchScreenSnapshot": {
                    "name": "ready-workflow-final",
                    "stableForMs": 10
                } },
                { "stop": {} }
            ]
        }))
        .expect("scenario JSON"),
    )
    .expect("write scenario");

    let second = temporary.path().join("simple.termtrace.json");
    fs_err::write(
        &second,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "simple workflow",
            "launch": {
                "command": ["/bin/sh", "-c", "test \"$SCENARIO_ID\" = simple && printf SIMPLE && sleep 30"],
                "environment": {
                    "base": "clear",
                    "set": { "SCENARIO_ID": "simple" },
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "auto"
                },
                "timeoutMs": 3000
            },
            "steps": [
                { "waitScreen": { "contains": "SIMPLE" } },
                { "stop": {} }
            ]
        }))
        .expect("second scenario JSON"),
    )
    .expect("write second scenario");

    let junit = temporary.path().join("junit.xml");
    let mut update = Command::cargo_bin("termtrace").expect("binary should build");
    update
        .arg("test")
        .arg(&scenario)
        .arg(&second)
        .arg("--artifacts")
        .arg(&artifacts)
        .arg("--update-snapshots")
        .args(["--jobs", "2"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .success();

    assert!(artifacts
        .join("snapshots/ready-workflow-final.frame.json")
        .is_file());
    assert!(fs_err::read_to_string(&junit)
        .expect("JUnit XML")
        .contains("failures=\"0\""));

    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = command
        .arg("test")
        .arg(&scenario)
        .arg(&second)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--jobs", "2"])
        .assert()
        .success();

    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("scenario report");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["cases"].as_array().expect("cases").len(), 2);

    let attempt = &report["cases"][0]["attempts"][0]["report"];

    assert_eq!(attempt["steps"].as_array().expect("steps").len(), 8);

    let canonical = PathBuf::from(attempt["canonical_session"].as_str().expect("path"));
    assert!(canonical.is_dir());

    let log = EventLog::read(canonical.join("events.jsonl")).expect("scenario event log");
    validate_completed_session(&canonical).expect("scenario completion closure");

    let replayed = replay(&log).expect("scenario final replay");
    assert!(
        replayed.states.last().is_some(),
        "final state must be inspectable"
    );

    for step in attempt["steps"].as_array().expect("steps") {
        let Some(state_hash) = step["state_hash"].as_str() else {
            continue;
        };

        let mut show = Command::cargo_bin("termtrace").expect("binary should build");
        show.arg("screen")
            .arg(&canonical)
            .args(["--state-hash", state_hash, "--format", "json-compact"])
            .assert()
            .success();
    }

    let mut timeline = Command::cargo_bin("termtrace").expect("binary should build");
    timeline
        .args(["analyze", "timeline"])
        .arg(&canonical)
        .args(["--limit", "1", "--jsonl"])
        .assert()
        .success();

    assert!(
        !replay_checkpoints(&canonical).is_empty(),
        "closure must checkpoint an asserted endpoint"
    );
}

#[test]
fn bundled_agent_skill_scenario_is_directly_executable() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("skills/termtrace/examples/ready.termtrace.json");

    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    command
        .arg("test")
        .arg(scenario)
        .arg("--artifacts")
        .arg(temporary.path().join("skill-example-artifacts"))
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status":"passed""#));
}

#[test]
fn scenario_failure_publishes_atomic_diagnostic_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("failure.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "missing screen",
            "launch": {
                "command": ["/bin/sh", "-c", "printf Ready; sleep 30"],
                "environment": {
                    "base": "clear",
                    "set": { "SCENARIO_SECRET": "scenario-private" },
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "never"
                },
                "geometry": { "cols": 200, "rows": 100 },
                "outputByteBudget": 1_000_000,
                "timeoutMs": 1000
            },
            "steps": [{ "waitScreen": { "contains": "Missing", "timeoutMs": 50 } }]
        }))
        .expect("scenario JSON"),
    )
    .expect("write scenario");

    let junit = temporary.path().join("failure.xml");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = command
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--reruns", "1"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .failure();

    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("failure report");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["cases"][0]["attempts"]
            .as_array()
            .expect("attempts")
            .len(),
        2
    );

    assert_scenario_json_xml(&report, &fs_err::read_to_string(&junit).unwrap());
    let attempts = report["cases"][0]["attempts"].as_array().unwrap();
    for (index, attempt) in attempts.iter().enumerate() {
        assert_eq!(attempt["attempt"], index + 1);

        let retained = &attempt["report"];
        let directory = Path::new(retained["failure_artifacts"].as_str().unwrap());
        let on_disk: serde_json::Value =
            serde_json::from_slice(&fs_err::read(directory.join("result.json")).unwrap()).unwrap();
        assert_eq!(&on_disk, retained);

        validate_completed_session(Path::new(retained["canonical_session"].as_str().unwrap()))
            .unwrap();
    }

    assert_ne!(
        attempts[0]["report"]["failure_artifacts"],
        attempts[1]["report"]["failure_artifacts"]
    );
    assert_ne!(
        attempts[0]["report"]["session_id"],
        attempts[1]["report"]["session_id"]
    );

    let attempt = &report["cases"][0]["attempts"][1]["report"];

    assert_eq!(attempt["failure"]["step_index"], 0);

    let failure = Path::new(attempt["failure_artifacts"].as_str().expect("failure path"));
    assert!(failure.join("scenario.json").is_file());
    assert!(failure.join("result.json").is_file());
    assert!(failure.join("final-screen.json").is_file());
    assert!(failure.join("final-screen.txt").is_file());
    assert!(failure.join("final-screen.png").is_file());

    let final_snapshot: ControlPayload = serde_json::from_slice(
        &fs_err::read(failure.join("final-screen.json")).expect("final screen snapshot"),
    )
    .expect("final screen snapshot JSON");
    let ControlPayload::Snapshot {
        viewport_text,
        cols,
        rows,
        ..
    } = final_snapshot
    else {
        panic!("final screen artifact must retain the actor snapshot payload");
    };

    assert_eq!((cols, rows), (200, 100));
    assert!(viewport_text.contains("Ready"));
    assert_eq!(
        fs_err::read_to_string(failure.join("final-screen.txt")).expect("final screen text"),
        viewport_text
    );
    assert!(failure.join("timeline.json").is_file());
    assert!(failure.join("diagnose.json").is_file());

    let canonical = Path::new(
        attempt["canonical_session"]
            .as_str()
            .expect("canonical session"),
    );
    assert_eq!(
        read_manifest(canonical)
            .expect("scenario manifest")
            .resource_limits
            .output_bytes,
        Some(1_000_000)
    );

    let copied_scenario = fs_err::read(failure.join("scenario.json")).expect("copied scenario");
    let encoded_secret = serde_json::to_vec(&ByteString::from(b"scenario-private".as_slice()))
        .expect("encoded secret");

    assert!(!copied_scenario
        .windows(b"scenario-private".len())
        .any(|window| return window == b"scenario-private"));

    let encoded_secret = &encoded_secret[1..encoded_secret.len() - 1];

    for entry in fs_err::read_dir(failure).expect("failure artifacts") {
        let path = entry.expect("failure artifact entry").path();

        if path.is_file() {
            let bytes = fs_err::read(&path).expect("failure artifact");

            for secret in [b"scenario-private".as_slice(), encoded_secret] {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| return window == secret),
                    "scenario secret leaked into {}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn scenario_retry_stops_after_success_and_preserves_real_attempt_outcomes() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("retry.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    let counter = temporary.path().join("attempt-count");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "success on second attempt",
            "launch": {
                "command": [
                    "/bin/sh", "-c",
                    "attempt=0; if test -f \"$ATTEMPTS\"; then IFS= read -r attempt < \"$ATTEMPTS\"; fi; attempt=$((attempt + 1)); printf '%s\\n' \"$attempt\" > \"$ATTEMPTS\"; printf READY; IFS= read -r line; if test \"$attempt\" -eq 1; then printf BROKEN; exit 0; fi; printf '\\r\\nDONE:%s' \"$line\"; IFS= read -r line"
                ],
                "geometry": { "cols": 40, "rows": 4 },
                "environment": { "base": "clear", "set": { "ATTEMPTS": counter } },
                "timeoutMs": 3000
            },
            "steps": [
                { "waitScreen": { "contains": "READY", "stableForMs": 10 } },
                { "send": [{ "text": "go" }, { "key": "enter" }] },
                { "waitScreen": { "contains": "DONE:go", "stableForMs": 10 } },
                { "mark": "after-success" },
                { "stop": {} }
            ]
        }))
        .expect("scenario JSON"),
    )
    .expect("write retry scenario");

    let junit = temporary.path().join("permissive.xml");
    let assertion = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--reruns", "3"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .success();

    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("suite report");
    assert_eq!(report["status"], "flaky");
    assert_eq!(report["fail_on_flaky"], false);
    assert_eq!(
        fs_err::read_to_string(&counter).expect("attempt count"),
        "2\n"
    );

    assert_scenario_json_xml(&report, &fs_err::read_to_string(&junit).unwrap());
    let attempts = report["cases"][0]["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 2);

    let failed = &attempts[0]["report"];
    let passed = &attempts[1]["report"];

    assert_eq!(failed["status"], "failed");
    assert_eq!(
        failed["steps"].as_array().expect("successful prefix").len(),
        2
    );
    assert_eq!(failed["failure"]["step_index"], 2);
    assert_eq!(passed["status"], "passed");
    assert_eq!(
        passed["steps"].as_array().expect("completed steps").len(),
        5
    );
    assert_ne!(failed["session_id"], passed["session_id"]);
    assert_ne!(failed["canonical_session"], passed["canonical_session"]);
    assert_ne!(
        passed["steps"][0]["state_hash"],
        passed["steps"][2]["state_hash"]
    );

    for (attempt, expected_marker) in [(failed, false), (passed, true)] {
        let path = Path::new(
            attempt["canonical_session"]
                .as_str()
                .expect("canonical path"),
        );

        let log = EventLog::read(path.join("events.jsonl")).expect("canonical events");

        validate_completed_session(path).expect("valid attempt closure");

        let contains_marker = log.events.iter().any(|record| {
            return matches!(&record.event, Event::Marker { label, .. } if label == "after-success");
        });
        assert_eq!(contains_marker, expected_marker);
    }

    fs_err::remove_file(&counter).unwrap();
    let strict_junit = temporary.path().join("strict.xml");
    let strict = Command::cargo_bin("termtrace")
        .unwrap()
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--reruns", "3", "--fail-on-flaky"])
        .arg("--junit")
        .arg(&strict_junit)
        .assert()
        .failure();

    let strict_report: serde_json::Value =
        serde_json::from_slice(&strict.get_output().stdout).unwrap();
    assert_eq!(strict_report["status"], "flaky");
    assert_eq!(strict_report["fail_on_flaky"], true);
    assert_eq!(
        strict_report["cases"][0]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fs_err::read_to_string(&counter).unwrap(), "2\n");

    assert_scenario_json_xml(
        &strict_report,
        &fs_err::read_to_string(&strict_junit).unwrap(),
    );

    // A later recovery must retain both failed fresh-session evidence sets as well.
    let mut third_scenario: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&scenario).unwrap()).unwrap();

    let command = third_scenario["launch"]["command"][2]
        .as_str()
        .unwrap()
        .replace("-eq 1", "-lt 3");
    third_scenario["launch"]["command"][2] = command.into();
    fs_err::write(&scenario, serde_json::to_vec(&third_scenario).unwrap()).unwrap();

    fs_err::remove_file(&counter).unwrap();
    let third_junit = temporary.path().join("third-attempt.xml");
    let third = Command::cargo_bin("termtrace")
        .unwrap()
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--reruns", "3"])
        .arg("--junit")
        .arg(&third_junit)
        .assert()
        .success();

    let third_report: serde_json::Value =
        serde_json::from_slice(&third.get_output().stdout).unwrap();
    assert_eq!(third_report["status"], "flaky");

    let third_attempts = third_report["cases"][0]["attempts"].as_array().unwrap();
    assert_eq!(third_attempts.len(), 3);
    assert_eq!(fs_err::read_to_string(&counter).unwrap(), "3\n");
    for (index, a) in third_attempts.iter().enumerate() {
        assert_eq!(a["attempt"], index + 1);
        assert_eq!(
            a["report"]["status"],
            if index < 2 { "failed" } else { "passed" }
        );

        if index < 2 {
            assert!(
                Path::new(a["report"]["failure_artifacts"].as_str().unwrap())
                    .join("result.json")
                    .is_file()
            );
        }
    }

    assert_scenario_json_xml(
        &third_report,
        &fs_err::read_to_string(&third_junit).unwrap(),
    );
}

#[test]
fn scenario_wait_requires_the_requested_stability_interval() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("unstable.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "unstable matching screen",
            "launch": {
                "command": [
                    "/bin/sh",
                    "-c",
                    "printf 'READY\\n'; i=0; while :; do printf '\\r%04d' \"$i\"; i=$((i + 1)); sleep .01; done"
                ],
                "environment": {
                    "base": "clear",
                    "set": {},
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "never"
                },
                "timeoutMs": 4000
            },
            "steps": [{
                "waitScreen": {
                    "contains": "READY",
                    "stableForMs": 1000,
                    "timeoutMs": 2000
                }
            }]
        }))
        .expect("scenario JSON"),
    )
    .expect("write scenario");

    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = command
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .assert()
        .failure();

    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("scenario report");
    assert_eq!(report["status"], "failed");

    let attempt = &report["cases"][0]["attempts"][0]["report"];

    assert_eq!(attempt["failure"]["step_index"], 0);
    assert!(attempt["failure"]["message"]
        .as_str()
        .is_some_and(|message| return message.contains("requested stability before Deadline")));
}

#[test]
fn reviewer_summary_is_read_only_and_never_replays_recorded_controls() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("review-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r"printf '\033]52;c;hostile-payload\007SAFE'",
        ])
        .assert()
        .success();

    let event_path = session.join("events.jsonl");
    let before = fs_err::read(&event_path).expect("event log before review");
    let mut review = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = review
        .args(["analyze", "review"])
        .arg(&session)
        .arg("--json")
        .assert()
        .success();

    let summary: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("review summary");
    assert_eq!(
        summary["rendering_policy"],
        "semantic_cells_only_no_raw_replay"
    );
    assert!(summary["state_count"].as_u64().expect("state count") > 0);
    assert!(!assertion
        .get_output()
        .stdout
        .windows("hostile-payload".len())
        .any(|window| return window == b"hostile-payload"));

    let mut search = Command::cargo_bin("termtrace").expect("binary should build");
    let searched = search
        .args(["analyze", "review"])
        .arg(&session)
        .args(["--json", "--search", "SAFE"])
        .assert()
        .success();

    let result: serde_json::Value =
        serde_json::from_slice(&searched.get_output().stdout).expect("review search");
    assert_eq!(result["query"], "SAFE");
    assert!(!result["matches"].as_array().expect("matches").is_empty());
    assert!(result["matches"][0]["selector"]
        .as_str()
        .is_some_and(|selector| return selector.starts_with("--state ")));

    let state_id = result["matches"][0]["state_id"]
        .as_u64()
        .expect("matched state ID");

    let exported = temporary.path().join("review-export");
    let mut export = Command::cargo_bin("termtrace").expect("binary should build");
    export
        .args(["analyze", "review"])
        .arg(&session)
        .arg("--export-state")
        .arg(&exported)
        .arg("--state")
        .arg(state_id.to_string())
        .assert()
        .success();

    assert!(exported.join("manifest.json").is_file());
    assert!(exported.join("state.json").is_file());
    assert!(exported.join("screen.txt").is_file());
    assert_eq!(
        fs_err::read(event_path).expect("event log after review"),
        before
    );
}

#[test]
fn reviewer_exposes_scriptable_commands_without_a_session() {
    let mut review = Command::cargo_bin("termtrace").expect("binary should build");
    review
        .args(["analyze", "review", "--commands"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "semantic_cells_only_no_raw_replay",
        ))
        .stdout(predicate::str::contains("next_state"))
        .stdout(predicate::str::contains("next_marker"));
}

#[expect(
    clippy::expect_used,
    reason = "reviewer fixture setup must stop when the CLI cannot create its recording"
)]
fn recorded_reviewer_session(temporary: &TempDir, name: &str) -> PathBuf {
    let session = temporary.path().join(name);
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf REVIEWER"])
        .assert()
        .success();

    return session;
}

#[expect(
    clippy::expect_used,
    reason = "real-PTY lifecycle assertions must stop when their terminal fixture cannot run"
)]
fn reviewer_pty_output(session: &Path, signal: Option<&str>) -> Vec<u8> {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 1_000,
            pixel_height: 480,
        })
        .expect("reviewer PTY");

    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termtrace"));
    command.args([
        "analyze",
        "review",
        session.to_str().expect("UTF-8 reviewer session"),
    ]);
    let mut child = pty.slave.spawn_command(command).expect("spawn reviewer");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
    let (chunk_sender, chunk_receiver) = mpsc::channel();

    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 4_096];

        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }

            let chunk = buffer.iter().copied().take(count).collect::<Vec<_>>();
            output.extend_from_slice(&chunk);
            if chunk_sender.send(chunk).is_err() {
                break;
            }
        }

        return output;
    });

    let mut writer = pty.master.take_writer().expect("take PTY writer");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut entered_alternate_screen = false;
    let mut observed = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());

        match chunk_receiver.recv_timeout(remaining) {
            Ok(chunk) => {
                observed.extend_from_slice(&chunk);
                if observed
                    .windows(b"\x1b[?1049h".len())
                    .any(|window| return window == b"\x1b[?1049h")
                {
                    entered_alternate_screen = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if !entered_alternate_screen {
        child.kill().expect("terminate stalled reviewer");
        let _status = child.wait().expect("reap stalled reviewer");
    }

    assert!(
        entered_alternate_screen,
        "reviewer did not enter the alternate screen: {}",
        String::from_utf8_lossy(&observed)
    );

    if let Some(signal) = signal {
        let process_id = child.process_id().expect("reviewer process ID");
        let status = StdCommand::new("/bin/kill")
            .arg(format!("-{signal}"))
            .arg(process_id.to_string())
            .status()
            .expect("signal reviewer");

        assert!(status.success(), "reviewer signal delivery failed");
    } else {
        writer.write_all(b"q").expect("quit reviewer");
        writer.flush().expect("flush reviewer input");
    }

    let status = child.wait().expect("wait for reviewer");
    assert!(status.success(), "reviewer did not exit successfully");

    drop(writer);
    drop(pty.master);

    let output = reader_thread.join().expect("join reviewer PTY reader");

    assert!(
        output
            .windows(b"\x1b[?1049l".len())
            .any(|window| return window == b"\x1b[?1049l"),
        "reviewer did not leave the alternate screen: {}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        output
            .windows(b"\x1b[?1000l".len())
            .any(|window| return window == b"\x1b[?1000l"),
        "reviewer did not disable mouse capture: {}",
        String::from_utf8_lossy(&output)
    );

    return output;
}

#[test]
fn reviewer_restores_real_pty_after_quit_and_termination_signal() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = recorded_reviewer_session(&temporary, "reviewer-lifecycle");
    let quit_output = reviewer_pty_output(&session, None);
    assert!(
        quit_output
            .windows(b"\x1b[?25h".len())
            .any(|window| return window == b"\x1b[?25h"),
        "normal reviewer exit did not restore the cursor"
    );
    let _signal_output = reviewer_pty_output(&session, Some("TERM"));
}

#[test]
fn reviewer_rolls_back_raw_mode_when_terminal_setup_fails() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = recorded_reviewer_session(&temporary, "reviewer-setup-failure");
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 1_000,
            pixel_height: 480,
        })
        .expect("reviewer rollback PTY");

    let script = r#"
before=$(stty -g) || exit 91
"$1" analyze review "$2" >/dev/full
reviewer_status=$?
after=$(stty -g) || exit 92
if [ "$before" = "$after" ]; then
    restored=yes
else
    restored=no
fi
printf '\n__TERMTRACE_REVIEWER_ROLLBACK__=%s reviewer_status=%s\n' "$restored" "$reviewer_status"
"#;

    let mut command = CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        script,
        "termtrace-reviewer-rollback",
        env!("CARGO_BIN_EXE_termtrace"),
        session.to_str().expect("UTF-8 reviewer session"),
    ]);
    let mut child = pty
        .slave
        .spawn_command(command)
        .expect("spawn rollback shell");

    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        let _read_result = reader.read_to_end(&mut output);

        return output;
    });

    let status = child.wait().expect("wait for rollback shell");
    assert!(status.success(), "rollback shell failed");

    drop(pty.master);

    let output = reader_thread.join().expect("join rollback PTY reader");
    assert!(
        output
            .windows(b"__TERMTRACE_REVIEWER_ROLLBACK__=yes".len())
            .any(|window| return window == b"__TERMTRACE_REVIEWER_ROLLBACK__=yes"),
        "reviewer setup failure left the PTY altered: {}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        !output
            .windows(b"reviewer_status=0".len())
            .any(|window| return window == b"reviewer_status=0"),
        "reviewer terminal setup unexpectedly succeeded"
    );
}

#[test]
fn reviewer_pages_preserve_exact_hidden_states_and_reject_tampered_sources() {
    use std::sync::atomic::AtomicBool;
    use termtrace_engine::{
        analysis::reviewer::{load_review_page, ReviewQuery, REVIEW_PAGE_STATES},
        observation::state_text,
        replay::inspect,
    };
    use termtrace_model::terminal::replay::StateSelector;

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("paged-reviewer");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf '\\033[?2026habc'; sleep 0.08; printf '\\033[?2026l'; printf '%0120dTARGET' 0",
        ])
        .assert()
        .success();

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let states = replay(&log)
        .expect("bounded fixture reference replay")
        .states;

    let hidden = states
        .iter()
        .find(|state| return !state.presented)
        .expect("fixture contains synchronized hidden semantic boundaries");

    let cancelled = AtomicBool::new(false);
    let page = load_review_page(&session, &ReviewQuery::State(hidden.state_id), &cancelled)
        .expect("exact hidden page")
        .expect("selected page");
    assert!(page.entries.len() <= REVIEW_PAGE_STATES);

    let selected = page
        .entries
        .iter()
        .find(|entry| return entry.state_id == page.selected)
        .expect("selected hidden state retained");
    assert_eq!(selected.state_hash, hidden.state_hash);
    assert_eq!(selected.event_seq, hidden.event_seq);
    assert_eq!(selected.event_byte_offset, hidden.event_byte_offset);
    assert!(!selected.presented);

    let page_ids = page
        .entries
        .iter()
        .map(|entry| return entry.state_id)
        .collect::<Vec<_>>();

    let first = states
        .iter()
        .position(|state| return Some(&state.state_id) == page_ids.first())
        .expect("page starts at a replayed state");
    assert_eq!(
        page_ids,
        states[first..first + page_ids.len()]
            .iter()
            .map(|state| return state.state_id)
            .collect::<Vec<_>>(),
        "pages cannot sample away hidden states"
    );

    let next_presented = states
        .iter()
        .find(|state| return state.state_id > hidden.state_id && state.presented)
        .expect("next presented state");

    let next = load_review_page(
        &session,
        &ReviewQuery::Move {
            from: hidden.state_id,
            forward: true,
            presented_only: true,
        },
        &cancelled,
    )
    .expect("presented navigation")
    .expect("next page");
    assert_eq!(next.selected, next_presented.state_id);

    let found = load_review_page(
        &session,
        &ReviewQuery::Search {
            from: None,
            forward: true,
            query: "TARGET".to_owned(),
        },
        &cancelled,
    )
    .expect("paged semantic search")
    .expect("matching page");

    let inspection =
        inspect(&log, StateSelector::StateId(found.selected)).expect("reference matched state");
    assert!(state_text(&inspection.state).contains("TARGET"));
    assert!(load_review_page(&session, &ReviewQuery::Final, &AtomicBool::new(true)).is_err());

    let path = session.join("events.jsonl");
    let mut changed = fs_err::read(&path).expect("canonical bytes");
    changed.extend_from_slice(b"tampered\n");
    fs_err::write(&path, changed).expect("tamper warmed source");

    assert!(load_review_page(&session, &ReviewQuery::State(0), &cancelled).is_err());
}

#[test]
fn reviewer_cancels_large_background_loading_and_restores_the_terminal() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("cancel-reviewer-load");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os; os.write(1,b'x'*1048576)",
        ])
        .assert()
        .success();

    let _quit = reviewer_pty_output(&session, None);
    let _signal = reviewer_pty_output(&session, Some("TERM"));
}

#[test]
fn reviewer_pages_resolve_marker_and_input_source_boundaries() {
    use std::sync::atomic::AtomicBool;
    use termtrace_engine::analysis::reviewer::{load_review_page, ReviewQuery};

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("reviewer-source-jumps");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf READY; IFS= read -r line; printf 'DONE:%s' \"$line\"",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("record source jump fixture");

    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("fixture ready");

    send_request(
        &session,
        &ControlRequest::new(ControlCommand::Mark {
            label: "checkpoint".to_owned(),
        }),
    )
    .expect("canonical marker");

    send_request(
        &session,
        &ControlRequest::new(ControlCommand::Input {
            data: ByteString::new(b"accepted\n".to_vec()),
            input_kind: InputKind::Text,
            caller_intent: None,
        }),
    )
    .expect("canonical input");

    let output = child.wait_with_output().expect("fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("closed evidence");
    let states = replay(&log).expect("bounded reference states").states;
    let cancelled = AtomicBool::new(false);

    for (query, marker) in [
        (ReviewQuery::Marker("checkpoint".to_owned()), true),
        (
            ReviewQuery::Event {
                from_ns: 0,
                forward: true,
                marker: true,
            },
            true,
        ),
        (
            ReviewQuery::Event {
                from_ns: u64::MAX,
                forward: false,
                marker: false,
            },
            false,
        ),
    ] {
        let time = log
            .events
            .iter()
            .find_map(|record| {
                let selected = if marker {
                    matches!(record.event, Event::Marker { .. })
                } else {
                    matches!(
                        record.event,
                        Event::InputIntent {
                            source: InputSource::Agent,
                            ..
                        }
                    )
                };

                return selected.then_some(record.monotonic_ns);
            })
            .expect("canonical source event");

        let expected = states
            .iter()
            .find(|state| return state.monotonic_ns >= time)
            .or_else(|| return states.last())
            .expect("state at source boundary");

        let page = load_review_page(&session, &query, &cancelled)
            .expect("source boundary page")
            .expect("selected boundary");
        assert_eq!(page.selected, expected.state_id);
    }
}

fn assert_scenario_json_xml(report: &serde_json::Value, xml: &str) {
    macro_rules! required {
        ($expression:expr) => {{
            let result = $expression;
            assert!(result.is_some(), "missing {}", stringify!($expression));
            let Some(value) = result else {
                return;
            };
            value
        }};
    }
    macro_rules! successful {
        ($expression:expr) => {{
            let result = $expression;
            assert!(
                result.is_ok(),
                "failed {}: {:?}",
                stringify!($expression),
                result.as_ref().err()
            );
            let Ok(value) = result else {
                return;
            };
            value
        }};
    }
    let document = successful!(roxmltree::Document::parse(xml));
    let root = document.root_element();

    let cases = required!(report["cases"].as_array());
    assert_eq!(
        successful!(required!(root.attribute("tests")).parse::<usize>()),
        cases.len()
    );
    let expected_time = |ms: u64| return format!("{}.{:03}", ms / 1000, ms % 1000);
    assert_eq!(
        required!(root.attribute("time")),
        expected_time(required!(report["duration_ms"].as_u64()))
    );
    let nodes: Vec<_> = root
        .children()
        .filter(|n| return n.has_tag_name("testcase"))
        .collect();
    assert_eq!(nodes.len(), cases.len());
    let mut failures = 0;
    let mut errors = 0;

    for (node, case) in nodes.iter().zip(cases) {
        assert_eq!(
            required!(node.attribute("classname")),
            required!(case["scenario"].as_str())
        );
        let attempts = required!(case["attempts"].as_array());
        let last_attempt = required!(attempts.last());
        let last_report = required!(last_attempt.get("report"));
        assert_eq!(
            required!(node.attribute("name")),
            required!(last_report["name"].as_str())
        );
        let mut total = 0_u64;
        for attempt in attempts {
            total += required!(attempt["duration_ms"].as_u64());
        }

        assert_eq!(required!(node.attribute("time")), expected_time(total));
        for (name, value) in [
            ("verdict", required!(case["status"].as_str())),
            (
                "fail_on_flaky",
                if report["fail_on_flaky"] == true { "true" } else { "false" },
            ),
        ] {
            assert_eq!(
                required!(node.descendants().find(
                    |n| return n.has_tag_name("property") && n.attribute("name") == Some(name)
                ))
                .attribute("value"),
                Some(value)
            );
        }

        let infrastructure = attempts.iter().any(|a| {
            return a.get("report").and_then(|r| return r.get("status"))
                == Some(&serde_json::json!("error"));
        }) || !required!(report["infrastructure_errors"].as_array())
            .is_empty();

        let failed = !infrastructure
            && (case["status"] == "failed"
                || (case["status"] == "flaky" && report["fail_on_flaky"] == true));

        assert_eq!(
            node.children().any(|n| return n.has_tag_name("error")),
            infrastructure
        );
        assert_eq!(
            node.children().any(|n| return n.has_tag_name("failure")),
            failed
        );
        errors += usize::from(infrastructure);
        failures += usize::from(failed);

        let out_node = required!(node
            .children()
            .find(|n| return n.has_tag_name("system-out")));

        let out = required!(out_node.text());

        for attempt in attempts {
            assert!(out.contains(&format!(
                "attempt {} duration_ms={}",
                attempt["attempt"], attempt["duration_ms"]
            )));
            let attempt_report = required!(attempt.get("report"));

            for field in ["canonical_session", "failure_artifacts"] {
                if let Some(path) = attempt_report.get(field).and_then(|v| return v.as_str()) {
                    assert!(out.contains(path));
                }
            }
        }
    }

    assert_eq!(
        successful!(required!(root.attribute("failures")).parse::<usize>()),
        failures
    );
    assert_eq!(
        successful!(required!(root.attribute("errors")).parse::<usize>()),
        errors
    );
    let suite_schema: serde_json::Value = successful!(serde_json::from_slice(include_bytes!(
        "../../../schemas/scenario-suite-report.schema.json"
    )));

    let attempt_schema: serde_json::Value = successful!(serde_json::from_slice(include_bytes!(
        "../../../schemas/scenario-report.schema.json"
    )));

    let suite_validator = successful!(jsonschema::validator_for(&suite_schema));
    assert!(
        suite_validator.is_valid(report),
        "suite schema: {:?}",
        suite_validator
            .iter_errors(report)
            .map(|e| return e.to_string())
            .collect::<Vec<_>>()
    );
    let attempt_validator = successful!(jsonschema::validator_for(&attempt_schema));

    for case in cases {
        for attempt in required!(case["attempts"].as_array()) {
            assert!(attempt_validator.is_valid(&attempt["report"]));
        }
    }
}

#[test]
fn scenario_junit_parsed_escaping_is_exactly_once_in_attributes_and_text() {
    use termtrace_model::scenario::*;
    let special = "Unicode 🦀 < > & ' \" &amp; ]]>\t\n\r\0\u{1}\u{b}\u{fffe}";
    let normalized = special.replace(['\0', '\u{1}', '\u{b}', '\u{fffe}'], "�");
    let attempt: ScenarioReport = serde_json::from_value(serde_json::json!({
        "version": 3, "name": special, "status": "failed", "session_id": "00000000-0000-0000-0000-000000000000",
        "canonical_session": special, "steps": [], "failure": { "step_index": 0, "step_kind": special, "request_ids": [], "message": special },
        "infrastructure_errors": [], "artifact_warnings": [], "failure_artifacts": null
    })).unwrap();

    let report = ScenarioSuiteReport {
        version: 2,
        status: ScenarioSuiteStatus::Failed,
        duration_ms: 1001,
        fail_on_flaky: false,
        infrastructure_errors: vec![],
        jobs: 1,
        shard_index: None,
        shard_count: None,
        cases: vec![ScenarioCaseReport {
            scenario: special.into(),
            status: ScenarioCaseStatus::Failed,
            attempts: vec![ScenarioAttemptReport {
                attempt: 1,
                duration_ms: 1001,
                report: attempt,
            }],
        }],
    };
    let xml = termtrace_scenario::scenario_junit(&report);
    assert_eq!(xml, termtrace_scenario::scenario_junit(&report));

    let document = roxmltree::Document::parse(&xml).unwrap();
    let case = document
        .descendants()
        .find(|n| return n.has_tag_name("testcase"))
        .unwrap();
    assert_eq!(case.attribute("name"), Some(normalized.as_str()));
    assert_eq!(case.attribute("classname"), Some(normalized.as_str()));

    let out = case
        .children()
        .find(|n| return n.has_tag_name("system-out"))
        .unwrap()
        .text()
        .unwrap();

    // Character references preserve carriage returns without XML newline normalization.
    let text_normalized = normalized;

    assert!(out.contains(&format!("step 0 ({text_normalized}) {text_normalized}")));
    assert!(out.contains("&amp; ]]>"));

    let mut malformed = report;
    malformed.cases[0].attempts.clear();
    let xml = termtrace_scenario::scenario_junit(&malformed);
    let document = roxmltree::Document::parse(&xml).unwrap();
    assert_eq!(document.root_element().attribute("errors"), Some("1"));
    assert!(document
        .descendants()
        .any(|n| return n.has_tag_name("error")));
}

#[test]
fn scenario_infrastructure_siblings_matrix_shards_and_report_write_errors() {
    let temp = TempDir::new().unwrap();
    let valid = temp.path().join("a-matrix.json");
    let invalid = temp.path().join("b-launch.json");
    let prelaunch = temp.path().join("c-prelaunch.json");
    fs_err::write(&valid, serde_json::to_vec(&serde_json::json!({"version": 2, "name": "matrix {{theme}}", "launch": {"command": ["/bin/sh", "-c", "printf READY; read line"], "timeoutMs": 5000}, "matrix": {"theme": ["dark", "light"]}, "steps": [{"waitScreen": {"contains": "READY"}}, {"stop": {}}]})).unwrap()).unwrap();
    fs_err::write(&invalid, serde_json::to_vec(&serde_json::json!({"version": 2, "name": "launch error", "launch": {"command": ["/does-not-exist-scenario"], "timeoutMs": 5000}, "steps": [{"stop": {}}]})).unwrap()).unwrap();
    fs_err::write(&prelaunch, serde_json::to_vec(&serde_json::json!({"version":2,"name":"prelaunch", "launch":{"command":["/bin/true"], "workingDirectory":temp.path().join("missing-directory"),"timeoutMs":5000},"steps":[{"stop":{}}]})).unwrap()).unwrap();
    let junit = temp.path().join("mixed.xml");
    let output = Command::cargo_bin("termtrace")
        .unwrap()
        .arg("test")
        .arg(&valid)
        .arg(&invalid)
        .arg(&prelaunch)
        .arg("--artifacts")
        .arg(temp.path().join("mixed"))
        .args(["--jobs", "2", "--reruns", "2"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .failure();

    let report: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(report["cases"].as_array().unwrap().len(), 4);
    assert_eq!(report["cases"][0]["status"], "passed");
    assert_eq!(report["cases"][1]["status"], "passed");
    assert_eq!(report["cases"][2]["attempts"].as_array().unwrap().len(), 1);
    assert!(report["cases"][2]["attempts"][0]["report"]["session_id"].is_string());
    assert_eq!(
        report["cases"][2]["attempts"][0]["report"]["infrastructure_errors"][0]["code"],
        "spawn_failed"
    );
    assert_eq!(report["cases"][3]["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["cases"][3]["attempts"][0]["report"]["session_id"],
        serde_json::Value::Null
    );

    assert_scenario_json_xml(&report, &fs_err::read_to_string(&junit).unwrap());

    let mut selected = Vec::new();
    for shard in [0, 1] {
        let path = temp.path().join(format!("shard-{shard}.xml"));
        let output = Command::cargo_bin("termtrace")
            .unwrap()
            .arg("test")
            .arg(&valid)
            .arg("--artifacts")
            .arg(temp.path().join(format!("shard-{shard}")))
            .args([
                "--jobs",
                "2",
                "--shard-count",
                "2",
                "--shard-index",
                &shard.to_string(),
            ])
            .arg("--junit")
            .arg(&path)
            .assert()
            .success();

        let r: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
        assert_scenario_json_xml(&r, &fs_err::read_to_string(path).unwrap());
        selected.push(r["cases"][0]["scenario"].clone());
    }

    assert_ne!(selected[0], selected[1]);

    // sysfs forbids creating arbitrary files, including when tests run as root: exercise
    // a real Linux permission/read-only publication failure without a permission-based skip.
    let denied = PathBuf::from(format!(
        "/sys/termtrace-scenario-junit-{}.xml",
        uuid::Uuid::now_v7()
    ));

    for bad in [
        junit.clone(),
        temp.path().join("not-a-file"),
        denied.clone(),
    ] {
        if bad != junit && bad != denied {
            fs_err::create_dir(&bad).unwrap();
        }

        let before = bad.is_file().then(|| return fs_err::read(&bad).unwrap());
        let output = Command::cargo_bin("termtrace")
            .unwrap()
            .arg("test")
            .arg(&valid)
            .arg("--artifacts")
            .arg(temp.path().join("write-errors"))
            .arg("--junit")
            .arg(&bad)
            .assert()
            .failure();

        let r: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
        assert_eq!(r["status"], "failed");
        assert_eq!(r["cases"][0]["status"], "passed");
        assert_eq!(r["infrastructure_errors"][0]["phase"], "report_write");

        if let Some(bytes) = before {
            assert_eq!(fs_err::read(&bad).unwrap(), bytes);
        }
    }
}

#[test]
fn scenario_publication_failure_preserves_primary_and_successful_sibling() {
    let temp = TempDir::new().unwrap();
    let artifacts = temp.path().join("artifacts");
    fs_err::create_dir(&artifacts).unwrap();
    fs_err::write(artifacts.join("failures"), "not a directory").unwrap();
    let fail = temp.path().join("a-fail.json");
    let pass = temp.path().join("b-pass.json");
    for (path, expected) in [(&fail, "NEVER"), (&pass, "READY")] {
        fs_err::write(path, serde_json::to_vec(&serde_json::json!({"version":2, "name":expected, "launch":{"command":["/bin/sh","-c","printf READY; read line"], "timeoutMs":5000, "geometry":{"cols":40,"rows":4}}, "steps":[{"waitScreen":{"contains":expected,"timeoutMs":300}}, {"stop":{}}]})).unwrap()).unwrap();
    }

    let junit = temp.path().join("publication.xml");
    let output = Command::cargo_bin("termtrace")
        .unwrap()
        .arg("test")
        .arg(&fail)
        .arg(&pass)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--jobs", "2", "--reruns", "3"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .failure();

    let r: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let attempts = r["cases"][0]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);

    let report = &attempts[0]["report"];

    assert_eq!(report["status"], "error");
    assert_eq!(report["failure"]["step_index"], 0);
    assert_eq!(report["infrastructure_errors"][0]["phase"], "publication");
    assert_eq!(report["failure_artifacts"], serde_json::Value::Null);
    assert_eq!(r["cases"][1]["status"], "passed");

    assert_scenario_json_xml(&r, &fs_err::read_to_string(junit).unwrap());

    assert_eq!(
        fs_err::read_to_string(artifacts.join("failures")).unwrap(),
        "not a directory"
    );
}

#[test]
fn scenario_transport_failure_force_closes_recorder_and_drains_siblings() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let temp = TempDir::new().unwrap();
    let stopped = temp.path().join("a-stopped.json");
    let good = temp.path().join("b-good.json");
    let pids = temp.path().join("pids");
    fs_err::write(&stopped, serde_json::to_vec(&serde_json::json!({"version":2,"name":"stopped recorder", "launch":{"command":["/bin/sh","-c","printf READY; IFS= read -r line; printf '%s %s\\n' \"$$\" \"$PPID\" > \"$PIDS\"; kill -STOP \"$PPID\"; while :; do sleep 1; done"], "environment":{"base":"clear","set":{"PIDS":pids},"copy":["PATH"]}, "geometry":{"cols":40,"rows":4},"timeoutMs":5000},"steps":[{"waitScreen":{"contains":"READY"}},{"send":[{"text":"go\n"}]},{"waitScreen":{"contains":"NEVER","timeoutMs":100}}]})).unwrap()).unwrap();
    fs_err::write(&good, serde_json::to_vec(&serde_json::json!({"version":2,"name":"good sibling", "launch":{"command":["/bin/sh","-c","printf READY; read line"],"timeoutMs":5000},"steps":[{"waitScreen":{"contains":"READY"}},{"stop":{}}]})).unwrap()).unwrap();
    let junit = temp.path().join("transport.xml");
    let output = Command::cargo_bin("termtrace")
        .unwrap()
        .arg("test")
        .arg(&stopped)
        .arg(&good)
        .arg("--artifacts")
        .arg(temp.path().join("artifacts"))
        .args(["--jobs", "2", "--reruns", "3"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .failure();

    let r: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    assert_eq!(r["cases"][1]["status"], "passed");

    let attempts = r["cases"][0]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);

    let report = &attempts[0]["report"];

    assert_eq!(report["status"], "error");
    assert!(report["failure"].is_object());

    let errors = report["infrastructure_errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| return e["code"] == "control_timeout"));
    assert!(errors.iter().any(|e| return e["phase"] == "stop"));
    assert!(errors.iter().any(|e| return e["phase"] == "completion"));
    assert!(errors.iter().any(|e| return e["phase"] == "replay"));

    assert_scenario_json_xml(&r, &fs_err::read_to_string(junit).unwrap());
    let pids = fs_err::read_to_string(pids).unwrap();
    let pids: Vec<u32> = pids
        .split_whitespace()
        .map(|pid| return pid.parse().expect("numeric PID"))
        .collect();
    assert_eq!(pids.len(), 2);
    assert!(
        !Path::new(&format!("/proc/{}", pids[1])).exists(),
        "owned recorder must be reaped"
    );

    let deadline = Instant::now() + Duration::from_secs(1);

    for pid in pids {
        let process = PathBuf::from(format!("/proc/{pid}"));
        while process.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            !process.exists(),
            "child/recorder {pid} remained after force-close"
        );
    }
}

#[test]
fn scenario_json_write_failure_including_broken_pipe_is_nonzero_after_junit() {
    use std::{
        fs::OpenOptions,
        process::{Command as ProcessCommand, Stdio},
    };
    let temp = TempDir::new().unwrap();
    let scenario = temp.path().join("success.json");
    fs_err::write(&scenario, serde_json::to_vec(&serde_json::json!({"version":2,"name":"JSON write failure", "launch":{"command":["/bin/sh","-c","printf READY; read line"],"timeoutMs":5000},"steps":[{"waitScreen":{"contains":"READY"}},{"stop":{}}]})).unwrap()).unwrap();
    for broken_pipe in [false, true] {
        let junit = temp.path().join(format!("junit-{broken_pipe}.xml"));
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_termtrace"));
        command
            .env_remove("TERMTRACE_RESPONSE")
            .arg("test")
            .arg(&scenario)
            .arg("--artifacts")
            .arg(temp.path().join(format!("artifacts-{broken_pipe}")))
            .arg("--junit")
            .arg(&junit)
            .stderr(Stdio::piped());
        let output = if broken_pipe {
            let mut child = command.stdout(Stdio::piped()).spawn().unwrap();
            drop(child.stdout.take());

            child.wait_with_output().unwrap()
        } else {
            command
                .stdout(OpenOptions::new().write(true).open("/dev/full").unwrap())
                .output()
                .unwrap()
        };

        assert!(!output.status.success(), "lost JSON verdict must fail CI");

        let xml = fs_err::read_to_string(junit).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(doc.root_element().attribute("tests"), Some("1"));
        assert_eq!(doc.root_element().attribute("failures"), Some("0"));
        assert_eq!(doc.root_element().attribute("errors"), Some("0"));
    }

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .env_remove("TERMTRACE_RESPONSE")
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(temp.path().join("stderr-error"))
        .arg("--junit")
        .arg(temp.path().join("junit-false.xml"))
        .stderr(OpenOptions::new().write(true).open("/dev/full").unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success());

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stderr failure must not suppress JSON");
    assert_eq!(report["infrastructure_errors"][0]["phase"], "report_write");
}
