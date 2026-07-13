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
        )));
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
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.args(["send", "--session"])
        .arg(&session)
        .args(["--text", "hello\n", "--intent", "complete shell read"])
        .assert()
        .success();

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
