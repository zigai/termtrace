use super::{
    ensure_owner_directory, fs, read_manifest, read_registry_record_nofollow, recorder_identity,
    registry_error, sync_directory, validate_owner_directory, DetachedSessionPaths, Duration,
    Error, ErrorKind, GarbageCollectionCandidate, GarbageCollectionReport, HashSet, OsStr, Path,
    PathBuf, RegistryRecord, Result, SessionRegistry, SystemTime, Uuid, UNIX_EPOCH,
};
use std::cmp::Reverse;
use termtrace_evidence::session::validate_completed_session;

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
        return self.garbage_collect_with_quota(older_than_days, keep_latest, None, apply);
    }

    /// Plan or apply retention with an optional completed-recording byte target.
    ///
    /// Quota pressure overrides age, but never registry protection or `keep_latest`.
    /// Oldest eligible recordings are removed first. Live and incomplete evidence
    /// is excluded from this quota and is never deleted. The report exposes any
    /// excess that cannot be reclaimed without deleting protected evidence.
    ///
    /// # Errors
    ///
    /// Returns an error when state inspection, validation, or durable deletion fails.
    pub fn garbage_collect_with_quota(
        &self,
        older_than_days: u64,
        keep_latest: u64,
        max_completed_bytes: Option<u64>,
        apply: bool,
    ) -> Result<GarbageCollectionReport> {
        // Serialize alias changes with the protection scan and apply phase.
        let _lock = self.lock()?;
        let sessions = self.state_root.join("sessions");
        let mut report = GarbageCollectionReport {
            state_root: self.state_root.clone(),
            changes_applied: apply,
            older_than_days,
            keep_latest,
            max_completed_bytes,
            completed_bytes_before: 0,
            completed_bytes_after_plan: 0,
            quota_excess_bytes: 0,
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
            self.reap_and_resolve_protected_paths(apply)?;
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

            let protected = registered.contains(&canonical) || registered.contains(&session);
            if protected {
                report.protected_registered = report.protected_registered.saturating_add(1);
            }

            let completion_path = session.join("completion.json");
            let Ok(completion) = validate_completed_session(&session) else {
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

            let bytes = directory_bytes_nofollow(&session)?;
            report.completed_bytes_before = report.completed_bytes_before.saturating_add(bytes);

            if protected {
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
            completed_sessions.push((session, completion.session_id, completed_at, bytes));
        }

        completed_sessions.sort_by_key(|entry| return Reverse(entry.2));

        let retention = Duration::from_secs(older_than_days.saturating_mul(86_400));
        let cutoff = SystemTime::now()
            .checked_sub(retention)
            .unwrap_or(SystemTime::UNIX_EPOCH);

        report.completed_bytes_after_plan = report.completed_bytes_before;

        for (index, (session, session_id, completed, bytes)) in
            completed_sessions.into_iter().enumerate().rev()
        {
            let over_quota = max_completed_bytes
                .is_some_and(|limit| return report.completed_bytes_after_plan > limit);

            if u64::try_from(index).unwrap_or(u64::MAX) < keep_latest
                || (completed > cutoff && !over_quota)
            {
                continue;
            }

            report.completed_bytes_after_plan =
                report.completed_bytes_after_plan.saturating_sub(bytes);

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

        report.quota_excess_bytes = max_completed_bytes.map_or(0, |limit| {
            return report.completed_bytes_after_plan.saturating_sub(limit);
        });

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

            let Ok(record) = read_registry_record_nofollow(&path) else {
                corrupt_entries = corrupt_entries.saturating_add(1);
                continue;
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
                    // Retention age applies to completed evidence, never to an
                    // authenticated live recorder or its interactive process group.
                    let should_reap = !completion_exists && !is_live;
                    if should_reap {
                        if apply {
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

    let completion = validate_completed_session(&candidate.session)?;
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
