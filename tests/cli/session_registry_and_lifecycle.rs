//! session registry and lifecycle CLI integration tests.

use crate::support::*;

#[test]
fn registry_binds_alias_to_canonical_session_identity() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("canonical-session");
    let state = temporary.path().join("state");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/true"])
        .assert()
        .success();
    let manifest = read_manifest(&session).expect("manifest");
    let evidence_before = ["session.json", "events.jsonl", "completion.json"]
        .into_iter()
        .map(|name| {
            return (
                name,
                fs_err::read(session.join(name)).expect("evidence file"),
            );
        })
        .collect::<Vec<_>>();

    let mut register = Command::cargo_bin("termtrace").expect("binary should build");
    let registered = register
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "register"])
        .arg("demo")
        .arg("--session")
        .arg(&session)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let registered = compact_json_line(&registered);
    assert_eq!(registered["session_id"], manifest.session_id.to_string());
    assert_eq!(
        registered["canonical_session_display"],
        session
            .canonicalize()
            .expect("canonical")
            .display()
            .to_string()
    );

    let mut resolve = Command::cargo_bin("termtrace").expect("binary should build");
    resolve
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "resolve", "demo"])
        .assert()
        .success()
        .stdout(format!(
            "{}\n",
            session.canonicalize().expect("canonical").display()
        ));

    let mut structured = Command::cargo_bin("termtrace").expect("binary should build");
    structured
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "resolve", "demo", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(manifest.session_id.to_string()))
        .stdout(predicate::str::contains(r#""alias":"demo""#));

    let entry_path = state.join("registry/demo.json");
    assert_eq!(
        fs_err::metadata(&entry_path)
            .expect("entry metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "register"])
        .arg("demo")
        .arg("--session")
        .arg(&session)
        .assert()
        .failure();

    let mut active_only = Command::cargo_bin("termtrace").expect("binary should build");
    active_only
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "list", "--json"])
        .assert()
        .success()
        .stdout("[]\n");
    let mut all = Command::cargo_bin("termtrace").expect("binary should build");
    let all = all
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "list", "--all", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"completed""#))
        .stdout(predicate::str::contains(r#""alias":"demo""#))
        .get_output()
        .stdout
        .clone();
    let rows: serde_json::Value = serde_json::from_slice(&all).expect("registry list JSON");
    let row = rows
        .as_array()
        .and_then(|rows| return rows.first())
        .expect("registry row");
    validate_json_value("registry-list-entry.schema.json", row);
    assert!(
        row.get("canonical_session_display").is_none(),
        "machine registry-list contract changed unexpectedly: {row}"
    );

    let mut human = Command::cargo_bin("termtrace").expect("binary should build");
    let human = human
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "list", "--all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).expect("registry table");
    let mut lines = human.lines();
    let header = lines.next().expect("registry table header");
    let row = lines.next().expect("registry table row");
    assert!(
        lines.next().is_none(),
        "unexpected registry table:\n{human}"
    );
    let starts = table_column_starts(header);
    assert_eq!(starts.len(), 4, "unexpected registry header: {human}");
    assert_eq!(
        table_cells(header, &starts),
        ["ALIAS", "LIFECYCLE", "SESSION ID", "CANONICAL SESSION",]
    );
    assert_eq!(table_column_starts(row), starts, "misaligned row:\n{human}");
    let session_id = manifest.session_id.to_string();
    let canonical_display = session
        .canonicalize()
        .expect("canonical")
        .display()
        .to_string();
    assert_eq!(
        table_cells(row, &starts),
        [
            "demo",
            "completed",
            session_id.as_str(),
            canonical_display.as_str(),
        ]
    );

    let mut remove = Command::cargo_bin("termtrace").expect("binary should build");
    let removed = remove
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "remove", "demo"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let removed = compact_json_line(&removed);
    assert_eq!(removed["evidence_preserved"], true);
    assert_eq!(
        removed["canonical_session_display"],
        session
            .canonicalize()
            .expect("canonical")
            .display()
            .to_string()
    );
    assert!(!entry_path.exists(), "registry entry should be removed");
    for (name, expected) in evidence_before {
        assert_eq!(
            fs_err::read(session.join(name)).expect("preserved evidence"),
            expected,
            "remove changed {name}"
        );
    }
}

#[test]
fn garbage_collection_is_dry_run_by_default_and_preserves_registered_or_incomplete_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let sessions = state.join("sessions");
    fs_err::create_dir_all(&sessions).expect("managed session directory");
    fs_err::set_permissions(&state, Permissions::from_mode(0o700)).expect("private state root");
    fs_err::set_permissions(&sessions, Permissions::from_mode(0o700))
        .expect("private managed session directory");
    let eligible = sessions.join("eligible.termtrace");
    let registered = sessions.join("registered.termtrace");
    for session in [&eligible, &registered] {
        let mut record = Command::cargo_bin("termtrace").expect("binary should build");
        record
            .args(["record", "--no-input", "--out"])
            .arg(session)
            .args(["--", "/bin/sh", "-c", "printf GC"])
            .assert()
            .success();
    }
    let eligible_id = read_manifest(&eligible)
        .expect("eligible manifest")
        .session_id;
    let mut register = Command::cargo_bin("termtrace").expect("binary should build");
    register
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["registry", "register", "protected", "--session"])
        .arg(&registered)
        .assert()
        .success();
    let incomplete = sessions.join("incomplete.termtrace");
    fs_err::create_dir(&incomplete).expect("incomplete managed session");
    fs_err::set_permissions(&incomplete, Permissions::from_mode(0o700))
        .expect("private incomplete session");
    let supervisor = state.join("supervisor");
    fs_err::create_dir(&supervisor).expect("supervisor directory");
    fs_err::set_permissions(&supervisor, Permissions::from_mode(0o700))
        .expect("private supervisor directory");
    for extension in ["stdout", "stderr"] {
        fs_err::write(
            supervisor.join(format!("{eligible_id}.{extension}")),
            b"diagnostic",
        )
        .expect("supervisor diagnostic");
    }

    let mut dry_run = Command::cargo_bin("termtrace").expect("binary should build");
    let dry_run = dry_run
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "registry",
            "gc",
            "--older-than-days",
            "0",
            "--keep-latest",
            "0",
        ])
        .assert()
        .success();
    let dry_run: serde_json::Value =
        serde_json::from_slice(&dry_run.get_output().stdout).expect("GC dry-run report");
    assert_eq!(dry_run["applied"], false);
    assert_eq!(dry_run["candidates"].as_array().map(Vec::len), Some(1));
    assert_eq!(dry_run["protected_registered"], 1);
    assert!(
        dry_run["skipped_non_completed"]
            .as_u64()
            .is_some_and(|count| return count >= 1)
    );
    assert!(eligible.is_dir(), "dry run removed eligible evidence");

    let mut apply = Command::cargo_bin("termtrace").expect("binary should build");
    apply
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "registry",
            "gc",
            "--older-than-days",
            "0",
            "--keep-latest",
            "0",
            "--apply",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""removed_sessions":1"#))
        .stdout(predicate::str::contains(r#""removed_supervisor_files":2"#));
    assert!(!eligible.exists());
    assert!(registered.is_dir());
    assert!(incomplete.is_dir());
    assert!(!supervisor.join(format!("{eligible_id}.stdout")).exists());
    assert!(!supervisor.join(format!("{eligible_id}.stderr")).exists());
}

#[test]
fn start_publishes_alias_only_after_detached_recorder_readiness() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "--cols",
            "90",
            "--rows",
            "25",
            "demo",
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
    assert!(
        response
            .pointer("/recorder_process_start_ticks")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    );
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
    status
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["status", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"status""#));
    let session_count = fs_err::read_dir(state.join("sessions"))
        .expect("sessions")
        .count();
    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["start", "demo", "--", "/bin/true"])
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
    failed
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "broken",
            "--",
            "/definitely/missing/termtrace-command",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "detached recorder exited before readiness",
        ))
        .stderr(predicate::str::contains("stdout log"));
    assert!(
        !state.join("registry/broken.json").exists(),
        "failed startup must not publish an alias"
    );
}

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
    let predecessor_bytes: ByteString = serde_json::from_value(
        started_json
            .pointer("/canonical_session")
            .expect("predecessor path")
            .clone(),
    )
    .expect("predecessor path bytes");
    let predecessor = PathBuf::from(OsString::from_vec(predecessor_bytes.as_bytes().to_vec()));
    let predecessor_manifest = read_manifest(&predecessor).expect("predecessor manifest");

    let mut restart = Command::cargo_bin("termtrace").expect("binary should build");
    let restarted = restart
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["restart", "demo"])
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
        .args(["registry", "resolve", "demo"])
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
        .args(["restart", "demo", "--cwd"])
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
            "test \"${#RESTART_SECRET}\" -eq 16; trap 'exit 0' TERM; while :; do sleep .1; done",
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
    let mut pending = vec![state.clone()];
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

fn table_column_starts(line: &str) -> Vec<usize> {
    if line.is_empty() {
        return Vec::new();
    }
    let mut starts = vec![0];
    let mut separator_start = None;
    for (index, byte) in line.bytes().enumerate() {
        if byte == b' ' {
            if separator_start.is_none() {
                separator_start = Some(index);
            }
        } else if let Some(separator) = separator_start.take()
            && index.saturating_sub(separator) >= 2
        {
            starts.push(index);
        }
    }
    return starts;
}

fn table_cells<'line>(line: &'line str, starts: &[usize]) -> Vec<&'line str> {
    return starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(line.len());
            return line.get(*start..end).map_or("", str::trim);
        })
        .collect();
}
