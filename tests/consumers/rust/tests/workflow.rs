//! Runs the Rust workflow example as a subprocess, normally and in mismatch mode.

use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run(root: &Path, mismatch: bool) -> Output {
    let _ = fs::remove_dir_all(root);
    let mut command = Command::new(env!("CARGO_BIN_EXE_workflow"));
    command.arg(root).env_remove("TERMTRACE_UPDATE_SNAPSHOTS");
    if mismatch {
        command.env("TERMTRACE_WORKFLOW_MISMATCH", "1");
    } else {
        command.env_remove("TERMTRACE_WORKFLOW_MISMATCH");
    }

    return command.output().expect("run the workflow example");
}

fn session_id(session: &Path, file: &str) -> Value {
    let bytes = fs::read(session.join(file)).expect("read session evidence");
    let document: Value = serde_json::from_slice(&bytes).expect("session evidence JSON");

    return document["session_id"].clone();
}

#[test]
fn line_workflow_passes() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("workflow-pass");
    let output = run(&root, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn mismatch_fails_with_the_assertion_session_observation_and_evidence() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("workflow-mismatch");
    let output = run(&root, true);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the mismatch run passed: {stderr}"
    );

    let line = stderr
        .lines()
        .find_map(|line| return line.strip_prefix("TERMTRACE_WORKFLOW_FAILURE "))
        .unwrap_or_else(|| panic!("no failure report: {stderr}"));
    let failure: Value = serde_json::from_str(line).expect("failure report JSON");
    assert!(
        failure["assertion"]
            .as_str()
            .is_some_and(|text| return text.contains("LINE_VALUE=goodbye")),
        "{failure}"
    );
    assert!(
        failure["observation"]
            .as_str()
            .is_some_and(|text| return text.contains("LINE_VALUE=hello")),
        "{failure}"
    );

    let evidence = PathBuf::from(failure["evidence"].as_str().expect("evidence directory"));
    let session = PathBuf::from(failure["session"].as_str().expect("session path"));
    let mode = fs::metadata(&evidence)
        .expect("retained evidence")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "{}", evidence.display());
    assert!(session.starts_with(&evidence), "{failure}");
    assert_eq!(
        session_id(&session, "completion.json"),
        session_id(&session, "session.json")
    );
}
