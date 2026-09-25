use super::*;

#[test]
fn start_publishes_alias_only_after_detached_recorder_readiness() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "record",
            "-d",
            "demo",
            "--json",
            "--cols",
            "90",
            "--rows",
            "25",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY; while :; do sleep 0.1; done",
        ])
        .assert()
        .success();

    let response: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("start response");
    assert_eq!(
        response
            .pointer("/ready")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert!(response
        .pointer("/recorder_process_start_ticks")
        .and_then(serde_json::Value::as_u64)
        .is_some());

    let canonical: ByteString = serde_json::from_value(
        response
            .pointer("/canonical_session")
            .expect("canonical session")
            .clone(),
    )
    .expect("canonical path bytes");

    let session = PathBuf::from(OsString::from_vec(canonical.as_bytes().to_vec()));
    assert_eq!(
        response["canonical_session_display"],
        session.display().to_string()
    );

    let manifest = read_manifest(&session).expect("started manifest");
    assert_eq!(manifest.creation_alias.as_deref(), Some("demo"));
    assert_eq!((manifest.geometry.cols, manifest.geometry.rows), (90, 25));

    let mut status = Command::cargo_bin("termtrace").expect("binary should build");
    let status = status
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["status", "--json", "demo"])
        .assert()
        .success();

    let status = compact_json_line(&status.get_output().stdout);
    assert_eq!(status["lifecycle"], "live");
    assert_eq!(status["session"], "demo");
    assert_eq!(status["lifecycle"], "live");
    assert!(status["pid"].as_u64().is_some());

    let session_count = fs_err::read_dir(state.join("sessions"))
        .expect("sessions")
        .count();

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["record", "-d", "demo", "--", "/bin/true"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("alias is already"));

    assert_eq!(
        fs_err::read_dir(state.join("sessions"))
            .expect("sessions")
            .count(),
        session_count,
        "duplicate start must fail before allocating evidence"
    );

    for key in ["stdout_log", "stderr_log"] {
        let path = PathBuf::from(
            response
                .pointer(&format!("/{key}"))
                .and_then(serde_json::Value::as_str)
                .expect("diagnostic path"),
        );
        assert_eq!(
            fs_err::metadata(path)
                .expect("diagnostic metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "demo", "--wait"])
        .assert()
        .success();

    assert!(session.join("completion.json").is_file());

    let mut failed = Command::cargo_bin("termtrace").expect("binary should build");
    let failed = failed
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "record",
            "-d",
            "broken",
            "--json",
            "--",
            "/definitely/missing/termtrace-command",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let failed = compact_json_line(&failed);
    assert_eq!(failed["startup_state"], "completed_failure");

    let failed_session = PathBuf::from(
        failed["canonical_session_display"]
            .as_str()
            .or_else(|| return failed["path"].as_str())
            .expect("failed session path"),
    );
    assert!(failed_session.join("completion.json").is_file());
    assert!(
        state.join("registry/broken.json").is_file(),
        "valid failure evidence must remain addressable by alias"
    );
}

#[test]
fn start_promotes_fast_completion_and_reports_child_outcome() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut successful = Command::cargo_bin("termtrace").expect("binary should build");
    let successful = successful
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "record",
            "-d",
            "fast-success",
            "--json",
            "--",
            "/bin/sh",
            "-c",
            "exit 0",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let successful = compact_json_line(&successful);
    assert_eq!(successful["startup_state"], "completed_success");
    assert_eq!(successful["completion"]["exit_code"], 0);

    let mut wait_idle = Command::cargo_bin("termtrace").expect("binary should build");
    wait_idle
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["wait", "--json", "fast-success", "--idle"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""quality": "completed""#));

    let mut completed_screen = Command::cargo_bin("termtrace").expect("binary should build");
    completed_screen
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["screen", "--json", "fast-success"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle": "completed""#))
        .stdout(predicate::str::contains(r#""ended": true"#));

    let mut require_live_stability = Command::cargo_bin("termtrace").expect("binary should build");
    require_live_stability
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "wait",
            "--json",
            "fast-success",
            "--idle",
            "--require-live-stability",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""operation":"wait""#))
        .stdout(predicate::str::contains(r#""lifecycle":"completed""#));

    let mut unsuccessful = Command::cargo_bin("termtrace").expect("binary should build");
    let unsuccessful = unsuccessful
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "record",
            "-d",
            "fast-failure",
            "--json",
            "--",
            "/bin/sh",
            "-c",
            "exit 7",
        ])
        .assert()
        .code(7)
        .get_output()
        .stdout
        .clone();

    let unsuccessful = compact_json_line(&unsuccessful);
    assert_eq!(unsuccessful["startup_state"], "completed_failure");
    assert_eq!(unsuccessful["completion"]["exit_code"], 7);

    let mut status = Command::cargo_bin("termtrace").expect("binary should build");
    status
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["status", "--json", "fast-failure"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind": "completed""#));
}

#[test]
fn concurrent_start_reservation_launches_only_one_recorder() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let binary = assert_cmd::cargo::cargo_bin!("termtrace");
    let command = || {
        let mut command = StdCommand::new(binary);
        command
            .env("TERMTRACE_STATE_DIR", &state)
            .args([
                "record",
                "-d",
                "contended",
                "--json",
                "/bin/sh",
                "-c",
                "trap 'exit 0' TERM; while :; do sleep 0.1; done",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        return command;
    };
    let first = command().spawn().expect("first concurrent start");
    let second = command().spawn().expect("second concurrent start");
    let first = first.wait_with_output().expect("first start output");
    let second = second.wait_with_output().expect("second start output");
    assert_ne!(first.status.success(), second.status.success());

    let winner = if first.status.success() { &first } else { &second };
    let loser = if first.status.success() { &second } else { &first };
    let started = compact_json_line(&winner.stdout);
    assert_eq!(started["alias"], "contended");
    assert!(
        String::from_utf8_lossy(&loser.stderr).contains("reserved or registered"),
        "unexpected loser diagnostic: {}",
        String::from_utf8_lossy(&loser.stderr)
    );
    assert_eq!(
        fs_err::read_dir(state.join("sessions"))
            .expect("managed sessions")
            .count(),
        1,
        "losing start launched orphan evidence"
    );

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "contended", "--wait"])
        .assert()
        .success();
}
