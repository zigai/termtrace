use std::{
    ffi::OsString,
    fs,
    io::{self, ErrorKind},
    os::unix::ffi::OsStringExt as _,
    path::PathBuf,
    process,
};

/// A running process's ID paired with the kernel start time that distinguishes a reused PID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    /// Process ID.
    pub pid: u32,
    /// Start time in clock ticks since boot (`/proc/<pid>/stat` field 22).
    pub start_ticks: u64,
}

impl ProcessIdentity {
    /// Identify the running process `pid`.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the process is gone or a zombie, and `InvalidData`
    /// when its `/proc` status is malformed.
    pub fn of(pid: u32) -> io::Result<Self> {
        let status = ProcessStatus::read(pid)?;
        if !status.running {
            return Err(io::Error::new(ErrorKind::NotFound, "process has exited"));
        }
        return Ok(Self {
            pid,
            start_ticks: status.start_ticks,
        });
    }

    /// Identify the calling process.
    ///
    /// # Errors
    ///
    /// Returns an error when `/proc/self` status is unreadable or malformed.
    pub fn current() -> io::Result<Self> {
        return Self::of(process::id());
    }

    /// Whether this exact process is still running and not a zombie.
    #[must_use]
    pub fn still_running(&self) -> bool {
        return ProcessStatus::read(self.pid).is_ok_and(|status| {
            return status.running && status.start_ticks == self.start_ticks;
        });
    }
}

struct ProcessStatus {
    running: bool,
    start_ticks: u64,
}

impl ProcessStatus {
    fn read(pid: u32) -> io::Result<Self> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let malformed =
            || return io::Error::new(ErrorKind::InvalidData, "malformed process status");
        // The command name may contain spaces and parentheses; fields follow its last `)`.
        let (_, fields) = stat.rsplit_once(')').ok_or_else(malformed)?;
        let mut fields = fields.split_whitespace();
        let state = fields.next().ok_or_else(malformed)?;
        let start_ticks = fields
            .nth(18)
            .and_then(|ticks| return ticks.parse().ok())
            .ok_or_else(malformed)?;

        return Ok(Self {
            running: !matches!(state, "Z" | "X"),
            start_ticks,
        });
    }
}

/// The executable and arguments of a process, read from `/proc`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessImage {
    /// Resolved path of the running executable.
    pub executable: PathBuf,
    /// Command-line arguments, including the program name.
    pub arguments: Vec<OsString>,
}

impl ProcessImage {
    /// Read the image of process `pid`, or `None` when the process no longer exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the process exists but its image is unreadable.
    pub fn of(pid: u32) -> io::Result<Option<Self>> {
        let process = PathBuf::from(format!("/proc/{pid}"));
        let executable = match fs::read_link(process.join("exe")) {
            Ok(path) => path,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };

        let command_line = match fs::read(process.join("cmdline")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };

        // Every argument, including the last, is NUL-terminated.
        let arguments = command_line
            .strip_suffix(&[0])
            .map(|terminated| {
                return terminated
                    .split(|byte| return *byte == 0)
                    .map(|argument| return OsString::from_vec(argument.to_vec()))
                    .collect();
            })
            .unwrap_or_default();

        return Ok(Some(Self {
            executable,
            arguments,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        process::{Command, Stdio},
    };

    #[test]
    fn identity_tracks_a_process_until_it_is_reaped() {
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn");

        let identity = ProcessIdentity::of(child.id()).expect("identity");
        assert!(identity.still_running());
        assert!(!ProcessIdentity {
            start_ticks: identity.start_ticks + 1,
            ..identity
        }
        .still_running());

        child.kill().expect("kill");
        child.wait().expect("reap");

        assert!(!identity.still_running());
        assert!(ProcessIdentity::of(identity.pid).is_err());
    }

    #[test]
    fn image_reads_executable_and_arguments() {
        let image = ProcessImage::of(process::id())
            .expect("readable image")
            .expect("running process");
        assert_eq!(image.executable, env::current_exe().expect("exe"));
        assert!(!image.arguments.is_empty());
    }
}
