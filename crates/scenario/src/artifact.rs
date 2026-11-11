use crate::{
    error::{invalid, operation},
    Error, Result,
};
use serde::Serialize;
use std::{
    fmt::Display,
    io::Write as _,
    path::{Path, PathBuf},
};
use termtrace_engine::{
    analysis::diagnostics::diagnose_snapshot, timeline::visit_snapshot_timeline_while,
    Error as EngineError,
};
use termtrace_evidence::{
    artifact::{commit_directory, create_private_directory, finish_writer, secure_writer},
    session::SessionSnapshot,
};
use termtrace_model::scenario::{Scenario, ScenarioReport};
use termtrace_model::{
    protocol::control::ControlPayload, terminal::analysis::DiagnosticCheck,
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
    final_snapshot: Option<&ControlPayload>,
) -> Result<Vec<String>> {
    let parent = destination.parent().ok_or_else(|| {
        return invalid("scenario failure destination must have a parent directory");
    })?;

    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&temporary)
        .map_err(|e| return operation("stage failure evidence", e))?;

    let result = (|| {
        let warnings = populate_failure(&temporary, destination, scenario, report, final_snapshot)?;
        commit_directory(parent, &temporary, destination, false)
            .map_err(|e| return operation("publish failure evidence", e))?;

        return Ok(warnings);
    })();

    if result.is_err() {
        let _remove = fs_err::remove_dir_all(&temporary);
    }

    return result;
}

fn populate_failure(
    temporary: &Path,
    destination: &Path,
    scenario: &Scenario,
    report: &ScenarioReport,
    final_snapshot: Option<&ControlPayload>,
) -> Result<Vec<String>> {
    let mut redacted_scenario = scenario.clone();
    for value in redacted_scenario.launch.environment.set.values_mut() {
        "<redacted>".clone_into(value);
    }

    for value in redacted_scenario.launch.environment.set_bytes.values_mut() {
        value.base64 = ByteString::from(b"<redacted>".as_slice());
    }

    write_json(temporary.join("scenario.json"), &redacted_scenario)?;

    // This copy only claims publication in the transaction that successfully publishes it.
    let mut published_report = report.clone();
    published_report.failure_artifacts = None;

    // Return only new publication warnings; the caller appends them to its existing
    // diagnostics. The published report retains both sets in their original order.
    let mut derivation_errors = Vec::new();

    if let Some(snapshot) = final_snapshot {
        write_json(temporary.join("final-screen.json"), snapshot)?;

        if let ControlPayload::Snapshot {
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
                Ok(png) => {
                    if write_bytes(temporary.join("final-screen.png"), &png).is_err() {
                        let _remove = fs_err::remove_file(temporary.join("final-screen.png"));

                        derivation_errors.push("semantic image unavailable".to_owned());
                    }
                }
                Err(_) => derivation_errors.push("semantic image unavailable".to_owned()),
            }
        }
    }

    match report
        .canonical_session
        .as_ref()
        .ok_or_else(|| return "no canonical session".to_owned())
        .and_then(|path| {
            return SessionSnapshot::capture(path, None)
                .map_err(|_error| return "canonical evidence unavailable".to_owned());
        }) {
        Ok(snapshot) => {
            if let Err(_error) = write_timeline(&snapshot, &temporary.join("timeline.json")) {
                derivation_errors.push("timeline unavailable".to_owned());
            }

            match diagnose_snapshot(&snapshot, DiagnosticCheck::all(), 50) {
                Ok(diagnosis) => {
                    if write_json(temporary.join("diagnose.json"), &diagnosis).is_err() {
                        let _remove = fs_err::remove_file(temporary.join("diagnose.json"));

                        derivation_errors.push("diagnose unavailable".to_owned());
                    }
                }
                Err(_) => derivation_errors.push("diagnose unavailable".to_owned()),
            }
        }
        Err(_) => derivation_errors.push("event log unavailable".to_owned()),
    }

    published_report
        .artifact_warnings
        .extend(derivation_errors.iter().cloned());

    if !published_report.artifact_warnings.is_empty() {
        write_json(
            temporary.join("derivation-errors.json"),
            &published_report.artifact_warnings,
        )?;
    }

    // The caller sets its evidence path only after the atomic commit succeeds.
    published_report.failure_artifacts = Some(destination.to_path_buf());
    write_json(temporary.join("result.json"), &published_report)?;

    return Ok(derivation_errors);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::error_report;

    #[test]
    fn failed_publication_cleans_staging_and_never_replaces_or_claims_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let scenario: Scenario = serde_json::from_value(serde_json::json!({"version":2,"name":"publication", "launch":{"command":["/bin/true"]}, "steps":[]})).unwrap();
        let mut report = error_report("publication", "replay", "replay_integrity");
        let id = uuid::Uuid::now_v7();
        report.session_id = Some(id);
        report.canonical_session = Some(temp.path().join("missing.tt"));
        let destination = temp.path().join(id.to_string());
        fs_err::create_dir(&destination).unwrap();
        fs_err::write(destination.join("existing"), "untouched").unwrap();

        assert!(publish_failure(&destination, &scenario, &report, None).is_err());
        assert_eq!(report.failure_artifacts, None);
        assert_eq!(
            fs_err::read_to_string(destination.join("existing")).unwrap(),
            "untouched"
        );
        assert_eq!(
            fs_err::read_dir(temp.path()).unwrap().count(),
            1,
            "failed rename must remove staging"
        );

        fs_err::remove_dir_all(&destination).unwrap();
        let warnings = publish_failure(&destination, &scenario, &report, None).unwrap();
        assert_eq!(warnings, vec!["event log unavailable"]);
        let published: ScenarioReport =
            serde_json::from_slice(&fs_err::read(destination.join("result.json")).unwrap())
                .unwrap();
        assert_eq!(published.failure_artifacts, Some(destination));
        assert_eq!(published.artifact_warnings, warnings);
        assert!(
            report.failure_artifacts.is_none(),
            "caller marks publication only after success"
        );
    }
}
