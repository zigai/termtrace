//! The pinned Ghostty terminfo entry, installed for recorded programs.

use super::support::terminal_error;
use crate::{Error, Result};
use directories::BaseDirs;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use termtrace_terminal::{GHOSTTY_TERMINFO, GHOSTTY_TERMINFO_NAMES};

/// Install the entry and return its terminfo directory, or `None` when `term` names another
/// terminal.
pub(in crate::recording) fn ghostty_terminfo_directory(term: &str) -> Result<Option<PathBuf>> {
    if !GHOSTTY_TERMINFO_NAMES.contains(&term) {
        return Ok(None);
    }

    let cache = BaseDirs::new()
        .map(|directories| return directories.cache_dir().to_path_buf())
        .ok_or_else(|| {
            return terminal_error(
                "install terminfo",
                "platform user cache directory is unavailable",
            );
        })?;

    let digest = blake3::hash(GHOSTTY_TERMINFO).to_hex();
    let directory = cache
        .join("termtrace")
        .join("terminfo")
        .join(digest.as_str().get(..16).unwrap_or(digest.as_str()));

    for name in GHOSTTY_TERMINFO_NAMES {
        install_entry(&directory, name)?;
    }

    return Ok(Some(directory));
}

fn install_entry(directory: &Path, name: &str) -> Result<()> {
    // ncurses looks entries up in a subdirectory named by their first character.
    let parent = directory.join(name.get(..1).unwrap_or(name));
    let path = parent.join(name);
    if fs::read(&path).is_ok_and(|bytes| return bytes == GHOSTTY_TERMINFO) {
        return Ok(());
    }

    fs::create_dir_all(&parent).map_err(filesystem(&parent))?;
    let staged = parent.join(format!(".{name}.{}", uuid::Uuid::now_v7()));
    fs::write(&staged, GHOSTTY_TERMINFO).map_err(filesystem(&staged))?;
    fs::rename(&staged, &path).map_err(filesystem(&path))?;

    return Ok(());
}

fn filesystem(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    return |source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    };
}
