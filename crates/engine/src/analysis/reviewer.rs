//! Read-only semantic forensic reviewer workflows.

use std::{
    fs::{self, DirBuilder},
    io,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use termtrace_evidence::{
    artifact::write_secure, event_log::EventLog, session::validate_completed_log,
};
use termtrace_model::{
    execution::event::Event,
    terminal::replay::{StateRecord, StateSelector},
    terminal::reviewer::{
        ReviewExportManifest, ReviewSearchMatch, ReviewSearchResult, ReviewSummary,
    },
};

use crate::{
    Error, Result,
    analysis::diagnostics::{ValidatedVisualManifest, native_correlations},
    observation::state_text,
    replay::{inspect, replay},
};

struct ReviewSource {
    session: PathBuf,
    log: EventLog,
    states: Vec<StateRecord>,
    native_correlation_count: u64,
}

impl ReviewSource {
    fn open(
        session: impl AsRef<Path>,
        around_marker: Option<&str>,
        visual_evidence: &[ValidatedVisualManifest],
    ) -> Result<(Self, usize)> {
        let session = session.as_ref().to_path_buf();
        let log = EventLog::read(session.join("events.jsonl"))?;
        validate_completed_log(&session, &log)?;
        let replayed = replay(&log)?;
        if replayed.states.is_empty() {
            return Err(review_error(
                "completed session contains no semantic states",
            ));
        }
        let selected = match around_marker {
            Some(label) => marker_state_index(&log, &replayed.states, label)?,
            None => replayed.states.len().saturating_sub(1),
        };
        let native_correlation_count = u64::try_from(
            native_correlations(&log, visual_evidence)
                .values()
                .map(Vec::len)
                .sum::<usize>(),
        )
        .unwrap_or(u64::MAX);
        return Ok((
            Self {
                session,
                log,
                states: replayed.states,
                native_correlation_count,
            },
            selected,
        ));
    }

    fn summary(&self) -> ReviewSummary {
        return ReviewSummary {
            canonical_session: self.session.clone(),
            state_count: u64::try_from(self.states.len()).unwrap_or(u64::MAX),
            presented_state_count: u64::try_from(
                self.states
                    .iter()
                    .filter(|state| return state.presented)
                    .count(),
            )
            .unwrap_or(u64::MAX),
            marker_count: u64::try_from(
                self.log
                    .events
                    .iter()
                    .filter(|record| matches!(record.event, Event::Marker { .. }))
                    .count(),
            )
            .unwrap_or(u64::MAX),
            native_correlation_count: self.native_correlation_count,
            final_state_hash: self
                .states
                .last()
                .map_or_else(String::new, |state| return state.state_hash.clone()),
            rendering_policy: "semantic_cells_only_no_raw_replay".to_owned(),
        };
    }
}

/// Load and summarize a validated completed session without changing it.
pub fn summarize(
    session: impl AsRef<Path>,
    around_marker: Option<&str>,
    visual_evidence: &[ValidatedVisualManifest],
) -> Result<ReviewSummary> {
    let (source, selected) = ReviewSource::open(session, around_marker, visual_evidence)?;
    let _selected_state = source
        .states
        .get(selected)
        .ok_or_else(|| return review_error("selected reviewer state is out of bounds"))?;
    return Ok(source.summary());
}

/// Search every semantic viewport in a validated completed session.
pub fn search(session: impl AsRef<Path>, query: &str) -> Result<ReviewSearchResult> {
    validate_query(query)?;
    let (source, _) = ReviewSource::open(session, None, &[])?;
    let mut matches: Vec<ReviewSearchMatch> = Vec::new();
    for state in &source.states {
        let inspection = inspect(&source.log, StateSelector::StateId(state.state_id))?;
        if !state_text(&inspection.state).contains(query) {
            continue;
        }
        matches.push(ReviewSearchMatch {
            state_id: state.state_id,
            event_seq: state.event_seq,
            presented: state.presented,
            state_hash: state.state_hash.clone(),
            selector: format!("--state {}", state.state_id),
        });
    }
    return Ok(ReviewSearchResult {
        canonical_session: source.session,
        query: query.to_owned(),
        matches,
    });
}

/// Atomically export one semantic state and viewport text without replacement.
pub fn export_state(
    session: impl AsRef<Path>,
    state_id: u64,
    destination: impl AsRef<Path>,
) -> Result<ReviewExportManifest> {
    let (source, _) = ReviewSource::open(session, None, &[])?;
    let state = source
        .states
        .iter()
        .find(|state| return state.state_id == state_id)
        .ok_or_else(|| return review_error(format!("review state {state_id} does not exist")))?;
    let inspection = inspect(&source.log, StateSelector::StateId(state_id))?;
    let state_json = serde_json::to_vec_pretty(&inspection).map_err(|source_error| {
        return Error::InvalidJson {
            path: PathBuf::from("<review-state>"),
            json_path: "$".to_owned(),
            source: source_error,
        };
    })?;
    let screen = state_text(&inspection.state);
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(review_error("review export destination already exists"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return review_error("review export destination has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|source_error| return filesystem_error(parent, source_error))?;
    let temporary = temporary_export_path(parent)?;
    DirBuilder::new()
        .mode(0o700)
        .create(&temporary)
        .map_err(|source_error| return filesystem_error(&temporary, source_error))?;
    let result = export_into(&temporary, destination, &source, state, state_json, screen);
    if result.is_err() {
        let _cleanup_result = fs::remove_dir_all(&temporary);
    }
    return result;
}

fn export_into(
    temporary: &Path,
    destination: &Path,
    source: &ReviewSource,
    state: &StateRecord,
    state_json: Vec<u8>,
    screen: String,
) -> Result<ReviewExportManifest> {
    write_secure(&temporary.join("state.json"), &state_json)?;
    write_secure(&temporary.join("screen.txt"), screen.as_bytes())?;
    let manifest = ReviewExportManifest {
        version: 1,
        canonical_session: source.session.clone(),
        state_id: state.state_id,
        state_hash: state.state_hash.clone(),
        source_final_event_hash: source.log.events.last().map_or_else(
            || return source.log.header.session_manifest_hash.clone(),
            |record| return record.hash.clone(),
        ),
        state_json_blake3: blake3::hash(&state_json).to_hex().to_string(),
        screen_text_blake3: blake3::hash(screen.as_bytes()).to_hex().to_string(),
    };
    let mut manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|source_error| {
        return Error::InvalidJson {
            path: temporary.join("manifest.json"),
            json_path: "$".to_owned(),
            source: source_error,
        };
    })?;
    manifest_json.push(b'\n');
    write_secure(&temporary.join("manifest.json"), &manifest_json)?;
    fs::rename(temporary, destination)
        .map_err(|source_error| return filesystem_error(destination, source_error))?;
    return Ok(manifest);
}

fn marker_state_index(log: &EventLog, states: &[StateRecord], label: &str) -> Result<usize> {
    let matches: Vec<_> = log
        .events
        .iter()
        .filter(
            |record| matches!(&record.event, Event::Marker { label: value, .. } if value == label),
        )
        .collect();
    let [marker] = matches.as_slice() else {
        return Err(review_error(format!(
            "review marker `{label}` must occur exactly once, found {}",
            matches.len()
        )));
    };
    return states
        .iter()
        .position(|state| return state.monotonic_ns >= marker.monotonic_ns)
        .or_else(|| return states.len().checked_sub(1))
        .ok_or_else(|| return review_error("review contains no state near marker"));
}

fn validate_query(query: &str) -> Result<()> {
    if !query.is_empty() && query.len() <= 4096 {
        return Ok(());
    }
    return Err(review_error(
        "review search must contain 1 through 4096 bytes",
    ));
}

fn temporary_export_path(parent: &Path) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| {
            return review_error(format!("system clock precedes Unix epoch: {source}"));
        })?
        .as_nanos();
    return Ok(parent.join(format!(".review-export-{}-{timestamp}.tmp", process::id())));
}

fn filesystem_error(path: &Path, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.to_path_buf(),
        source,
    };
}

fn review_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
