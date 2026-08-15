//! show workflows CLI integration tests.

use crate::support::*;

#[test]
fn show_uses_actor_fast_path_and_validated_completed_replay() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "demo",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf SHOWME; while :; do sleep .1; done",
        ])
        .assert()
        .success();
    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.env("TERMTRACE_STATE_DIR", &state)
        .args(["wait", "demo", "SHOWME", "--timeout-ms", "5000"])
        .assert()
        .success();
    let mut live_show = Command::cargo_bin("termtrace").expect("binary should build");
    live_show
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("SHOWME"));
    let mut live_json = Command::cargo_bin("termtrace").expect("binary should build");
    let live = live_json
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "demo", "--format", "json-compact"])
        .assert()
        .success();
    let live: serde_json::Value =
        serde_json::from_slice(&live.get_output().stdout).expect("live observation");
    assert_eq!(
        live.pointer("/source_kind")
            .and_then(serde_json::Value::as_str),
        Some("live_session")
    );
    assert_eq!(live["resolution"], "actor_snapshot");
    for format in ["svg", "png"] {
        let mut live_image = Command::cargo_bin("termtrace").expect("binary should build");
        let image = live_image
            .env("TERMTRACE_STATE_DIR", &state)
            .args(["show", "demo", "--format", format])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_semantic_image(format, &image);
    }
    let mut live_hidden_image = Command::cargo_bin("termtrace").expect("binary should build");
    let image = live_hidden_image
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "demo", "--format", "png", "--include-hidden"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_semantic_image("png", &image);
    let mut stable_frame = Command::cargo_bin("termtrace").expect("binary should build");
    let stable = stable_frame
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "show",
            "demo",
            "--stable-for",
            "20ms",
            "--format",
            "frame-json",
            "--metadata",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("source=live_session"))
        .stderr(predicate::str::contains("resolution=actor_snapshot"))
        .stderr(predicate::str::contains("quality=stable"))
        .stderr(predicate::str::contains("truth=semantic"))
        .stderr(predicate::str::contains("Some(").not())
        .stderr(predicate::str::contains("None").not());
    let frame: serde_json::Value =
        serde_json::from_slice(&stable.get_output().stdout).expect("stable frame JSON");
    assert_eq!(frame["version"], 1);
    assert_eq!(frame["capture_quality"], "stable");
    assert!(
        frame["frame"]["viewport_text"]
            .as_str()
            .is_some_and(|text| return text.contains("SHOWME"))
    );
    assert_eq!(frame["frame_hash"].as_str().map(str::len), Some(64));
    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "demo", "--wait"])
        .assert()
        .success();
    let mut completed_json = Command::cargo_bin("termtrace").expect("binary should build");
    completed_json
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "demo", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""source_kind":"completed_session""#,
        ))
        .stdout(predicate::str::contains(r#""capture_quality":"completed""#))
        .stdout(predicate::str::contains(r#""source_completed":true"#))
        .stdout(predicate::str::contains(
            r#""resolution":"canonical_replay""#,
        ));
    for format in ["svg", "png"] {
        let manifest_path = temporary
            .path()
            .join(format!("completed-{format}-manifest.json"));
        let mut completed_image = Command::cargo_bin("termtrace").expect("binary should build");
        let image = completed_image
            .env("TERMTRACE_STATE_DIR", &state)
            .args(["show", "demo", "--format", format, "--manifest"])
            .arg(&manifest_path)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_semantic_image(format, &image);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs_err::read(&manifest_path).expect("completed image manifest"),
        )
        .expect("completed image manifest JSON");
        assert_eq!(
            manifest["observation"]["resolution"],
            "validated_checkpoint"
        );
        assert_eq!(
            manifest["blake3"],
            blake3::hash(&image).to_hex().to_string()
        );
    }
    let mut warm = Command::cargo_bin("termtrace").expect("binary should build");
    warm.env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "demo", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""resolution":"validated_checkpoint""#,
        ));
    let session_path = fs_err::read_dir(state.join("sessions"))
        .expect("session store")
        .next()
        .expect("stored session")
        .expect("stored session entry")
        .path();
    let cache = fs_err::read_dir(session_path.join("derived-cache"))
        .expect("checkpoint directory")
        .next()
        .expect("checkpoint entry")
        .expect("checkpoint path")
        .path();
    assert_eq!(
        cache.extension().and_then(|value| return value.to_str()),
        Some("zst")
    );
    assert!(
        fs_err::read(&cache)
            .expect("compressed checkpoint")
            .starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
    );
    let mut cache_status = Command::cargo_bin("termtrace").expect("binary should build");
    let status = cache_status
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["analyze", "cache", "status", "demo"])
        .assert()
        .success();
    let status: serde_json::Value =
        serde_json::from_slice(&status.get_output().stdout).expect("cache status");
    assert_eq!(status["entries"], 1);
    assert!(
        status["compressed_bytes"]
            .as_u64()
            .is_some_and(|bytes| return bytes > 0)
    );
    assert_eq!(status["maximum_entries"], 32);
    fs_err::write(&cache, b"{}\n").expect("tamper disposable checkpoint");
    let mut fallback = Command::cargo_bin("termtrace").expect("binary should build");
    fallback
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["show", "demo", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""resolution":"canonical_replay""#,
        ));
    let mut prune = Command::cargo_bin("termtrace").expect("binary should build");
    prune
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["analyze", "cache", "prune", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""removed_entries":1"#))
        .stdout(predicate::str::contains(r#""entries":0"#));
}

#[test]
fn show_requires_explicit_permission_for_hidden_states() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("hidden-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", r"printf '\033[?2026hHIDDEN'"])
        .assert()
        .success();
    let hidden = inspect_session(&session, StateSelector::Final).expect("hidden final state");
    assert!(!hidden.record.presented);
    let mut default_show = Command::cargo_bin("termtrace").expect("binary should build");
    default_show
        .arg("show")
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains("HIDDEN").not());
    let mut rejected = Command::cargo_bin("termtrace").expect("binary should build");
    rejected
        .arg("show")
        .arg(&session)
        .args(["--state", &hidden.record.state_id.to_string()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("hidden"));
    let mut included = Command::cargo_bin("termtrace").expect("binary should build");
    included
        .arg("show")
        .arg(&session)
        .args([
            "--state",
            &hidden.record.state_id.to_string(),
            "--include-hidden",
            "--format",
            "json-compact",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""presented":false"#))
        .stdout(predicate::str::contains("HIDDEN"));
    let mut native = Command::cargo_bin("termtrace").expect("binary should build");
    native
        .args(["artifact", "screenshot"])
        .arg(&session)
        .args([
            "--renderer",
            "native",
            "--state",
            &hidden.record.state_id.to_string(),
            "--out",
        ])
        .arg(temporary.path().join("rejected-native"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("presentation eligible"));
}

#[test]
fn show_requires_explicit_prefix_recovery_for_incomplete_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("interrupted-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf PREFIX_ONLY"])
        .assert()
        .success();
    fs_err::remove_file(session.join("completion.json")).expect("remove completion fixture");

    let mut implicit = Command::cargo_bin("termtrace").expect("binary should build");
    implicit.arg("show").arg(&session).assert().failure();

    let mut recovered = Command::cargo_bin("termtrace").expect("binary should build");
    recovered
        .arg("show")
        .arg(&session)
        .args(["--recover-prefix", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""source_kind":"interrupted_session""#,
        ))
        .stdout(predicate::str::contains(
            r#""capture_quality":"incomplete""#,
        ))
        .stdout(predicate::str::contains("PREFIX_ONLY"));
}

#[test]
fn show_labels_ansi_files_and_stdin_as_unchained() {
    let temporary = TempDir::new().expect("temporary directory");
    let ansi = temporary.path().join("input.ansi");
    fs_err::write(&ansi, b"\x1b[31mERROR\x1b[0m\n").expect("ANSI fixture");
    let mut file_show = Command::cargo_bin("termtrace").expect("binary should build");
    file_show
        .args(["show", "--input"])
        .arg(&ansi)
        .args(["--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""source_kind":"ansi_stream""#))
        .stdout(predicate::str::contains(r#""integrity":"unchained""#))
        .stdout(predicate::str::contains(r#""session_id":null"#))
        .stdout(predicate::str::contains("ERROR"));

    let mut stdin_show = Command::cargo_bin("termtrace").expect("binary should build");
    stdin_show
        .args(["show", "--input", "-"])
        .write_stdin("wide:界")
        .assert()
        .success()
        .stdout(predicate::str::contains("wide:界"));

    let imported = temporary.path().join("imported-ansi");
    let mut import_show = Command::cargo_bin("termtrace").expect("binary should build");
    import_show
        .args(["show", "--input"])
        .arg(&ansi)
        .args(["--record-out"])
        .arg(&imported)
        .args(["--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""source_kind":"imported_ansi""#))
        .stdout(predicate::str::contains(
            r#""integrity":"canonical_chained""#,
        ));
    let imported_manifest = read_manifest(&imported).expect("import manifest");
    let provenance = imported_manifest
        .import_provenance
        .expect("chain-bound import provenance");
    let ansi_bytes = fs_err::read(&ansi).expect("ANSI source bytes");
    assert_eq!(provenance.source_kind, "ansi_stream");
    assert_eq!(provenance.byte_count, ansi_bytes.len() as u64);
    assert_eq!(
        provenance.blake3,
        blake3::hash(&ansi_bytes).to_hex().to_string()
    );
    let imported_log = EventLog::read(imported.join("events.jsonl")).expect("import event log");
    assert_eq!(
        imported_log
            .events
            .iter()
            .filter_map(|record| {
                return match &record.event {
                    Event::Output { data } => Some(data.as_bytes()),
                    _ => None,
                };
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>(),
        ansi_bytes
    );
    for format in ["svg", "png"] {
        let mut imported_image = Command::cargo_bin("termtrace").expect("binary should build");
        let image = imported_image
            .arg("show")
            .arg(&imported)
            .args(["--format", format])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_semantic_image(format, &image);
    }

    let mut vt_show = Command::cargo_bin("termtrace").expect("binary should build");
    vt_show
        .args(["show", "--input"])
        .arg(&ansi)
        .args(["--format", "vt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ERROR"))
        .stdout(predicate::str::contains("\x1b"));
    let mut html_show = Command::cargo_bin("termtrace").expect("binary should build");
    html_show
        .args(["show", "--input"])
        .arg(&ansi)
        .args(["--format", "html"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ERROR"))
        .stdout(predicate::str::contains("<"));

    let image_manifest = temporary.path().join("show-image.json");
    let mut png_show = Command::cargo_bin("termtrace").expect("binary should build");
    let png = png_show
        .args(["show", "--input"])
        .arg(&ansi)
        .args(["--format", "png", "--manifest"])
        .arg(&image_manifest)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&image_manifest).expect("show image manifest"))
            .expect("show image manifest JSON");
    assert_eq!(manifest["truth_layer"], "semantic_rendering");
    assert_eq!(manifest["blake3"], blake3::hash(&png).to_hex().to_string());

    let mut native = Command::cargo_bin("termtrace").expect("binary should build");
    native
        .args(["show", "--input"])
        .arg(&ansi)
        .args(["--format", "png", "--renderer", "native"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("fallback is disabled"));
}

#[test]
fn show_records_one_off_commands_as_completed_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    fs_err::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("owner-only state directory");
    let mut show = Command::cargo_bin("termtrace").expect("binary should build");
    let output = show
        .env("TERMTRACE_STATE_DIR", temporary.path())
        .args([
            "show",
            "--cols",
            "30",
            "--rows",
            "4",
            "--format",
            "json-compact",
            "--",
            "/bin/sh",
            "-c",
            "printf '\\033[32mONE-OFF\\033[0m'",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let observation: serde_json::Value =
        serde_json::from_slice(&output).expect("one-off observation JSON");
    assert_eq!(observation["source_kind"], "one_off_command");
    assert_eq!(observation["source_completed"], true);
    assert_eq!(observation["capture_quality"], "completed");
    assert_eq!(observation["integrity"], "canonical_chained");
    assert!(observation["session_id"].is_string());
    assert!(
        observation
            .pointer("/frame/viewport_text")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| return text.contains("ONE-OFF"))
    );
    let sessions = fs_err::read_dir(temporary.path().join("sessions"))
        .expect("one-off sessions directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("one-off session entries");
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].path().join("completion.json").is_file());

    for format in ["svg", "png"] {
        let evidence = temporary.path().join(format!("one-off-{format}"));
        let mut image_show = Command::cargo_bin("termtrace").expect("binary should build");
        let image = image_show
            .env("TERMTRACE_STATE_DIR", temporary.path())
            .args([
                "show", "--cols", "30", "--rows", "4", "--format", format, "--out",
            ])
            .arg(&evidence)
            .args(["--", "/bin/sh", "-c", "printf ONE-OFF-IMAGE"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_semantic_image(format, &image);
        assert!(evidence.join("completion.json").is_file());
    }

    let explicit = temporary.path().join("explicit-one-off");
    let mut explicit_show = Command::cargo_bin("termtrace").expect("binary should build");
    explicit_show
        .env("TERMTRACE_STATE_DIR", temporary.path())
        .args(["show", "--out"])
        .arg(&explicit)
        .args(["--", "/bin/sh", "-c", "printf EXPLICIT"])
        .assert()
        .success()
        .stdout(predicate::str::contains("EXPLICIT"));
    assert!(explicit.join("completion.json").is_file());

    let sessions_before_discard = fs_err::read_dir(temporary.path().join("sessions"))
        .expect("sessions before discard")
        .count();
    let mut discard = Command::cargo_bin("termtrace").expect("binary should build");
    discard
        .env("TERMTRACE_STATE_DIR", temporary.path())
        .args([
            "show",
            "--discard-evidence",
            "--",
            "/bin/sh",
            "-c",
            "printf DISPOSABLE",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("DISPOSABLE"))
        .stderr(predicate::str::contains(
            "no longer independently replayable",
        ));
    assert_eq!(
        fs_err::read_dir(temporary.path().join("sessions"))
            .expect("sessions after discard")
            .count(),
        sessions_before_discard
    );
}

#[test]
fn show_one_off_waits_stop_durably_and_detach_is_explicit() {
    let temporary = TempDir::new().expect("temporary directory");
    fs_err::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("owner-only state directory");

    let mut waited = Command::cargo_bin("termtrace").expect("binary should build");
    let output = waited
        .env("TERMTRACE_STATE_DIR", temporary.path())
        .args([
            "show",
            "--format",
            "json-compact",
            "--wait-regex",
            "READY-[0-9]+",
            "--stable-for",
            "20ms",
            "--timeout-ms",
            "5000",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY-42; while :; do sleep .1; done",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let observation: serde_json::Value =
        serde_json::from_slice(&output).expect("waited one-off observation");
    assert_eq!(observation["source_kind"], "one_off_command");
    assert_eq!(observation["source_completed"], true);
    assert!(
        observation
            .pointer("/frame/viewport_text")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| return text.contains("READY-42"))
    );

    let mut detached = Command::cargo_bin("termtrace").expect("binary should build");
    let detached_output = detached
        .env("TERMTRACE_STATE_DIR", temporary.path())
        .args([
            "show",
            "--detach",
            "--timeout-ms",
            "5000",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf DETACHED; while :; do sleep .1; done",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let detached: serde_json::Value =
        serde_json::from_slice(&detached_output).expect("detached one-off result");
    assert_eq!(detached["detached"], true);
    assert_eq!(detached["source_completed"], false);
    let session_bytes: ByteString = serde_json::from_value(detached["canonical_session"].clone())
        .expect("canonical detached session bytes");
    let session = PathBuf::from(OsString::from_vec(session_bytes.into_bytes()));
    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", temporary.path())
        .arg("stop")
        .arg(&session)
        .arg("--wait")
        .assert()
        .success();
    assert!(session.join("completion.json").is_file());
}

#[test]
fn synchronization_commands_reject_incomplete_stability() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");

    let mut idle_start = Command::cargo_bin("termtrace").expect("binary should build");
    let idle_output = idle_start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "idle",
            "--until-idle",
            "10ms",
            "--timeout",
            "2s",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; sleep 30",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let idle: serde_json::Value =
        serde_json::from_slice(&idle_output).expect("idle start response");
    assert_eq!(idle["postcondition"]["matched"], true);
    let mut stop_idle = Command::cargo_bin("termtrace").expect("binary should build");
    stop_idle
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "idle", "--wait"])
        .assert()
        .success();

    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "stable-contract",
            "--until-screen",
            "READY",
            "--timeout",
            "5s",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf 'READY\\n'; i=0; while :; do printf '\\r%04d' \"$i\"; i=$((i + 1)); sleep .01; done",
        ])
        .assert()
        .success();

    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "wait",
            "stable-contract",
            "READY",
            "--stable-for",
            "200ms",
            "--timeout-ms",
            "500",
        ])
        .assert()
        .failure();

    let mut show = Command::cargo_bin("termtrace").expect("binary should build");
    show.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "show",
            "stable-contract",
            "--stable-for",
            "200ms",
            "--timeout-ms",
            "500",
            "--format",
            "json-compact",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "live stability was not established before Deadline",
        ));

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.env("TERMTRACE_STATE_DIR", &state)
        .args(["stop", "stable-contract", "--wait"])
        .assert()
        .success();
}

#[test]
fn show_one_off_rejects_an_unmatched_wait() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut show = Command::cargo_bin("termtrace").expect("binary should build");
    fs_err::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("owner-only state directory");
    show.env("TERMTRACE_STATE_DIR", temporary.path())
        .args([
            "show",
            "--wait",
            "NEVER",
            "--timeout-ms",
            "500",
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; sleep 30",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "one-off wait did not meet the requested predicate stability before Deadline",
        ));
}

fn assert_semantic_image(format: &str, bytes: &[u8]) {
    if format == "svg" {
        assert!(bytes.starts_with(b"<svg "), "missing SVG signature");
        assert!(
            bytes
                .windows(b"data-truth-layer=\"semantic_rendering\"".len())
                .any(|window| {
                    return window == b"data-truth-layer=\"semantic_rendering\"";
                }),
            "missing semantic truth label"
        );
        return;
    }
    assert_eq!(format, "png", "unsupported image assertion format");
    assert!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "missing PNG signature"
    );
}
