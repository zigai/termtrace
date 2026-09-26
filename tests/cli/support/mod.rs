//! Shared imports and helpers for CLI integration tests.

pub(crate) use assert_cmd::cargo::CargoError;
use assert_cmd::Command as AssertCommand;
pub(crate) use predicates::prelude::*;
pub(crate) use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, Permissions},
    io::{self, BufRead, BufReader, Read, Write},
    ops::{Deref, DerefMut},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{symlink, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct Command(AssertCommand);

impl Command {
    pub(crate) fn cargo_bin<S: AsRef<str>>(name: S) -> Result<Self, CargoError> {
        let mut command = AssertCommand::cargo_bin(name)?;
        command.env_remove("TERMTRACE_RESPONSE");

        return Ok(Self(command));
    }

    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let mut command = AssertCommand::new(program);
        command.env_remove("TERMTRACE_RESPONSE");

        return Self(command);
    }
}

impl Deref for Command {
    type Target = AssertCommand;

    fn deref(&self) -> &Self::Target {
        return &self.0;
    }
}

impl DerefMut for Command {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return &mut self.0;
    }
}

#[derive(Debug)]
pub(crate) struct StdCommand(ProcessCommand);

impl StdCommand {
    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let mut command = ProcessCommand::new(program.as_ref());
        command.env_remove("TERMTRACE_RESPONSE");

        return Self(command);
    }
}

impl Deref for StdCommand {
    type Target = ProcessCommand;

    fn deref(&self) -> &Self::Target {
        return &self.0;
    }
}

impl DerefMut for StdCommand {
    fn deref_mut(&mut self) -> &mut Self::Target {
        return &mut self.0;
    }
}

#[derive(Debug)]
pub(crate) struct TempDir(tempfile::TempDir);

impl TempDir {
    pub(crate) fn new() -> io::Result<Self> {
        let directory =
            env::var_os("TERMTRACE_CI_EVIDENCE_ROOT").map_or_else(tempfile::TempDir::new, |root| {
                return tempfile::TempDir::new_in(root);
            });

        return directory.map(Self);
    }

    pub(crate) fn path(&self) -> &Path {
        return self.0.path();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if thread::panicking() && env::var_os("TERMTRACE_CI_EVIDENCE_ROOT").is_some() {
            self.0.disable_cleanup(true);
        }
    }
}

pub(crate) use termtrace_engine::{
    inspection::{inspect_session, wait_for_completion, wait_for_output},
    recording::control_socket_name,
    replay::{derived_directory_name, index_session_range, index_session_with_options, replay},
};
pub(crate) use termtrace_evidence::{
    event_log::EventLog,
    provenance::collect_execution_provenance,
    request_journal::read_mutation_journal,
    session::{read_manifest, validate_completed_session},
};
pub(crate) use termtrace_model::terminal::state::KeyChord;
pub(crate) use termtrace_model::{
    execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    execution::provenance::{
        ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind, ProvenanceSensitivity,
    },
    execution::session::{
        CaptureTarget, GeometrySource, SessionCompletion, SessionManifest, TmuxPaneLaunchKind,
    },
    protocol::control::{
        ControlCommand, ControlData, ControlRequest, ControlResult, ScreenMode, ScreenPredicate,
        ScreenWaitQuality, StableCaptureQuality, StateHashComparison,
    },
    terminal::analysis::DiagnosticCheck,
    terminal::observation::StatePoint,
    terminal::replay::{
        IndexOptions, ReplayManifest, SourceRange, StateRecord, StateSelection, StateSelector,
        GHOSTTY_REVISION,
    },
    terminal::screen::{ScreenFrameData, ScreenFrameSource, ScreenProjectionRequest, ScreenRegion},
    terminal::state::{KeyModifiers, NamedKey},
    value::ByteString,
};
pub(crate) use termtrace_sdk::{
    control_endpoint_is_reachable, send_request, send_request_response, ActRequest,
    ActionCondition, InputAtom, LaunchSpec, StableCaptureOptions, StopRequest,
    Terminal as ClientTerminal,
};

/// The Enter key with no modifiers.
pub(crate) const ENTER: KeyChord = KeyChord {
    key: NamedKey::Enter,
    modifiers: KeyModifiers {
        shift: false,
        control: false,
        alt: false,
        super_key: false,
    },
};

pub(crate) use zstd::stream::read::Decoder;

pub(crate) fn canonical_output_bytes(log: &EventLog) -> Vec<u8> {
    return log
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
        .collect();
}

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
    return serde_json::from_slice(bytes).expect("result JSON");
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
    validate_json_lines_text(
        schema,
        &fs_err::read_to_string(instance).expect("JSONL instance"),
    );
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
    clippy::panic,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
pub(crate) fn validate_json_lines_text(schema: &str, text: &str) {
    let validator = compile_schema(schema);

    for (index, line) in text.lines().enumerate() {
        let value = serde_json::from_str(line).expect("JSONL value");

        if let Err(error) = validator.validate(&value) {
            panic!("{schema} rejected record {}: {error}", index + 1);
        }
    }
}

#[allow(
    clippy::expect_used,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
pub(crate) fn compile_schema(schema: &str) -> jsonschema::Validator {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("schemas")
        .join(schema);

    let schema_value: serde_json::Value =
        serde_json::from_slice(&fs_err::read(path).expect("schema file")).expect("schema JSON");

    return jsonschema::validator_for(&schema_value).expect("valid schema");
}

pub(crate) fn validate_json_value(schema: &str, instance: &serde_json::Value) {
    validate_compiled_json_value(&compile_schema(schema), schema, instance);
}

#[allow(
    clippy::panic,
    reason = "schema validation is an assertion helper used only by integration tests"
)]
pub(crate) fn validate_compiled_json_value(
    validator: &jsonschema::Validator,
    schema: &str,
    instance: &serde_json::Value,
) {
    if let Err(error) = validator.validate(instance) {
        panic!("{schema} rejected instance {instance}: {error}");
    }
}
