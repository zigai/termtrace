//! live session control CLI integration tests.

use std::os::unix::net::{SocketAddr, UnixListener};

use crate::support::*;
use termtrace::{
    control::PointerOperation,
    ghostty_vt::MouseButton,
    locator::LocatorError,
    observation::CaptureQuality,
    session::{read_completion, read_manifest},
};

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
    wait_for_control(&session);
    let registry_state = temporary.path().join("registry-state");
    let mut register = Command::cargo_bin("termtrace").expect("binary should build");
    register
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["registry", "register", "live-demo", "--session"])
        .arg(&session)
        .assert()
        .success();
    let mut named_ping = Command::cargo_bin("termtrace").expect("binary should build");
    named_ping
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["control", "ping", "live-demo"])
        .assert()
        .success();
    let entry: serde_json::Value = serde_json::from_slice(
        &fs_err::read(registry_state.join("registry/live-demo.json")).expect("registry entry"),
    )
    .expect("registry JSON");
    assert!(
        entry
            .pointer("/recorder_pid")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    );
    assert!(
        entry
            .pointer("/recorder_process_start_ticks")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    );
    let mut named_list = Command::cargo_bin("termtrace").expect("binary should build");
    named_list
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["registry", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"live""#))
        .stdout(predicate::str::contains(r#""alias":"live-demo""#));

    let first_boundary = match send_request(&session, &ControlRequest::new(ControlCommand::Status))
        .expect("first observer-free status")
    {
        ControlData::Status { boundaries, .. } => Some(boundaries),
        _ => None,
    }
    .expect("status response data");
    send_request(&session, &ControlRequest::new(ControlCommand::Ping)).expect("observer-free ping");
    let second_boundary =
        match send_request(&session, &ControlRequest::new(ControlCommand::Status))
            .expect("second observer-free status")
        {
            ControlData::Status { boundaries, .. } => Some(boundaries),
            _ => None,
        }
        .expect("status response data");
    assert_eq!(first_boundary, second_boundary);

    let duplicate_request = ControlRequest::new(ControlCommand::Mark {
        label: "deduplicated".to_owned(),
    });
    send_request(&session, &duplicate_request).expect("first marker request");
    send_request(&session, &duplicate_request).expect("duplicate should return cached response");
    let collision = ControlRequest {
        version: duplicate_request.version,
        request_id: duplicate_request.request_id,
        expected_presented_state_hash: None,
        command: ControlCommand::Mark {
            label: "different".to_owned(),
        },
    };
    assert!(matches!(
        send_request(&session, &collision),
        Err(Error::ControlRejected { code, .. })
            if code == "request_id_collision"
    ));

    for arguments in [
        vec!["control", "ping", session.to_str().expect("UTF-8 path")],
        vec!["status", session.to_str().expect("UTF-8 path")],
        vec![
            "control",
            "mark",
            session.to_str().expect("UTF-8 path"),
            "--label",
            "before-input",
        ],
        vec![
            "control",
            "resize",
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
            session.to_str().expect("UTF-8 path"),
            "--output",
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
    send.arg("send")
        .arg(&session)
        .args(["text:hello\n", "--intent", "complete shell read"])
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
    assert!(
        !control_endpoint_is_reachable(&session),
        "control endpoint should close on exit"
    );

    let recording = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    assert_control_events(&recording, &session);
    let marker = recording
        .events
        .iter()
        .find(|record| {
            return matches!(&record.event, Event::Marker { label, .. } if label == "before-input");
        })
        .expect("before-input marker");
    let selectors = [
        vec!["--at-marker".to_owned(), "before-input".to_owned()],
        vec!["--event-seq".to_owned(), marker.seq.to_string()],
        vec!["--at".to_owned(), format!("{}ns", marker.monotonic_ns)],
    ];
    let mut selected_hashes = Vec::new();
    for selector in selectors {
        let mut show = Command::cargo_bin("termtrace").expect("binary should build");
        let shown = show
            .arg("show")
            .arg(&session)
            .args(selector)
            .args(["--format", "json-compact"])
            .assert()
            .success();
        let observation: serde_json::Value =
            serde_json::from_slice(&shown.get_output().stdout).expect("selected observation");
        selected_hashes.push(
            observation
                .pointer("/state_hash")
                .and_then(serde_json::Value::as_str)
                .expect("state hash")
                .to_owned(),
        );
    }
    assert!(selected_hashes.windows(2).all(|pair| {
        return matches!(pair, [left, right] if left == right);
    }));
    let journal_path = session.join("requests.jsonl");
    validate_json_lines("mutation-journal-record.schema.json", &journal_path);
    let mutation_records = read_mutation_journal(&journal_path).expect("mutation journal");
    assert_eq!(
        mutation_records
            .iter()
            .filter(|record| return record.request_id == duplicate_request.request_id)
            .count(),
        1,
        "retried mutation must have one persistent outcome"
    );
    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .args(["integrity", "verify"])
        .arg(&session)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""mutation_journal_status":"verified""#,
        ))
        .stdout(predicate::str::contains(
            r#""mutation_journal_records_verified":4"#,
        ));
}

#[test]
fn live_control_uses_a_bounded_endpoint_for_long_evidence_paths() {
    let temporary = TempDir::new().expect("temporary directory");
    let evidence_root = temporary.path().join("x".repeat(180));
    fs_err::create_dir(&evidence_root).expect("long evidence root");
    let session = evidence_root.join("session");
    assert!(session.as_os_str().as_bytes().len() > 108);
    let recorder = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import signal,sys,time; signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)); print('READY', flush=True); time.sleep(30)",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder in long evidence root");
    wait_for_control(&session);
    assert!(matches!(
        send_request(&session, &ControlRequest::new(ControlCommand::Ping)),
        Ok(ControlData::Acknowledged { .. })
    ));
    send_request(&session, &ControlRequest::new(ControlCommand::Stop))
        .expect("stop long-path child");
    let output = recorder
        .wait_with_output()
        .expect("wait for long-path recorder");
    assert!(
        output.status.success(),
        "long-path recorder failed: {output:?}"
    );
    assert!(!session.join("control.sock").exists());
}

#[test]
fn stable_capture_returns_the_exact_actor_state_and_typed_deadline() {
    let temporary = TempDir::new().expect("temporary directory");
    let stable_session = temporary.path().join("stable");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let mut stable_child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&stable_session)
        .args(["--", "/bin/sh", "-c", "printf READY; sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stable recorder");
    wait_for_control(&stable_session);
    wait_for_output(&stable_session, b"READY", Duration::from_secs(2)).expect("ready output");

    let capture = send_request(
        &stable_session,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: 30_000_000,
            timeout_ns: 2_000_000_000,
            projection: ScreenProjectionRequest::default(),
        }),
    )
    .expect("stable capture");
    let ControlData::StableSnapshot { frame, quality, .. } = capture else {
        panic!("wrong stable capture payload");
    };
    assert_eq!(quality, StableCaptureQuality::Stable);
    let ScreenFrameData::Spans { rows } = &frame.data else {
        panic!("default projection should return spans");
    };
    assert!(rows.iter().any(|row| return row.text.contains("READY")));
    let state_hash = frame.state_hash.clone();
    assert_eq!(state_hash.len(), 64);
    let ScreenFrameSource::Actor { boundaries, .. } = &frame.source else {
        panic!("stable capture should be actor sourced");
    };
    assert_eq!(boundaries.observed.output_bytes, 5);
    let mut compact_idle = Command::cargo_bin("termtrace").expect("binary should build");
    let compact_output = compact_idle
        .arg("wait-idle")
        .arg(&stable_session)
        .args(["--stable-for", "10ms", "--timeout-ms", "2000"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let compact: serde_json::Value =
        serde_json::from_slice(&compact_output).expect("compact idle response");
    assert_eq!(
        compact.pointer("/kind").and_then(serde_json::Value::as_str),
        Some("stable_snapshot")
    );
    assert_eq!(
        compact
            .pointer("/state")
            .and_then(serde_json::Value::as_str),
        Some(state_hash.as_str())
    );
    assert!(compact.pointer("/screen").is_some());
    assert!(compact.pointer("/boundary").is_none());
    assert!(compact.pointer("/semantic_state_zstd").is_none());
    let mut full_idle = Command::cargo_bin("termtrace").expect("binary should build");
    full_idle
        .args(["--response", "full", "wait-idle"])
        .arg(&stable_session)
        .args([
            "--stable-for",
            "10ms",
            "--timeout-ms",
            "2000",
            "--full-state",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""semantic_state_zstd""#));
    send_request(&stable_session, &ControlRequest::new(ControlCommand::Stop))
        .expect("stop stable child");
    let _stable_status = stable_child.wait().expect("stable recorder exit");

    let changing_session = temporary.path().join("changing");
    let mut changing_child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&changing_session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,time\nfor i in range(500): os.write(1, f'\\r{i}'.encode()); time.sleep(.005)",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn changing recorder");
    wait_for_control(&changing_session);
    let deadline = send_request(
        &changing_session,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: 100_000_000,
            timeout_ns: 250_000_000,
            projection: ScreenProjectionRequest::default(),
        }),
    )
    .expect("typed deadline capture");
    assert!(matches!(
        deadline,
        ControlData::StableSnapshot {
            quality: StableCaptureQuality::Deadline,
            ..
        }
    ));
    send_request(
        &changing_session,
        &ControlRequest::new(ControlCommand::Stop),
    )
    .expect("stop changing child");
    let _changing_status = changing_child.wait().expect("changing recorder exit");

    let completed_session = temporary.path().join("child-completed");
    let mut completed_child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&completed_session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,time\nos.write(1,b'READY')\nif os.fork() == 0:\n time.sleep(30); os._exit(0)\ntime.sleep(1); os._exit(0)",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child-completed recorder");
    wait_for_control(&completed_session);
    wait_for_output(&completed_session, b"READY", Duration::from_secs(2))
        .expect("completed fixture ready");
    let completed = send_request(
        &completed_session,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: 3_600_000_000_000,
            timeout_ns: 3_600_000_000_000,
            projection: ScreenProjectionRequest::default(),
        }),
    )
    .expect("typed child completion");
    let ControlData::StableSnapshot { quality, frame, .. } = completed else {
        panic!("unexpected completed-child outcome: {completed:?}");
    };
    assert!(matches!(
        quality,
        StableCaptureQuality::ChildCompleted | StableCaptureQuality::OutputClosed
    ));
    assert_eq!(
        frame.capture_quality,
        CaptureQuality::Live,
        "actor-owned termination is not validated completed evidence"
    );
    let _completed_status = completed_child.wait().expect("completed recorder exit");

    let closed_session = temporary.path().join("output-closed");
    let mut closed_child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&closed_session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf CLOSED; exec 0<&- 1>&- 2>&-; sleep 8",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn output-closed recorder");
    wait_for_control(&closed_session);
    wait_for_output(&closed_session, b"CLOSED", Duration::from_secs(2))
        .expect("closed fixture output");
    let closed = send_request(
        &closed_session,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: 1_000_000_000,
            timeout_ns: 3_000_000_000,
            projection: ScreenProjectionRequest::default(),
        }),
    )
    .expect("typed output closure");
    let ControlData::StableSnapshot {
        quality: StableCaptureQuality::OutputClosed,
        frame,
        ..
    } = closed
    else {
        panic!("unexpected output-closed outcome: {closed:?}");
    };
    assert_eq!(
        frame.capture_quality,
        CaptureQuality::Live,
        "actor-owned output closure is not validated completed evidence"
    );
    let mut strict_idle = Command::cargo_bin("termtrace").expect("binary should build");
    strict_idle
        .arg("wait-idle")
        .arg(&closed_session)
        .args(["--stable-for", "1s", "--timeout-ms", "3000"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("output_closed"));
    let mut permissive_idle = Command::cargo_bin("termtrace").expect("binary should build");
    permissive_idle
        .arg("wait-idle")
        .arg(&closed_session)
        .args([
            "--stable-for",
            "1s",
            "--timeout-ms",
            "3000",
            "--allow-incomplete",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""quality":"output_closed""#));
    let _closed_status = closed_child.wait().expect("closed recorder exit");
}

#[test]
fn locator_deadline_bounds_an_in_flight_actor_capture() {
    let temporary = TempDir::new().expect("temporary directory");
    let session_path = temporary.path().join("locator-deadline");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let recorder = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session_path)
        .args(["--", "/bin/sh", "-c", "printf READY; sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn locator deadline recorder");
    wait_for_control(&session_path);
    wait_for_output(&session_path, b"READY", Duration::from_secs(2))
        .expect("locator fixture ready");

    let terminal = ClientTerminal::new(binary, temporary.path().join("unused-evidence-root"));
    let fake_session_path = temporary.path().join("stalled-control");
    fs::create_dir_all(&fake_session_path).expect("create stalled session directory");
    let mut manifest = read_manifest(&session_path).expect("read session manifest");
    manifest.control_socket = format!("{}-deadline", manifest.control_socket);
    let address = {
        use std::os::linux::net::SocketAddrExt as _;

        SocketAddr::from_abstract_name(manifest.control_socket.as_bytes())
            .expect("deadline socket address")
    };
    let listener = UnixListener::bind_addr(&address).expect("bind deadline socket");
    fs::write(
        fake_session_path.join("session.json"),
        serde_json::to_vec(&manifest).expect("encode deadline manifest"),
    )
    .expect("install deadline manifest");
    let session = terminal
        .attach(&fake_session_path)
        .expect("attach stalled locator session");
    let stalled_server = thread::spawn(move || {
        let (_stream, _) = listener.accept().expect("accept deadline request");
        thread::sleep(Duration::from_secs(1));
    });

    let screen = session.screen();
    let locator = screen.get_by_text("READY").expect("valid locator");
    let started_at = Instant::now();
    let error = locator
        .wait(Duration::from_millis(50), Duration::from_millis(5))
        .expect_err("stalled capture transport must honor the locator deadline");
    let elapsed = started_at.elapsed();
    assert!(
        matches!(error, LocatorError::Timeout { .. }),
        "unexpected locator failure: {error:?}"
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "locator transport exceeded its deadline by too much: {elapsed:?}"
    );

    stalled_server.join().expect("join stalled control server");
    send_request(&session_path, &ControlRequest::new(ControlCommand::Stop))
        .expect("stop locator fixture");
    let _output = recorder
        .wait_with_output()
        .expect("wait for locator recorder");
}

#[test]
fn environment_values_never_enter_recorder_argv_or_manifest() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("secret-safe");
    let secret = b"argv-must-not-contain-this-secret";
    let spec = EnvironmentSpec {
        base: EnvironmentBase::Clear,
        set: BTreeMap::from([(
            "TERMTRACE_SECRET".to_owned(),
            ByteString::from(secret.as_slice()),
        )]),
        ..EnvironmentSpec::default()
    };
    let encoded = serde_json::to_vec(&spec).expect("environment JSON");
    let mut recorder = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--output-byte-budget", "1000000"])
        .arg("--environment-spec-stdin")
        .args([
            "--",
            "/bin/sh",
            "-c",
            "test -n \"$TERMTRACE_SECRET\"; sleep 30",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");
    recorder
        .stdin
        .take()
        .expect("environment stdin")
        .write_all(&encoded)
        .expect("write environment stdin");
    wait_for_control(&session);
    let argv = fs_err::read(format!("/proc/{}/cmdline", recorder.id())).expect("recorder argv");
    assert!(
        !argv
            .windows(secret.len())
            .any(|window| return window == secret)
    );
    let manifest = fs_err::read(session.join("session.json")).expect("manifest");
    assert!(
        !manifest
            .windows(secret.len())
            .any(|window| return window == secret)
    );
    let manifest: SessionManifest = serde_json::from_slice(&manifest).expect("manifest JSON");
    assert_eq!(manifest.resource_limits.output_bytes, Some(1_000_000));
    send_request(&session, &ControlRequest::new(ControlCommand::Stop)).expect("stop child");
    let recorder_output = recorder.wait_with_output().expect("recorder exit");
    assert!(
        !recorder_output
            .stderr
            .windows(secret.len())
            .any(|window| return window == secret)
    );

    let invalid_session = temporary.path().join("invalid-secret-safe");
    let invalid_secret = b"error-must-not-contain-this-secret";
    let invalid_spec = EnvironmentSpec {
        base: EnvironmentBase::Clear,
        set: BTreeMap::from([(
            "TERMTRACE_SECRET".to_owned(),
            ByteString::from(invalid_secret.as_slice()),
        )]),
        copy: BTreeSet::from(["TERMTRACE_VARIABLE_THAT_MUST_NOT_EXIST_7B11".to_owned()]),
        ..EnvironmentSpec::default()
    };
    let mut invalid = Command::cargo_bin("termtrace").expect("binary should build");
    let invalid_output = invalid
        .args(["record", "--no-input", "--out"])
        .arg(&invalid_session)
        .arg("--environment-spec-stdin")
        .args(["--", "/bin/true"])
        .write_stdin(serde_json::to_vec(&invalid_spec).expect("invalid spec JSON"))
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(
        !invalid_output
            .windows(invalid_secret.len())
            .any(|window| return window == invalid_secret)
    );
    assert!(!invalid_session.exists());
}

#[test]
#[allow(
    clippy::needless_collect,
    reason = "all recorders must launch before any join to prove concurrent isolation"
)]
fn concurrent_client_launches_keep_environment_snapshots_isolated() {
    let temporary = TempDir::new().expect("temporary directory");
    let evidence = temporary.path().join("concurrent-evidence");
    fs_err::create_dir(&evidence).expect("evidence root");
    let workers = (0..8)
        .map(|index| {
            let evidence = evidence.clone();
            return thread::spawn(move || {
                let value = format!("isolated-{index}-{}", "x".repeat(index));
                let script = format!(
                    "test \"${{#ISOLATED_ID}}\" -eq {}; printf OK; sleep 30",
                    value.len()
                );
                let terminal = ClientTerminal::new(env!("CARGO_BIN_EXE_termtrace"), &evidence);
                let mut session = terminal
                    .launch([
                        OsString::from("/bin/sh"),
                        OsString::from("-c"),
                        OsString::from(script),
                    ])
                    .clear_environment()
                    .copy_environment("PATH")
                    .set_environment("ISOLATED_ID", value.into_bytes())
                    .color_policy(ColorPolicy::Never)
                    .output_byte_budget(1_000_000)
                    .start()
                    .expect("concurrent launch");
                wait_for_output(session.path(), b"OK", Duration::from_secs(3))
                    .expect("concurrent output");
                session
                    .stop(Some(Duration::from_secs(5)))
                    .expect("concurrent stop");
                let manifest = read_manifest(session.path()).expect("concurrent manifest");
                assert_eq!(manifest.resource_limits.output_bytes, Some(1_000_000));
                return manifest.child_environment.effective_digest;
            });
        })
        .collect::<Vec<_>>();
    let digests = workers
        .into_iter()
        .map(|worker| return worker.join().expect("launch worker"))
        .collect::<BTreeSet<_>>();
    assert_eq!(digests.len(), 8);
    assert!(
        fs_err::read_dir(&evidence)
            .expect("evidence entries")
            .filter_map(|entry| return entry.ok())
            .all(|entry| return !entry.file_name().to_string_lossy().contains("environment"))
    );
}

#[test]
fn optimistic_state_guards_reject_stale_input_without_canonical_events() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("guarded-input");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf READY; IFS= read -r line; printf '<%s>' \"$line\"",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn guarded fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("ready output");

    let snapshot = send_request(&session, &ControlRequest::new(ControlCommand::Snapshot))
        .expect("presented snapshot");
    let ControlData::Snapshot { state_hash, .. } = snapshot else {
        panic!("expected snapshot response");
    };
    let stale_request = ControlRequest::new(ControlCommand::Input {
        data: ByteString::new(b"blocked\n".to_vec()),
        input_kind: InputKind::Text,
        caller_intent: Some("must not apply".to_owned()),
    })
    .with_expected_presented_state_hash("0".repeat(64));
    let stale_response =
        send_request_response(&session, &stale_request).expect("structured stale response");
    let ControlResult::Error {
        code,
        details: Some(details),
        ..
    } = &stale_response.result
    else {
        panic!("expected structured stale-observation response");
    };
    assert_eq!(code, "stale_observation");
    assert_eq!(details.current_presented_state_hash, state_hash);
    assert_eq!(details.scope, "actor_processed_presented_output");
    assert_eq!(
        send_request_response(&session, &stale_request).expect("idempotent stale retry"),
        stale_response
    );

    for mut command in [
        {
            let mut command = Command::cargo_bin("termtrace").expect("binary should build");
            command
                .args(["--response", "agent", "send", "--if-state"])
                .arg("0".repeat(64))
                .arg(&session)
                .arg("text:blocked");
            command
        },
        {
            let mut command = Command::cargo_bin("termtrace").expect("binary should build");
            command
                .args(["--response", "agent", "act", "--if-state"])
                .arg("0".repeat(64))
                .arg(&session)
                .arg("text:blocked");
            command
        },
        {
            let mut command = Command::cargo_bin("termtrace").expect("binary should build");
            command
                .args(["--response", "agent", "control", "resize", "--if-state"])
                .arg("0".repeat(64))
                .arg(&session)
                .args(["--cols", "81", "--rows", "25"]);
            command
        },
    ] {
        let output = command.assert().failure().get_output().stdout.clone();
        let rejection = compact_json_line(&output);
        assert_eq!(rejection["code"], "stale_observation");
        assert_eq!(rejection["applied"], "none");
        assert_eq!(rejection["retry"], "safe_new_request");
        assert_eq!(rejection["session_state"], "live");
        assert_eq!(rejection["observation"]["state"], state_hash);
        assert_eq!(
            rejection["observation"]["scope"],
            "actor_processed_presented_output"
        );
    }

    let guarded_input = ControlRequest::new(ControlCommand::Input {
        data: ByteString::new(b"accepted\n".to_vec()),
        input_kind: InputKind::Text,
        caller_intent: Some("fresh observation".to_owned()),
    })
    .with_expected_presented_state_hash(state_hash);
    send_request(&session, &guarded_input).expect("fresh guarded input");
    let output = child
        .wait_with_output()
        .expect("guarded fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let recording = EventLog::read(session.join("events.jsonl")).expect("event log");
    let agent_inputs = recording
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                source: InputSource::Agent,
                data,
                ..
            } => return Some(data.as_bytes()),
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert_eq!(agent_inputs, vec![b"accepted\n".as_slice()]);
}

#[test]
fn rejected_control_requests_do_not_terminate_the_recorder_actor() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("request-rejections");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'READY'); d=b''\nwhile len(d)<3: d+=os.read(0,3-len(d))\nos.write(1,b'GOT:'+d)",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn request rejection fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("ready output");

    let requests = [
        (
            ControlCommand::Key {
                key: NamedKey::Function(0),
                modifiers: KeyModifiers::default(),
            },
            "key_encoding_failed",
        ),
        (
            ControlCommand::Pointer {
                column: u16::MAX,
                row: 0,
                operation: PointerOperation::Click {
                    button: MouseButton::Left,
                },
                strict: true,
            },
            "pointer_cell_out_of_bounds",
        ),
        (
            ControlCommand::WaitScreen {
                predicate: ScreenPredicate::Contains {
                    text: "never".to_owned(),
                },
                stable_for_ns: 1,
                timeout_ns: 0,
                projection: ScreenProjectionRequest::default(),
            },
            "invalid_screen_wait",
        ),
    ];
    for (index, (command, expected_code)) in requests.into_iter().enumerate() {
        let response = send_request_response(&session, &ControlRequest::new(command))
            .expect("structured request rejection");
        assert!(matches!(
            &response.result,
            ControlResult::Error { code, .. } if code == expected_code
        ));
        send_request(&session, &ControlRequest::new(ControlCommand::Ping))
            .expect("ping after rejection");
        assert!(matches!(
            send_request(&session, &ControlRequest::new(ControlCommand::Status)),
            Ok(ControlData::Status { .. })
        ));
        send_request(
            &session,
            &ControlRequest::new(ControlCommand::Input {
                data: ByteString::new(vec![b'a'.saturating_add(u8::try_from(index).unwrap_or(0))]),
                input_kind: InputKind::Text,
                caller_intent: Some("prove actor survived rejection".to_owned()),
            }),
        )
        .expect("valid input after rejection");
    }

    let output = child.wait_with_output().expect("request rejection fixture");
    assert!(output.status.success(), "fixture failed: {output:?}");
    wait_for_output(&session, b"GOT:abc", Duration::from_secs(5)).expect("valid inputs");
    let completion = read_completion(&session).expect("durable completion");
    assert!(completion.failure.is_none());
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its recording fixture is invalid"
)]
fn assert_control_events(recording: &EventLog, session: &Path) {
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            input_kind: InputKind::Text,
            caller_intent: Some(intent),
            ..
        } if intent == "complete shell read"
    )));
    assert_eq!(
        recording
            .events
            .iter()
            .filter(|record| matches!(
                &record.event,
                Event::Marker { label, .. } if label == "deduplicated"
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
        Event::Marker { label, .. } if label == "before-input"
    )));
    assert!(recording.pending_operations.is_empty());

    let resize_operation = recording
        .events
        .iter()
        .find_map(|record| match record.event {
            Event::ResizeIntent {
                operation_id,
                cols: 100,
                rows: 30,
                ..
            } => return Some(operation_id),
            _ => return None,
        })
        .expect("resize intent");
    let resize_result_time = recording
        .events
        .iter()
        .find_map(|record| match record.event {
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } if operation_id == resize_operation => return Some(record.monotonic_ns),
            _ => return None,
        })
        .expect("resize result");
    let ranged = index_session_range(session, Some(resize_result_time), None)
        .expect("range crossing resize intent/result");
    let ranged_states = read_zstd_text(&ranged.directory.join("states.jsonl.zst"));
    let ranged_states = ranged_states
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("state JSON"))
        .collect::<Vec<_>>();
    assert!(
        ranged_states
            .iter()
            .any(|state| return state.delta.geometry == Some((100, 30)))
    );
    assert!(ranged_states.iter().all(|state| {
        let Some(event_sequence) = state.event_seq else {
            return true;
        };
        return recording.events.iter().any(|record| {
            return record.seq == event_sequence
                && matches!(
                    record.event,
                    Event::Output { .. }
                        | Event::ResizeResult {
                            status: OperationStatus::Applied,
                            ..
                        }
                );
        });
    }));
}
