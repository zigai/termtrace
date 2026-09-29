use crate::{
    error::{invalid, operation},
    Error, Result,
};
use serde::Serialize;
use std::{
    fmt::Display,
    fs::Permissions,
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};
use termtrace_engine::{
    analysis::diagnostics::diagnose_snapshot, timeline::visit_snapshot_timeline_while,
    Error as EngineError,
};
use termtrace_evidence::{
    artifact::{finish_writer, secure_writer},
    session::SessionSnapshot,
};
use termtrace_model::scenario::{Scenario, ScenarioReport};
use termtrace_model::{
    protocol::control::ControlData, terminal::analysis::DiagnosticCheck,
    terminal::state::SemanticState, value::ByteString,
};
use zstd::stream::decode_all;

pub(crate) fn snapshot_path(failure_root: &Path, name: &str) -> Result<PathBuf> {
    let artifact_root = failure_root
        .parent()
        .ok_or_else(|| return invalid("scenario failure root has no artifact parent"))?;

    return Ok(artifact_root
        .join("snapshots")
        .join(format!("{name}.frame.json")));
}

pub(crate) fn publish_failure(
    destination: &Path,
    scenario: &Scenario,
    report: &ScenarioReport,
    final_snapshot: Option<&ControlData>,
) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        return invalid("scenario failure destination must have a parent directory");
    })?;

    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;

    let temporary = parent.join(format!(".{}.tmp", report.session_id));
    fs_err::create_dir(&temporary).map_err(|source| {
        return Error::Filesystem {
            path: temporary.clone(),
            source,
        };
    })?;
    fs_err::set_permissions(&temporary, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: temporary.clone(),
            source,
        };
    })?;

    let publish_result = populate_failure(&temporary, scenario, report, final_snapshot);
    if let Err(error) = publish_result {
        let _remove = fs_err::remove_dir_all(&temporary);

        return Err(error);
    }

    fs_err::rename(&temporary, destination).map_err(|source| {
        return Error::Filesystem {
            path: destination.to_path_buf(),
            source,
        };
    })?;

    return Ok(());
}

fn populate_failure(
    temporary: &Path,
    scenario: &Scenario,
    report: &ScenarioReport,
    final_snapshot: Option<&ControlData>,
) -> Result<()> {
    let mut redacted_scenario = scenario.clone();
    for value in redacted_scenario.launch.environment.set.values_mut() {
        "<redacted>".clone_into(value);
    }

    for value in redacted_scenario.launch.environment.set_bytes.values_mut() {
        value.base64 = ByteString::from(b"<redacted>".as_slice());
    }

    write_json(temporary.join("scenario.json"), &redacted_scenario)?;
    write_json(temporary.join("result.json"), report)?;

    let mut derivation_errors: Vec<String> = Vec::new();

    if let Some(snapshot) = final_snapshot {
        write_json(temporary.join("final-screen.json"), snapshot)?;

        if let ControlData::Snapshot {
            semantic_state_zstd,
            viewport_text,
            ..
        } = snapshot
        {
            write_bytes(temporary.join("final-screen.txt"), viewport_text.as_bytes())?;

            match decode_all(semantic_state_zstd.as_bytes())
                .map_err(|error| return error.to_string())
                .and_then(|json| {
                    return serde_json::from_slice::<SemanticState>(&json)
                        .map_err(|error| return error.to_string());
                })
                .and_then(|state| {
                    return termtrace_render::render_png(&state)
                        .map_err(|error| return error.to_string());
                }) {
                Ok(png) => write_bytes(temporary.join("final-screen.png"), &png)?,
                Err(error) => derivation_errors.push(format!("semantic image: {error}")),
            }
        }
    }

    match SessionSnapshot::capture(&report.canonical_session, None) {
        Ok(snapshot) => {
            if let Err(error) = write_timeline(&snapshot, &temporary.join("timeline.json")) {
                derivation_errors.push(format!("timeline: {error}"));
            }

            match diagnose_snapshot(&snapshot, DiagnosticCheck::all(), 50) {
                Ok(diagnosis) => write_json(temporary.join("diagnose.json"), &diagnosis)?,
                Err(error) => derivation_errors.push(format!("diagnose: {error}")),
            }
        }
        Err(error) => derivation_errors.push(format!("event log: {error}")),
    }

    if !derivation_errors.is_empty() {
        write_json(temporary.join("derivation-errors.json"), &derivation_errors)?;
    }

    return Ok(());
}

const MAX_SCENARIO_FAILURE_TIMELINE_ENTRIES: usize = 1_000;

fn write_timeline(snapshot: &SessionSnapshot, path: &Path) -> Result<()> {
    let write = (|| -> Result<()> {
        let mut output =
            secure_writer(path).map_err(|error| return operation("create timeline", error))?;
        output
            .write_all(b"[\n")
            .map_err(|error| return operation("write timeline", error))?;

        let mut first = true;
        let mut entry_count = 0_usize;

        visit_snapshot_timeline_while(snapshot, true, true, None, |entry| {
            if entry_count >= MAX_SCENARIO_FAILURE_TIMELINE_ENTRIES {
                return Ok(false);
            }

            if !first {
                output.write_all(b",\n").map_err(timeline_error)?;
            }

            first = false;
            entry_count = entry_count.saturating_add(1);
            serde_json::to_writer(&mut output, &entry).map_err(timeline_error)?;

            return Ok(true);
        })
        .map_err(|error| return operation("derive timeline", error))?;

        output
            .write_all(b"\n]\n")
            .map_err(|error| return operation("write timeline", error))?;
        finish_writer(output, path).map_err(|error| return operation("finish timeline", error))?;

        return Ok(());
    })();

    if write.is_err() {
        let _cleanup = fs_err::remove_file(path);
    }

    return write;
}

fn timeline_error(error: impl Display) -> EngineError {
    return EngineError::InvalidReplay {
        reason: format!("write scenario timeline: {error}"),
        source: None,
    };
}

pub(crate) fn write_bytes(path: PathBuf, bytes: &[u8]) -> Result<()> {
    fs_err::write(&path, bytes).map_err(|source| return Error::Filesystem { path, source })?;

    return Ok(());
}

pub(crate) fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let mut encoded = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    encoded.push(b'\n');

    return write_bytes(path, &encoded);
}
