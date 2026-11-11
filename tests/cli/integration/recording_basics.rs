//! recording basics CLI integration tests.

use crate::support::*;

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
        Some(CaptureTarget::new(800, 600))
    );
}

#[test]
fn durability_thresholds_are_only_accepted_in_bounded_mode() {
    let temporary = TempDir::new().expect("temporary directory");

    for (mode, threshold, value) in [
        ("strict", "--durability-max-interval-ms", "1"),
        ("throughput", "--durability-max-uncommitted-bytes", "1024"),
    ] {
        let session = temporary.path().join(format!("{mode}-rejected"));
        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        command
            .args(["record", "--no-input", "--out"])
            .arg(&session)
            .args(["--durability", mode, threshold, value, "--", "/bin/true"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("require --durability bounded"));
        assert!(
            !session.exists(),
            "invalid durability options created evidence"
        );
    }

    let session = temporary.path().join("bounded-defaults");
    let mut bounded = Command::cargo_bin("termtrace").expect("binary should build");
    bounded
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--durability", "bounded", "--", "/bin/true"])
        .assert()
        .success();

    let manifest = read_manifest(&session).expect("bounded manifest");
    assert_eq!(manifest.durability_policy.maximum_interval_ms, Some(50));
    assert_eq!(
        manifest.durability_policy.maximum_uncommitted_bytes,
        Some(65_536)
    );
}

#[test]
fn output_budget_exhaustion_returns_failure_after_fast_child_exit() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("budget");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args([
            "record",
            "--no-input",
            "--output-byte-budget",
            "4095",
            "--out",
        ])
        .arg(&session)
        .args([
            "--",
            "/usr/bin/python3",
            "-c",
            "import os; os.write(1, b'x' * 4096)",
        ])
        .assert()
        .failure();

    let completion: serde_json::Value =
        serde_json::from_slice(&fs_err::read(session.join("completion.json")).expect("completion"))
            .expect("completion JSON");

    assert_eq!(completion["resource_limit"]["kind"], "output_bytes");
    assert_eq!(completion["resource_limit"]["limit"], 4095);
    assert!(completion["resource_limit"]["observed"]
        .as_u64()
        .is_some_and(|observed| return observed >= 4096));
}

#[test]
fn semantic_record_without_ghostty_has_stable_nonwarning_stderr() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("semantic-only-session");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let recorded = command
        .env("PATH", "")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf SEMANTIC_ONLY"])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&recorded.get_output().stderr);
    assert!(
        stderr.contains("recording:"),
        "missing success diagnostic: {stderr}"
    );
    assert!(!stderr.contains("WARN"), "unexpected warning: {stderr}");
    assert!(
        !stderr.contains("Ghostty is unavailable"),
        "unexpected optional-host warning: {stderr}"
    );
    assert!(
        !stderr.lines().any(has_rfc3339_log_prefix),
        "unexpected timestamped log line: {stderr}"
    );

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");

    assert_eq!(log.header.colors.source, "libghostty_builtin");
}

#[test]
fn semantic_visual_profile_needs_no_native_tools_and_keeps_semantic_pixels() {
    let temporary = TempDir::new().expect("temporary directory");
    let script =
        "printf 'BUNDLED \\033[1mbold\\033[0m \\033[3mitalic\\033[0m \\346\\274\\242e\\314\\201'";

    let record_and_shoot = |name: &str, profile: Option<&str>| {
        let session = temporary.path().join(name);
        let mut record = Command::cargo_bin("termtrace").expect("binary should build");
        record
            .env("PATH", "")
            .args(["record", "--no-input", "--cols", "40", "--rows", "4"]);

        if let Some(profile) = profile {
            record.args(["--visual-profile", profile]);
        }

        record
            .arg("--out")
            .arg(&session)
            .args(["--", "/bin/sh", "-c", script])
            .assert()
            .success();

        let shot = temporary.path().join(format!("{name}-shot"));
        Command::cargo_bin("termtrace")
            .expect("binary should build")
            .env("PATH", "")
            .arg("screenshot")
            .arg(&session)
            .arg("--out")
            .arg(&shot)
            .assert()
            .success();

        return (
            read_manifest(&session).expect("session manifest"),
            fs::read(shot.join("screen.png")).expect("semantic screenshot"),
        );
    };
    let (legacy, legacy_png) = record_and_shoot("legacy", None);
    let (profiled, profiled_png) = record_and_shoot(
        "profiled",
        Some(r#"{"version":1,"semantic":{"preset":"bundled-v1"}}"#),
    );

    assert_eq!(legacy.version, 12);
    assert!(legacy.visual_profile.is_none());
    assert_eq!(profiled.version, 13);
    assert_eq!(
        profiled.visual_profile,
        Some(
            VisualProfile::semantic_bundled()
                .record()
                .expect("valid profile")
        )
    );
    assert!(profiled.ghostty_visual.is_none());
    assert_eq!(legacy_png, profiled_png);
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

fn has_rfc3339_log_prefix(line: &str) -> bool {
    let bytes = line.as_bytes();
    let Some(prefix) = bytes.get(..19) else {
        return false;
    };

    return prefix.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => return *byte == b'-',
        10 => return *byte == b'T',
        13 | 16 => return *byte == b':',
        _ => return byte.is_ascii_digit(),
    });
}
