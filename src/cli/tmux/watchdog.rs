use super::{
    client::{option_text, TmuxClient},
    OPTION_OUTPUT_ROOT, OPTION_TERMTRACE, OPTION_VERSION, OPTION_WATCHDOG_PID, SUPERVISOR_INTERVAL,
    TMUX_INTEGRATION_VERSION,
};
use miette::{IntoDiagnostic as _, Result};
use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg},
    libc::{O_NOFOLLOW, O_NONBLOCK, S_IFMT, S_IFREG},
    unistd::geteuid,
};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    os::unix::{
        ffi::OsStringExt as _,
        fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _},
    },
    path::PathBuf,
    process::{self, Command, ExitCode},
    thread,
};
use termtrace_engine::registry::SessionRegistry;

/// The guard lives for the whole observer lifetime. Kernel release on process
/// death prevents a killed observer from permanently blocking its replacement.
pub(super) fn claim(client: &TmuxClient, role: &str) -> Result<Option<Flock<File>>> {
    let socket = PathBuf::from(OsString::from_vec(client.output([
        "display-message",
        "-p",
        "#{socket_path}",
    ])?));

    let metadata = fs_err::symlink_metadata(&socket).into_diagnostic()?;
    if !metadata.file_type().is_socket() || metadata.uid() != geteuid().as_raw() {
        return Err(miette::miette!(
            "observer requires a tmux socket owned by the current user"
        ));
    }

    let root = client.option(OPTION_OUTPUT_ROOT)?;
    let registry = if root.is_empty() {
        SessionRegistry::open_default()
    } else {
        SessionRegistry::open(PathBuf::from(OsString::from_vec(root)))
    }
    .into_diagnostic()?;

    let path = registry.state_root().join(format!(
        ".tmux-{}-{}-{role}.lock",
        metadata.dev(),
        metadata.ino(),
    ));

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .into_diagnostic()?;

    let metadata = file.metadata().into_diagnostic()?;
    if metadata.mode() & S_IFMT != S_IFREG || metadata.uid() != geteuid().as_raw() {
        return Err(miette::miette!(
            "observer lock must be a regular file owned by the current user"
        ));
    }
    return match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(Some(lock)),
        Err((_, Errno::EWOULDBLOCK)) => Ok(None),
        Err((_, error)) => Err(miette::miette!("could not lock tmux observer: {error}")),
    };
}

pub(super) fn run() -> Result<ExitCode> {
    let client = TmuxClient::inherited();
    if !enabled(&client) {
        return Ok(ExitCode::SUCCESS);
    }

    let Some(_lock) = claim(&client, "watchdog")? else {
        return Ok(ExitCode::SUCCESS);
    };

    let executable = PathBuf::from(OsString::from_vec(client.option(OPTION_TERMTRACE)?));
    let pid = process::id().to_string();
    client.set_option(OPTION_WATCHDOG_PID, OsStr::new(&pid))?;

    while enabled(&client) {
        // Waiting owns and reaps the child; only one observer is spawned at a
        // time, and both roles hold independent crash-released locks.
        match Command::new(&executable)
            .args(["tmux", "supervise", "-q"])
            .status()
        {
            Ok(status) => tracing::debug!(%status, "tmux observer exited"),
            Err(error) => tracing::warn!(%error, "could not start tmux observer"),
        }

        if enabled(&client) {
            thread::sleep(SUPERVISOR_INTERVAL);
        }
    }

    if option_text(&client, OPTION_WATCHDOG_PID).ok().as_deref() == Some(pid.as_str()) {
        client.unset_option(OPTION_WATCHDOG_PID)?;
    }

    return Ok(ExitCode::SUCCESS);
}

fn enabled(client: &TmuxClient) -> bool {
    return option_text(client, OPTION_VERSION).ok().as_deref() == Some(TMUX_INTEGRATION_VERSION);
}
