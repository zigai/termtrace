//! High-level blocking Rust facade over canonical sessions and actor controls.

use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::{BufRead as _, BufReader, Error as IoError, ErrorKind, Read as _, Write as _},
    net::Shutdown,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use uuid::Uuid;

use crate::{
    Error, Result,
    analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, diagnose, explain},
    artifact::sync_directory,
    bundle::{
        BundleManifest, BundleProfile, BundleVerificationPolicy, create as create_bundle,
        verify as verify_bundle,
    },
    control::{
        ActionInputResult, ActionPostcondition, ActionPostconditionResult, ActorBoundaries,
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlErrorDetails, ControlRequest,
        ControlResponse, ControlResult, InputSequenceAtom, MAX_CONTROL_MESSAGE_BYTES,
        PointerOperation, ScreenPostconditionQuality, ScreenPredicate, StableCaptureQuality,
        connect_session_control, control_endpoint_is_reachable, send_request,
        send_request_response, validate_frame,
    },
    diff::{SemanticDiff, diff_states},
    environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    error::RetrySafety,
    event::{ByteString, EventProjection, InputKind},
    event_log::EventLog,
    ghostty_vt::{KeyModifiers, MouseButton, NamedKey},
    inspection::{
        ScreenWaitOutcome, WaitOutcome, wait_for_completion, wait_for_output, wait_for_screen,
    },
    observation::{
        CaptureQuality, Observation, StatePoint, observe_session_with_options,
        resolved_screen_state_for_session,
    },
    save::{SaveFormat, SaveOutcome, save, verify_saved_artifact},
    screen_frame::{
        FrameDetail, InvalidRegionReason, ScreenFrame, ScreenFrameError, ScreenFrameLimitKind,
        ScreenProjectionRequest, ScreenRegion, project_screen_frame,
    },
    session::{
        SessionCompletion, VerificationLevel, VerificationReport, read_manifest,
        validate_completed_log, verify_session,
    },
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
    /// Caller-owned parent directory used for fresh canonical sessions.
    #[must_use]
    pub fn evidence_root(&self) -> &Path {
        return &self.evidence_root;
    }

    /// Clone this launcher with a different caller-owned evidence root.
    #[must_use]
    pub fn with_evidence_root(&self, evidence_root: impl Into<PathBuf>) -> Self {
        return Self {
            binary: self.binary.clone(),
            evidence_root: evidence_root.into(),
        };
    }

    /// Configure a fresh canonical session launch.
    #[must_use]
    pub fn launch(&self, command: impl IntoIterator<Item = OsString>) -> SessionBuilder {
        return SessionBuilder {
            terminal: self.clone(),
            command: command.into_iter().collect(),
            cols: 160,
            rows: 45,
            working_directory: None,
            startup_timeout: Duration::from_secs(10),
            environment: EnvironmentSpec::default(),
            output_byte_budget: None,
        };
    }

    /// Attach a high-level handle without changing the session.
    pub fn attach(&self, session: impl AsRef<Path>) -> Result<Session> {
        let path = session.as_ref().canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.as_ref().to_path_buf(),
                source,
            };
        })?;
        return Session::open(path, None);
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
        while !control_endpoint_is_reachable(&path) {
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
/// One immutable user-intent atom accepted by [`Session::act`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputAtom {
    /// UTF-8 terminal text.
    Text(String),
    /// Exact raw terminal bytes.
    Bytes(Vec<u8>),
    /// One symbolic key encoded against live terminal modes.
    Key {
        /// Layout-independent key.
        key: NamedKey,
        /// Typed modifier state.
        modifiers: KeyModifiers,
    },
    /// UTF-8 paste encoded against live bracketed-paste mode.
    Paste(String),
}

/// Typed public postcondition using exact Rust durations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionCondition {
    /// Return immediately after complete input application.
    None,
    /// Require one unchanged presented state.
    Idle {
        /// Required unchanged interval.
        stable_for: Duration,
        /// Bounded actor deadline.
        timeout: Duration,
    },
    /// Evaluate one actor-portable presented-screen predicate.
    Screen {
        /// Exact actor predicate.
        predicate: ScreenPredicate,
        /// Bounded actor deadline.
        timeout: Duration,
    },
}

/// Public options for one actor-ordered input action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionOptions {
    /// Caller-retainable idempotency identifier.
    pub request_id: Option<Uuid>,
    /// Optional optimistic presented-state guard.
    pub if_state: Option<String>,
    /// Optional bounded intent attached to every atom.
    pub caller_intent: Option<String>,
    /// Exact delay between atoms.
    pub pace: Duration,
    /// Retain exact per-atom outcomes.
    pub verbose_atom_outcomes: bool,
    /// Actor-side postcondition evaluated without releasing ordering.
    pub postcondition: ActionCondition,
    /// Rich projection returned from the exact terminating state.
    pub projection: ScreenProjectionRequest,
    /// Return incomplete terminal qualities instead of raising a typed error.
    pub allow_incomplete: bool,
}

impl Default for ActionOptions {
    fn default() -> Self {
        return Self {
            request_id: None,
            if_state: None,
            caller_intent: None,
            pace: Duration::ZERO,
            verbose_atom_outcomes: false,
            postcondition: ActionCondition::None,
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
        };
    }
}

/// Typed public action completion without a protocol envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionCompletion {
    /// Input completed and no additional postcondition was requested.
    None,
    /// Idle postcondition terminated.
    Idle {
        /// Whether the requested stable interval was reached.
        matched: bool,
        /// Closed stability classification.
        quality: StableCaptureQuality,
    },
    /// Screen predicate postcondition terminated.
    Screen {
        /// Whether the predicate matched.
        matched: bool,
        /// Closed screen completion classification.
        quality: ScreenPostconditionQuality,
    },
}

/// Complete owned result of one actor-ordered action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionResult {
    /// Caller-retainable action identifier.
    pub request_id: Uuid,
    /// Exact completely applied input accounting.
    pub input: ActionInputResult,
    /// Typed postcondition completion.
    pub completion: ActionCompletion,
    /// Exact terminating frame returned by the actor.
    pub frame: ScreenFrame,
    /// Enclosing actor boundaries, equal to the frame actor source boundaries.
    pub boundaries: ActorBoundaries,
}

/// Options for one exact stable rich-screen capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableCaptureOptions {
    /// Required unchanged presented-state interval.
    pub stable_for: Duration,
    /// Bounded actor-side deadline interval.
    pub timeout: Duration,
    /// Rich projection returned from the exact terminating state.
    pub projection: ScreenProjectionRequest,
    /// Return incomplete terminal qualities instead of raising a typed error.
    pub allow_incomplete: bool,
}

/// High-level live/completed session handle.
pub struct Session {
    path: PathBuf,
    session_id: Uuid,
    recorder: Option<Child>,
    recorded_process_group: Option<u32>,
}

impl Session {
    fn open(path: PathBuf, recorder: Option<Child>) -> Result<Self> {
        let manifest = read_manifest(&path)?;
        let recorded_process_group = if recorder.is_some() {
            match send_request(&path, &ControlRequest::new(ControlCommand::Status)) {
                Ok(ControlData::Status { pid, .. }) => pid,
                _ => None,
            }
        } else {
            None
        };
        return Ok(Self {
            path,
            session_id: manifest.session_id,
            recorder,
            recorded_process_group,
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

    /// Completed evidence, artifact, and retention namespace.
    #[must_use]
    pub fn evidence(&self) -> Evidence<'_> {
        return Evidence { session: self };
    }

    /// Send a caller-owned request so its UUID can be retained for retries.
    pub fn send(&self, request: &ControlRequest) -> Result<ControlData> {
        return send_request(&self.path, request);
    }
    /// Apply input and evaluate its postcondition under one actor ordering point.
    pub fn act(&self, atoms: &[InputAtom], options: ActionOptions) -> Result<ActionResult> {
        validate_action_options(atoms, &options)?;
        let request_id = options.request_id.unwrap_or_else(Uuid::now_v7);
        let wire_atoms = atoms
            .iter()
            .enumerate()
            .map(|(index, atom)| {
                return action_atom(
                    atom,
                    action_operation_id(request_id, index),
                    options.caller_intent.clone(),
                );
            })
            .collect();
        let mut request = ControlRequest::new(ControlCommand::Act {
            atoms: wire_atoms,
            pace_ns: exact_duration_ns("pace", options.pace)?,
            include_atom_outcomes: options.verbose_atom_outcomes,
            postcondition: wire_postcondition(&options.postcondition)?,
            projection: options.projection,
        });
        request.request_id = request_id;
        request.expected_presented_state_hash = options.if_state.clone();
        let response = send_request_response(&self.path, &request)?;
        let data = match response.result {
            ControlResult::Ok { data } => data,
            ControlResult::Error {
                code,
                message,
                details,
            } => {
                return Err(public_control_error(
                    code,
                    message,
                    details,
                    Some(request_id),
                    options.if_state,
                ));
            }
        };
        let ControlData::ActionApplied {
            input,
            postcondition,
            boundaries,
        } = data
        else {
            return Err(client_error("actor returned the wrong action payload"));
        };
        let (completion, frame, complete) = match postcondition {
            ActionPostconditionResult::None { frame, .. } => (ActionCompletion::None, frame, true),
            ActionPostconditionResult::Idle {
                matched,
                quality,
                frame,
            } => (
                ActionCompletion::Idle { matched, quality },
                frame,
                quality == StableCaptureQuality::Stable,
            ),
            ActionPostconditionResult::Screen {
                matched,
                quality,
                frame,
            } => (
                ActionCompletion::Screen { matched, quality },
                frame,
                matched,
            ),
        };
        let result = ActionResult {
            request_id,
            input,
            completion,
            frame,
            boundaries,
        };
        if !complete && !options.allow_incomplete {
            return Err(Error::PostconditionIncomplete {
                result: Box::new(result),
            });
        }
        return Ok(result);
    }

    /// Request graceful stop and optionally wait for immutable completion.
    pub fn stop(&mut self, timeout: Option<Duration>) -> Result<Option<SessionCompletion>> {
        send_request(&self.path, &ControlRequest::new(ControlCommand::Stop))?;
        if let Some(timeout) = timeout {
            return self.complete(timeout).map(Some);
        }
        return Ok(None);
    }

    /// Wait for and validate immutable completion evidence.
    pub fn complete(&mut self, timeout: Duration) -> Result<SessionCompletion> {
        let completion = wait_for_completion(&self.path, timeout)?;
        if let Some(mut child) = self.recorder.take() {
            let _status = child.wait();
        }
        return Ok(completion);
    }

    /// Forcefully terminate and reap the recorded process group and owned recorder after bounded
    /// graceful cleanup fails.
    pub fn force_close(&mut self) -> Result<()> {
        let Some(mut child) = self.recorder.take() else {
            return Ok(());
        };
        let process_group_result = self
            .recorded_process_group
            .take()
            .and_then(|pid| return i32::try_from(pid).ok())
            .map_or(Ok(()), |pid| {
                return match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
                    Ok(()) | Err(Errno::ESRCH) => Ok(()),
                    Err(error) => Err(Error::Filesystem {
                        path: self.path.clone(),
                        source: IoError::from_raw_os_error(error as i32),
                    }),
                };
            });
        let recorder_kill_result = child.kill().map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        });
        let recorder_wait_result = child.wait().map(|_| ()).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        });
        process_group_result?;
        recorder_kill_result?;
        return recorder_wait_result;
    }

    /// Delete one validated completed canonical session owned by this handle.
    ///
    /// # Errors
    ///
    /// Returns an error when completion is missing or invalid, the recorder cannot be
    /// reaped, or the canonical session tree cannot be removed durably.
    pub fn discard_completed(mut self) -> Result<()> {
        let log = EventLog::read(self.path.join("events.jsonl"))?;
        validate_completed_log(&self.path, &log)?;
        if let Some(mut child) = self.recorder.take() {
            child.wait().map_err(|source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            })?;
        }
        let parent = self.path.parent().map(Path::to_path_buf);
        fs_err::remove_dir_all(&self.path).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;
        if let Some(parent) = parent {
            sync_directory(&parent)?;
        }
        return Ok(());
    }

    /// Detach without stopping the independently valid recording.
    pub fn detach(self) -> PathBuf {
        let (path, recorder) = self.into_detached_parts();
        if let Some(mut child) = recorder {
            thread::spawn(move || {
                if let Err(error) = child.wait() {
                    tracing::warn!(%error, "failed to reap detached recorder");
                }
            });
        }
        return path;
    }

    pub(crate) fn into_detached_parts(mut self) -> (PathBuf, Option<Child>) {
        return (self.path, self.recorder.take());
    }
}

/// Completed evidence, artifact, and retention operations.
pub struct Evidence<'session> {
    session: &'session Session,
}

impl Evidence<'_> {
    /// Save and verify a deterministic semantic PNG for one exact state.
    pub fn semantic_screenshot(
        &self,
        destination: impl AsRef<Path>,
        point: StatePoint,
    ) -> Result<SaveOutcome> {
        let outcome = save(
            &self.session.path,
            destination.as_ref(),
            point,
            &[SaveFormat::Png],
            false,
        )?;
        verify_saved_artifact(&outcome.directory, &outcome.manifest)?;
        return Ok(outcome);
    }

    /// Create and independently verify a bounded evidence bundle.
    pub fn bundle(
        &self,
        destination: impl AsRef<Path>,
        profile: BundleProfile,
    ) -> Result<BundleManifest> {
        let destination = destination.as_ref();
        let manifest = create_bundle(&self.session.path, destination, profile)?;
        let policy = BundleVerificationPolicy::new(&self.session.path, profile);
        let verified = verify_bundle(destination, &policy)?;
        if verified != manifest {
            return Err(client_error(
                "verified bundle manifest changed after publication",
            ));
        }
        return Ok(verified);
    }

    /// Verify the completed canonical session without mutating it.
    #[must_use]
    pub fn verify(&self, level: VerificationLevel) -> VerificationReport {
        return verify_session(&self.session.path, level);
    }
}

/// Presented-screen operations.
pub struct Screen<'session> {
    session: &'session Session,
}

/// Atomic stable-screen outcome retaining the exact actor frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableScreenCapture {
    /// Typed reason the actor returned.
    pub quality: StableCaptureQuality,
    /// Exact frame projected before waiter ordering was released.
    pub frame: ScreenFrame,
    /// First actor timestamp at which this exact state was observed.
    pub first_observed_monotonic_ns: u64,
    /// Actor timestamp of the returned snapshot.
    pub observed_monotonic_ns: u64,
    /// Requested unchanged interval.
    pub stable_for_ns: u64,
}

impl Screen<'_> {
    pub(crate) fn session(&self) -> &Session {
        return self.session;
    }

    /// Return the latest actor-presented compact compatibility snapshot.
    pub fn snapshot(&self) -> Result<ControlData> {
        return send_request(
            &self.session.path,
            &ControlRequest::new(ControlCommand::Snapshot),
        );
    }

    /// Return one bounded rich latest-presented frame from a live or completed session.
    pub fn frame(&self, projection: ScreenProjectionRequest) -> Result<ScreenFrame> {
        if self.session.path.join("completion.json").is_file() {
            let (state, source, presented) = resolved_screen_state_for_session(
                &self.session.path,
                StatePoint::LatestPresented,
                false,
            )?;
            return project_screen_frame(
                &state,
                &projection,
                source,
                presented,
                CaptureQuality::Completed,
            )
            .map_err(Error::ScreenFrame);
        }
        let request = ControlRequest::new(ControlCommand::ScreenFrame { projection });
        let response = send_request_response(&self.session.path, &request)?;
        let data = match response.result {
            ControlResult::Ok { data } => data,
            ControlResult::Error {
                code,
                message,
                details,
            } => return Err(public_control_error(code, message, details, None, None)),
        };
        let ControlData::ScreenFrame { frame } = data else {
            return Err(client_error("actor returned the wrong rich-screen payload"));
        };
        return Ok(frame);
    }

    /// Return exact cells for one explicit absolute region.
    pub fn cells(&self, region: ScreenRegion) -> Result<ScreenFrame> {
        return self.frame(ScreenProjectionRequest {
            detail: FrameDetail::Cells,
            region: Some(region),
        });
    }

    /// Return a truth-labeled latest presented compact observation.
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

    /// Atomically capture one exact rich frame under actor stability ordering.
    pub fn capture_stable(&self, options: StableCaptureOptions) -> Result<StableScreenCapture> {
        return self.capture_stable_inner(options);
    }

    fn capture_stable_inner(&self, options: StableCaptureOptions) -> Result<StableScreenCapture> {
        validate_stable_options(options)?;
        let stable_for_ns = exact_duration_ns("stable_for", options.stable_for)?;
        if self.session.path.join("completion.json").is_file() {
            return Ok(StableScreenCapture {
                quality: StableCaptureQuality::Stable,
                frame: self.frame(options.projection)?,
                first_observed_monotonic_ns: 0,
                observed_monotonic_ns: 0,
                stable_for_ns,
            });
        }
        let mut request = ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns,
            timeout_ns: exact_duration_ns("timeout", options.timeout)?,
            projection: options.projection,
        });
        request.request_id = Uuid::now_v7();
        let response =
            send_request_response_with_timeout(&self.session.path, &request, options.timeout)?;
        let data = match response.result {
            ControlResult::Ok { data } => data,
            ControlResult::Error {
                code,
                message,
                details,
            } => return Err(public_control_error(code, message, details, None, None)),
        };
        let ControlData::StableSnapshot {
            frame,
            first_observed_monotonic_ns,
            observed_monotonic_ns,
            stable_for_ns,
            quality,
            semantic_state_zstd: _,
        } = data
        else {
            return Err(client_error(
                "actor returned the wrong stable-capture payload",
            ));
        };
        let capture = StableScreenCapture {
            quality,
            frame,
            first_observed_monotonic_ns,
            observed_monotonic_ns,
            stable_for_ns,
        };
        if quality != StableCaptureQuality::Stable && !options.allow_incomplete {
            return Err(Error::StableCaptureIncomplete {
                result: Box::new(capture),
            });
        }
        return Ok(capture);
    }

    /// Compatibility shorthand for a strict spans-detail stable capture.
    pub fn wait_for_idle(
        &self,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<StableScreenCapture> {
        return self.capture_stable(StableCaptureOptions {
            stable_for,
            timeout,
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
        });
    }

    /// Compatibility shorthand retaining incomplete stable outcomes.
    pub fn wait_for_idle_outcome(
        &self,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<StableScreenCapture> {
        return self.capture_stable(StableCaptureOptions {
            stable_for,
            timeout,
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: true,
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
            caller_intent: None,
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

fn validate_action_options(atoms: &[InputAtom], options: &ActionOptions) -> Result<()> {
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(Error::InvalidAtom {
            atom_index: atoms.len(),
            reason: "action must contain 1 through 4096 atoms".to_owned(),
        });
    }
    if options
        .caller_intent
        .as_ref()
        .is_some_and(|intent| return intent.len() > 1024)
    {
        return Err(Error::InvalidAtom {
            atom_index: 0,
            reason: "caller intent must not exceed 1024 UTF-8 bytes".to_owned(),
        });
    }
    for (index, atom) in atoms.iter().enumerate() {
        let byte_len = match atom {
            InputAtom::Text(text) | InputAtom::Paste(text) => text.len(),
            InputAtom::Bytes(bytes) => bytes.len(),
            InputAtom::Key { .. } => continue,
        };
        if byte_len == 0 || byte_len > 64 * 1024 {
            return Err(Error::InvalidAtom {
                atom_index: index,
                reason: "text, bytes, and paste atoms must contain 1 through 65536 bytes"
                    .to_owned(),
            });
        }
    }
    let pace_ns = exact_duration_ns("pace", options.pace)?;
    if pace_ns > 60_000_000_000 {
        return Err(Error::InvalidDuration {
            field: "pace",
            reason: "pace may not exceed 60 seconds".to_owned(),
        });
    }
    let atom_count = u64::try_from(atoms.len()).unwrap_or(u64::MAX);
    if pace_ns
        .checked_mul(atom_count)
        .is_none_or(|total| return total > 60_000_000_000)
    {
        return Err(Error::InvalidDuration {
            field: "pace",
            reason: "pace multiplied by atom count may not exceed 60 seconds".to_owned(),
        });
    }
    validate_postcondition_durations(&options.postcondition)?;
    if matches!(options.projection.detail, FrameDetail::Cells)
        && options.projection.region.is_none()
    {
        return Err(Error::ScreenFrame(ScreenFrameError::InvalidRegion {
            region: None,
            cols: 0,
            rows: 0,
            reason: InvalidRegionReason::CellsRequireExplicitRegion,
        }));
    }
    return Ok(());
}

fn validate_postcondition_durations(postcondition: &ActionCondition) -> Result<()> {
    match postcondition {
        ActionCondition::None => return Ok(()),
        ActionCondition::Idle {
            stable_for,
            timeout,
        } => {
            let stable_for_ns = exact_duration_ns("stable_for", *stable_for)?;
            let timeout_ns = exact_duration_ns("timeout", *timeout)?;
            if stable_for_ns == 0 {
                return Err(Error::InvalidDuration {
                    field: "stable_for",
                    reason: "idle stability must be greater than zero".to_owned(),
                });
            }
            if timeout_ns == 0 || stable_for_ns > timeout_ns {
                return Err(Error::InvalidDuration {
                    field: "timeout",
                    reason: "timeout must be nonzero and at least stable_for".to_owned(),
                });
            }
            if timeout_ns > 3_600_000_000_000 {
                return Err(Error::InvalidDuration {
                    field: "timeout",
                    reason: "timeout may not exceed one hour".to_owned(),
                });
            }
        }
        ActionCondition::Screen { timeout, .. } => {
            let timeout_ns = exact_duration_ns("timeout", *timeout)?;
            if timeout_ns == 0 || timeout_ns > 3_600_000_000_000 {
                return Err(Error::InvalidDuration {
                    field: "timeout",
                    reason: "screen timeout must be nonzero and at most one hour".to_owned(),
                });
            }
        }
    }
    return Ok(());
}

fn wire_postcondition(postcondition: &ActionCondition) -> Result<ActionPostcondition> {
    return match postcondition {
        ActionCondition::None => Ok(ActionPostcondition::None),
        ActionCondition::Idle {
            stable_for,
            timeout,
        } => Ok(ActionPostcondition::Idle {
            stable_for_ns: exact_duration_ns("stable_for", *stable_for)?,
            timeout_ns: exact_duration_ns("timeout", *timeout)?,
        }),
        ActionCondition::Screen { predicate, timeout } => Ok(ActionPostcondition::Screen {
            predicate: predicate.clone(),
            timeout_ns: exact_duration_ns("timeout", *timeout)?,
        }),
    };
}

fn validate_stable_options(options: StableCaptureOptions) -> Result<()> {
    let stable_for_ns = exact_duration_ns("stable_for", options.stable_for)?;
    let timeout_ns = exact_duration_ns("timeout", options.timeout)?;
    if stable_for_ns == 0 {
        return Err(Error::InvalidDuration {
            field: "stable_for",
            reason: "stable_for must be greater than zero".to_owned(),
        });
    }
    if timeout_ns == 0 || stable_for_ns > timeout_ns || timeout_ns > 3_600_000_000_000 {
        return Err(Error::InvalidDuration {
            field: "timeout",
            reason: "timeout must satisfy stable_for <= timeout <= one hour".to_owned(),
        });
    }
    if matches!(options.projection.detail, FrameDetail::Cells)
        && options.projection.region.is_none()
    {
        return Err(Error::ScreenFrame(ScreenFrameError::InvalidRegion {
            region: None,
            cols: 0,
            rows: 0,
            reason: InvalidRegionReason::CellsRequireExplicitRegion,
        }));
    }
    return Ok(());
}

fn send_request_response_with_timeout(
    session_directory: &Path,
    request: &ControlRequest,
    timeout: Duration,
) -> Result<ControlResponse> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        return Error::InvalidDuration {
            field: "request_timeout",
            reason: "request timeout exceeds the monotonic clock range".to_owned(),
        };
    })?;
    let socket = PathBuf::from("<abstract-control-socket>");
    let mut request_frame = serde_json::to_vec(request).map_err(|source| {
        return Error::InvalidJson {
            path: socket.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if u64::try_from(request_frame.len().saturating_add(1)).unwrap_or(u64::MAX)
        > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "request must be no larger than 1 MiB".to_owned(),
        });
    }
    request_frame.push(b'\n');

    let mut stream = connect_session_control(session_directory)?;
    let write_timeout =
        remaining_control_timeout(deadline, session_directory, "write request", false)?;
    stream
        .set_read_timeout(Some(write_timeout))
        .and_then(|()| return stream.set_write_timeout(Some(write_timeout)))
        .map_err(|source| {
            return client_control_transport_error(
                session_directory,
                "configure deadlines",
                write_timeout,
                false,
                source,
            );
        })?;
    stream.write_all(&request_frame).map_err(|source| {
        return client_control_transport_error(
            session_directory,
            "write request",
            write_timeout,
            true,
            source,
        );
    })?;
    stream.shutdown(Shutdown::Write).map_err(|source| {
        return client_control_transport_error(
            session_directory,
            "finish request",
            write_timeout,
            true,
            source,
        );
    })?;

    let read_timeout =
        remaining_control_timeout(deadline, session_directory, "read response", true)?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|source| {
            return client_control_transport_error(
                session_directory,
                "configure response deadline",
                read_timeout,
                true,
                source,
            );
        })?;
    let mut encoded: Vec<u8> = Vec::new();
    BufReader::new(stream)
        .take(MAX_CONTROL_MESSAGE_BYTES + 1)
        .read_until(b'\n', &mut encoded)
        .map_err(|source| {
            return client_control_transport_error(
                session_directory,
                "read response",
                read_timeout,
                true,
                source,
            );
        })?;
    validate_frame(&encoded)?;
    let response: ControlResponse = serde_json::from_slice(&encoded).map_err(|source| {
        return Error::InvalidJson {
            path: socket,
            json_path: "$".to_owned(),
            source,
        };
    })?;
    if response.version != CONTROL_PROTOCOL_VERSION || response.request_id != request.request_id {
        return Err(Error::InvalidControlProtocol {
            reason: "response version or request identifier does not match".to_owned(),
        });
    }
    return Ok(response);
}

fn remaining_control_timeout(
    deadline: Instant,
    session: &Path,
    phase: &'static str,
    request_dispatched: bool,
) -> Result<Duration> {
    return deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| return !remaining.is_zero())
        .ok_or_else(|| {
            return Error::ControlTimeout {
                session: session.to_path_buf(),
                phase,
                timeout_ms: 0,
                request_dispatched,
                source: IoError::new(ErrorKind::TimedOut, "control deadline elapsed"),
            };
        });
}

fn client_control_transport_error(
    session: &Path,
    phase: &'static str,
    timeout: Duration,
    request_dispatched: bool,
    source: IoError,
) -> Error {
    if matches!(source.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        return Error::ControlTimeout {
            session: session.to_path_buf(),
            phase,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            request_dispatched,
            source,
        };
    }
    return Error::ControlTransport {
        session: session.to_path_buf(),
        phase,
        request_dispatched,
        source,
    };
}

fn exact_duration_ns(field: &'static str, duration: Duration) -> Result<u64> {
    return u64::try_from(duration.as_nanos()).map_err(|_| {
        return Error::InvalidDuration {
            field,
            reason: "duration exceeds unsigned 64-bit nanoseconds".to_owned(),
        };
    });
}

fn action_atom(
    atom: &InputAtom,
    operation_id: Uuid,
    caller_intent: Option<String>,
) -> InputSequenceAtom {
    return match atom {
        InputAtom::Text(text) => InputSequenceAtom::Bytes {
            operation_id,
            data: ByteString::new(text.as_bytes().to_vec()),
            input_kind: InputKind::Text,
            caller_intent,
        },
        InputAtom::Bytes(bytes) => InputSequenceAtom::Bytes {
            operation_id,
            data: ByteString::new(bytes.clone()),
            input_kind: InputKind::Raw,
            caller_intent,
        },
        InputAtom::Key { key, modifiers } => InputSequenceAtom::Key {
            operation_id,
            key: *key,
            modifiers: *modifiers,
            caller_intent,
        },
        InputAtom::Paste(text) => InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(text.as_bytes().to_vec()),
            caller_intent,
        },
    };
}

fn action_operation_id(request_id: Uuid, index: usize) -> Uuid {
    let atom_number = u128::try_from(index).unwrap_or(u128::MAX).saturating_add(1);
    return Uuid::from_u128(request_id.as_u128() ^ atom_number);
}

fn public_control_error(
    code: String,
    message: String,
    details: Option<ControlErrorDetails>,
    request_id: Option<Uuid>,
    expected_state_hash: Option<String>,
) -> Error {
    if code == "stale_observation"
        && let (Some(expected_state_hash), Some(details)) = (expected_state_hash, details.as_ref())
    {
        return Error::StaleState {
            expected_state_hash,
            current_state_hash: details.current_presented_state_hash.clone(),
            boundaries: Box::new(details.boundaries.clone()),
            retry_safety: RetrySafety::NewRequest,
        };
    }
    if matches!(code.as_str(), "action_busy" | "input_sequence_busy") {
        return Error::ActionBusy { message };
    }
    if code == "input_sequence_incomplete"
        && let Some(details) = details.as_ref()
        && let Some(input_sequence) = details.input_sequence.clone()
    {
        let uncertain = input_sequence.failed_atom_bytes_written != 0;
        return Error::PartialInput {
            details: input_sequence,
            retry_safety: RetrySafety::SameRequestOnly,
            uncertain,
        };
    }
    if let Some(details) = details.as_ref()
        && let Some(screen) = details.screen_frame.as_ref()
    {
        if code == "invalid_screen_region"
            && let (Some(cols), Some(rows), Some(reason)) =
                (screen.cols, screen.rows, screen.region_reason)
        {
            let source = ScreenFrameError::InvalidRegion {
                region: screen.region,
                cols,
                rows,
                reason,
            };
            return public_screen_frame_error(request_id, details, source);
        }
        if code == "screen_frame_resource_limit"
            && let (Some(detail), Some(region), Some(limit_kind), Some(requested), Some(maximum)) = (
                screen.detail,
                screen.region,
                screen.limit_kind.as_deref(),
                screen.requested,
                screen.maximum,
            )
        {
            let limit = match limit_kind {
                "cell_count" => ScreenFrameLimitKind::CellCount,
                "serialized_bytes" => ScreenFrameLimitKind::SerializedBytes,
                "response_bytes" => ScreenFrameLimitKind::ResponseBytes,
                _ => return Error::ControlRejected { code, message },
            };
            let source = ScreenFrameError::ResourceLimit {
                limit,
                requested,
                maximum,
                detail,
                region,
            };
            return public_screen_frame_error(request_id, details, source);
        }
    }
    return Error::ControlRejected { code, message };
}

fn public_screen_frame_error(
    request_id: Option<Uuid>,
    details: &ControlErrorDetails,
    source: ScreenFrameError,
) -> Error {
    if details.scope == "action_terminating_screen_frame"
        && let Some(request_id) = request_id
    {
        return Error::ActionTerminatingFrame {
            request_id,
            boundaries: Box::new(details.boundaries.clone()),
            input_fully_applied: true,
            retry_safety: RetrySafety::SameRequestOnly,
            source,
        };
    }
    return Error::ScreenFrame(source);
}

fn client_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detach_reaps_the_recorder_after_it_exits() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn recorder stand-in");
        let process_path = PathBuf::from(format!("/proc/{}", child.id()));
        let session = Session {
            path: PathBuf::from("/unused"),
            session_id: Uuid::now_v7(),
            recorder: Some(child),
            recorded_process_group: None,
        };

        let _path = session.detach();
        let deadline = Instant::now() + Duration::from_secs(1);
        while process_path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            !process_path.exists(),
            "detached recorder remained as an unreaped process"
        );
    }

    #[test]
    fn action_projection_failure_retains_generated_request_id() {
        use crate::{control::ScreenFrameFailureDetails, event_log::CanonicalBoundary};

        let request_id = Uuid::now_v7();
        let boundary = CanonicalBoundary {
            next_event_seq: 4,
            event_hash: "a".repeat(64),
            output_bytes: 7,
        };
        let boundaries = ActorBoundaries {
            durable: boundary.clone(),
            observed: boundary,
        };
        let details = ControlErrorDetails {
            current_presented_state_hash: "b".repeat(64),
            boundaries: boundaries.clone(),
            scope: "action_terminating_screen_frame".to_owned(),
            input_sequence: None,
            screen_frame: Some(ScreenFrameFailureDetails {
                detail: Some(FrameDetail::Cells),
                region: Some(ScreenRegion {
                    top: 0,
                    left: 0,
                    right: 1,
                    bottom: 1,
                }),
                region_reason: None,
                cols: None,
                rows: None,
                limit_kind: Some("response_bytes".to_owned()),
                requested: Some(2),
                maximum: Some(1),
            }),
        };

        let error = public_control_error(
            "screen_frame_resource_limit".to_owned(),
            "projection failed".to_owned(),
            Some(details),
            Some(request_id),
            None,
        );

        let Error::ActionTerminatingFrame {
            request_id: actual_request_id,
            boundaries: actual_boundaries,
            input_fully_applied,
            retry_safety,
            source,
        } = error
        else {
            panic!("expected typed action terminating-frame failure");
        };
        assert_eq!(actual_request_id, request_id);
        assert_eq!(*actual_boundaries, boundaries);
        assert!(input_fully_applied);
        assert_eq!(retry_safety, RetrySafety::SameRequestOnly);
        assert!(matches!(
            source,
            ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::ResponseBytes,
                requested: 2,
                maximum: 1,
                ..
            }
        ));
    }
}
