//! show workflows CLI integration tests.

use std::fs::{File, FileTimes};
use termtrace_engine::replay::semantic_state_hash;
use termtrace_model::{terminal::observation::ReplayCheckpoint, value::ByteString};
use zstd::stream::{decode_all, encode_all};

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
        .args(["wait", "demo", "--screen", "SHOWME", "--timeout", "5s"])
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
        .stdout(predicate::str::contains(r#""source_completed":true"#));
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
fn completed_cache_rejects_forged_state_and_same_length_source_tampering() {
    let temporary = TempDir::new().expect("temporary directory");
    let state_root = temporary.path().join("state");
    let session = temporary.path().join("session");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .env("TERMTRACE_STATE_DIR", &state_root)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf TRUSTED"])
        .assert()
        .success();
    let observe = || {
        return Command::cargo_bin("termtrace")
            .expect("binary")
            .env("TERMTRACE_STATE_DIR", &state_root)
            .arg("show")
            .arg(&session)
            .args(["--format", "json-compact"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
    };
    let original: serde_json::Value = serde_json::from_slice(&observe()).expect("observation");
    let cache = fs_err::read_dir(session.join("derived-cache"))
        .expect("cache")
        .next()
        .expect("cache entry")
        .expect("cache path")
        .path();
    let compressed = fs_err::read(&cache).expect("checkpoint");
    let decoded = decode_all(compressed.as_slice()).expect("decode cache");
    let mut envelope: serde_json::Value = serde_json::from_slice(&decoded).expect("cache envelope");
    let mut checkpoint: ReplayCheckpoint =
        serde_json::from_value(envelope["payload"].clone()).expect("checkpoint payload");
    checkpoint.state.metadata.title = ByteString::from(b"FORGED".as_slice());
    checkpoint.state_hash = semantic_state_hash(&checkpoint.state).expect("forged semantic hash");
    checkpoint.state_blake3 =
        blake3::hash(&serde_json::to_vec(&checkpoint.state).expect("state bytes"))
            .to_hex()
            .to_string();
    envelope["payload"] = serde_json::to_value(checkpoint).expect("forged payload");
    let forged = serde_json::to_vec(&envelope).expect("forged envelope");
    fs_err::write(
        &cache,
        encode_all(forged.as_slice(), 3).expect("compress forged cache"),
    )
    .expect("tamper cache");
    let after_forgery: serde_json::Value =
        serde_json::from_slice(&observe()).expect("replayed observation");
    assert_eq!(after_forgery["state_hash"], original["state_hash"]);

    let events = session.join("events.jsonl");
    let before = fs_err::metadata(&events).expect("source metadata");
    let canonical = fs_err::read_to_string(&events).expect("source");
    let corrupted = canonical.replace("VFJVU1RFRA==", "Rk9SR0VEIQ==");
    assert_ne!(
        corrupted, canonical,
        "fixture output payload must be present"
    );
    assert_eq!(corrupted.len(), canonical.len());
    fs_err::write(&events, corrupted).expect("tamper canonical output");
    File::options()
        .write(true)
        .open(&events)
        .expect("source handle")
        .set_times(FileTimes::new().set_modified(before.modified().expect("source mtime")))
        .expect("restore original mtime");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .env("TERMTRACE_STATE_DIR", &state_root)
        .arg("show")
        .arg(&session)
        .assert()
        .failure();
    Command::cargo_bin("termtrace")
        .expect("binary")
        .env("TERMTRACE_STATE_DIR", &state_root)
        .args(["integrity", "verify"])
        .arg(&session)
        .arg("--json")
        .assert()
        .failure()
        .stdout(predicate::str::contains("event_log_invalid"));
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
        .args(["screenshot"])
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
fn artifact_import_chains_ansi_files_and_stdin() {
    let temporary = TempDir::new().expect("temporary directory");
    let ansi = temporary.path().join("input.ansi");
    fs_err::write(&ansi, b"\x1b[31mERROR\x1b[0m\n").expect("ANSI fixture");

    let imported = temporary.path().join("imported-ansi");
    let mut import_file = Command::cargo_bin("termtrace").expect("binary should build");
    import_file
        .args(["--response", "full", "artifact", "import", "--input"])
        .arg(&ansi)
        .arg("--out")
        .arg(&imported)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"ansi_import""#));

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
    assert_eq!(canonical_output_bytes(&imported_log), ansi_bytes);

    let mut show = Command::cargo_bin("termtrace").expect("binary should build");
    show.arg("show")
        .arg(&imported)
        .args(["--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""source_completed":true"#))
        .stdout(predicate::str::contains("ERROR"));

    let mut vt_show = Command::cargo_bin("termtrace").expect("binary should build");
    vt_show
        .arg("show")
        .arg(&imported)
        .args(["--format", "vt"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ERROR"))
        .stdout(predicate::str::contains("\x1b"));
    let mut html_show = Command::cargo_bin("termtrace").expect("binary should build");
    html_show
        .arg("show")
        .arg(&imported)
        .args(["--format", "html"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ERROR"))
        .stdout(predicate::str::contains("<"));

    let stdin_imported = temporary.path().join("stdin-imported");
    let mut import_stdin = Command::cargo_bin("termtrace").expect("binary should build");
    import_stdin
        .args(["artifact", "import", "--input", "-", "--out"])
        .arg(&stdin_imported)
        .write_stdin("wide:界")
        .assert()
        .success();
    let mut stdin_show = Command::cargo_bin("termtrace").expect("binary should build");
    stdin_show
        .arg("show")
        .arg(&stdin_imported)
        .assert()
        .success()
        .stdout(predicate::str::contains("wide:界"));
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
            "5s",
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
            "trap 'exit 0' TERM; exec /usr/bin/python3 -u -c 'import sys, time; sys.stdout.write(\"READY\\n\"); sys.stdout.flush(); i = 0\nwhile True:\n    sys.stdout.write(f\"\\r{i:04d}\")\n    sys.stdout.flush()\n    time.sleep(0.005)\n    i += 1'",
        ])
        .assert()
        .success();

    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.env("TERMTRACE_STATE_DIR", &state)
        .args([
            "wait",
            "stable-contract",
            "--screen",
            "READY",
            "--stable-for",
            "200ms",
            "--timeout",
            "500ms",
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
            "--timeout",
            "500ms",
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
