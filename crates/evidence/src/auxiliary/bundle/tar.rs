use crate::{
    auxiliary::bundle::{
        collect_regular_files, verify, BundleIdentity, MAX_ARCHIVE_EXPANDED_BYTES,
        MAX_BUNDLE_MANIFEST_BYTES, MAX_BUNDLE_MEMBERS,
    },
    storage::{
        artifact::{
            create_private_directory, open_regular_file_nofollow, secure_file,
            sync_parent_directory,
        },
        Error, Result,
    },
};
use nix::{
    errno::Errno,
    fcntl::{renameat2, RenameFlags},
};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs::File,
    io::{self, Read as _},
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
    str,
};
use tar_archive::{Archive, Builder, EntryType, Header};
use termtrace_model::artifact::bundle::BundleManifest;
use uuid::Uuid;

/// Verify a bundle directory and publish it as a deterministic compressed tar archive.
///
/// # Errors
///
/// Returns an error when verification fails, the destination exists, or archival cannot be
/// completed durably.
pub fn archive(
    directory: &Path,
    destination: &Path,
    expected: Option<&BundleIdentity>,
) -> Result<()> {
    verify(directory, expected)?;

    if destination.exists() {
        return Err(Error::DestinationExists {
            path: destination.to_path_buf(),
        });
    }

    let parent = match destination.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        Some(_) => Path::new("."),
        None => return Err(Error::invalid("archive path has no parent")),
    };

    if !parent.is_dir() {
        return Err(Error::invalid("archive parent directory does not exist"));
    }

    let staging = parent.join(format!(".bundle-archive-{}.tmp", Uuid::now_v7()));
    let mut cleanup = Cleanup::file(staging.clone());
    let file = secure_file(&staging)?;
    let encoder = zstd::stream::write::Encoder::new(file, 9)
        .map_err(|source| return Error::filesystem(&staging, source))?;
    let mut builder = Builder::new(encoder);
    builder
        .get_mut()
        .include_checksum(true)
        .map_err(|source| return Error::filesystem(&staging, source))?;

    let mut files: Vec<PathBuf> = Vec::new();
    collect_regular_files(directory, directory, &mut files)?;
    files.sort();

    for relative in files {
        write_tar_file(&mut builder, directory, &relative)?;
    }

    builder
        .finish()
        .map_err(|source| return Error::filesystem(&staging, source))?;
    let encoder = builder
        .into_inner()
        .map_err(|source| return Error::filesystem(&staging, source))?;
    let file = encoder
        .finish()
        .map_err(|source| return Error::filesystem(&staging, source))?;
    file.sync_all()
        .map_err(|source| return Error::filesystem(&staging, source))?;

    let staging_name = staging
        .file_name()
        .ok_or_else(|| return Error::invalid("archive staging path has no basename"))?;

    let destination_name = destination
        .file_name()
        .ok_or_else(|| return Error::invalid("archive path has no basename"))?;

    let parent_file =
        File::open(parent).map_err(|source| return Error::filesystem(parent, source))?;
    renameat2(
        &parent_file,
        staging_name,
        &parent_file,
        destination_name,
        RenameFlags::RENAME_NOREPLACE,
    )
    .map_err(|error| {
        if error == Errno::EEXIST {
            return Error::DestinationExists {
                path: destination.to_path_buf(),
            };
        }
        return Error::filesystem(destination, io::Error::from_raw_os_error(error as i32));
    })?;

    sync_parent_directory(destination)?;

    cleanup.disarm();
    return Ok(());
}

/// Extract and verify a compressed bundle archive in a private temporary directory.
///
/// # Errors
///
/// Returns an error when extraction exceeds safety limits or bundle verification fails.
pub fn verify_archive(archive: &Path, expected: Option<&BundleIdentity>) -> Result<BundleManifest> {
    let parent = match archive.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        Some(_) => Path::new("."),
        None => return Err(Error::invalid("archive path has no parent")),
    };

    let staging = parent.join(format!(".bundle-verify-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let _cleanup = Cleanup::directory(staging.clone());
    extract_archive(archive, &staging)?;

    return verify(&staging, expected);
}

struct Cleanup {
    path: PathBuf,
    directory: bool,
    armed: bool,
}

impl Cleanup {
    fn file(path: PathBuf) -> Self {
        return Self {
            path,
            directory: false,
            armed: true,
        };
    }

    fn directory(path: PathBuf) -> Self {
        return Self {
            path,
            directory: true,
            armed: true,
        };
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        // Drop has no error channel, so removing an armed partial output is best-effort.
        if self.directory {
            drop(fs_err::remove_dir_all(&self.path));

            return;
        }

        drop(fs_err::remove_file(&self.path));
    }
}

fn write_tar_file<W: io::Write>(
    builder: &mut Builder<W>,
    root: &Path,
    relative: &Path,
) -> Result<()> {
    let path = relative
        .to_str()
        .ok_or_else(|| return Error::invalid("bundle archive paths must be UTF-8"))?;

    let source = root.join(relative);
    let (mut input, size) = open_regular_file_nofollow(&source)?;
    let mut header = Header::new_ustar();
    header
        .set_path(path)
        .map_err(|source_error| return Error::filesystem(&source, source_error))?;
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_size(size);
    header.set_mtime(0);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();

    let read_limit = size
        .checked_add(1)
        .ok_or_else(|| return Error::invalid("bundle member size overflow"))?;
    let mut contents = input.by_ref().take(read_limit);
    builder
        .append(&header, &mut contents)
        .map_err(|source_error| return Error::filesystem(&source, source_error))?;
    let copied = read_limit.saturating_sub(contents.limit());

    if copied != size {
        return Err(Error::invalid("bundle member changed while archiving"));
    }
    return Ok(());
}

#[derive(Default)]
struct TarReadState {
    position: u64,
    padding: Option<Range<u64>>,
}

#[derive(Clone)]
struct TarReadTracker(Rc<RefCell<TarReadState>>);

impl TarReadTracker {
    fn new() -> Self {
        return Self(Rc::new(RefCell::new(TarReadState::default())));
    }

    fn register_padding(&self, file_position: u64, size: u64) -> Result<()> {
        let padding_start = file_position
            .checked_add(size)
            .ok_or_else(|| return Error::invalid("bundle archive size overflow"))?;

        let padded_size = size
            .checked_add(511)
            .ok_or_else(|| return Error::invalid("bundle archive size overflow"))?
            & !511;

        let padding_end = file_position
            .checked_add(padded_size)
            .ok_or_else(|| return Error::invalid("bundle archive size overflow"))?;

        if padding_start == padding_end {
            return Ok(());
        }

        let mut state = self.0.borrow_mut();

        if state.padding.is_some() || state.position > padding_start {
            return Err(Error::invalid("tar archive member positions are invalid"));
        }

        state.padding = Some(padding_start..padding_end);
        return Ok(());
    }
}

struct PaddingCheckedReader<R> {
    inner: R,
    tracker: TarReadTracker,
}

impl<R: io::Read> io::Read for PaddingCheckedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        let mut state = self.tracker.0.borrow_mut();
        let read_start = state.position;
        let read_end = read_start
            .checked_add(u64::try_from(read).map_err(|source| {
                return io::Error::new(io::ErrorKind::InvalidData, source);
            })?)
            .ok_or_else(|| {
                return io::Error::new(io::ErrorKind::InvalidData, "tar position overflow");
            })?;
        state.position = read_end;

        let Some(padding) = state.padding.clone() else {
            return Ok(read);
        };
        let overlap_start = read_start.max(padding.start);
        let overlap_end = read_end.min(padding.end);

        if overlap_start < overlap_end {
            let start = usize::try_from(overlap_start - read_start).map_err(|source| {
                return io::Error::new(io::ErrorKind::InvalidData, source);
            })?;
            let end = usize::try_from(overlap_end - read_start).map_err(|source| {
                return io::Error::new(io::ErrorKind::InvalidData, source);
            })?;
            let overlap = buf.get(start..end).ok_or_else(|| {
                return io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tar padding range exceeds decoded input",
                );
            })?;

            if overlap.iter().any(|byte| return *byte != 0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "tar archive member has nonzero padding",
                ));
            }
        }

        if read_end >= padding.end {
            state.padding = None;
        }

        return Ok(read);
    }
}

pub(super) fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    let (file, _) = open_regular_file_nofollow(archive)?;
    let decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|source| return Error::filesystem(archive, source))?;

    let tracker = TarReadTracker::new();
    let reader = PaddingCheckedReader {
        inner: decoder,
        tracker: tracker.clone(),
    };
    let mut tar_archive = Archive::new(reader);
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut total_size = 0_u64;

    {
        let entries = tar_archive
            .entries()
            .map_err(|source| return Error::filesystem(archive, source))?
            .raw(true);

        for (entry_count, entry) in entries.enumerate() {
            if entry_count >= MAX_BUNDLE_MEMBERS {
                return Err(Error::invalid("bundle archive has too many members"));
            }

            let mut entry = entry.map_err(|source| {
                return Error::invalid(format!("tar archive is invalid: {source}"));
            })?;

            if !entry.header().entry_type().is_file() {
                return Err(Error::invalid(
                    "bundle archive contains a non-regular member",
                ));
            }

            tracker.register_padding(entry.raw_file_position(), entry.size())?;
            let path_bytes = entry.path_bytes();
            let path_text = str::from_utf8(path_bytes.as_ref()).map_err(|source| {
                return Error::invalid_with_source("tar paths must be UTF-8", source);
            })?;

            let path = PathBuf::from(path_text);
            validate_archive_path(&path)?;
            if !paths.insert(path.clone()) {
                return Err(Error::invalid("bundle archive contains a duplicate path"));
            }

            let size = entry.size();

            if path == Path::new("manifest.json") && size > MAX_BUNDLE_MANIFEST_BYTES {
                return Err(Error::invalid("bundle manifest exceeds its byte limit"));
            }

            total_size = total_size
                .checked_add(size)
                .ok_or_else(|| return Error::invalid("bundle archive size overflow"))?;

            if total_size > MAX_ARCHIVE_EXPANDED_BYTES {
                return Err(Error::invalid("bundle archive expands beyond 8 GiB"));
            }

            let target = destination.join(&path);
            let mut writer = secure_file(&target)?;
            let copied = io::copy(&mut entry, &mut writer)
                .map_err(|source| return Error::filesystem(&target, source))?;

            if copied != size {
                return Err(Error::invalid("bundle archive member is truncated"));
            }

            writer
                .sync_all()
                .map_err(|source| return Error::filesystem(&target, source))?;
        }
    }

    let mut reader = tar_archive.into_inner();
    let mut terminator = [0_u8; 512];
    reader
        .read_exact(&mut terminator)
        .map_err(|source| return Error::filesystem(archive, source))?;

    if !terminator.iter().all(|byte| return *byte == 0) {
        return Err(Error::invalid("tar archive has an invalid terminator"));
    }

    let mut trailing = [0_u8; 1];
    let trailing_bytes = reader
        .read(&mut trailing)
        .map_err(|source| return Error::filesystem(archive, source))?;

    if trailing_bytes != 0 {
        return Err(Error::invalid(
            "tar archive contains trailing decoded content",
        ));
    }
    return Ok(());
}

fn validate_archive_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| return !matches!(component, Component::Normal(_)))
    {
        return Err(Error::invalid("bundle archive member path is unsafe"));
    }
    return Ok(());
}
