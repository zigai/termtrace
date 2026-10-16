//! output events and postprocessing CLI integration tests.

use crate::support::*;
use std::{process::Child, sync::mpsc::channel};
use termtrace_engine::timeline::resolve_session_prefix_source_selection;

#[test]
fn stripped_output_preserves_utf8_continuation_control_bytes() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("unicode-output");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/printf", "\\311\\200"])
        .assert()
        .success();

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["analyze", "output"])
        .arg(&session)
        .arg("--strip-controls")
        .assert()
        .success()
        .stdout(predicate::eq(b"\xc9\x80".as_slice()));
}

#[test]
fn output_raw_reproduces_exact_canonical_child_bytes() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("raw-output-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf 'A\\033[?1049h\\033[31mB\\033[0m\\377C'",
        ])
        .assert()
        .success();

    let log = EventLog::read(session.join("events.jsonl")).expect("canonical log");
    let expected = canonical_output_bytes(&log);
    assert_eq!(expected, b"A\x1b[?1049h\x1b[31mB\x1b[0m\xffC");

    let mut output = Command::cargo_bin("termtrace").expect("binary should build");
    output
        .args(["analyze", "output"])
        .arg(&session)
        .arg("--raw")
        .assert()
        .success()
        .stdout(predicate::eq(expected));

    let (first, selected_bytes) = log
        .events
        .iter()
        .find_map(|record| {
            return match &record.event {
                Event::Output { data } => Some((record.seq, data.as_bytes())),
                _ => None,
            };
        })
        .expect("output event");

    let mut bounded = Command::cargo_bin("termtrace").expect("binary should build");
    bounded
        .args(["analyze", "output"])
        .arg(&session)
        .args([
            "--raw",
            "--from-event-seq",
            &first.to_string(),
            "--to-event-seq",
            &first.saturating_add(1).to_string(),
        ])
        .assert()
        .success()
        .stdout(predicate::eq(selected_bytes));

    let projection_manifest = temporary.path().join("output-projection.json");
    let mut stripped = Command::cargo_bin("termtrace").expect("binary should build");
    stripped
        .args(["analyze", "output"])
        .arg(&session)
        .args(["--strip-controls", "--manifest"])
        .arg(&projection_manifest)
        .assert()
        .success()
        .stdout(predicate::eq(b"AB\xffC".as_slice()))
        .stderr(predicate::str::contains(
            "projection_quality=unsuitable_for_full_screen",
        ));

    let projection = fs_err::read_to_string(projection_manifest).expect("projection report");
    assert!(projection.contains(r#""projection_quality": "unsuitable_for_full_screen""#));
    assert!(projection.contains(r#""alternate_screen_observed": true"#));
}

#[test]
fn output_follow_emits_each_durable_prefix_once_until_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("follow-output-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let recorder = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--durability",
            "strict",
            "--",
            "/bin/sh",
            "-c",
            "printf A; sleep 0.10; printf '\\033['; sleep 0.10; printf '31mB\\377'",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");

    wait_for_control(&session);
    let follower = StdCommand::new(binary)
        .args(["analyze", "output"])
        .arg(&session)
        .args(["--raw", "--follow"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn output follower");

    let recorder_output = recorder.wait_with_output().expect("wait for recorder");
    assert!(recorder_output.status.success(), "{recorder_output:?}");

    let followed = follower.wait_with_output().expect("wait for follower");
    assert!(followed.status.success(), "{followed:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("completed event log");
    let expected = canonical_output_bytes(&log);
    assert_eq!(expected, b"A\x1b[31mB\xff");
    assert_eq!(followed.stdout, expected);

    let stripped = StdCommand::new(binary)
        .args(["analyze", "output"])
        .arg(&session)
        .args(["--strip-controls", "--follow"])
        .output()
        .expect("follow completed stripped output");
    assert!(stripped.status.success(), "{stripped:?}");
    assert_eq!(stripped.stdout, b"AB\xff");
    assert!(String::from_utf8_lossy(&stripped.stderr).contains("lossy_line_oriented"));
}

#[test]
fn events_filters_typed_records_and_summarizes_sensitive_bytes() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("event-stream-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf secret-bytes"])
        .assert()
        .success();

    let mut metadata = Command::cargo_bin("termtrace").expect("binary should build");
    let output = metadata
        .args(["analyze", "events"])
        .arg(&session)
        .args(["--type", "output", "--metadata-only"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!output
        .windows(b"secret-bytes".len())
        .any(|w| return w == b"secret-bytes"));

    let mut byte_count = 0;
    for line in output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
    {
        let record: serde_json::Value = serde_json::from_slice(line).expect("metadata event JSON");
        assert_eq!(record["event_type"], "output");
        assert_eq!(record["metadata_only"], true);
        assert!(record["event"].is_null());

        let payloads = record["byte_payloads"]
            .as_array()
            .expect("byte payload metadata");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0]["field"], "data");

        byte_count += payloads[0]["byte_count"]
            .as_u64()
            .expect("output byte count");
    }

    assert_eq!(byte_count, b"secret-bytes".len() as u64);

    let mut exact = Command::cargo_bin("termtrace").expect("binary should build");
    let exact_output = exact
        .args(["analyze", "events"])
        .arg(&session)
        .args(["--type", "output"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let exact_line = exact_output
        .split(|byte| return *byte == b'\n')
        .find(|line| return !line.is_empty())
        .expect("exact event JSON line");

    let exact_record: serde_json::Value =
        serde_json::from_slice(exact_line).expect("exact event JSON");
    assert_eq!(exact_record["metadata_only"], false);
    assert_eq!(exact_record["event_type"], "output");
    assert_eq!(exact_record["event"]["type"], "output");
}

#[test]
fn events_follow_resumes_filtered_durable_records_without_duplicates() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("follow-events-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let recorder = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--durability",
            "strict",
            "--",
            "/bin/sh",
            "-c",
            "printf first; sleep 0.40; printf second",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");

    wait_for_control(&session);
    let follower = StdCommand::new(binary)
        .args(["analyze", "events"])
        .arg(&session)
        .args(["--follow", "--type", "output", "--metadata-only"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn event follower");

    assert!(recorder
        .wait_with_output()
        .expect("wait for recorder")
        .status
        .success());

    let followed = follower.wait_with_output().expect("wait for follower");
    assert!(followed.status.success(), "{followed:?}");

    let projected = followed
        .stdout
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| return serde_json::from_slice::<serde_json::Value>(line).expect("projection"))
        .collect::<Vec<_>>();

    let log = EventLog::read(session.join("events.jsonl")).expect("completed event log");
    let expected_sequences = log
        .events
        .iter()
        .filter(|record| return matches!(record.event, Event::Output { .. }))
        .map(|record| return record.seq)
        .collect::<Vec<_>>();

    let projected_sequences = projected
        .iter()
        .map(|value| return value["seq"].as_u64().expect("sequence"))
        .collect::<Vec<_>>();
    assert_eq!(projected_sequences, expected_sequences);
    assert!(projected.iter().all(|value| {
        return value["event_type"] == "output" && value["metadata_only"] == true;
    }));
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration test helper stops when the recording fixture cannot run"
)]
fn start_event_follow_recorder(session: &Path) -> Child {
    let mut command = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"));
    command
        .args(["record", "--no-input", "--out"])
        .arg(session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 0.1; done",
        ]);
    let recorder = command.spawn().expect("spawn recorder");

    wait_for_control(session);

    return recorder;
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration test helper stops when the recording fixture cannot run"
)]
fn follow_test_marker(session: &Path, label: &str) -> u64 {
    let data = send_request(
        session,
        &ControlRequest::new(ControlCommand::Mark {
            label: label.to_owned(),
        }),
    )
    .expect("durable marker");

    let ControlPayload::Marked { event_seq, .. } = data else {
        panic!("unexpected marker response");
    };

    return event_seq;
}

#[test]
fn events_follow_recent_actor_cursor_delivers_markers_and_completed_transition() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("recent-follow");
    let recorder = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,signal,sys,tty\nsignal.signal(signal.SIGTERM,lambda *_: sys.exit(0))\ntty.setraw(0)\nfor _ in range(1100):\n os.write(1,b'\\x1b[6n')\n response=b''\n while not response.endswith(b'R'): response+=os.read(0,64)\nos.write(1,b'PREFIX_READY')\nwhile True: os.read(0,4096)",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn query-heavy prefix recorder");

    wait_for_control(&session);
    wait_for_output(&session, b"PREFIX_READY", Duration::from_secs(30))
        .expect("canonical prefix exceeds retained cursor history");

    let ControlPayload::Status { boundaries, .. } =
        send_request(&session, &ControlRequest::new(ControlCommand::Status)).expect("status")
    else {
        panic!("status response");
    };
    let resume = boundaries.durable.next_event_seq;
    let race_marker = follow_test_marker(&session, "between-status-and-follow");

    let ControlPayload::EventCursor {
        start: Some(start),
        durable,
        ..
    } = send_request(
        &session,
        &ControlRequest::new(ControlCommand::EventCursor {
            next_event_seq: resume,
        }),
    )
    .expect("actor resume authority")
    else {
        panic!("recent cursor must survive writer advancement");
    };

    assert_eq!(start.next_event_seq, resume);
    assert!(start.file_offset > 1_000_000);
    assert!(durable.next_event_seq > start.next_event_seq);

    // Retention eviction selects explicit historical streaming, not an unauthenticated seek.
    assert!(matches!(
        send_request(
            &session,
            &ControlRequest::new(ControlCommand::EventCursor { next_event_seq: 0 })
        )
        .expect("historical cursor"),
        ControlPayload::EventCursor { start: None, .. }
    ));

    let historical = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["analyze", "events"])
        .arg(&session)
        .args(["--follow", "--from-event-seq", "0", "--to-event-seq", "1"])
        .output()
        .expect("historical follow");
    assert!(historical.status.success(), "{historical:?}");

    let first: serde_json::Value = serde_json::from_slice(&historical.stdout).expect("first event");
    assert_eq!(first["seq"], 0);

    let mut follower = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["analyze", "events"])
        .arg(&session)
        .args([
            "--follow",
            "--from-event-seq",
            &resume.to_string(),
            "--type",
            "marker",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn follower");
    let stdout = follower.stdout.take().expect("follower stdout");
    let (sender, receiver) = channel();

    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let value: serde_json::Value =
                serde_json::from_str(&line.expect("follow line")).expect("projection");
            if sender.send(value).is_err() {
                break;
            }
        }
    });

    let raced = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("retained marker delivery");
    assert_eq!(raced["seq"], race_marker);

    let fresh = follow_test_marker(&session, "fresh-after-follow-start");
    let delivered = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("new durable marker delivery");
    assert_eq!(delivered["seq"], fresh);

    send_request(&session, &ControlRequest::new(ControlCommand::Stop)).expect("stop recorder");

    let recorded = recorder.wait_with_output().expect("recorder completion");
    assert!(recorded.status.success(), "{recorded:?}");

    let followed = follower.wait_with_output().expect("follow completion");
    assert!(followed.status.success(), "{followed:?}");

    reader.join().expect("reader completion");

    assert!(receiver.try_iter().next().is_none(), "no duplicate markers");

    validate_completed_session(&session).expect("valid completed transition");
}

#[test]
fn events_follow_rejects_self_hashed_suffix_before_publishing() {
    use std::io::{Seek as _, SeekFrom};
    use termtrace_model::execution::event::{EventBody, EventRecord};

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("tampered-follow");
    let recorder = start_event_follow_recorder(&session);
    let resume = follow_test_marker(&session, "prior") + 1;
    follow_test_marker(&session, "genuine");

    let ControlPayload::EventCursor {
        start: Some(start), ..
    } = send_request(
        &session,
        &ControlRequest::new(ControlCommand::EventCursor {
            next_event_seq: resume,
        }),
    )
    .expect("actor cursor")
    else {
        panic!("retained cursor");
    };

    let path = session.join("events.jsonl");
    let mut source = File::open(&path).expect("open canonical source");
    source
        .seek(SeekFrom::Start(start.file_offset))
        .expect("seek suffix");

    let mut line = String::new();
    BufReader::new(source)
        .read_line(&mut line)
        .expect("read marker");
    let mut record: EventRecord = serde_json::from_str(&line).expect("marker record");
    let Event::Marker { label, .. } = &mut record.event else {
        panic!("marker event");
    };
    *label = "forgery".to_owned();

    let body = serde_json::to_vec(&EventBody {
        observed_monotonic_ns: record.observed_monotonic_ns,
        seq: record.seq,
        monotonic_ns: record.monotonic_ns,
        event: &record.event,
    })
    .expect("hash preimage");

    let mut hasher = blake3::Hasher::new();
    hasher.update(
        blake3::Hash::from_hex(&record.prev_hash)
            .expect("prior hash")
            .as_bytes(),
    );
    hasher.update(&body);
    record.hash = hasher.finalize().to_hex().to_string();
    let mut changed = serde_json::to_vec(&record).expect("forged record");
    changed.push(b'\n');

    assert_eq!(changed.len(), line.len(), "overwrite without truncation");

    let mut writer = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open overwrite");
    writer
        .seek(SeekFrom::Start(start.file_offset))
        .expect("seek overwrite");
    writer.write_all(&changed).expect("replace suffix");
    writer.sync_all().expect("sync forgery");

    let followed = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["analyze", "events"])
        .arg(&session)
        .args([
            "--follow",
            "--from-event-seq",
            &resume.to_string(),
            "--to-event-seq",
            &(resume + 1).to_string(),
        ])
        .output()
        .expect("follow tampered suffix");
    assert!(!followed.status.success(), "{followed:?}");
    assert!(
        followed.stdout.is_empty(),
        "untrusted suffix must not escape staging"
    );
    assert!(String::from_utf8_lossy(&followed.stderr).contains("trusted boundary"));

    // Restore canonical bytes so recorder shutdown is independent of the attack.
    writer
        .seek(SeekFrom::Start(start.file_offset))
        .expect("seek restore");
    writer
        .write_all(line.as_bytes())
        .expect("restore canonical record");
    writer.sync_all().expect("sync restoration");

    send_request(&session, &ControlRequest::new(ControlCommand::Stop)).expect("stop");

    let recorded = recorder.wait_with_output().expect("recorder exit");
    assert!(recorded.status.success(), "{recorded:?}");
}

#[test]
fn events_follow_rejects_replaced_actor_source_descriptor() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("replaced-follow");
    let recorder = start_event_follow_recorder(&session);
    let resume = follow_test_marker(&session, "prior") + 1;
    follow_test_marker(&session, "genuine");

    let path = session.join("events.jsonl");
    let original = session.join("original-events.jsonl");
    fs::rename(&path, &original).expect("move actor file");
    fs::copy(&original, &path).expect("identical imported replacement");

    let followed = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["analyze", "events"])
        .arg(&session)
        .args([
            "--follow",
            "--from-event-seq",
            &resume.to_string(),
            "--to-event-seq",
            &(resume + 1).to_string(),
        ])
        .output()
        .expect("follow replaced source");
    assert!(!followed.status.success(), "{followed:?}");
    assert!(followed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&followed.stderr).contains("source identity"));

    fs::rename(&original, &path).expect("restore actor descriptor path");

    send_request(&session, &ControlRequest::new(ControlCommand::Stop)).expect("stop");

    let recorded = recorder.wait_with_output().expect("recorder exit");
    assert!(recorded.status.success(), "{recorded:?}");
}

#[test]
fn live_postprocessing_is_explicit_and_stop_waits_for_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("live-snapshot");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY; while :; do sleep 0.1; done",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn live recorder");

    wait_for_control(&session);

    let mut marker = Command::cargo_bin("termtrace").expect("binary should build");
    marker
        .args(["send"])
        .arg(&session)
        .args(["mark:settled", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""type": "mark""#));

    let mut incomplete_index = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_index
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is incomplete"));

    let mut incomplete_capture = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_capture
        .args(["artifact", "capture", "--session"])
        .arg(&session)
        .args(["--width", "320", "--height", "200"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is incomplete"));

    assert!(matches!(
        index_session_with_options(&session, IndexOptions::default()),
        Err(termtrace_engine::Error::SessionIncomplete { .. })
    ));

    let snapshot = index_session_with_options(
        &session,
        IndexOptions {
            live_snapshot: true,
            state_selection: StateSelection::Presented,
            ..IndexOptions::default()
        },
    )
    .expect("explicit live snapshot");
    assert!(!snapshot.source_complete);
    assert!(snapshot
        .directory
        .file_name()
        .expect("snapshot basename")
        .to_string_lossy()
        .contains("snapshot-"));

    let snapshot_manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(snapshot.directory.join("manifest.json")).expect("snapshot manifest"),
    )
    .expect("snapshot manifest JSON");
    assert!(!snapshot_manifest.source_complete);
    assert_eq!(snapshot_manifest.state_selection, StateSelection::Presented);

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.args(["stop", "--json", "--wait", "--timeout-ms", "5000"])
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind": "completed""#));

    let output = child.wait_with_output().expect("wait for live recorder");
    assert!(output.status.success(), "recorder failed: {output:?}");

    let mut status = Command::cargo_bin("termtrace").expect("binary should build");
    status
        .args(["status", "--json", "--wait-complete"])
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind": "completed""#));

    let mut marker_range = Command::cargo_bin("termtrace").expect("binary should build");
    marker_range
        .args(["analyze", "index", "--from-marker", "settled", "--session"])
        .arg(&session)
        .assert()
        .success();
}

#[test]
fn marker_listing_distinguishes_each_repeated_label_occurrence() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("repeated-markers");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 0.1; done",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn marker fixture");

    wait_for_control(&session);

    for _ in 0..2 {
        send_request(
            &session,
            &ControlRequest::new(ControlCommand::Mark {
                label: "repeated".to_owned(),
            }),
        )
        .expect("record repeated marker");
    }

    send_request(&session, &ControlRequest::new(ControlCommand::Stop))
        .expect("stop marker fixture");

    let output = child.wait_with_output().expect("wait for marker fixture");
    assert!(output.status.success(), "marker fixture failed: {output:?}");

    let markers = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["analyze", "markers"])
        .arg(&session)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let markers: Vec<serde_json::Value> =
        serde_json::from_slice(&markers).expect("marker listing JSON");
    assert_eq!(markers.len(), 2);
    assert!(markers
        .iter()
        .all(|marker| return marker["occurrence_count"] == 2));
    assert_eq!(markers[0]["occurrence_index"], 1);
    assert_eq!(markers[1]["occurrence_index"], 2);

    let first_seq = markers[0]["event_seq"]
        .as_u64()
        .expect("first marker event");
    let first_time = markers[0]["monotonic_ns"]
        .as_u64()
        .expect("first marker time");

    let selected = resolve_session_prefix_source_selection(
        &session,
        Some(first_seq + 1),
        None,
        None,
        Some("repeated"),
        None,
    )
    .expect("later duplicate excluded by finite prefix")
    .expect("selected range");
    assert_eq!(selected.start_monotonic_ns, first_time);
    assert_eq!(selected.end_monotonic_ns, first_time + 1);
    assert!(resolve_session_prefix_source_selection(
        &session,
        None,
        None,
        None,
        Some("repeated"),
        None,
    )
    .is_err());
}
