use super::*;
use termtrace_model::execution::layout::SessionLayout;

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
        .args(["session", "register", "live-demo", "--session"])
        .arg(&session)
        .assert()
        .success();

    let mut named_status = Command::cargo_bin("termtrace").expect("binary should build");
    named_status
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["status", "live-demo"])
        .assert()
        .success();

    let entry: serde_json::Value = serde_json::from_slice(
        &fs_err::read(registry_state.join("registry/live-demo.json")).expect("registry entry"),
    )
    .expect("registry JSON");
    assert!(entry
        .pointer("/recorder_pid")
        .and_then(serde_json::Value::as_u64)
        .is_some());
    assert!(entry
        .pointer("/recorder_process_start_ticks")
        .and_then(serde_json::Value::as_u64)
        .is_some());

    let mut named_list = Command::cargo_bin("termtrace").expect("binary should build");
    named_list
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["session", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle": "live""#))
        .stdout(predicate::str::contains(r#""alias": "live-demo""#));

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
        Err(termtrace_control::Error::Rejected { code, .. })
            if code == "request_id_collision"
    ));

    for arguments in [
        vec!["status", session.to_str().expect("UTF-8 path")],
        vec![
            "send",
            session.to_str().expect("UTF-8 path"),
            "mark:before-input",
        ],
        vec![
            "send",
            session.to_str().expect("UTF-8 path"),
            "resize:100,30",
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
            "--timeout",
            "5s",
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

    let recording =
        EventLog::read(SessionLayout::new(&session).event_log()).expect("valid event log");
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
            .arg("screen")
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

    let journal_path = SessionLayout::new(&session).mutation_journal();
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
            r#""mutation_journal_status": "verified""#,
        ))
        .stdout(predicate::str::contains(
            r#""mutation_journal_records_verified": 4"#,
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

    let ready_deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < ready_deadline {
        let events =
            fs_err::read_to_string(SessionLayout::new(&session).event_log()).unwrap_or_default();
        if events.contains("READY") {
            break;
        }

        thread::sleep(Duration::from_millis(20));
    }

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
    assert!(!control_endpoint_is_reachable(&session));
}
