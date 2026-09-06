//! Complete, atomically published evidence exports for aggregate tmux recordings.

use termtrace_evidence::{tmux_catalog::TmuxCatalog, tmux_journal};

use std::{
    cell::Cell,
    collections::BTreeSet,
    ffi::OsString,
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::FileTypeExt as _,
    },
    path::{Path, PathBuf},
};

use termtrace_evidence::artifact::{
    collect_artifact_members, commit_directory, create_private_directory, sync_directory,
    verify_artifact_members, write_secure,
};
use termtrace_model::{
    artifact::bundle::{BundleProfile, BundleVerificationPolicy},
    artifact::tmux::{
        TMUX_BUNDLE_FORMAT, TMUX_BUNDLE_VERSION, TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION,
        TmuxBundleManifest, TmuxBundleTrack, TmuxRecordingLifecycle, TmuxRecordingManifest,
    },
    value::ByteString,
};
use uuid::Uuid;

use crate::{Error, Result, bundle};

pub mod streaming;

const MAX_AGGREGATE_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TMUX_BUNDLE_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_AGGREGATE_TRACKS: usize = 1_024;
const MAX_AGGREGATE_BUNDLE_ENTRIES: usize = 131_072;

/// Publish one complete aggregate tmux recording without replacing an existing destination.
///
/// # Errors
///
/// Returns an error unless the aggregate and every linked track are complete,
/// securely readable, internally related, and atomically publishable under the
/// destination parent.
pub fn create(
    aggregate_manifest: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<TmuxBundleManifest> {
    let aggregate_manifest = aggregate_manifest.as_ref();
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(Error::invalid(format!(
            "tmux evidence export destination `{}` already exists",
            destination.display()
        )));
    }
    let aggregate_bytes = read_regular_bounded(
        aggregate_manifest,
        MAX_AGGREGATE_MANIFEST_BYTES,
        "aggregate tmux manifest",
    )?;
    let aggregate = parse_aggregate(aggregate_manifest, &aggregate_bytes)?;
    validate_aggregate(&aggregate)?;
    let source_tracks = source_tracks(aggregate_manifest, &aggregate)?;
    validate_destination(destination, aggregate_manifest, &source_tracks)?;

    let parent = destination
        .parent()
        .ok_or_else(|| return Error::invalid("tmux evidence export destination has no parent"))?;
    let staging = parent.join(format!(".tmux-export-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let cleanup = DirectoryCleanup::new(staging.clone());
    write_secure(
        &staging.join("aggregate-recording-manifest.json"),
        &aggregate_bytes,
    )?;
    if let Some(boundary) = &aggregate.lifecycle_journal {
        let source_directory = aggregate_manifest
            .parent()
            .ok_or_else(|| return Error::invalid("tmux manifest has no parent"))?;
        tmux_journal::copy_committed(
            &source_directory.join(tmux_journal::JOURNAL_NAME),
            &staging.join("aggregate-lifecycle.jsonl"),
            boundary,
        )?;
    }
    create_private_directory(&staging.join("tracks"))?;

    let mut tracks: Vec<TmuxBundleTrack> = Vec::with_capacity(source_tracks.len());
    for (index, source) in source_tracks.iter().enumerate() {
        let directory = track_directory(index, source.pane_id.as_deref());
        let nested = bundle::create(
            &source.session,
            &staging.join(&directory),
            BundleProfile::Forensic,
            None,
        )?;
        tracks.push(TmuxBundleTrack {
            role: source.role.to_owned(),
            pane_id: source.pane_id.clone(),
            session_id: nested.session_id,
            canonical_session: ByteString::from(source.session.as_os_str().as_bytes()),
            directory,
            source_final_event_hash: nested.source_final_event_hash,
        });
    }

    ensure_bounded_tree(&staging, MAX_AGGREGATE_BUNDLE_ENTRIES)?;
    let members = collect_artifact_members(&staging)?;
    let manifest = TmuxBundleManifest {
        format: TMUX_BUNDLE_FORMAT.to_owned(),
        version: TMUX_BUNDLE_VERSION,
        recording_id: aggregate.recording_id,
        recording_name: aggregate.name.clone(),
        aggregate_manifest: ByteString::from(aggregate_manifest.as_os_str().as_bytes()),
        aggregate_manifest_blake3: blake3::hash(&aggregate_bytes).to_hex().to_string(),
        tracks,
        members,
    };
    let manifest_path = staging.join("manifest.json");
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|source| return Error::json(&manifest_path, source))?;
    manifest_bytes.push(b'\n');
    if u64::try_from(manifest_bytes.len()).unwrap_or(u64::MAX) > MAX_TMUX_BUNDLE_MANIFEST_BYTES {
        return Err(Error::invalid(format!(
            "tmux evidence export manifest exceeds {MAX_TMUX_BUNDLE_MANIFEST_BYTES} bytes"
        )));
    }
    write_secure(&manifest_path, &manifest_bytes)?;
    verify_contents(
        &staging,
        aggregate_manifest,
        &aggregate_bytes,
        &aggregate,
        &source_tracks,
    )?;
    sync_directory(&staging)?;
    commit_directory(parent, &staging, destination)?;
    cleanup.disarm();
    return Ok(manifest);
}

/// Verify a composite tmux evidence export against its caller-owned aggregate manifest.
///
/// # Errors
///
/// Returns an error when the aggregate relationship, root inventory, any nested
/// forensic export, or any source-bound identity does not match.
pub fn verify(
    directory: impl AsRef<Path>,
    aggregate_manifest: impl AsRef<Path>,
) -> Result<TmuxBundleManifest> {
    let directory = directory.as_ref();
    let aggregate_manifest = aggregate_manifest.as_ref();
    let aggregate_bytes = read_regular_bounded(
        aggregate_manifest,
        MAX_AGGREGATE_MANIFEST_BYTES,
        "aggregate tmux manifest",
    )?;
    let aggregate = parse_aggregate(aggregate_manifest, &aggregate_bytes)?;
    validate_aggregate(&aggregate)?;
    let source_tracks = source_tracks(aggregate_manifest, &aggregate)?;
    return verify_contents(
        directory,
        aggregate_manifest,
        &aggregate_bytes,
        &aggregate,
        &source_tracks,
    );
}

struct SourceTrack {
    role: &'static str,
    pane_id: Option<String>,
    session: PathBuf,
    session_id: Uuid,
}

fn parse_aggregate(path: &Path, bytes: &[u8]) -> Result<TmuxRecordingManifest> {
    return serde_json::from_slice(bytes).map_err(|source| return Error::json(path, source));
}

fn source_tracks(
    aggregate_manifest: &Path,
    aggregate: &TmuxRecordingManifest,
) -> Result<Vec<SourceTrack>> {
    let mut tracks = Vec::new();
    visit_source_tracks(aggregate_manifest, aggregate, |track| {
        if tracks.len() >= MAX_AGGREGATE_TRACKS {
            return Err(Error::invalid(
                "aggregate export exceeds its materialized track budget; use streaming export",
            ));
        }
        tracks.push(track);
        return Ok(());
    })?;
    return Ok(tracks);
}

fn visit_source_tracks(
    aggregate_manifest: &Path,
    aggregate: &TmuxRecordingManifest,
    mut visit: impl FnMut(SourceTrack) -> Result<()>,
) -> Result<()> {
    let composite = aggregate.composite_track.session.as_ref().ok_or_else(|| {
        return Error::invalid("aggregate tmux recording has no linked composite track");
    })?;
    visit(SourceTrack {
        role: "composite",
        pane_id: None,
        session: path_from_bytes(composite),
        session_id: aggregate
            .composite_track
            .session_id
            .ok_or_else(|| return Error::invalid("composite track has no identity"))?,
    })?;
    let mut current = aggregate
        .pane_tracks
        .iter()
        .map(|(pane, session)| return (pane.clone(), path_from_bytes(session)))
        .collect::<BTreeSet<_>>();
    if aggregate.catalog.is_some() {
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
    } else {
        for (identity, track) in &aggregate.track_history {
            if identity != &track.session_id {
                return Err(Error::invalid(
                    "historical pane track identity differs from its inventory key",
                ));
            }
            current.remove(&(track.pane_id.clone(), path_from_bytes(&track.session)));
            visit(SourceTrack {
                role: "pane",
                pane_id: Some(track.pane_id.clone()),
                session: path_from_bytes(&track.session),
                session_id: track.session_id,
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
    if !matches!(aggregate.lifecycle, TmuxRecordingLifecycle::Completed)
        || !matches!(
            aggregate.composite_track.lifecycle,
            TmuxRecordingLifecycle::Completed
        )
    {
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
    let parent = destination
        .parent()
        .ok_or_else(|| return Error::invalid("tmux evidence export destination has no parent"))?;
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

fn verify_contents(
    directory: &Path,
    aggregate_manifest: &Path,
    aggregate_bytes: &[u8],
    aggregate: &TmuxRecordingManifest,
    source_tracks: &[SourceTrack],
) -> Result<TmuxBundleManifest> {
    ensure_bounded_tree(directory, MAX_AGGREGATE_BUNDLE_ENTRIES)?;
    if let Some(boundary) = &aggregate.lifecycle_journal {
        tmux_journal::visit(
            &directory.join("aggregate-lifecycle.jsonl"),
            boundary,
            |_| return Ok(()),
        )?;
    }
    let manifest_path = directory.join("manifest.json");
    let manifest_bytes = read_regular_bounded(
        &manifest_path,
        MAX_TMUX_BUNDLE_MANIFEST_BYTES,
        "tmux evidence export manifest",
    )?;
    let manifest: TmuxBundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|source| return Error::json(&manifest_path, source))?;
    if manifest.format != TMUX_BUNDLE_FORMAT || manifest.version != TMUX_BUNDLE_VERSION {
        return Err(Error::invalid(
            "unsupported tmux evidence export manifest format or version",
        ));
    }
    if manifest.recording_id != aggregate.recording_id
        || manifest.recording_name != aggregate.name
        || manifest.aggregate_manifest
            != ByteString::from(aggregate_manifest.as_os_str().as_bytes())
        || manifest.aggregate_manifest_blake3 != blake3::hash(aggregate_bytes).to_hex().to_string()
    {
        return Err(Error::invalid(
            "tmux evidence export does not bind the requested aggregate recording",
        ));
    }
    if manifest.tracks.len() != source_tracks.len() {
        return Err(Error::invalid(
            "tmux evidence export track inventory does not match the aggregate recording",
        ));
    }
    let stored_aggregate = read_regular_bounded(
        &directory.join("aggregate-recording-manifest.json"),
        MAX_AGGREGATE_MANIFEST_BYTES,
        "embedded aggregate tmux manifest",
    )?;
    if stored_aggregate != aggregate_bytes {
        return Err(Error::invalid(
            "embedded aggregate tmux manifest differs from the trusted source",
        ));
    }
    verify_artifact_members(directory, &manifest.members)?;
    for (index, (track, source)) in manifest.tracks.iter().zip(source_tracks.iter()).enumerate() {
        let expected_directory = track_directory(index, source.pane_id.as_deref());
        if track.role != source.role
            || track.pane_id != source.pane_id
            || track.directory != expected_directory
            || track.canonical_session != ByteString::from(source.session.as_os_str().as_bytes())
        {
            return Err(Error::invalid(format!(
                "tmux evidence export track {index} does not match its aggregate relationship"
            )));
        }
        let policy = BundleVerificationPolicy::new(&source.session, BundleProfile::Forensic);
        let nested = bundle::verify_profile(&directory.join(&track.directory), &policy)?;
        if track.session_id != source.session_id
            || track.session_id != nested.session_id
            || track.source_final_event_hash != nested.source_final_event_hash
        {
            return Err(Error::invalid(format!(
                "tmux evidence export track {index} does not match its nested forensic export"
            )));
        }
    }
    return Ok(manifest);
}

fn ensure_bounded_tree(root: &Path, maximum: usize) -> Result<()> {
    let mut entries = 0_usize;
    count_tree_entries(root, root, &mut entries, maximum)?;
    return Ok(());
}

fn count_tree_entries(
    root: &Path,
    directory: &Path,
    entries: &mut usize,
    maximum: usize,
) -> Result<()> {
    for entry in
        fs_err::read_dir(directory).map_err(|source| return Error::filesystem(directory, source))?
    {
        let entry = entry.map_err(|source| return Error::filesystem(directory, source))?;
        if directory == root && entry.file_name() == "manifest.json" {
            continue;
        }
        *entries = entries.checked_add(1).ok_or_else(|| {
            return Error::invalid("tmux evidence export tree entry count overflow");
        })?;
        if *entries > maximum {
            return Err(Error::invalid(format!(
                "tmux evidence export exceeds {maximum} tree entries"
            )));
        }
        let path = entry.path();
        if !path.starts_with(root) {
            return Err(Error::invalid(
                "tmux evidence export traversal escaped its root",
            ));
        }
        let file_type = entry
            .file_type()
            .map_err(|source| return Error::filesystem(&path, source))?;
        if file_type.is_dir() {
            count_tree_entries(root, &path, entries, maximum)?;
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
