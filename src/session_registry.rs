//! Owner-only named-session registry backed by canonical session identity.

use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions, Permissions},
    io::{ErrorKind, Read as _},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    os::unix::fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use nix::{
    fcntl::{Flock, FlockArg},
    libc::O_NOFOLLOW,
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::getuid,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{sync_directory, sync_parent_directory, write_secure},
    control::{CONTROL_PROTOCOL_VERSION, connect_session_control},
    event::ByteString,
    event_log::EventLog,
    session::{
        SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION, SessionGeometry, read_completion,
        read_manifest, validate_completed_log,
    },
};

/// Registry-entry format identifier.
pub const REGISTRY_ENTRY_FORMAT: &str = "termtrace-session-registry-entry";
/// Registry-entry schema version.
pub const REGISTRY_ENTRY_VERSION: u32 = 1;
const REGISTRY_DIRECTORY: &str = "registry";
const REGISTRY_RESERVATION_FORMAT: &str = "termtrace-session-registry-reservation";
const REGISTRY_RESERVATION_VERSION: u32 = 1;

/// Exclusive, leased ownership of an alias while detached startup is pending.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AliasReservation {
    /// Stable reservation format identifier.
    pub format: String,
    /// Reservation schema version.
    pub version: u32,
    /// Validated friendly alias.
    pub alias: String,
    /// Unforgeable token required to promote or release this reservation.
    pub owner_token: Uuid,
    /// Reservation creation time from the local system clock.
    pub created_unix_ns: u128,
    /// Time after which another owner may reclaim the alias.
    pub lease_expires_unix_ns: u128,
    /// Exact path bytes allocated for the pending canonical session.
    pub pending_session: ByteString,
}

impl AliasReservation {
    /// Reconstruct the exact pending session path.
    pub fn session_path(&self) -> PathBuf {
        return PathBuf::from(OsString::from_vec(self.pending_session.as_bytes().to_vec()));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RegistryRecord {
    Entry(RegistryEntry),
    Reservation(AliasReservation),
}

/// One friendly name bound to an immutable canonical session identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    /// Stable entry format identifier.
    pub format: String,
    /// Entry schema version.
    pub version: u32,
    /// Validated friendly alias.
    pub alias: String,
    /// Exact canonical session-directory path bytes.
    pub canonical_session: ByteString,
    /// Immutable session ID read from `session.json` during registration.
    pub session_id: Uuid,
    /// Recorder PID when a supervisor supplied one.
    pub recorder_pid: Option<u32>,
    /// Linux process start-time ticks paired with `recorder_pid` against PID reuse.
    pub recorder_process_start_ticks: Option<u64>,
    /// Control protocol expected by the registering build.
    pub control_protocol_version: u32,
    /// Registry publication time from the local system clock.
    pub registered_unix_ns: u128,
}

/// Lifecycle classification derived without changing canonical evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryLifecycle {
    /// Recorder identity and control socket are present.
    Live,
    /// A completion manifest exists for the registered session ID.
    Completed,
    /// Evidence exists but has neither a live recorder identity nor completion.
    Incomplete,
    /// Socket or PID metadata remains but the recorder identity no longer matches.
    Stale,
    /// Canonical evidence is missing, insecure, or unreadable.
    MissingEvidence,
    /// The manifest session ID differs from the registered immutable identity.
    IdentityMismatch,
    /// Registry or control protocol format is unsupported.
    Incompatible,
}

impl RegistryLifecycle {
    /// Stable lowercase CLI and diagnostic spelling.
    pub fn as_str(self) -> &'static str {
        return match self {
            Self::Live => "live",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Stale => "stale",
            Self::MissingEvidence => "missing_evidence",
            Self::IdentityMismatch => "identity_mismatch",
            Self::Incompatible => "incompatible",
        };
    }
}

/// One registry listing row with evidence-derived session summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryListEntry {
    /// Friendly alias.
    pub alias: String,
    /// Exact canonical session path bytes.
    pub canonical_session: ByteString,
    /// Immutable registered session ID.
    pub session_id: Uuid,
    /// Effective command bytes when the manifest is readable.
    pub command: Option<Vec<ByteString>>,
    /// Effective working directory when the manifest is readable.
    pub working_directory: Option<ByteString>,
    /// Initial grid and pixel geometry when the manifest is readable.
    pub geometry: Option<SessionGeometry>,
    /// Session wall-clock origin when the manifest is readable.
    pub started_unix_ns: Option<u128>,
    /// Registry publication time.
    pub registered_unix_ns: u128,
    /// Current derived lifecycle classification.
    pub lifecycle: RegistryLifecycle,
    /// Focused diagnostic for unhealthy entries.
    pub diagnostic: Option<String>,
}

impl RegistryEntry {
    /// Reconstruct the exact canonical session path.
    pub fn session_path(&self) -> PathBuf {
        return PathBuf::from(OsString::from_vec(
            self.canonical_session.as_bytes().to_vec(),
        ));
    }
}

/// A named session path paired with the registry's evidence-derived lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrySessionResolution {
    /// Exact registered or pending session path.
    pub session: PathBuf,
    /// Current lifecycle classification.
    pub lifecycle: RegistryLifecycle,
    /// Focused explanation for incomplete or unhealthy states.
    pub diagnostic: Option<String>,
}

/// Secure storage boundary for named session aliases.
#[derive(Clone, Debug)]
pub struct SessionRegistry {
    state_root: PathBuf,
    directory: PathBuf,
}

/// Collision-free paths allocated for one detached recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetachedSessionPaths {
    /// New canonical session path, not yet created.
    pub session: PathBuf,
    /// Owner-only mirrored PTY output log outside canonical evidence.
    pub stdout_log: PathBuf,
    /// Owner-only recorder diagnostic log outside canonical evidence.
    pub stderr_log: PathBuf,
}

/// One completed, unregistered state-root recording eligible for explicit deletion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GarbageCollectionCandidate {
    /// Canonical managed session directory.
    pub session: PathBuf,
    /// Immutable session identity.
    pub session_id: Uuid,
    /// Completion-file modification timestamp used for retention ordering.
    pub completed_unix_ns: u128,
    /// Complete bytes below the session directory.
    pub bytes: u64,
}

/// Safe state-root retention plan or applied deletion report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GarbageCollectionReport {
    /// Owner-only Termtrace state root considered.
    pub state_root: PathBuf,
    /// Whether eligible candidates were actually deleted.
    pub applied: bool,
    /// Minimum completed-recording age in whole days.
    pub older_than_days: u64,
    /// Newest completed, unregistered recordings protected regardless of age.
    pub keep_latest: u64,
    /// Completed managed recordings protected by a registry alias.
    pub protected_registered: u64,
    /// Live, incomplete, malformed, or non-directory entries never considered.
    pub skipped_non_completed: u64,
    /// Malformed or insecure registry records isolated from this scan.
    pub skipped_corrupt_registry_entries: u64,
    /// Exact eligible recordings, retained in the report even after deletion.
    pub candidates: Vec<GarbageCollectionCandidate>,
    /// Session directories removed when `applied` is true.
    pub removed_sessions: u64,
    /// Detached supervisor log files removed with deleted sessions.
    pub removed_supervisor_files: u64,
    /// Bytes reclaimed from canonical managed session directories.
    pub reclaimed_bytes: u64,
}

impl SessionRegistry {
    /// Open or securely initialize the default per-user registry.
    ///
    /// `TERMTRACE_STATE_DIR` overrides the base directory. Otherwise the base
    /// is `$XDG_STATE_HOME/termtrace`, falling back to `$HOME/.local/state/termtrace`.
    ///
    /// # Errors
    ///
    /// Returns an error when no state root can be selected or an existing root
    /// is not an owner-only real directory.
    pub fn open_default() -> Result<Self> {
        let root = default_state_root()?;
        return Self::open(root);
    }

    /// Open or securely initialize a registry beneath an explicit state root.
    ///
    /// # Errors
    ///
    /// Returns an error when directories cannot be created or do not have
    /// owner-only ownership and permissions.
    pub fn open(state_root: impl AsRef<Path>) -> Result<Self> {
        let state_root = state_root.as_ref();
        ensure_owner_directory(state_root)?;
        let directory = state_root.join(REGISTRY_DIRECTORY);
        ensure_owner_directory(&directory)?;
        return Ok(Self {
            state_root: state_root.to_path_buf(),
            directory,
        });
    }

    /// Return the secured state root used for managed session allocation.
    pub fn state_root(&self) -> &Path {
        return &self.state_root;
    }

    /// Register a new alias without replacing an existing binding or reservation.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid alias, insecure or unreadable session,
    /// schema/identity mismatch, or existing alias reservation/binding.
    pub fn register(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
        let _lock = self.lock()?;
        return self.register_locked(alias, session);
    }

    fn register_locked(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
        validate_alias(alias)?;
        if self.entry_path(alias).exists() {
            return Err(registry_error(
                "register session alias",
                "alias is already reserved or registered",
            ));
        }
        let entry = self.entry_for(alias, session)?;
        let encoded = self.encode_entry(&entry)?;
        write_secure(&self.entry_path(alias), &encoded)?;
        return Ok(entry);
    }

    /// Atomically reserve an alias for one pending detached session.
    ///
    /// Expired reservations are reclaimed while holding the registry lock.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid aliases, zero leases, or a live reservation/binding.
    pub fn reserve(
        &self,
        alias: &str,
        pending_session: &Path,
        lease: Duration,
    ) -> Result<AliasReservation> {
        validate_alias(alias)?;
        if lease.is_zero() {
            return Err(registry_error(
                "reserve session alias",
                "reservation lease must be greater than zero",
            ));
        }
        let _lock = self.lock()?;
        let now = system_time_unix_ns()?;
        let path = self.entry_path(alias);
        if path.exists() {
            match read_registry_record_nofollow(&path)? {
                RegistryRecord::Reservation(reservation)
                    if valid_reservation(alias, &reservation)
                        && reservation.lease_expires_unix_ns <= now =>
                {
                    fs_err::remove_file(&path).map_err(|source| {
                        return Error::Filesystem {
                            path: path.clone(),
                            source,
                        };
                    })?;
                    sync_parent_directory(&path)?;
                }
                RegistryRecord::Reservation(reservation)
                    if !valid_reservation(alias, &reservation) =>
                {
                    return Err(registry_error(
                        "reserve session alias",
                        "existing alias reservation is incompatible",
                    ));
                }
                RegistryRecord::Reservation(_) | RegistryRecord::Entry(_) => {
                    return Err(registry_error(
                        "reserve session alias",
                        "alias is already reserved or registered",
                    ));
                }
            }
        }
        let lease_expires_unix_ns = now.saturating_add(lease.as_nanos());
        let reservation = AliasReservation {
            format: REGISTRY_RESERVATION_FORMAT.to_owned(),
            version: REGISTRY_RESERVATION_VERSION,
            alias: alias.to_owned(),
            owner_token: Uuid::now_v7(),
            created_unix_ns: now,
            lease_expires_unix_ns,
            pending_session: ByteString::from(pending_session.as_os_str().as_bytes()),
        };
        let encoded = self.encode_reservation(&reservation)?;
        write_secure(&path, &encoded)?;
        return Ok(reservation);
    }

    /// Atomically replace an owned reservation with its validated session binding.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the reservation when ownership changed or
    /// pending session evidence is not ready for publication.
    pub fn promote(&self, reservation: &AliasReservation) -> Result<RegistryEntry> {
        let _lock = self.lock()?;
        let path = self.entry_path(&reservation.alias);
        let RegistryRecord::Reservation(current) = read_registry_record_nofollow(&path)? else {
            return Err(registry_error(
                "promote session alias",
                "alias is no longer reserved",
            ));
        };
        if !valid_reservation(&reservation.alias, &current)
            || current.owner_token != reservation.owner_token
            || current.created_unix_ns != reservation.created_unix_ns
            || current.lease_expires_unix_ns != reservation.lease_expires_unix_ns
            || current.pending_session != reservation.pending_session
        {
            return Err(registry_error(
                "promote session alias",
                "alias reservation owner changed before promotion",
            ));
        }
        if current.lease_expires_unix_ns <= system_time_unix_ns()? {
            return Err(registry_error(
                "promote session alias",
                "alias reservation lease expired before promotion",
            ));
        }
        let entry = self.entry_for(&reservation.alias, &reservation.session_path())?;
        let encoded = self.encode_entry(&entry)?;
        let temporary =
            self.directory
                .join(format!(".{}.{}.tmp", reservation.alias, Uuid::now_v7()));
        write_secure(&temporary, &encoded)?;
        if let Err(source) = fs_err::rename(&temporary, &path) {
            let _cleanup = fs_err::remove_file(&temporary);
            return Err(Error::Filesystem { path, source });
        }
        sync_parent_directory(&path)?;
        return Ok(entry);
    }

    /// Release a reservation only when its owner token still matches.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the alias when ownership changed.
    pub fn release(&self, reservation: &AliasReservation) -> Result<()> {
        let _lock = self.lock()?;
        let path = self.entry_path(&reservation.alias);
        let RegistryRecord::Reservation(current) = read_registry_record_nofollow(&path)? else {
            return Err(registry_error(
                "release session alias",
                "alias is no longer reserved",
            ));
        };
        if !valid_reservation(&reservation.alias, &current)
            || current.owner_token != reservation.owner_token
        {
            return Err(registry_error(
                "release session alias",
                "alias reservation owner changed before release",
            ));
        }
        fs_err::remove_file(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        sync_parent_directory(&path)?;
        return Ok(());
    }

    /// Atomically replace an alias only when it still names the expected predecessor.
    ///
    /// # Errors
    ///
    /// Returns an error without changing the binding when the expected identity no
    /// longer owns the alias, or when successor validation/publication fails.
    pub fn retarget(
        &self,
        alias: &str,
        expected_session_id: Uuid,
        successor: &Path,
    ) -> Result<RegistryEntry> {
        validate_alias(alias)?;
        let _lock = self.lock()?;
        let path = self.entry_path(alias);
        let current = read_entry_nofollow(&path)?;
        if current.alias != alias || current.session_id != expected_session_id {
            return Err(registry_error(
                "retarget session alias",
                "alias owner changed before successor publication",
            ));
        }
        let entry = self.entry_for(alias, successor)?;
        let encoded = self.encode_entry(&entry)?;
        let temporary = self
            .directory
            .join(format!(".{alias}.{}.tmp", Uuid::now_v7()));
        write_secure(&temporary, &encoded)?;
        if let Err(source) = fs_err::rename(&temporary, &path) {
            let _cleanup = fs_err::remove_file(&temporary);
            return Err(Error::Filesystem { path, source });
        }
        sync_parent_directory(&path)?;
        return Ok(entry);
    }

    /// Validate that an alias is well formed and currently unbound.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid or already-published aliases.
    pub fn ensure_available(&self, alias: &str) -> Result<()> {
        validate_alias(alias)?;
        let _lock = self.lock()?;
        let path = self.entry_path(alias);
        if path.exists() {
            return Err(registry_error(
                "reserve session alias",
                "alias is already reserved or registered",
            ));
        }
        return Ok(());
    }

    /// Allocate collision-free session and detached diagnostic paths.
    ///
    /// The canonical session path is intentionally left absent because the
    /// recorder publishes it with no-replace semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when owner-only state directories cannot be prepared.
    pub fn allocate_detached_paths(&self) -> Result<DetachedSessionPaths> {
        let sessions = self.state_root.join("sessions");
        let diagnostics = self.state_root.join("supervisor");
        ensure_owner_directory(&sessions)?;
        ensure_owner_directory(&diagnostics)?;
        loop {
            let identity = Uuid::now_v7();
            let session = sessions.join(format!("{identity}.termtrace"));
            let stdout_log = diagnostics.join(format!("{identity}.stdout"));
            let stderr_log = diagnostics.join(format!("{identity}.stderr"));
            if !session.exists() && !stdout_log.exists() && !stderr_log.exists() {
                return Ok(DetachedSessionPaths {
                    session,
                    stdout_log,
                    stderr_log,
                });
            }
        }
    }

    /// Plan or apply bounded retention for completed, unregistered managed recordings.
    ///
    /// Live/incomplete sessions, arbitrary external paths, and every session named by a
    /// registry alias are always preserved. Deletion occurs only when `apply` is true.
    ///
    /// # Errors
    ///
    /// Returns an error when the owner-only state root cannot be inspected or a requested
    /// deletion cannot be completed durably.
    pub fn garbage_collect(
        &self,
        older_than_days: u64,
        keep_latest: u64,
        apply: bool,
    ) -> Result<GarbageCollectionReport> {
        // Serialize alias changes with the protection scan and apply phase.
        let _lock = self.lock()?;
        let sessions = self.state_root.join("sessions");
        let mut report = GarbageCollectionReport {
            state_root: self.state_root.clone(),
            applied: apply,
            older_than_days,
            keep_latest,
            protected_registered: 0,
            skipped_non_completed: 0,
            skipped_corrupt_registry_entries: 0,
            candidates: Vec::new(),
            removed_sessions: 0,
            removed_supervisor_files: 0,
            reclaimed_bytes: 0,
        };
        if !sessions.exists() {
            return Ok(report);
        }
        validate_owner_directory(&sessions)?;
        let (registered, corrupt_entries) = self.protected_session_paths()?;
        report.skipped_corrupt_registry_entries = corrupt_entries;
        if corrupt_entries != 0 {
            return Ok(report);
        }
        let mut completed_sessions = Vec::new();
        for item in fs_err::read_dir(&sessions).map_err(|source| {
            return Error::Filesystem {
                path: sessions.clone(),
                source,
            };
        })? {
            let item = item.map_err(|source| {
                return Error::Filesystem {
                    path: sessions.clone(),
                    source,
                };
            })?;
            let session = item.path();
            let metadata = fs_err::symlink_metadata(&session).map_err(|source| {
                return Error::Filesystem {
                    path: session.clone(),
                    source,
                };
            })?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                report.skipped_non_completed = report.skipped_non_completed.saturating_add(1);
                continue;
            }
            let canonical = session.canonicalize().map_err(|source| {
                return Error::Filesystem {
                    path: session.clone(),
                    source,
                };
            })?;
            if registered.contains(&canonical) || registered.contains(&session) {
                report.protected_registered = report.protected_registered.saturating_add(1);
                continue;
            }
            let completion_path = session.join("completion.json");
            let Ok(log) = EventLog::read(session.join("events.jsonl")) else {
                report.skipped_non_completed = report.skipped_non_completed.saturating_add(1);
                continue;
            };
            let Ok(completion) = validate_completed_log(&session, &log) else {
                report.skipped_non_completed = report.skipped_non_completed.saturating_add(1);
                continue;
            };
            let Ok(manifest) = read_manifest(&session) else {
                report.skipped_non_completed = report.skipped_non_completed.saturating_add(1);
                continue;
            };
            if completion.session_id != manifest.session_id {
                report.skipped_non_completed = report.skipped_non_completed.saturating_add(1);
                continue;
            }
            let completed_at = fs_err::symlink_metadata(&completion_path)
                .and_then(|metadata| return metadata.modified())
                .map_err(|source| {
                    return Error::Filesystem {
                        path: completion_path,
                        source,
                    };
                })?;
            completed_sessions.push((session, completion.session_id, completed_at));
        }
        completed_sessions.sort_by(|left, right| return right.2.cmp(&left.2));
        let retention = Duration::from_secs(older_than_days.saturating_mul(86_400));
        let cutoff = SystemTime::now()
            .checked_sub(retention)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        for (index, (session, session_id, completed)) in completed_sessions.into_iter().enumerate()
        {
            if u64::try_from(index).unwrap_or(u64::MAX) < keep_latest || completed > cutoff {
                continue;
            }
            let bytes = directory_bytes_nofollow(&session)?;
            let completed_unix_ns = completed
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            report.candidates.push(GarbageCollectionCandidate {
                session,
                session_id,
                completed_unix_ns,
                bytes,
            });
        }
        if !apply {
            return Ok(report);
        }
        let supervisor = self.state_root.join("supervisor");
        for candidate in &report.candidates {
            validate_gc_candidate(candidate)?;
            fs_err::remove_dir_all(&candidate.session).map_err(|source| {
                return Error::Filesystem {
                    path: candidate.session.clone(),
                    source,
                };
            })?;
            report.removed_sessions = report.removed_sessions.saturating_add(1);
            report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(candidate.bytes);
            for extension in ["stdout", "stderr"] {
                let log = supervisor.join(format!("{}.{extension}", candidate.session_id));
                match fs_err::remove_file(&log) {
                    Ok(()) => {
                        report.removed_supervisor_files =
                            report.removed_supervisor_files.saturating_add(1);
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(source) => return Err(Error::Filesystem { path: log, source }),
                }
            }
        }
        sync_directory(&sessions)?;
        if supervisor.is_dir() {
            sync_directory(&supervisor)?;
        }
        return Ok(report);
    }

    /// Return the live recorder PID/start pair observed through socket credentials.
    pub fn recorder_identity(&self, session: &Path) -> Option<(u32, u64)> {
        return recorder_process_identity(session);
    }

    /// Resolve and validate an alias against its canonical session manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is missing, insecure, malformed,
    /// incompatible, or no longer names the registered immutable session ID.
    pub fn resolve(&self, alias: &str) -> Result<RegistryEntry> {
        validate_alias(alias)?;
        let path = self.entry_path(alias);
        let entry = match read_registry_record_nofollow(&path)? {
            RegistryRecord::Entry(entry) => entry,
            RegistryRecord::Reservation(_) => {
                return Err(registry_error(
                    "resolve registry entry",
                    "alias startup reservation is still pending",
                ));
            }
        };
        if entry.format != REGISTRY_ENTRY_FORMAT
            || entry.version != REGISTRY_ENTRY_VERSION
            || entry.alias != alias
            || entry.control_protocol_version != CONTROL_PROTOCOL_VERSION
            || entry.recorder_pid.is_some() != entry.recorder_process_start_ticks.is_some()
        {
            return Err(registry_error(
                "resolve registry entry",
                "entry format, alias, protocol, or process identity is incompatible",
            ));
        }
        let session = entry.session_path();
        validate_owner_directory(&session)?;
        let canonical = session.canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.clone(),
                source,
            };
        })?;
        if canonical.as_os_str().as_bytes() != entry.canonical_session.as_bytes() {
            return Err(registry_error(
                "resolve registry entry",
                "canonical session path changed after registration",
            ));
        }
        let manifest = read_manifest(&canonical)?;
        if manifest.session_id != entry.session_id {
            return Err(registry_error(
                "resolve registry entry",
                "session manifest identity no longer matches the registry binding",
            ));
        }
        return Ok(entry);
    }

    /// Resolve a named session without discarding pending or unhealthy lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry record itself is missing, insecure, or malformed.
    pub fn resolve_lifecycle(&self, alias: &str) -> Result<RegistrySessionResolution> {
        validate_alias(alias)?;
        let record = read_registry_record_nofollow(&self.entry_path(alias))?;
        return match record {
            RegistryRecord::Entry(entry) => {
                let row = inspect_entry(&entry);
                Ok(RegistrySessionResolution {
                    session: entry.session_path(),
                    lifecycle: row.lifecycle,
                    diagnostic: row.diagnostic,
                })
            }
            RegistryRecord::Reservation(reservation) => {
                if !valid_reservation(alias, &reservation) {
                    return Ok(RegistrySessionResolution {
                        session: reservation.session_path(),
                        lifecycle: RegistryLifecycle::Incompatible,
                        diagnostic: Some("alias startup reservation is incompatible".to_owned()),
                    });
                }
                let expired = reservation.lease_expires_unix_ns <= system_time_unix_ns()?;
                Ok(RegistrySessionResolution {
                    session: reservation.session_path(),
                    lifecycle: if expired {
                        RegistryLifecycle::Stale
                    } else {
                        RegistryLifecycle::Incomplete
                    },
                    diagnostic: Some(if expired {
                        "alias startup reservation lease expired before promotion".to_owned()
                    } else {
                        "alias startup reservation is pending promotion".to_owned()
                    }),
                })
            }
        };
    }

    /// List live aliases, or every alias when `include_all` is true.
    ///
    /// # Errors
    ///
    /// Returns an error when the registry directory or an entry cannot be read
    /// securely. Broken session evidence is represented as a lifecycle row.
    pub fn list(&self, include_all: bool) -> Result<Vec<RegistryListEntry>> {
        let mut rows = Vec::new();
        for item in fs_err::read_dir(&self.directory).map_err(|source| {
            return Error::Filesystem {
                path: self.directory.clone(),
                source,
            };
        })? {
            let item = item.map_err(|source| {
                return Error::Filesystem {
                    path: self.directory.clone(),
                    source,
                };
            })?;
            let path = item.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let entry = match read_registry_record_nofollow(&path) {
                Ok(RegistryRecord::Entry(entry)) => entry,
                Ok(RegistryRecord::Reservation(_)) => continue,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "isolating corrupt registry entry from list"
                    );
                    continue;
                }
            };
            let row = inspect_entry(&entry);
            if include_all || row.lifecycle == RegistryLifecycle::Live {
                rows.push(row);
            }
        }
        rows.sort_by(|left, right| return left.alias.cmp(&right.alias));
        return Ok(rows);
    }

    /// Remove only a valid registry binding and preserve its session directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the alias or entry is invalid or removal cannot be
    /// durably published.
    pub fn remove(&self, alias: &str) -> Result<RegistryEntry> {
        validate_alias(alias)?;
        let _lock = self.lock()?;
        let path = self.entry_path(alias);
        let entry = read_entry_nofollow(&path)?;
        if entry.alias != alias {
            return Err(registry_error(
                "remove registry entry",
                "entry alias does not match its filename",
            ));
        }
        fs_err::remove_file(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        sync_parent_directory(&path)?;
        return Ok(entry);
    }

    fn entry_path(&self, alias: &str) -> PathBuf {
        return self.directory.join(format!("{alias}.json"));
    }

    fn protected_session_paths(&self) -> Result<(HashSet<PathBuf>, u64)> {
        let mut paths = HashSet::new();
        let mut corrupt_entries = 0_u64;
        for item in fs_err::read_dir(&self.directory).map_err(|source| {
            return Error::Filesystem {
                path: self.directory.clone(),
                source,
            };
        })? {
            let item = item.map_err(|source| {
                return Error::Filesystem {
                    path: self.directory.clone(),
                    source,
                };
            })?;
            let path = item.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let session = match read_registry_record_nofollow(&path) {
                Ok(RegistryRecord::Entry(entry)) => entry.session_path(),
                Ok(RegistryRecord::Reservation(reservation)) => reservation.session_path(),
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "isolating corrupt registry entry from garbage collection"
                    );
                    corrupt_entries = corrupt_entries.saturating_add(1);
                    continue;
                }
            };
            paths.insert(session);
        }
        return Ok((paths, corrupt_entries));
    }

    fn entry_for(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
        validate_owner_directory(session)?;
        let canonical = session.canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.to_path_buf(),
                source,
            };
        })?;
        validate_owner_directory(&canonical)?;
        let manifest = read_manifest(&canonical)?;
        if manifest.format != SESSION_MANIFEST_FORMAT
            || manifest.version != SESSION_MANIFEST_VERSION
        {
            return Err(registry_error(
                "publish session alias",
                "session manifest format is unsupported by this registry build",
            ));
        }
        let registered_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| return Error::ClockBeforeEpoch)?
            .as_nanos();
        let process_identity = recorder_process_identity(&canonical);
        return Ok(RegistryEntry {
            format: REGISTRY_ENTRY_FORMAT.to_owned(),
            version: REGISTRY_ENTRY_VERSION,
            alias: alias.to_owned(),
            canonical_session: ByteString::from(canonical.as_os_str().as_bytes()),
            session_id: manifest.session_id,
            recorder_pid: process_identity.map(|identity| return identity.0),
            recorder_process_start_ticks: process_identity.map(|identity| return identity.1),
            control_protocol_version: CONTROL_PROTOCOL_VERSION,
            registered_unix_ns,
        });
    }

    fn encode_entry(&self, entry: &RegistryEntry) -> Result<Vec<u8>> {
        return serde_json::to_vec_pretty(entry).map_err(|source| {
            return Error::InvalidJson {
                path: self.entry_path(&entry.alias),
                json_path: "$".to_owned(),
                source,
            };
        });
    }

    fn encode_reservation(&self, reservation: &AliasReservation) -> Result<Vec<u8>> {
        return serde_json::to_vec_pretty(reservation).map_err(|source| {
            return Error::InvalidJson {
                path: self.entry_path(&reservation.alias),
                json_path: "$".to_owned(),
                source,
            };
        });
    }

    fn lock(&self) -> Result<Flock<File>> {
        let path = self.directory.join(".registry.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW)
            .open(&path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: path.clone(),
                    source,
                };
            })?;
        validate_entry_file(&path, &file)?;
        return Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, error)| {
            return registry_error("lock session registry", error.to_string());
        });
    }
}

fn inspect_entry(entry: &RegistryEntry) -> RegistryListEntry {
    let mut row = RegistryListEntry {
        alias: entry.alias.clone(),
        canonical_session: entry.canonical_session.clone(),
        session_id: entry.session_id,
        command: None,
        working_directory: None,
        geometry: None,
        started_unix_ns: None,
        registered_unix_ns: entry.registered_unix_ns,
        lifecycle: RegistryLifecycle::MissingEvidence,
        diagnostic: None,
    };
    if entry.format != REGISTRY_ENTRY_FORMAT
        || entry.version != REGISTRY_ENTRY_VERSION
        || entry.control_protocol_version != CONTROL_PROTOCOL_VERSION
        || entry.recorder_pid.is_some() != entry.recorder_process_start_ticks.is_some()
    {
        row.lifecycle = RegistryLifecycle::Incompatible;
        row.diagnostic =
            Some("registry format, protocol, or process identity is incompatible".to_owned());
        return row;
    }
    let session = entry.session_path();
    if let Err(error) = validate_owner_directory(&session) {
        row.diagnostic = Some(error.to_string());
        return row;
    }
    let canonical = match session.canonicalize() {
        Ok(path) if path.as_os_str().as_bytes() == entry.canonical_session.as_bytes() => path,
        Ok(_) => {
            row.diagnostic = Some("canonical session path changed after registration".to_owned());
            return row;
        }
        Err(error) => {
            row.diagnostic = Some(error.to_string());
            return row;
        }
    };
    let manifest = match read_manifest(&canonical) {
        Ok(manifest) => manifest,
        Err(error) => {
            row.diagnostic = Some(error.to_string());
            return row;
        }
    };
    row.command = Some(manifest.command.clone());
    row.working_directory = manifest.working_directory.clone();
    row.geometry = Some(manifest.geometry.clone());
    row.started_unix_ns = Some(manifest.started_unix_ns);
    if manifest.session_id != entry.session_id {
        row.lifecycle = RegistryLifecycle::IdentityMismatch;
        row.diagnostic = Some("manifest session ID differs from registry binding".to_owned());
        return row;
    }
    let completion_path = canonical.join("completion.json");
    if completion_path.exists() {
        match read_completion(&canonical) {
            Ok(completion) if completion.session_id == entry.session_id => {
                row.lifecycle = RegistryLifecycle::Completed;
                return row;
            }
            Ok(_) => {
                row.lifecycle = RegistryLifecycle::IdentityMismatch;
                row.diagnostic =
                    Some("completion session ID differs from registry binding".to_owned());
                return row;
            }
            Err(error) => {
                row.lifecycle = RegistryLifecycle::MissingEvidence;
                row.diagnostic = Some(error.to_string());
                return row;
            }
        }
    }
    let responsive_identity = recorder_process_identity(&canonical);
    match (entry.recorder_pid, entry.recorder_process_start_ticks) {
        (Some(pid), Some(start_ticks)) if responsive_identity == Some((pid, start_ticks)) => {
            row.lifecycle = RegistryLifecycle::Live;
        }
        (Some(_), Some(_)) => {
            row.lifecycle = RegistryLifecycle::Stale;
            row.diagnostic =
                Some("recorder PID/start identity or control socket is stale".to_owned());
        }
        (None, None) if responsive_identity.is_some() => row.lifecycle = RegistryLifecycle::Live,
        (None, None) => row.lifecycle = RegistryLifecycle::Incomplete,
        _ => {
            row.lifecycle = RegistryLifecycle::Incompatible;
            row.diagnostic = Some("partial recorder process identity".to_owned());
        }
    }
    return row;
}

fn recorder_process_identity(session: &Path) -> Option<(u32, u64)> {
    let stream = connect_session_control(session).ok()?;
    let credentials = getsockopt(&stream, PeerCredentials).ok()?;
    let pid = u32::try_from(credentials.pid()).ok()?;
    return Some((pid, process_start_ticks(pid)?));
}

fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = fs_err::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    return stat
        .get(close + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok();
}

fn directory_bytes_nofollow(directory: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for item in fs_err::read_dir(directory).map_err(|source| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source,
        };
    })? {
        let item = item.map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?;
        let path = item.path();
        let metadata = fs_err::symlink_metadata(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(registry_error(
                "measure managed session",
                "managed session contains a symbolic link",
            ));
        }
        if file_type.is_dir() {
            total = total.saturating_add(directory_bytes_nofollow(&path)?);
        } else if file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(registry_error(
                "measure managed session",
                "managed session contains a non-file member",
            ));
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    return Ok(total);
}

fn validate_gc_candidate(candidate: &GarbageCollectionCandidate) -> Result<()> {
    let metadata = fs_err::symlink_metadata(&candidate.session).map_err(|source| {
        return Error::Filesystem {
            path: candidate.session.clone(),
            source,
        };
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(registry_error(
            "apply managed-session retention",
            "garbage-collection candidate is no longer a real directory",
        ));
    }
    let log = EventLog::read(candidate.session.join("events.jsonl"))?;
    let completion = validate_completed_log(&candidate.session, &log)?;
    let manifest = read_manifest(&candidate.session)?;
    if completion.session_id != candidate.session_id || manifest.session_id != candidate.session_id
    {
        return Err(registry_error(
            "apply managed-session retention",
            "garbage-collection candidate identity changed after planning",
        ));
    }
    return Ok(());
}

fn default_state_root() -> Result<PathBuf> {
    if let Some(path) = env::var_os("TERMTRACE_STATE_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("termtrace"));
    }
    let home = env::var_os("HOME").ok_or_else(|| {
        return registry_error(
            "select registry state root",
            "TERMTRACE_STATE_DIR, XDG_STATE_HOME, and HOME are unset",
        );
    })?;
    return Ok(PathBuf::from(home).join(".local/state/termtrace"));
}

fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty()
        || alias.len() > 64
        || !alias.bytes().all(|byte| {
            return byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
        })
        || !alias
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(registry_error(
            "validate session alias",
            "alias must be 1-64 ASCII letters, digits, '-' or '_', beginning with a letter or digit",
        ));
    }
    return Ok(());
}

fn system_time_unix_ns() -> Result<u128> {
    return Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| return Error::ClockBeforeEpoch)?
        .as_nanos());
}

fn valid_reservation(alias: &str, reservation: &AliasReservation) -> bool {
    return reservation.format == REGISTRY_RESERVATION_FORMAT
        && reservation.version == REGISTRY_RESERVATION_VERSION
        && reservation.alias == alias
        && reservation.created_unix_ns < reservation.lease_expires_unix_ns
        && !reservation.pending_session.as_bytes().is_empty();
}

fn ensure_owner_directory(path: &Path) -> Result<()> {
    if path.exists() {
        return validate_owner_directory(path);
    }
    fs_err::create_dir_all(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    fs_err::set_permissions(path, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    sync_parent_directory(path)?;
    return validate_owner_directory(path);
}

fn validate_owner_directory(path: &Path) -> Result<()> {
    let metadata = fs_err::symlink_metadata(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mode = metadata.mode() & 0o777;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != getuid().as_raw()
        || mode != 0o700
    {
        return Err(Error::InsecureArtifactDirectory {
            path: path.to_path_buf(),
            owner_uid: metadata.uid(),
            mode,
        });
    }
    return Ok(());
}

#[allow(
    clippy::verbose_file_reads,
    reason = "the already-opened O_NOFOLLOW descriptor must be read without reopening its path"
)]
fn read_registry_record_nofollow(path: &Path) -> Result<RegistryRecord> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    validate_entry_file(path, &file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if value.get("owner_token").is_some() {
        return serde_json::from_value(value)
            .map(RegistryRecord::Reservation)
            .map_err(|source| {
                return Error::InvalidJson {
                    path: path.to_path_buf(),
                    json_path: "$".to_owned(),
                    source,
                };
            });
    }
    return serde_json::from_value(value)
        .map(RegistryRecord::Entry)
        .map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        });
}

fn read_entry_nofollow(path: &Path) -> Result<RegistryEntry> {
    return match read_registry_record_nofollow(path)? {
        RegistryRecord::Entry(entry) => Ok(entry),
        RegistryRecord::Reservation(_) => Err(registry_error(
            "read registry entry",
            "alias startup reservation is still pending",
        )),
    };
}

fn validate_entry_file(path: &Path, file: &File) -> Result<()> {
    let metadata = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    if metadata.mode() & 0o170_000 != 0o100_000
        || metadata.uid() != getuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(registry_error(
            "validate registry entry",
            "entry must be an owner-owned regular file with mode 0600",
        ));
    }
    return Ok(());
}

fn registry_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        environment::EnvironmentSpec,
        event::TerminalColors,
        event_log::DurabilityPolicy,
        provenance::ProvenancePolicy,
        record::{RecordOptions, record},
        session::{GeometryProvenance, ResourceLimits},
    };
    use std::{ffi::OsString, os::unix::fs::symlink};
    use tempfile::TempDir;

    #[test]
    fn alias_validation_rejects_paths_and_ambiguous_prefixes() {
        for alias in ["", "-bad", "../escape", "has space", "é"] {
            assert!(validate_alias(alias).is_err(), "accepted alias {alias:?}");
        }
        assert!(validate_alias("build-01_main").is_ok());
    }

    #[test]
    fn registry_rejects_symlink_entries_without_following_them() {
        let temporary = TempDir::new().expect("temporary directory");
        let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
        let target = temporary.path().join("target");
        fs_err::write(&target, b"{}").expect("target");
        symlink(&target, registry.entry_path("demo")).expect("symlink");
        assert!(registry.resolve("demo").is_err());
    }

    #[test]
    fn reservation_excludes_concurrent_owners_and_promotes_only_the_winner() {
        use std::{
            sync::{Arc, Barrier},
            thread,
        };

        let temporary = TempDir::new().expect("temporary directory");
        let registry =
            Arc::new(SessionRegistry::open(temporary.path().join("state")).expect("registry"));
        let sessions = registry.state_root().join("sessions");
        fs_err::create_dir(&sessions).expect("sessions directory");
        let barrier = Arc::new(Barrier::new(3));
        let mut contenders = Vec::new();
        for name in ["first", "second"] {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            let pending = sessions.join(format!("{name}.termtrace"));
            contenders.push(thread::spawn(move || {
                barrier.wait();
                return registry.reserve("demo", &pending, Duration::from_secs(30));
            }));
        }
        barrier.wait();
        let results = contenders
            .into_iter()
            .map(|contender| return contender.join().expect("reservation contender"))
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| return result.is_ok())
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| return result.is_err())
                .count(),
            1
        );

        let reservation = results
            .into_iter()
            .find_map(Result::ok)
            .expect("winning reservation");
        let encoded: serde_json::Value = serde_json::from_slice(
            &fs_err::read(registry.entry_path("demo")).expect("reservation record"),
        )
        .expect("reservation JSON");
        assert_eq!(encoded["owner_token"], reservation.owner_token.to_string());
        assert_eq!(
            encoded["pending_session"],
            serde_json::to_value(&reservation.pending_session).expect("pending path JSON")
        );
        assert!(encoded["created_unix_ns"].is_number());
        assert!(encoded["lease_expires_unix_ns"].is_number());
        let mut wrong_owner = reservation.clone();
        wrong_owner.owner_token = Uuid::now_v7();
        assert!(registry.release(&wrong_owner).is_err());

        record_fixture(reservation.session_path());
        let entry = registry.promote(&reservation).expect("promote winner");
        assert_eq!(registry.resolve("demo").expect("resolve promoted"), entry);
    }

    #[test]
    fn expired_reservation_is_reclaimed_without_accepting_the_old_owner() {
        let temporary = TempDir::new().expect("temporary directory");
        let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
        let first_path = registry.state_root().join("sessions/first.termtrace");
        let first = registry
            .reserve("demo", &first_path, Duration::from_secs(30))
            .expect("first reservation");
        let mut expired = first.clone();
        expired.created_unix_ns = 0;
        expired.lease_expires_unix_ns = 1;
        fs_err::write(
            registry.entry_path("demo"),
            registry
                .encode_reservation(&expired)
                .expect("expired reservation encoding"),
        )
        .expect("publish expired reservation fixture");

        let second_path = registry.state_root().join("sessions/second.termtrace");
        let second = registry
            .reserve("demo", &second_path, Duration::from_secs(30))
            .expect("reclaim expired reservation");
        assert_ne!(second.owner_token, first.owner_token);
        assert!(registry.promote(&first).is_err());
        let resolved = registry
            .resolve_lifecycle("demo")
            .expect("resolve replacement reservation");
        assert_eq!(resolved.lifecycle, RegistryLifecycle::Incomplete);
        assert_eq!(resolved.session, second_path);
        registry.release(&second).expect("release replacement");
    }

    #[test]
    fn retarget_uses_compare_and_swap_and_preserves_previous_binding_on_failure() {
        let temporary = TempDir::new().expect("temporary directory");
        let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
        let first = record_fixture(temporary.path().join("first"));
        let second = record_fixture(temporary.path().join("second"));
        let original = registry.register("demo", &first).expect("register first");

        let rejected = registry.retarget("demo", Uuid::now_v7(), &second);
        assert!(rejected.is_err());
        assert_eq!(
            registry.resolve("demo").unwrap().session_id,
            original.session_id
        );

        let successor = registry
            .retarget("demo", original.session_id, &second)
            .expect("retarget successor");
        assert_ne!(successor.session_id, original.session_id);
        assert_eq!(registry.resolve("demo").unwrap(), successor);
    }

    fn record_fixture(output_directory: PathBuf) -> PathBuf {
        record(&RecordOptions {
            output_directory: output_directory.clone(),
            creation_alias: None,
            lineage: None,
            import_provenance: None,
            tmux_pane: None,
            command: vec![OsString::from("/bin/true")],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            geometry_provenance: GeometryProvenance::default(),
            ghostty_profile: None,
            term: "xterm-ghostty".to_owned(),
            environment: EnvironmentSpec::default(),
            colors: TerminalColors::default(),
            forward_stdin: false,
            continue_after_output_mirror_closes: false,
            durability_policy: DurabilityPolicy::default(),
            provenance_policy: ProvenancePolicy::default(),
            resource_limits: ResourceLimits::default(),
        })
        .expect("record fixture");
        return output_directory;
    }
}
