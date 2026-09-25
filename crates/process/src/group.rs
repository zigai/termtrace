use nix::{
    errno::Errno,
    sys::signal::{killpg, Signal},
    unistd::Pid,
};

/// A process group named by its leader's process ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessGroup {
    leader: Pid,
}

impl ProcessGroup {
    /// The group led by `pid`, or `None` when `pid` cannot name a process group.
    #[must_use]
    pub fn from_leader(pid: u32) -> Option<Self> {
        return i32::try_from(pid)
            .ok()
            .filter(|pid| return *pid > 0)
            .map(|pid| {
                return Self {
                    leader: Pid::from_raw(pid),
                };
            });
    }

    /// Send `signal` to every member of the group.
    ///
    /// # Errors
    ///
    /// Returns the kernel error, `ESRCH` when the group no longer exists.
    pub fn signal(&self, signal: Signal) -> Result<(), Errno> {
        return killpg(self.leader, signal);
    }

    /// Send `SIGKILL` to every member of the group; a group that is already gone is killed.
    ///
    /// # Errors
    ///
    /// Returns any kernel error other than `ESRCH`.
    pub fn kill(&self) -> Result<(), Errno> {
        return match killpg(self.leader, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error),
        };
    }

    /// Whether any process, including an unreaped zombie, is still a member of the group.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        return killpg(self.leader, None) != Err(Errno::ESRCH);
    }
}
