use super::*;

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
        .args(["wait", "--json"])
        .arg(&stable_session)
        .args(["--idle=10ms", "--timeout", "2s"])
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
            .pointer("/state_hash")
            .and_then(serde_json::Value::as_str),
        Some(state_hash.as_str())
    );
    assert!(compact.pointer("/frame").is_some() || compact.pointer("/screen").is_some());
    assert!(compact.pointer("/boundary").is_none());
    assert!(compact.pointer("/semantic_state_zstd").is_none());

    let mut full_idle = Command::cargo_bin("termtrace").expect("binary should build");
    full_idle
        .args(["wait", "--json"])
        .arg(&stable_session)
        .args(["--idle=10ms", "--timeout", "2s", "--full-state"])
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
            "import os, time\ni = 0\nwhile True:\n    os.write(1, f'\\r{i}'.encode())\n    time.sleep(0.005)\n    i += 1",
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
        .args(["wait", "--json"])
        .arg(&closed_session)
        .args(["--idle=1s", "--timeout", "3s"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("output_closed"));

    let mut permissive_idle = Command::cargo_bin("termtrace").expect("binary should build");
    permissive_idle
        .args(["wait", "--json"])
        .arg(&closed_session)
        .args(["--idle=1s", "--timeout", "3s", "--allow-incomplete"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""quality": "output_closed""#));

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
