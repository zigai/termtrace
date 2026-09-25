use super::{Error, Result};
use std::{
    fs::{self, File},
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    path::{Path, PathBuf},
};
use termtrace_engine::replay::derived_directory_name_for;
use termtrace_evidence::{
    artifact::{finish_writer, secure_writer, verify_artifact_members},
    event_log::EventLogSummary,
};
use termtrace_model::{
    artifact::metadata::ArtifactMember,
    artifact::visual::VideoMapEntry,
    terminal::replay::{
        ReplayManifest, SourceRange, StateIndexEntry, StateSelection, GHOSTTY_REVISION,
        STATE_FORMAT_VERSION, STATE_INDEX,
    },
};

#[derive(Clone)]
pub(super) struct StateIndexSource {
    path: PathBuf,
    member: ArtifactMember,
}

struct StateIndexCursor {
    source: StateIndexSource,
    reader: BufReader<File>,
    hash: blake3::Hasher,
    byte_length: u64,
    line: Vec<u8>,
}

impl StateIndexSource {
    fn open(&self) -> Result<StateIndexCursor> {
        let file = File::open(&self.path).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;

        return Ok(StateIndexCursor {
            source: self.clone(),
            reader: BufReader::new(file),
            hash: blake3::Hasher::new(),
            byte_length: 0,
            line: Vec::new(),
        });
    }
}

impl StateIndexCursor {
    fn next_state(&mut self) -> Result<Option<StateIndexEntry>> {
        self.line.clear();
        let count = self
            .reader
            .by_ref()
            .take(64 * 1024 + 1)
            .read_until(b'\n', &mut self.line)
            .map_err(|source| {
                return Error::Filesystem {
                    path: self.source.path.clone(),
                    source,
                };
            })?;

        if count == 0 {
            if self.byte_length != self.source.member.byte_length
                || self.hash.finalize().to_hex().as_str() != self.source.member.blake3
            {
                return Err(Error::InvalidReplay {
                    reason: "state index changed or does not match its manifest".to_owned(),
                });
            }
            return Ok(None);
        }

        if count > 64 * 1024 {
            return Err(Error::InvalidReplay {
                reason: "state-index entry exceeds 64 KiB".to_owned(),
            });
        }

        self.hash.update(&self.line);
        self.byte_length = self
            .byte_length
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));

        if self.byte_length > self.source.member.byte_length {
            return Err(Error::InvalidReplay {
                reason: "state index exceeds its committed length".to_owned(),
            });
        }
        return serde_json::from_slice(&self.line)
            .map(Some)
            .map_err(|source| {
                return Error::InvalidJson {
                    path: self.source.path.clone(),
                    json_path: "$".to_owned(),
                    source,
                };
            });
    }
}

pub(super) fn validate_replay_manifest(
    session: &Path,
    log: &EventLogSummary,
    range: Option<SourceRange>,
    selection: StateSelection,
) -> Result<StateIndexSource> {
    let path = session
        .join(derived_directory_name_for(range, selection))
        .join("manifest.json");

    let bytes = fs::read(&path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            return Error::InvalidReplay {
                reason: "matching semantic index is missing; run analyze index for the requested range and state selection".to_owned(),
            };
        }
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

    let source = StateIndexSource {
        path: session
            .join(derived_directory_name_for(range, selection))
            .join(STATE_INDEX),
        member: manifest
            .members
            .iter()
            .find(|member| return member.path == STATE_INDEX)
            .ok_or_else(|| {
                return Error::InvalidReplay {
                    reason: "semantic manifest has no state-index member".to_owned(),
                };
            })?
            .clone(),
    };
    let mut states = source.open()?;
    let mut count = 0_u64;
    let mut final_hash = None;
    let mut hidden = false;

    while let Some(state) = states.next_state()? {
        count = count.saturating_add(1);
        final_hash = Some(state.state_hash);
        hidden |= !state.presented;
    }

    let expected_hash = log.terminal.as_ref().map(|event| return event.hash.clone());
    if manifest.version != STATE_FORMAT_VERSION
        || manifest.session_id != log.header.session_id
        || manifest.source_event_hash != expected_hash
        || manifest.source_range != range
        || !manifest.source_complete
        || manifest.state_selection != selection
        || manifest.ghostty_revision != GHOSTTY_REVISION
        || manifest.state_count != count
        || manifest.final_retained_state_hash != final_hash
        || (selection == StateSelection::Presented && hidden)
    {
        return Err(Error::InvalidReplay { reason: "semantic index does not match the capture request; recreate the matching derived index".to_owned() });
    }
    return Ok(source);
}

pub(super) fn write_video_map(
    path: &Path,
    source: &StateIndexSource,
    capture_log: &Path,
    replay_origin: u128,
    range: Option<SourceRange>,
) -> Result<()> {
    let mut writer = secure_writer(path)?;
    let file = File::open(capture_log).map_err(|source| {
        return Error::Filesystem {
            path: capture_log.to_path_buf(),
            source,
        };
    })?;

    let mut frames = AcquisitionFrames::new(BufReader::new(file).lines());
    let mut frame = frames.next_frame()?;
    let mut states = source.open()?;

    while let Some(state) = states.next_state()? {
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

        let target = replay_origin.saturating_add(u128::from(relative_source_ns));
        while frame.is_some_and(|(_, acquired)| return acquired < target) {
            frame = frames.next_frame()?;
        }

        let correlated = frame.filter(|_| return state.presented);
        serde_json::to_writer(
            &mut writer,
            &VideoMapEntry {
                state_id: state.state_id,
                state_hash: state.state_hash.clone(),
                monotonic_ns: state.monotonic_ns,
                frame: correlated.map(|(number, _)| return number),
                presented: state.presented,
                acquisition_unix_ns: correlated.map(|(_, acquired)| return acquired),
                correlation: Some("acquisition_clock_approximate".to_owned()),
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

pub(super) fn validate_acquisition_frames(path: &Path, expected_count: u64) -> Result<u128> {
    let file = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    let mut frames = AcquisitionFrames::new(BufReader::new(file).lines());
    let mut first = None;

    let mut count = 0;
    while let Some((_, acquired)) = frames.next_frame()? {
        first.get_or_insert(acquired);
        count += 1;
    }

    if count != expected_count {
        return Err(Error::InvalidReplay {
            reason: format!(
                "acquisition has {count} frames but encoded video has {expected_count}"
            ),
        });
    }
    return first.ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "native acquisition has no first frame".to_owned(),
        };
    });
}

pub(super) fn write_original_acquisition_map(
    path: &Path,
    acquisition_log: &Path,
    execution_origin_unix_ns: u128,
) -> Result<()> {
    let file = File::open(acquisition_log).map_err(|source| {
        return Error::Filesystem {
            path: acquisition_log.to_path_buf(),
            source,
        };
    })?;

    let mut frames = AcquisitionFrames::new(BufReader::new(file).lines());
    let mut writer = secure_writer(path)?;

    while let Some((frame, acquisition_unix_ns)) = frames.next_frame()? {
        let elapsed_since_execution_start_ns =
            acquisition_unix_ns.checked_sub(execution_origin_unix_ns);
        serde_json::to_writer(
            &mut writer,
            &serde_json::json!({
                "frame": frame,
                "acquisition_unix_ns": acquisition_unix_ns,
                "elapsed_since_execution_start_ns": elapsed_since_execution_start_ns,
                "correlation": "acquisition_clock_only_no_presentation_claim",
            }),
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

/// Parse integer timestamps rather than FFmpeg's rounded `pts_time` display.
struct AcquisitionFrames<I> {
    lines: I,
    time_base: Option<(u128, u128)>,
    previous: Option<(u64, u128)>,
}

impl<I: Iterator<Item = io::Result<String>>> AcquisitionFrames<I> {
    fn new(lines: I) -> Self {
        return Self {
            lines,
            time_base: None,
            previous: None,
        };
    }

    fn next_frame(&mut self) -> Result<Option<(u64, u128)>> {
        for line in self.lines.by_ref() {
            let line = line.map_err(|source| {
                return Error::InvalidReplay {
                    reason: format!("cannot read acquisition timestamps: {source}"),
                };
            })?;
            if !line.contains("Parsed_showinfo_") {
                continue;
            }

            if let Some((_, rest)) = line.split_once("config in time_base: ") {
                self.time_base = rest.split(',').next().and_then(|value| {
                    let (numerator, denominator) = value.trim().split_once('/')?;

                    return Some((numerator.parse().ok()?, denominator.parse().ok()?));
                });
            }

            let Some((_, rest)) = line.split_once(" n:") else {
                continue;
            };

            let invalid = || {
                return Error::InvalidReplay {
                    reason: "invalid or nonmonotonic native acquisition timestamp".to_owned(),
                };
            };
            let (number, rest) = rest.split_once(" pts:").ok_or_else(invalid)?;
            let number: u64 = number.trim().parse().map_err(|_| return invalid())?;
            let pts: u128 = rest
                .split_whitespace()
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| return invalid())?;
            let (numerator, denominator) = self.time_base.ok_or_else(invalid)?;
            let acquired = pts
                .checked_mul(numerator)
                .and_then(|value| return value.checked_mul(1_000_000_000))
                .and_then(|value| return value.checked_div(denominator))
                .ok_or_else(invalid)?;

            if self.previous.map_or(number != 0, |(previous, timestamp)| {
                return number != previous + 1 || acquired < timestamp;
            }) {
                return Err(invalid());
            }

            self.previous = Some((number, acquired));

            return Ok(self.previous);
        }

        if self.previous.is_none() {
            return Err(Error::InvalidReplay {
                reason: "native video has no acquisition timestamps".to_owned(),
            });
        }
        return Ok(None);
    }
}

#[cfg(test)]
mod timing_tests {
    use super::AcquisitionFrames;

    #[test]
    fn preserves_integer_precision_and_irregular_frame_intervals() {
        let lines = [
            "[Parsed_showinfo_0 @ x] config in time_base: 1/1000000, frame_rate: 24/1",
            "[Parsed_showinfo_0 @ x] n:   0 pts:1700000000123456 pts_time:1.7e+09",
            "[Parsed_showinfo_0 @ x] n:   1 pts:1700000000223456 pts_time:1.7e+09",
        ];
        let mut frames =
            AcquisitionFrames::new(lines.into_iter().map(|line| return Ok(line.to_owned())));
        assert_eq!(
            frames.next_frame().unwrap(),
            Some((0, 1_700_000_000_123_456_000))
        );
        assert_eq!(
            frames.next_frame().unwrap(),
            Some((1, 1_700_000_000_223_456_000))
        );
        assert_eq!(frames.next_frame().unwrap(), None);
    }
}

#[cfg(test)]
mod state_cursor_tests {
    use super::*;
    use termtrace_evidence::artifact::collect_artifact_members;

    fn source(directory: &Path, bytes: &[u8]) -> StateIndexSource {
        let path = directory.join(STATE_INDEX);
        fs::write(&path, bytes).expect("state index fixture");
        let member = collect_artifact_members(directory)
            .expect("members")
            .into_iter()
            .find(|member| return member.path == STATE_INDEX)
            .expect("state index member");

        return StateIndexSource { path, member };
    }

    #[test]
    fn streaming_state_source_rejects_a_changed_second_pass() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let state = StateIndexEntry {
            state_id: 1,
            state_hash: "a".repeat(64),
            event_seq: Some(0),
            event_byte_offset: Some(0),
            monotonic_ns: 1,
            presented: true,
            checkpoint: false,
            timeline_rank: None,
            control_index_offset: None,
            control_rank: None,
        };
        let mut bytes = serde_json::to_vec(&state).expect("state JSON");
        bytes.push(b'\n');
        let source = source(directory.path(), &bytes);
        let mut first = source.open().expect("first cursor");
        assert_eq!(
            first
                .next_state()
                .expect("first state")
                .expect("state")
                .state_id,
            1
        );
        assert!(first.next_state().expect("verified EOF").is_none());

        let mut changed = state;
        changed.state_id = 2;
        let mut changed = serde_json::to_vec(&changed).expect("changed JSON");
        changed.push(b'\n');
        fs::write(&source.path, changed).expect("replace index");
        let mut second = source.open().expect("second cursor");
        assert_eq!(
            second
                .next_state()
                .expect("changed entry")
                .expect("state")
                .state_id,
            2
        );
        assert!(
            second.next_state().is_err(),
            "second pass must retain the original commitment"
        );
    }

    #[test]
    fn state_cursor_bounds_each_untrusted_line() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = source(directory.path(), &vec![b'x'; 65 * 1024]);
        let error = source
            .open()
            .expect("cursor")
            .next_state()
            .expect_err("oversized line");
        assert!(error.to_string().contains("64 KiB"));
    }
}
