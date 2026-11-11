//! launch validation and stdio CLI integration tests.

use crate::support::*;

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
            "required arguments were not provided:\n  --viewport <VIEWPORT>",
        ));
}

#[test]
fn visual_convenience_flags_reject_invalid_values_before_recording() {
    for (flag, value, error) in [
        (
            "--font-size",
            "0",
            "font size must be a positive finite number",
        ),
        ("--background", "black", "color must use #RRGGBB syntax"),
    ] {
        Command::cargo_bin("termtrace")
            .expect("binary should build")
            .args(["record", flag, value, "--out", "unused", "--", "/bin/true"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(error));
    }
}

#[test]
fn visual_profile_rejects_invalid_requests_before_the_application_starts() {
    let temporary = TempDir::new().expect("temporary directory");
    let marker = temporary.path().join("started");
    let config = temporary.path().join("ghostty.config");
    fs::write(&config, "font-size = 12\n").expect("ghostty config");

    for (arguments, error) in [
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"native":{"font_family":"Example Mono\nbackground = #000000"}}"#,
            ],
            "control characters",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"native":{"font_size_points":"NaN"}}"#,
            ],
            "font_size_points",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"native":{"font_size_points":"0"}}"#,
            ],
            "greater than zero",
        ),
        (
            vec![
                "--visual-profile",
                r##"{"version":1,"native":{"foreground":"#fff"}}"##,
            ],
            "#RRGGBB",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"semantic":{"preset":"bundled-v1","font_family":"Example Mono"}}"#,
            ],
            "unknown field",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"semantic":{"preset":"custom"}}"#,
            ],
            "unknown variant",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":2,"semantic":{"preset":"bundled-v1"}}"#,
            ],
            "unsupported visual profile version 2",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"semantic":{"preset":"bundled-v1"}}"#,
                "--font-size",
                "14",
            ],
            "--visual-profile replaces",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"semantic":{"preset":"bundled-v1"}}"#,
                "--ghostty-config",
                config.to_str().expect("UTF-8 path"),
            ],
            "--visual-profile replaces",
        ),
        (
            vec![
                "--visual-profile",
                r#"{"version":1,"semantic":{"preset":"bundled-v1"}}"#,
                "--geometry",
                "fit",
                "--viewport",
                "640x400",
            ],
            "requires a native visual profile section",
        ),
    ] {
        let session = temporary.path().join("rejected");
        Command::cargo_bin("termtrace")
            .expect("binary should build")
            .env("PATH", "")
            .arg("record")
            .args(&arguments)
            .arg("--out")
            .arg(&session)
            .args(["--", "/bin/sh", "-c", "printf started > \"$1\"", "probe"])
            .arg(&marker)
            .assert()
            .failure()
            .stderr(predicate::str::contains(error));
        assert!(!marker.exists(), "{arguments:?} started the application");
        assert!(!session.exists(), "{arguments:?} created a session");
    }
}

#[test]
fn rust_launch_spec_commits_the_same_visual_profile_contract() {
    let temporary = TempDir::new().expect("temporary directory");
    let root = temporary.path().join("evidence");
    let marker = temporary.path().join("started");
    let terminal = ClientTerminal::new(env!("CARGO_BIN_EXE_termtrace"), &root);
    let command = [
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from("printf started > \"$1\""),
        OsString::from("probe"),
        marker.clone().into_os_string(),
    ];

    let mut invalid = VisualProfile::native_standard();
    if let Some(native) = invalid.native.as_mut() {
        native.font_size_points = Some("NaN".to_owned());
    }

    let Err(error) = terminal.start(LaunchSpec::new(command.clone()).visual_profile(invalid))
    else {
        panic!("invalid profile must not launch");
    };

    assert_eq!(
        (error.kind().as_str(), error.code()),
        ("invalid_operation", "invalid_visual_profile")
    );
    assert!(!marker.exists());
    assert!(!root.exists());

    let session = terminal
        .start(LaunchSpec::new(command).visual_profile(VisualProfile::semantic_bundled()))
        .expect("semantic profile launch");

    let deadline = Instant::now() + Duration::from_secs(10);
    while session.completion().expect("completion").is_none() {
        assert!(
            Instant::now() < deadline,
            "profiled session did not complete"
        );

        thread::sleep(Duration::from_millis(10));
    }

    let manifest = read_manifest(session.path()).expect("profiled manifest");
    assert_eq!(
        manifest.visual_profile,
        Some(
            VisualProfile::semantic_bundled()
                .record()
                .expect("valid profile")
        )
    );
    assert!(marker.exists());
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
