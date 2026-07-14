//! End-to-end tests for the public command-line interface.

use assert_cmd::Command;
use predicates::prelude::*;
use std::{
    ffi::OsString,
    fs::{File, Permissions},
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::PermissionsExt as _,
    },
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use termtrace::{
    Error,
    control::{ControlCommand, ControlData, ControlRequest, send_request},
    event::{
        ByteString, EVENT_LOG_VERSION, Event, InputDescriptor, InputKind, InputSource,
        OperationStatus,
    },
    event_log::EventLog,
    provenance::{
        ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind, ProvenanceSensitivity,
        collect_execution_provenance,
    },
    replay::{
        GHOSTTY_REVISION, IndexOptions, ReplayManifest, StateRecord, StateSelection,
        derived_directory_name, index_session_range, index_session_with_options, replay,
    },
    request_journal::read_mutation_journal,
    session::{CaptureTarget, GeometrySource, SessionCompletion, read_manifest},
    timeline::SourceRange,
};

#[test]
fn version_subcommand_reports_package_version() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains(concat!(
            "termtrace ",
            env!("CARGO_PKG_VERSION")
        )))
        .stdout(predicate::str::contains(format!(
            "libghostty-vt {}",
            GHOSTTY_REVISION
        )))
        .stdout(predicate::str::contains(format!(
            "event-log v{EVENT_LOG_VERSION}"
        )));
}

#[test]
fn verify_reports_manifest_binding_and_detects_tampering() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("verified-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf verified"])
        .assert()
        .success();

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .arg("verify")
        .arg(&session)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid":true"#))
        .stdout(predicate::str::contains(r#""level":"content""#));

    let mut verify_durability = Command::cargo_bin("termtrace").expect("binary should build");
    verify_durability
        .arg("verify")
        .arg(&session)
        .args(["--level", "durability-record", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid":true"#))
        .stdout(predicate::str::contains(r#""level":"durability_record""#));

    let manifest_path = session.join("session.json");
    let manifest = fs_err::read_to_string(&manifest_path).expect("session manifest");
    let tampered = manifest.replace("L2Jpbi9zaA==", "L2Jpbi9ueA==");
    assert_ne!(manifest, tampered, "fixture command should be replaced");
    fs_err::write(&manifest_path, tampered).expect("tamper session manifest");

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .arg("verify")
        .arg(&session)
        .arg("--json")
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""valid":false"#))
        .stdout(predicate::str::contains(
            r#""code":"session_manifest_hash_mismatch""#,
        ));
}

#[test]
fn detached_receipt_separates_cryptography_from_signer_trust() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("signed-session");
    let key = temporary.path().join("signing-key");
    let allowed = temporary.path().join("allowed-signers");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/true"])
        .assert()
        .success();
    assert!(
        StdCommand::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .expect("launch key generator")
            .success(),
        "generate signing key"
    );
    let public = fs_err::read_to_string(key.with_extension("pub")).expect("public key");
    let mut fields = public.split_whitespace();
    let algorithm = fields.next().expect("public key algorithm");
    let encoded = fields.next().expect("public key value");
    fs_err::write(&allowed, format!("fixture-signer {algorithm} {encoded}\n"))
        .expect("allowed signers");

    let mut sign = Command::cargo_bin("termtrace").expect("binary should build");
    sign.arg("sign")
        .arg(&session)
        .arg("--key")
        .arg(&key)
        .args(["--signer", "fixture-signer"])
        .assert()
        .success();

    let mut cryptography_only = Command::cargo_bin("termtrace").expect("binary should build");
    cryptography_only
        .arg("verify")
        .arg(&session)
        .args(["--level", "signature", "--json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            r#""signature_cryptographic_valid":true"#,
        ))
        .stdout(predicate::str::contains(
            r#""signature_signer_trusted":false"#,
        ))
        .stdout(predicate::str::contains(
            r#""code":"signature_trust_policy_missing""#,
        ));

    let mut trusted = Command::cargo_bin("termtrace").expect("binary should build");
    trusted
        .arg("verify")
        .arg(&session)
        .args(["--level", "signature", "--allowed-signers"])
        .arg(&allowed)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""session_authenticity_established":true"#,
        ))
        .stdout(predicate::str::contains(
            r#""signature_signer_trusted":true"#,
        ));

    let receipt_path = session.join("evidence-receipt.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&receipt_path).expect("receipt"))
            .expect("receipt JSON");
    *receipt
        .pointer_mut("/signature/value")
        .expect("signature value") = serde_json::to_value(ByteString::from(b"invalid".as_slice()))
        .expect("invalid signature value");
    fs_err::write(
        &receipt_path,
        serde_json::to_vec_pretty(&receipt).expect("tampered receipt JSON"),
    )
    .expect("tamper receipt");
    let mut tampered = Command::cargo_bin("termtrace").expect("binary should build");
    tampered
        .arg("verify")
        .arg(&session)
        .args(["--level", "signature", "--allowed-signers"])
        .arg(&allowed)
        .arg("--json")
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""code":"signature_invalid""#));
}

#[test]
fn record_mirrors_output_persists_events_and_propagates_exit_code() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["record", "--out"])
        .arg(&session)
        .args(["--viewport", "800x600"])
        .args(["--", "/bin/sh", "-c", "printf recorded; exit 7"])
        .assert()
        .code(7)
        .stdout("recorded")
        .stderr(predicate::str::contains("recording:"));

    let event_log = session.join("events.jsonl");
    assert!(event_log.is_file(), "event log should be persisted");
    let recording = EventLog::read(event_log).expect("valid event log");
    assert!(matches!(
        recording.events.last().map(|record| return &record.event),
        Some(Event::Exit { code: Some(7), .. })
    ));
    let manifest = read_manifest(&session).expect("session manifest");
    assert_eq!(manifest.geometry_provenance.source, GeometrySource::Manual);
    assert_eq!(
        manifest.geometry_provenance.capture_target,
        Some(CaptureTarget {
            width: 800,
            height: 600,
        })
    );
}

#[test]
fn custom_provenance_policy_hashes_without_retaining_values() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("custom-provenance");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args([
            "record",
            "--no-input",
            "--provenance",
            "custom",
            "--provenance-env",
            "TERM=hash",
            "--provenance-env",
            "PRIVATE_TOKEN=not-captured",
            "--out",
        ])
        .arg(&session)
        .args(["--", "/bin/true"])
        .assert()
        .success();

    let manifest = read_manifest(&session).expect("session manifest");
    assert_eq!(
        manifest.execution_provenance.policy.kind,
        ProvenancePolicyKind::Custom
    );
    let terminal = manifest
        .execution_provenance
        .environment
        .get("TERM")
        .expect("TERM inventory");
    assert_eq!(terminal.capture, ProvenanceCapture::Hash);
    assert!(terminal.value.is_none());
    assert_eq!(
        terminal.value_hash.as_deref(),
        Some(blake3::hash(b"xterm-ghostty").to_hex().as_str())
    );
    let private = manifest
        .execution_provenance
        .environment
        .get("PRIVATE_TOKEN")
        .expect("private inventory");
    assert_eq!(private.capture, ProvenanceCapture::NotCaptured);
    assert_eq!(
        private.sensitivity,
        ProvenanceSensitivity::PotentiallySensitive
    );
    assert!(private.value.is_none() && private.value_hash.is_none());
}

#[test]
fn record_inherits_and_persists_effective_working_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let workspace = temporary.path().join("workspace");
    fs_err::create_dir_all(&workspace).expect("workspace directory");
    let inherited_session = temporary.path().join("inherited-session");
    let mut inherited = Command::cargo_bin("termtrace").expect("binary should build");

    inherited
        .current_dir(&workspace)
        .args(["record", "--no-input", "--out"])
        .arg(&inherited_session)
        .args(["--", "/bin/pwd"])
        .assert()
        .success()
        .stdout(predicate::str::contains(workspace.display().to_string()));

    let inherited_manifest = read_manifest(&inherited_session).expect("inherited manifest");
    assert_eq!(
        inherited_manifest
            .working_directory
            .expect("effective working directory")
            .as_bytes(),
        workspace.as_os_str().as_bytes()
    );

    let override_directory = temporary.path().join("override");
    fs_err::create_dir_all(&override_directory).expect("override directory");
    let override_session = temporary.path().join("override-session");
    let mut overridden = Command::cargo_bin("termtrace").expect("binary should build");
    overridden
        .current_dir(temporary.path())
        .args(["record", "--no-input", "--cwd", "override", "--out"])
        .arg(&override_session)
        .args(["--", "/bin/pwd"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            override_directory.display().to_string(),
        ));

    let override_manifest = read_manifest(&override_session).expect("override manifest");
    assert_eq!(
        override_manifest
            .working_directory
            .expect("overridden working directory")
            .as_bytes(),
        override_directory.as_os_str().as_bytes()
    );
}

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
    register
        .env("TERMTRACE_STATE_DIR", &state)
        .arg("register")
        .arg("demo")
        .arg("--session")
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(manifest.session_id.to_string()));

    let mut resolve = Command::cargo_bin("termtrace").expect("binary should build");
    resolve
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["resolve", "demo"])
        .assert()
        .success()
        .stdout(format!(
            "{}\n",
            session.canonicalize().expect("canonical").display()
        ));

    let mut structured = Command::cargo_bin("termtrace").expect("binary should build");
    structured
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["resolve", "demo", "--json"])
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
        .arg("register")
        .arg("demo")
        .arg("--session")
        .arg(&session)
        .assert()
        .failure();

    let mut active_only = Command::cargo_bin("termtrace").expect("binary should build");
    active_only
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout("[]\n");
    let mut all = Command::cargo_bin("termtrace").expect("binary should build");
    all.env("TERMTRACE_STATE_DIR", &state)
        .args(["list", "--all", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"completed""#))
        .stdout(predicate::str::contains(r#""alias":"demo""#));

    let mut remove = Command::cargo_bin("termtrace").expect("binary should build");
    remove
        .env("TERMTRACE_STATE_DIR", &state)
        .args(["remove", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""evidence_preserved":true"#));
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
fn start_publishes_alias_only_after_detached_recorder_readiness() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let mut start = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = start
        .env("TERMTRACE_STATE_DIR", &state)
        .args([
            "start",
            "demo",
            "--cols",
            "90",
            "--rows",
            "25",
            "--",
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
            "--",
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
        .args(["resolve", "demo"])
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
            "--",
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
        (700, 432)
    );
    assert_eq!(
        manifest.child_environment.get("TERM").map(String::as_str),
        Some("vt100")
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
            "--geometry fit requires --viewport WIDTHxHEIGHT",
        ));
}

#[test]
fn visual_convenience_flags_reject_invalid_values_before_recording() {
    let mut font_size = Command::cargo_bin("termtrace").expect("binary should build");
    font_size
        .args([
            "record",
            "--font-size",
            "0",
            "--out",
            "unused",
            "--",
            "/bin/true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "font size must be a positive finite number",
        ));

    let mut background = Command::cargo_bin("termtrace").expect("binary should build");
    background
        .args([
            "record",
            "--background",
            "black",
            "--out",
            "unused",
            "--",
            "/bin/true",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("color must use #RRGGBB syntax"));
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

#[test]
fn live_control_drives_and_audits_a_detached_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("controlled");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "IFS= read -r line; printf 'agent:%s' \"$line\"",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");
    let socket = session.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "control socket did not become ready");
    let registry_state = temporary.path().join("registry-state");
    let mut register = Command::cargo_bin("termtrace").expect("binary should build");
    register
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["register", "live-demo", "--session"])
        .arg(&session)
        .assert()
        .success();
    let mut named_ping = Command::cargo_bin("termtrace").expect("binary should build");
    named_ping
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["ping", "live-demo"])
        .assert()
        .success();
    let entry: serde_json::Value = serde_json::from_slice(
        &fs_err::read(registry_state.join("registry/live-demo.json")).expect("registry entry"),
    )
    .expect("registry JSON");
    assert!(
        entry
            .pointer("/recorder_pid")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    );
    assert!(
        entry
            .pointer("/recorder_process_start_ticks")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    );
    let mut named_list = Command::cargo_bin("termtrace").expect("binary should build");
    named_list
        .env("TERMTRACE_STATE_DIR", &registry_state)
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""lifecycle":"live""#))
        .stdout(predicate::str::contains(r#""alias":"live-demo""#));

    let first_boundary = match send_request(&session, &ControlRequest::new(ControlCommand::Status))
        .expect("first observer-free status")
    {
        ControlData::Status { boundaries, .. } => Some(boundaries),
        _ => None,
    }
    .expect("status response data");
    send_request(&session, &ControlRequest::new(ControlCommand::Ping)).expect("observer-free ping");
    let second_boundary =
        match send_request(&session, &ControlRequest::new(ControlCommand::Status))
            .expect("second observer-free status")
        {
            ControlData::Status { boundaries, .. } => Some(boundaries),
            _ => None,
        }
        .expect("status response data");
    assert_eq!(first_boundary, second_boundary);

    let duplicate_request = ControlRequest::new(ControlCommand::Mark {
        label: "deduplicated".to_owned(),
    });
    send_request(&session, &duplicate_request).expect("first marker request");
    send_request(&session, &duplicate_request).expect("duplicate should return cached response");
    let collision = ControlRequest {
        version: duplicate_request.version,
        request_id: duplicate_request.request_id,
        command: ControlCommand::Mark {
            label: "different".to_owned(),
        },
    };
    assert!(matches!(
        send_request(&session, &collision),
        Err(Error::ControlRejected { code, .. })
            if code == "request_id_collision"
    ));

    for arguments in [
        vec!["ping", session.to_str().expect("UTF-8 path")],
        vec!["status", session.to_str().expect("UTF-8 path")],
        vec![
            "mark",
            session.to_str().expect("UTF-8 path"),
            "--label",
            "before-input",
        ],
        vec![
            "resize",
            session.to_str().expect("UTF-8 path"),
            "--cols",
            "100",
            "--rows",
            "30",
        ],
    ] {
        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        command.args(arguments).assert().success();
    }
    let waiter = StdCommand::new(binary)
        .args([
            "wait",
            session.to_str().expect("UTF-8 path"),
            "--text",
            "agent:hello",
            "--timeout-ms",
            "5000",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn output waiter");
    thread::sleep(Duration::from_millis(50));
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.arg("send")
        .arg(&session)
        .args(["text:hello\n", "--intent", "complete shell read"])
        .assert()
        .success();

    let wait_output = waiter.wait_with_output().expect("wait for output waiter");
    assert!(wait_output.status.success(), "wait failed: {wait_output:?}");

    let output = child.wait_with_output().expect("wait for recorder");
    assert!(output.status.success(), "recorder failed: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("agent:hello"),
        "agent input did not reach child: {output:?}"
    );
    assert!(!socket.exists(), "control socket should be removed on exit");

    let recording = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    assert_control_events(&recording, &session);
    let journal_path = session.join("requests.jsonl");
    validate_json_lines("mutation-journal-record.schema.json", &journal_path);
    let mutation_records = read_mutation_journal(&journal_path).expect("mutation journal");
    assert_eq!(
        mutation_records
            .iter()
            .filter(|record| return record.request_id == duplicate_request.request_id)
            .count(),
        1,
        "retried mutation must have one persistent outcome"
    );
    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .arg("verify")
        .arg(&session)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""mutation_journal_status":"verified""#,
        ))
        .stdout(predicate::str::contains(
            r#""mutation_journal_records_verified":4"#,
        ));
}

#[test]
fn named_key_is_encoded_from_live_application_cursor_mode() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("key-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[?1hREADY'); d=os.read(0,3); os.write(1,b'KEY:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn key fixture");
    let socket = session.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "key fixture did not become ready");
    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.arg("wait")
        .arg(&session)
        .args(["--text", "READY", "--timeout-ms", "5000"])
        .assert()
        .success();
    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    send.arg("send").arg(&session).arg("up").assert().success();
    let output = child.wait_with_output().expect("key fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            input_kind: InputKind::Keys,
            descriptor: InputDescriptor::Key {
                key,
                encoder,
                terminal_modes_hash,
                ..
            },
            data,
            ..
        } if key == "up"
            && encoder == "libghostty-key-encoder-v1"
            && terminal_modes_hash.len() == 64
            && data.as_bytes() == b"\x1bOA"
    )));
}

#[test]
fn paced_unicode_text_splits_only_at_scalar_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("paced-session");
    let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os,tty; tty.setraw(0); os.write(1,b'READY'); d=b''\nwhile len(d)<6: d+=os.read(0,6-len(d))\nos.write(1,b'GOT:'+d.hex().encode())",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn paced fixture");
    let socket = session.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "paced fixture did not become ready");
    let mut wait = Command::cargo_bin("termtrace").expect("binary should build");
    wait.arg("wait")
        .arg(&session)
        .args(["--text", "READY", "--timeout-ms", "5000"])
        .assert()
        .success();

    let mut send = Command::cargo_bin("termtrace").expect("binary should build");
    let sent = send
        .arg("send")
        .arg(&session)
        .args(["--pace", "1ms", "text:é界", "enter"])
        .assert()
        .success();
    let response: serde_json::Value =
        serde_json::from_slice(&sent.get_output().stdout).expect("sequence response");
    assert_eq!(
        response
            .pointer("/atoms")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(3)
    );
    let output = child.wait_with_output().expect("paced fixture completion");
    assert!(output.status.success(), "fixture failed: {output:?}");
    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let text_chunks = log
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::InputIntent {
                input_kind: InputKind::Text,
                data,
                ..
            } => return Some(data.as_bytes()),
            _ => return None,
        })
        .collect::<Vec<_>>();
    assert_eq!(text_chunks, vec!["é".as_bytes(), "界".as_bytes()]);
    assert!(
        text_chunks
            .iter()
            .all(|chunk| return str::from_utf8(chunk).is_ok())
    );
}

#[test]
fn live_postprocessing_is_explicit_and_stop_waits_for_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("live-snapshot");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let child = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf READY; while :; do sleep 0.1; done",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn live recorder");
    wait_for_path(&session.join("control.sock"));

    let mut marker = Command::cargo_bin("termtrace").expect("binary should build");
    marker
        .arg("mark")
        .arg(&session)
        .args(["--label", "settled"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"marked""#));

    let mut incomplete_index = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_index
        .args(["index", "--session"])
        .arg(&session)
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not complete"));
    let mut incomplete_capture = Command::cargo_bin("termtrace").expect("binary should build");
    incomplete_capture
        .args(["capture", "--session"])
        .arg(&session)
        .args(["--width", "320", "--height", "200"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not complete"));

    assert!(matches!(
        index_session_with_options(&session, IndexOptions::default()),
        Err(Error::SessionIncomplete { .. })
    ));
    let snapshot = index_session_with_options(
        &session,
        IndexOptions {
            live_snapshot: true,
            state_selection: StateSelection::Presented,
            ..IndexOptions::default()
        },
    )
    .expect("explicit live snapshot");
    assert!(!snapshot.source_complete);
    assert!(
        snapshot
            .directory
            .file_name()
            .expect("snapshot basename")
            .to_string_lossy()
            .contains("snapshot-")
    );
    let snapshot_manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(snapshot.directory.join("manifest.json")).expect("snapshot manifest"),
    )
    .expect("snapshot manifest JSON");
    assert!(!snapshot_manifest.source_complete);
    assert_eq!(snapshot_manifest.state_selection, StateSelection::Presented);

    let mut stop = Command::cargo_bin("termtrace").expect("binary should build");
    stop.args(["stop", "--wait", "--timeout-ms", "5000"])
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"completed""#));
    let output = child.wait_with_output().expect("wait for live recorder");
    assert!(output.status.success(), "recorder failed: {output:?}");

    let mut status = Command::cargo_bin("termtrace").expect("binary should build");
    status
        .args(["status", "--wait-complete"])
        .arg(&session)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"completed""#));

    let mut marker_range = Command::cargo_bin("termtrace").expect("binary should build");
    marker_range
        .args(["index", "--from-marker", "settled", "--session"])
        .arg(&session)
        .assert()
        .success();
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its recording fixture is invalid"
)]
fn assert_control_events(recording: &EventLog, session: &Path) {
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            input_kind: InputKind::Text,
            intent: Some(intent),
            ..
        } if intent == "complete shell read"
    )));
    assert_eq!(
        recording
            .events
            .iter()
            .filter(|record| matches!(
                &record.event,
                Event::Marker { label, .. } if label == "deduplicated"
            ))
            .count(),
        1
    );
    assert!(recording.events.iter().any(|record| matches!(
        record.event,
        Event::ResizeIntent {
            cols: 100,
            rows: 30,
            ..
        }
    )));
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::Marker { label, .. } if label == "before-input"
    )));
    assert!(recording.pending_operations.is_empty());

    let resize_operation = recording
        .events
        .iter()
        .find_map(|record| match record.event {
            Event::ResizeIntent {
                operation_id,
                cols: 100,
                rows: 30,
                ..
            } => return Some(operation_id),
            _ => return None,
        })
        .expect("resize intent");
    let resize_result_time = recording
        .events
        .iter()
        .find_map(|record| match record.event {
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } if operation_id == resize_operation => return Some(record.monotonic_ns),
            _ => return None,
        })
        .expect("resize result");
    let ranged = index_session_range(session, Some(resize_result_time), None)
        .expect("range crossing resize intent/result");
    let ranged_states = read_zstd_text(&ranged.directory.join("states.jsonl.zst"));
    let ranged_states = ranged_states
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("state JSON"))
        .collect::<Vec<_>>();
    assert!(
        ranged_states
            .iter()
            .any(|state| return state.delta.geometry == Some((100, 30)))
    );
    assert!(ranged_states.iter().all(|state| {
        let Some(event_sequence) = state.event_seq else {
            return true;
        };
        return recording.events.iter().any(|record| {
            return record.seq == event_sequence
                && matches!(
                    record.event,
                    Event::Output { .. }
                        | Event::ResizeResult {
                            status: OperationStatus::Applied,
                            ..
                        }
                );
        });
    }));
}

#[test]
fn index_replays_every_distinct_state_deterministically() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("replay");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r"printf '\033[?2026hA\033[38;2;1;2;3mB\033[?2026l界\n\033[?1049hALT\033[?1049l'",
        ])
        .assert()
        .success();

    let event_log = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    let cast = temporary.path().join("session.cast");
    let mut export = Command::cargo_bin("termtrace").expect("binary should build");
    export
        .args(["export-asciicast", "--session"])
        .arg(&session)
        .args(["--out"])
        .arg(&cast)
        .assert()
        .success();
    let cast_text = fs_err::read_to_string(&cast).expect("asciicast");
    let mut cast_lines = cast_text.lines();
    let cast_header: serde_json::Value =
        serde_json::from_str(cast_lines.next().expect("cast header")).expect("cast header JSON");
    assert_eq!(cast_header["version"], 2);
    assert!(cast_lines.next().is_some(), "cast should contain output");
    let first = replay(&event_log).expect("first replay");
    let second = replay(&event_log).expect("second replay");
    assert_eq!(first, second, "replay must be deterministic");
    assert!(first.states.iter().any(|state| return !state.presented));
    assert!(first.states.iter().any(|state| {
        return state
            .delta
            .modes
            .as_ref()
            .is_some_and(|modes| return modes.alternate_screen);
    }));
    assert!(
        first
            .states
            .windows(2)
            .all(|pair| return pair[0].state_hash != pair[1].state_hash)
    );

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["index", "--session"])
        .arg(&session)
        .assert()
        .success();
    assert_replay_artifacts(&session, first.artifacts.state_count);

    let mut presented = Command::cargo_bin("termtrace").expect("binary should build");
    presented
        .args(["index", "--presented-only", "--session"])
        .arg(&session)
        .assert()
        .success();
    let presented_directory = session.join("derived-presented");
    let presented_manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(presented_directory.join("manifest.json")).expect("presented manifest"),
    )
    .expect("presented manifest JSON");
    assert_eq!(
        presented_manifest.state_selection,
        StateSelection::Presented
    );
    assert!(
        presented_manifest.state_count < presented_manifest.semantic_state_count,
        "fixture should suppress synchronized-output internals"
    );
    let presented_states = read_zstd_text(&presented_directory.join("states.jsonl.zst"));
    let presented_states = presented_states
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("presented state"))
        .collect::<Vec<_>>();
    assert!(presented_states.iter().all(|state| return state.presented));
    assert!(presented_states.iter().enumerate().all(|(index, state)| {
        return state.state_id == u64::try_from(index).unwrap_or(u64::MAX);
    }));
    assert_eq!(
        presented_states
            .first()
            .and_then(|state| return state.delta.geometry),
        Some((160, 40)),
        "first retained state must be independently reconstructable"
    );
    let mut inspect_text = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_text
        .args(["inspect", "--session"])
        .arg(&session)
        .args(["--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("AB界"));
    let mut inspect_initial = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_initial
        .args(["inspect", "--session"])
        .arg(&session)
        .args(["--state", "0", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""state_id": 0"#));

    let mut inspect_row = Command::cargo_bin("termtrace").expect("binary should build");
    inspect_row
        .args(["inspect", "--session"])
        .arg(&session)
        .args(["--state", "0", "--row", "0", "--format", "json-compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""row":0"#));

    validate_json_lines(
        "artifact-root-record.schema.json",
        &session.join("artifact-roots.jsonl"),
    );
    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .arg("verify")
        .arg(&session)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""artifact_roots_verified":2"#))
        .stdout(predicate::str::contains(
            r#""artifact_authenticity_established":false"#,
        ));

    let tampered_member = session.join("derived/final.txt");
    let mut contents = fs_err::read(&tampered_member).expect("derived member");
    contents.extend_from_slice(b"tampered");
    fs_err::write(&tampered_member, contents).expect("tamper derived member");
    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .arg("verify")
        .arg(&session)
        .arg("--json")
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            r#""code":"artifact_integrity_invalid""#,
        ));

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .args(["index", "--session"])
        .arg(&session)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn ranged_index_reconstructs_preroll_and_honors_half_open_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("ranged-replay");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r"printf BEFORE; sleep 0.08; printf '\033[?2026hRANGE\033[?2026l'; sleep 0.08; printf AFTER",
        ])
        .assert()
        .success();

    let event_log = EventLog::read(session.join("events.jsonl")).expect("valid event log");
    let range_start = output_timestamp_containing(&event_log, b"RANGE");
    let range_end = output_timestamp_containing(&event_log, b"AFTER");
    assert!(range_start < range_end, "fixture outputs must be separated");
    let source_range = SourceRange {
        start_monotonic_ns: range_start,
        end_monotonic_ns: range_end,
    };

    let mut empty = Command::cargo_bin("termtrace").expect("binary should build");
    empty
        .args(["index", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{range_start}ns"),
            "--to",
            &format!("{range_start}ns"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be non-empty"));

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["index", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{range_start}ns"),
            "--to",
            &format!("{range_end}ns"),
        ])
        .assert()
        .success();

    let derived = session.join(derived_directory_name(Some(source_range)));
    let manifest: ReplayManifest = serde_json::from_slice(
        &fs_err::read(derived.join("manifest.json")).expect("range manifest"),
    )
    .expect("range manifest JSON");
    assert_eq!(manifest.source_range, Some(source_range));
    validate_json_file(
        "replay-manifest.schema.json",
        &derived.join("manifest.json"),
    );
    validate_json_lines_text(
        "state-record.schema.json",
        &read_zstd_text(&derived.join("states.jsonl.zst")),
    );
    validate_json_lines(
        "state-index-entry.schema.json",
        &derived.join("state-index.jsonl"),
    );
    let states = read_zstd_text(&derived.join("states.jsonl.zst"))
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("state JSON"))
        .collect::<Vec<_>>();
    assert!(!states.is_empty(), "range must include a baseline");
    assert_eq!(states[0].state_id, 0);
    assert_eq!(states[0].event_seq, None);
    assert_eq!(states[0].monotonic_ns, range_start);
    assert!(states.iter().all(|state| {
        return state.monotonic_ns >= range_start && state.monotonic_ns < range_end;
    }));
    assert!(states.iter().any(|state| return !state.presented));
    let final_text = fs_err::read_to_string(derived.join("final.txt")).expect("range final text");
    assert!(final_text.contains("BEFORERANGE"));
    assert!(!final_text.contains("AFTER"));

    let mut full = Command::cargo_bin("termtrace").expect("binary should build");
    full.args(["index", "--session"])
        .arg(&session)
        .assert()
        .success();
    assert!(session.join("derived").is_dir());
    assert!(derived.is_dir(), "range index must coexist with full index");

    let mut unmatched_capture = Command::cargo_bin("termtrace").expect("binary should build");
    unmatched_capture
        .args(["capture", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{}ns", range_start + 1),
            "--to",
            &format!("{range_end}ns"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "matching semantic index is missing",
        ));

    let mut duplicate = Command::cargo_bin("termtrace").expect("binary should build");
    duplicate
        .args(["index", "--session"])
        .arg(&session)
        .args([
            "--from",
            &format!("{range_start}ns"),
            "--to",
            &format!("{range_end}ns"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn inspect_exits_cleanly_when_downstream_closes_the_pipe() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("broken-pipe");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/printf", "pipe-test"])
        .assert()
        .success();

    let binary = env!("CARGO_BIN_EXE_termtrace");
    let mut inspect = StdCommand::new(binary)
        .args(["inspect", "--session"])
        .arg(&session)
        .args(["--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn piped inspection");
    let stdout = inspect.stdout.take().expect("inspection stdout");
    let mut reader = BufReader::new(stdout);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).expect("first JSON line");
    assert_eq!(first_line, "{\n");
    drop(reader);
    let output = inspect.wait_with_output().expect("wait for inspection");
    assert!(
        output.status.success(),
        "broken pipe should be a successful early consumer exit: {output:?}"
    );
    assert!(output.stderr.is_empty(), "unexpected panic: {output:?}");
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its recording fixture is invalid"
)]
fn output_timestamp_containing(event_log: &EventLog, needle: &[u8]) -> u64 {
    return event_log
        .events
        .iter()
        .find_map(|record| match &record.event {
            Event::Output { data }
                if data
                    .as_bytes()
                    .windows(needle.len())
                    .any(|part| return part == needle) =>
            {
                return Some(record.monotonic_ns);
            }
            _ => return None,
        })
        .expect("fixture output event");
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        path.exists(),
        "path did not become ready: {}",
        path.display()
    );
}

#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helper failures should stop at the failed artifact assertion"
)]
fn assert_replay_artifacts(session: &Path, expected_states: u64) {
    use std::os::unix::fs::PermissionsExt as _;

    let derived = session.join("derived");
    assert_eq!(
        fs_err::metadata(&derived)
            .expect("derived metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for file in [
        "states.jsonl.zst",
        "state-index.jsonl",
        "final.txt",
        "final.vt",
        "final.html",
        "manifest.json",
    ] {
        let path = derived.join(file);
        assert!(path.is_file(), "missing {}", path.display());
        assert_eq!(
            fs_err::metadata(path)
                .expect("artifact metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let states = read_zstd_text(&derived.join("states.jsonl.zst"));
    let lines = states.lines().count();
    assert_eq!(u64::try_from(lines).unwrap(), expected_states);
    let text = fs_err::read_to_string(derived.join("final.txt")).expect("final text");
    assert!(text.contains("AB界"));
    assert!(
        !fs_err::read(derived.join("final.vt"))
            .expect("final VT")
            .is_empty()
    );
    let html = fs_err::read_to_string(derived.join("final.html")).expect("final HTML");
    assert!(html.contains('A') && html.contains('B'));

    validate_json_file("session.schema.json", &session.join("session.json"));
    validate_json_file("completion.schema.json", &session.join("completion.json"));
    validate_json_file(
        "replay-manifest.schema.json",
        &derived.join("manifest.json"),
    );
    validate_json_lines_text("state-record.schema.json", &states);
    validate_json_lines(
        "state-index-entry.schema.json",
        &derived.join("state-index.jsonl"),
    );
    let event_log = fs_err::read_to_string(session.join("events.jsonl")).expect("event log");
    let mut event_lines = event_log.lines();
    validate_json_value(
        "event-log-header.schema.json",
        &serde_json::from_str(event_lines.next().expect("event header")).expect("header JSON"),
    );
    for line in event_lines {
        validate_json_value(
            "event-record.schema.json",
            &serde_json::from_str(line).expect("event JSON"),
        );
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
fn validate_json_file(schema: &str, instance: &Path) {
    let value = serde_json::from_slice(&fs_err::read(instance).expect("instance file"))
        .expect("instance JSON");
    validate_json_value(schema, &value);
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
fn validate_json_lines(schema: &str, instance: &Path) {
    for line in fs_err::read_to_string(instance)
        .expect("JSONL instance")
        .lines()
    {
        validate_json_value(schema, &serde_json::from_str(line).expect("JSONL value"));
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid compressed artifact"
)]
fn read_zstd_text(path: &Path) -> String {
    let file = File::open(path).expect("compressed JSONL artifact");
    let mut decoder = zstd::stream::read::Decoder::new(file).expect("zstd decoder");
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .expect("decompress JSONL artifact");
    return text;
}

#[allow(
    clippy::expect_used,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
fn validate_json_lines_text(schema: &str, text: &str) {
    for line in text.lines() {
        validate_json_value(schema, &serde_json::from_str(line).expect("JSONL value"));
    }
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
fn validate_json_value(schema: &str, instance: &serde_json::Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("schemas")
        .join(schema);
    let schema_value: serde_json::Value =
        serde_json::from_slice(&fs_err::read(path).expect("schema file")).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema_value).expect("valid schema");
    if let Err(error) = validator.validate(instance) {
        panic!("{schema} rejected instance: {error}");
    }
}

#[test]
fn playback_does_not_echo_terminal_query_responses() {
    use std::collections::BTreeMap;

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use termtrace::{
        event::{ByteString, Event, EventLogHeader},
        event_log::{DurabilityPolicy, EventLogWriter, SessionClock},
        session::{
            GeometryProvenance, SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION, SessionGeometry,
            SessionManifest,
        },
    };
    use uuid::Uuid;

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("playback");
    fs_err::create_dir(&session).expect("session directory");
    fs_err::set_permissions(&session, Permissions::from_mode(0o700)).expect("private session");
    let clock = SessionClock::now().expect("session clock");
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        "0000000000000000000000000000000000000000000000000000000000000000",
        clock.started_unix_ns(),
        80,
        24,
        720,
        432,
        "xterm-ghostty",
    );
    let mut event_log =
        EventLogWriter::create(session.join("events.jsonl"), &header, clock).expect("event log");
    let query = b"\x1b]11;?\x1b\\";
    event_log
        .append(Event::Output {
            data: ByteString::from(query.as_slice()),
        })
        .expect("query output");
    event_log
        .append(Event::Exit {
            code: Some(0),
            signal: None,
            signal_name: None,
        })
        .expect("exit event");
    drop(event_log);
    let manifest = SessionManifest {
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id: header.session_id,
        creation_alias: None,
        lineage: None,
        started_unix_ns: header.started_unix_ns,
        command: Vec::new(),
        working_directory: None,
        geometry: SessionGeometry {
            cols: 80,
            rows: 24,
            pixel_width: 720,
            pixel_height: 432,
        },
        geometry_provenance: GeometryProvenance::default(),
        ghostty_visual: None,
        child_environment: BTreeMap::new(),
        execution_provenance: collect_execution_provenance(
            &ProvenancePolicy::default(),
            &BTreeMap::new(),
            &[OsString::from("/bin/true")],
            Path::new("/"),
        ),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: "linux".to_owned(),
        architecture: "test".to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: "control.sock".to_owned(),
        request_journal: "requests.jsonl".to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: DurabilityPolicy::default(),
    };
    fs_err::write(
        session.join("session.json"),
        serde_json::to_vec(&manifest).expect("session manifest JSON"),
    )
    .expect("session manifest");

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 720,
            pixel_height: 432,
        })
        .expect("playback PTY");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termtrace"));
    let timing_file = temporary.path().join("timing.json");
    let error_file = temporary.path().join("playback-error.json");
    let ready_file = temporary.path().join("ready");
    command.args([
        "playback",
        "--session",
        session.to_str().expect("UTF-8 session"),
        "--timing-file",
        timing_file.to_str().expect("UTF-8 timing"),
        "--error-file",
        error_file.to_str().expect("UTF-8 playback error"),
        "--ready-file",
        ready_file.to_str().expect("UTF-8 ready"),
        "--start-delay-ms",
        "200",
        "--final-hold-ms",
        "1000",
    ]);
    let mut child = pty.slave.spawn_command(command).expect("spawn playback");
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        return output;
    });
    let mut writer = pty.master.take_writer().expect("take PTY writer");
    writer
        .write_all(b"\x1b[4;432;720t\x1b[6;18;9t\x1b[8;24;80t")
        .expect("write geometry responses");
    thread::sleep(Duration::from_millis(400));
    writer
        .write_all(b"HOST-RESPONSE\n")
        .expect("write simulated host response");
    child.wait().expect("wait for playback");
    drop(writer);
    drop(pty.master);
    let output = reader_thread.join().expect("join PTY reader");

    assert!(
        output
            .windows(query.len())
            .any(|window| return window == query)
    );
    assert!(
        !output
            .windows(b"HOST-RESPONSE".len())
            .any(|window| return window == b"HOST-RESPONSE"),
        "playback echoed a capture-host response: {output:?}"
    );
}
