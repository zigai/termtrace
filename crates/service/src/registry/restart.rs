//! One restart orchestration used by CLI, bindings and successor extensions.

use super::{spawn_detached, RegistryService};
use crate::{
    client::{await_ready, original_capture_status, Readiness, Session, StopOutcome, StopRequest},
    Error, Result,
};
use std::{
    ffi::OsString,
    os::unix::ffi::OsStrExt as _,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use termtrace_control::read_completed;
use termtrace_evidence::session::{manifest_hash, read_manifest};
use termtrace_media::native_capture::verify_original_capture;
use termtrace_model::{
    artifact::original::OriginalCaptureState,
    execution::{environment::EnvironmentSpec, layout::SessionLayout, session::SessionLineage},
    protocol::management::{
        ManagementFailure, RestartAttempt, RestartFailureFacts, RestartFailurePhase,
        SuccessorReadiness,
    },
    registry::{RestartConfiguration, RestartReport},
    value::ByteString,
};

impl RegistryService {
    /// Typed recorder launch through shared restart admission. No retry or rollback semantics.
    /// # Errors
    /// Propagates invalid configuration, predecessor, readiness/lineage and alias-CAS failures.
    pub fn restart(
        &self,
        binary: &Path,
        alias: &str,
        overrides: &RestartConfiguration,
        environment: Option<&EnvironmentSpec>,
        timeout: Duration,
    ) -> Result<RestartReport> {
        let admission = |error: Error| {
            return error.with_management(ManagementFailure::RegistryRestart(
                RestartFailureFacts {
                    alias: alias.into(),
                    phase: RestartFailurePhase::ResolvePredecessor,
                    predecessor_path: None,
                    predecessor_completion: None,
                    successor: None,
                },
            ));
        };
        if overrides.command.as_ref().is_some_and(|command| {
            return command.is_empty()
                || command
                    .first()
                    .is_some_and(|arg| return arg.as_bytes().is_empty());
        }) || overrides.cols == Some(0)
            || overrides.rows == Some(0)
            || overrides.cell_width == Some(0)
            || overrides.cell_height == Some(0)
            || (overrides.clear_output_byte_budget && overrides.output_byte_budget.is_some())
            || (overrides.clear_max_duration && overrides.max_duration_ms.is_some())
            || (overrides.clear_idle_timeout && overrides.idle_timeout_ms.is_some())
        {
            return Err(admission(Error::InvalidOperation {
                reason: "invalid or contradictory restart overrides".into(),
                source: None,
            }));
        }

        let encoded = serde_json::to_string(overrides)
            .map_err(|source| {
                return Error::InvalidJson {
                    path: "<restart-options>".into(),
                    json_path: "$".into(),
                    source,
                };
            })
            .map_err(admission)?;
        let stdin = environment
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|source| {
                return Error::InvalidJson {
                    path: "<environment-spec-stdin>".into(),
                    json_path: "$".into(),
                    source,
                };
            })
            .map_err(admission)?;

        return self.restart_with(
            binary,
            alias,
            timeout,
            stdin.as_deref(),
            |predecessor, session| {
                let mut args = vec![
                    OsString::from("record"),
                    "--no-input".into(),
                    "--out".into(),
                    session.as_os_str().into(),
                    "--session-alias".into(),
                    alias.into(),
                    "--restart-source".into(),
                    predecessor.as_os_str().into(),
                    "--restart-overrides".into(),
                    encoded.into(),
                ];

                if stdin.is_some() {
                    args.push("--environment-spec-stdin".into());
                }

                args.extend([OsString::from("--"), "/bin/true".into()]);
                return Ok(args);
            },
        );
    }

    /// Faithful original-capture successor: same profile, initial grid, canvas and rate.
    ///
    /// The predecessor capture must publish before the successor starts; its supervisor is
    /// given `timeout` to finish after canonical completion.
    /// # Errors
    /// Same as [`Self::restart_with`], plus a predecessor capture that did not publish.
    pub fn restart_original(
        &self,
        binary: &Path,
        alias: &str,
        environment: Option<&EnvironmentSpec>,
        timeout: Duration,
    ) -> Result<RestartReport> {
        let stdin = environment
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|source| {
                return Error::InvalidJson {
                    path: "<environment-spec-stdin>".into(),
                    json_path: "$".into(),
                    source,
                };
            })?;

        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| return client_error("restart timeout is out of range"))?;

        return self.restart_with(
            binary,
            alias,
            timeout,
            stdin.as_deref(),
            |predecessor, session| {
                await_published_capture(predecessor, deadline)?;

                let mut args = vec![
                    OsString::from("record"),
                    "--original-native".into(),
                    "--out".into(),
                    session.as_os_str().into(),
                    "--session-alias".into(),
                    alias.into(),
                    "--restart-source".into(),
                    predecessor.as_os_str().into(),
                ];

                if stdin.is_some() {
                    args.push("--environment-spec-stdin".into());
                }

                args.extend([OsString::from("--"), "/bin/true".into()]);
                return Ok(args);
            },
        );
    }

    /// Successor extension hook: callback constructs recorder arguments, while this operation
    /// owns predecessor completion, readiness/lineage checks, orphan diagnostics and alias CAS.
    /// # Errors
    /// Failed successor admission preserves the predecessor alias unless concurrently changed by
    /// another owner; exact recoverable source/log paths remain in the error.
    pub fn restart_with(
        &self,
        binary: &Path,
        alias: &str,
        timeout: Duration,
        stdin: Option<&[u8]>,
        arguments: impl FnOnce(&Path, &Path) -> Result<Vec<OsString>>,
    ) -> Result<RestartReport> {
        let mut facts = RestartFailureFacts {
            alias: alias.into(),
            phase: RestartFailurePhase::ResolvePredecessor,
            predecessor_path: None,
            predecessor_completion: None,
            successor: None,
        };

        let result = (|| {
            let predecessor_entry = self.registry.resolve(alias)?;
            let predecessor = predecessor_entry.session_path();
            facts.predecessor_path = Some(ByteString::from(predecessor.as_os_str().as_bytes()));
            facts.phase = RestartFailurePhase::StoppingPredecessor;
            let StopOutcome::Completed { completion } = Session::attach(&predecessor, binary)?
                .stop(StopRequest::default(), Some(timeout))?
            else {
                return Err(Error::InvalidOperation {
                    reason: "predecessor stop did not wait for completion".into(),
                    source: None,
                });
            };
            facts.predecessor_completion = Some(completion.clone());
            facts.phase = RestartFailurePhase::PredecessorCompleted;
            let manifest_path = SessionLayout::new(&predecessor).manifest();
            let bytes = fs_err::read(&manifest_path).map_err(|source| {
                return Error::Filesystem {
                    path: manifest_path,
                    source,
                };
            })?;

            let expected = SessionLineage {
                predecessor_session_id: predecessor_entry.session_id,
                predecessor_manifest_hash: manifest_hash(&bytes),
                predecessor_final_event_hash: completion.final_event_hash.clone(),
                reason: "restart".into(),
            };

            let paths = self.registry.allocate_detached_paths()?;
            facts.successor = Some(RestartAttempt {
                session: ByteString::from(paths.session.as_os_str().as_bytes()),
                stdout_log: ByteString::from(paths.stdout_log.as_os_str().as_bytes()),
                stderr_log: ByteString::from(paths.stderr_log.as_os_str().as_bytes()),
                launched: false,
                readiness: None,
            });

            facts.phase = RestartFailurePhase::SuccessorAllocated;
            let orphan = |reason: String| {
                return Error::RecorderStartup { reason: format!("{reason}; alias may have changed concurrently; recoverable orphan: {}; stdout log: {}; stderr log: {}", paths.session.display(), paths.stdout_log.display(), paths.stderr_log.display()) };
            };

            let args = arguments(&predecessor, &paths.session)?;
            let deadline = Instant::now()
                .checked_add(timeout)
                .ok_or_else(|| return orphan("restart timeout overflow".into()))?;

            let mut recorder = spawn_detached(binary, args, &paths, stdin)?;
            let attempt = facts
                .successor
                .as_mut()
                .ok_or_else(|| return orphan("missing successor allocation".into()))?;
            attempt.launched = true;
            facts.phase = RestartFailurePhase::SuccessorLaunched;
            let readiness = await_ready(&paths.session, &mut recorder, deadline);
            // Transfer the actual Child on every post-spawn path, including failed admission.
            thread::spawn(move || {
                let _status = recorder.wait();
            });

            attempt.readiness = Some(match &readiness {
                Ok(Readiness::Live) => SuccessorReadiness::Live,
                Ok(Readiness::Completed(_)) => SuccessorReadiness::Completed,
                Ok(Readiness::Exited(_)) => SuccessorReadiness::Exited,
                Ok(Readiness::TimedOut) => SuccessorReadiness::TimedOut,
                Err(_) => SuccessorReadiness::Failed,
            });

            match readiness? {
                Readiness::Live | Readiness::Completed(_) => {}
                Readiness::Exited(status) => {
                    return Err(orphan(format!(
                        "restart successor exited before readiness with {status}"
                    )))
                }
                Readiness::TimedOut => {
                    return Err(orphan(format!(
                        "restart successor did not become ready within {} ms",
                        timeout.as_millis()
                    )))
                }
            }

            facts.phase = RestartFailurePhase::SuccessorReady;
            let manifest = read_manifest(&paths.session)?;

            if manifest.lineage.as_ref() != Some(&expected) {
                return Err(orphan(
                    "successor lineage does not match the completed predecessor".into(),
                ));
            }

            facts.phase = RestartFailurePhase::AliasRetarget;
            let successor =
                self.registry
                    .retarget(alias, predecessor_entry.session_id, &paths.session)?;

            return Ok(RestartReport {
                alias: alias.into(),
                predecessor: predecessor_entry,
                successor,
                stdout_log: ByteString::from(paths.stdout_log.as_os_str().as_bytes()),
                stderr_log: ByteString::from(paths.stderr_log.as_os_str().as_bytes()),
                ready: true,
            });
        })();

        return result.map_err(|source: Error| {
            return source.with_management(ManagementFailure::RegistryRestart(facts));
        });
    }
}

fn await_published_capture(predecessor: &Path, deadline: Instant) -> Result<()> {
    loop {
        let status = original_capture_status(predecessor)?;
        match (status.state, status.result) {
            (OriginalCaptureState::Published, Some(capture)) => {
                let completion = read_completed(predecessor)?.ok_or_else(|| {
                    return client_error(
                        "predecessor capture is published without canonical completion",
                    );
                })?;

                return verify_original_capture(predecessor, &completion, &capture).map_err(
                    |error| {
                        return client_error(format!(
                            "predecessor original capture does not verify: {error}"
                        ));
                    },
                );
            }
            (
                OriginalCaptureState::Acquiring
                | OriginalCaptureState::Live
                | OriginalCaptureState::Finalizing,
                _,
            ) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            (state, _) => {
                return Err(client_error(format!(
                    "predecessor original capture is {state:?}, not published; a successor would break capture continuity"
                )));
            }
        }
    }
}

fn client_error(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
        source: None,
    };
}
