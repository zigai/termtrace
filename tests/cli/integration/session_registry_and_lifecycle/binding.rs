use super::*;

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
        .args(["--response", "full", "session", "register"])
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
        .args(["session", "resolve", "demo"])
        .assert()
        .success()
        .stdout(format!(
            "{}\n",
            session.canonicalize().expect("canonical").display()
        ));

    let mut structured = Command::cargo_bin("termtrace").expect("binary should build");
    structured
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "resolve", "demo", "--json"])
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
        .args(["session", "register"])
        .arg("demo")
        .arg("--session")
        .arg(&session)
        .assert()
        .failure();

    let mut active_only = Command::cargo_bin("termtrace").expect("binary should build");
    active_only
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "list", "--json"])
        .assert()
        .success()
        .stdout("[]\n");

    let mut all = Command::cargo_bin("termtrace").expect("binary should build");
    let all = all
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "list", "--all", "--json"])
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
        .args(["session", "list", "--all"])
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
        .args(["--response", "full", "session", "remove", "demo"])
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
fn status_and_control_preserve_incomplete_and_stale_registry_lifecycle() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let session = temporary.path().join("interrupted");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/true"])
        .assert()
        .success();

    fs_err::remove_file(session.join("completion.json")).expect("remove completion fixture");

    let mut register = Command::cargo_bin("termtrace").expect("binary should build");
    register
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "register", "interrupted", "--session"])
        .arg(&session)
        .assert()
        .success();

    let mut incomplete_status = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_status
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["status", "interrupted"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"incomplete""#))
        .stdout(predicate::str::contains("control_unavailable").not());

    let mut incomplete_control = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_control
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["control", "mark", "interrupted", "--label", "probe"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""lifecycle":"incomplete""#))
        .stderr(predicate::str::contains("control_unavailable").not());

    let mut incomplete_screen = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_screen
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "screen", "interrupted"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""operation":"screen""#))
        .stdout(predicate::str::contains(r#""lifecycle":"incomplete""#))
        .stderr(predicate::str::contains("control_unavailable").not());

    let mut incomplete_video = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_video
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "video",
            "interrupted",
            "--renderer",
            "semantic",
            "-o",
        ])
        .arg(temporary.path().join("incomplete.mp4"))
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""operation":"video""#))
        .stdout(predicate::str::contains(r#""lifecycle":"incomplete""#));

    let entry_path = state.join("registry/interrupted.json");
    let mut entry: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&entry_path).expect("registry entry"))
            .expect("registry entry JSON");
    entry["recorder_pid"] = serde_json::json!(u32::MAX);
    entry["recorder_process_start_ticks"] = serde_json::json!(1);
    fs_err::write(
        &entry_path,
        serde_json::to_vec_pretty(&entry).expect("updated registry entry"),
    )
    .expect("write stale registry fixture");

    let mut stale_status = Command::cargo_bin("termtrace").expect("binary should build");
    stale_status
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["status", "interrupted"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"stale""#))
        .stderr(predicate::str::contains("control_unavailable").not());

    let mut stale_screen = Command::cargo_bin("termtrace").expect("binary should build");
    stale_screen
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["--response", "full", "screen", "interrupted"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""operation":"screen""#))
        .stdout(predicate::str::contains(r#""lifecycle":"stale""#))
        .stderr(predicate::str::contains("control_unavailable").not());

    let mut stale_video = Command::cargo_bin("termtrace").expect("binary should build");
    stale_video
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "--response",
            "full",
            "video",
            "interrupted",
            "--renderer",
            "semantic",
            "-o",
        ])
        .arg(temporary.path().join("stale.mp4"))
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""operation":"video""#))
        .stdout(predicate::str::contains(r#""lifecycle":"stale""#));
}
