use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg, OFlag},
    unistd::geteuid,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, ErrorKind},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::Path,
};

/// An exclusive `flock` on an owner-private regular file, released when dropped or when
/// the holding process dies.
#[derive(Debug)]
pub struct ExclusiveLock {
    file: Flock<File>,
}

impl ExclusiveLock {
    /// Create or open the lock file at `path` and block until the lock is held.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened or locked, or is not a regular
    /// file owned by the effective user with no group or other permissions.
    pub fn acquire(path: &Path) -> io::Result<Self> {
        return Flock::lock(open_private(path)?, FlockArg::LockExclusive)
            .map(|file| return Self { file })
            .map_err(|(_, error)| return io::Error::from(error));
    }

    /// Like [`Self::acquire`], but return `None` instead of waiting for another holder.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::acquire`].
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        return match Flock::lock(open_private(path)?, FlockArg::LockExclusiveNonblock) {
            Ok(file) => Ok(Some(Self { file })),
            Err((_, Errno::EWOULDBLOCK)) => Ok(None),
            Err((_, error)) => Err(io::Error::from(error)),
        };
    }

    /// The locked file, for recording the holder.
    #[must_use]
    pub fn file(&self) -> &File {
        return &self.file;
    }
}

fn open_private(path: &Path) -> io::Result<File> {
    // No symlink is followed and a planted FIFO cannot block the open.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)?;

    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "lock `{}` must be a regular file private to the current user",
                path.display()
            ),
        ));
    }
    return Ok(file);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt as _},
    };

    #[test]
    fn a_held_lock_excludes_other_holders_until_dropped() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("held.lock");
        let held = ExclusiveLock::acquire(&path).expect("first holder");

        assert!(ExclusiveLock::try_acquire(&path)
            .expect("second open")
            .is_none());

        drop(held);

        assert!(ExclusiveLock::try_acquire(&path).expect("reopen").is_some());
    }

    #[test]
    fn shared_or_linked_lock_files_are_refused() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let shared = directory.path().join("shared.lock");
        fs::write(&shared, b"").expect("create");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).expect("chmod");

        assert_eq!(
            ExclusiveLock::acquire(&shared).expect_err("shared").kind(),
            ErrorKind::PermissionDenied
        );

        let linked = directory.path().join("linked.lock");
        symlink(&shared, &linked).expect("symlink");

        assert!(ExclusiveLock::try_acquire(&linked).is_err());
    }
}
