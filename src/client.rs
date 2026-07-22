//! High-level blocking Rust facade over canonical sessions and actor controls.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::Write as _,
    os::unix::{ffi::OsStrExt as _, fs::OpenOptionsExt as _},
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
        CONTROL_SOCKET_NAME, ControlCommand, ControlData, ControlRequest, PointerOperation,
        ScreenPredicate, StableCaptureQuality, send_request,
    },
    diff::{SemanticDiff, diff_states},
    environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
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
            environment: EnvironmentSpec::default(),
            output_byte_budget: None,
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
    environment: EnvironmentSpec,
    output_byte_budget: Option<u64>,
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

    /// Replace the complete deterministic child-environment policy.
    #[must_use]
    pub fn environment(mut self, environment: EnvironmentSpec) -> Self {
        self.environment = environment;
        return self;
    }

    /// Begin with an empty child environment instead of inheriting the recorder environment.
    #[must_use]
    pub fn clear_environment(mut self) -> Self {
        self.environment.base = EnvironmentBase::Clear;
        return self;
    }

    /// Set one child variable without putting its value in process arguments or logs.
    #[must_use]
    pub fn set_environment(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.environment
            .set
            .insert(name.into(), ByteString::new(value.into()));
        return self;
    }

    /// Remove one child variable after base-environment resolution.
    #[must_use]
    pub fn unset_environment(mut self, name: impl Into<String>) -> Self {
        self.environment.unset.insert(name.into());
        return self;
    }

    /// Copy one variable from the recorder snapshot into an otherwise clear environment.
    #[must_use]
    pub fn copy_environment(mut self, name: impl Into<String>) -> Self {
        self.environment.copy.insert(name.into());
        return self;
    }

    /// Select deterministic common color-variable behavior.
    #[must_use]
    pub fn color_policy(mut self, policy: ColorPolicy) -> Self {
        self.environment.color = policy;
        return self;
    }

    /// Terminate runaway work after this many output bytes without truncating evidence.
    #[must_use]
    pub fn output_byte_budget(mut self, bytes: u64) -> Self {
        self.output_byte_budget = Some(bytes);
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
        self.environment.validate()?;
        let path = self
            .terminal
            .evidence_root
            .join(format!("{}.tt", Uuid::now_v7().simple()));
        let control_path = path.join(CONTROL_SOCKET_NAME);
        if control_path.as_os_str().as_bytes().len() >= 108 {
            return Err(client_error(format!(
                "evidence root is too long for a Linux control socket; choose a shorter root than `{}`",
                self.terminal.evidence_root.display()
            )));
        }
        let log_path = self
            .terminal
            .evidence_root
            .join(format!(".{}.driver.log", Uuid::now_v7()));
        let environment_bytes = serde_json::to_vec(&self.environment).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<environment-spec-stdin>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;
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
            .arg("--environment-spec-stdin");
        if let Some(limit) = self.output_byte_budget {
            process.arg("--output-byte-budget").arg(limit.to_string());
        }
        process
            .arg("--")
            .args(&self.command)
            .stdin(Stdio::piped())
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
        let environment_write = child
            .stdin
            .take()
            .ok_or_else(|| return client_error("recorder environment pipe was not created"))
            .and_then(|mut stdin| {
                return stdin.write_all(&environment_bytes).map_err(|error| {
                    return client_error(format!("write environment pipe: {error}"));
                });
            });
        if let Err(error) = environment_write {
            let _kill = child.kill();
            let _wait = child.wait();
            return Err(error);
        }
        let deadline = Instant::now() + self.startup_timeout;
        while !path.join("control.sock").exists() {
            let status = child.try_wait().map_err(|source| {
                let _kill = child.kill();
                return Error::Filesystem {
                    path: self.terminal.binary.clone(),
                    source,
                };
            })?;
            if let Some(status) = status {
                return Err(client_error(format!(
                    "recorder exited before readiness with {status}"
                )));
            }
            if Instant::now() >= deadline {
                let _kill = child.kill();
                let _wait = child.wait();
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

/// Atomic stable-screen outcome retaining the exact actor snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableScreenCapture {
    /// Typed reason the actor returned.
    pub quality: StableCaptureQuality,
    /// Exact `StableSnapshot` control payload from the same actor ordering point.
    pub snapshot: ControlData,
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

    /// Atomically wait for idle and return only a fully stable exact snapshot.
    pub fn wait_for_idle(
        &self,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<StableScreenCapture> {
        let capture = self.wait_for_idle_outcome(stable_for, timeout)?;
        if capture.quality != StableCaptureQuality::Stable {
            return Err(Error::ControlRejected {
                code: "incomplete_stable_capture".to_owned(),
                message: format!(
                    "stable-screen capture ended with {}; use wait_for_idle_outcome to inspect incomplete snapshots",
                    capture.quality.as_str()
                ),
            });
        }
        return Ok(capture);
    }

    /// Atomically wait for idle and retain typed stable/completed/output-closed/deadline outcomes.
    pub fn wait_for_idle_outcome(
        &self,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<StableScreenCapture> {
        let data = send_request(
            &self.session.path,
            &ControlRequest::new(ControlCommand::CaptureStable {
                stable_for_ns: u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX),
                timeout_ns: u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX),
            }),
        )?;
        let ControlData::StableSnapshot { quality, .. } = &data else {
            return Err(client_error(
                "actor returned the wrong stable-capture payload",
            ));
        };
        return Ok(StableScreenCapture {
            quality: *quality,
            snapshot: data,
        });
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
