//! Complete, atomically published evidence bundles for aggregate tmux recordings.

use std::{
    ffi::OsString,
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::FileTypeExt as _,
    },
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, collect_artifact_members_bounded, commit_directory,
        create_private_directory, sync_directory, verify_artifact_members, write_secure,
    },
    bundle::{BundleProfile, create as create_bundle},
    event::ByteString,
    tmux_recording::{
        TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION, TmuxRecordingLifecycle,
        TmuxRecordingManifest,
    },
};

/// Stable aggregate tmux bundle format.
pub const TMUX_BUNDLE_FORMAT: &str = "termtrace-tmux-bundle";
/// Current aggregate tmux bundle schema version.
pub const TMUX_BUNDLE_VERSION: u32 = 1;

const MAX_AGGREGATE_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_AGGREGATE_TRACKS: usize = 1_024;
const MAX_AGGREGATE_BUNDLE_ENTRIES: usize = 131_072;

/// One complete canonical track embedded in an aggregate tmux bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxBundleTrack {
    /// `composite` or `pane`.
    pub role: String,
    /// Stable tmux pane ID for a pane track.
    pub pane_id: Option<String>,
    /// Immutable canonical Termtrace session identity.
    pub session_id: Uuid,
    /// Exact source session path bytes.
    pub canonical_session: ByteString,
    /// Relative directory containing a forensic Termtrace bundle.
    pub directory: String,
    /// Final canonical event hash bound by the nested forensic bundle.
    pub source_final_event_hash: String,
}

/// Whole-file inventory for a complete aggregate tmux evidence bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TmuxBundleManifest {
    /// Stable format identifier.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Immutable aggregate recording identity.
    pub recording_id: Uuid,
    /// Friendly aggregate recording name.
    pub recording_name: String,
    /// Exact source aggregate manifest path bytes.
    pub aggregate_manifest: ByteString,
    /// BLAKE3 of the complete source aggregate manifest.
    pub aggregate_manifest_blake3: String,
    /// Composite track followed by pane tracks in stable pane-ID order.
    pub tracks: Vec<TmuxBundleTrack>,
    /// Whole-file inventory excluding this root manifest.
    pub members: Vec<ArtifactMember>,
}

/// Publish one complete aggregate tmux recording without replacing an existing destination.
///
/// # Errors
///
/// Returns an error unless the aggregate and every linked track are complete, securely readable,
/// and can be atomically published under the destination parent.
pub fn create_tmux_bundle(
    aggregate_manifest: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<TmuxBundleManifest> {
    let aggregate_manifest = aggregate_manifest.as_ref();
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let aggregate_bytes = read_aggregate_manifest(aggregate_manifest)?;
    let aggregate: TmuxRecordingManifest =
        serde_json::from_slice(&aggregate_bytes).map_err(|source| {
            return Error::InvalidJson {
                path: aggregate_manifest.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
    validate_aggregate(&aggregate)?;
    let source_tracks = source_tracks(&aggregate)?;
    validate_destination(destination, aggregate_manifest, &source_tracks)?;

    let parent = destination.parent().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "tmux bundle destination has no parent".to_owned(),
        };
    })?;
    let staging = parent.join(format!(".tmux-bundle-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let _cleanup = DirectoryCleanup(staging.clone());
    write_secure(
        &staging.join("aggregate-recording-manifest.json"),
        &aggregate_bytes,
    )?;
    let tracks_directory = staging.join("tracks");
    create_private_directory(&tracks_directory)?;

    let mut tracks: Vec<TmuxBundleTrack> = Vec::with_capacity(source_tracks.len());
    for (index, source) in source_tracks.iter().enumerate() {
        let directory = if source.pane_id.is_some() {
            format!("tracks/pane-{index:04}")
        } else {
            "tracks/composite".to_owned()
        };
        let nested = create_bundle(
            &source.session,
            &staging.join(&directory),
            BundleProfile::Forensic,
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

    let members = collect_artifact_members_bounded(&staging, MAX_AGGREGATE_BUNDLE_ENTRIES)?;
    let manifest = TmuxBundleManifest {
        format: TMUX_BUNDLE_FORMAT.to_owned(),
        version: TMUX_BUNDLE_VERSION,
        recording_id: aggregate.recording_id,
        recording_name: aggregate.name,
        aggregate_manifest: ByteString::from(aggregate_manifest.as_os_str().as_bytes()),
        aggregate_manifest_blake3: blake3::hash(&aggregate_bytes).to_hex().to_string(),
        tracks,
        members,
    };
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("manifest.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    manifest_bytes.push(b'\n');
    write_secure(&staging.join("manifest.json"), &manifest_bytes)?;
    verify_artifact_members(&staging, &manifest.members)?;
    sync_directory(&staging)?;
    commit_directory(parent, &staging, destination)?;
    return Ok(manifest);
}

struct SourceTrack {
    role: &'static str,
    pane_id: Option<String>,
    session: PathBuf,
}

fn source_tracks(aggregate: &TmuxRecordingManifest) -> Result<Vec<SourceTrack>> {
    let composite = aggregate.composite_track.session.as_ref().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "aggregate tmux recording has no linked composite track".to_owned(),
        };
    })?;
    if aggregate.pane_tracks.len().saturating_add(1) > MAX_AGGREGATE_TRACKS {
        return Err(Error::InvalidReplay {
            reason: format!(
                "aggregate tmux recording exceeds {MAX_AGGREGATE_TRACKS} linked tracks"
            ),
        });
    }
    let mut tracks: Vec<SourceTrack> = Vec::with_capacity(aggregate.pane_tracks.len() + 1);
    tracks.push(SourceTrack {
        role: "composite",
        pane_id: None,
        session: path_from_bytes(composite),
    });
    for (pane_id, session) in &aggregate.pane_tracks {
        tracks.push(SourceTrack {
            role: "pane",
            pane_id: Some(pane_id.clone()),
            session: path_from_bytes(session),
        });
    }
    return Ok(tracks);
}

fn path_from_bytes(bytes: &ByteString) -> PathBuf {
    return PathBuf::from(OsString::from_vec(bytes.as_bytes().to_vec()));
}

fn validate_aggregate(aggregate: &TmuxRecordingManifest) -> Result<()> {
    if aggregate.format != TMUX_RECORDING_FORMAT || aggregate.version != TMUX_RECORDING_VERSION {
        return Err(Error::InvalidReplay {
            reason: "unsupported aggregate tmux recording manifest".to_owned(),
        });
    }
    if !matches!(aggregate.lifecycle, TmuxRecordingLifecycle::Completed)
        || !matches!(
            aggregate.composite_track.lifecycle,
            TmuxRecordingLifecycle::Completed
        )
    {
        return Err(Error::InvalidReplay {
            reason: "aggregate tmux recording must be completed before bundling".to_owned(),
        });
    }
    return Ok(());
}

fn validate_destination(
    destination: &Path,
    aggregate_manifest: &Path,
    tracks: &[SourceTrack],
) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "tmux bundle destination has no parent".to_owned(),
        };
    })?;
    let parent = parent.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    let name = destination.file_name().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "tmux bundle destination has no basename".to_owned(),
        };
    })?;
    let candidate = parent.join(name);
    let aggregate_root = aggregate_manifest
        .parent()
        .and_then(|path| return path.canonicalize().ok());
    if aggregate_root
        .as_ref()
        .is_some_and(|root| return candidate.starts_with(root))
    {
        return Err(Error::InvalidReplay {
            reason: "tmux bundle destination must be outside the aggregate recording".to_owned(),
        });
    }
    for track in tracks {
        let canonical = track.session.canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: track.session.clone(),
                source,
            };
        })?;
        if candidate.starts_with(&canonical) {
            return Err(Error::InvalidReplay {
                reason: "tmux bundle destination must be outside every canonical track".to_owned(),
            });
        }
    }
    return Ok(());
}

fn read_aggregate_manifest(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs_err::symlink_metadata(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let file_type = metadata.file_type();
    if file_type.is_symlink()
        || file_type.is_dir()
        || file_type.is_block_device()
        || file_type.is_char_device()
        || file_type.is_fifo()
        || file_type.is_socket()
    {
        return Err(Error::InvalidReplay {
            reason: "aggregate tmux manifest must be a regular file".to_owned(),
        });
    }
    if metadata.len() > MAX_AGGREGATE_MANIFEST_BYTES {
        return Err(Error::InvalidReplay {
            reason: format!("aggregate tmux manifest exceeds {MAX_AGGREGATE_MANIFEST_BYTES} bytes"),
        });
    }
    return fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

struct DirectoryCleanup(PathBuf);

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _result = fs_err::remove_dir_all(&self.0);
    }
}
