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
    analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, diagnose, explain},
    control::{
        ControlCommand, ControlData, ControlRequest, PointerOperation, ScreenPredicate,
        send_request,
    },
    diff::{SemanticDiff, diff_states},
    event::{ByteString, EventProjection, InputKind},
    event_log::EventLog,
    ghostty_vt::{KeyModifiers, MouseButton, NamedKey},
    inspection::{
        ScreenWaitOutcome, WaitOutcome, wait_for_completion, wait_for_output, wait_for_screen,
    },
    observation::{Observation, StatePoint, observe_session_with_options},
    session::{SessionCompletion, read_manifest, validate_completed_log},
    timeline::{TimelineEntry, build_timeline},
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

    /// Mode-aware pointer and focus namespace.
    #[must_use]
    pub fn pointer(&self) -> Pointer<'_> {
        return Pointer { session: self };
    }

    /// Canonical output-history namespace.
    #[must_use]
    pub fn output(&self) -> Output<'_> {
        return Output { session: self };
    }

    /// Completed semantic derivation namespace.
    #[must_use]
    pub fn semantic(&self) -> Semantic<'_> {
        return Semantic { session: self };
    }

    /// Typed canonical event namespace.
    #[must_use]
    pub fn events(&self) -> Events<'_> {
        return Events { session: self };
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

    /// Return a truth-labeled latest presented observation.
    pub fn observation(&self) -> Result<Observation> {
        return observe_session_with_options(
            &self.session.path,
            StatePoint::LatestPresented,
            false,
        );
    }

    /// Wait inside the actor for a stable presented predicate.
    pub fn wait(
        &self,
        predicate: ScreenPredicate,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<ScreenWaitOutcome> {
        return wait_for_screen(&self.session.path, predicate, stable_for, timeout);
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

/// Mode-aware cell pointer and focus operations.
pub struct Pointer<'session> {
    session: &'session Session,
}

impl Pointer<'_> {
    /// Emit a press/release click plan at a zero-based cell.
    pub fn click(
        &self,
        column: u16,
        row: u16,
        button: MouseButton,
        strict: bool,
    ) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Click { button },
                strict,
            }));
    }

    /// Emit signed vertical wheel ticks at a zero-based cell.
    pub fn scroll(&self, column: u16, row: u16, lines: i16, strict: bool) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Scroll { lines },
                strict,
            }));
    }

    /// Report focus state when the application enabled focus reporting.
    pub fn focus(&self, focused: bool, strict: bool) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Focus {
                focused,
                strict,
            }));
    }
}

/// Canonical child-output history operations.
pub struct Output<'session> {
    session: &'session Session,
}

impl Output<'_> {
    /// Wait for exact bytes ever emitted by the canonical child stream.
    pub fn wait_for(&self, bytes: &[u8], timeout: Duration) -> Result<WaitOutcome> {
        return wait_for_output(&self.session.path, bytes, timeout);
    }
}

/// Completed semantic inspection and diagnosis operations.
pub struct Semantic<'session> {
    session: &'session Session,
}

impl Semantic<'_> {
    /// Inspect one truth-labeled state point.
    pub fn inspect(&self, point: StatePoint, include_hidden: bool) -> Result<Observation> {
        return observe_session_with_options(&self.session.path, point, include_hidden);
    }

    /// Build the ordered semantic/canonical timeline.
    pub fn timeline(&self, all_states: bool, controls: bool) -> Result<Vec<TimelineEntry>> {
        let log = self.completed_log()?;
        return build_timeline(&log, all_states, controls, None);
    }

    /// Compare two complete semantic endpoints.
    pub fn diff(&self, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
        return diff_states(&self.completed_log()?, from, to);
    }

    /// Explain exact VT frames and transitions around one point.
    pub fn explain(&self, point: StatePoint, context_events: u64) -> Result<ExplainReport> {
        return explain(&self.completed_log()?, point, context_events);
    }

    /// Run conservative source-bound detectors.
    pub fn diagnose(
        &self,
        checks: Vec<DiagnosticCheck>,
        broad_change_percent: u8,
    ) -> Result<DiagnoseReport> {
        let log = self.completed_log()?;
        return diagnose(&self.session.path, &log, checks, broad_change_percent);
    }

    fn completed_log(&self) -> Result<EventLog> {
        let log = EventLog::read(self.session.path.join("events.jsonl"))?;
        validate_completed_log(&self.session.path, &log)?;
        return Ok(log);
    }
}

/// Bounded typed canonical event reads.
pub struct Events<'session> {
    session: &'session Session,
}

impl Events<'_> {
    /// Read one bounded projection page and its resume sequence.
    pub fn page(
        &self,
        from_event_seq: u64,
        limit: u16,
        metadata_only: bool,
    ) -> Result<(Vec<EventProjection>, u64)> {
        if limit == 0 || limit > 4096 {
            return Err(client_error("event limit must be from 1 through 4096"));
        }
        let log = EventLog::read(self.session.path.join("events.jsonl"))?;
        let events = log
            .events
            .iter()
            .filter(|record| return record.seq >= from_event_seq)
            .take(usize::from(limit))
            .map(|record| return EventProjection::from_record(record, metadata_only))
            .collect::<Vec<_>>();
        let next = events
            .last()
            .map_or(from_event_seq, |event| return event.seq.saturating_add(1));
        return Ok((events, next));
    }
}

fn client_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
