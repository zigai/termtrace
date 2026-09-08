use std::{
    env::temp_dir,
    fs,
    io::{self, Read as _, Write as _},
    os::unix::fs::PermissionsExt as _,
    path::{absolute, Path},
};

use termtrace_model::execution::event_log::RecoveryReport;

use crate::storage::{
    artifact::{
        commit_directory, open_regular_file_nofollow, secure_file, sync_directory, write_secure,
    },
    event_log::{EventLog, EventLogCursor, EventLogSummary},
    Error, Result,
};

use super::{
    io::{manifest_hash, read_manifest},
    validation::{validate_manifest, validate_replay_profile_file},
};

/// Owned, verified disk snapshot of a finite canonical prefix, without a completion claim.
pub struct SessionSnapshot {
    directory: tempfile::TempDir,
}

impl SessionSnapshot {
    /// Freeze a source's current byte boundary, optionally limited to an actor's durable event count.
    ///
    /// # Errors
    /// Rejects malformed complete records, unavailable prefixes, changing context, and unsafe files.
    pub fn capture(source: &Path, event_count: Option<u64>) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("termtrace-prefix-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .map_err(|error| return Error::filesystem(temp_dir(), error))?;

        copy_context_file(
            &source.join("session.json"),
            &directory.path().join("session.json"),
        )?;

        let manifest = read_manifest(directory.path())?;
        validate_manifest(&manifest)?;

        if let Some(profile) = &manifest.ghostty_visual {
            copy_context_file(
                &source.join(&profile.replay_profile),
                &directory.path().join(&profile.replay_profile),
            )?;

            validate_replay_profile_file(directory.path(), &manifest)?;
        }

        let manifest_path = directory.path().join("session.json");
        let manifest_bytes = fs::read(&manifest_path)
            .map_err(|error| return Error::filesystem(&manifest_path, error))?;

        let mut cursor = EventLogCursor::open_snapshot(source.join("events.jsonl"))?;

        if cursor.header().session_id != manifest.session_id
            || cursor.header().session_manifest_hash != manifest_hash(&manifest_bytes)
        {
            return Err(Error::invalid(
                "snapshot header does not bind the session manifest",
            ));
        }

        let path = directory.path().join("events.jsonl");
        let mut output = io::BufWriter::new(secure_file(&path)?);
        write_snapshot_record(&path, &mut output, cursor.header())?;
        while event_count.is_none_or(|count| return cursor.next_event_seq() < count) {
            let Some(record) = cursor.next_record()? else {
                break;
            };
            write_snapshot_record(&path, &mut output, &record)?;
        }

        if event_count.is_some_and(|count| return cursor.next_event_seq() != count) {
            return Err(Error::invalid("requested snapshot prefix is unavailable"));
        }

        let _pending = cursor.finish_snapshot()?;
        output
            .flush()
            .map_err(|error| return Error::filesystem(&path, error))?;

        return Ok(Self { directory });
    }

    /// Private snapshot directory; valid until this owner is dropped.
    pub fn path(&self) -> &Path {
        return self.directory.path();
    }
}

fn write_snapshot_record(
    path: &Path,
    output: &mut impl io::Write,
    record: &impl serde::Serialize,
) -> Result<()> {
    serde_json::to_writer(&mut *output, record).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    output
        .write_all(b"\n")
        .map_err(|error| return Error::filesystem(path, error))?;

    return Ok(());
}

/// Recover a verifiable prefix and its immutable session context into a new directory.
///
/// Publication is atomic and never replaces existing evidence. The destination
/// intentionally has no completion record: recovery cannot establish that no
/// output was lost, even if an exit event survived. Use explicit prefix analysis.
/// Source evidence and derived artifacts are never modified or copied wholesale.
///
/// # Errors
///
/// Returns an error for invalid manifest/header bindings, unsafe files, an existing
/// destination, or failure to recover and durably publish the validated prefix.
pub fn recover_session(source: &Path, destination: &Path) -> Result<RecoveryReport> {
    let destination =
        absolute(destination).map_err(|error| return Error::filesystem(destination, error))?;

    let parent = destination
        .parent()
        .ok_or_else(|| return Error::invalid("recovery destination has no parent"))?;

    let staging = tempfile::Builder::new()
        .prefix(".termtrace-recovery-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .map_err(|error| return Error::filesystem(parent, error))?;

    copy_context_file(
        &source.join("session.json"),
        &staging.path().join("session.json"),
    )?;

    let manifest = read_manifest(staging.path())?;
    validate_manifest(&manifest)?;

    if let Some(profile) = &manifest.ghostty_visual {
        copy_context_file(
            &source.join(&profile.replay_profile),
            &staging.path().join(&profile.replay_profile),
        )?;

        validate_replay_profile_file(staging.path(), &manifest)?;
    }

    let mut report = EventLog::recover_prefix(
        source.join("events.jsonl"),
        staging.path().join("events.jsonl"),
    )?;

    let summary = EventLogSummary::read(staging.path().join("events.jsonl"))?;
    let manifest_bytes = fs::read(staging.path().join("session.json"))
        .map_err(|error| return Error::filesystem(staging.path().join("session.json"), error))?;

    if summary.header.session_id != manifest.session_id
        || summary.header.session_manifest_hash != manifest_hash(&manifest_bytes)
    {
        return Err(Error::invalid(
            "recovered event header does not bind the source session manifest",
        ));
    }

    report.recovered = destination.join("events.jsonl");
    let recovery_path = staging.path().join("recovery.json");
    let bytes = serde_json::to_vec(&serde_json::json!({
        "format": "termtrace-session-recovery",
        "version": 1,
        "complete_evidence": false,
        "source_session": source,
        "session_id": manifest.session_id,
        "session_manifest_hash": summary.header.session_manifest_hash,
        "recovery": report,
    }))
    .map_err(|source| {
        return Error::InvalidJson {
            path: recovery_path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    write_secure(&recovery_path, &bytes)?;

    sync_directory(staging.path())?;
    commit_directory(parent, staging.path(), &destination)?;

    return Ok(report);
}

fn copy_context_file(source: &Path, destination: &Path) -> Result<()> {
    let (input, size) = open_regular_file_nofollow(source)?;
    if size > 4 * 1024 * 1024 {
        return Err(Error::invalid("recovery context file exceeds 4 MiB"));
    }

    let mut output = secure_file(destination)?;
    let copied = io::copy(&mut input.take(4 * 1024 * 1024 + 1), &mut output)
        .map_err(|error| return Error::filesystem(destination, error))?;

    if copied > 4 * 1024 * 1024 {
        return Err(Error::invalid("recovery context file grew beyond 4 MiB"));
    }

    output
        .sync_all()
        .map_err(|error| return Error::filesystem(destination, error))?;

    return Ok(());
}
