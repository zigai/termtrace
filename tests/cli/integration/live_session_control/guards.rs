use super::*;
use termtrace_model::protocol::mutation::MutationEffect;

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

    let ControlPayload::Snapshot { state_hash, .. } = snapshot else {
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
        effect,
        details: Some(details),
        ..
    } = &stale_response.result
    else {
        panic!("expected structured stale-observation response");
    };

    assert_eq!(code, "stale_observation");
    assert_eq!(*effect, MutationEffect::NotApplied);
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
                .args(["send", "--json", "--if-state"])
                .arg("0".repeat(64))
                .arg(&session)
                .arg("text:blocked");

            command
        },
        {
            let mut command = Command::cargo_bin("termtrace").expect("binary should build");
            command
                .args(["send", "--json", "--if-state"])
                .arg("0".repeat(64))
                .arg(&session)
                .arg("resize:81,25");

            command
        },
    ] {
        let output = command.assert().failure().get_output().stdout.clone();
        let rejection = compact_json_line(&output);
        assert_eq!(rejection["code"], "stale_observation");

        let effect = rejection
            .pointer("/effect/kind")
            .or_else(|| return rejection.pointer("/input/effect/kind"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(effect, Some("not_applied"));

        let retry = rejection
            .get("retry")
            .or_else(|| return rejection.pointer("/input/retry"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(retry, Some("new_request"));

        let observed_state = rejection
            .pointer("/observation/state")
            .or_else(|| return rejection.pointer("/details/current_presented_state_hash"))
            .and_then(serde_json::Value::as_str);
        assert_eq!(observed_state, Some(state_hash.as_str()));
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
            Ok(ControlPayload::Status { .. })
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
