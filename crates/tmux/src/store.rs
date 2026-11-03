//! The aggregate recording directory: snapshot, journal, catalog, and their lock.

use crate::{
    artifact::{
        TmuxEvent, TmuxEventRecord, TmuxPaneTrack, TmuxRecordingManifest, TmuxRecordingState,
        TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION,
    },
    catalog::TmuxCatalog,
    error::{Error, Result},
    journal::{self, JOURNAL_NAME},
    reducer::{self, Input},
};
use std::{
    ffi::OsStr,
    fs::{OpenOptions, Permissions},
    io::{self, Write as _},
    ops::ControlFlow,
    os::unix::{
        ffi::OsStrExt as _,
        fs::{OpenOptionsExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
};
use termtrace_evidence::{artifact::sync_directory, session::read_manifest};
use termtrace_model::value::ByteString;
use termtrace_process::ExclusiveLock;

/// Directory under the state root holding every aggregate recording.
pub const RECORDINGS_DIRECTORY: &str = "tmux-recordings";
/// Snapshot filename within one aggregate recording directory.
pub const MANIFEST_NAME: &str = "manifest.json";
/// Generated tmux configuration for recordings that start their own server.
pub const CONFIG_NAME: &str = "tmux.conf";
const LOCK_NAME: &str = ".manifest.lock";
const MAX_EVENT_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// One aggregate recording directory.
#[derive(Clone, Debug)]
pub struct RecordingStore {
    directory: PathBuf,
}

/// A bounded page of lifecycle history.
#[derive(Clone, Debug, serde::Serialize, schemars::JsonSchema)]
pub struct EventPage {
    /// Events in sequence order.
    pub events: Vec<TmuxEventRecord>,
    /// First sequence of the following page, if any.
    pub next_sequence: Option<u64>,
}

impl RecordingStore {
    /// Locate a named recording under a state root.
    ///
    /// # Errors
    /// Returns an invalid recording name.
    pub fn locate(state_root: &Path, name: &str) -> Result<Self> {
        validate_name(name)?;

        return Ok(Self {
            directory: state_root.join(RECORDINGS_DIRECTORY).join(name),
        });
    }

    /// The recording whose snapshot is at `manifest`.
    ///
    /// # Errors
    /// Returns a path without a parent directory.
    pub fn at(manifest: &Path) -> Result<Self> {
        let directory = manifest
            .parent()
            .ok_or_else(|| return Error::invalid("tmux manifest has no parent directory"))?;

        return Ok(Self {
            directory: directory.to_path_buf(),
        });
    }

    /// Claim a new recording name by creating its private directory.
    ///
    /// # Errors
    /// Returns [`Error::Exists`] when the name is taken.
    pub fn create(state_root: &Path, name: &str) -> Result<Self> {
        let store = Self::locate(state_root, name)?;
        let parent = state_root.join(RECORDINGS_DIRECTORY);

        fs_err::create_dir_all(&parent)
            .map_err(|error| return Error::filesystem(&parent, error))?;

        set_private(&parent)?;

        if let Err(error) = fs_err::create_dir(&store.directory) {
            if error.kind() == io::ErrorKind::AlreadyExists {
                return Err(Error::Exists(name.to_owned()));
            }
            return Err(Error::filesystem(&store.directory, error));
        }

        set_private(&store.directory)?;

        return Ok(store);
    }

    /// Delete a recording that never started, releasing its name.
    ///
    /// # Errors
    /// Returns filesystem failures.
    pub fn release(self) -> Result<()> {
        return fs_err::remove_dir_all(&self.directory)
            .map_err(|error| return Error::filesystem(&self.directory, error));
    }

    /// Recording directory.
    pub fn directory(&self) -> &Path {
        return &self.directory;
    }

    /// Snapshot path.
    pub fn manifest_path(&self) -> PathBuf {
        return self.directory.join(MANIFEST_NAME);
    }

    /// Write a private file beside the snapshot.
    ///
    /// # Errors
    /// Returns filesystem failures, including an existing file.
    pub fn write_private(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.directory.join(name);
        write_private_file(&path, bytes)?;

        return Ok(path);
    }

    /// Read the current snapshot.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`], invalid JSON, or an unsupported format.
    pub fn read(&self) -> Result<TmuxRecordingManifest> {
        let path = self.manifest_path();
        if !path.is_file() {
            return Err(Error::NotFound(self.name()));
        }

        let bytes = fs_err::read(&path).map_err(|error| return Error::filesystem(&path, error))?;
        let manifest: TmuxRecordingManifest =
            serde_json::from_slice(&bytes).map_err(|error| return Error::json(&path, error))?;

        if manifest.format != TMUX_RECORDING_FORMAT || manifest.version != TMUX_RECORDING_VERSION {
            return Err(Error::invalid(
                "unsupported aggregate tmux recording manifest",
            ));
        }
        return Ok(manifest);
    }

    /// Publish the first snapshot of a newly created recording.
    ///
    /// # Errors
    /// Returns journal, catalog, or publication failures.
    pub fn initialize(
        &self,
        manifest: TmuxRecordingManifest,
        events: Vec<TmuxEvent>,
        now: u64,
    ) -> Result<()> {
        let mut locked = LockedRecording {
            store: self,
            _lock: self.acquire()?,
            manifest,
        };
        let state = locked.manifest.state.clone();

        return locked.commit(state, events, now);
    }

    /// Hold the recording's writer lock and read its snapshot.
    ///
    /// # Errors
    /// Returns [`Error::NotFound`] or lock and read failures.
    pub fn lock(&self) -> Result<LockedRecording<'_>> {
        let lock = self.acquire()?;
        let manifest = self.read()?;

        return Ok(LockedRecording {
            store: self,
            _lock: lock,
            manifest,
        });
    }

    /// Open a read view over the complete history committed by `manifest`.
    ///
    /// # Errors
    /// Returns catalog or journal failures.
    pub fn catalog(&self, manifest: &TmuxRecordingManifest) -> Result<TmuxCatalog> {
        return TmuxCatalog::read(&self.directory, manifest);
    }

    /// Canonical journal path.
    pub fn journal_path(&self) -> PathBuf {
        return self.directory.join(JOURNAL_NAME);
    }

    /// Read at most `limit` events from `from_sequence`, within four MiB.
    ///
    /// # Errors
    /// Returns catalog, journal, or encoding failures.
    pub fn events(
        &self,
        manifest: &TmuxRecordingManifest,
        from_sequence: u64,
        limit: u16,
    ) -> Result<EventPage> {
        let catalog = self.catalog(manifest)?;
        let mut page = EventPage {
            events: Vec::new(),
            next_sequence: None,
        };

        let mut bytes = 0_usize;
        catalog.visit_events(from_sequence, limit, |event| {
            let encoded = serde_json::to_vec(&event)
                .map_err(|error| return Error::json(self.journal_path(), error))?;
            if bytes.saturating_add(encoded.len()) > MAX_EVENT_PAGE_BYTES {
                page.next_sequence = Some(event.sequence);

                return Ok(ControlFlow::Break(()));
            }

            bytes = bytes.saturating_add(encoded.len());
            page.events.push(event);

            return Ok(ControlFlow::Continue(()));
        })?;

        if page.next_sequence.is_none() && page.events.len() == usize::from(limit) {
            let following = from_sequence.max(1).saturating_add(u64::from(limit));
            if following <= manifest.lifecycle_journal.sequence {
                page.next_sequence = Some(following);
            }
        }

        return Ok(page);
    }

    /// Resolve a published pane recording to its immutable identity.
    ///
    /// A recording already in the catalog keeps its first link time and may not move to
    /// another pane or path.
    ///
    /// # Errors
    /// Returns unreadable session evidence or a rebound identity.
    pub fn resolve_track(
        &self,
        manifest: &TmuxRecordingManifest,
        pane_id: &str,
        session: &ByteString,
        now: u64,
    ) -> Result<TmuxPaneTrack> {
        let canonical = read_manifest(&path_from_bytes(session))?;
        let mut track = TmuxPaneTrack {
            pane_id: pane_id.to_owned(),
            session_id: canonical.session_id,
            session: session.clone(),
            linked_unix_ns: now,
        };

        if let Some(existing) = self.catalog(manifest)?.track(canonical.session_id)? {
            let existing = existing.track;
            if existing.pane_id != track.pane_id || existing.session != track.session {
                return Err(Error::invalid(
                    "tmux recording identity was rebound to another pane or path",
                ));
            }

            track.linked_unix_ns = existing.linked_unix_ns;
        }

        return Ok(track);
    }

    fn acquire(&self) -> Result<ExclusiveLock> {
        if !self.directory.is_dir() {
            return Err(Error::NotFound(self.name()));
        }

        let path = self.directory.join(LOCK_NAME);

        return ExclusiveLock::acquire(&path)
            .map_err(|error| return Error::filesystem(&path, error));
    }

    fn name(&self) -> String {
        return self.directory.file_name().map_or_else(
            || return "unknown".to_owned(),
            |name| return name.to_string_lossy().into_owned(),
        );
    }
}

/// A recording snapshot held under its writer lock.
pub struct LockedRecording<'store> {
    store: &'store RecordingStore,
    _lock: ExclusiveLock,
    manifest: TmuxRecordingManifest,
}

impl LockedRecording<'_> {
    /// Snapshot as last committed.
    pub fn manifest(&self) -> &TmuxRecordingManifest {
        return &self.manifest;
    }

    /// Consume the lock, returning the last committed snapshot.
    pub fn into_manifest(self) -> TmuxRecordingManifest {
        return self.manifest;
    }

    /// Reduce `input` at `now` and commit the resulting state and events.
    ///
    /// # Errors
    /// Returns reducer rejections or commit failures; nothing is committed on error.
    pub fn apply(&mut self, input: Input, now: u64) -> Result<()> {
        let step = reducer::reduce(&self.manifest.name, &self.manifest.state, input, now)?;

        return self.commit(step.state, step.events, now);
    }

    /// Append `events` to the journal and catalog, then atomically publish `state`.
    ///
    /// An uncommitted journal suffix left by an interrupted publication is discarded.
    ///
    /// # Errors
    /// Returns journal, catalog, or publication failures.
    pub fn commit(
        &mut self,
        state: TmuxRecordingState,
        events: Vec<TmuxEvent>,
        now: u64,
    ) -> Result<()> {
        let directory = self.store.directory();
        let mut next = self.manifest.clone();
        next.state = state;

        let mut catalog = if events.is_empty() {
            None
        } else {
            Some(TmuxCatalog::for_append(directory, &self.manifest)?)
        };

        let (written, boundary) = journal::append(
            &self.store.journal_path(),
            &self.manifest.lifecycle_journal,
            events,
            now,
        )?;

        if let Some(catalog) = catalog.as_mut() {
            next.catalog = catalog.append(&written, &boundary)?;
        }

        drop(catalog);
        next.lifecycle_journal = boundary;

        let path = self.store.manifest_path();
        let mut bytes =
            serde_json::to_vec_pretty(&next).map_err(|error| return Error::json(&path, error))?;
        bytes.push(b'\n');

        let temporary = path.with_extension(format!("json.{}.tmp", uuid::Uuid::now_v7()));
        write_private_file(&temporary, &bytes)?;
        fs_err::rename(&temporary, &path)
            .map_err(|error| return Error::filesystem(&path, error))?;

        sync_directory(directory)?;

        self.manifest = next;

        return Ok(());
    }
}

/// Reject names that are not 1-40 ASCII letters, digits, `-` or `_`, starting alphanumeric.
///
/// # Errors
/// Returns the naming rule.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 40
        || !name
            .bytes()
            .all(|byte| return byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(Error::invalid(
            "tmux recording name must be 1-40 ASCII letters, digits, '-' or '_', beginning with a letter or digit",
        ));
    }
    return Ok(());
}

pub(crate) fn path_from_bytes(bytes: &ByteString) -> PathBuf {
    return PathBuf::from(OsStr::from_bytes(bytes.as_bytes()));
}

fn set_private(path: &Path) -> Result<()> {
    return fs_err::set_permissions(path, Permissions::from_mode(0o700))
        .map_err(|error| return Error::filesystem(path, error));
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| return Error::filesystem(path, error))?;
    file.write_all(bytes)
        .map_err(|error| return Error::filesystem(path, error))?;
    file.sync_all()
        .map_err(|error| return Error::filesystem(path, error))?;

    return Ok(());
}
