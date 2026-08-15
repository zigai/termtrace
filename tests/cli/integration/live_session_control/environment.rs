use super::*;

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
