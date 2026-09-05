use super::*;
use termtrace_model::{
    protocol::control::CONTROL_PROTOCOL_VERSION,
    registry::{REGISTRY_ENTRY_FORMAT, REGISTRY_ENTRY_VERSION, RegistryEntry},
    value::ByteString,
};
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
        .args(["session", "register", "protected", "--session"])
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
            "session",
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
    assert_eq!(dry_run["changes_applied"], false);
    assert!(dry_run.get("applied").is_none());
    let envelope: CliResponseEnvelope =
        serde_json::from_value(dry_run.clone()).expect("GC response envelope");
    assert_eq!(
        envelope.fields.get("changes_applied"),
        Some(&serde_json::Value::Bool(false))
    );
    let round_trip = serde_json::to_value(envelope).expect("round-trip GC response envelope");
    let schema: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../schemas/cli-response.schema.json"
    ))
    .expect("CLI response schema");
    let validator = jsonschema::validator_for(&schema).expect("CLI response schema validator");
    assert!(
        validator.is_valid(&round_trip),
        "round-tripped GC response must satisfy the CLI response schema"
    );
    assert_eq!(round_trip["changes_applied"], false);
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
            "session",
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
fn corrupt_registry_entries_do_not_poison_listing_or_unsafe_garbage_collection() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let sessions = state.join("sessions");
    fs_err::create_dir_all(&sessions).expect("managed session directory");
    fs_err::set_permissions(&state, Permissions::from_mode(0o700)).expect("private state root");
    fs_err::set_permissions(&sessions, Permissions::from_mode(0o700))
        .expect("private managed session directory");
    let eligible = sessions.join("eligible.termtrace");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&eligible)
        .args(["--", "/bin/sh", "-c", "printf GC"])
        .assert()
        .success();

    let mut initialize = Command::cargo_bin("termtrace").expect("binary should build");
    initialize
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "list", "--all", "--json"])
        .assert()
        .success();
    let corrupt = state.join("registry").join("poison.json");
    fs_err::write(&corrupt, b"{not-json").expect("corrupt registry record");
    fs_err::set_permissions(&corrupt, Permissions::from_mode(0o600))
        .expect("private corrupt record");

    let mut list = Command::cargo_bin("termtrace").expect("binary should build");
    list.env("TERMTRACE_STATE_DIR", &state)
        .args(["session", "list", "--all", "--json"])
        .assert()
        .success()
        .stdout("[]\n");

    let mut apply = Command::cargo_bin("termtrace").expect("binary should build");
    let output = apply
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "session",
            "gc",
            "--older-than-days",
            "0",
            "--keep-latest",
            "0",
            "--apply",
        ])
        .assert()
        .success();
    let report: serde_json::Value =
        serde_json::from_slice(&output.get_output().stdout).expect("GC isolation report");
    assert_eq!(report["skipped_corrupt_registry_entries"], 1);
    assert_eq!(report["removed_sessions"], 0);
    assert!(
        eligible.is_dir(),
        "GC removed ambiguously protected evidence"
    );
}

#[test]
fn stale_aliases_with_exited_recorder_are_reaped_by_garbage_collection() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let sessions = state.join("sessions");
    let registry = state.join("registry");
    fs_err::create_dir_all(&sessions).expect("managed session directory");
    fs_err::create_dir_all(&registry).expect("managed registry directory");
    fs_err::set_permissions(&state, Permissions::from_mode(0o700)).expect("private state root");
    fs_err::set_permissions(&sessions, Permissions::from_mode(0o700))
        .expect("private managed session directory");
    fs_err::set_permissions(&registry, Permissions::from_mode(0o700))
        .expect("private registry directory");

    let session = sessions.join("stale.termtrace");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf STALE"])
        .assert()
        .success();

    let completion_file = session.join("completion.json");
    if completion_file.exists() {
        fs_err::remove_file(&completion_file).expect("remove completion");
    }

    let manifest = read_manifest(&session).expect("manifest");
    let alias_file = registry.join("stale-alias.json");
    let entry = RegistryEntry {
        format: REGISTRY_ENTRY_FORMAT.to_owned(),
        version: REGISTRY_ENTRY_VERSION,
        alias: "stale-alias".to_owned(),
        canonical_session: ByteString::from(
            session
                .canonicalize()
                .unwrap_or_else(|_| return session.clone())
                .as_os_str()
                .as_bytes(),
        ),
        session_id: manifest.session_id,
        recorder_pid: Some(9_999_999),
        recorder_process_start_ticks: Some(12345),
        control_protocol_version: CONTROL_PROTOCOL_VERSION,
        registered_unix_ns: 1_700_000_000_000_000_000_u128,
    };
    fs_err::write(
        &alias_file,
        serde_json::to_vec_pretty(&entry).expect("entry JSON"),
    )
    .expect("write entry");
    fs_err::set_permissions(&alias_file, Permissions::from_mode(0o600))
        .expect("private alias record");
    let mut apply = Command::cargo_bin("termtrace").expect("binary should build");
    let output = apply
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "session",
            "gc",
            "--older-than-days",
            "0",
            "--keep-latest",
            "0",
            "--apply",
        ])
        .assert()
        .success();
    let report: serde_json::Value =
        serde_json::from_slice(&output.get_output().stdout).expect("GC report");
    assert_eq!(report["removed_stale_aliases"], 1);
    assert!(!alias_file.exists(), "stale alias file was not removed");
}
