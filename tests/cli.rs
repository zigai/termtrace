//! End-to-end tests for the public command-line interface.

use assert_cmd::Command;
use predicates::prelude::*;
use std::{
    process::{Command as StdCommand, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

#[test]
fn version_subcommand_reports_package_version() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains(concat!(
            "termtrace ",
            env!("CARGO_PKG_VERSION")
        )))
        .stdout(predicate::str::contains(format!(
            "libghostty-vt {}",
            termtrace::replay::GHOSTTY_REVISION
        )))
        .stdout(predicate::str::contains("event-log v1"));
}

#[test]
fn record_mirrors_output_persists_events_and_propagates_exit_code() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["record", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf recorded; exit 7"])
        .assert()
        .code(7)
        .stdout("recorded")
        .stderr(predicate::str::contains("recording:"));

    let event_log = session.join("events.jsonl");
    assert!(event_log.is_file(), "event log should be persisted");
    let recording = termtrace::event_log::EventLog::read(event_log).expect("valid event log");
    assert!(matches!(
        recording.events.last().map(|record| &record.event),
        Some(termtrace::event::Event::Exit { code: Some(7), .. })
    ));
}

#[test]
fn record_forwards_and_audits_standard_input() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("interactive");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["record", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "IFS= read -r line; printf '<%s>' \"$line\"",
        ])
        .write_stdin("hello\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("<hello>"));

    let recording = termtrace::event_log::EventLog::read(session.join("events.jsonl"))
        .expect("valid event log");
    let input_intent = recording
        .events
        .iter()
        .find_map(|record| match &record.event {
            termtrace::event::Event::InputIntent {
                operation_id, data, ..
            } => Some((*operation_id, data.as_bytes())),
            _ => None,
        });
    let (operation_id, bytes) = input_intent.expect("input intent");
    assert_eq!(bytes, b"hello\n");
    assert!(recording.events.iter().any(|record| matches!(
        record.event,
        termtrace::event::Event::InputResult {
            operation_id: result_id,
            status: termtrace::event::OperationStatus::Applied,
            bytes_written: 6,
            error_code: None,
        } if result_id == operation_id
    )));
    assert!(recording.pending_operations.is_empty());
}

#[test]
fn live_control_drives_and_audits_a_detached_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("controlled");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "IFS= read -r line; printf 'agent:%s' \"$line\"",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");
    let socket = session.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "control socket did not become ready");

    let duplicate_request =
        termtrace::control::ControlRequest::new(termtrace::control::ControlCommand::Mark {
            label: "deduplicated".to_owned(),
        });
    termtrace::control::send_request(&session, &duplicate_request).expect("first marker request");
    termtrace::control::send_request(&session, &duplicate_request)
        .expect("duplicate should return cached response");
    let collision = termtrace::control::ControlRequest {
        version: duplicate_request.version,
        request_id: duplicate_request.request_id,
        command: termtrace::control::ControlCommand::Ping,
    };
    assert!(matches!(
        termtrace::control::send_request(&session, &collision),
        Err(termtrace::Error::ControlRejected { ref code, .. })
            if code == "request_id_collision"
    ));

    for arguments in [
        vec!["ping", "--session", session.to_str().expect("UTF-8 path")],
        vec!["status", "--session", session.to_str().expect("UTF-8 path")],
        vec![
            "mark",
            "--session",
            session.to_str().expect("UTF-8 path"),
            "--label",
            "before-input",
        ],
        vec![
            "resize",
            "--session",
            session.to_str().expect("UTF-8 path"),
            "--cols",
            "100",
            "--rows",
            "30",
        ],
    ] {
        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        command.args(arguments).assert().success();
    }
    let waiter = StdCommand::new(binary)
        .args([
            "wait",
            "--session",
            session.to_str().expect("UTF-8 path"),
            "--text",
            "agent:hello",
            "--timeout-ms",
            "5000",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn output waiter");
    thread::sleep(Duration::from_millis(50));
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.args(["send", "--session"])
        .arg(&session)
        .args(["--text", "hello\n", "--intent", "complete shell read"])
        .assert()
        .success();

    let wait_output = waiter.wait_with_output().expect("wait for output waiter");
    assert!(wait_output.status.success(), "wait failed: {wait_output:?}");

    let output = child.wait_with_output().expect("wait for recorder");
    assert!(output.status.success(), "recorder failed: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("agent:hello"),
        "agent input did not reach child: {output:?}"
    );
    assert!(!socket.exists(), "control socket should be removed on exit");

    let recording = termtrace::event_log::EventLog::read(session.join("events.jsonl"))
        .expect("valid event log");
    assert_control_events(&recording);
}

fn assert_control_events(recording: &termtrace::event_log::EventLog) {
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        termtrace::event::Event::InputIntent {
            source: termtrace::event::InputSource::Agent,
            intent: Some(intent),
            ..
        } if intent == "complete shell read"
    )));
    assert_eq!(
        recording
            .events
            .iter()
            .filter(|record| matches!(
                &record.event,
                termtrace::event::Event::Marker { label } if label == "deduplicated"
            ))
            .count(),
        1
    );
    assert!(recording.events.iter().any(|record| matches!(
        record.event,
        termtrace::event::Event::ResizeIntent {
            cols: 100,
            rows: 30,
            ..
        }
    )));
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        termtrace::event::Event::Marker { label } if label == "before-input"
    )));
    assert!(recording.pending_operations.is_empty());
}

#[test]
fn index_replays_every_distinct_state_deterministically() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("replay");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r"printf '\033[?2026hA\033[38;2;1;2;3mB\033[?2026l界\n\033[?1049hALT\033[?1049l'",
        ])
        .assert()
        .success();

    let event_log = termtrace::event_log::EventLog::read(session.join("events.jsonl"))
        .expect("valid event log");
    let cast = temporary.path().join("session.cast");
    let mut export = Command::cargo_bin("termtrace").expect("binary should build");
    export
        .args(["export-asciicast", "--session"])
        .arg(&session)
        .args(["--out"])
        .arg(&cast)
        .assert()
        .success();
    let cast_text = fs_err::read_to_string(&cast).expect("asciicast");
    let mut cast_lines = cast_text.lines();
    let cast_header: serde_json::Value =
        serde_json::from_str(cast_lines.next().expect("cast header")).expect("cast header JSON");
    assert_eq!(cast_header["version"], 2);
    assert!(cast_lines.next().is_some(), "cast should contain output");
    let first = termtrace::replay::replay(&event_log).expect("first replay");
    let second = termtrace::replay::replay(&event_log).expect("second replay");
    assert_eq!(first, second, "replay must be deterministic");
    assert!(first.states.iter().any(|state| !state.presented));
    assert!(first.states.iter().any(|state| {
        state
            .delta
            .modes
            .as_ref()
            .is_some_and(|modes| modes.alternate_screen)
    }));
    assert!(
        first
            .states
            .windows(2)
            .all(|pair| pair[0].state_hash != pair[1].state_hash)
    );

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["index", "--session"])
        .arg(&session)
        .assert()
        .success();
    assert_replay_artifacts(&session, first.artifacts.state_count);
    let mut inspect_text = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_text
        .args(["inspect", "--session"])
        .arg(&session)
        .args(["--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("AB界"));
    let mut inspect_initial = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_initial
        .args(["inspect", "--session"])
        .arg(&session)
        .args(["--state", "0", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""state_id": 0"#));

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .args(["index", "--session"])
        .arg(&session)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

fn assert_replay_artifacts(session: &std::path::Path, expected_states: u64) {
    use std::os::unix::fs::PermissionsExt as _;

    let derived = session.join("derived");
    assert_eq!(
        fs_err::metadata(&derived)
            .expect("derived metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for file in [
        "states.jsonl",
        "state-index.jsonl",
        "final.txt",
        "final.vt",
        "final.html",
        "manifest.json",
    ] {
        let path = derived.join(file);
        assert!(path.is_file(), "missing {}", path.display());
        assert_eq!(
            fs_err::metadata(path)
                .expect("artifact metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let lines = fs_err::read_to_string(derived.join("states.jsonl"))
        .expect("states")
        .lines()
        .count();
    assert_eq!(u64::try_from(lines).unwrap(), expected_states);
    let text = fs_err::read_to_string(derived.join("final.txt")).expect("final text");
    assert!(text.contains("AB界"));
    assert!(
        !fs_err::read(derived.join("final.vt"))
            .expect("final VT")
            .is_empty()
    );
    let html = fs_err::read_to_string(derived.join("final.html")).expect("final HTML");
    assert!(html.contains('A') && html.contains('B'));

    validate_json_file("session.schema.json", &session.join("session.json"));
    validate_json_file("completion.schema.json", &session.join("completion.json"));
    validate_json_file(
        "replay-manifest.schema.json",
        &derived.join("manifest.json"),
    );
    validate_json_lines("state-record.schema.json", &derived.join("states.jsonl"));
    validate_json_lines(
        "state-index-entry.schema.json",
        &derived.join("state-index.jsonl"),
    );
    let event_log = fs_err::read_to_string(session.join("events.jsonl")).expect("event log");
    let mut event_lines = event_log.lines();
    validate_json_value(
        "event-log-header.schema.json",
        &serde_json::from_str(event_lines.next().expect("event header")).expect("header JSON"),
    );
    for line in event_lines {
        validate_json_value(
            "event-record.schema.json",
            &serde_json::from_str(line).expect("event JSON"),
        );
    }
}

fn validate_json_file(schema: &str, instance: &std::path::Path) {
    let value = serde_json::from_slice(&fs_err::read(instance).expect("instance file"))
        .expect("instance JSON");
    validate_json_value(schema, &value);
}

fn validate_json_lines(schema: &str, instance: &std::path::Path) {
    for line in fs_err::read_to_string(instance)
        .expect("JSONL instance")
        .lines()
    {
        validate_json_value(schema, &serde_json::from_str(line).expect("JSONL value"));
    }
}

fn validate_json_value(schema: &str, instance: &serde_json::Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("schemas")
        .join(schema);
    let schema_value: serde_json::Value =
        serde_json::from_slice(&fs_err::read(path).expect("schema file")).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema_value).expect("valid schema");
    if let Err(error) = validator.validate(instance) {
        panic!("{schema} rejected instance: {error}");
    }
}
