use std::{
    fs::{self, File},
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
};

use termtrace_engine::replay::derived_directory_name_for;
use termtrace_evidence::{
    artifact::{finish_writer, secure_writer, verify_artifact_members},
    event_log::EventLog,
};
use termtrace_model::{
    artifact::visual::VideoMapEntry,
    terminal::replay::{
        GHOSTTY_REVISION, ReplayManifest, STATE_FORMAT_VERSION, SourceRange, StateIndexEntry,
        StateSelection,
    },
};

use super::{Error, Result};

pub(super) fn read_state_index(
    session: &Path,
    range: Option<SourceRange>,
    selection: StateSelection,
) -> Result<Vec<StateIndexEntry>> {
    let path = session
        .join(derived_directory_name_for(range, selection))
        .join("state-index.jsonl");
    if !path.is_file() {
        return Err(Error::InvalidReplay {
            reason: format!(
                "matching semantic index is missing; run `termtrace analyze index --session {}` first",
                session.display()
            ),
        });
    }
    return BufReader::new(File::open(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?)
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
                path: Path::new(&format!("{}:line {}", path.display(), index + 1)).to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        });
    })
    .collect();
}

pub(super) fn validate_replay_manifest(
    session: &Path,
    log: &EventLog,
    states: &[StateIndexEntry],
    range: Option<SourceRange>,
    selection: StateSelection,
) -> Result<()> {
    let path = session
        .join(derived_directory_name_for(range, selection))
        .join("manifest.json");
    let bytes = fs::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let manifest: ReplayManifest = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    verify_artifact_members(
        path.parent().ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "semantic manifest has no artifact directory".to_owned(),
            };
        })?,
        &manifest.members,
    )?;
    let expected_hash = log.events.last().map(|event| return event.hash.clone());
    let count = u64::try_from(states.len()).unwrap_or(u64::MAX);
    let final_hash = states.last().map(|state| return state.state_hash.as_str());
    if manifest.version != STATE_FORMAT_VERSION
        || manifest.session_id != log.header.session_id
        || manifest.source_event_hash != expected_hash
        || manifest.source_range != range
        || !manifest.source_complete
        || manifest.state_selection != selection
        || manifest.ghostty_revision != GHOSTTY_REVISION
        || manifest.state_count != count
        || manifest.final_retained_state_hash.as_deref() != final_hash
        || (selection == StateSelection::Presented
            && states.iter().any(|state| return !state.presented))
    {
        return Err(Error::InvalidReplay { reason: "semantic index does not match the capture request; recreate the matching derived index".to_owned() });
    }
    return Ok(());
}

pub(super) fn write_video_map(
    path: &Path,
    states: &[StateIndexEntry],
    fps: u16,
    capture_origin: u128,
    replay_origin: u128,
    range: Option<SourceRange>,
) -> Result<()> {
    let mut writer = secure_writer(path)?;
    for state in states {
        let relative_source_ns = match range {
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
        let relative = replay_origin
            .saturating_add(u128::from(relative_source_ns))
            .saturating_sub(capture_origin);
        let frame = state.presented.then(|| {
            return u64::try_from(relative.saturating_mul(u128::from(fps)) / 1_000_000_000)
                .unwrap_or(u64::MAX);
        });
        serde_json::to_writer(
            &mut writer,
            &VideoMapEntry {
                state_id: state.state_id,
                state_hash: state.state_hash.clone(),
                monotonic_ns: state.monotonic_ns,
                frame,
                presented: state.presented,
            },
        )
        .map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        writer.write_all(b"\n").map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    }
    return finish_writer(writer, path).map_err(Error::from);
}
