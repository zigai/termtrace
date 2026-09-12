//! Aggregate exports with a bounded manifest and a streamed track inventory.

use super::{
    bundle, commit_directory, create_private_directory, parse_aggregate, read_regular_bounded,
    sync_directory, tmux_journal, track_directory, validate_aggregate, validate_destination,
    visit_source_tracks, write_secure, BundleProfile, BundleVerificationPolicy, ByteString,
    DirectoryCleanup, Error, Result, TmuxBundleTrack, TmuxRecordingManifest, Uuid,
    MAX_AGGREGATE_MANIFEST_BYTES,
};
use std::os::unix::ffi::OsStrExt as _;
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    path::Path,
};
use termtrace_evidence::artifact::{
    finish_writer, hash_regular_file, open_regular_file_nofollow, secure_writer,
};
use termtrace_model::artifact::tmux::TmuxStreamingBundleManifest;

const FORMAT: &str = "termtrace-tmux-streaming-bundle";
const VERSION: u32 = 1;
const INVENTORY: &str = "tracks.jsonl";
const MAX_ROW_BYTES: u64 = 1024 * 1024;

#[cfg(test)]
mod tests;

/// Export all historical tracks with bounded memory regardless of track count.
///
/// # Errors
/// Rejects incomplete recordings, source mismatches, unsafe paths, and I/O failures.
pub fn create(
    aggregate_manifest: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<TmuxStreamingBundleManifest> {
    let aggregate_manifest = aggregate_manifest.as_ref();
    let destination = destination.as_ref();

    if destination.exists() {
        return Err(Error::invalid("tmux export destination already exists"));
    }

    let bytes = read_regular_bounded(
        aggregate_manifest,
        MAX_AGGREGATE_MANIFEST_BYTES,
        "aggregate manifest",
    )?;

    let aggregate = parse_aggregate(aggregate_manifest, &bytes)?;
    validate_aggregate(&aggregate)?;

    validate_destination(destination, aggregate_manifest, &[])?;

    // Validate every source before creating a destination that might lie inside it.
    visit_source_tracks(aggregate_manifest, &aggregate, |source| {
        return validate_destination(destination, aggregate_manifest, &[source]);
    })?;

    let parent = match destination.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        Some(_) => Path::new("."),
        None => return Err(Error::invalid("export destination has no parent")),
    };

    let staging = parent.join(format!(".tmux-export-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let cleanup = DirectoryCleanup::new(staging.clone());
    write_secure(&staging.join("aggregate-recording-manifest.json"), &bytes)?;

    if let Some(boundary) = &aggregate.lifecycle_journal {
        let source = aggregate_manifest
            .parent()
            .ok_or_else(|| return Error::invalid("aggregate has no parent"))?;

        tmux_journal::copy_committed(
            &source.join(tmux_journal::JOURNAL_NAME),
            &staging.join("aggregate-lifecycle.jsonl"),
            boundary,
        )?;
    }

    create_private_directory(&staging.join("tracks"))?;
    let inventory_path = staging.join(INVENTORY);
    let mut writer = secure_writer(&inventory_path)?;
    let mut count = 0_usize;

    visit_source_tracks(aggregate_manifest, &aggregate, |source| {
        let directory = track_directory(count, source.pane_id.as_deref());
        let nested = bundle::create(
            &source.session,
            &staging.join(&directory),
            BundleProfile::Forensic,
            None,
        )?;

        if nested.session_id != source.session_id {
            return Err(Error::invalid(
                "historical track identity differs from its canonical session",
            ));
        }

        let track = TmuxBundleTrack {
            role: source.role.to_owned(),
            pane_id: source.pane_id,
            session_id: nested.session_id,
            canonical_session: ByteString::from(source.session.as_os_str().as_bytes()),
            directory,
            source_final_event_hash: nested.source_final_event_hash,
        };
        let mut row = serde_json::to_vec(&track)
            .map_err(|error| return Error::json(&inventory_path, error))?;
        row.push(b'\n');
        if u64::try_from(row.len()).unwrap_or(u64::MAX) > MAX_ROW_BYTES {
            return Err(Error::invalid(
                "tmux track inventory row exceeds its byte budget",
            ));
        }

        writer
            .write_all(&row)
            .map_err(|error| return Error::filesystem(&inventory_path, error))?;

        count = count
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("tmux track count overflow"))?;

        return Ok(());
    })?;

    finish_writer(writer, &inventory_path)?;
    let (inventory_bytes, inventory_hash) = hash_regular_file(&inventory_path)?;
    let manifest = TmuxStreamingBundleManifest {
        format: FORMAT.to_owned(),
        version: VERSION,
        recording_id: aggregate.recording_id,
        recording_name: aggregate.name.clone(),
        aggregate_manifest: ByteString::from(aggregate_manifest.as_os_str().as_bytes()),
        aggregate_manifest_blake3: blake3::hash(&bytes).to_hex().to_string(),
        track_count: u64::try_from(count)
            .map_err(|_| return Error::invalid("tmux track count overflow"))?,
        track_inventory_bytes: inventory_bytes,
        track_inventory_blake3: inventory_hash,
    };

    let path = staging.join("manifest.json");
    write_secure(
        &path,
        &serde_json::to_vec_pretty(&manifest).map_err(|error| return Error::json(&path, error))?,
    )?;

    verify_contents(&staging, aggregate_manifest, &bytes, &aggregate)?;
    sync_directory(&staging)?;
    commit_directory(parent, &staging, destination)?;

    cleanup.disarm();

    return Ok(manifest);
}

/// Verify every exported track against the caller-owned aggregate and canonical sessions.
///
/// # Errors
/// Rejects missing, extra, changed, or reordered tracks and source mismatches.
pub fn verify(
    directory: impl AsRef<Path>,
    aggregate_manifest: impl AsRef<Path>,
) -> Result<TmuxStreamingBundleManifest> {
    let path = aggregate_manifest.as_ref();
    let bytes = read_regular_bounded(path, MAX_AGGREGATE_MANIFEST_BYTES, "aggregate manifest")?;
    let aggregate = parse_aggregate(path, &bytes)?;
    validate_aggregate(&aggregate)?;

    return verify_contents(directory.as_ref(), path, &bytes, &aggregate);
}

fn verify_contents(
    directory: &Path,
    aggregate_path: &Path,
    aggregate_bytes: &[u8],
    aggregate: &TmuxRecordingManifest,
) -> Result<TmuxStreamingBundleManifest> {
    let path = directory.join("manifest.json");
    let manifest_bytes =
        read_regular_bounded(&path, MAX_ROW_BYTES, "streaming tmux bundle manifest")?;
    let manifest: TmuxStreamingBundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| return Error::json(&path, error))?;

    if manifest.format != FORMAT
        || manifest.version != VERSION
        || manifest.recording_id != aggregate.recording_id
        || manifest.recording_name != aggregate.name
        || manifest.aggregate_manifest != ByteString::from(aggregate_path.as_os_str().as_bytes())
        || manifest.aggregate_manifest_blake3 != blake3::hash(aggregate_bytes).to_hex().as_str()
    {
        return Err(Error::invalid(
            "streaming tmux export does not bind the requested aggregate",
        ));
    }

    let embedded = read_regular_bounded(
        &directory.join("aggregate-recording-manifest.json"),
        MAX_AGGREGATE_MANIFEST_BYTES,
        "embedded aggregate",
    )?;

    if embedded != aggregate_bytes {
        return Err(Error::invalid(
            "embedded aggregate differs from the trusted source",
        ));
    }

    if let Some(boundary) = &aggregate.lifecycle_journal {
        let journal = directory.join("aggregate-lifecycle.jsonl");
        let (_, length) = open_regular_file_nofollow(&journal)?;

        if length != boundary.bytes {
            return Err(Error::invalid(
                "aggregate journal has an unexpected suffix or truncation",
            ));
        }

        tmux_journal::visit(&journal, boundary, |_| return Ok(()))?;
    }

    let inventory = directory.join(INVENTORY);
    let (bytes, hash) = hash_regular_file(&inventory)?;

    if bytes != manifest.track_inventory_bytes || hash != manifest.track_inventory_blake3 {
        return Err(Error::invalid(
            "tmux track inventory digest or length changed",
        ));
    }

    let (file, _) = open_regular_file_nofollow(&inventory)?;
    let mut reader = BufReader::new(file);
    let mut count = 0_usize;
    let mut hasher = blake3::Hasher::new();

    visit_source_tracks(aggregate_path, aggregate, |source| {
        let mut row = Vec::new();
        reader
            .by_ref()
            .take(MAX_ROW_BYTES + 1)
            .read_until(b'\n', &mut row)
            .map_err(|error| return Error::filesystem(&inventory, error))?;

        if row.last() != Some(&b'\n')
            || u64::try_from(row.len()).unwrap_or(u64::MAX) > MAX_ROW_BYTES
        {
            return Err(Error::invalid(
                "missing, incomplete, or oversized tmux inventory row",
            ));
        }

        hasher.update(&row);

        let track: TmuxBundleTrack =
            serde_json::from_slice(&row).map_err(|error| return Error::json(&inventory, error))?;

        let expected = track_directory(count, source.pane_id.as_deref());
        if track.role != source.role
            || track.pane_id != source.pane_id
            || track.directory != expected
            || track.session_id != source.session_id
            || track.canonical_session != ByteString::from(source.session.as_os_str().as_bytes())
        {
            return Err(Error::invalid(
                "tmux track differs from its historical aggregate relationship",
            ));
        }

        let nested_path = directory.join(&expected);
        if !fs_err::symlink_metadata(&nested_path)
            .map_err(|error| return Error::filesystem(&nested_path, error))?
            .is_dir()
        {
            return Err(Error::invalid("nested track must be a real directory"));
        }

        let nested = bundle::verify_profile(
            &nested_path,
            &BundleVerificationPolicy::new(&source.session, BundleProfile::Forensic),
        )?;

        if nested.session_id != source.session_id
            || nested.source_final_event_hash != track.source_final_event_hash
        {
            return Err(Error::invalid(
                "nested track identity or final hash differs",
            ));
        }

        count = count
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("tmux track count overflow"))?;

        return Ok(());
    })?;

    if !reader
        .fill_buf()
        .map_err(|error| return Error::filesystem(&inventory, error))?
        .is_empty()
        || u64::try_from(count).ok() != Some(manifest.track_count)
        || hasher.finalize().to_hex().as_str() != manifest.track_inventory_blake3
    {
        return Err(Error::invalid(
            "tmux inventory changed or contains extra tracks",
        ));
    }

    verify_directory_entries(
        directory,
        aggregate.lifecycle_journal.is_some(),
        manifest.track_count,
    )?;

    return Ok(manifest);
}

fn verify_directory_entries(directory: &Path, journal: bool, track_count: u64) -> Result<()> {
    for entry in
        fs_err::read_dir(directory).map_err(|error| return Error::filesystem(directory, error))?
    {
        let entry = entry.map_err(|error| return Error::filesystem(directory, error))?;
        let name = entry.file_name();
        let allowed = name == "manifest.json"
            || name == "aggregate-recording-manifest.json"
            || name == INVENTORY
            || (journal && name == "aggregate-lifecycle.jsonl");

        let kind = entry
            .file_type()
            .map_err(|error| return Error::filesystem(entry.path(), error))?;
        #[expect(
            clippy::filetype_is_file,
            reason = "aggregate exports require regular files; symlinks, sockets, FIFOs, and devices are invalid"
        )]
        if !((allowed && kind.is_file()) || (name == "tracks" && kind.is_dir())) {
            return Err(Error::invalid("unexpected entry in aggregate export"));
        }
    }

    let tracks = directory.join("tracks");

    let mut count = 0_u64;
    for entry in
        fs_err::read_dir(&tracks).map_err(|error| return Error::filesystem(&tracks, error))?
    {
        let entry = entry.map_err(|error| return Error::filesystem(&tracks, error))?;
        if !entry
            .file_type()
            .map_err(|error| return Error::filesystem(entry.path(), error))?
            .is_dir()
        {
            return Err(Error::invalid(
                "unexpected entry in aggregate track directory",
            ));
        }

        count = count
            .checked_add(1)
            .ok_or_else(|| return Error::invalid("track directory count overflow"))?;
    }

    if count != track_count {
        return Err(Error::invalid(
            "aggregate export contains extra or missing track directories",
        ));
    }
    return Ok(());
}

#[cfg(test)]
mod directory_tests {
    use super::verify_directory_entries;
    use std::{
        fs,
        os::unix::{fs::symlink, net::UnixListener},
    };

    #[test]
    fn aggregate_entries_require_regular_files() {
        let root = tempfile::tempdir().expect("temporary export");
        fs::create_dir_all(root.path().join("tracks")).expect("tracks directory");
        let manifest = root.path().join("manifest.json");
        fs::write(&manifest, b"{}").expect("regular manifest");

        verify_directory_entries(root.path(), false, 0).expect("regular file accepted");

        fs::remove_file(&manifest).expect("remove regular file");

        let target = tempfile::NamedTempFile::new().expect("external regular file");
        symlink(target.path(), &manifest).expect("manifest symlink");

        assert!(verify_directory_entries(root.path(), false, 0).is_err());

        fs::remove_file(&manifest).expect("remove symlink");

        let _socket = UnixListener::bind(&manifest).expect("manifest socket");

        assert!(verify_directory_entries(root.path(), false, 0).is_err());

        fs::remove_file(&manifest).expect("remove socket path");

        fs::create_dir_all(&manifest).expect("manifest directory");

        assert!(verify_directory_entries(root.path(), false, 0).is_err());
    }
}
