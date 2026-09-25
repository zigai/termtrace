use super::*;
use directories::BaseDirs;

impl SessionRegistry {
    /// Open the default owner-only session registry.
    ///
    /// `TERMTRACE_STATE_DIR` overrides the platform user state directory.
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

    /// Atomically retarget an alias when its current immutable session ID matches.
    ///
    /// Returns an error when the caller no longer owns the alias or successor
    /// validation/publication fails.
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

        if let Err(source) = fs::rename(&temporary, &path) {
            let _cleanup = fs::remove_file(&temporary);

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
                        lifecycle: SessionPhase::Incompatible,
                        diagnostic: Some("alias startup reservation is incompatible".to_owned()),
                    });
                }

                let expired = reservation.lease_expires_unix_ns <= system_time_unix_ns()?;

                Ok(RegistrySessionResolution {
                    session: reservation.session_path(),
                    lifecycle: if expired { SessionPhase::Stale } else { SessionPhase::Incomplete },
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
        for item in fs::read_dir(&self.directory).map_err(|source| {
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
                Err(_) => continue,
            };

            let row = inspect_entry(&entry);

            if include_all || row.lifecycle == SessionPhase::Live {
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

        fs::remove_file(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        sync_parent_directory(&path)?;

        return Ok(entry);
    }

    pub(super) fn entry_path(&self, alias: &str) -> PathBuf {
        return self.directory.join(format!("{alias}.json"));
    }

    pub(super) fn entry_for(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
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

        let recorder = recorder_identity(&canonical);

        return Ok(RegistryEntry {
            format: REGISTRY_ENTRY_FORMAT.to_owned(),
            version: REGISTRY_ENTRY_VERSION,
            alias: alias.to_owned(),
            canonical_session: ByteString::from(canonical.as_os_str().as_bytes()),
            session_id: manifest.session_id,
            recorder_pid: recorder.map(|identity| return identity.pid),
            recorder_process_start_ticks: recorder.map(|identity| return identity.start_ticks),
            control_protocol_version: CONTROL_PROTOCOL_VERSION,
            registered_unix_ns,
        });
    }

    fn encode_record(&self, alias: &str, value: &impl serde::Serialize) -> Result<Vec<u8>> {
        return serde_json::to_vec_pretty(value).map_err(|source| {
            return Error::InvalidJson {
                path: self.entry_path(alias),
                json_path: "$".to_owned(),
                source,
            };
        });
    }

    pub(super) fn encode_entry(&self, entry: &RegistryEntry) -> Result<Vec<u8>> {
        return self.encode_record(&entry.alias, entry);
    }

    pub(super) fn encode_reservation(&self, reservation: &AliasReservation) -> Result<Vec<u8>> {
        return self.encode_record(&reservation.alias, reservation);
    }

    pub(super) fn lock(&self) -> Result<ExclusiveLock> {
        let path = self.directory.join(".registry.lock");

        return ExclusiveLock::acquire(&path).map_err(|source| {
            return Error::Filesystem { path, source };
        });
    }
}

pub(super) fn inspect_entry(entry: &RegistryEntry) -> RegistryListEntry {
    let mut row = RegistryListEntry {
        alias: entry.alias.clone(),
        canonical_session: entry.canonical_session.clone(),
        session_id: entry.session_id,
        command: None,
        working_directory: None,
        geometry: None,
        started_unix_ns: None,
        registered_unix_ns: entry.registered_unix_ns,
        lifecycle: SessionPhase::MissingEvidence,
        diagnostic: None,
    };

    if entry.format != REGISTRY_ENTRY_FORMAT
        || entry.version != REGISTRY_ENTRY_VERSION
        || entry.control_protocol_version != CONTROL_PROTOCOL_VERSION
        || entry.recorder_pid.is_some() != entry.recorder_process_start_ticks.is_some()
    {
        row.lifecycle = SessionPhase::Incompatible;
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

    row.working_directory
        .clone_from(&manifest.working_directory);

    row.geometry = Some(manifest.geometry.clone());
    row.started_unix_ns = Some(manifest.started_unix_ns);

    if manifest.session_id != entry.session_id {
        row.lifecycle = SessionPhase::IdentityMismatch;
        row.diagnostic = Some("manifest session ID differs from registry binding".to_owned());

        return row;
    }

    let observation = observe_phase(&canonical);
    row.lifecycle = observation.phase;
    row.diagnostic = observation.diagnostic;

    if let Some(recorded) = entry.recorder_pid.zip(entry.recorder_process_start_ticks)
        && matches!(row.lifecycle, SessionPhase::Live | SessionPhase::Incomplete)
        && observation
            .recorder
            .map(|identity| return (identity.pid, identity.start_ticks))
            != Some(recorded)
    {
        row.lifecycle = SessionPhase::Stale;
        row.diagnostic = Some("recorder PID/start identity or control socket is stale".to_owned());
    }

    return row;
}

pub(super) fn default_state_root() -> Result<PathBuf> {
    if let Some(path) = env::var_os("TERMTRACE_STATE_DIR") {
        return Ok(PathBuf::from(path));
    }

    let state_home = BaseDirs::new()
        .and_then(|dirs| {
            return dirs.state_dir().map(Path::to_path_buf);
        })
        .ok_or_else(|| {
            return registry_error(
                "select registry state root",
                "platform user state directory is unavailable",
            );
        })?;

    return Ok(state_home.join("termtrace"));
}

pub(super) fn ensure_owner_directory(path: &Path) -> Result<()> {
    if path.exists() {
        return validate_owner_directory(path);
    }

    fs::create_dir_all(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    fs::set_permissions(path, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    sync_parent_directory(path)?;

    return validate_owner_directory(path);
}

pub(super) fn validate_owner_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
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

pub(super) fn read_registry_record_nofollow(path: &Path) -> Result<RegistryRecord> {
    let file = OpenOptions::new()
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

    return serde_json::from_reader(file).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

pub(super) fn read_entry_nofollow(path: &Path) -> Result<RegistryEntry> {
    return match read_registry_record_nofollow(path)? {
        RegistryRecord::Entry(entry) => Ok(entry),
        RegistryRecord::Reservation(_) => Err(registry_error(
            "read registry entry",
            "alias startup reservation is still pending",
        )),
    };
}

pub(super) fn validate_entry_file(path: &Path, file: &File) -> Result<()> {
    let metadata = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    if !metadata.is_file()
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

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    let directory = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    return directory.sync_all().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

pub(super) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        return registry_error("sync registry parent", "path has no parent directory");
    })?;

    return sync_directory(parent);
}

pub(super) fn registry_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}
