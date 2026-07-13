//! End-to-end tests for the public command-line interface.

use assert_cmd::Command;
use predicates::prelude::*;
use std::{
    fs::Permissions,
    io::{Read as _, Write as _},
    os::unix::fs::PermissionsExt as _,
    path::Path,
    process::{Command as StdCommand, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use termtrace::{
    Error,
    control::{ControlCommand, ControlRequest, send_request},
    event::{Event, InputSource, OperationStatus},
    event_log::EventLog,
    replay::{GHOSTTY_REVISION, replay},
    session::{CaptureTarget, GeometrySource, read_manifest},
};

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
            GHOSTTY_REVISION
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
        .args(["--viewport", "800x600"])
        .args(["--", "/bin/sh", "-c", "printf recorded; exit 7"])
        .assert()
        .code(7)
        .stdout("recorded")
        .stderr(predicate::str::contains("recording:"));

    let event_log = session.join("events.jsonl");
    assert!(event_log.is_file(), "event log should be persisted");
    let recording = EventLog::read(event_log).expect("valid event log");
    assert!(matches!(
        recording.events.last().map(|record| return &record.event),
        Some(Event::Exit { code: Some(7), .. })
    ));
    let manifest = read_manifest(&session).expect("session manifest");
    assert_eq!(manifest.geometry_provenance.source, GeometrySource::Manual);
    assert_eq!(
        manifest.geometry_provenance.capture_target,
        Some(CaptureTarget {
            width: 800,
            height: 600,
        })
    );
}

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

    let duplicate_request = ControlRequest::new(ControlCommand::Mark {
        label: "deduplicated".to_owned(),
    });
    send_request(&session, &duplicate_request).expect("first marker request");
    send_request(&session, &duplicate_request).expect("duplicate should return cached response");
    let collision = ControlRequest {
        version: duplicate_request.version,
        request_id: duplicate_request.request_id,
        command: ControlCommand::Ping,
    };
    assert!(matches!(
        send_request(&session, &collision),
        Err(Error::ControlRejected { code, .. })
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

    let recording = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    assert_control_events(&recording);
}

fn assert_control_events(recording: &EventLog) {
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
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
                Event::Marker { label } if label == "deduplicated"
            ))
            .count(),
        1
    );
    assert!(recording.events.iter().any(|record| matches!(
        record.event,
        Event::ResizeIntent {
            cols: 100,
            rows: 30,
            ..
        }
    )));
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::Marker { label } if label == "before-input"
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

    let event_log = EventLog::read(session.join("events.jsonl")).expect("valid event log");
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
    let first = replay(&event_log).expect("first replay");
    let second = replay(&event_log).expect("second replay");
    assert_eq!(first, second, "replay must be deterministic");
    assert!(first.states.iter().any(|state| return !state.presented));
    assert!(first.states.iter().any(|state| {
        return state
            .delta
            .modes
            .as_ref()
            .is_some_and(|modes| return modes.alternate_screen);
    }));
    assert!(
        first
            .states
            .windows(2)
            .all(|pair| return pair[0].state_hash != pair[1].state_hash)
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

#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helper failures should stop at the failed artifact assertion"
)]
fn assert_replay_artifacts(session: &Path, expected_states: u64) {
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

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
fn validate_json_file(schema: &str, instance: &Path) {
    let value = serde_json::from_slice(&fs_err::read(instance).expect("instance file"))
        .expect("instance JSON");
    validate_json_value(schema, &value);
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
fn validate_json_lines(schema: &str, instance: &Path) {
    for line in fs_err::read_to_string(instance)
        .expect("JSONL instance")
        .lines()
    {
        validate_json_value(schema, &serde_json::from_str(line).expect("JSONL value"));
    }
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
fn validate_json_value(schema: &str, instance: &serde_json::Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("schemas")
        .join(schema);
    let schema_value: serde_json::Value =
        serde_json::from_slice(&fs_err::read(path).expect("schema file")).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema_value).expect("valid schema");
    if let Err(error) = validator.validate(instance) {
        panic!("{schema} rejected instance: {error}");
    }
}

#[test]
fn playback_does_not_echo_terminal_query_responses() {
    use std::collections::BTreeMap;

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use termtrace::{
        event::{ByteString, Event, EventLogHeader},
        event_log::{EventLogWriter, SessionClock},
        session::{
            GeometryProvenance, SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION, SessionGeometry,
            SessionManifest,
        },
    };
    use uuid::Uuid;

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("playback");
    fs_err::create_dir(&session).expect("session directory");
    fs_err::set_permissions(&session, Permissions::from_mode(0o700)).expect("private session");
    let clock = SessionClock::now().expect("session clock");
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        clock.started_unix_ns(),
        80,
        24,
        720,
        432,
        "xterm-ghostty",
    );
    let mut event_log =
        EventLogWriter::create(session.join("events.jsonl"), &header, clock).expect("event log");
    let query = b"\x1b]11;?\x1b\\";
    event_log
        .append(Event::Output {
            data: ByteString::from(query.as_slice()),
        })
        .expect("query output");
    event_log
        .append(Event::Exit {
            code: Some(0),
            signal: None,
            signal_name: None,
        })
        .expect("exit event");
    drop(event_log);
    let manifest = SessionManifest {
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id: header.session_id,
        started_unix_ns: header.started_unix_ns,
        command: Vec::new(),
        working_directory: None,
        geometry: SessionGeometry {
            cols: 80,
            rows: 24,
            pixel_width: 720,
            pixel_height: 432,
        },
        geometry_provenance: GeometryProvenance::default(),
        ghostty_visual: None,
        child_environment: BTreeMap::new(),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: "linux".to_owned(),
        architecture: "test".to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: "control.sock".to_owned(),
        contains_sensitive_terminal_data: true,
    };
    fs_err::write(
        session.join("session.json"),
        serde_json::to_vec(&manifest).expect("session manifest JSON"),
    )
    .expect("session manifest");

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 720,
            pixel_height: 432,
        })
        .expect("playback PTY");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termtrace"));
    let timing_file = temporary.path().join("timing.json");
    let ready_file = temporary.path().join("ready");
    command.args([
        "playback",
        "--session",
        session.to_str().expect("UTF-8 session"),
        "--timing-file",
        timing_file.to_str().expect("UTF-8 timing"),
        "--ready-file",
        ready_file.to_str().expect("UTF-8 ready"),
        "--start-delay-ms",
        "200",
        "--final-hold-ms",
        "1000",
    ]);
    let mut child = pty.slave.spawn_command(command).expect("spawn playback");
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        return output;
    });
    let mut writer = pty.master.take_writer().expect("take PTY writer");
    writer
        .write_all(b"\x1b[4;432;720t\x1b[6;18;9t\x1b[8;24;80t")
        .expect("write geometry responses");
    thread::sleep(Duration::from_millis(400));
    writer
        .write_all(b"HOST-RESPONSE\n")
        .expect("write simulated host response");
    child.wait().expect("wait for playback");
    drop(writer);
    drop(pty.master);
    let output = reader_thread.join().expect("join PTY reader");

    assert!(
        output
            .windows(query.len())
            .any(|window| return window == query)
    );
    assert!(
        !output
            .windows(b"HOST-RESPONSE".len())
            .any(|window| return window == b"HOST-RESPONSE"),
        "playback echoed a capture-host response: {output:?}"
    );
}
