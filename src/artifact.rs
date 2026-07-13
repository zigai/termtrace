//! Owner-only, durable, no-replace artifact filesystem helpers.

use std::{
    fs::{DirBuilder, File, OpenOptions, Permissions},
    io::{BufWriter, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use crate::{Error, Result};

pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).map_err(|source| Error::Filesystem {
        path: path.to_path_buf(),
        source,
    })?;
    fs_err::set_permissions(path, Permissions::from_mode(0o700)).map_err(|source| {
        Error::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    })
}

pub(crate) fn secure_writer(path: &Path) -> Result<BufWriter<File>> {
    secure_file(path).map(BufWriter::new)
}

pub(crate) fn secure_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn finish_writer(mut writer: BufWriter<File>, path: &Path) -> Result<()> {
    writer
        .flush()
        .and_then(|()| writer.get_ref().sync_data())
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn write_secure(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut writer = secure_writer(path)?;
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .and_then(|()| writer.get_ref().sync_data())
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| Error::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn commit_directory(session: &Path, staging: &Path, destination: &Path) -> Result<()> {
    let directory = File::open(session).map_err(|source| Error::Filesystem {
        path: session.to_path_buf(),
        source,
    })?;
    let staging_name = staging.file_name().ok_or_else(|| Error::InvalidReplay {
        reason: "staging directory has no basename".to_owned(),
    })?;
    let destination_name = destination
        .file_name()
        .ok_or_else(|| Error::InvalidReplay {
            reason: "artifact directory has no basename".to_owned(),
        })?;
    nix::fcntl::renameat2(
        &directory,
        staging_name,
        &directory,
        destination_name,
        nix::fcntl::RenameFlags::RENAME_NOREPLACE,
    )
    .map_err(|error| {
        if error == nix::errno::Errno::EEXIST {
            Error::ArtifactExists {
                path: destination.to_path_buf(),
            }
        } else {
            Error::Filesystem {
                path: destination.to_path_buf(),
                source: std::io::Error::from_raw_os_error(error as i32),
            }
        }
    })
}
