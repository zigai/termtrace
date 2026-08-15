use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, Read as _, Write as _},
    path::{Component, Path, PathBuf},
    str,
};

use nix::{
    errno::Errno,
    fcntl::{RenameFlags, renameat2},
};
use termtrace_model::artifact::bundle::BundleManifest;
use uuid::Uuid;

use crate::{
    auxiliary::bundle::{
        BundleIdentity, MAX_ARCHIVE_EXPANDED_BYTES, MAX_BUNDLE_MANIFEST_BYTES, MAX_BUNDLE_MEMBERS,
        collect_regular_files, create_private_directory, create_private_parents,
        open_regular_nofollow, secure_file, sync_parent, verify,
    },
    storage::{Error, Result},
};

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
    let parent = destination
        .parent()
        .ok_or_else(|| return Error::invalid("archive path has no parent"))?;
    if !parent.is_dir() {
        return Err(Error::invalid("archive parent directory does not exist"));
    }
    let staging = parent.join(format!(".bundle-archive-{}.tmp", Uuid::now_v7()));
    let mut cleanup = Cleanup::file(staging.clone());
    let file = secure_file(&staging)?;
    let mut encoder = zstd::stream::write::Encoder::new(file, 9)
        .map_err(|source| return Error::filesystem(&staging, source))?;
    encoder
        .include_checksum(true)
        .map_err(|source| return Error::filesystem(&staging, source))?;
    let mut files: Vec<PathBuf> = Vec::new();
    collect_regular_files(directory, directory, &mut files)?;
    files.sort();
    for relative in files {
        write_tar_file(&mut encoder, directory, &relative)?;
    }
    encoder
        .write_all(&[0_u8; 1024])
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
    sync_parent(destination)?;
    cleanup.disarm();
    return Ok(());
}

/// Extract and verify a compressed bundle archive in a private temporary directory.
///
/// # Errors
///
/// Returns an error when extraction exceeds safety limits or bundle verification fails.
pub fn verify_archive(archive: &Path, expected: Option<&BundleIdentity>) -> Result<BundleManifest> {
    let parent = archive
        .parent()
        .ok_or_else(|| return Error::invalid("archive path has no parent"))?;
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
        if self.directory {
            let _ = fs_err::remove_dir_all(&self.path);
            return;
        }
        let _ = fs_err::remove_file(&self.path);
    }
}

fn write_tar_file(writer: &mut impl io::Write, root: &Path, relative: &Path) -> Result<()> {
    let path = relative
        .to_str()
        .ok_or_else(|| return Error::invalid("bundle archive paths must be UTF-8"))?;
    let source = root.join(relative);
    let (mut input, size) = open_regular_nofollow(&source)?;
    let mut header = [0_u8; 512];
    write_tar_path(&mut header, path)?;
    write_octal(&mut header, 100, 8, 0o600)?;
    write_octal(&mut header, 108, 8, 0)?;
    write_octal(&mut header, 116, 8, 0)?;
    write_octal(&mut header, 124, 12, size)?;
    write_octal(&mut header, 136, 12, 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| return u64::from(*byte)).sum();
    write_checksum(&mut header, checksum)?;
    writer
        .write_all(&header)
        .map_err(|source_error| return Error::filesystem(&source, source_error))?;
    let copied = io::copy(&mut input, writer)
        .map_err(|source_error| return Error::filesystem(&source, source_error))?;
    if copied != size {
        return Err(Error::invalid("bundle member changed while archiving"));
    }
    let padding = usize::try_from((512 - size % 512) % 512).unwrap_or(511);
    let zeroes = [0_u8; 512];
    let padding_bytes = zeroes
        .get(..padding)
        .ok_or_else(|| return Error::invalid("tar padding exceeds one block"))?;
    writer
        .write_all(padding_bytes)
        .map_err(|source_error| return Error::filesystem(&source, source_error))?;
    return Ok(());
}

pub(super) fn write_tar_path(header: &mut [u8; 512], path: &str) -> Result<()> {
    if path.len() <= 100 {
        header
            .get_mut(..path.len())
            .ok_or_else(|| return Error::invalid("tar member name exceeds its field"))?
            .copy_from_slice(path.as_bytes());
        return Ok(());
    }
    let split = path
        .char_indices()
        .filter(|(_, character)| return *character == '/')
        .map(|(index, _)| return index)
        .rev()
        .find(|index| return *index <= 155 && path.len().saturating_sub(*index + 1) <= 100)
        .ok_or_else(|| return Error::invalid("bundle path exceeds USTAR limits"))?;
    let (prefix, name) = path.split_at(split);
    let name = name.trim_start_matches('/');
    header
        .get_mut(..name.len())
        .ok_or_else(|| return Error::invalid("tar member name exceeds its field"))?
        .copy_from_slice(name.as_bytes());
    header
        .get_mut(345..345 + prefix.len())
        .ok_or_else(|| return Error::invalid("tar member prefix exceeds its field"))?
        .copy_from_slice(prefix.as_bytes());
    return Ok(());
}

pub(super) fn write_octal(
    header: &mut [u8; 512],
    offset: usize,
    width: usize,
    value: u64,
) -> Result<()> {
    let encoded = format!("{:0width$o}\0", value, width = width.saturating_sub(1));
    let target = header
        .get_mut(offset..offset.saturating_add(width))
        .ok_or_else(|| return Error::invalid("tar numeric header overflowed"))?;
    if encoded.len() != width {
        return Err(Error::invalid("tar numeric value exceeds field width"));
    }
    target.copy_from_slice(encoded.as_bytes());
    return Ok(());
}

pub(super) fn write_checksum(header: &mut [u8; 512], checksum: u64) -> Result<()> {
    let encoded = format!("{checksum:06o}\0 ");
    if encoded.len() != 8 {
        return Err(Error::invalid("tar checksum exceeds field width"));
    }
    header[148..156].copy_from_slice(encoded.as_bytes());
    return Ok(());
}

pub(super) fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    let (file, _) = open_regular_nofollow(archive)?;
    let mut decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|source| return Error::filesystem(archive, source))?;
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut total_size = 0_u64;
    let maximum_archive_members = u64::try_from(MAX_BUNDLE_MEMBERS)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    for entry_count in 0..=maximum_archive_members {
        let mut header = [0_u8; 512];
        decoder
            .read_exact(&mut header)
            .map_err(|source| return Error::filesystem(archive, source))?;
        if header.iter().all(|byte| return *byte == 0) {
            let mut terminator = [0_u8; 512];
            decoder
                .read_exact(&mut terminator)
                .map_err(|source| return Error::filesystem(archive, source))?;
            if !terminator.iter().all(|byte| return *byte == 0) {
                return Err(Error::invalid("tar archive has an invalid terminator"));
            }
            let mut trailing = [0_u8; 1];
            let trailing_bytes = decoder
                .read(&mut trailing)
                .map_err(|source| return Error::filesystem(archive, source))?;
            if trailing_bytes != 0 {
                return Err(Error::invalid(
                    "tar archive contains trailing decoded content",
                ));
            }
            return Ok(());
        }
        if entry_count == maximum_archive_members {
            return Err(Error::invalid("bundle archive has too many members"));
        }
        verify_tar_checksum(&header)?;
        let path = tar_path(&header)?;
        validate_archive_path(&path)?;
        if !paths.insert(path.clone()) {
            return Err(Error::invalid("bundle archive contains a duplicate path"));
        }
        let type_flag = header[156];
        if type_flag != b'0' && type_flag != 0 {
            return Err(Error::invalid(
                "bundle archive contains a non-regular member",
            ));
        }
        let size = parse_tar_octal(&header[124..136])?;
        if path == Path::new("manifest.json") && size > MAX_BUNDLE_MANIFEST_BYTES {
            return Err(Error::invalid("bundle manifest exceeds its byte limit"));
        }
        total_size = total_size
            .checked_add(size)
            .ok_or_else(|| return Error::invalid("bundle archive size overflow"))?;
        if total_size > MAX_ARCHIVE_EXPANDED_BYTES {
            return Err(Error::invalid("bundle archive expands beyond 8 GiB"));
        }
        create_private_parents(destination, &path)?;
        let target = destination.join(&path);
        let mut writer = secure_file(&target)?;
        let copied = io::copy(&mut decoder.by_ref().take(size), &mut writer)
            .map_err(|source| return Error::filesystem(&target, source))?;
        if copied != size {
            return Err(Error::invalid("bundle archive member is truncated"));
        }
        writer
            .sync_all()
            .map_err(|source| return Error::filesystem(&target, source))?;
        let padding = usize::try_from((512 - size % 512) % 512).unwrap_or(511);
        let mut padding_bytes = [0_u8; 512];
        let padding_slice = padding_bytes
            .get_mut(..padding)
            .ok_or_else(|| return Error::invalid("tar padding exceeds one block"))?;
        decoder
            .read_exact(padding_slice)
            .map_err(|source| return Error::filesystem(archive, source))?;
        if padding_slice.iter().any(|byte| return *byte != 0) {
            return Err(Error::invalid("tar member padding is nonzero"));
        }
    }
    return Err(Error::invalid("bundle archive has no terminator"));
}

fn tar_path(header: &[u8; 512]) -> Result<PathBuf> {
    let name = tar_text(&header[..100])?;
    let prefix = tar_text(&header[345..500])?;
    if name.is_empty() {
        return Err(Error::invalid("tar member name is empty"));
    }
    return Ok(if prefix.is_empty() {
        PathBuf::from(name)
    } else {
        PathBuf::from(prefix).join(name)
    });
}

fn tar_text(field: &[u8]) -> Result<&str> {
    let length = field
        .iter()
        .position(|byte| return *byte == 0)
        .unwrap_or(field.len());
    let text = field
        .get(..length)
        .ok_or_else(|| return Error::invalid("tar text field length is invalid"))?;
    return str::from_utf8(text).map_err(|_| return Error::invalid("tar paths must be UTF-8"));
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

fn parse_tar_octal(field: &[u8]) -> Result<u64> {
    let value = tar_text(field)?.trim();
    if value.is_empty() {
        return Ok(0);
    }
    return u64::from_str_radix(value, 8)
        .map_err(|_| return Error::invalid("tar numeric field is invalid"));
}

fn verify_tar_checksum(header: &[u8; 512]) -> Result<()> {
    let expected = parse_tar_octal(&header[148..156])?;
    let actual = header.iter().enumerate().fold(0_u64, |sum, (index, byte)| {
        return sum.saturating_add(if (148..156).contains(&index) {
            u64::from(b' ')
        } else {
            u64::from(*byte)
        });
    });
    if actual != expected {
        return Err(Error::invalid("tar header checksum is invalid"));
    }
    return Ok(());
}
