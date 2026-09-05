use super::*;

#[test]
fn restart_publishes_an_evidence_bound_successor_without_mutating_predecessor() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let working = temporary.path().join("working");
    fs_err::create_dir(&working).expect("working directory");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    let started = start
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["start", "demo", "--cwd"])
        .arg(&working)
        .args([
            "--cols",
            "91",
            "--rows",
            "27",
            "--cell-width",
            "8",
            "--cell-height",
            "16",
            "--term",
            "xterm-256color",
            "--durability",
            "strict",
            "--provenance",
            "reproducible",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 0.1; done",
        ])
        .assert()
        .success();
    let started_json: serde_json::Value =
        serde_json::from_slice(&started.get_output().stdout).expect("start response");
    let predecessor = PathBuf::from(
        started_json
            .pointer("/path")
            .and_then(serde_json::Value::as_str)
            .expect("predecessor path"),
    );
    let predecessor_manifest = read_manifest(&predecessor).expect("predecessor manifest");

    let mut restart = Command::cargo_bin("termtrace").expect("binary should build");
    let restarted = restart
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "restart", "demo"])
        .assert()
        .success();
    let response: serde_json::Value =
        serde_json::from_slice(&restarted.get_output().stdout).expect("restart response");
    let successor_bytes: ByteString = serde_json::from_value(
        response
            .pointer("/successor/canonical_session")
            .expect("successor path")
            .clone(),
    )
    .expect("successor path bytes");
    let successor = PathBuf::from(OsString::from_vec(successor_bytes.as_bytes().to_vec()));
    assert_eq!(
        response["predecessor"]["canonical_session_display"],
        predecessor.display().to_string()
    );
    assert_eq!(
        response["successor"]["canonical_session_display"],
        successor.display().to_string()
    );
    assert_ne!(predecessor, successor);
    let predecessor_evidence = ["session.json", "events.jsonl", "completion.json"]
        .map(|name| return fs_err::read(predecessor.join(name)).expect("predecessor evidence"));
    let successor_manifest = read_manifest(&successor).expect("successor manifest");
    assert_eq!(successor_manifest.command, predecessor_manifest.command);
    assert_eq!(
        successor_manifest.working_directory,
        predecessor_manifest.working_directory
    );
    assert_eq!(successor_manifest.geometry, predecessor_manifest.geometry);
    assert_eq!(
        successor_manifest.geometry_provenance,
        predecessor_manifest.geometry_provenance
    );
    assert_eq!(
        successor_manifest.durability_policy,
        predecessor_manifest.durability_policy
    );
    assert_eq!(
        successor_manifest.execution_provenance.policy,
        predecessor_manifest.execution_provenance.policy
    );
    let completion: SessionCompletion =
        serde_json::from_slice(&predecessor_evidence[2]).expect("predecessor completion");
    let lineage = successor_manifest.lineage.expect("restart lineage");
    assert_eq!(
        lineage.predecessor_session_id,
        predecessor_manifest.session_id
    );
    assert_eq!(
        lineage.predecessor_manifest_hash,
        blake3::hash(&predecessor_evidence[0]).to_hex().to_string()
    );
    assert_eq!(
        lineage.predecessor_final_event_hash,
        completion.final_event_hash
    );

    let mut resolve = Command::cargo_bin("termtrace").expect("binary should build");
    resolve
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "resolve", "demo"])
        .assert()
        .success()
        .stdout(format!("{}\n", successor.display()));
    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "demo", "--wait"])
        .assert()
        .success();
    for (name, expected) in ["session.json", "events.jsonl", "completion.json"]
        .into_iter()
        .zip(predecessor_evidence)
    {
        assert_eq!(fs_err::read(predecessor.join(name)).unwrap(), expected);
    }
}

#[test]
fn restart_applies_only_explicit_successor_overrides() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let changed_cwd = temporary.path().join("changed");
    fs_err::create_dir(&changed_cwd).expect("changed cwd");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "demo",
            "--cols",
            "80",
            "--rows",
            "24",
            "--durability",
            "strict",
            "--provenance",
            "reproducible",
            "--clear-env",
            "--copy-env",
            "PATH",
            "--color-policy",
            "never",
            "--output-byte-budget",
            "1000000",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 0.1; done",
        ])
        .assert()
        .success();

    let mut restart = Command::cargo_bin("termtrace").expect("binary should build");
    let restarted = restart
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "restart", "demo", "--cwd"])
        .arg(&changed_cwd)
        .args([
            "--cols",
            "100",
            "--cell-width",
            "7",
            "--term",
            "vt100",
            "--durability",
            "bounded",
            "--durability-max-interval-ms",
            "25",
            "--durability-max-uncommitted-bytes",
            "4096",
            "--provenance",
            "minimal",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep 0.2; done",
        ])
        .assert()
        .success();
    let response: serde_json::Value =
        serde_json::from_slice(&restarted.get_output().stdout).expect("restart response");
    let successor_bytes: ByteString = serde_json::from_value(
        response
            .pointer("/successor/canonical_session")
            .expect("successor path")
            .clone(),
    )
    .expect("successor path bytes");
    let successor = PathBuf::from(OsString::from_vec(successor_bytes.as_bytes().to_vec()));
    let manifest = read_manifest(&successor).expect("successor manifest");
    assert_eq!(
        manifest.working_directory.as_ref().unwrap().as_bytes(),
        changed_cwd.as_os_str().as_bytes()
    );
    assert_eq!((manifest.geometry.cols, manifest.geometry.rows), (100, 24));
    assert_eq!(
        (
            manifest.geometry.pixel_width,
            manifest.geometry.pixel_height
        ),
        (700, 576)
    );
    assert_eq!(
        manifest
            .execution_provenance
            .environment
            .get("TERM")
            .and_then(|item| return item.value.as_ref())
            .map(|value| return value.as_bytes()),
        Some(b"vt100".as_slice())
    );
    assert_eq!(manifest.durability_policy.maximum_interval_ms, Some(25));
    assert_eq!(
        manifest.durability_policy.maximum_uncommitted_bytes,
        Some(4096)
    );
    assert_eq!(
        manifest.execution_provenance.policy.kind,
        ProvenancePolicyKind::Minimal
    );
    assert_eq!(manifest.child_environment.base, EnvironmentBase::Clear);
    assert_eq!(
        manifest.child_environment.copied_names,
        BTreeSet::from(["PATH".to_owned()])
    );
    assert_eq!(manifest.child_environment.color, ColorPolicy::Never);
    assert_eq!(manifest.resource_limits.output_bytes, Some(1_000_000));
    assert_eq!(
        manifest.command[2].as_bytes(),
        b"trap 'exit 0' TERM; while :; do sleep 0.2; done"
    );

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "demo", "--wait"])
        .assert()
        .success();
}

#[test]
fn restart_verifies_resupplied_environment_and_requires_explicit_change() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let original_path = temporary.path().join("original-environment.json");
    let changed_path = temporary.path().join("changed-environment.json");
    let original = EnvironmentSpec {
        base: EnvironmentBase::Clear,
        set: BTreeMap::from([(
            "RESTART_SECRET".to_owned(),
            ByteString::from(b"original-private".as_slice()),
        )]),
        copy: BTreeSet::from(["PATH".to_owned()]),
        ..EnvironmentSpec::default()
    };
    let mut changed = original.clone();
    changed.set.insert(
        "RESTART_SECRET".to_owned(),
        ByteString::from(b"changed-private".as_slice()),
    );
    for (path, spec) in [(&original_path, &original), (&changed_path, &changed)] {
        fs_err::write(path, serde_json::to_vec(spec).expect("environment JSON"))
            .expect("environment file");
        fs_err::set_permissions(path, Permissions::from_mode(0o600)).expect("private spec");
    }
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["start", "secret-restart", "--environment-spec-stdin"])
        .args([
            "--provenance",
            "custom",
            "--provenance-env",
            "PATH=hash",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep .1; done",
        ])
        .write_stdin(serde_json::to_vec(&original).expect("start environment JSON"))
        .assert()
        .success();

    let mut exact = Command::cargo_bin("termtrace").expect("binary should build");
    exact
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["restart", "secret-restart", "--environment-spec"])
        .arg(&original_path)
        .assert()
        .success();

    let mut rejected = Command::cargo_bin("termtrace").expect("binary should build");
    rejected
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["restart", "secret-restart", "--environment-spec"])
        .arg(&changed_path)
        .assert()
        .failure();
    let supervisor_errors = fs_err::read_dir(state.join("supervisor"))
        .expect("supervisor directory")
        .filter_map(|entry| return entry.ok().map(|entry| return entry.path()))
        .filter(|path| {
            return path
                .extension()
                .is_some_and(|extension| return extension == "stderr");
        })
        .filter_map(|path| return fs_err::read_to_string(path).ok())
        .collect::<String>();
    assert!(supervisor_errors.contains("does not match the predecessor"));

    let mut changed_restart = Command::cargo_bin("termtrace").expect("binary should build");
    changed_restart
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["restart", "secret-restart", "--environment-spec"])
        .arg(&changed_path)
        .arg("--allow-environment-change")
        .assert()
        .success();
    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "secret-restart", "--wait"])
        .assert()
        .success();
    let mut pending = vec![state];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                fs_err::read_dir(&path)
                    .expect("state directory")
                    .filter_map(|entry| return entry.ok().map(|entry| return entry.path())),
            );
        } else if let Ok(bytes) = fs_err::read(&path) {
            for secret in [
                b"original-private".as_slice(),
                b"changed-private".as_slice(),
            ] {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| return window == secret),
                    "secret leaked into {}",
                    path.display()
                );
            }
        }
    }
}
