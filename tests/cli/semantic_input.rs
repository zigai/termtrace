//! semantic input CLI integration tests.

use crate::support::*;

#[test]
fn named_key_is_encoded_from_live_application_cursor_mode() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("key-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[?1hREADY'); d=os.read(0,3); os.write(1,b'KEY:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn key fixture");
    wait_for_control(&session);
    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.arg("wait")
        .arg(&session)
        .args(["--output", "READY", "--timeout-ms", "5000"])
        .assert()
        .success();
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.arg("send").arg(&session).arg("up").assert().success();
    let output = child.wait_with_output().expect("key fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            input_kind: InputKind::Keys,
            descriptor: InputDescriptor::Key {
                key,
                encoder,
                terminal_modes_hash,
                ..
            },
            data,
            ..
        } if key == "up"
            && encoder == "libghostty-key-encoder-v1"
            && terminal_modes_hash.len() == 64
            && data.as_bytes() == b"\x1bOA"
    )));
}

#[test]
fn send_accepts_natural_modifier_aliases_and_single_character_text() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("natural-key-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'READY'); data=b''\nwhile b'\\r' not in data: data += os.read(0,64)\nos.write(1,b'KEYS:'+data.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn natural key fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("ready output");

    for atom in ["q", "ctrl+f", "ctrl+down", "enter"] {
        let mut send = Command::cargo_bin("termtrace").expect("binary should build");
        send.arg("send").arg(&session).arg(atom).assert().success();
    }
    let output = child.wait_with_output().expect("natural key completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            input_kind: InputKind::Text,
            data,
            ..
        } if data.as_bytes() == b"q"
    )));
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            descriptor: InputDescriptor::Key { key, control, .. },
            ..
        } if key == "down" && *control
    )));

    let mut keys = Command::cargo_bin("termtrace").expect("binary should build");
    keys.args(["control", "keys"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""format":"termtrace-key-contract""#,
        ))
        .stdout(predicate::str::contains("ctrl+f"));
}

#[test]
fn focus_events_are_mode_gated_encoded_and_audited() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("focus-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[?1004hREADY'); d=os.read(0,3)+os.read(0,3); os.write(1,b'FOCUS:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn focus fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("focus mode ready");

    for direction in ["in", "out"] {
        let mut focus = Command::cargo_bin("termtrace").expect("binary should build");
        focus
            .args(["control", "focus"])
            .arg(&session)
            .arg(direction)
            .arg("--strict")
            .assert()
            .success();
    }
    let output = child.wait_with_output().expect("focus fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let focus_events = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                input_kind: InputKind::Focus,
                descriptor:
                    InputDescriptor::Focus {
                        focused,
                        encoder,
                        terminal_modes_hash,
                    },
                data,
                ..
            } => {
                return Some((
                    *focused,
                    encoder.as_str(),
                    terminal_modes_hash.as_str(),
                    data.as_bytes(),
                ));
            }
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        focus_events.as_slice(),
        [(true, "libghostty-focus-encoder-v1", first_hash, b"\x1b[I"),
         (false, "libghostty-focus-encoder-v1", second_hash, b"\x1b[O")]
            if first_hash.len() == 64 && second_hash.len() == 64
    ));
}

#[test]
fn pointer_click_uses_live_sgr_mode_geometry_and_audit_descriptor() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("pointer-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args([
            "record",
            "--no-input",
            "--cols",
            "80",
            "--rows",
            "24",
            "--cell-width",
            "10",
            "--cell-height",
            "20",
            "--out",
        ])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty\ntty.setraw(0)\nos.write(1,b'\\x1b[?1000h\\x1b[?1006hREADY')\nd=b''\nwhile len(d)<18: d+=os.read(0,18-len(d))\nos.write(1,b'POINTER:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pointer fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("pointer mode ready");

    let mut pointer = Command::cargo_bin("termtrace").expect("binary should build");
    pointer
        .args(["control", "pointer"])
        .arg(&session)
        .args(["click", "--cell", "2,3", "--button", "left", "--strict"])
        .assert()
        .success();
    let output = child
        .wait_with_output()
        .expect("pointer fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let pointer_event = log.events.iter().find_map(|record| match &record.event {
        Event::InputIntent {
            input_kind: InputKind::Pointer,
            descriptor:
                InputDescriptor::Pointer {
                    operation,
                    button,
                    lines,
                    column,
                    row,
                    end_column,
                    end_row,
                    click_count,
                    cols,
                    rows,
                    pixel_width,
                    pixel_height,
                    encoder,
                    terminal_modes_hash,
                },
            data,
            ..
        } => {
            return Some((
                operation,
                button,
                *lines,
                *column,
                *row,
                *end_column,
                *end_row,
                *click_count,
                *cols,
                *rows,
                *pixel_width,
                *pixel_height,
                encoder,
                terminal_modes_hash,
                data.as_bytes(),
            ));
        }
        _ => return None,
    });
    let Some((
        operation,
        button,
        lines,
        column,
        row,
        end_column,
        end_row,
        click_count,
        cols,
        rows,
        pixel_width,
        pixel_height,
        encoder,
        modes_hash,
        data,
    )) = pointer_event
    else {
        panic!("missing pointer intent");
    };
    assert_eq!(operation, "click");
    assert_eq!(button, "left");
    assert_eq!((lines, column, row), (0, 2, 3));
    assert_eq!((end_column, end_row, click_count), (None, None, 1));
    assert_eq!((cols, rows, pixel_width, pixel_height), (80, 24, 800, 480));
    assert_eq!(encoder, "libghostty-mouse-encoder-v1");
    assert_eq!(modes_hash.len(), 64);
    assert_eq!(data, b"\x1b[<0;3;4M\x1b[<0;3;4m");
}

#[test]
fn pointer_scroll_accepts_separated_and_equals_negative_values() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("pointer-scroll-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args([
            "record",
            "--no-input",
            "--cols",
            "80",
            "--rows",
            "24",
            "--out",
        ])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf '\\033[?1000h\\033[?1006hREADY'; while :; do sleep .1; done",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pointer scroll fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("pointer mode ready");

    let mut separated = Command::cargo_bin("termtrace").expect("binary should build");
    separated
        .args(["control", "pointer"])
        .arg(&session)
        .args(["scroll", "--cell", "2,3", "--lines", "-2", "--strict"])
        .assert()
        .success();
    let mut equals = Command::cargo_bin("termtrace").expect("binary should build");
    equals
        .args(["control", "pointer"])
        .arg(&session)
        .args(["scroll", "--cell", "4,5", "--lines=-3", "--strict"])
        .assert()
        .success();

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.arg("stop")
        .arg(&session)
        .arg("--wait")
        .assert()
        .success();
    let output = child
        .wait_with_output()
        .expect("pointer fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let scrolls = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                input_kind: InputKind::Pointer,
                data,
                descriptor:
                    InputDescriptor::Pointer {
                        operation,
                        button,
                        lines,
                        column,
                        row,
                        click_count,
                        ..
                    },
                ..
            } if operation == "scroll" => {
                return Some((
                    *lines,
                    *column,
                    *row,
                    button.as_str(),
                    *click_count,
                    data.as_bytes(),
                ));
            }
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        scrolls,
        [
            (
                -2,
                2,
                3,
                "wheel-up",
                0,
                b"\x1b[<64;3;4M\x1b[<64;3;4M".as_slice(),
            ),
            (
                -3,
                4,
                5,
                "wheel-up",
                0,
                b"\x1b[<64;5;6M\x1b[<64;5;6M\x1b[<64;5;6M".as_slice(),
            ),
        ]
    );
}

#[test]
fn activate_resolves_visible_text_rejects_ambiguity_and_records_region() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("activate-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--cols", "80", "--rows", "24", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty\ntty.setraw(0)\nos.write(1,b'\\x1b[?1000h\\x1b[?1006hSave changes Save changes')\nd=b''\nwhile len(d)<18: d+=os.read(0,18-len(d))\nos.write(1,b'ACTIVATED:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn activation fixture");
    wait_for_control(&session);
    wait_for_output(
        &session,
        b"Save changes Save changes",
        Duration::from_secs(5),
    )
    .expect("activation text ready");

    let mut ambiguous = Command::cargo_bin("termtrace").expect("binary should build");
    ambiguous
        .args(["control", "activate"])
        .arg(&session)
        .args(["--text", "Save changes", "--unique", "--strict"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            r#""code":"activation_text_ambiguous""#,
        ))
        .stdout(predicate::str::contains(r#""applied":"none""#))
        .stdout(predicate::str::contains(r#""retry":"safe_new_request""#))
        .stderr(predicate::str::is_empty());
    let mut activate = Command::cargo_bin("termtrace").expect("binary should build");
    activate
        .args(["control", "activate"])
        .arg(&session)
        .args(["--text", "Save changes", "--strict"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"activated""#));
    let output = child
        .wait_with_output()
        .expect("activation fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let activations = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                descriptor:
                    InputDescriptor::Activate {
                        text,
                        start_column,
                        end_column,
                        row,
                        target_column,
                        presented_state_hash,
                        encoder,
                        terminal_modes_hash,
                        ..
                    },
                data,
                ..
            } => {
                return Some((
                    text,
                    *start_column,
                    *end_column,
                    *row,
                    *target_column,
                    presented_state_hash,
                    encoder,
                    terminal_modes_hash,
                    data.as_bytes(),
                ));
            }
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        activations.as_slice(),
        [(text, 0, 12, 0, 5, state_hash, encoder, modes_hash, data)]
            if text.as_str() == "Save changes"
                && encoder.as_str() == "libghostty-mouse-encoder-v1"
                && state_hash.len() == 64
                && modes_hash.len() == 64
                && *data == b"\x1b[<0;6;1M\x1b[<0;6;1m"
    ));
}

#[test]
fn paced_unicode_text_splits_only_at_scalar_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("paced-session");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'READY'); d=b''\nwhile len(d)<6: d+=os.read(0,6-len(d))\nos.write(1,b'GOT:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn paced fixture");
    wait_for_control(&session);
    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.arg("wait")
        .arg(&session)
        .args(["--output", "READY", "--timeout-ms", "5000"])
        .assert()
        .success();

    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    let sent = send
        .arg("send")
        .arg(&session)
        .args(["--pace", "1ms", "--verbose", "text:é界", "enter"])
        .assert()
        .success();
    let response: serde_json::Value =
        serde_json::from_slice(&sent.get_output().stdout).expect("sequence response");
    assert_eq!(
        response
            .pointer("/outcomes")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(3)
    );
    let output = child.wait_with_output().expect("paced fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");
    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let text_chunks = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                input_kind: InputKind::Text,
                data,
                ..
            } => return Some(data.as_bytes()),
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert_eq!(text_chunks, vec!["é".as_bytes(), "界".as_bytes()]);
    assert!(
        text_chunks
            .iter()
            .all(|chunk| return str::from_utf8(chunk).is_ok())
    );
}

#[test]
fn semantic_paste_uses_live_bracketed_mode_and_records_wrappers() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("paste-session");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[?2004hREADY'); d=b''\nwhile len(d)<19: d+=os.read(0,19-len(d))\nos.write(1,b'PASTE:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn paste fixture");
    wait_for_control(&session);
    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.arg("wait")
        .arg(&session)
        .args(["--output", "READY", "--timeout-ms", "5000"])
        .assert()
        .success();
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.arg("send")
        .arg(&session)
        .arg("paste:one\ntwo")
        .assert()
        .success();
    let output = child.wait_with_output().expect("paste fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");
    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            input_kind: InputKind::Paste,
            descriptor: InputDescriptor::Paste {
                bracketed: true,
                terminal_modes_hash,
            },
            data,
            ..
        } if terminal_modes_hash.len() == 64
            && data.as_bytes() == b"\x1b[200~one\ntwo\x1b[201~"
    )));
}

#[test]
fn paced_sequence_encodes_each_atom_after_pumping_mode_changes() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("paced-mode-sequence");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,select,tty; tty.setraw(0); os.write(1,b'READY'); first=os.read(0,1); os.write(1,b'\\x1b[?2004h'); ready,_,_=select.select([0],[],[],2); rest=os.read(0,64) if ready else b''; os.write(1,b'SEQ:'+first.hex().encode()+b':'+rest.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn paced mode fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"READY", Duration::from_secs(5)).expect("ready output");

    let caller_intent = "verify current modes for every atom";
    let request_id = uuid::Uuid::now_v7();
    let request_id_text = request_id.to_string();
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    let sent = send
        .arg("send")
        .arg(&session)
        .args([
            "--request-id",
            &request_id_text,
            "--pace",
            "200ms",
            "--intent",
            caller_intent,
            "--verbose",
            "raw:x",
            "paste:z",
        ])
        .assert()
        .success();
    let response: serde_json::Value =
        serde_json::from_slice(&sent.get_output().stdout).expect("sequence response");
    assert_eq!(
        response
            .pointer("/request_id")
            .and_then(serde_json::Value::as_str),
        Some(request_id_text.as_str())
    );
    assert_eq!(
        response
            .pointer("/outcomes")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    let output = child.wait_with_output().expect("paced mode fixture");
    assert!(output.status.success(), "fixture failed: {output:?}");
    wait_for_output(
        &session,
        b"SEQ:78:1b5b3230307e7a1b5b3230317e",
        Duration::from_secs(5),
    )
    .expect("mode-sensitive sequence output");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let inputs = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                source: InputSource::Agent,
                terminal_modes_hash: Some(mode_hash),
                semantic_input: Some(semantic_input),
                caller_intent: Some(intent),
                ..
            } if intent == caller_intent => {
                return Some((mode_hash.as_str(), semantic_input.as_str()));
            }
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0].1, "raw");
    assert_eq!(inputs[1].1, "paste");
    assert_ne!(inputs[0].0, inputs[1].0);
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            descriptor: InputDescriptor::Paste { bracketed: true, .. },
            data,
            ..
        } if data.as_bytes() == b"\x1b[200~z\x1b[201~"
    )));
}

#[test]
fn actor_snapshot_excludes_uncommitted_synchronized_output() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("snapshot-session");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,signal,sys,time,tty; tty.setraw(0); signal.signal(signal.SIGTERM, lambda *_: sys.exit(0)); os.write(1,b'VISIBLE\\x1b[?2026hHIDDEN'); os.read(0,1); os.write(1,b'\\r\\x1b[2KBUSY\\x1b[?2026l'); time.sleep(30)",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn snapshot fixture");
    wait_for_control(&session);
    wait_for_output(&session, b"HIDDEN", Duration::from_secs(5)).expect("hidden output");
    let mut visible = Command::cargo_bin("termtrace").expect("binary should build");
    visible
        .arg("wait")
        .arg(&session)
        .args(["VISIBLE", "--timeout-ms", "5000"])
        .assert()
        .success();
    let hidden = send_request(&session, &ControlRequest::new(ControlCommand::Snapshot))
        .expect("hidden snapshot");
    let hidden_snapshot = match hidden {
        ControlData::Snapshot {
            viewport_text,
            presented,
            ..
        } => Some((viewport_text, presented)),
        _ => None,
    }
    .expect("snapshot response");
    let (viewport_text, presented) = hidden_snapshot;
    assert!(presented);
    assert!(viewport_text.contains("VISIBLE"));
    assert!(!viewport_text.contains("HIDDEN"));

    let mut release = Command::cargo_bin("termtrace").expect("binary should build");
    release
        .arg("send")
        .arg(&session)
        .arg("text:x")
        .assert()
        .success();
    wait_for_output(&session, b"BUSY", Duration::from_secs(5)).expect("committed output");
    let committed = send_request(&session, &ControlRequest::new(ControlCommand::Snapshot))
        .expect("committed snapshot");
    let viewport_text = match committed {
        ControlData::Snapshot { viewport_text, .. } => Some(viewport_text),
        _ => None,
    }
    .expect("snapshot response");
    assert!(viewport_text.contains("BUSY"));
    let mut visible_wait = Command::cargo_bin("termtrace").expect("binary should build");
    visible_wait
        .arg("wait")
        .arg(&session)
        .args(["BUSY", "--stable-for", "50ms", "--timeout-ms", "1000"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""scope":"screen""#))
        .stdout(predicate::str::contains(r#""quality":"stable""#));
    let mut absent_wait = Command::cargo_bin("termtrace").expect("binary should build");
    absent_wait
        .arg("wait")
        .arg(&session)
        .args([
            "--screen-absent",
            "VISIBLE",
            "--stable-for",
            "20ms",
            "--timeout-ms",
            "1000",
        ])
        .assert()
        .success();
    for arguments in [
        ["--screen-regex", "B.SY"],
        ["--row-contains", "0:BUSY"],
        ["--region-contains", "0,0,4,1:BUSY"],
        ["--cursor-at", "4,0"],
        ["--mode", "wraparound"],
    ] {
        let mut structural_wait = Command::cargo_bin("termtrace").expect("binary should build");
        structural_wait
            .arg("wait")
            .arg(&session)
            .args(arguments)
            .args(["--timeout-ms", "1000"])
            .assert()
            .success();
    }
    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.arg("stop")
        .arg(&session)
        .arg("--wait")
        .assert()
        .success();
    let output = child
        .wait_with_output()
        .expect("snapshot fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");
}
