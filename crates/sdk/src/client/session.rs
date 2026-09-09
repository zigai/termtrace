use super::control::{
    action_atom, client_error, exact_duration_ns, public_control_error, validate_action_options,
    wire_postcondition,
};
use super::{
    derive_input_operation_id, killpg, read_manifest, send_request, send_request_response,
    sync_directory, thread, validate_completed_session, wait_for_completion, ActionCompletion,
    ActionOptions, ActionPostconditionResult, ActionResult, Child, ControlCommand, ControlData,
    ControlRequest, ControlResult, Duration, Errno, Error, Events, Evidence, InputAtom, IoError,
    Keyboard, Native, Output, Path, PathBuf, Pid, Pointer, Result, Screen, Semantic,
    SessionCompletion, Signal, StableCaptureQuality, Uuid,
};

/// High-level live/completed session handle.
pub struct Session {
    pub(super) path: PathBuf,
    pub(super) helper: PathBuf,
    pub(super) session_id: Uuid,
    pub(super) recorder: Option<Child>,
    pub(super) recorded_process_group: Option<u32>,
}

impl Session {
    pub(super) fn open(path: PathBuf, helper: PathBuf, recorder: Option<Child>) -> Result<Self> {
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
            helper,
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

    /// Native capture, playback, and presentation namespace.
    #[must_use]
    pub fn native(&self) -> Native<'_> {
        return Native { session: self };
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
                    derive_input_operation_id(request_id, index),
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
        request
            .expected_presented_state_hash
            .clone_from(&options.if_state);

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
        validate_completed_session(&self.path)?;

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

    /// Split an owned session into its canonical path and recorder process.
    ///
    /// Process adapters must retain and reap the returned child before deleting the session tree.
    #[must_use]
    pub fn into_detached_parts(mut self) -> (PathBuf, Option<Child>) {
        return (self.path, self.recorder.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, time::Instant};

    #[test]
    fn detach_reaps_the_recorder_after_it_exits() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn recorder stand-in");

        let process_path = PathBuf::from(format!("/proc/{}", child.id()));
        let session = Session {
            path: PathBuf::from("/unused"),
            helper: PathBuf::from("/unused-termtrace"),
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
}
