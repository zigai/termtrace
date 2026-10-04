use super::{client_error, Events, Evidence, Native, Output, Pointer, Semantic};
use crate::{
    actions::{duration_ns, ActOutcome, ActRequest, MutationGuard},
    defaults,
    screen::{StableCaptureOptions, StableScreenCapture},
    Error, Result,
};
use std::{
    io,
    path::{Path, PathBuf},
    process::Child,
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
    execution::session::SessionCompletion,
    protocol::control::{
        ClockSample, CommandError, ControlCommand, ControlPayload, ControlRequest, ScreenPredicate,
        ScreenWaitQuality, StableCaptureQuality,
    },
    terminal::{
        inspection::ScreenWaitOutcome,
        observation::{ActorBoundaries, CaptureQuality, Observation, StatePoint},
        screen::{ScreenFrame, ScreenFrameSource, ScreenProjectionRequest},
        state::SemanticState,
    },
};
use termtrace_process::ProcessGroup;
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
    Launched(Option<Recorder>),
}

struct Recorder {
    process: Child,
    recorded_group: Option<ProcessGroup>,
}

/// One stop request: the actor stops the child and finalizes evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StopRequest {
    /// Request identity and presented-state guard.
    pub guard: MutationGuard,
}

/// What [`Session::stop`] observed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StopOutcome {
    /// The actor acknowledged the request; completion was not awaited.
    Requested {
        /// Canonical positions when the actor acknowledged the stop.
        boundaries: ActorBoundaries,
    },
    /// Validated completion evidence, published before or after the request.
    Completed(Box<SessionCompletion>),
}

/// What [`Session::status`] observed.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    Completed(Box<SessionCompletion>),
}

impl Session {
    pub(super) fn open(path: PathBuf, helper: PathBuf, recorder: Option<Child>) -> Result<Self> {
        let manifest = read_manifest(&path)?;
        let ownership = recorder.map_or(Ownership::Attached, |process| {
            let recorded_group =
                match send_request(&path, &ControlRequest::new(ControlCommand::Status)) {
                    Ok(ControlPayload::Status { pid, .. }) => {
                        pid.and_then(ProcessGroup::from_leader)
                    }
                    _ => None,
                };

            return Ownership::Launched(Some(Recorder {
                process,
                recorded_group,
            }));
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
            Reached::Completed(completion) => Ok(SessionStatus::Completed(completion)),
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

    /// [`Self::capture_stable`], abandoning the exchange at `deadline` when one is given.
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
        });

        let timing = deadline.map_or_else(
            || return ExchangeTiming::Timeout(request.command.client_response_timeout()),
            ExchangeTiming::Deadline,
        );

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
        let wire = request.guard.request(ControlCommand::Stop);

        return match (self.reach(&wire)?, wait) {
            (Reached::Live(acknowledgement), None) => match *acknowledgement {
                ControlPayload::Acknowledged { boundaries } => {
                    Ok(StopOutcome::Requested { boundaries })
                }
                _ => Err(client_error("actor returned the wrong stop payload")),
            },
            (Reached::Live(_) | Reached::Completed(_), wait) => Ok(StopOutcome::Completed(
                Box::new(self.complete(wait.unwrap_or(Duration::ZERO))?),
            )),
        };
    }

    /// Wait for validated completion evidence and reap an owned recorder.
    ///
    /// # Errors
    ///
    /// Returns an error when completion is not published in time or is invalid.
    pub fn complete(&mut self, timeout: Duration) -> Result<SessionCompletion> {
        let completion = wait_for_completion(&self.path, timeout)?;

        if let Some(mut recorder) = self.take_recorder() {
            let _status = recorder.process.wait();
        }

        return Ok(completion);
    }

    fn take_recorder(&mut self) -> Option<Recorder> {
        let Ownership::Launched(recorder) = &mut self.ownership else {
            return None;
        };

        return recorder.take();
    }

    /// Kill the recorded process group and the owned recorder after graceful stop failed.
    ///
    /// # Errors
    ///
    /// Returns an error when a kill signal or reaping the recorder fails.
    pub fn force_close(&mut self) -> Result<()> {
        let Some(mut recorder) = self.take_recorder() else {
            return Ok(());
        };

        let filesystem = |source| {
            return Error::Filesystem {
                path: self.path.clone(),
                source,
            };
        };

        let group_result = recorder
            .recorded_group
            .map_or(Ok(()), |group| return group.kill())
            .map_err(|error| return filesystem(io::Error::from(error)));

        let kill_result = recorder.process.kill().map_err(filesystem);
        let wait_result = recorder.process.wait().map(|_| ()).map_err(filesystem);

        group_result?;

        kill_result?;

        return wait_result;
    }

    /// Delete one validated completed canonical session.
    ///
    /// # Errors
    ///
    /// Returns an error when completion is missing or invalid, the recorder cannot be
    /// reaped, or the canonical session tree cannot be removed durably.
    pub fn discard_completed(&mut self) -> Result<()> {
        validate_completed_session(&self.path)?;

        if let Some(mut recorder) = self.take_recorder() {
            recorder.process.wait().map_err(|source| {
                return Error::Filesystem {
                    path: self.path.clone(),
                    source,
                };
            })?;
        }

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
    pub fn detach(mut self) -> PathBuf {
        if let Some(mut recorder) = self.take_recorder() {
            thread::spawn(move || {
                if let Err(error) = recorder.process.wait() {
                    tracing::warn!(%error, "failed to reap detached recorder");
                }
            });
        }

        return self.path;
    }
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
    use std::{process::Command, time::Instant};

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
            ownership: Ownership::Launched(Some(Recorder {
                process,
                recorded_group: None,
            })),
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
