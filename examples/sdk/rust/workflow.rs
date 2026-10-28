//! Ordinary test workflow: launch the line fixture at 80x24, wait for readiness, send guarded
//! text and Enter, assert the reply, compare a reviewed snapshot, finalize, inspect evidence.
//!
//! Usage: `workflow EVIDENCE_ROOT`. Set `TERMTRACE_WORKFLOW_MISMATCH=1` to expect a value the
//! fixture never prints; the run then fails and reports where its evidence was kept.

use serde_json::json;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};
use termtrace_sdk::{
    assertions::{assert_screen_text, SnapshotProfile},
    input::{ActRequest, KeyChord},
    screen::{frame_text, ScreenPredicate},
    testing::{
        run_terminal_test, EvidencePolicy, FinalizationReport, TerminalTestConfig, TestFailure,
        TestOutcome,
    },
    Terminal,
};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Mismatch {
    assertion: String,
    session: PathBuf,
    observation: String,
}

type Failure = Box<dyn std::error::Error + Send + Sync>;

fn contains(text: &str) -> ScreenPredicate {
    return ScreenPredicate::Contains {
        text: text.to_owned(),
    };
}

fn inspect(report: &FinalizationReport) -> Result<PathBuf, Failure> {
    let session = report
        .verified_artifacts
        .iter()
        .find(|path| return path.join("completion.json").is_file())
        .ok_or("no verified session evidence was retained")?;
    let completion: serde_json::Value =
        serde_json::from_slice(&fs::read(session.join("completion.json"))?)?;
    if completion["exit_code"] != 0 {
        return Err(format!("the fixture did not exit cleanly: {completion}").into());
    }

    return Ok(session.clone());
}

fn main() -> ExitCode {
    let root = env::args_os().nth(1).map(PathBuf::from);
    let binary = env::var_os("TERMTRACE_BIN");
    let (Some(root), Some(binary)) = (root, binary) else {
        eprintln!("usage: TERMTRACE_BIN=termtrace workflow EVIDENCE_ROOT");
        return ExitCode::from(2);
    };
    let Ok(repository) = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
    else {
        eprintln!("the repository checkout is unavailable");
        return ExitCode::from(2);
    };
    let expected = if env::var_os("TERMTRACE_WORKFLOW_MISMATCH").is_some() {
        "LINE_VALUE=goodbye"
    } else {
        "LINE_VALUE=hello"
    };

    let mut config = TerminalTestConfig::new(
        Terminal::new(binary, &root),
        [repository
            .join("fixtures/programs/terminal/line-command.sh")
            .into_os_string()],
    );
    config.columns = 80;
    config.rows = 24;
    config.evidence_policy = EvidencePolicy::Always;
    config.snapshot_root = repository.join("examples/sdk/snapshots");
    config.update_snapshots = env::var_os("TERMTRACE_UPDATE_SNAPSHOTS").is_some();

    let result = run_terminal_test(config, |test| -> Result<_, Result<Mismatch, Failure>> {
        let session = test
            .session()
            .ok_or_else(|| return Err("session closed".into()))?;
        let ready = session
            .wait_screen(contains("LINE_READY"), Duration::ZERO, TIMEOUT)
            .map_err(|error| return Err(error.into()))?;
        let enter = KeyChord::parse("enter")
            .map_err(|error| return Err(error.into()))?
            .ok_or_else(|| return Err("enter is a key".into()))?;
        let action = session
            .act(
                ActRequest::new()
                    .if_state(ready.frame.state_hash)
                    .text("hello")
                    .key(enter)
                    .until_screen(contains("LINE_VALUE="), TIMEOUT),
            )
            .map_err(|error| return Err(error.into()))?;
        let frame = action
            .frame()
            .ok_or_else(|| return Err("the action returned no frame".into()))?;
        if let Err(error) = assert_screen_text(frame, expected) {
            return Err(Ok(Mismatch {
                assertion: error.to_string(),
                session: session.path().to_path_buf(),
                observation: frame_text(frame),
            }));
        }

        test.assert_snapshot(
            "line-value",
            frame,
            SnapshotProfile::Text,
            Default::default(),
        )
        .map_err(|error| return Err(error.into()))?;
        let report = test.finalize(TestOutcome::Passed);
        assert_eq!(test.finalize(TestOutcome::Error), report);

        return Ok(report);
    });

    match result {
        Ok(report) => match inspect(&report) {
            Ok(session) => {
                println!("workflow passed; evidence: {}", session.display());
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("workflow evidence is incomplete: {error}");
                return ExitCode::FAILURE;
            }
        },
        Err(TestFailure::Body {
            error: Ok(mismatch),
            finalization,
        }) => {
            let failure = json!({
                "assertion": mismatch.assertion,
                "session": mismatch.session,
                "observation": mismatch.observation,
                "evidence": finalization.evidence_directory,
            });
            eprintln!("TERMTRACE_WORKFLOW_FAILURE {failure}");
            return ExitCode::FAILURE;
        }
        Err(TestFailure::Body {
            error: Err(error),
            finalization,
        }) => {
            eprintln!("workflow failed: {error}; evidence: {finalization:?}");
            return ExitCode::FAILURE;
        }
        Err(failure) => {
            eprintln!("workflow failed: {failure:?}");
            return ExitCode::FAILURE;
        }
    }
}
