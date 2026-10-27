use super::{
    io::{manifest_hash, read_manifest},
    validation::{validate_manifest, validate_replay_profile_file},
};
use crate::storage::{
    artifact::{
        commit_directory, open_regular_file_nofollow, secure_file, sync_directory, write_secure,
    },
    event_log::{EventLog, EventLogCursor, EventLogSummary},
    Error, Result,
};
use serde::Serialize;
use std::{
    env::temp_dir,
    fs,
    io::{self, Read as _, Write as _},
    os::unix::fs::PermissionsExt as _,
    path::{absolute, Path},
};
use termtrace_model::execution::{
    event_log::RecoveryReport, layout::SessionLayout, session::SessionManifest,
};
use uuid::Uuid;

#[derive(Serialize)]
struct RecoveryDiagnostic<'report> {
    format: &'static str,
    version: u32,
    complete_evidence: bool,
    #[serde(with = "termtrace_model::value::exact_path")]
    source_session: &'report Path,
    session_id: Uuid,
    session_manifest_hash: &'report str,
    recovery: &'report RecoveryReport,
}

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

        let (manifest, manifest_hash) = stage_context(source, directory.path())?;
        let mut cursor = EventLogCursor::open_snapshot(SessionLayout::new(source).event_log())?;

        if cursor.header().session_id != manifest.session_id
            || cursor.header().session_manifest_hash != manifest_hash
        {
            return Err(Error::invalid(
                "snapshot header does not bind the session manifest",
            ));
        }

        let path = SessionLayout::new(directory.path()).event_log();
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

    // A nested destination would publish new files into the canonical source
    // tree, even without overwriting its existing records. Resolve parent links
    // before staging so aliases cannot bypass the non-destructive boundary.
    let canonical_source =
        fs::canonicalize(source).map_err(|error| return Error::filesystem(source, error))?;

    let canonical_parent =
        fs::canonicalize(parent).map_err(|error| return Error::filesystem(parent, error))?;

    if canonical_parent.starts_with(&canonical_source) {
        return Err(Error::invalid(
            "recovery destination cannot be inside the source session",
        ));
    }

    let staging = tempfile::Builder::new()
        .prefix(".termtrace-recovery-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .map_err(|error| return Error::filesystem(parent, error))?;

    let (manifest, manifest_hash) = stage_context(source, staging.path())?;
    let staged = SessionLayout::new(staging.path());
    let mut report =
        EventLog::recover_prefix(SessionLayout::new(source).event_log(), staged.event_log())?;

    let summary = EventLogSummary::read(staged.event_log())?;
    if summary.header.session_id != manifest.session_id
        || summary.header.session_manifest_hash != manifest_hash
    {
        return Err(Error::invalid(
            "recovered event header does not bind the source session manifest",
        ));
    }

    report.recovered = SessionLayout::new(&destination).event_log();
    let recovery_path = staged.recovery_diagnostic();
    let diagnostic = RecoveryDiagnostic {
        format: "termtrace-session-recovery",
        version: 1,
        complete_evidence: false,
        source_session: source,
        session_id: manifest.session_id,
        session_manifest_hash: &summary.header.session_manifest_hash,
        recovery: &report,
    };
    // Preserve the historical sorted JSON object bytes for UTF8 paths, but never
    // use json!'s infallible conversion for filesystem paths.
    let bytes = serde_json::to_value(&diagnostic)
        .and_then(|value| return serde_json::to_vec(&value))
        .map_err(|source| {
            return Error::InvalidJson {
                path: recovery_path.clone(),
                json_path: "$".to_owned(),
                source,
            };
        })?;

    write_secure(&recovery_path, &bytes)?;

    sync_directory(staging.path())?;
    commit_directory(parent, staging.path(), &destination, false)?;

    return Ok(report);
}

/// Copy and validate the immutable session context, returning the manifest and its hash.
fn stage_context(source: &Path, destination: &Path) -> Result<(SessionManifest, String)> {
    let source = SessionLayout::new(source);
    let staged = SessionLayout::new(destination);
    copy_context_file(&source.manifest(), &staged.manifest())?;

    let manifest = read_manifest(destination)?;
    validate_manifest(&manifest)?;
    if manifest.ghostty_visual.is_some() {
        copy_context_file(
            &source.ghostty_replay_profile(),
            &staged.ghostty_replay_profile(),
        )?;

        validate_replay_profile_file(destination, &manifest)?;
    }

    let manifest_bytes = fs::read(staged.manifest())
        .map_err(|error| return Error::filesystem(staged.manifest(), error))?;

    return Ok((manifest, manifest_hash(&manifest_bytes)));
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
