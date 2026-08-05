use super::*;

pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    let parent = open_parent_directory(path, true)?;
    let leaf = safe_file_name(path)?;
    mkdirat(&parent, leaf, Mode::from_bits_truncate(0o700)).map_err(|error| {
        return nix_filesystem(path, error);
    })?;
    let directory = open_directory_at(&parent, leaf, path)?;
    validate_private_directory_fd(&directory, path)?;
    return sync_fd(
        &parent,
        path.parent().unwrap_or_else(|| return Path::new(".")),
    );
}

pub(crate) fn secure_writer(path: &Path) -> Result<BufWriter<File>> {
    return secure_file(path).map(BufWriter::new);
}

pub(crate) fn secure_file(path: &Path) -> Result<File> {
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

pub(super) fn open_parent_directory(path: &Path, create_missing: bool) -> Result<OwnedFd> {
    let Some(parent) = path.parent() else {
        return Err(Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        });
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

pub(crate) fn finish_writer(mut writer: BufWriter<File>, path: &Path) -> Result<()> {
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

/// Create, flush, and parent-sync one owner-only no-replace artifact file.
///
/// # Errors
///
/// Returns an error when the destination exists or any write/sync operation fails.
pub fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut writer = secure_writer(path)?;
    writer
        .write_all(bytes)
        .and_then(|()| return writer.flush())
        .and_then(|()| return writer.get_ref().sync_data())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    return sync_parent_directory(path);
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    let directory = open_directory_path(path, false)?;
    return sync_fd(&directory, path);
}

pub(crate) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        };
    })?;
    let directory = open_directory_path(parent, false)?;
    return sync_fd(&directory, parent);
}

pub(crate) fn commit_directory(session: &Path, staging: &Path, destination: &Path) -> Result<()> {
    if staging.parent() != Some(session) || destination.parent() != Some(session) {
        return Err(invalid_artifact(
            "staging and destination directories must be direct session children",
        ));
    }
    let directory = open_directory_path(session, false)?;
    let staging_name = staging.file_name().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "staging directory has no basename".to_owned(),
        };
    })?;
    let destination_name = destination.file_name().ok_or_else(|| {
        return Error::InvalidReplay {
            reason: "artifact directory has no basename".to_owned(),
        };
    })?;
    renameat2(
        &directory,
        staging_name,
        &directory,
        destination_name,
        RenameFlags::RENAME_NOREPLACE,
    )
    .map_err(|error| {
        if error == Errno::EEXIST {
            return Error::ArtifactExists {
                path: destination.to_path_buf(),
            };
        } else {
            return Error::Filesystem {
                path: destination.to_path_buf(),
                source: IoError::from_raw_os_error(error as i32),
            };
        }
    })?;
    return sync_fd(&directory, session);
}
