use super::*;
use crate::recording::terminate_process_group;
use std::cmp::Reverse;

impl SessionRegistry {
    /// Allocate collision-free paths for a detached session and supervisor logs.
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
        let retention = Duration::from_secs(older_than_days.saturating_mul(86_400));
        let cutoff = SystemTime::now()
            .checked_sub(retention)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut report = GarbageCollectionReport {
            state_root: self.state_root.clone(),
            changes_applied: apply,
            older_than_days,
            keep_latest,
            protected_registered: 0,
            skipped_non_completed: 0,
            skipped_corrupt_registry_entries: 0,
            candidates: Vec::new(),
            removed_sessions: 0,
            removed_supervisor_files: 0,
            removed_stale_aliases: 0,
            reclaimed_bytes: 0,
        };
        if !sessions.exists() {
            return Ok(report);
        }
        validate_owner_directory(&sessions)?;
        let (registered, corrupt_entries, removed_stale_aliases) =
            self.reap_and_resolve_protected_paths(cutoff, older_than_days, apply)?;
        report.skipped_corrupt_registry_entries = corrupt_entries;
        report.removed_stale_aliases = removed_stale_aliases;
        if corrupt_entries != 0 {
            return Ok(report);
        }
        let mut completed_sessions = Vec::new();
        for item in fs::read_dir(&sessions).map_err(|source| {
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
            let metadata = fs::symlink_metadata(&session).map_err(|source| {
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
            let completed_at = fs::symlink_metadata(&completion_path)
                .and_then(|metadata| return metadata.modified())
                .map_err(|source| {
                    return Error::Filesystem {
                        path: completion_path,
                        source,
                    };
                })?;
            completed_sessions.push((session, completion.session_id, completed_at));
        }
        completed_sessions.sort_by_key(|entry| return Reverse(entry.2));
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
            fs::remove_dir_all(&candidate.session).map_err(|source| {
                return Error::Filesystem {
                    path: candidate.session.clone(),
                    source,
                };
            })?;
            report.removed_sessions = report.removed_sessions.saturating_add(1);
            report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(candidate.bytes);
            for extension in ["stdout", "stderr"] {
                let log = supervisor.join(format!("{}.{extension}", candidate.session_id));
                match fs::remove_file(&log) {
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

    fn reap_and_resolve_protected_paths(
        &self,
        cutoff: SystemTime,
        older_than_days: u64,
        apply: bool,
    ) -> Result<(HashSet<PathBuf>, u64, u64)> {
        let mut paths = HashSet::new();
        let mut corrupt_entries = 0_u64;
        let mut removed_stale_aliases = 0_u64;
        let now_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
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
            let record = match read_registry_record_nofollow(&path) {
                Ok(record) => record,
                Err(_) => {
                    corrupt_entries = corrupt_entries.saturating_add(1);
                    continue;
                }
            };
            match record {
                RegistryRecord::Reservation(reservation) => {
                    if reservation.lease_expires_unix_ns < now_unix_ns {
                        if apply {
                            let _ = fs::remove_file(&path);
                        }
                        removed_stale_aliases = removed_stale_aliases.saturating_add(1);
                    } else {
                        paths.insert(reservation.session_path());
                    }
                }
                RegistryRecord::Entry(entry) => {
                    let session = entry.session_path();
                    let responsive_identity = recorder_identity(&session);
                    let is_live = entry
                        .recorder_pid
                        .zip(entry.recorder_process_start_ticks)
                        .map_or_else(
                            || return responsive_identity.is_some(),
                            |identity| return responsive_identity == Some(identity),
                        );

                    let completion_exists = session.join("completion.json").is_file();
                    let mut should_reap = false;

                    if !completion_exists {
                        if !is_live {
                            should_reap = true;
                        } else if older_than_days > 0 {
                            let last_activity = fs::symlink_metadata(session.join("events.jsonl"))
                                .and_then(|m| return m.modified())
                                .or_else(|_| {
                                    return fs::symlink_metadata(session.join("requests.jsonl"))
                                        .and_then(|m| return m.modified());
                                })
                                .unwrap_or(SystemTime::UNIX_EPOCH);
                            if last_activity < cutoff {
                                should_reap = true;
                            }
                        }
                    }

                    if should_reap {
                        if apply {
                            if let Some(pid) = entry.recorder_pid {
                                terminate_process_group(Some(pid));
                            }
                            let _ = fs::remove_file(&path);
                        }
                        removed_stale_aliases = removed_stale_aliases.saturating_add(1);
                    } else {
                        if let Ok(canonical) = session.canonicalize() {
                            paths.insert(canonical);
                        }
                        paths.insert(session);
                    }
                }
            }
        }
        if apply && removed_stale_aliases > 0 {
            sync_directory(&self.directory)?;
        }
        return Ok((paths, corrupt_entries, removed_stale_aliases));
    }
}

pub(super) fn directory_bytes_nofollow(directory: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for item in fs::read_dir(directory).map_err(|source| {
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
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
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
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else {
            return Err(registry_error(
                "measure managed session",
                "managed session contains a non-file member",
            ));
        }
    }
    return Ok(total);
}

pub(super) fn validate_gc_candidate(candidate: &GarbageCollectionCandidate) -> Result<()> {
    let metadata = fs::symlink_metadata(&candidate.session).map_err(|source| {
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
