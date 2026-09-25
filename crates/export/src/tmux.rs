//! Complete, atomically published evidence exports for aggregate tmux recordings.

use crate::{bundle, Error, Result};
use std::{
    cell::Cell,
    collections::BTreeSet,
    ffi::OsString,
    os::unix::{ffi::OsStringExt as _, fs::FileTypeExt as _},
    path::{Path, PathBuf},
};
use termtrace_evidence::artifact::{
    commit_directory, create_private_directory, sync_directory, write_secure,
};
use termtrace_model::{
    artifact::bundle::{BundleProfile, BundleVerificationPolicy},
    value::ByteString,
};
use termtrace_tmux::{
    artifact::{
        TmuxBundleTrack, TmuxRecordingLifecycle, TmuxRecordingManifest, TMUX_RECORDING_FORMAT,
        TMUX_RECORDING_VERSION,
    },
    catalog::TmuxCatalog,
    journal,
};
use uuid::Uuid;

pub mod streaming;

const MAX_AGGREGATE_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

struct SourceTrack {
    role: &'static str,
    pane_id: Option<String>,
    session: PathBuf,
    session_id: Uuid,
}

fn parse_aggregate(path: &Path, bytes: &[u8]) -> Result<TmuxRecordingManifest> {
    return serde_json::from_slice(bytes).map_err(|source| return Error::json(path, source));
}

fn visit_source_tracks(
    aggregate_manifest: &Path,
    aggregate: &TmuxRecordingManifest,
    mut visit: impl FnMut(SourceTrack) -> Result<()>,
) -> Result<()> {
    let composite = aggregate
        .state
        .composite_track
        .link
        .as_ref()
        .ok_or_else(|| {
            return Error::invalid("aggregate tmux recording has no linked composite track");
        })?;

    visit(SourceTrack {
        role: "composite",
        pane_id: None,
        session: path_from_bytes(&composite.session),
        session_id: composite.session_id,
    })?;

    let mut current = aggregate
        .state
        .pane_tracks
        .iter()
        .map(|(pane, session)| return (pane.clone(), path_from_bytes(session)))
        .collect::<BTreeSet<_>>();

    let directory = aggregate_manifest
        .parent()
        .ok_or_else(|| return Error::invalid("aggregate manifest has no parent"))?;

    let catalog = TmuxCatalog::read_verified(directory, aggregate)?;

    let mut next = 0;
    loop {
        let page = catalog.tracks(next, 128, false)?;
        if page.is_empty() {
            break;
        }

        for entry in page {
            next = entry
                .sequence
                .checked_add(1)
                .ok_or_else(|| return Error::invalid("tmux catalog sequence overflow"))?;

            current.remove(&(
                entry.track.pane_id.clone(),
                path_from_bytes(&entry.track.session),
            ));

            visit(SourceTrack {
                role: "pane",
                pane_id: Some(entry.track.pane_id),
                session: path_from_bytes(&entry.track.session),
                session_id: entry.track.session_id,
            })?;
        }
    }

    if !current.is_empty() {
        return Err(Error::invalid(
            "current pane track is missing from the historical inventory",
        ));
    }
    return Ok(());
}

fn path_from_bytes(bytes: &ByteString) -> PathBuf {
    return PathBuf::from(OsString::from_vec(bytes.as_bytes().to_vec()));
}

fn track_directory(index: usize, pane_id: Option<&str>) -> String {
    if pane_id.is_some() {
        return format!("tracks/pane-{index:04}");
    }
    return "tracks/composite".to_owned();
}

fn validate_aggregate(aggregate: &TmuxRecordingManifest) -> Result<()> {
    if aggregate.format != TMUX_RECORDING_FORMAT || aggregate.version != TMUX_RECORDING_VERSION {
        return Err(Error::invalid(
            "unsupported aggregate tmux recording manifest format or version",
        ));
    }

    if aggregate.state.lifecycle != TmuxRecordingLifecycle::Completed {
        return Err(Error::invalid(
            "aggregate tmux recording and composite track must be completed before export",
        ));
    }
    return Ok(());
}

fn validate_destination(
    destination: &Path,
    aggregate_manifest: &Path,
    tracks: &[SourceTrack],
) -> Result<()> {
    let parent = match destination.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        Some(_) => Path::new("."),
        None => {
            return Err(Error::invalid(
                "tmux evidence export destination has no parent",
            ))
        }
    };

    let parent = parent
        .canonicalize()
        .map_err(|source| return Error::filesystem(parent, source))?;

    let name = destination
        .file_name()
        .ok_or_else(|| return Error::invalid("tmux evidence export destination has no basename"))?;

    let candidate = parent.join(name);

    let aggregate_root = aggregate_manifest
        .parent()
        .and_then(|path| return path.canonicalize().ok());

    if aggregate_root
        .as_ref()
        .is_some_and(|root| return candidate.starts_with(root))
    {
        return Err(Error::invalid(
            "tmux evidence export destination must be outside the aggregate recording",
        ));
    }

    for track in tracks {
        let canonical = track
            .session
            .canonicalize()
            .map_err(|source| return Error::filesystem(&track.session, source))?;
        if candidate.starts_with(&canonical) {
            return Err(Error::invalid(
                "tmux evidence export destination must be outside every canonical track",
            ));
        }
    }

    return Ok(());
}

fn read_regular_bounded(path: &Path, maximum: u64, description: &str) -> Result<Vec<u8>> {
    let metadata =
        fs_err::symlink_metadata(path).map_err(|source| return Error::filesystem(path, source))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink()
        || file_type.is_dir()
        || file_type.is_block_device()
        || file_type.is_char_device()
        || file_type.is_fifo()
        || file_type.is_socket()
    {
        return Err(Error::invalid(format!(
            "{description} `{}` must be a regular file",
            path.display()
        )));
    }

    if metadata.len() > maximum {
        return Err(Error::invalid(format!(
            "{description} `{}` exceeds {maximum} bytes",
            path.display()
        )));
    }
    return fs_err::read(path).map_err(|source| return Error::filesystem(path, source));
}

struct DirectoryCleanup {
    path: PathBuf,
    armed: Cell<bool>,
}

impl DirectoryCleanup {
    fn new(path: PathBuf) -> Self {
        return Self {
            path,
            armed: Cell::new(true),
        };
    }

    fn disarm(&self) {
        self.armed.set(false);
    }
}

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        if self.armed.get() {
            let _result = fs_err::remove_dir_all(&self.path);
        }
    }
}
