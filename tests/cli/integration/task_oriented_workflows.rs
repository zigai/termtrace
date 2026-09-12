//! Task-oriented public CLI workflow tests.

use crate::support::*;
use termtrace_engine::{analysis::last_action, registry::SessionRegistry};
use termtrace_evidence::event_log::EventLog;

#[test]
fn doctor_and_wait_help_remain_directly_discoverable() {
    let mut doctor = Command::cargo_bin("termtrace").expect("binary should build");
    doctor
        .args(["--response", "full", "doctor", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""id":"semantic""#));

    let mut wait_help = Command::cargo_bin("termtrace").expect("binary should build");
    wait_help
        .args(["wait", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--screen <SCREEN>"))
        .stdout(predicate::str::contains("--screen-regex"))
        .stdout(predicate::str::contains("--output"))
        .stdout(predicate::str::contains("--idle"));
}

#[test]
fn start_act_and_screen_form_one_state_safe_workflow() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    let output = start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "start",
            "--until-screen",
            "READY",
            "--timeout",
            "5s",
            "workflow",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY; while :; do sleep 0.1; done",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let started: serde_json::Value =
        serde_json::from_slice(&output).expect("start response should be JSON");
    assert_eq!(started["postcondition"]["matched"], true);
    assert_eq!(started["postcondition"]["kind"], "screen");

    let request_id = "019fddda-17c8-7000-87da-8785e71ab5c8";
    let mut act = Command::cargo_bin("termtrace").expect("binary should build");
    let output = act
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "act",
            "workflow",
            "text:PING",
            "--request-id",
            request_id,
            "--until-screen",
            "PING",
            "--timeout",
            "5s",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let action: serde_json::Value =
        serde_json::from_slice(&output).expect("action response should be JSON");
    assert_eq!(action["request_id"], request_id);
    assert_eq!(action["input"]["applied"], "all");
    assert_eq!(action["input"]["atoms"], 1);
    assert_eq!(action["input"]["bytes"], 4);
    assert_eq!(action["postcondition"]["matched"], true);
    assert_eq!(action["postcondition"]["outcome"], "matched_unsettled");
    assert_eq!(
        action["observation"]["state_hash"].as_str().map(str::len),
        Some(64)
    );
    assert!(action["boundaries"]["observed"]["next_event_seq"].is_number());

    let mut retry = Command::cargo_bin("termtrace").expect("binary should build");
    let retry_output = retry
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "act",
            "workflow",
            "text:PING",
            "--request-id",
            request_id,
            "--until-screen",
            "PING",
            "--timeout",
            "5s",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let retried_action: serde_json::Value =
        serde_json::from_slice(&retry_output).expect("retried action response should be JSON");
    assert_eq!(retried_action, action);
    assert_eq!(action["ended"], false);

    let mut unmet = Command::cargo_bin("termtrace").expect("binary should build");
    let output = unmet
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "act",
            "workflow",
            "text:X",
            "--until-screen",
            "NEVER_APPEARS",
            "--timeout",
            "20ms",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let unmet: serde_json::Value =
        serde_json::from_slice(&output).expect("failed postcondition response should be JSON");
    assert_eq!(unmet["input"]["applied"], "all");
    assert_eq!(unmet["input"]["atoms"], 1);
    assert_eq!(unmet["input"]["bytes"], 1);
    assert_eq!(unmet["postcondition"]["matched"], false);
    assert_eq!(unmet["postcondition"]["outcome"], "deadline");
    assert_eq!(unmet["code"], "postcondition_failed");
    assert_eq!(
        unmet["observation"]["state"].as_str().map(str::len),
        Some(64)
    );
    assert!(unmet["request_id"].as_str().is_some());
    assert!(unmet["boundaries"]["observed"]["next_event_seq"].is_number());
    assert_eq!(unmet["ended"], false);

    let mut explicit_wait = Command::cargo_bin("termtrace").expect("binary should build");
    explicit_wait
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "wait",
            "workflow",
            "--screen",
            "PING",
            "--timeout",
            "1s",
        ])
        .assert()
        .success();

    let mut screen = Command::cargo_bin("termtrace").expect("binary should build");
    let output = screen
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "screen",
            "workflow",
            "--screen",
            "sparse",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let observed: serde_json::Value =
        serde_json::from_slice(&output).expect("screen response should be JSON");
    assert_eq!(observed["screen"]["encoding"], "rows");
    assert_eq!(observed["lifecycle"], "live");
    assert!(observed["screen"]["rows"]
        .as_array()
        .is_some_and(|rows| return !rows.is_empty()));

    let state_hash = observed["state"].as_str().expect("presented state hash");
    let mut unchanged = Command::cargo_bin("termtrace").expect("binary should build");
    unchanged
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "screen",
            "workflow",
            "--since",
            state_hash,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""unchanged":true"#));

    let mut show = Command::cargo_bin("termtrace").expect("binary should build");
    show.env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "workflow", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""source_kind":"live_session""#));

    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "send", "workflow", "--text", "Y"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""bytes_written":1"#));

    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "wait",
            "workflow",
            "--screen",
            "Y",
            "--timeout",
            "1s",
        ])
        .assert()
        .success();

    let mut changed = Command::cargo_bin("termtrace").expect("binary should build");
    changed
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "screen",
            "workflow",
            "--screen",
            "diff",
            "--since",
            state_hash,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""encoding":"diff_rows""#));

    let mut wait_idle = Command::cargo_bin("termtrace").expect("binary should build");
    wait_idle
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "wait",
            "workflow",
            "--idle",
            "10ms",
            "--timeout",
            "1s",
        ])
        .assert()
        .success();

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "stop", "workflow", "--wait"])
        .assert()
        .success();

    let mut completed_screen = Command::cargo_bin("termtrace").expect("binary should build");
    completed_screen
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "screen", "workflow"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"completed""#))
        .stdout(predicate::str::contains(r#""ended":true"#));
}

#[test]
fn child_completed_wait_returns_the_output_drained_frame() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "start",
            "drained-frame",
            "--",
            "/bin/sh",
            "-c",
            "sleep 3.5; dd if=/dev/zero bs=16384 count=8 2>/dev/null | tr '\\0' X; printf TAIL_SENTINEL",
        ])
        .assert()
        .success();

    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    let result = wait
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "wait",
            "drained-frame",
            "--screen",
            "NEVER_APPEARS",
            "--timeout",
            "15s",
        ])
        .assert()
        .code(1)
        .get_output()
        .clone();

    let output = &result.stdout;
    let waited: serde_json::Value = serde_json::from_slice(output).unwrap_or_else(|error| {
        panic!(
            "child-completed wait response: {error}; stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    });
    assert_eq!(waited["quality"], "child_completed");
    assert!(
        String::from_utf8_lossy(output).contains("TAIL_SENTINEL"),
        "terminating wait frame omitted queued PTY output: {waited}"
    );

    let mut completed = Command::cargo_bin("termtrace").expect("binary should build");
    let output = completed
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "screen", "drained-frame"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let completed: serde_json::Value =
        serde_json::from_slice(&output).expect("completed screen response");
    assert_eq!(waited["frame"]["state_hash"], completed["state"]);
    assert!(
        String::from_utf8_lossy(&output).contains("TAIL_SENTINEL"),
        "completed frame omitted sentinel: {completed}"
    );

    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "start",
            "drained-action",
            "--",
            "/bin/sh",
            "-c",
            "sleep 1.5; dd if=/dev/zero bs=16384 count=8 2>/dev/null | tr '\\0' Y; printf ACTION_TAIL",
        ])
        .assert()
        .success();

    let mut action = Command::cargo_bin("termtrace").expect("binary should build");
    let output = action
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "act",
            "drained-action",
            "text:Z",
            "--until-screen",
            "NEVER_APPEARS",
            "--timeout",
            "5s",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let action: serde_json::Value =
        serde_json::from_slice(&output).expect("child-completed action response");
    assert_eq!(action["postcondition"]["outcome"], "child_completed");
    assert!(
        String::from_utf8_lossy(&output).contains("ACTION_TAIL"),
        "terminating action frame omitted queued PTY output: {action}"
    );
}

#[test]
fn act_excludes_concurrent_mutations_until_its_postcondition_finishes() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("atomic-action");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'READY'); a=os.read(0,1); os.write(1,b'FIRST:'+a.hex().encode()); b=os.read(0,1); os.write(1,b'SECOND:'+b.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn atomic action fixture");

    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("fixture ready");

    let action = StdCommand::new(binary)
        .args(["--response", "full", "act"])
        .arg(&session)
        .args([
            "raw:A",
            "--until-screen",
            "NEVER_APPEARS",
            "--timeout",
            "5s",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn actor-ordered action");

    wait_for_output(&session, b"FIRST:41", Duration::from_secs(5))
        .expect("first action input reached fixture");

    let mut interleaved = Command::cargo_bin("termtrace").expect("binary should build");
    let rejected = interleaved
        .args(["--response", "full", "act"])
        .arg(&session)
        .arg("raw:B")
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let rejected: serde_json::Value =
        serde_json::from_slice(&rejected).expect("concurrent rejection JSON");
    assert_eq!(rejected["code"], "action_busy");
    assert_eq!(rejected["input"]["applied"], "none");

    let action_output = action.wait_with_output().expect("atomic action response");
    assert_eq!(action_output.status.code(), Some(1));

    let action: serde_json::Value =
        serde_json::from_slice(&action_output.stdout).expect("atomic action JSON");
    assert_eq!(action["input"]["applied"], "all");
    assert_eq!(action["input"]["atoms"], 1);
    assert_eq!(action["input"]["bytes"], 1);
    assert_eq!(action["postcondition"]["matched"], false);
    assert_eq!(action["postcondition"]["outcome"], "deadline");
    assert_eq!(action["ended"], false);
    assert!(action["observation"]["state_hash"].as_str().is_some());
    assert!(action["boundaries"]["observed"]["next_event_seq"].is_number());

    let mut after = Command::cargo_bin("termtrace").expect("binary should build");
    after
        .args(["--response", "full", "act"])
        .arg(&session)
        .arg("raw:B")
        .assert()
        .success();

    let child_output = child.wait_with_output().expect("atomic action fixture");
    assert!(
        child_output.status.success(),
        "atomic action fixture failed: {child_output:?}"
    );

    wait_for_output(&session, b"SECOND:42", Duration::from_secs(5))
        .expect("post-action input reached fixture");
}

#[test]
fn debug_and_handoff_facades_cover_the_last_canonical_action() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("facade-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "IFS= read -r line; printf '\\033[2J\\033[Hresult:%s' \"$line\"",
        ])
        .write_stdin("hello\n")
        .assert()
        .success();

    let mut inspect = Command::cargo_bin("termtrace").expect("binary should build");
    let inspection_bytes = inspect
        .args(["inspect"])
        .arg(&session)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let inspection: serde_json::Value =
        serde_json::from_slice(&inspection_bytes).expect("last-action inspection JSON");
    assert_eq!(inspection["format"], "termtrace-last-action");
    assert!(inspection["rows_changed"]
        .as_array()
        .is_some_and(|rows| return !rows.is_empty()));
    assert!(
        inspection["output_generated"]["byte_count"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(inspection["presentation_eligible"]
        .as_bool()
        .unwrap_or(false));
    assert!(inspection["warnings"].as_array().is_some());

    let mut diagnose = Command::cargo_bin("termtrace").expect("binary should build");
    let diagnostics_bytes = diagnose
        .args(["diagnose"])
        .arg(&session)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let diagnostics: serde_json::Value =
        serde_json::from_slice(&diagnostics_bytes).expect("last-action diagnostics JSON");
    assert_eq!(diagnostics["format"], "termtrace-last-action-diagnostics");
    assert_eq!(
        diagnostics["action"]["operation_ids"],
        inspection["action"]["operation_ids"]
    );
    assert!(diagnostics["findings"].as_array().is_some());

    let screenshot = temporary.path().join("screenshot");
    let mut screenshot_command = Command::cargo_bin("termtrace").expect("binary should build");
    screenshot_command
        .arg("screenshot")
        .arg(&session)
        .args(["-o"])
        .arg(&screenshot)
        .assert()
        .success();

    assert!(fs_err::read(screenshot.join("screen.png"))
        .expect("screenshot bytes")
        .starts_with(b"\x89PNG\r\n\x1a\n"));

    let bundle = temporary.path().join("facade-bundle");
    let mut bundle_command = Command::cargo_bin("termtrace").expect("binary should build");
    bundle_command
        .arg("bundle")
        .arg(&session)
        .args(["-o"])
        .arg(&bundle)
        .assert()
        .success();

    let bundle_manifest: serde_json::Value = serde_json::from_slice(
        &fs_err::read(bundle.join("manifest.json")).expect("bundle manifest"),
    )
    .expect("bundle manifest JSON");
    assert_eq!(bundle_manifest["profile"], "screen");
    assert_eq!(bundle_manifest["includes_canonical_evidence"], false);

    let bug_report = temporary.path().join("bug-report");
    let mut bug_report_command = Command::cargo_bin("termtrace").expect("binary should build");
    bug_report_command
        .arg("bug-report")
        .arg(&session)
        .args(["-o"])
        .arg(&bug_report)
        .assert()
        .success();

    for member in [
        "metadata.json",
        "last-screen.txt",
        "last-action.json",
        "diagnostics.json",
        "session-reference.json",
        "manifest.json",
    ] {
        assert!(bug_report.join(member).is_file(), "missing {member}");
    }

    assert!(!bug_report.join("events.jsonl").exists());
    assert!(!bug_report.join("screenshot.png").exists());

    let report_manifest: serde_json::Value = serde_json::from_slice(
        &fs_err::read(bug_report.join("manifest.json")).expect("bug-report manifest"),
    )
    .expect("bug-report manifest JSON");
    assert_eq!(report_manifest["format"], "termtrace-bug-report");
    assert_eq!(report_manifest["includes_raw_streams"], false);
    assert_eq!(report_manifest["includes_screenshot"], false);
    assert_eq!(report_manifest["includes_last_action"], true);
    assert_eq!(report_manifest["members"].as_array().map_or(0, Vec::len), 5);

    let bug_report_with_screenshot = temporary.path().join("bug-report-with-screenshot");
    let mut screenshot_report = Command::cargo_bin("termtrace").expect("binary should build");
    screenshot_report
        .arg("bug-report")
        .arg(&session)
        .args(["-o"])
        .arg(&bug_report_with_screenshot)
        .arg("--screenshot")
        .assert()
        .success();

    assert!(
        fs_err::read(bug_report_with_screenshot.join("screenshot.png"))
            .expect("bug-report screenshot")
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );
}

#[test]
fn bug_report_supports_completed_sessions_without_input_actions() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("no-action-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf STARTUP_FAILURE"])
        .assert()
        .success();

    let report = temporary.path().join("no-action-report");
    let mut bug_report = Command::cargo_bin("termtrace").expect("binary should build");
    bug_report
        .arg("bug-report")
        .arg(&session)
        .args(["-o"])
        .arg(&report)
        .assert()
        .success();

    for member in [
        "metadata.json",
        "last-screen.txt",
        "session-reference.json",
        "manifest.json",
    ] {
        assert!(report.join(member).is_file(), "missing {member}");
    }

    assert!(!report.join("last-action.json").exists());
    assert!(!report.join("diagnostics.json").exists());

    let manifest: serde_json::Value = serde_json::from_slice(
        &fs_err::read(report.join("manifest.json")).expect("no-action bug-report manifest"),
    )
    .expect("no-action bug-report manifest JSON");
    assert_eq!(manifest["version"], 2);
    assert_eq!(manifest["includes_last_action"], false);
    assert_eq!(manifest["members"].as_array().map_or(0, Vec::len), 3);
}

#[test]
fn last_action_uses_the_full_journaled_multi_atom_request_boundary() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let request_id = "019fddda-17c8-7000-87da-8785e71ab5d8";
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "start",
            "--until-screen",
            "READY",
            "--timeout",
            "5s",
            "sequence",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY; while IFS= read -r line; do printf '\\r\\nRESULT:%s' \"$line\"; done",
        ])
        .assert()
        .success();

    let mut act = Command::cargo_bin("termtrace").expect("binary should build");
    act.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "act",
            "sequence",
            "text:hello",
            "enter",
            "--request-id",
            request_id,
            "--until-screen",
            "RESULT:hello",
            "--timeout",
            "5s",
        ])
        .assert()
        .success();

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "stop", "sequence", "--wait"])
        .assert()
        .success();

    let mut inspect = Command::cargo_bin("termtrace").expect("binary should build");
    let output = inspect
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["inspect", "sequence"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value =
        serde_json::from_slice(&output).expect("multi-atom last-action report");

    let session = SessionRegistry::open(&state)
        .expect("registry")
        .resolve("sequence")
        .expect("recording")
        .session_path();

    let log = EventLog::read(session.join("events.jsonl")).expect("canonical source");
    let legacy = last_action::analyze_last_action(&session, &log, 50)
        .expect("materialized last-action analysis");

    let streamed = last_action::analyze_last_action_session(&session, 50)
        .expect("streamed last-action analysis");
    assert_eq!(
        serde_json::to_value(&streamed).expect("streamed JSON"),
        serde_json::to_value(&legacy).expect("materialized JSON")
    );
    assert_eq!(
        report,
        serde_json::to_value(&legacy.inspection).expect("inspection JSON")
    );
    assert_eq!(report["action"]["request_id"], request_id);
    assert_eq!(report["action"]["atom_count"], 2);
    assert_eq!(report["action"]["intended_bytes"], 6);
    assert_eq!(
        report["action"]["operation_ids"]
            .as_array()
            .map_or(0, Vec::len),
        2
    );
    assert!(report["action"]["request_event_start_seq"]
        .as_u64()
        .zip(report["action"]["intent_event_seq"].as_u64())
        .is_some_and(|(start, intent)| return start <= intent));
    assert!(report["action"]["result_event_seq"]
        .as_u64()
        .zip(report["action"]["request_event_end_seq"].as_u64())
        .is_some_and(|(result, end)| return result < end));
    assert!(report["rows_changed"]
        .as_array()
        .is_some_and(|rows| return !rows.is_empty()));
}

#[test]
fn last_action_uses_the_full_journaled_multi_atom_action_boundary() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let request_id = "019fddda-17c8-7000-87da-8785e71ab5d9";
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "start",
            "--until-screen",
            "READY",
            "--timeout",
            "5s",
            "sequence-send",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY; while IFS= read -r line; do printf '\\r\\nRESULT:%s' \"$line\"; done",
        ])
        .assert()
        .success();

    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "act",
            "sequence-send",
            "text:world",
            "enter",
            "--request-id",
            request_id,
        ])
        .assert()
        .success();

    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "wait",
            "sequence-send",
            "--screen",
            "RESULT:world",
            "--timeout",
            "5s",
        ])
        .assert()
        .success();

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "stop", "sequence-send", "--wait"])
        .assert()
        .success();

    let mut inspect = Command::cargo_bin("termtrace").expect("binary should build");
    let output = inspect
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["inspect", "sequence-send"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value =
        serde_json::from_slice(&output).expect("multi-atom action last-action report");
    assert_eq!(report["action"]["request_id"], request_id);
    assert_eq!(report["action"]["atom_count"], 2);
    assert_eq!(report["action"]["intended_bytes"], 6);
    assert_eq!(
        report["action"]["operation_ids"]
            .as_array()
            .map_or(0, Vec::len),
        2
    );
    assert!(report["action"]["intent_event_seq"]
        .as_u64()
        .zip(report["action"]["result_event_seq"].as_u64())
        .is_some_and(|(intent, result)| return intent < result));
}
