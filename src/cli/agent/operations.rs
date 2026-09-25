use super::runtime::{Driver, SessionOwnership};
use crate::cli::tooling::require_capability;
use std::{
    env,
    ffi::OsString,
    os::unix::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    result::Result as StdResult,
    thread,
    time::Duration,
};
use termtrace_control::{
    is_completed, read_completed, send_request_response, Error as ControlError,
};
use termtrace_engine::{
    analysis::diagnostics::diagnose_session,
    diff::diff_session_states_with_semantic_states,
    inspection::{wait_for_completion, wait_for_output},
    observation::{resolved_screen_state_for_session, CaptureQuality},
    registry::SessionRegistry,
    timeline::build_session_timeline_page,
};
use termtrace_evidence::{
    artifact::sync_directory,
    event_log::EventLog,
    session::{read_completion, read_manifest, validate_completed_session, verify_session},
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
    protocol::control::{
        ControlCommand, ControlData, ControlErrorDetails, ControlRequest, ControlResponse,
        ControlResult, CONTROL_PROTOCOL_VERSION,
    },
    protocol::mutation::MutationEffect,
    terminal::observation::StatePoint,
    terminal::replay::STATE_FORMAT_VERSION,
    terminal::screen::ScreenFrameError,
    value::ByteString,
};
use termtrace_sdk::{observe_session_with_options, Error, Result, Terminal};
use termtrace_terminal::project_screen_frame;
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
                    let path = self.session_path(handle)?;
                    let mut control = ControlRequest::new(command.clone());
                    control.request_id = request.request_id;
                    control
                        .expected_presented_state_hash
                        .clone_from(expected_presented_state_hash);

                    match send_request_response(path, &control) {
                        Ok(response) => match response.result {
                            ControlResult::Ok { data } => Ok(DriverData::Control { data }),
                            ControlResult::Error {
                                code,
                                message,
                                effect,
                                details,
                            } => Err(driver_failure_with_effect(
                                &code,
                                message,
                                Some(path.clone()),
                                effect,
                                details,
                            )),
                        },
                        Err(error) => Err(driver_control_failure_from_error(error, path.clone())),
                    }
                }
                DriverMethod::WaitOutput {
                    handle,
                    data,
                    timeout_ms,
                } => {
                    let path = self.session_path(handle)?;

                    wait_for_output(path, data.as_bytes(), Duration::from_millis(*timeout_ms))
                        .map(|outcome| return DriverData::OutputWait { outcome })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::Completion { handle } => {
                    let path = self.session_path(handle)?;

                    read_completion(path)
                        .map(|completion| return DriverData::Completion { completion })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::Complete { handle, timeout_ms } => {
                    let path = self.session_path(handle)?.clone();
                    let completion = wait_for_completion(&path, Duration::from_millis(*timeout_ms))
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })?;

                    self.reap_recorder(handle, &path)?;

                    Ok(DriverData::Completion { completion })
                }
                DriverMethod::DiscardCompleted { handle } => {
                    let session = self.sessions.get(handle).ok_or_else(|| {
                        return driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        );
                    })?;

                    if !matches!(session.ownership, SessionOwnership::Launched { .. }) {
                        return Err(driver_failure(
                            "evidence_not_driver_owned",
                            "only evidence launched by this driver can be discarded",
                            Some(session.path.clone()),
                        ));
                    }

                    let path = session.path.clone();
                    validate_completed_session(&path).map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })?;

                    self.reap_recorder(handle, &path)?;
                    fs_err::remove_dir_all(&path).map_err(|error| {
                        return driver_failure(
                            "evidence_discard_failed",
                            error.to_string(),
                            Some(path.clone()),
                        );
                    })?;

                    if let Some(parent) = path.parent() {
                        sync_directory(parent).map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })?;
                    }

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
                    let path = self.session_path(handle)?;

                    observe_session_with_options(path, point.clone(), *include_hidden)
                        .map(|observation| {
                            return DriverData::Observation {
                                observation: Box::new(observation),
                            };
                        })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::ScreenFrame {
                    handle,
                    point,
                    include_hidden,
                    projection,
                } => {
                    let path = self.session_path(handle)?;

                    let completed = read_completed(path)
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })?
                        .is_some();

                    if completed {
                        let (state, source, presented) =
                            resolved_screen_state_for_session(path, point.clone(), *include_hidden)
                                .map_err(|error| {
                                    return driver_failure_from_error(error, Some(path.clone()));
                                })?;

                        project_screen_frame(
                            &state,
                            projection,
                            source,
                            presented,
                            CaptureQuality::Completed,
                        )
                        .map(|frame| return DriverData::ScreenFrame { frame })
                        .map_err(|error| {
                            return driver_failure_from_error(
                                Error::ScreenFrame(error),
                                Some(path.clone()),
                            );
                        })
                    } else {
                        if point != &StatePoint::LatestPresented || *include_hidden {
                            return Err(driver_failure(
                                "invalid_state_point",
                                "live rich frames support only latest-presented selection without hidden-state permission",
                                Some(path.clone()),
                            ));
                        }

                        let control = ControlRequest {
                            version: CONTROL_PROTOCOL_VERSION,
                            request_id: request.request_id,
                            expected_presented_state_hash: None,
                            command: ControlCommand::ScreenFrame {
                                projection: *projection,
                            },
                        };

                        match send_request_response(path, &control) {
                            Ok(ControlResponse {
                                result:
                                    ControlResult::Ok {
                                        data: ControlData::ScreenFrame { frame },
                                    },
                                ..
                            }) => Ok(DriverData::ScreenFrame { frame }),
                            Ok(ControlResponse {
                                result: ControlResult::Ok { data: _ },
                                ..
                            }) => Err(driver_failure(
                                "invalid_control_response",
                                "actor returned the wrong rich-screen payload",
                                Some(path.clone()),
                            )),
                            Ok(ControlResponse {
                                result:
                                    ControlResult::Error {
                                        code,
                                        message,
                                        effect,
                                        details,
                                    },
                                ..
                            }) => Err(driver_failure_with_effect(
                                &code,
                                message,
                                Some(path.clone()),
                                effect,
                                details,
                            )),
                            Err(error) => {
                                Err(driver_control_failure_from_error(error, path.clone()))
                            }
                        }
                    }
                }
                DriverMethod::Timeline {
                    handle,
                    all_states,
                    controls,
                    offset,
                    limit,
                } => {
                    let path = self.session_path(handle)?;

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
                    let path = self.session_path(handle)?;

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
                    let path = self.session_path(handle)?;

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
                    let path = self.session_path(handle)?;

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
                    let path = self.session_path(handle)?.clone();

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
                    let path = self.session_path(handle)?;

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
                    let path = self.session_path(handle)?;
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
                    self.detach_recorder(handle);
                    if self.sessions.remove(handle).is_none() {
                        return Err(driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        ));
                    }

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

    fn session_path(&self, handle: &Uuid) -> StdResult<&PathBuf, Box<DriverResult>> {
        return self
            .sessions
            .get(handle)
            .map(|s| return &s.path)
            .ok_or_else(|| {
                return driver_failure("unknown_handle", "session handle is not attached", None);
            });
    }

    fn attach(&mut self, reference: &Path) -> StdResult<DriverData, Box<DriverResult>> {
        let path = resolve_reference(reference)
            .map_err(|error| return driver_failure_from_error(error, None))?;

        let canonical = path.canonicalize().map_err(|error| {
            return driver_failure("session_unavailable", error.to_string(), Some(path.clone()));
        })?;
        let manifest = read_manifest(&canonical)
            .map_err(|error| return driver_failure_from_error(error, Some(canonical.clone())))?;

        let handle = Uuid::now_v7();
        self.sessions.insert(
            handle,
            super::runtime::AttachedSession {
                path: canonical.clone(),
                ownership: super::runtime::SessionOwnership::External,
            },
        );

        return Ok(DriverData::Attached {
            handle,
            session_id: manifest.session_id,
            completed: is_completed(&canonical),
            canonical_session: canonical,
        });
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

        let terminal = Terminal::new(binary, evidence_root);
        let command = command
            .iter()
            .map(|argument| return OsString::from_vec(argument.as_bytes().to_vec()))
            .collect::<Vec<_>>();

        let mut builder = terminal
            .launch(command)
            .geometry(cols, rows)
            .environment(environment);

        if let Some(limit) = resource_limits.output_bytes {
            builder = builder.output_byte_budget(limit);
        }

        if let Some(directory) = working_directory {
            builder = builder.working_directory(directory);
        }

        let session = builder.start().map_err(|error| {
            return driver_failure_from_error(error, Some(evidence_root.to_path_buf()));
        })?;

        let session_id = session.session_id();
        let completed = is_completed(session.path());
        let (canonical_session, recorder) = session.into_detached_parts();
        let handle = Uuid::now_v7();
        self.sessions.insert(
            handle,
            super::runtime::AttachedSession {
                path: canonical_session.clone(),
                ownership: super::runtime::SessionOwnership::Launched { recorder },
            },
        );

        return Ok(DriverData::Attached {
            handle,
            session_id,
            canonical_session,
            completed,
        });
    }

    fn reap_recorder(&mut self, handle: &Uuid, path: &Path) -> StdResult<(), Box<DriverResult>> {
        let Some(session) = self.sessions.get_mut(handle) else {
            return Ok(());
        };
        let super::runtime::SessionOwnership::Launched { recorder } = &mut session.ownership else {
            return Ok(());
        };
        let Some(mut recorder) = recorder.take() else {
            return Ok(());
        };
        recorder.wait().map_err(|error| {
            return driver_failure(
                "recorder_wait_failed",
                error.to_string(),
                Some(path.to_path_buf()),
            );
        })?;

        return Ok(());
    }

    fn detach_recorder(&mut self, handle: &Uuid) {
        let Some(session) = self.sessions.get_mut(handle) else {
            return;
        };
        let super::runtime::SessionOwnership::Launched { recorder } = &mut session.ownership else {
            return;
        };
        let Some(mut recorder) = recorder.take() else {
            return;
        };

        thread::spawn(move || {
            if let Err(error) = recorder.wait() {
                tracing::warn!(%error, "failed to reap detached driver recorder");
            }
        });
    }
}

pub(super) fn resolve_reference(reference: &Path) -> Result<PathBuf> {
    let text = reference.to_string_lossy();
    if let Some(path) = text.strip_prefix("path:") {
        return Ok(PathBuf::from(path));
    }

    let name = text.strip_prefix("name:").unwrap_or(&text);
    if !name.contains('/') && !name.starts_with('.') {
        return Ok(SessionRegistry::open_default()?
            .resolve(name)?
            .session_path());
    }
    return Ok(reference.to_path_buf());
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
            "capability_blocked".to_owned(),
            format!(
                "required capability `{id}` is blocked ({code}): {detail}; fix: {remediation}; inspect with `termtrace doctor --require {id} --explain`"
            ),
        ),
        Error::Control(ControlError::Rejected { code, .. }) => (code.clone(), error.to_string()),
        Error::ScreenFrame(ScreenFrameError::InvalidRegion { .. }) => {
            ("invalid_screen_region".to_owned(), error.to_string())
        }
        Error::ScreenFrame(ScreenFrameError::ResourceLimit { .. }) => {
            ("screen_frame_resource_limit".to_owned(), error.to_string())
        }
        Error::ScreenFrame(_) => (
            "screen_frame_projection_failed".to_owned(),
            error.to_string(),
        ),
        _ => ("operation_failed".to_owned(), error.to_string()),
    };

    return driver_failure_with_effect(&code, message, evidence, error.effect(), None);
}

pub(super) fn driver_control_failure_from_error(
    error: ControlError,
    evidence: PathBuf,
) -> Box<DriverResult> {
    let code = match &error {
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

    return driver_failure_with_effect(
        code,
        error.to_string(),
        Some(evidence),
        error.effect(),
        None,
    );
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
