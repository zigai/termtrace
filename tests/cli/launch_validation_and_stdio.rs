//! launch validation and stdio CLI integration tests.

use crate::support::*;

#[test]
fn fitted_geometry_requires_a_viewport() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("missing-viewport");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["record", "--geometry", "fit", "--out"])
        .arg(session)
        .args(["--", "/bin/true"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--geometry fit requires --viewport WIDTHxHEIGHT",
        ));
}

#[test]
fn visual_convenience_flags_reject_invalid_values_before_recording() {
    let mut font_size = Command::cargo_bin("termtrace").expect("binary should build");
    font_size
        .args([
            "record",
            "--font-size",
            "0",
            "--out",
            "unused",
            "--",
            "/bin/true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "font size must be a positive finite number",
        ));

    let mut background = Command::cargo_bin("termtrace").expect("binary should build");
    background
        .args([
            "record",
            "--background",
            "black",
            "--out",
            "unused",
            "--",
            "/bin/true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("color must use #RRGGBB syntax"));
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

    let recording = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    let input_intent = recording
        .events
        .iter()
        .find_map(|record| match &record.event {
            Event::InputIntent {
                operation_id, data, ..
            } => return Some((*operation_id, data.as_bytes())),
            _ => return None,
        });
    let (operation_id, bytes) = input_intent.expect("input intent");
    assert_eq!(bytes, b"hello\n");
    assert!(recording.events.iter().any(|record| matches!(
        record.event,
        Event::InputResult {
            operation_id: result_id,
            status: OperationStatus::Applied,
            bytes_written: 6,
            error_code: None,
        } if result_id == operation_id
    )));
    assert!(recording.pending_operations.is_empty());
}
