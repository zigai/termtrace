//! adoption surfaces CLI integration tests.

use crate::support::*;
use std::os::{
    linux::net::SocketAddrExt as _,
    unix::net::{SocketAddr, UnixListener},
};

#[test]
fn framework_neutral_terminal_fixtures_cover_input_modes_and_queries() {
    let temporary = TempDir::new().expect("temporary directory");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/programs/terminal");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("fixture-evidence"),
    );

    let mut line = terminal
        .start(LaunchSpec::new([fixtures
            .join("line-command.sh")
            .into_os_string()]))
        .expect("launch line fixture");

    line.wait_screen(
        ScreenPredicate::Contains {
            text: "LINE_READY".to_owned(),
        },
        Duration::ZERO,
        Duration::from_secs(5),
    )
    .expect("line fixture ready");
    line.wait_screen(
        ScreenPredicate::All {
            predicates: vec![
                ScreenPredicate::MatchesRegex {
                    pattern: "LINE_R[A-Z]+".to_owned(),
                },
                ScreenPredicate::RowContains {
                    row: 0,
                    text: "LINE_READY".to_owned(),
                },
                ScreenPredicate::RegionContains {
                    region: ScreenRegion {
                        left: 0,
                        top: 0,
                        right: 10,
                        bottom: 1,
                    },
                    text: "LINE_READY".to_owned(),
                },
                ScreenPredicate::Cursor {
                    column: None,
                    row: None,
                    visible: Some(true),
                    shape: None,
                },
                ScreenPredicate::Mode {
                    mode: ScreenMode::Wraparound,
                    enabled: true,
                },
                ScreenPredicate::StateHash {
                    hash: "0".repeat(64),
                    comparison: StateHashComparison::Changed,
                },
                ScreenPredicate::Any {
                    predicates: vec![
                        ScreenPredicate::Contains {
                            text: "missing".to_owned(),
                        },
                        ScreenPredicate::ExcludesRegex {
                            pattern: "BUSY$".to_owned(),
                        },
                    ],
                },
            ],
        },
        Duration::ZERO,
        Duration::from_secs(5),
    )
    .expect("structural screen predicate");

    let state_hash = match send_request(line.path(), &ControlRequest::new(ControlCommand::Snapshot))
        .expect("line snapshot")
    {
        ControlPayload::Snapshot { state_hash, .. } => state_hash,
        other => panic!("unexpected snapshot response: {other:?}"),
    };

    line.wait_screen(
        ScreenPredicate::StateHash {
            hash: state_hash,
            comparison: StateHashComparison::Unchanged,
        },
        Duration::ZERO,
        Duration::from_secs(5),
    )
    .expect("unchanged screen predicate");
    line.act(ActRequest::new().text("fixture-value\n"))
        .expect("line input");
    wait_for_output(
        line.path(),
        b"LINE_VALUE=fixture-value",
        Duration::from_secs(5),
    )
    .expect("line fixture output");
    line.complete(Duration::from_secs(5))
        .expect("line completion");

    let mut alternate = terminal
        .start(LaunchSpec::new([fixtures
            .join("alternate-screen.py")
            .into_os_string()]))
        .expect("launch alternate-screen fixture");

    alternate
        .wait_screen(
            ScreenPredicate::Contains {
                text: "ALTERNATE_READY".to_owned(),
            },
            Duration::ZERO,
            Duration::from_secs(5),
        )
        .expect("alternate fixture ready");
    alternate
        .act(ActRequest::new().key(KeyChord {
            key: NamedKey::ArrowRight,
            modifiers: KeyModifiers::default(),
        }))
        .expect("mode-aware arrow");
    wait_for_output(alternate.path(), b"INPUT_HEX=", Duration::from_secs(5))
        .expect("alternate fixture input evidence");
    alternate
        .complete(Duration::from_secs(5))
        .expect("alternate completion");

    let mut final_state = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("printf FINAL_READY; sleep .2"),
        ]))
        .expect("launch final-state fixture");

    let final_wait = final_state
        .wait_screen(
            ScreenPredicate::Contains {
                text: "FINAL_READY".to_owned(),
            },
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .expect("completed final state is stable");
    assert!(final_wait.is_satisfied());
    assert!(matches!(
        final_wait.quality,
        ScreenWaitQuality::ChildCompleted | ScreenWaitQuality::Completed
    ));

    final_state
        .complete(Duration::from_secs(5))
        .expect("final-state completion");

    let query_session = temporary.path().join("query-heavy");
    let mut query = Command::cargo_bin("termtrace").expect("binary should build");
    query
        .args(["record", "--no-input", "--out"])
        .arg(&query_session)
        .arg("--")
        .arg(fixtures.join("query-heavy.py"))
        .assert()
        .success();

    wait_for_output(
        &query_session,
        b"QUERY_RESPONSES_OK",
        Duration::from_secs(1),
    )
    .expect("query response fixture output");

    let log = EventLog::read(query_session.join("events.jsonl")).expect("query event log");
    assert!(log.events.iter().any(|record| {
        return matches!(record.event, Event::QueryResponse { .. });
    }));
}

#[test]
fn blocking_rust_facade_launches_drives_and_completes_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("client-evidence"),
    );

    let mut session = terminal
        .start(
            LaunchSpec::new([
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("printf READY; IFS= read -r line; printf '<%s>' \"$line\""),
            ])
            .geometry(90, 25),
        )
        .expect("launch through Rust facade");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5)).expect("ready output");
    let frame = session
        .frame(ScreenProjectionRequest::default())
        .expect("screen frame");

    assert_eq!(frame.geometry.cols, 90);
    assert_eq!(frame.geometry.rows, 25);
    assert!(session
        .observe(StatePoint::LatestPresented, false)
        .expect("screen observation")
        .frame
        .viewport_text
        .contains("READY"));

    session
        .wait_screen(
            ScreenPredicate::Contains {
                text: "READY".to_owned(),
            },
            Duration::ZERO,
            Duration::from_secs(5),
        )
        .expect("facade screen wait");

    assert_eq!(
        session
            .capture_stable(StableCaptureOptions {
                stable_for: Duration::from_millis(20),
                timeout: Duration::from_secs(2),
                projection: ScreenProjectionRequest::default(),
                allow_incomplete: false,
                reaction: None,
            })
            .expect("facade atomic stable capture")
            .quality,
        StableCaptureQuality::Stable
    );

    session
        .act(ActRequest::new().text("client\n"))
        .expect("typed client input");
    wait_for_output(session.path(), b"<client>", Duration::from_secs(5)).expect("client output");
    let completion = session
        .complete(Duration::from_secs(5))
        .expect("completed client evidence");
    assert_eq!(completion.session_id, session.session_id());
    assert!(
        session
            .observe(StatePoint::LatestPresented, false)
            .expect("semantic inspection")
            .source_completed
    );
    assert!(!session
        .semantic()
        .timeline(false, true)
        .expect("timeline")
        .is_empty());
    assert!(
        session
            .semantic()
            .diff(
                StatePoint::StateId { state_id: 0 },
                StatePoint::LatestPresented
            )
            .expect("semantic diff")
            .changed
    );

    session
        .semantic()
        .diagnose(vec![DiagnosticCheck::Dirty], 50)
        .expect("semantic diagnosis");

    assert!(!session
        .events()
        .page(0, 32, true)
        .expect("event page")
        .0
        .is_empty());
}

#[test]
fn rust_facade_bridges_actor_shutdown_to_completion_publication() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("short-lived-session");

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf TRANSITION"])
        .assert()
        .success();

    let completion = session.join("completion.json");
    let pending_completion = session.join("completion.pending.json");
    fs::rename(&completion, &pending_completion).expect("stage delayed completion");

    let manifest = read_manifest(&session).expect("read short-lived session manifest");
    let address = SocketAddr::from_abstract_name(manifest.control_socket.as_bytes())
        .expect("construct fake control address");
    let listener = UnixListener::bind_addr(&address).expect("bind fake stopping control endpoint");
    let restore_source = pending_completion.clone();
    let restore_destination = completion.clone();

    let restore = thread::spawn(move || {
        let (mut connection, _) = listener.accept().expect("accept snapshot request");
        let mut request: Vec<u8> = Vec::new();
        connection
            .read_to_end(&mut request)
            .expect("read snapshot request");
        assert!(!request.is_empty(), "snapshot request was not dispatched");

        drop(connection);

        thread::sleep(Duration::from_millis(100));

        fs::rename(restore_source, restore_destination).expect("publish delayed completion");
    });

    let attached = termtrace_sdk::Session::attach(&session, env!("CARGO_BIN_EXE_termtrace"))
        .expect("attach short-lived session");

    let observation = attached
        .observe(StatePoint::LatestPresented, false)
        .expect("bridge observation across completion publication");

    restore.join().expect("join completion publisher");

    assert!(observation.source_completed);

    fs::rename(&completion, &pending_completion).expect("restage delayed completion");
    let restore_source = pending_completion;
    let restore_destination = completion;

    let restore = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));

        fs::rename(restore_source, restore_destination).expect("republish delayed completion");
    });

    let (_, state_hash) = attached
        .semantic_state(&StatePoint::LatestPresented, true)
        .expect("bridge semantic state across completion publication");

    restore.join().expect("join semantic completion publisher");

    assert_eq!(state_hash, observation.state_hash);
}

#[test]
fn rust_facade_attach_retains_the_canonical_session_path() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("evidence"),
    );

    let mut recording = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("sleep .2"),
        ]))
        .expect("launch completed recording");
    recording
        .complete(Duration::from_secs(5))
        .expect("complete recording");
    let canonical = recording
        .path()
        .canonicalize()
        .expect("canonical recording path");

    drop(recording);

    let reference = temporary.path().join("session-link");
    symlink(&canonical, &reference).expect("link session");
    let attached = terminal.attach(&reference).expect("attach through symlink");
    fs::remove_file(&reference).expect("remove original link");
    symlink(temporary.path(), &reference).expect("retarget session link");

    assert_eq!(attached.path(), canonical);

    attached
        .observe(StatePoint::LatestPresented, false)
        .expect("inspect original attached session");
}
