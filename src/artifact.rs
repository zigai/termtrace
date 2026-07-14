//! Owner-only, durable, no-replace artifact filesystem helpers.

use std::{
    fs::{DirBuilder, File, OpenOptions, Permissions},
    io::{BufWriter, Error as IoError, ErrorKind, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use nix::{
    errno::Errno,
    fcntl::{RenameFlags, renameat2},
};

use crate::{Error, Result};

pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    fs_err::set_permissions(path, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return sync_parent_directory(path);
}

pub(crate) fn secure_writer(path: &Path) -> Result<BufWriter<File>> {
    return secure_file(path).map(BufWriter::new);
}

pub(crate) fn secure_file(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        });
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

pub(crate) fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
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
    return File::open(path)
        .and_then(|directory| return directory.sync_all())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        });
}

pub(crate) fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source: IoError::new(ErrorKind::InvalidInput, "path has no parent directory"),
        };
    })?;
    return sync_directory(parent);
}

pub(crate) fn commit_directory(session: &Path, staging: &Path, destination: &Path) -> Result<()> {
    let directory = File::open(session).map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
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
    return sync_directory(session);
}
