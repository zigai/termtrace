//! replay index and playback CLI integration tests.

use crate::support::*;

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
    let manifest = read_manifest(&session).expect("session manifest");
    assert_eq!(
        (
            manifest.geometry.cols,
            manifest.geometry.rows,
            manifest.geometry.pixel_width,
            manifest.geometry.pixel_height,
        ),
        (160, 45, 1920, 1080),
        "default recording geometry must retain a 1080p 16:9 semantic canvas"
    );
    let cast = temporary.path().join("session.cast");
    let mut export = Command::cargo_bin("termtrace").expect("binary should build");
    export
        .args(["artifact", "asciicast", "--session"])
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
    assert!(first.states.array_windows().all(|[before, after]| {
        return before.state_hash != after.state_hash;
    }));

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .success();
    assert_replay_artifacts(&session, first.artifacts.state_count);

    let mut presented = Command::cargo_bin("termtrace").expect("binary should build");
    presented
        .args(["analyze", "index", "--presented-only", "--session"])
        .arg(&session)
        .assert()
        .success();
    let presented_directory = session.join("derived-presented");
    let presented_manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(presented_directory.join("manifest.json")).expect("presented manifest"),
    )
    .expect("presented manifest JSON");
    assert_eq!(
        presented_manifest.state_selection,
        StateSelection::Presented
    );
    assert!(
        presented_manifest.state_count < presented_manifest.semantic_state_count,
        "fixture should suppress synchronized-output internals"
    );
    let presented_states = read_zstd_text(&presented_directory.join("states.jsonl.zst"));
    let presented_states = presented_states
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("presented state"))
        .collect::<Vec<_>>();
    assert!(presented_states.iter().all(|state| return state.presented));
    assert!(presented_states.iter().enumerate().all(|(index, state)| {
        return state.state_id == u64::try_from(index).unwrap_or(u64::MAX);
    }));
    assert_eq!(
        presented_states
            .first()
            .and_then(|state| return state.delta.geometry),
        Some((160, 45)),
        "first retained state must be independently reconstructable"
    );
    let mut inspect_text = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_text
        .args(["analyze", "inspect", "--session"])
        .arg(&session)
        .args(["--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("AB界"));
    let mut inspect_initial = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_initial
        .args(["analyze", "inspect", "--session"])
        .arg(&session)
        .args(["--state", "0", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""state_id": 0"#));

    let mut inspect_row = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_row
        .args(["analyze", "inspect", "--session"])
        .arg(&session)
        .args(["--state", "0", "--row", "0", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""row":0"#));

    validate_json_lines(
        "artifact-root-record.schema.json",
        &session.join("artifact-roots.jsonl"),
    );
    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .args(["integrity", "verify"])
        .arg(&session)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""artifact_roots_verified":2"#))
        .stdout(predicate::str::contains(
            r#""artifact_authenticity_established":false"#,
        ));

    let tampered_member = session.join("derived/final.txt");
    let mut contents = fs_err::read(&tampered_member).expect("derived member");
    contents.extend_from_slice(b"tampered");
    fs_err::write(&tampered_member, contents).expect("tamper derived member");
    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .args(["integrity", "verify"])
        .arg(&session)
        .arg("--json")
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            r#""code":"artifact_integrity_invalid""#,
        ));

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn ranged_index_reconstructs_preroll_and_honors_half_open_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("ranged-replay");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r"printf BEFORE; sleep 0.08; printf '\033[?2026hRANGE\033[?2026l'; sleep 0.08; printf AFTER",
        ])
        .assert()
        .success();

    let event_log = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    let range_start = output_timestamp_containing(&event_log, b"RANGE");
    let range_end = output_timestamp_containing(&event_log, b"AFTER");
    assert!(range_start < range_end, "fixture outputs must be separated");
    let source_range = SourceRange {
        start_monotonic_ns: range_start,
        end_monotonic_ns: range_end,
    };

    let mut empty = Command::cargo_bin("termtrace").expect("binary should build");
    empty
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{range_start}ns"),
            "--to",
            &format!("{range_start}ns"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be non-empty"));

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{range_start}ns"),
            "--to",
            &format!("{range_end}ns"),
        ])
        .assert()
        .success();

    let derived = session.join(derived_directory_name(Some(source_range)));
    let manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(derived.join("manifest.json")).expect("range manifest"),
    )
    .expect("range manifest JSON");
    assert_eq!(manifest.source_range, Some(source_range));
    validate_json_file(
        "replay-manifest.schema.json",
        &derived.join("manifest.json"),
    );
    validate_json_lines_text(
        "state-record.schema.json",
        &read_zstd_text(&derived.join("states.jsonl.zst")),
    );
    validate_json_lines(
        "state-index-entry.schema.json",
        &derived.join("state-index.jsonl"),
    );
    let states = read_zstd_text(&derived.join("states.jsonl.zst"))
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("state JSON"))
        .collect::<Vec<_>>();
    assert!(!states.is_empty(), "range must include a baseline");
    assert_eq!(states[0].state_id, 0);
    assert_eq!(states[0].event_seq, None);
    assert_eq!(states[0].monotonic_ns, range_start);
    assert!(states.iter().all(|state| {
        return state.monotonic_ns >= range_start && state.monotonic_ns < range_end;
    }));
    assert!(states.iter().any(|state| return !state.presented));
    let final_text = fs_err::read_to_string(derived.join("final.txt")).expect("range final text");
    assert!(final_text.contains("BEFORERANGE"));
    assert!(!final_text.contains("AFTER"));

    let mut full = Command::cargo_bin("termtrace").expect("binary should build");
    full.args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .success();
    assert!(session.join("derived").is_dir());
    assert!(derived.is_dir(), "range index must coexist with full index");

    let mut unmatched_capture = Command::cargo_bin("termtrace").expect("binary should build");
    unmatched_capture
        .args(["artifact", "capture", "--session"])
        .arg(&session)
        .args([
            "--width",
            "640",
            "--height",
            "480",
            "--from",
            &format!("{}ns", range_start + 1),
            "--to",
            &format!("{range_end}ns"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "matching semantic index is missing",
        ));

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{range_start}ns"),
            "--to",
            &format!("{range_end}ns"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn inspect_exits_cleanly_when_downstream_closes_the_pipe() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("broken-pipe");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/printf", "pipe-test"])
        .assert()
        .success();

    let binary = env!("CARGO_BIN_EXE_termtrace");
    let mut inspect = StdCommand::new(binary)
        .args(["analyze", "inspect", "--session"])
        .arg(&session)
        .args(["--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn piped inspection");
    let stdout = inspect.stdout.take().expect("inspection stdout");
    let mut reader = BufReader::new(stdout);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).expect("first JSON line");
    assert_eq!(first_line, "{\n");
    drop(reader);
    let output = inspect.wait_with_output().expect("wait for inspection");
    assert!(
        output.status.success(),
        "broken pipe should be a successful early consumer exit: {output:?}"
    );
    assert!(output.stderr.is_empty(), "unexpected panic: {output:?}");
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its recording fixture is invalid"
)]
fn output_timestamp_containing(event_log: &EventLog, needle: &[u8]) -> u64 {
    return event_log
        .events
        .iter()
        .find_map(|record| match &record.event {
            Event::Output { data }
                if data
                    .as_bytes()
                    .windows(needle.len())
                    .any(|part| return part == needle) =>
            {
                return Some(record.monotonic_ns);
            }
            _ => return None,
        })
        .expect("fixture output event");
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
        "states.jsonl.zst",
        "checkpoints.jsonl.zst",
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
    let states = read_zstd_text(&derived.join("states.jsonl.zst"));
    let lines = states.lines().count();
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
    validate_json_lines_text("state-record.schema.json", &states);
    validate_json_lines(
        "state-index-entry.schema.json",
        &derived.join("state-index.jsonl"),
    );
    validate_json_lines_text(
        "semantic-replay-checkpoint.schema.json",
        &read_zstd_text(&derived.join("checkpoints.jsonl.zst")),
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

#[test]
fn playback_does_not_echo_terminal_query_responses() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use termtrace_engine::recording::EffectiveEnvironment;
    use termtrace_evidence::event_log::{DurabilityPolicy, EventLogWriter, SessionClock};
    use termtrace_evidence::provenance::RecorderBuildIdentity;
    use termtrace_model::{
        execution::environment::EnvironmentSpec,
        execution::event::{Event, EventLogHeader},
        execution::session::{
            GeometryProvenance, ResourceLimits, SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
            SessionGeometry, SessionManifest,
        },
        value::ByteString,
    };
    use uuid::Uuid;

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("playback");
    fs_err::create_dir(&session).expect("session directory");
    fs_err::set_permissions(&session, Permissions::from_mode(0o700)).expect("private session");
    let clock = SessionClock::now().expect("session clock");
    let environment = EffectiveEnvironment::resolve(&EnvironmentSpec::default(), "xterm-ghostty")
        .expect("environment");
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        "0000000000000000000000000000000000000000000000000000000000000000",
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
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
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
        child_environment: environment.policy().clone(),
        execution_provenance: collect_execution_provenance(
            &ProvenancePolicy::default(),
            environment.variables(),
            &[OsString::from("/bin/true")],
            Path::new("/"),
            RecorderBuildIdentity {
                version: env!("CARGO_PKG_VERSION"),
                source_revision: env!("TERMTRACE_BUILD_REVISION"),
                semantic_engine_revision: GHOSTTY_REVISION,
            },
        ),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: "linux".to_owned(),
        architecture: "test".to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: control_socket_name(header.session_id),
        request_journal: "requests.jsonl".to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: DurabilityPolicy::default(),
        resource_limits: ResourceLimits::default(),
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
    let error_file = temporary.path().join("playback-error.json");
    let geometry_file = temporary.path().join("geometry.json");
    let ready_file = temporary.path().join("ready");
    command.args([
        "playback",
        "--session",
        session.to_str().expect("UTF-8 session"),
        "--timing-file",
        timing_file.to_str().expect("UTF-8 timing"),
        "--error-file",
        error_file.to_str().expect("UTF-8 playback error"),
        "--geometry-file",
        geometry_file.to_str().expect("UTF-8 geometry"),
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

    assert!(geometry_file.is_file());
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
