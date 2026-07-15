//! High-level blocking Rust facade over canonical sessions and actor controls.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::{
    Error, Result,
    control::{ControlCommand, ControlData, ControlRequest, send_request},
    event::{ByteString, InputKind},
    ghostty_vt::{KeyModifiers, NamedKey},
    inspection::wait_for_completion,
    session::{SessionCompletion, read_manifest},
};

/// Process launcher and canonical evidence root.
#[derive(Clone, Debug)]
pub struct Terminal {
    binary: PathBuf,
    evidence_root: PathBuf,
}

impl Terminal {
    /// Create a blocking facade with an explicit executable and evidence root.
    pub fn new(binary: impl Into<PathBuf>, evidence_root: impl Into<PathBuf>) -> Self {
        return Self {
            binary: binary.into(),
            evidence_root: evidence_root.into(),
        };
    }

    /// Configure a fresh canonical session launch.
    #[must_use]
    pub fn launch(&self, command: impl IntoIterator<Item = OsString>) -> SessionBuilder {
        return SessionBuilder {
            terminal: self.clone(),
            command: command.into_iter().collect(),
            cols: 80,
            rows: 24,
            working_directory: None,
            startup_timeout: Duration::from_secs(10),
        };
    }

    /// Attach a high-level handle without changing the session.
    pub fn attach(&self, session: impl AsRef<Path>) -> Result<Session> {
        return Session::open(session.as_ref().to_path_buf(), None);
    }
}

/// Fresh-session launch configuration.
#[derive(Clone, Debug)]
pub struct SessionBuilder {
    terminal: Terminal,
    command: Vec<OsString>,
    cols: u16,
    rows: u16,
    working_directory: Option<PathBuf>,
    startup_timeout: Duration,
}

impl SessionBuilder {
    /// Set the immutable initial grid.
    #[must_use]
    pub fn geometry(mut self, cols: u16, rows: u16) -> Self {
        self.cols = cols;
        self.rows = rows;
        return self;
    }

    /// Set the child working directory.
    #[must_use]
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        return self;
    }

    /// Set the actor readiness deadline.
    #[must_use]
    pub fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        return self;
    }

    /// Spawn the recorder and wait for actor readiness.
    pub fn start(self) -> Result<Session> {
        if self.command.is_empty() {
            return Err(client_error("launch command is empty"));
        }
        fs_err::create_dir_all(&self.terminal.evidence_root).map_err(|source| {
            return Error::Filesystem {
                path: self.terminal.evidence_root.clone(),
                source,
            };
        })?;
        let path = self
            .terminal
            .evidence_root
            .join(format!("{}.termtrace", Uuid::now_v7()));
        let log_path = self
            .terminal
            .evidence_root
            .join(format!(".{}.driver.log", Uuid::now_v7()));
        let log = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: log_path.clone(),
                    source,
                };
            })?;
        let mut process = Command::new(&self.terminal.binary);
        process
            .args([
                "record",
                "--no-input",
                "--cols",
                &self.cols.to_string(),
                "--rows",
                &self.rows.to_string(),
                "--out",
            ])
            .arg(&path)
            .arg("--")
            .args(&self.command)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|source| {
                return Error::Filesystem {
                    path: log_path.clone(),
                    source,
                };
            })?))
            .stderr(Stdio::from(log));
        if let Some(directory) = self.working_directory {
            process.current_dir(directory);
        }
        let mut child = process.spawn().map_err(|source| {
            return Error::Filesystem {
                path: self.terminal.binary.clone(),
                source,
            };
        })?;
        let deadline = Instant::now() + self.startup_timeout;
        while !path.join("control.sock").exists() {
            if let Some(status) = child.try_wait().map_err(|source| {
                return Error::Filesystem {
                    path: self.terminal.binary.clone(),
                    source,
                };
            })? {
                return Err(client_error(format!(
                    "recorder exited before readiness with {status}"
                )));
            }
            if Instant::now() >= deadline {
                return Err(client_error(format!(
                    "recorder readiness timed out; diagnostics: {}",
                    log_path.display()
                )));
            }
            thread::sleep(Duration::from_millis(10));
        }
        return Session::open(path, Some(child));
    }
}

/// High-level live/completed session handle.
pub struct Session {
    path: PathBuf,
    session_id: Uuid,
    recorder: Option<Child>,
}

impl Session {
    fn open(path: PathBuf, recorder: Option<Child>) -> Result<Self> {
        let manifest = read_manifest(&path)?;
        return Ok(Self {
            path,
            session_id: manifest.session_id,
            recorder,
        });
    }

    /// Canonical evidence path.
    #[must_use]
    pub fn path(&self) -> &Path {
        return &self.path;
    }

    /// Immutable canonical session identity.
    #[must_use]
    pub fn session_id(&self) -> Uuid {
        return self.session_id;
    }

    /// Presented-screen namespace.
    #[must_use]
    pub fn screen(&self) -> Screen<'_> {
        return Screen { session: self };
    }

    /// Typed keyboard/input namespace.
    #[must_use]
    pub fn keyboard(&self) -> Keyboard<'_> {
        return Keyboard { session: self };
    }

    /// Send a caller-owned request so its UUID can be retained for retries.
    pub fn send(&self, request: &ControlRequest) -> Result<ControlData> {
        return send_request(&self.path, request);
    }

    /// Request graceful stop and optionally wait for immutable completion.
    pub fn stop(&mut self, timeout: Option<Duration>) -> Result<Option<SessionCompletion>> {
        send_request(&self.path, &ControlRequest::new(ControlCommand::Stop))?;
        if let Some(timeout) = timeout {
            return wait_for_completion(&self.path, timeout).map(Some);
        }
        return Ok(None);
    }

    /// Wait for and validate immutable completion evidence.
    pub fn complete(&mut self, timeout: Duration) -> Result<SessionCompletion> {
        let completion = wait_for_completion(&self.path, timeout)?;
        if let Some(child) = &mut self.recorder {
            let _status = child.wait();
        }
        return Ok(completion);
    }

    /// Detach without stopping the independently valid recording.
    pub fn detach(mut self) -> PathBuf {
        self.recorder.take();
        return self.path.clone();
    }
}

/// Presented-screen operations.
pub struct Screen<'session> {
    session: &'session Session,
}

impl Screen<'_> {
    /// Return the latest actor-presented snapshot.
    pub fn snapshot(&self) -> Result<ControlData> {
        return send_request(
            &self.session.path,
            &ControlRequest::new(ControlCommand::Snapshot),
        );
    }
}

/// Typed keyboard and byte input operations.
pub struct Keyboard<'session> {
    session: &'session Session,
}

impl Keyboard<'_> {
    /// Apply exact UTF-8 text with a caller-retainable request.
    pub fn text(&self, text: impl Into<String>) -> Result<ControlData> {
        let request = ControlRequest::new(ControlCommand::Input {
            data: ByteString::new(text.into().into_bytes()),
            input_kind: InputKind::Text,
            intent: None,
        });
        return self.session.send(&request);
    }

    /// Encode one symbolic key from live modes.
    pub fn press(&self, key: NamedKey, modifiers: KeyModifiers) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Key { key, modifiers }));
    }

    /// Encode caller-provided semantic paste content.
    pub fn paste(&self, data: impl Into<Vec<u8>>) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Paste {
                data: ByteString::new(data.into()),
            }));
    }
}

fn client_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
