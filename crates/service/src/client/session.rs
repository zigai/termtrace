use super::{client_error, Events, Evidence, Native, Output, Pointer, Semantic};
use crate::{
    actions::{duration_ns, ActOutcome, ActRequest, MutationGuard, Reaction},
    defaults,
    screen::{StableCaptureOptions, StableScreenCapture},
    Error, Result,
};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{
    io,
    path::{Path, PathBuf},
    process::Child,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use termtrace_control::{
    observe_phase, read_completed, send_or_completed, send_request, ExchangeTiming,
    PhaseObservation, Reached,
};
use termtrace_engine::{
    inspection::wait_for_completion,
    observation::{
        observation_from_live_snapshot, observe_session_with_options, project_screen_frame,
        resolved_screen_state_for_session, semantic_state_from_live_snapshot,
        semantic_state_with_hash_for_session, state_text,
    },
};
use termtrace_evidence::{
    artifact::sync_directory,
    session::{read_manifest, validate_completed_session},
};
use termtrace_model::{
    execution::{layout::SessionLayout, session::SessionCompletion},
    protocol::control::{
        ClockSample, CommandError, ControlCommand, ControlPayload, ControlRequest, ScreenPredicate,
        ScreenWaitQuality, StableCaptureQuality,
    },
    protocol::mutation::MutationEffect,
    terminal::{
        inspection::ScreenWaitOutcome,
        observation::{ActorBoundaries, CaptureQuality, Observation, StatePoint},
        screen::{ScreenFrame, ScreenFrameSource, ScreenProjectionRequest},
        state::SemanticState,
    },
};
use termtrace_process::{ProcessGroup, ProcessIdentity};
use uuid::Uuid;

/// One live or completed canonical session, owning its recorder when this process launched it.
///
/// Every interface drives sessions through these operations. Reads of a session that has
/// completed, or completes within [`defaults::COMPLETION_GRACE`] of its actor becoming
/// unreachable, are answered from validated completed evidence.
pub struct Session {
    path: PathBuf,
    helper: PathBuf,
    session_id: Uuid,
    ownership: Ownership,
}

enum Ownership {
    Attached,
    Launched(Arc<RecorderOwner>),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RecorderDisposition {
    Pending,
    Completed,
    Forced,
}

struct Recorder {
    process: Option<Child>,
    recorded_group: Option<ProcessGroup>,
    recorded_identity: Option<ProcessIdentity>,
    group_identity_refused: bool,
    force_at: Option<Instant>,
    recorder_kill_sent: bool,
    force_error: Option<StoredIoError>,
    disposition: RecorderDisposition,
}

// Preserve repeatable OS error diagnostics after another view reaps the shared Child.
struct StoredIoError {
    kind: io::ErrorKind,
    raw: Option<i32>,
    message: String,
}

impl From<io::Error> for StoredIoError {
    fn from(error: io::Error) -> Self {
        return Self {
            kind: error.kind(),
            raw: error.raw_os_error(),
            message: error.to_string(),
        };
    }
}

impl StoredIoError {
    fn restore(&self) -> io::Error {
        return self.raw.map_or_else(
            || return io::Error::new(self.kind, self.message.clone()),
            io::Error::from_raw_os_error,
        );
    }
}

// The Child is never copied or reconstructed from a PID. All lifecycle views share this exact
// unreaped process capability, serialize nonblocking wait/kill, and retire it once only.
struct RecorderOwner {
    recorder: Mutex<Recorder>,
    forced: AtomicBool,
    sealed: AtomicBool,
    operations: AtomicUsize,
}

impl RecorderOwner {
    fn new(
        process: Child,
        recorded_group: Option<ProcessGroup>,
        recorded_identity: Option<ProcessIdentity>,
    ) -> Self {
        return Self {
            recorder: Mutex::new(Recorder {
                process: Some(process),
                recorded_group,
                recorded_identity,
                group_identity_refused: false,
                force_at: None,
                recorder_kill_sent: false,
                force_error: None,
                disposition: RecorderDisposition::Pending,
            }),
            forced: AtomicBool::new(false),
            sealed: AtomicBool::new(false),
            operations: AtomicUsize::new(0),
        };
    }
}

impl Drop for RecorderOwner {
    fn drop(&mut self) {
        if let Some(mut process) = self
            .recorder
            .get_mut()
            .unwrap_or_else(|error| return error.into_inner())
            .process
            .take()
        {
            // Dropping a view never stops input. A last dropped owner keeps an actual Child in
            // an eventual reaper, including after a bounded force-reap timeout (e.g. D-state I/O).
            thread::spawn(move || {
                if let Err(error) = process.wait() {
                    tracing::warn!(%error, "failed to reap released recorder");
                }
            });
        }
    }
}

struct LifecycleOperation(Option<Arc<RecorderOwner>>);

impl Drop for LifecycleOperation {
    fn drop(&mut self) {
        if let Some(owner) = &self.0 {
            owner.operations.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// One stop request: the actor stops the child and finalizes evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StopRequest {
    /// Request identity and presented-state guard.
    pub guard: MutationGuard,
}

/// What [`Session::stop`] observed.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopOutcome {
    /// The actor acknowledged the request; completion was not awaited.
    Requested {
        /// Canonical positions when the actor acknowledged the stop.
        boundaries: ActorBoundaries,
    },
    /// Validated completion evidence, published before or after the request.
    Completed {
        /// Final completion record.
        completion: Box<SessionCompletion>,
    },
}

/// What [`Session::status`] observed.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionStatus {
    /// The live actor answered.
    Live {
        /// Child process identifier when available.
        pid: Option<u32>,
        /// Durable and observed canonical positions.
        boundaries: ActorBoundaries,
        /// Mapping between session-relative and system monotonic time.
        clock_sample: ClockSample,
    },
    /// The session has completed.
    Completed {
        /// Final completion record.
        completion: Box<SessionCompletion>,
    },
}

impl Session {
    pub(super) fn open(path: PathBuf, helper: PathBuf, recorder: Option<Child>) -> Result<Self> {
        let manifest = read_manifest(&path)?;
        let ownership = recorder.map_or(Ownership::Attached, |process| {
            let (recorded_group, recorded_identity) =
                match send_request(&path, &ControlRequest::new(ControlCommand::Status)) {
                    Ok(ControlPayload::Status { pid, .. }) => (
                        pid.and_then(ProcessGroup::from_leader),
                        pid.and_then(|pid| return ProcessIdentity::of(pid).ok()),
                    ),
                    _ => (None, None),
                };

            return Ownership::Launched(Arc::new(RecorderOwner::new(
                process,
                recorded_group,
                recorded_identity,
            )));
        });

        return Ok(Self {
            path,
            helper,
            session_id: manifest.session_id,
            ownership,
        });
    }

    /// Attach to an existing session without owning its recorder; `helper` is the Termtrace
    /// executable used for native capture.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory is missing or has no valid manifest.
    pub fn attach(session: impl AsRef<Path>, helper: impl Into<PathBuf>) -> Result<Self> {
        let path = session.as_ref().canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.as_ref().to_path_buf(),
                source,
            };
        })?;

        return Self::open(path, helper.into(), None);
    }

    /// Canonical evidence path.
    #[must_use]
    pub fn path(&self) -> &Path {
        return &self.path;
    }

    /// Termtrace executable used for native capture.
    #[must_use]
    pub fn helper(&self) -> &Path {
        return &self.helper;
    }

    /// Immutable canonical session identity.
    #[must_use]
    pub const fn session_id(&self) -> Uuid {
        return self.session_id;
    }

    /// Whether this handle launched the recording, and so owns its evidence.
    #[must_use]
    pub const fn is_launched(&self) -> bool {
        return matches!(self.ownership, Ownership::Launched(_));
    }

    /// Per-session force/reap budget after the graceful lifecycle deadline.
    pub const FORCE_REAP_TIMEOUT: Duration = Duration::from_secs(2);
    /// Additional bounded drain for already admitted lifecycle observers.
    pub const OBSERVER_DRAIN_TIMEOUT: Duration = Duration::from_millis(2250);

    /// Create a lifecycle view sharing the original recorder authority, not an attachment.
    /// A cancelled worker can hold a view without hiding force-close or duplicating a Child/PID.
    #[must_use]
    pub fn lifecycle_observer(&self) -> Self {
        let ownership = match &self.ownership {
            Ownership::Attached => Ownership::Attached,
            Ownership::Launched(owner) => Ownership::Launched(Arc::clone(owner)),
        };

        return Self {
            path: self.path.clone(),
            helper: self.helper.clone(),
            session_id: self.session_id,
            ownership,
        };
    }

    /// Whether a shared launched recorder has been reaped exactly once.
    #[must_use]
    pub fn lifecycle_finished(&self) -> bool {
        return match &self.ownership {
            Ownership::Attached => false,
            Ownership::Launched(owner) => owner
                .recorder
                .lock()
                .unwrap_or_else(|error| return error.into_inner())
                .process
                .is_none(),
        };
    }

    /// Whether force-close was admitted on any view of the original recorder.
    #[must_use]
    pub fn lifecycle_force_requested(&self) -> bool {
        return match &self.ownership {
            Ownership::Attached => false,
            Ownership::Launched(owner) => owner.forced.load(Ordering::Acquire),
        };
    }

    /// Reap a launched recorder that has exited, without waiting; `true` once it is gone.
    /// Attached sessions own no recorder and report `false`.
    ///
    /// # Errors
    /// Returns the nonblocking wait failure.
    pub fn reap_exited_recorder(&self) -> Result<bool> {
        let Ownership::Launched(owner) = &self.ownership else {
            return Ok(false);
        };
        let mut recorder = owner
            .recorder
            .lock()
            .unwrap_or_else(|error| return error.into_inner());

        if let Some(process) = recorder.process.as_mut()
            && process
                .try_wait()
                .map_err(|source| return self.lifecycle_filesystem(source))?
                .is_some()
        {
            let _reaped = recorder.process.take();
        }

        return Ok(recorder.process.is_none());
    }

    fn lifecycle_operation(&self) -> Result<LifecycleOperation> {
        return match &self.ownership {
            Ownership::Attached => Ok(LifecycleOperation(None)),
            Ownership::Launched(owner) => {
                let _admission = owner
                    .recorder
                    .lock()
                    .unwrap_or_else(|error| return error.into_inner());

                if owner.sealed.load(Ordering::Acquire) {
                    return Err(client_error(
                        "lifecycle observer was sealed by finalization",
                    ));
                }

                owner.operations.fetch_add(1, Ordering::AcqRel);

                Ok(LifecycleOperation(Some(Arc::clone(owner))))
            }
        };
    }

    /// Close lifecycle admission before draining, including workers queued but not yet started.
    /// Does not discard force-close authority or interrupt arbitrarily blocked filesystem I/O.
    pub fn seal_lifecycle_observers(&self) {
        if let Ownership::Launched(owner) = &self.ownership {
            let _admission = owner
                .recorder
                .lock()
                .unwrap_or_else(|error| return error.into_inner());

            owner.sealed.store(true, Ordering::Release);
        }
    }

    /// Drain previously admitted lifecycle I/O before helper evidence disposal.
    ///
    /// # Errors
    /// Reports an observer-drain timeout without discarding recorder authority. Arbitrarily
    /// blocked filesystem I/O is not made interruptible by this deadline.
    pub fn drain_lifecycle_observers(&self, timeout: Duration) -> Result<()> {
        let Ownership::Launched(owner) = &self.ownership else {
            return Ok(());
        };

        let deadline = lifecycle_deadline(timeout)?;
        while owner.operations.load(Ordering::Acquire) != 0 {
            if Instant::now() >= deadline {
                return Err(client_error("lifecycle observer drain deadline expired"));
            }

            thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }

        return Ok(());
    }

    /// Observe the session phase now.
    #[must_use]
    pub fn phase(&self) -> PhaseObservation {
        return observe_phase(&self.path);
    }

    /// Mode-aware pointer and focus namespace.
    #[must_use]
    pub const fn pointer(&self) -> Pointer<'_> {
        return Pointer { session: self };
    }

    /// Canonical output-history namespace.
    #[must_use]
    pub const fn output(&self) -> Output<'_> {
        return Output { session: self };
    }

    /// Typed analysis namespace, an alias of the existing semantic capability.
    pub const fn analysis(&self) -> Semantic<'_> {
        return self.semantic();
    }

    /// Typed integrity namespace, sharing evidence operations and source identity.
    pub const fn integrity(&self) -> Evidence<'_> {
        return self.evidence();
    }

    /// Completed semantic derivation namespace.
    #[must_use]
    pub const fn semantic(&self) -> Semantic<'_> {
        return Semantic { session: self };
    }

    /// Typed canonical event namespace.
    #[must_use]
    pub const fn events(&self) -> Events<'_> {
        return Events { session: self };
    }

    /// Completed evidence, artifact, and retention namespace.
    #[must_use]
    pub const fn evidence(&self) -> Evidence<'_> {
        return Evidence { session: self };
    }

    /// Native capture, playback, and presentation namespace.
    #[must_use]
    pub const fn native(&self) -> Native<'_> {
        return Native { session: self };
    }

    /// Send one caller-owned request to the live actor; its UUID can be retained for retries.
    ///
    /// # Errors
    ///
    /// Returns the transport failure or the actor's rejection.
    pub fn control(&self, request: &ControlRequest) -> Result<ControlPayload> {
        request.command.validate()?;

        return Ok(send_request(&self.path, request)?);
    }

    /// Read the live actor's status, or the completion of a session that has ended.
    ///
    /// Does not wait for a session whose actor is unreachable to publish completion.
    ///
    /// # Errors
    ///
    /// Returns the transport failure, the actor's rejection, or a completion-evidence failure.
    pub fn status(&self) -> Result<SessionStatus> {
        return match self.reach_or_completed(ControlCommand::Status, Duration::ZERO)? {
            Reached::Live(payload) => match *payload {
                ControlPayload::Status {
                    pid,
                    boundaries,
                    clock_sample,
                } => Ok(SessionStatus::Live {
                    pid,
                    boundaries,
                    clock_sample,
                }),
                _ => Err(client_error("actor returned the wrong status payload")),
            },
            Reached::Completed(completion) => Ok(SessionStatus::Completed { completion }),
        };
    }

    /// Published, identity-checked completion; `None` while the session has not completed.
    ///
    /// # Errors
    ///
    /// Returns an error when completion or manifest evidence is unreadable or names a
    /// different session.
    pub fn completion(&self) -> Result<Option<SessionCompletion>> {
        return Ok(read_completed(&self.path)?);
    }

    /// Send `command` to the live actor, or report completion published within `grace` of the
    /// actor becoming unreachable.
    pub(in crate::client) fn reach_or_completed(
        &self,
        command: ControlCommand,
        grace: Duration,
    ) -> Result<Reached> {
        let request = ControlRequest::new(command);
        request.command.validate()?;

        let timing = ExchangeTiming::Timeout(request.command.client_response_timeout());

        return Ok(send_or_completed(&self.path, &request, timing, grace)?);
    }

    fn reach(&self, request: &ControlRequest) -> Result<Reached> {
        let timing = ExchangeTiming::Timeout(request.command.client_response_timeout());

        return self.reach_within(request, timing);
    }

    fn reach_within(&self, request: &ControlRequest, timing: ExchangeTiming) -> Result<Reached> {
        request.command.validate()?;

        let grace = match &timing {
            ExchangeTiming::Timeout(_) => defaults::COMPLETION_GRACE,
            ExchangeTiming::Deadline(deadline) => deadline
                .saturating_duration_since(Instant::now())
                .min(defaults::COMPLETION_GRACE),
        };

        return Ok(send_or_completed(&self.path, request, timing, grace)?);
    }

    /// Apply input and evaluate its postcondition under one actor ordering point.
    ///
    /// # Errors
    ///
    /// Returns the actor's rejection or transport failure; its effect states what was applied.
    /// An unmet postcondition is an error unless the request allows incomplete outcomes.
    pub fn act(&self, request: ActRequest) -> Result<ActOutcome> {
        let request_id = request.guard.request_id.unwrap_or_else(Uuid::now_v7);
        let mut wire = ControlRequest::new(ControlCommand::Act {
            atoms: request.wire_atoms(request_id),
            pace_ns: duration_ns(request.pace),
            include_atom_outcomes: request.verbose_atom_outcomes,
            postcondition: request.postcondition.to_wire(),
            projection: request.projection,
        });
        wire.request_id = request_id;
        wire.expected_presented_state_hash = request.guard.if_state;
        let ControlPayload::ActionApplied {
            input,
            postcondition,
            boundaries,
        } = self.control(&wire)?
        else {
            return Err(client_error("actor returned the wrong action payload"));
        };
        if input.sequence_id != request_id {
            return Err(client_error(
                "actor returned an action input identifier that does not match the request",
            ));
        }

        let outcome = ActOutcome {
            request_id,
            input,
            postcondition,
            boundaries,
        };

        if !outcome.is_matched() && !request.allow_incomplete {
            return Err(Error::PostconditionIncomplete {
                result: Box::new(outcome),
            });
        }
        return Ok(outcome);
    }

    /// Wait for a presented-screen predicate, or evaluate it once on a completed session.
    ///
    /// # Errors
    ///
    /// Returns the actor's rejection, a transport failure, or a completed-evidence failure.
    pub fn wait_screen(
        &self,
        predicate: ScreenPredicate,
        stable_for: Duration,
        timeout: Duration,
    ) -> Result<ScreenWaitOutcome> {
        let request = ControlRequest::new(ControlCommand::WaitScreen {
            predicate: predicate.clone(),
            stable_for_ns: duration_ns(stable_for),
            timeout_ns: duration_ns(timeout),
            projection: ScreenProjectionRequest::default(),
        });

        let data = match self.reach(&request)? {
            Reached::Live(data) => *data,
            Reached::Completed(_) => {
                return self.completed_screen_wait(predicate, duration_ns(stable_for));
            }
        };

        let ControlPayload::ScreenWait {
            predicate,
            matched,
            first_match_monotonic_ns,
            observed_monotonic_ns,
            stable_for_ns,
            quality,
            frame,
        } = data
        else {
            return Err(client_error("actor returned the wrong screen-wait payload"));
        };

        return Ok(ScreenWaitOutcome {
            predicate,
            matched,
            first_match_monotonic_ns,
            observed_monotonic_ns,
            stable_for_ns,
            quality,
            frame,
        });
    }

    fn completed_screen_wait(
        &self,
        predicate: ScreenPredicate,
        stable_for_ns: u64,
    ) -> Result<ScreenWaitOutcome> {
        let compiled = predicate
            .compile()
            .map_err(CommandError::ScreenWaitPredicate)?;

        let (state, source, presented) =
            resolved_screen_state_for_session(&self.path, StatePoint::LatestPresented, false)?;

        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            source,
            presented,
            CaptureQuality::Completed,
        )
        .map_err(Error::ScreenFrame)?;

        let matched = compiled.matches(&state, &state_text(&state), &frame.state_hash);
        let observed_monotonic_ns = frame_monotonic_ns(&frame);

        return Ok(ScreenWaitOutcome {
            predicate,
            matched,
            first_match_monotonic_ns: matched.then_some(observed_monotonic_ns),
            observed_monotonic_ns,
            stable_for_ns,
            quality: ScreenWaitQuality::Completed,
            frame,
        });
    }

    /// Capture one exact frame once it stays unchanged, or the final frame once completed.
    ///
    /// # Errors
    ///
    /// Returns the actor's rejection, a transport failure, or, unless incomplete captures are
    /// allowed, [`Error::StableCaptureIncomplete`] when the frame did not settle.
    pub fn capture_stable(&self, options: StableCaptureOptions) -> Result<StableScreenCapture> {
        return self.capture_stable_by(options, None);
    }

    /// [`Self::capture_stable`], abandoning the exchange at `deadline` when one is given and the
    /// command's client response bound is later.
    pub(crate) fn capture_stable_by(
        &self,
        options: StableCaptureOptions,
        deadline: Option<Instant>,
    ) -> Result<StableScreenCapture> {
        let stable_for_ns = duration_ns(options.stable_for);
        let request = ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns,
            timeout_ns: duration_ns(options.timeout),
            projection: options.projection,
            reaction: options.reaction.as_ref().map(Reaction::to_wire),
        });

        let client_timeout = request.command.client_response_timeout();
        let timing = deadline.map_or(ExchangeTiming::Timeout(client_timeout), |deadline| {
            // The client still outwaits the actor's own bound, but never the caller's deadline.
            let exchange_deadline = Instant::now()
                .checked_add(client_timeout)
                .map_or(deadline, |bound| return bound.min(deadline));
            return ExchangeTiming::Deadline(exchange_deadline);
        });

        let capture = match self.reach_within(&request, timing)? {
            Reached::Live(data) => {
                let ControlPayload::StableSnapshot {
                    frame,
                    semantic_state_zstd,
                    first_observed_monotonic_ns,
                    observed_monotonic_ns,
                    stable_for_ns,
                    quality,
                } = *data
                else {
                    return Err(client_error(
                        "actor returned the wrong stable-capture payload",
                    ));
                };

                StableScreenCapture {
                    quality,
                    frame,
                    first_observed_monotonic_ns,
                    observed_monotonic_ns,
                    stable_for_ns,
                    semantic_state_zstd: Some(semantic_state_zstd),
                }
            }
            Reached::Completed(_) => {
                let frame =
                    self.completed_frame(StatePoint::LatestPresented, false, options.projection)?;
                let observed_monotonic_ns = frame_monotonic_ns(&frame);

                StableScreenCapture {
                    quality: StableCaptureQuality::Completed,
                    frame,
                    first_observed_monotonic_ns: observed_monotonic_ns,
                    observed_monotonic_ns,
                    stable_for_ns,
                    semantic_state_zstd: None,
                }
            }
        };

        if !capture.quality.is_settled() && !options.allow_incomplete {
            return Err(Error::StableCaptureIncomplete {
                result: Box::new(capture),
            });
        }
        return Ok(capture);
    }

    /// Observe the latest presented state of a live session, or any state once completed.
    ///
    /// # Errors
    ///
    /// Returns an error when a live session is asked for another point, or observation fails.
    pub fn observe(&self, point: StatePoint, include_hidden: bool) -> Result<Observation> {
        if self.live_selectable(&point, false)? {
            let request = ControlRequest::new(ControlCommand::Snapshot);
            if let Reached::Live(snapshot) = self.reach(&request)? {
                return Ok(observation_from_live_snapshot(&self.path, *snapshot)?);
            }
        }

        return Ok(observe_session_with_options(
            &self.path,
            point,
            include_hidden,
        )?);
    }

    /// Resolve the complete semantic state and its hash, live or completed.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::observe`].
    pub fn semantic_state(
        &self,
        point: &StatePoint,
        include_hidden: bool,
    ) -> Result<(SemanticState, String)> {
        if self.live_selectable(point, false)? {
            let request = ControlRequest::new(ControlCommand::Snapshot);
            if let Reached::Live(snapshot) = self.reach(&request)? {
                return Ok(semantic_state_from_live_snapshot(*snapshot)?);
            }
        }

        return Ok(semantic_state_with_hash_for_session(
            &self.path,
            point,
            include_hidden,
        )?);
    }

    /// Project one bounded rich frame of the latest presented state.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::frame_at`].
    pub fn frame(&self, projection: ScreenProjectionRequest) -> Result<ScreenFrame> {
        return self.frame_at(StatePoint::LatestPresented, false, projection);
    }

    /// Project one bounded rich frame: the latest presented state of a live session, or any
    /// state once completed.
    ///
    /// # Errors
    ///
    /// Returns an error when a live session is asked for another point or hidden states, the
    /// projection exceeds its bounds, or observation fails.
    pub fn frame_at(
        &self,
        point: StatePoint,
        include_hidden: bool,
        projection: ScreenProjectionRequest,
    ) -> Result<ScreenFrame> {
        if self.live_selectable(&point, include_hidden)? {
            let request = ControlRequest::new(ControlCommand::ScreenFrame { projection });
            if let Reached::Live(data) = self.reach(&request)? {
                let ControlPayload::ScreenFrame { frame } = *data else {
                    return Err(client_error("actor returned the wrong rich-screen payload"));
                };
                return Ok(frame);
            }
        }

        return self.completed_frame(point, include_hidden, projection);
    }

    /// Whether `point` may be answered by the live actor; points only completed evidence can
    /// answer are rejected while the session is still live.
    fn live_selectable(&self, point: &StatePoint, include_hidden: bool) -> Result<bool> {
        if *point == StatePoint::LatestPresented && !include_hidden {
            return Ok(true);
        }

        if read_completed(&self.path)?.is_none() {
            return Err(client_error(
                "live sessions support only latest-presented selection without hidden states",
            ));
        }
        return Ok(false);
    }

    fn completed_frame(
        &self,
        point: StatePoint,
        include_hidden: bool,
        projection: ScreenProjectionRequest,
    ) -> Result<ScreenFrame> {
        let (state, source, presented) =
            resolved_screen_state_for_session(&self.path, point, include_hidden)?;

        return project_screen_frame(
            &state,
            &projection,
            source,
            presented,
            CaptureQuality::Completed,
        )
        .map_err(Error::ScreenFrame);
    }

    /// Ask the recording to end and, when `wait` is set, wait for validated completion.
    ///
    /// A session that has already completed reports its completion.
    ///
    /// # Errors
    ///
    /// Returns the actor's rejection, a transport failure, or a completion failure.
    pub fn stop(&mut self, request: StopRequest, wait: Option<Duration>) -> Result<StopOutcome> {
        let _operation = self.lifecycle_operation()?;
        let wire = request.guard.request(ControlCommand::Stop);

        return match (self.reach(&wire)?, wait) {
            (Reached::Live(acknowledgement), None) => match *acknowledgement {
                ControlPayload::Acknowledged { boundaries } => {
                    Ok(StopOutcome::Requested { boundaries })
                }
                _ => Err(client_error("actor returned the wrong stop payload")),
            },
            (reached, wait) => Ok(StopOutcome::Completed {
                completion: Box::new(
                    self.complete_after_stop(reached, wait.unwrap_or(Duration::ZERO))?,
                ),
            }),
        };
    }

    /// Stop and observe completion within one graceful deadline, keeping the shared owner.
    ///
    /// # Errors
    /// Preserves the protocol's transport/effect/retry errors. On expiry the caller still has
    /// force-close authority; no stop request is resent and no input is rolled back.
    pub fn stop_bounded(
        &mut self,
        request: StopRequest,
        timeout: Duration,
    ) -> Result<SessionCompletion> {
        let _operation = self.lifecycle_operation()?;
        let deadline = lifecycle_deadline(timeout)?;
        let wire = request.guard.request(ControlCommand::Stop);
        let reached = self.reach_within(&wire, ExchangeTiming::Deadline(deadline))?;
        return self
            .complete_after_stop(reached, deadline.saturating_duration_since(Instant::now()));
    }

    fn complete_after_stop(
        &mut self,
        reached: Reached,
        timeout: Duration,
    ) -> Result<SessionCompletion> {
        return match reached {
            Reached::Live(payload) => match *payload {
                ControlPayload::Acknowledged { .. } => self
                    .complete(timeout)
                    .map_err(|error| return error.with_proven_effect(MutationEffect::Applied)),
                _ => Err(client_error("actor returned the wrong stop payload")),
            },
            // Existing completion proves no mutation by this request. A query failure must
            // not inherit another request's Stop acknowledgement.
            Reached::Completed(_) => self.complete(timeout),
        };
    }

    /// Wait for validated completion evidence and reap an owned recorder.
    ///
    /// # Errors
    /// Returns the original completion-validation/timeout errors, or a bounded owner-reap error.
    /// A force-close on another shared view wakes this wait without losing the original Child.
    pub fn complete(&mut self, timeout: Duration) -> Result<SessionCompletion> {
        let _operation = self.lifecycle_operation()?;
        let deadline = lifecycle_deadline(timeout)?;
        let completion_path = SessionLayout::new(&self.path).completion();

        loop {
            if completion_path.is_file() || Instant::now() >= deadline {
                // Keep the existing full content validator and its error taxonomy.
                let completion = wait_for_completion(&self.path, Duration::ZERO)?;
                self.reap_recorder(Some(deadline), true)?;

                return Ok(completion);
            }

            if self.lifecycle_force_requested() {
                return Err(client_error(
                    "recording was force-closed while waiting for completion",
                ));
            }

            thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    fn reap_recorder(&self, deadline: Option<Instant>, valid_completion: bool) -> Result<()> {
        let Ownership::Launched(owner) = &self.ownership else {
            return Ok(());
        };

        loop {
            {
                let mut recorder = owner
                    .recorder
                    .lock()
                    .unwrap_or_else(|error| return error.into_inner());

                if recorder.process.is_some() && owner.forced.load(Ordering::Acquire) {
                    if !valid_completion {
                        return Ok(());
                    } // detach yields reaping to the force owner
                    return Err(client_error(
                        "recording was force-closed while reaping completion",
                    ));
                }

                if let Some(process) = recorder.process.as_mut()
                    && process
                        .try_wait()
                        .map_err(|source| return self.lifecycle_filesystem(source))?
                        .is_some()
                {
                    let _reaped = recorder.process.take();
                }

                if recorder.process.is_none() {
                    if valid_completion {
                        recorder.disposition = RecorderDisposition::Completed;
                    }

                    return Ok(());
                }
            }

            if deadline.is_some_and(|deadline| return Instant::now() >= deadline) {
                return Err(client_error(
                    "owned recorder reap deadline expired; authority is retained",
                ));
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn lifecycle_filesystem(&self, source: io::Error) -> Error {
        return Error::Filesystem {
            path: self.path.clone(),
            source,
        };
    }

    /// Forcefully terminate and exactly-once reap the original recorded group and recorder.
    ///
    /// # Errors
    /// Returns signal/wait failures or a force-reap deadline error. Authority is retained for
    /// recovery, or by an eventual reaper if all handles are dropped; no PID is reconstructed.
    pub fn force_close(&mut self) -> Result<()> {
        return self.force_close_with_grace(Duration::ZERO);
    }

    /// Bounded helper fallback: kill the recorded group, briefly resume an owned stopped
    /// recorder so it can reap that group, then kill/reap the recorder if it has not exited.
    /// The 250ms grace is included in [`Self::FORCE_REAP_TIMEOUT`], not added to it.
    ///
    /// # Errors
    /// Preserves force-close signal/wait errors and retains the original authority on timeout.
    pub fn force_close_for_finalization(&mut self) -> Result<()> {
        return self.force_close_with_grace(Duration::from_millis(250));
    }

    fn force_close_with_grace(&mut self, recorder_grace: Duration) -> Result<()> {
        let Ownership::Launched(owner) = &self.ownership else {
            return Ok(());
        };
        owner.forced.store(true, Ordering::Release);

        let deadline = lifecycle_deadline(Self::FORCE_REAP_TIMEOUT)?;

        loop {
            {
                let mut recorder = owner
                    .recorder
                    .lock()
                    .unwrap_or_else(|error| return error.into_inner());

                if recorder.disposition != RecorderDisposition::Pending {
                    return recorder.force_error.as_ref().map_or(Ok(()), |error| {
                        return Err(self.lifecycle_filesystem(error.restore()));
                    });
                }

                if let Some(process) = recorder.process.as_mut()
                    && process
                        .try_wait()
                        .map_err(|source| return self.lifecycle_filesystem(source))?
                        .is_some()
                {
                    let _reaped = recorder.process.take();
                }

                if recorder.force_at.is_none() {
                    recorder.force_at = Some(Instant::now());
                    if let Some(group) = recorder.recorded_group {
                        if recorder
                            .recorded_identity
                            .is_some_and(|identity| return recorded_identity_matches(identity))
                        {
                            recorder.force_error = group
                                .kill()
                                .err()
                                .map(io::Error::from)
                                .map(StoredIoError::from);
                        } else if group.is_alive() {
                            recorder.group_identity_refused = true;
                            recorder.force_error = Some(io::Error::new(io::ErrorKind::PermissionDenied,
                                "recorded group leader identity is unavailable or changed; refusing a stale group signal").into());
                        }
                    }

                    // A detached eventual reaper may already have reaped the recorder, but not
                    // its original recorded group. Retain that identity-guarded capability above.
                    if !recorder_grace.is_zero()
                        && let Some(process) = &recorder.process
                    {
                        // This exact Child is unreaped and reserved while the mutex is held.
                        // Never signal the numeric recorder PID after try_wait has reaped it.
                        if let Ok(pid) = i32::try_from(process.id()) {
                            let _resume = kill(Pid::from_raw(pid), Signal::SIGCONT);
                        }
                    }
                }

                if recorder.process.is_none() {
                    if recorder.group_identity_refused
                        && recorder
                            .recorded_group
                            .is_some_and(|group| return !group.is_alive())
                    {
                        recorder.force_error = None;
                    }

                    recorder.disposition = RecorderDisposition::Forced;

                    return recorder.force_error.as_ref().map_or(Ok(()), |error| {
                        return Err(self.lifecycle_filesystem(error.restore()));
                    });
                }

                if !recorder.recorder_kill_sent
                    && recorder.force_at.is_some_and(|started| {
                        return started.elapsed() >= recorder_grace;
                    })
                {
                    recorder.recorder_kill_sent = true;

                    if let Some(process) = recorder.process.as_mut()
                        && let Err(error) = process.kill()
                        && recorder.force_error.is_none()
                    {
                        recorder.force_error = Some(error.into());
                    }
                }
            }

            if Instant::now() >= deadline {
                return Err(client_error("force-close reap deadline expired; original recorder authority retained for eventual reaping"));
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Delete one validated completed canonical session.
    ///
    /// # Errors
    ///
    /// Returns an error when completion is missing or invalid, the recorder cannot be
    /// reaped, or the canonical session tree cannot be removed durably.
    pub fn discard_completed(&mut self) -> Result<()> {
        validate_completed_session(&self.path)?;

        self.reap_recorder(Some(lifecycle_deadline(Self::FORCE_REAP_TIMEOUT)?), true)?;

        fs_err::remove_dir_all(&self.path).map_err(|source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        })?;

        if let Some(parent) = self.path.parent() {
            sync_directory(parent)?;
        }

        return Ok(());
    }

    /// Stop managing the recording without stopping it; an owned recorder is reaped when it
    /// exits.
    #[must_use]
    pub fn detach(self) -> PathBuf {
        if matches!(self.ownership, Ownership::Launched(_)) {
            let observer = self.lifecycle_observer();
            thread::spawn(move || {
                if let Err(error) = observer.reap_recorder(None, false) {
                    tracing::warn!(%error, "failed to reap detached recorder");
                }
            });
        }

        return self.path;
    }
}

// ProcessIdentity::is_running deliberately excludes zombies. A still-present original zombie
// leader nevertheless reserves its PID and may have live group members that must be killed.
// Compare the same kernel start-ticks field without requiring a running leader; never treat a
// missing/unreadable/malformed identity as authority for a numeric group signal.
fn recorded_identity_matches(identity: ProcessIdentity) -> bool {
    let Ok(stat) = fs_err::read_to_string(format!("/proc/{}/stat", identity.pid)) else {
        return false;
    };

    // Linux stat field 22; comm can contain spaces/parentheses, as in ProcessIdentity's parser.
    return stat
        .rsplit_once(')')
        .and_then(|(_, fields)| return fields.split_whitespace().nth(19))
        .and_then(|ticks| return ticks.parse::<u64>().ok())
        == Some(identity.start_ticks);
}

fn lifecycle_deadline(timeout: Duration) -> Result<Instant> {
    return Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| return client_error("lifecycle timeout is out of range"));
}

fn frame_monotonic_ns(frame: &ScreenFrame) -> u64 {
    return match &frame.source {
        ScreenFrameSource::Actor {
            observed_monotonic_ns,
            ..
        } => *observed_monotonic_ns,
        ScreenFrameSource::Resolved { monotonic_ns, .. } => monotonic_ns.unwrap_or_default(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::process::CommandExt as _, process::Command, time::Instant};

    #[test]
    fn detach_reaps_the_recorder_after_it_exits() {
        let process = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn recorder stand-in");

        let process_path = PathBuf::from(format!("/proc/{}", process.id()));
        let session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused-termtrace"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Launched(Arc::new(RecorderOwner::new(process, None, None))),
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
    fn lifecycle_views_share_one_child_and_concurrent_reaping() {
        let process = Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .spawn()
            .expect("recorder stand-in");

        let process_path = PathBuf::from(format!("/proc/{}", process.id()));
        let mut session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Launched(Arc::new(RecorderOwner::new(process, None, None))),
        };
        let mut observer = session.lifecycle_observer();
        let (Ownership::Launched(first), Ownership::Launched(second)) =
            (&session.ownership, &observer.ownership)
        else {
            panic!("owned views");
        };

        assert!(Arc::ptr_eq(first, second));
        assert!(observer.is_launched());

        let worker = thread::spawn(move || return observer.force_close());
        session.force_close().expect("force original");

        worker.join().expect("worker").expect("force shared view");

        assert!(session.lifecycle_finished());

        session.seal_lifecycle_observers();
        let error = session
            .lifecycle_observer()
            .complete(Duration::from_secs(60))
            .expect_err("queued observer denied without I/O");
        assert!(
            matches!(error, Error::InvalidOperation { reason, .. } if reason.contains("sealed"))
        );

        session
            .drain_lifecycle_observers(Duration::ZERO)
            .expect("no new admission");

        assert!(
            !process_path.exists(),
            "original recorder was reaped before return"
        );

        // A stale lifecycle view has no remaining Child/PID target to signal.
        let mut unrelated = Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .spawn()
            .expect("unrelated child");
        session
            .lifecycle_observer()
            .force_close()
            .expect("idempotent retired view");

        assert!(unrelated.try_wait().expect("unrelated status").is_none());

        unrelated.kill().expect("kill unrelated test child");
        unrelated.wait().expect("reap unrelated test child");
    }

    #[test]
    fn force_close_refuses_a_reused_group_identity_and_caches_the_failure() {
        let process = Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .spawn()
            .expect("recorder");

        let mut unrelated = Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .process_group(0)
            .spawn()
            .expect("other group");

        let group = ProcessGroup::from_leader(unrelated.id()).expect("group");
        let mut identity = ProcessIdentity::of(unrelated.id()).expect("identity");
        identity.start_ticks += 1; // deterministic PID-reuse mismatch, never signal this group
        let mut session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Launched(Arc::new(RecorderOwner::new(
                process,
                Some(group),
                Some(identity),
            ))),
        };
        let first = session.force_close();
        let repeated = session.lifecycle_observer().force_close();
        let survived = unrelated.try_wait().expect("other status").is_none();

        if survived {
            unrelated.kill().expect("kill other test child");
        }

        unrelated.wait().expect("reap other test child");

        assert!(
            survived,
            "a mismatched group identity must never receive a kill"
        );
        assert!(session.lifecycle_finished());

        for result in [first, repeated] {
            let Error::Filesystem { source, .. } = result.expect_err("identity refusal") else {
                panic!("typed filesystem failure");
            };

            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            assert!(source.to_string().contains("identity"));
        }
    }

    #[test]
    fn detached_reaping_retains_original_group_authority_for_the_scope_owner() {
        let process = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("recorder");

        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .process_group(0)
            .spawn()
            .expect("recorded group stand-in");

        let identity = ProcessIdentity::of(child.id()).expect("identity");
        let session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Launched(Arc::new(RecorderOwner::new(
                process,
                ProcessGroup::from_leader(child.id()),
                Some(identity),
            ))),
        };
        let mut scope_owner = session.lifecycle_observer();
        let _path = session.detach();
        let deadline = Instant::now() + Duration::from_secs(1);

        while !scope_owner.lifecycle_finished() {
            assert!(Instant::now() < deadline, "detached recorder reaping");

            thread::sleep(Duration::from_millis(1));
        }

        let forced = scope_owner.force_close();
        if forced.is_err() {
            child.kill().expect("recover stand-in");
        }

        let status = child.wait().expect("reap group stand-in");

        forced.expect("original group capability remains available after recorder reaping");

        assert!(!status.success());
    }

    #[test]
    fn original_zombie_group_leader_does_not_hide_live_group_members() {
        use std::{io::Write as _, process::Stdio};
        let mut leader = Command::new("/bin/sh")
            .args(["-c", "read ignored; exit 0"])
            .stdin(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("group leader");

        let identity = ProcessIdentity::of(leader.id()).expect("original leader identity");
        let mut member = Command::new("/bin/sleep")
            .arg("60")
            .process_group(i32::try_from(leader.id()).expect("PGID"))
            .spawn()
            .expect("live group member");
        leader
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"exit\n")
            .expect("release leader");

        let deadline = Instant::now() + Duration::from_secs(1);
        while identity.is_running() {
            assert!(Instant::now() < deadline, "leader exit");

            thread::sleep(Duration::from_millis(1));
        }

        assert!(
            PathBuf::from(format!("/proc/{}", identity.pid)).exists(),
            "leader remains unreaped"
        );

        let process = Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("recorder stand-in");

        let mut session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Launched(Arc::new(RecorderOwner::new(
                process,
                ProcessGroup::from_leader(identity.pid),
                Some(identity),
            ))),
        };
        let forced = session.force_close();
        let deadline = Instant::now() + Duration::from_secs(1);
        let killed = loop {
            if member.try_wait().expect("member status").is_some() {
                break true;
            }

            if Instant::now() >= deadline {
                break false;
            }

            thread::sleep(Duration::from_millis(1));
        };

        if !killed {
            member.kill().expect("recover member");
        }

        member.wait().expect("reap member");
        leader.wait().expect("reap original leader");

        forced.expect("zombie original identity remains group authority");

        assert!(
            killed,
            "live group member must not escape because its original leader is a zombie"
        );
    }

    #[test]
    fn acknowledged_stop_keeps_applied_when_completion_observer_is_sealed() {
        let process = Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("recorder stand-in");

        let mut session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Launched(Arc::new(RecorderOwner::new(process, None, None))),
        };

        let frame: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/conformance/locators/parity-frame.json"
        ))
        .expect("frame");

        let boundaries =
            serde_json::from_value(frame["source"]["boundaries"].clone()).expect("boundaries");

        session.seal_lifecycle_observers();
        let error = session
            .complete_after_stop(
                Reached::Live(Box::new(ControlPayload::Acknowledged { boundaries })),
                Duration::ZERO,
            )
            .expect_err("sealed observer after acknowledgement");
        let query = session
            .complete(Duration::ZERO)
            .expect_err("completion-only query is sealed");
        session.force_close().expect("reap test child");

        assert_eq!(error.kind(), crate::ErrorKind::InvalidOperation);
        assert_eq!(error.code(), "invalid_operation");
        assert_eq!(error.effect(), MutationEffect::Applied);
        assert_eq!(error.retry(), MutationEffect::Applied.retry());

        let Error::MutationContext { source, .. } = error else {
            panic!("proven effect context");
        };

        assert!(
            matches!(source.as_ref(), Error::InvalidOperation { reason, .. } if reason.contains("sealed"))
        );
        assert_eq!(query.effect(), MutationEffect::NotApplied);
    }

    #[test]
    fn stop_reports_an_unreachable_session_that_never_completes() {
        let mut session = Session {
            path: PathBuf::from("/nonexistent/termtrace/test/path"),
            helper: PathBuf::from("/unused-termtrace"),
            session_id: Uuid::now_v7(),
            ownership: Ownership::Attached,
        };
        assert!(session
            .stop(StopRequest::default(), Some(Duration::from_millis(50)))
            .is_err());
        assert!(session.stop(StopRequest::default(), None).is_err());
    }
}
