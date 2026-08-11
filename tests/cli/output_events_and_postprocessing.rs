//! output events and postprocessing CLI integration tests.

use crate::support::*;

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
    let expected = log
        .events
        .iter()
        .filter_map(|record| {
            return match &record.event {
                Event::Output { data } => Some(data.as_bytes()),
                _ => None,
            };
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();

    let mut output = Command::cargo_bin("termtrace").expect("binary should build");
    output
        .args(["analyze", "output"])
        .arg(&session)
        .arg("--raw")
        .assert()
        .success()
        .stdout(predicate::eq(expected.clone()));

    let output_events = log
        .events
        .iter()
        .filter(|record| return matches!(record.event, Event::Output { .. }))
        .map(|record| return record.seq)
        .collect::<Vec<_>>();
    let first = output_events.first().copied().expect("output event");
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
        .stdout(predicate::eq(expected));

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
    let expected = log
        .events
        .iter()
        .filter_map(|record| {
            return match &record.event {
                Event::Output { data } => Some(data.as_bytes()),
                _ => None,
            };
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
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
        .stdout(predicate::str::contains(r#""event_type":"output""#))
        .stdout(predicate::str::contains(r#""metadata_only":true"#))
        .stdout(predicate::str::contains(r#""field":"data""#))
        .stdout(predicate::str::contains(r#""byte_count":12"#))
        .stdout(predicate::str::contains(r#""event":null"#))
        .stdout(predicate::str::contains("secret-bytes").not())
        .get_output()
        .stdout
        .clone();
    assert_eq!(output.split(|byte| return *byte == b'\n').count(), 2);

    let mut exact = Command::cargo_bin("termtrace").expect("binary should build");
    exact
        .args(["analyze", "events"])
        .arg(&session)
        .args(["--type", "output"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""metadata_only":false"#))
        .stdout(predicate::str::contains(r#""type":"output""#));
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
            "printf first; sleep 0.15; printf second",
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
    assert!(
        recorder
            .wait_with_output()
            .expect("wait for recorder")
            .status
            .success()
    );
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
        .args(["control", "mark"])
        .arg(&session)
        .args(["--label", "settled"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"marked""#));

    let mut incomplete_index = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_index
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not complete"));
    let mut incomplete_capture = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_capture
        .args(["artifact", "capture", "--session"])
        .arg(&session)
        .args(["--width", "320", "--height", "200"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not complete"));

    assert!(matches!(
        index_session_with_options(&session, IndexOptions::default()),
        Err(Error::SessionIncomplete { .. })
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
    assert!(
        snapshot
            .directory
            .file_name()
            .expect("snapshot basename")
            .to_string_lossy()
            .contains("snapshot-")
    );
    let snapshot_manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(snapshot.directory.join("manifest.json")).expect("snapshot manifest"),
    )
    .expect("snapshot manifest JSON");
    assert!(!snapshot_manifest.source_complete);
    assert_eq!(snapshot_manifest.state_selection, StateSelection::Presented);

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.args(["stop", "--wait", "--timeout-ms", "5000"])
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"completed""#));
    let output = child.wait_with_output().expect("wait for live recorder");
    assert!(output.status.success(), "recorder failed: {output:?}");

    let mut status = Command::cargo_bin("termtrace").expect("binary should build");
    status
        .args(["status", "--wait-complete"])
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"completed""#));

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
        .args(["--response", "full", "analyze", "markers"])
        .arg(&session)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let markers: Vec<serde_json::Value> =
        serde_json::from_slice(&markers).expect("marker listing JSON");
    assert_eq!(markers.len(), 2);
    assert!(
        markers
            .iter()
            .all(|marker| return marker["occurrence_count"] == 2)
    );
    assert_eq!(markers[0]["occurrence_index"], 1);
    assert_eq!(markers[1]["occurrence_index"], 2);
}
