//! Read-only semantic forensic reviewer workflows.

use std::{
    fs::{self, DirBuilder},
    io,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
    process,
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};

use termtrace_evidence::{
    artifact::{commit_directory, write_secure},
    event_log::EventLogCursor,
    session::validate_completed_session,
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
    analysis::diagnostics::{ValidatedVisualManifest, visit_native_correlations},
    observation::state_text,
    replay::{inspect_completed_session, visit_session_states_while},
};

mod pager;
pub use pager::{
    REVIEW_PAGE_STATES, ReviewEntry, ReviewPage, ReviewQuery, ReviewViewport, load_review_page,
};

/// Load and summarize a validated completed session without changing it.
pub fn summarize(
    session: impl AsRef<Path>,
    visual_evidence: &[ValidatedVisualManifest],
) -> Result<ReviewSummary> {
    let session = session.as_ref().to_path_buf();
    let completion = validate_completed_session(&session)?;
    let mut state_count = 0_u64;
    let mut presented_state_count = 0_u64;
    let mut final_state_hash = String::new();
    visit_session_states_while(&session, &AtomicBool::new(false), |record, _| {
        state_count = state_count.saturating_add(1);
        if record.presented {
            presented_state_count = presented_state_count.saturating_add(1);
        }
        final_state_hash.clone_from(&record.state_hash);
        return Ok(true);
    })?;
    if state_count == 0 {
        return Err(review_error(
            "completed session contains no semantic states",
        ));
    }
    let mut native_correlation_count = 0_u64;
    visit_native_correlations(completion.session_id, None, visual_evidence, |_| {
        native_correlation_count = native_correlation_count.saturating_add(1);
    });
    let mut marker_count = 0_u64;
    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;
    while let Some(record) = cursor.next_record()? {
        if matches!(record.event, Event::Marker { .. }) {
            marker_count = marker_count.saturating_add(1);
        }
    }
    cursor.validate_boundary(completion.event_count, &completion.final_event_hash)?;
    let _pending = cursor.finish()?;
    return Ok(ReviewSummary {
        canonical_session: session,
        state_count,
        presented_state_count,
        marker_count,
        native_correlation_count,
        final_state_hash,
        rendering_policy: "semantic_cells_only_no_raw_replay".to_owned(),
    });
}

/// Search every semantic viewport in a validated completed session.
pub fn search(session: impl AsRef<Path>, query: &str) -> Result<ReviewSearchResult> {
    validate_query(query)?;
    let session = session.as_ref().to_path_buf();
    validate_completed_session(&session)?;
    let mut matches: Vec<ReviewSearchMatch> = Vec::new();
    visit_session_states_while(&session, &AtomicBool::new(false), |record, state| {
        if state_text(state).contains(query) {
            matches.push(ReviewSearchMatch {
                state_id: record.state_id,
                event_seq: record.event_seq,
                presented: record.presented,
                state_hash: record.state_hash.clone(),
                selector: format!("--state {}", record.state_id),
            });
        }
        return Ok(true);
    })?;
    return Ok(ReviewSearchResult {
        canonical_session: session,
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
    let session = session.as_ref().to_path_buf();
    let completion = validate_completed_session(&session)?;
    let inspection = inspect_completed_session(&session, StateSelector::StateId(state_id))
        .map_err(|error| {
            return review_error(format!("review state {state_id} does not exist: {error}"));
        })?;
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
    let result = export_into(
        &temporary,
        destination,
        &session,
        &completion.final_event_hash,
        &inspection.record,
        state_json,
        screen,
    );
    if result.is_err() {
        let _cleanup_result = fs::remove_dir_all(&temporary);
    }
    return result;
}

fn export_into(
    temporary: &Path,
    destination: &Path,
    session: &Path,
    source_final_event_hash: &str,
    state: &StateRecord,
    state_json: Vec<u8>,
    screen: String,
) -> Result<ReviewExportManifest> {
    write_secure(&temporary.join("state.json"), &state_json)?;
    write_secure(&temporary.join("screen.txt"), screen.as_bytes())?;
    let manifest = ReviewExportManifest {
        version: 1,
        canonical_session: session.to_path_buf(),
        state_id: state.state_id,
        state_hash: state.state_hash.clone(),
        source_final_event_hash: source_final_event_hash.to_owned(),
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
    let parent = destination
        .parent()
        .ok_or_else(|| return review_error("review export destination has no parent"))?;
    commit_directory(parent, temporary, destination)?;
    return Ok(manifest);
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
