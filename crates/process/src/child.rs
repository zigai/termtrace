use crate::{poll::poll_until, ProcessGroup};
use nix::sys::signal::Signal;
use std::{
    convert::Infallible,
    io::{self, ErrorKind},
    os::unix::process::CommandExt as _,
    process::{Child, Command, ExitStatus},
    time::{Duration, Instant},
};

const TERMINATION_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A child spawned as the leader of its own process group, so its descendants can be
/// signalled and reaped together.
///
/// Dropping it signals nothing and leaves the leader unreaped: a leader that exited on
/// its own may leave descendants that must outlive it. Call [`Self::terminate`] or
/// [`Self::kill`] to end the group.
#[derive(Debug)]
pub struct SupervisedChild {
    child: Child,
    group: ProcessGroup,
}

/// How [`SupervisedChild::terminate`] ended a process group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Termination {
    /// Every member exited within the grace period after `SIGTERM`.
    Exited,
    /// Members remained after the grace period and received `SIGKILL`.
    Killed,
}

impl SupervisedChild {
    /// Spawn `command` as the leader of a new process group.
    ///
    /// # Errors
    ///
    /// Returns the spawn error, or `InvalidData` when the child's process ID cannot
    /// name a process group; that child is killed and reaped first.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let mut child = command.process_group(0).spawn()?;
        let Some(group) = ProcessGroup::from_leader(child.id()) else {
            let _kill = child.kill();
            let _wait = child.wait();

            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "child process ID cannot name a process group",
            ));
        };

        return Ok(Self { child, group });
    }

    /// The leader's process ID.
    #[must_use]
    pub fn id(&self) -> u32 {
        return self.child.id();
    }

    /// The group this child leads.
    #[must_use]
    pub const fn group(&self) -> ProcessGroup {
        return self.group;
    }

    /// The leader's standard-stream handles.
    pub const fn child_mut(&mut self) -> &mut Child {
        return &mut self.child;
    }

    /// Reap the leader when it has exited.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from waiting on the leader.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        return self.child.try_wait();
    }

    /// Send `SIGTERM` to the group, wait up to `grace` for every member to exit while
    /// reaping the leader, then `SIGKILL` whatever remains and reap the leader.
    pub fn terminate(mut self, grace: Duration) -> Termination {
        let _signal = self.group.signal(Signal::SIGTERM);
        let deadline = Instant::now() + grace;
        let exited = poll_until(deadline, TERMINATION_POLL_INTERVAL, || {
            let _status = self.child.try_wait();

            return Ok::<_, Infallible>((!self.group.is_alive()).then_some(()));
        });

        let termination = if matches!(exited, Ok(Some(()))) {
            Termination::Exited
        } else {
            let _kill = self.group.kill();
            Termination::Killed
        };

        let _kill = self.child.kill();
        let _wait = self.child.wait();

        return termination;
    }

    /// Send `SIGKILL` to the group and reap the leader.
    ///
    /// # Errors
    ///
    /// Returns the kernel error from reaping the leader.
    pub fn kill(mut self) -> io::Result<ExitStatus> {
        let _kill = self.group.kill();
        let _leader = self.child.kill();

        return self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Stdio, thread};

    fn spawn_shell(script: &str) -> SupervisedChild {
        return SupervisedChild::spawn(
            Command::new("/bin/sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("spawn process group");
    }

    fn assert_gone(group: ProcessGroup) {
        let gone = poll_until(
            Instant::now() + Duration::from_secs(5),
            Duration::from_millis(5),
            || return Ok::<_, Infallible>((!group.is_alive()).then_some(())),
        );
        assert_eq!(gone, Ok(Some(())), "process group outlived supervision");
    }

    #[test]
    fn terminate_reaps_a_cooperative_group_without_killing() {
        let child = spawn_shell("sleep 60 & wait");
        let group = child.group();
        assert_eq!(child.terminate(Duration::from_secs(5)), Termination::Exited);
        assert!(!group.is_alive());
    }

    #[test]
    fn terminate_kills_members_that_ignore_sigterm() {
        let child = spawn_shell("trap '' TERM; sleep 60 & wait");
        let group = child.group();
        thread::sleep(Duration::from_millis(50));

        assert_eq!(
            child.terminate(Duration::from_millis(50)),
            Termination::Killed
        );

        assert_gone(group);
    }

    #[test]
    fn kill_ends_descendants_of_an_exited_leader() {
        let mut child = spawn_shell("sleep 60 & exit 0");
        let group = child.group();

        while child.try_wait().expect("wait leader").is_none() {
            thread::sleep(Duration::from_millis(5));
        }

        assert!(group.is_alive(), "background member outlives its leader");

        child.kill().expect("reap leader");

        assert_gone(group);
    }
}
