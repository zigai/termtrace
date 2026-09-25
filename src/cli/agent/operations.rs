use super::runtime::Driver;
use crate::cli::tooling::require_capability;
use std::{
    env,
    ffi::OsString,
    os::unix::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::Duration,
};
use termtrace_control::{is_completed, Error as ControlError};
use termtrace_engine::{
    analysis::diagnostics::diagnose_session, diff::diff_session_states_with_semantic_states,
    inspection::wait_for_output, timeline::build_session_timeline_page,
};
use termtrace_evidence::{
    event_log::EventLog,
    session::{read_completion, verify_session},
};
use termtrace_export::bundle::{create as create_bundle, verify_profile as verify_bundle};
use termtrace_media::{
    native_capture::validated_visual_manifests,
    save::{save, verify_saved_artifact},
};
use termtrace_model::protocol::driver::{
    DriverData, DriverMethod, DriverRequest, DriverResponse, DriverResult,
    DriverScreenshotRenderer, DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES,
    DRIVER_PROTOCOL_VERSION,
};
use termtrace_model::{
    artifact::bundle::BundleVerificationPolicy,
    artifact::save::SaveFormat,
    execution::environment::EnvironmentSpec,
    execution::event::{EventProjection, EVENT_LOG_VERSION},
    execution::layout::SessionLayout,
    execution::session::ResourceLimits,
    protocol::capability::NATIVE_SCREENSHOT_CAPABILITY,
    protocol::control::{ControlErrorDetails, ControlRequest, CONTROL_PROTOCOL_VERSION},
    protocol::mutation::MutationEffect,
    terminal::replay::STATE_FORMAT_VERSION,
    terminal::screen::ScreenFrameError,
    value::ByteString,
};
use termtrace_sdk::{
    session::{LaunchSpec, Session, SessionTarget},
    Error, Terminal,
};
use uuid::Uuid;

impl Driver {
    pub(super) fn execute(&mut self, request: &DriverRequest) -> DriverResponse {
        let result = (|| -> StdResult<DriverData, Box<DriverResult>> {
            return match &request.method {
                DriverMethod::Hello => Ok(DriverData::Hello {
                    driver_protocol_version: DRIVER_PROTOCOL_VERSION,
                    control_protocol_version: CONTROL_PROTOCOL_VERSION,
                    event_log_version: EVENT_LOG_VERSION,
                    state_format_version: STATE_FORMAT_VERSION,
                    methods: [
                        "hello",
                        "launch",
                        "attach",
                        "control",
                        "wait_output",
                        "completion",
                        "complete",
                        "discard_completed",
                        "observe",
                        "screen_frame",
                        "timeline",
                        "diff",
                        "diagnose",
                        "events",
                        "screenshot",
                        "verify",
                        "bundle",
                        "detach",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    max_request_bytes: DRIVER_MAX_REQUEST_BYTES,
                    max_response_bytes: DRIVER_MAX_RESPONSE_BYTES,
                }),
                DriverMethod::Launch {
                    evidence_root,
                    command,
                    cols,
                    rows,
                    working_directory,
                    environment,
                    resource_limits,
                } => self.launch(
                    evidence_root,
                    command,
                    *cols,
                    *rows,
                    working_directory.as_deref(),
                    environment.clone(),
                    resource_limits,
                ),
                DriverMethod::Attach { session } => self.attach(session),
                DriverMethod::Control {
                    handle,
                    command,
                    expected_presented_state_hash,
                } => {
                    let session = self.session(handle)?;
                    let mut control = ControlRequest::new(command.clone());
                    control.request_id = request.request_id;
                    control
                        .expected_presented_state_hash
                        .clone_from(expected_presented_state_hash);

                    session
                        .control(&control)
                        .map(|data| return DriverData::Control { data })
                        .map_err(|error| return session_failure(error, session))
                }
                DriverMethod::WaitOutput {
                    handle,
                    data,
                    timeout_ms,
                } => {
                    let session = self.session(handle)?;

                    wait_for_output(
                        session.path(),
                        data.as_bytes(),
                        Duration::from_millis(*timeout_ms),
                    )
                    .map(|outcome| return DriverData::OutputWait { outcome })
                    .map_err(|error| return session_failure(error, session))
                }
                DriverMethod::Completion { handle } => {
                    let session = self.session(handle)?;

                    read_completion(session.path())
                        .map(|completion| return DriverData::Completion { completion })
                        .map_err(|error| return session_failure(error, session))
                }
                DriverMethod::Complete { handle, timeout_ms } => {
                    let session = self.session_mut(handle)?;

                    session
                        .complete(Duration::from_millis(*timeout_ms))
                        .map(|completion| return DriverData::Completion { completion })
                        .map_err(|error| return session_failure(error, session))
                }
                DriverMethod::DiscardCompleted { handle } => {
                    let session = self.session_mut(handle)?;

                    if !session.launched() {
                        return Err(driver_failure(
                            "evidence_not_driver_owned",
                            "only evidence launched by this driver can be discarded",
                            Some(session.path().to_path_buf()),
                        ));
                    }

                    session
                        .discard_completed()
                        .map_err(|error| return session_failure(error, session))?;

                    self.close_native(handle);
                    self.sessions.remove(handle);

                    Ok(DriverData::Discarded {
                        evidence_removed: true,
                    })
                }
                DriverMethod::Observe {
                    handle,
                    point,
                    include_hidden,
                } => {
                    let session = self.session(handle)?;

                    session
                        .observe(point.clone(), *include_hidden)
                        .map(|observation| {
                            return DriverData::Observation {
                                observation: Box::new(observation),
                            };
                        })
                        .map_err(|error| return session_failure(error, session))
                }
                DriverMethod::ScreenFrame {
                    handle,
                    point,
                    include_hidden,
                    projection,
                } => {
                    let session = self.session(handle)?;

                    session
                        .frame_at(point.clone(), *include_hidden, *projection)
                        .map(|frame| return DriverData::ScreenFrame { frame })
                        .map_err(|error| return session_failure(error, session))
                }
                DriverMethod::Timeline {
                    handle,
                    all_states,
                    controls,
                    offset,
                    limit,
                } => {
                    let path = &self.session(handle)?.path().to_path_buf();

                    if *limit == 0 || *limit > 4096 {
                        return Err(driver_failure(
                            "invalid_timeline_limit",
                            "timeline limit must be from 1 through 4096",
                            Some(path.clone()),
                        ));
                    }

                    build_session_timeline_page(path, *all_states, *controls, None, *offset, *limit)
                        .map(|page| {
                            return DriverData::Timeline {
                                entries: page.entries,
                                next_offset: page.next_offset,
                            };
                        })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::Diff { handle, from, to } => {
                    let path = &self.session(handle)?.path().to_path_buf();

                    diff_session_states_with_semantic_states(path, from.clone(), to.clone())
                        .map(|(difference, _, _)| {
                            return DriverData::Diff {
                                difference: Box::new(difference),
                            };
                        })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::Diagnose {
                    handle,
                    checks,
                    broad_change_percent,
                } => {
                    let path = &self.session(handle)?.path().to_path_buf();

                    if !(1..=100).contains(broad_change_percent) {
                        return Err(driver_failure(
                            "invalid_diagnostic_threshold",
                            "broad_change_percent must be from 1 through 100",
                            Some(path.clone()),
                        ));
                    }

                    let visual_evidence = validated_visual_manifests(path).map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })?;

                    diagnose_session(
                        path,
                        checks.clone(),
                        *broad_change_percent,
                        &visual_evidence,
                    )
                    .map(|report| return DriverData::Diagnose { report })
                    .map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })
                }
                DriverMethod::Events {
                    handle,
                    from_event_seq,
                    limit,
                    metadata_only,
                } => {
                    let path = &self.session(handle)?.path().to_path_buf();

                    if *limit == 0 || *limit > 4096 {
                        return Err(driver_failure(
                            "invalid_event_limit",
                            "event limit must be from 1 through 4096",
                            Some(path.clone()),
                        ));
                    }

                    let records = EventLog::read_page(
                        SessionLayout::new(path).event_log(),
                        *from_event_seq,
                        *limit,
                    )
                    .map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })?;

                    let events = records
                        .iter()
                        .map(|record| return EventProjection::from_record(record, *metadata_only))
                        .collect::<Vec<_>>();

                    let next_event_seq = events
                        .last()
                        .map_or(*from_event_seq, |event| return event.seq.saturating_add(1));

                    Ok(DriverData::Events {
                        events,
                        next_event_seq,
                    })
                }
                DriverMethod::Screenshot {
                    handle,
                    point,
                    destination,
                    renderer,
                    viewport,
                    max_bytes,
                } => {
                    let path = self.session(handle)?.path().to_path_buf();

                    match renderer {
                        DriverScreenshotRenderer::Semantic => save(
                            &path,
                            destination,
                            point.clone(),
                            &[SaveFormat::Png],
                            false,
                            *max_bytes,
                        )
                        .and_then(|outcome| {
                            verify_saved_artifact(&outcome.directory, &outcome.manifest)?;

                            return Ok(DriverData::Artifact {
                                artifact_type: "semantic_screenshot".to_owned(),
                                directory: outcome.directory.clone(),
                                primary: outcome.directory.join("screen.png"),
                                manifest: outcome.directory.join("save-manifest.json"),
                                truth_layer: "semantic_rendering".to_owned(),
                            });
                        })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        }),
                        DriverScreenshotRenderer::Native => {
                            if max_bytes.is_some() {
                                return Err(driver_failure(
                                    "unsupported_artifact_budget",
                                    "generation byte budgets are supported for semantic screenshots only",
                                    Some(path),
                                ));
                            }

                            require_capability(NATIVE_SCREENSHOT_CAPABILITY).map_err(|error| {
                                return driver_failure_from_error(error, Some(path.clone()));
                            })?;

                            self.native_screenshot(*handle, &path, point, destination, *viewport)
                                .map(|outcome| {
                                    return DriverData::Artifact {
                                        artifact_type: "native_screenshot".to_owned(),
                                        directory: outcome.directory.clone(),
                                        primary: outcome.screenshot,
                                        manifest: outcome.directory.join("manifest.json"),
                                        truth_layer: "native".to_owned(),
                                    };
                                })
                                .map_err(|error| {
                                    return driver_failure_from_error(error, Some(path.clone()));
                                })
                        }
                    }
                }
                DriverMethod::Verify { handle, level } => {
                    let path = &self.session(handle)?.path().to_path_buf();

                    Ok(DriverData::Verification {
                        report: Box::new(verify_session(path, *level)),
                    })
                }
                DriverMethod::Bundle {
                    handle,
                    destination,
                    profile,
                    max_bytes,
                } => {
                    let path = &self.session(handle)?.path().to_path_buf();
                    let manifest = create_bundle(path, destination, *profile, *max_bytes).map_err(
                        |error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        },
                    )?;

                    let policy = BundleVerificationPolicy::new(path, *profile);
                    let verified = verify_bundle(destination, &policy).map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })?;

                    if verified != manifest {
                        return Err(driver_failure(
                            "bundle_verification_failed",
                            "verified bundle manifest changed after publication",
                            Some(path.clone()),
                        ));
                    }

                    Ok(DriverData::Bundle {
                        directory: destination.clone(),
                        manifest: verified,
                    })
                }
                DriverMethod::Detach { handle } => {
                    self.close_native(handle);
                    let session = self.sessions.remove(handle).ok_or_else(|| {
                        return driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        );
                    })?;
                    let _path = session.detach();

                    Ok(DriverData::Detached)
                }
            };
        })();

        return match result {
            Ok(data) => DriverResponse {
                version: DRIVER_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: DriverResult::Ok { data },
            },
            Err(error) => DriverResponse {
                version: DRIVER_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: *error,
            },
        };
    }

    fn session(&self, handle: &Uuid) -> StdResult<&Session, Box<DriverResult>> {
        return self.sessions.get(handle).ok_or_else(|| {
            return driver_failure("unknown_handle", "session handle is not attached", None);
        });
    }

    fn session_mut(&mut self, handle: &Uuid) -> StdResult<&mut Session, Box<DriverResult>> {
        return self.sessions.get_mut(handle).ok_or_else(|| {
            return driver_failure("unknown_handle", "session handle is not attached", None);
        });
    }

    fn attach(&mut self, reference: &Path) -> StdResult<DriverData, Box<DriverResult>> {
        let binary = env::current_exe().map_err(|error| {
            return driver_failure("attach_failed", error.to_string(), None);
        })?;
        let session = SessionTarget::parse(reference.as_os_str())
            .and_then(|target| return target.path())
            .and_then(|path| return Session::attach(path, binary))
            .map_err(|error| return driver_failure_from_error(error, None))?;

        return Ok(self.insert(session));
    }

    fn launch(
        &mut self,
        evidence_root: &Path,
        command: &[ByteString],
        cols: u16,
        rows: u16,
        working_directory: Option<&Path>,
        environment: EnvironmentSpec,
        resource_limits: &ResourceLimits,
    ) -> StdResult<DriverData, Box<DriverResult>> {
        if command.is_empty() || cols == 0 || rows == 0 {
            return Err(driver_failure(
                "invalid_launch",
                "launch command and nonzero geometry are required",
                Some(evidence_root.to_path_buf()),
            ));
        }

        let binary = env::current_exe().map_err(|error| {
            return driver_failure("launch_failed", error.to_string(), None);
        })?;
        let command = command
            .iter()
            .map(|argument| return OsString::from_vec(argument.as_bytes().to_vec()));
        let mut spec = LaunchSpec::new(command)
            .geometry(cols, rows)
            .environment(environment);
        spec.output_byte_budget = resource_limits.output_bytes;
        spec.working_directory = working_directory.map(Path::to_path_buf);

        let session = Terminal::new(binary, evidence_root)
            .start(spec)
            .map_err(|error| {
                return driver_failure_from_error(error, Some(evidence_root.to_path_buf()));
            })?;

        return Ok(self.insert(session));
    }

    fn insert(&mut self, session: Session) -> DriverData {
        let handle = Uuid::now_v7();
        let data = DriverData::Attached {
            handle,
            session_id: session.session_id(),
            completed: is_completed(session.path()),
            canonical_session: session.path().to_path_buf(),
        };
        self.sessions.insert(handle, session);

        return data;
    }
}

fn session_failure(error: impl Into<Error>, session: &Session) -> Box<DriverResult> {
    return driver_failure_from_error(error, Some(session.path().to_path_buf()));
}

pub(super) fn driver_failure_from_error<E>(error: E, evidence: Option<PathBuf>) -> Box<DriverResult>
where
    E: Into<Error>,
{
    let error = error.into();
    let (code, message) = match &error {
        Error::Engine(termtrace_engine::Error::CapabilityBlocked {
            id,
            code,
            detail,
            remediation,
        }) => (
            "capability_blocked",
            format!(
                "required capability `{id}` is blocked ({code}): {detail}; fix: {remediation}; inspect with `termtrace doctor --require {id} --explain`"
            ),
        ),
        Error::Control(control) => (control_code(control), error.to_string()),
        Error::ScreenFrame(ScreenFrameError::InvalidRegion { .. }) => {
            ("invalid_screen_region", error.to_string())
        }
        Error::ScreenFrame(ScreenFrameError::ResourceLimit { .. }) => {
            ("screen_frame_resource_limit", error.to_string())
        }
        Error::ScreenFrame(_) => ("screen_frame_projection_failed", error.to_string()),
        _ => ("operation_failed", error.to_string()),
    };

    return driver_failure_with_effect(
        code,
        message,
        evidence,
        error.effect(),
        error.details().cloned(),
    );
}

fn control_code(error: &ControlError) -> &str {
    return match error {
        ControlError::Unavailable { .. } => "control_unavailable",
        ControlError::Timeout { .. } => "control_timeout",
        ControlError::Transport { .. } => "control_transport",
        ControlError::InvalidRequest { .. } => "invalid_control_request",
        ControlError::InvalidProtocol { .. }
        | ControlError::InvalidResponse { .. }
        | ControlError::InvalidJson { .. } => "invalid_control_response",
        ControlError::Rejected { code, .. } => code,
        ControlError::Filesystem { .. } | ControlError::CompletionIdentity { .. } => {
            "control_failed"
        }
    };
}

pub(super) fn driver_response_is_cacheable(response: &DriverResponse) -> bool {
    return !matches!(
        &response.result,
        DriverResult::Error { code, .. }
            if matches!(
                code.as_str(),
                "control_unavailable"
                    | "control_timeout"
                    | "control_transport"
                    | "invalid_control_response"
                    | "control_failed"
            )
    );
}

/// A failure the driver detected itself, before anything was applied.
pub(super) fn driver_failure(
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
) -> Box<DriverResult> {
    return driver_failure_with_effect(code, message, evidence, MutationEffect::NotApplied, None);
}

pub(super) fn driver_failure_with_effect(
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
    effect: MutationEffect,
    details: Option<ControlErrorDetails>,
) -> Box<DriverResult> {
    return Box::new(DriverResult::Error {
        code: code.to_owned(),
        message: message.into(),
        effect,
        evidence,
        details,
    });
}

pub(super) fn driver_error(
    request_id: Uuid,
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
) -> DriverResponse {
    return DriverResponse {
        version: DRIVER_PROTOCOL_VERSION,
        request_id,
        result: *driver_failure(code, message, evidence),
    };
}
