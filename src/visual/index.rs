use std::{
    fs::File,
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
};

use super::VideoMapEntry;
use crate::{
    Error, Result,
    artifact::{finish_writer, secure_writer, verify_artifact_members},
    event_log::EventLog,
    replay::{
        GHOSTTY_REVISION, ReplayManifest, STATE_FORMAT_VERSION, StateIndexEntry, StateSelection,
        derived_directory_name_for,
    },
    timeline::SourceRange,
};

use super::util::json_error;

pub(super) fn read_state_index(
    session: &Path,
    source_range: Option<SourceRange>,
    state_selection: StateSelection,
) -> Result<Vec<StateIndexEntry>> {
    let path = session
        .join(derived_directory_name_for(source_range, state_selection))
        .join("state-index.jsonl");
    if !path.is_file() {
        let range_flags = source_range.map_or_else(String::new, |range| {
            return format!(
                " --from {}ns --to {}ns",
                range.start_monotonic_ns, range.end_monotonic_ns
            );
        });
        return Err(Error::InvalidReplay {
            reason: format!(
                "matching semantic index is missing; run `termtrace analyze index --session {}{range_flags}` first",
                session.display()
            ),
        });
    }
    let reader = BufReader::new(File::open(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?);
    return reader
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let line = line.map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
            return serde_json::from_str(&line).map_err(|source| {
                return Error::InvalidJson {
                    path: path.clone(),
                    json_path: format!("line {}", index + 1),
                    source,
                };
            });
        })
        .collect();
}

pub(super) fn validate_replay_manifest(
    session: &Path,
    event_log: &EventLog,
    state_index: &[StateIndexEntry],
    source_range: Option<SourceRange>,
    state_selection: StateSelection,
) -> Result<()> {
    let path = session
        .join(derived_directory_name_for(source_range, state_selection))
        .join("manifest.json");
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let manifest: ReplayManifest =
        serde_json::from_slice(&bytes).map_err(|source| return json_error(path.clone(), source))?;
    let artifact_directory = path.parent().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "semantic manifest has no artifact directory".to_owned(),
        };
    })?;
    verify_artifact_members(artifact_directory, &manifest.members)?;
    let source_event_hash = event_log
        .events
        .last()
        .map(|event| return event.hash.clone());
    let indexed_count = u64::try_from(state_index.len()).unwrap_or(u64::MAX);
    let mut mismatches = Vec::new();
    if manifest.version != STATE_FORMAT_VERSION {
        mismatches.push("state format version");
    }
    if manifest.session_id != event_log.header.session_id {
        mismatches.push("session identifier");
    }
    if manifest.source_event_hash != source_event_hash {
        mismatches.push("source event hash");
    }
    if manifest.source_range != source_range {
        mismatches.push("source range");
    }
    if !manifest.source_complete {
        mismatches.push("completion status");
    }
    if manifest.state_selection != state_selection {
        mismatches.push("state selection");
    }
    if manifest.ghostty_revision != GHOSTTY_REVISION {
        mismatches.push("Ghostty revision");
    }
    if manifest.state_count != indexed_count {
        mismatches.push("retained state count");
    }
    let final_retained_state_hash = state_index
        .last()
        .map(|entry| return entry.state_hash.as_str());
    if manifest.final_retained_state_hash.as_deref() != final_retained_state_hash {
        mismatches.push("final retained state hash");
    }
    if state_selection == StateSelection::Presented
        && state_index.iter().any(|entry| return !entry.presented)
    {
        mismatches.push("presented-only contents");
    }
    if !mismatches.is_empty() {
        return Err(Error::InvalidReplay {
            reason: format!(
                "semantic index does not match the capture request ({}); remove the matching derived directory and run `termtrace analyze index` after recording completion",
                mismatches.join(", ")
            ),
        });
    }
    return Ok(());
}

pub(super) fn write_video_map(
    path: &Path,
    states: &[StateIndexEntry],
    fps: u16,
    capture_origin: u128,
    replay_origin: u128,
    source_range: Option<SourceRange>,
) -> Result<()> {
    let mut writer = secure_writer(path)?;
    for state in states {
        // State timestamps stay absolute so callers can join this map back to the
        // canonical event log. Only presentation math becomes range-relative;
        // rewriting provenance timestamps would make two range maps incomparable.
        let relative_source_ns = match source_range {
            Some(range) if range.contains(state.monotonic_ns) => {
                state.monotonic_ns - range.start_monotonic_ns
            }
            Some(range) => {
                return Err(Error::InvalidReplay {
                    reason: format!(
                        "semantic state {} at {} ns is outside capture range {}..{}",
                        state.state_id,
                        state.monotonic_ns,
                        range.start_monotonic_ns,
                        range.end_monotonic_ns
                    ),
                });
            }
            None => state.monotonic_ns,
        };
        let presentation = replay_origin.saturating_add(u128::from(relative_source_ns));
        let relative = presentation.saturating_sub(capture_origin);
        let frame = state.presented.then(|| {
            return u64::try_from(relative.saturating_mul(u128::from(fps)) / 1_000_000_000)
                .unwrap_or(u64::MAX);
        });
        let entry = VideoMapEntry {
            state_id: state.state_id,
            state_hash: state.state_hash.clone(),
            monotonic_ns: state.monotonic_ns,
            frame,
            presented: state.presented,
        };
        serde_json::to_writer(&mut writer, &entry)
            .map_err(|source| return json_error(path.to_path_buf(), source))?;
        writer.write_all(b"\n").map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    }
    return finish_writer(writer, path);
}
