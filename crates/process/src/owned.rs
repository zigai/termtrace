//! The owned-process helper: a separate process that runs one command in a private
//! process group for as long as the process that launched it is alive.
//!
//! Watching the owner's identity, including zombie state, from a separate process
//! survives abrupt owner death without tying the group to one of the owner's threads.
//! [`command`] builds the helper invocation; the helper's argument parser names its
//! subcommand and flags with the constants here, so both sides share one contract.

use crate::{poll::poll, ProcessIdentity, SupervisedChild};
use std::{
    ffi::OsStr,
    io,
    path::Path,
    process::{Command, ExitStatus},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Subcommand of the Termtrace executable that runs the helper.
pub const SUBCOMMAND: &str = "owned-process";
/// Long flag carrying the owner's process ID.
pub const PARENT_PID: &str = "parent-pid";
/// Long flag carrying the owner's kernel start ticks.
pub const PARENT_START_TICKS: &str = "parent-start-ticks";
/// How long the helper's group has to exit after `SIGTERM` before `SIGKILL`.
///
/// Callers terminating the helper itself must allow at least this plus one owner poll.
pub const GROUP_GRACE: Duration = Duration::from_millis(100);
const OWNER_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Build a `helper` invocation that runs `program` for as long as the calling process
/// lives. Append `program`'s arguments to the returned command.
///
/// # Errors
///
/// Returns an error when the calling process cannot be identified.
pub fn command(helper: &Path, program: impl AsRef<OsStr>) -> io::Result<Command> {
    let owner = ProcessIdentity::current()?;
    let mut command = Command::new(helper);
    command
        .arg(SUBCOMMAND)
        .arg(format!("--{PARENT_PID}"))
        .arg(owner.pid.to_string())
        .arg(format!("--{PARENT_START_TICKS}"))
        .arg(owner.start_ticks.to_string())
        .arg("--")
        .arg(program);
    return Ok(command);
}

/// Run `command` in a private process group until it exits, `owner` stops running, or
/// `cancelled` is set, then terminate whatever remains of the group.
///
/// Returns the command's exit status, or `None` when it was stopped early.
///
/// # Errors
///
/// Returns an error when the command cannot be spawned or waited on.
pub fn supervise(
    owner: ProcessIdentity,
    command: &mut Command,
    cancelled: &AtomicBool,
) -> io::Result<Option<ExitStatus>> {
    if !owner.is_running() {
        return Ok(None);
    }

    let mut child = SupervisedChild::spawn(command)?;
    let outcome = poll(OWNER_POLL_INTERVAL, || {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(Some(status)));
        }

        if cancelled.load(Ordering::Relaxed) || !owner.is_running() {
            return Ok(Some(None));
        }
        return Ok(None);
    });

    let _termination = child.terminate(GROUP_GRACE);

    return outcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_invocation_names_the_calling_process() {
        let command = command(Path::new("/helper"), "program").expect("identity");
        let owner = ProcessIdentity::current().expect("identity");
        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                OsStr::new(SUBCOMMAND),
                OsStr::new("--parent-pid"),
                OsStr::new(&owner.pid.to_string()),
                OsStr::new("--parent-start-ticks"),
                OsStr::new(&owner.start_ticks.to_string()),
                OsStr::new("--"),
                OsStr::new("program"),
            ]
        );
    }

    #[test]
    fn supervision_stops_when_the_owner_is_gone() {
        let gone = ProcessIdentity {
            start_ticks: ProcessIdentity::current().expect("identity").start_ticks + 1,
            ..ProcessIdentity::current().expect("identity")
        };
        let status = supervise(
            gone,
            Command::new("/bin/sleep").arg("60"),
            &AtomicBool::new(false),
        )
        .expect("supervise");
        assert_eq!(status, None);
    }

    #[test]
    fn supervision_reports_the_command_status() {
        let owner = ProcessIdentity::current().expect("identity");
        let status = supervise(
            owner,
            Command::new("/bin/sh").args(["-c", "exit 3"]),
            &AtomicBool::new(false),
        )
        .expect("supervise");
        assert_eq!(status.and_then(|status| return status.code()), Some(3));
    }
}
