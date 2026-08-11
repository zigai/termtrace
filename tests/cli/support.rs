//! Shared imports and helpers for CLI integration tests.

pub(crate) use assert_cmd::Command;
pub(crate) use predicates::prelude::*;
pub(crate) use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, File, Permissions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{Command as StdCommand, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
pub(crate) use tempfile::TempDir;
pub(crate) use termtrace::{
    Error,
    analysis::DiagnosticCheck,
    client::Terminal as ClientTerminal,
    control::{
        ControlCommand, ControlData, ControlRequest, ControlResult, ScreenMode, ScreenPredicate,
        ScreenRegion, StableCaptureQuality, StateHashComparison, control_endpoint_is_reachable,
        control_socket_name, send_request, send_request_response,
    },
    environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    event::{
        ByteString, EVENT_LOG_VERSION, Event, InputDescriptor, InputKind, InputSource,
        OperationStatus,
    },
    event_log::EventLog,
    ghostty_vt::{KeyModifiers, NamedKey},
    inspection::{inspect_session, wait_for_completion, wait_for_output, wait_for_screen},
    observation::StatePoint,
    provenance::{
        ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind, ProvenanceSensitivity,
        collect_execution_provenance,
    },
    replay::{
        GHOSTTY_REVISION, IndexOptions, ReplayManifest, StateRecord, StateSelection, StateSelector,
        derived_directory_name, index_session_range, index_session_with_options, replay,
    },
    request_journal::read_mutation_journal,
    session::{
        CaptureTarget, GeometrySource, SessionCompletion, SessionManifest, TmuxPaneLaunchKind,
        read_manifest, validate_completed_log,
    },
    timeline::SourceRange,
};
pub(crate) use zstd::stream::{decode_all, read::Decoder};

pub(crate) fn wait_for_control(session: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !control_endpoint_is_reachable(session) {
        assert!(
            Instant::now() < deadline,
            "recorder control endpoint did not become reachable"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when a result is not valid compact JSON"
)]
pub(crate) fn compact_json_line(bytes: &[u8]) -> serde_json::Value {
    assert_eq!(
        bytes.iter().filter(|byte| return **byte == b'\n').count(),
        1,
        "result must contain exactly one JSON line: {}",
        String::from_utf8_lossy(bytes)
    );
    assert_eq!(bytes.last(), Some(&b'\n'));
    assert!(
        !bytes.windows(2).any(|window| return window == b"\n "),
        "result must not contain pretty-JSON indentation: {}",
        String::from_utf8_lossy(bytes)
    );
    return serde_json::from_slice(bytes).expect("compact result JSON");
}

pub(crate) fn wait_for_path(path: &Path) {
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
    reason = "integration helper must stop when the generated checkpoint is invalid"
)]
pub(crate) fn assert_late_checkpoint_inspection(session: &Path) {
    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["analyze", "index", "--session"])
        .arg(session)
        .assert()
        .success();
    let entries =
        fs_err::read_to_string(session.join("derived/state-index.jsonl")).expect("state index");
    let entries = entries
        .lines()
        .map(|line| return serde_json::from_str::<serde_json::Value>(line).expect("index entry"))
        .collect::<Vec<_>>();
    let late_state = entries
        .last()
        .and_then(|entry| return entry["state_id"].as_u64())
        .expect("late state id");
    let checkpoint = entries
        .iter()
        .filter(|entry| return entry["checkpoint"] == true)
        .filter_map(|entry| return entry["state_id"].as_u64())
        .max()
        .expect("periodic checkpoint");
    assert!(
        checkpoint > 0 && checkpoint <= late_state,
        "late inspection requires a nonzero checkpoint: checkpoint={checkpoint} late={late_state}"
    );

    let mut inspect = Command::cargo_bin("termtrace").expect("binary should build");
    inspect
        .args(["analyze", "inspect", "--session"])
        .arg(session)
        .args([
            "--state",
            &late_state.to_string(),
            "--format",
            "json-compact",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            r#""state_id":{late_state}"#
        )));
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
pub(crate) fn validate_json_file(schema: &str, instance: &Path) {
    let value = serde_json::from_slice(&fs_err::read(instance).expect("instance file"))
        .expect("instance JSON");
    validate_json_value(schema, &value);
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
pub(crate) fn validate_json_lines(schema: &str, instance: &Path) {
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
pub(crate) fn read_zstd_text(path: &Path) -> String {
    let file = File::open(path).expect("compressed JSONL artifact");
    let mut decoder = Decoder::new(file).expect("zstd decoder");
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
pub(crate) fn validate_json_lines_text(schema: &str, text: &str) {
    for line in text.lines() {
        validate_json_value(schema, &serde_json::from_str(line).expect("JSONL value"));
    }
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
pub(crate) fn validate_json_value(schema: &str, instance: &serde_json::Value) {
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
