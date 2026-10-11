use crate::storage::{Error, Result};
use nix::{
    errno::Errno,
    fcntl::{open, openat2, renameat2, OFlag, OpenHow, RenameFlags, ResolveFlag},
    sys::stat::{fstat, mkdirat, Mode, SFlag},
    unistd::{fsync, unlinkat, Uid, UnlinkatFlags},
};
use std::{
    ffi::OsStr,
    fs::File,
    io::{BufWriter, Error as IoError, ErrorKind, Write as _},
    os::fd::{AsFd, OwnedFd},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

/// Create a new owner-only buffered artifact writer without replacing an existing file.
///
/// # Errors
///
/// Returns an error when the path is unsafe, the destination exists, or the file cannot be
/// created with owner-only permissions.
pub fn secure_writer(path: &Path) -> Result<BufWriter<File>> {
    return secure_file(path).map(|f| return BufWriter::with_capacity(64 * 1024, f));
}

/// Create a new owner-only artifact file without following symlinks or replacing an existing file.
///
/// # Errors
///
/// Returns an error when the path is unsafe, the destination exists, or the file cannot be
/// created with owner-only permissions.
pub fn secure_file(path: &Path) -> Result<File> {
    let parent = open_parent_directory(path, true)?;
    let leaf = safe_file_name(path)?;
    let file = open_file_at(
        &parent,
        leaf,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
        path,
    )?;

    validate_private_regular_file_fd(&file, path)?;
    sync_fd(
        &parent,
        path.parent().unwrap_or_else(|| return Path::new(".")),
    )?;

    return Ok(File::from(file));
}

/// Create and durably publish a new owner-only artifact directory.
///
/// # Errors
///
/// Returns an error when the path is unsafe, the destination exists, or the directory cannot be
/// created with owner-only permissions.
pub fn create_private_directory(path: &Path) -> Result<()> {
    let parent = open_parent_directory(path, true)?;
    let leaf = safe_file_name(path)?;
    mkdirat(&parent, leaf, Mode::from_bits_truncate(0o700))
        .map_err(|error| return nix_filesystem(path, error))?;

    let directory = open_directory_at(&parent, leaf, path)?;
    validate_private_directory_fd(&directory, path)?;

    return sync_fd(
        &parent,
        path.parent().unwrap_or_else(|| return Path::new(".")),
    );
}

pub(super) fn open_parent_directory(path: &Path, create_missing: bool) -> Result<OwnedFd> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        Some(_) => Path::new("."),
        None => {
            return Err(Error::Filesystem {
                path: path.to_path_buf(),
                source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
            });
        }
    };

    return open_directory_path(parent, create_missing);
}

pub(super) fn open_directory_path(path: &Path, create_missing: bool) -> Result<OwnedFd> {
    let starting_path = if path.is_absolute() { "/" } else { "." };
    let mut directory = open(
        starting_path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| return nix_filesystem(Path::new(starting_path), error))?;

    let mut traversed = PathBuf::from(starting_path);
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::Filesystem {
                    path: path.to_path_buf(),
                    source: IoError::new(
                        ErrorKind::InvalidInput,
                        "artifact path contains an unsupported parent or prefix component",
                    ),
                });
            }
        };
        traversed.push(name);

        let mut created_or_raced = false;
        let next = match open_directory_at(&directory, name, &traversed) {
            Ok(next) => next,
            Err(Error::Filesystem { source, .. })
                if create_missing && source.kind() == ErrorKind::NotFound =>
            {
                created_or_raced = true;

                match mkdirat(&directory, name, Mode::from_bits_truncate(0o700)) {
                    Ok(()) | Err(Errno::EEXIST) => {}
                    Err(error) => return Err(nix_filesystem(&traversed, error)),
                }

                let next = open_directory_at(&directory, name, &traversed)?;
                sync_fd(
                    &directory,
                    traversed.parent().unwrap_or_else(|| return Path::new(".")),
                )?;

                next
            }
            Err(error) => return Err(error),
        };

        if created_or_raced {
            validate_private_directory_fd(&next, &traversed)?;
        }

        directory = next;
    }

    return Ok(directory);
}

fn open_directory_at(parent: &OwnedFd, name: &OsStr, path: &Path) -> Result<OwnedFd> {
    return openat2(
        parent,
        name,
        OpenHow::new()
            .flags(OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW)
            .resolve(ResolveFlag::RESOLVE_BENEATH | ResolveFlag::RESOLVE_NO_SYMLINKS),
    )
    .map_err(|error| return nix_filesystem(path, error));
}

pub(super) fn open_file_at(
    parent: &OwnedFd,
    name: &OsStr,
    flags: OFlag,
    mode: Mode,
    path: &Path,
) -> Result<OwnedFd> {
    return openat2(
        parent,
        name,
        OpenHow::new()
            .flags(flags)
            .mode(mode)
            .resolve(ResolveFlag::RESOLVE_BENEATH | ResolveFlag::RESOLVE_NO_SYMLINKS),
    )
    .map_err(|error| return nix_filesystem(path, error));
}

pub(super) fn safe_file_name(path: &Path) -> Result<&OsStr> {
    return path.file_name().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "artifact path has no file name"),
        };
    });
}

pub(super) fn validate_private_directory_fd(directory: &OwnedFd, path: &Path) -> Result<()> {
    let stat = fstat(directory).map_err(|error| return nix_filesystem(path, error))?;
    let kind = SFlag::from_bits_truncate(stat.st_mode);
    let mode = stat.st_mode & 0o777;

    if !kind.contains(SFlag::S_IFDIR) || stat.st_uid != Uid::effective().as_raw() || mode != 0o700 {
        return Err(Error::InsecureArtifactDirectory {
            path: path.to_path_buf(),
            owner_uid: stat.st_uid,
            mode,
        });
    }
    return Ok(());
}

pub(super) fn validate_private_regular_file_fd(file: &OwnedFd, path: &Path) -> Result<()> {
    let stat = fstat(file).map_err(|error| return nix_filesystem(path, error))?;
    let kind = SFlag::from_bits_truncate(stat.st_mode);
    let mode = stat.st_mode & 0o777;
    if !kind.contains(SFlag::S_IFREG) || stat.st_uid != Uid::effective().as_raw() || mode != 0o600 {
        return Err(Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(
                ErrorKind::PermissionDenied,
                format!(
                    "artifact file must be owner-only regular file (owner={}, mode={mode:o})",
                    stat.st_uid
                ),
            ),
        });
    }
    return Ok(());
}

pub(super) fn sync_fd(fd: &impl AsFd, path: &Path) -> Result<()> {
    return fsync(fd).map_err(|error| return nix_filesystem(path, error));
}

pub(super) fn nix_filesystem(path: &Path, error: Errno) -> Error {
    return Error::Filesystem {
        path: path.to_path_buf(),
        source: IoError::from_raw_os_error(error as i32),
    };
}

/// Flush, data-sync, and parent-sync a secure buffered artifact writer.
///
/// # Errors
///
/// Returns an error when buffered bytes cannot be written or either sync operation fails.
pub fn finish_writer(mut writer: BufWriter<File>, path: &Path) -> Result<()> {
    writer
        .flush()
        .and_then(|()| return writer.get_ref().sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;

    return sync_parent_directory(path);
}

/// Sync an artifact directory after changing its entries.
///
/// # Errors
///
/// Returns an error when the directory path is unsafe, missing, or cannot be synced.
pub fn sync_directory(path: &Path) -> Result<()> {
    let directory = open_directory_path(path, false)?;

    return sync_fd(&directory, path);
}

/// Atomically publish a staged artifact directory as a direct child of its session.
///
/// Both paths must be direct session children; replacement leaves the old destination at `staging`.
///
/// # Errors
///
/// Returns an error when the paths violate the session boundary, replacement is refused, or the
/// rename and session-directory sync cannot be completed.
pub fn commit_directory(
    session: &Path,
    staging: &Path,
    destination: &Path,
    replace: bool,
) -> Result<()> {
    let session_norm = if session.as_os_str().is_empty() { Path::new(".") } else { session };
    let staging_parent = staging.parent().map(|p| {
        if p.as_os_str().is_empty() {
            return Path::new(".");
        }
        return p;
    });

    let destination_parent = destination.parent().map(|p| {
        if p.as_os_str().is_empty() {
            return Path::new(".");
        }
        return p;
    });

    if staging_parent != Some(session_norm) || destination_parent != Some(session_norm) {
        return Err(Error::InvalidEvidence {
            reason: "staging and destination directories must be direct session children"
                .to_owned(),
            source: None,
        });
    }

    let directory = open_directory_path(session_norm, false)?;
    let staging_name = staging.file_name().ok_or_else(|| {
        return Error::InvalidEvidence {
            reason: "staging directory has no basename".to_owned(),
            source: None,
        };
    })?;

    let destination_name = destination.file_name().ok_or_else(|| {
        return Error::InvalidEvidence {
            reason: "artifact directory has no basename".to_owned(),
            source: None,
        };
    })?;

    renameat2(
        &directory,
        staging_name,
        &directory,
        destination_name,
        RenameFlags::RENAME_NOREPLACE,
    )
    .or_else(|error| {
        if error == Errno::EEXIST && replace {
            return renameat2(
                &directory,
                staging_name,
                &directory,
                destination_name,
                RenameFlags::RENAME_EXCHANGE,
            );
        }
        return Err(error);
    })
    .map_err(|error| {
        if error == Errno::EEXIST {
            return Error::DestinationExists {
                path: destination.to_path_buf(),
            };
        }
        return Error::Filesystem {
            path: destination.to_path_buf(),
            source: IoError::from_raw_os_error(error as i32),
        };
    })?;

    return sync_fd(&directory, session_norm);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SecureWriteStage {
    Created,
    DataSynced,
    Renamed,
}

/// Atomically create, write, and durably publish a new owner-only regular file.
///
/// # Errors
///
/// Returns an error when the path is unsafe, already exists, or cannot be written, renamed, and
/// synced. Before the atomic rename, a failure leaves no destination file.
pub fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
    return write_secure_with_hook(path, bytes, |_| return Ok(()));
}

fn write_secure_with_hook(
    path: &Path,
    bytes: &[u8],
    mut after_stage: impl FnMut(SecureWriteStage) -> Result<()>,
) -> Result<()> {
    let parent_path = path.parent().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        };
    })?;

    let parent = open_parent_directory(path, true)?;
    let destination_name = safe_file_name(path)?;
    let staging_name = format!(".termtrace-write-{}.tmp", Uuid::now_v7());
    let staging_path = parent_path.join(&staging_name);
    let staging = open_file_at(
        &parent,
        OsStr::new(&staging_name),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
        &staging_path,
    )?;

    validate_private_regular_file_fd(&staging, &staging_path)?;
    let mut staging = File::from(staging);
    let mut renamed = false;

    let result = (|| {
        after_stage(SecureWriteStage::Created)?;
        staging.write_all(bytes).map_err(|source| {
            return Error::Filesystem {
                path: staging_path.clone(),
                source,
            };
        })?;
        staging.sync_data().map_err(|source| {
            return Error::Filesystem {
                path: staging_path.clone(),
                source,
            };
        })?;

        after_stage(SecureWriteStage::DataSynced)?;
        renameat2(
            &parent,
            OsStr::new(&staging_name),
            &parent,
            destination_name,
            RenameFlags::RENAME_NOREPLACE,
        )
        .map_err(|error| {
            if error == Errno::EEXIST {
                return Error::DestinationExists {
                    path: path.to_path_buf(),
                };
            }
            return nix_filesystem(path, error);
        })?;

        renamed = true;

        after_stage(SecureWriteStage::Renamed)?;

        return sync_fd(&parent, parent_path);
    })();

    if result.is_err() && !renamed {
        let _cleanup_result = unlinkat(
            &parent,
            OsStr::new(&staging_name),
            UnlinkatFlags::NoRemoveDir,
        )
        .and_then(|()| return fsync(&parent));
    }

    return result;
}

#[cfg(test)]
pub(super) fn write_secure_with_failure(
    path: &Path,
    bytes: &[u8],
    failure: SecureWriteStage,
) -> Result<()> {
    return write_secure_with_hook(path, bytes, |stage| {
        if stage == failure {
            return Err(Error::Filesystem {
                path: path.to_path_buf(),
                source: IoError::other(format!("injected secure-write failure after {stage:?}")),
            });
        }
        return Ok(());
    });
}

/// Durably record a file's directory entry by syncing its parent directory.
///
/// Syncing has no effect on content, so the parent is opened by ordinary resolution: the
/// directory synced is the one in which the caller created the file.
pub(crate) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        Some(_) => Path::new("."),
        None => {
            return Err(Error::Filesystem {
                path: path.to_path_buf(),
                source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
            });
        }
    };

    let directory = open(
        parent,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| return nix_filesystem(parent, error))?;

    return sync_fd(&directory, parent);
}
