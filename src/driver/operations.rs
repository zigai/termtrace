use super::*;

use crate::screen_frame::ScreenFrameError;

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
                    let path = self.sessions.get(handle).ok_or_else(|| {
                        return driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        );
                    })?;
                    let mut control = ControlRequest::new(command.clone());
                    control.request_id = request.request_id;
                    control.expected_presented_state_hash = expected_presented_state_hash.clone();
                    match send_request_response(path, &control) {
                        Ok(response) => match response.result {
                            ControlResult::Ok { data } => Ok(DriverData::Control { data }),
                            ControlResult::Error {
                                code,
                                message,
                                details,
                            } => {
                                let (retry_safety, uncertain) =
                                    actor_failure_retry(&code, details.as_ref());
                                Err(driver_failure_with_retry(
                                    &code,
                                    message,
                                    Some(path.clone()),
                                    retry_safety,
                                    uncertain,
                                    details,
                                ))
                            }
                        },
                        Err(error) => Err(driver_control_failure_from_error(
                            error,
                            path.clone(),
                            control_command_mutates(command),
                        )),
                    }
                }
                DriverMethod::WaitOutput {
                    handle,
                    data,
                    timeout_ms,
                } => {
                    let path = self.sessions.get(handle).ok_or_else(|| {
                        return driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        );
                    })?;
                    wait_for_output(path, data.as_bytes(), Duration::from_millis(*timeout_ms))
                        .map(|outcome| return DriverData::OutputWait { outcome })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::Completion { handle } => {
                    let path = self.sessions.get(handle).ok_or_else(|| {
                        return driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        );
                    })?;
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
                    if !self.launched.contains(handle) {
                        return Err(driver_failure(
                            "evidence_not_driver_owned",
                            "only evidence launched by this driver can be discarded",
                            self.sessions.get(handle).cloned(),
                        ));
                    }
                    let path = self.session_path(handle)?.clone();
                    let _log = completed_driver_log(&path)?;
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
                    self.sessions.remove(handle);
                    self.launched.remove(handle);
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
                    if path.join("completion.json").is_file() {
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
                                        details,
                                    },
                                ..
                            }) => {
                                let (retry_safety, uncertain) =
                                    actor_failure_retry(&code, details.as_ref());
                                Err(driver_failure_with_retry(
                                    &code,
                                    message,
                                    Some(path.clone()),
                                    retry_safety,
                                    uncertain,
                                    details,
                                ))
                            }
                            Err(error) => Err(driver_control_failure_from_error(
                                error,
                                path.clone(),
                                false,
                            )),
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
                    let log = completed_driver_log(path)?;
                    build_timeline_page(&log, *all_states, *controls, None, *offset, *limit)
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
                    let log = completed_driver_log(path)?;
                    diff_states(&log, from.clone(), to.clone())
                        .map(|difference| {
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
                    let log = completed_driver_log(path)?;
                    diagnose(path, &log, checks.clone(), *broad_change_percent)
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
                    let log = EventLog::read(path.join("events.jsonl")).map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })?;
                    let events = log
                        .events
                        .iter()
                        .filter(|record| return record.seq >= *from_event_seq)
                        .take(usize::from(*limit))
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
                } => {
                    let path = self.session_path(handle)?;
                    match renderer {
                        DriverScreenshotRenderer::Semantic => {
                            save(path, destination, point.clone(), &[SaveFormat::Png], false)
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
                                })
                        }
                        DriverScreenshotRenderer::Native => {
                            let log = completed_driver_log(path)?;
                            let inspection = inspect_point(&log, point).map_err(|error| {
                                return driver_failure_from_error(error, Some(path.clone()));
                            })?;
                            if !inspection.record.presented {
                                return Err(driver_failure(
                                    "hidden_native_state",
                                    "native screenshots require a presentation-eligible state",
                                    Some(path.clone()),
                                ));
                            }
                            let event_seq = inspection.record.event_seq.ok_or_else(|| {
                                return driver_failure(
                                    "missing_native_endpoint",
                                    "the initial state has no native replay endpoint",
                                    Some(path.clone()),
                                );
                            })?;
                            require_capability(NATIVE_SCREENSHOT_CAPABILITY).map_err(|error| {
                                return driver_failure_from_error(error, Some(path.clone()));
                            })?;
                            let (target, geometry_solver) = native_capture_target(path, *viewport)
                                .map_err(|error| {
                                    return driver_failure_from_error(error, Some(path.clone()));
                                })?;
                            capture(&CaptureOptions {
                                session_directory: path.clone(),
                                width: target.width,
                                height: target.height,
                                fps: 24,
                                start_monotonic_ns: None,
                                end_monotonic_ns: None,
                                state_selection: StateSelection::Presented,
                                geometry_solver,
                                kind: CaptureKind::Screenshot {
                                    state_id: inspection.record.state_id,
                                    state_hash: inspection.record.state_hash,
                                    event_seq,
                                    event_byte_offset: inspection.record.event_byte_offset,
                                },
                                destination: Some(destination.clone()),
                            })
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
                } => {
                    let path = self.session_path(handle)?;
                    let manifest = create_bundle(path, destination, *profile).map_err(|error| {
                        return driver_failure_from_error(error, Some(path.clone()));
                    })?;
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
                    self.launched.remove(handle);
                    if self.sessions.remove(handle).is_none() {
                        return Err(driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        ));
                    }
                    self.detach_recorder(handle);
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
        return self.sessions.get(handle).ok_or_else(|| {
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
        self.sessions.insert(handle, canonical.clone());
        return Ok(DriverData::Attached {
            handle,
            session_id: manifest.session_id,
            completed: canonical.join("completion.json").is_file(),
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
        let (canonical_session, recorder) = session.into_detached_parts();
        let handle = Uuid::now_v7();
        self.sessions.insert(handle, canonical_session.clone());
        self.launched.insert(handle);
        if let Some(recorder) = recorder {
            self.recorders.insert(handle, recorder);
        }
        return Ok(DriverData::Attached {
            handle,
            session_id,
            canonical_session,
            completed: false,
        });
    }

    fn reap_recorder(&mut self, handle: &Uuid, path: &Path) -> StdResult<(), Box<DriverResult>> {
        let Some(mut recorder) = self.recorders.remove(handle) else {
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
        let Some(mut recorder) = self.recorders.remove(handle) else {
            return;
        };
        thread::spawn(move || {
            if let Err(error) = recorder.wait() {
                tracing::warn!(%error, "failed to reap detached driver recorder");
            }
        });
    }
}

pub(super) fn completed_driver_log(path: &Path) -> StdResult<EventLog, Box<DriverResult>> {
    let log = EventLog::read(path.join("events.jsonl"))
        .map_err(|error| return driver_failure_from_error(error, Some(path.to_path_buf())))?;
    validate_completed_log(path, &log)
        .map_err(|error| return driver_failure_from_error(error, Some(path.to_path_buf())))?;
    return Ok(log);
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

pub(super) fn driver_failure_from_error(
    error: Error,
    evidence: Option<PathBuf>,
) -> Box<DriverResult> {
    let (code, message) = match &error {
        Error::CapabilityBlocked {
            id,
            code,
            detail,
            remediation,
        } => (
            "capability_blocked".to_owned(),
            format!(
                "required capability `{id}` is blocked ({code}): {detail}; fix: {remediation}; inspect with `termtrace tooling doctor --require {id} --explain`"
            ),
        ),
        Error::ControlRejected { code, .. } => (code.clone(), error.to_string()),
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
    return driver_failure(&code, message, evidence);
}

pub(super) fn driver_control_failure_from_error(
    error: Error,
    evidence: PathBuf,
    mutation: bool,
) -> Box<DriverResult> {
    let (code, dispatched) = match &error {
        Error::ControlUnavailable { .. } => ("control_unavailable", false),
        Error::ControlTimeout {
            request_dispatched, ..
        } => ("control_timeout", *request_dispatched),
        Error::ControlTransport {
            request_dispatched, ..
        } => ("control_transport", *request_dispatched),
        Error::InvalidControlProtocol { .. } | Error::InvalidJson { .. } => {
            ("invalid_control_response", true)
        }
        Error::ControlRejected { code, .. } => {
            return driver_failure_with_retry(
                code,
                error.to_string(),
                Some(evidence),
                "safe_new_request",
                false,
                None,
            );
        }
        _ => ("control_failed", true),
    };
    let uncertain = mutation && dispatched;
    return driver_failure_with_retry(
        code,
        error.to_string(),
        Some(evidence),
        if uncertain {
            "safe_same_request_id"
        } else {
            "safe_new_request"
        },
        uncertain,
        None,
    );
}

pub(super) fn actor_failure_retry(
    code: &str,
    details: Option<&ControlErrorDetails>,
) -> (&'static str, bool) {
    if details.is_some_and(|value| return value.scope == "action_terminating_screen_frame") {
        return ("safe_same_request_id", false);
    }
    if code == "input_sequence_incomplete" {
        let uncertain = details
            .and_then(|value| return value.input_sequence.as_ref())
            .is_some_and(|sequence| return sequence.failed_atom_bytes_written != 0);
        return ("safe_same_request_id", uncertain);
    }
    if code == "input_write_failed" {
        return ("safe_same_request_id", true);
    }
    return ("safe_new_request", false);
}

pub(super) fn control_command_mutates(command: &ControlCommand) -> bool {
    return !matches!(
        command,
        ControlCommand::Ping
            | ControlCommand::Status
            | ControlCommand::ScreenFrame { .. }
            | ControlCommand::Snapshot
            | ControlCommand::WaitScreen { .. }
            | ControlCommand::CaptureStable { .. }
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

pub(super) fn driver_failure(
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
) -> Box<DriverResult> {
    return driver_failure_with_retry(code, message, evidence, "safe_new_request", false, None);
}

pub(super) fn driver_failure_with_retry(
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
    retry_safety: &str,
    uncertain: bool,
    details: Option<ControlErrorDetails>,
) -> Box<DriverResult> {
    return Box::new(DriverResult::Error {
        code: code.to_owned(),
        message: message.into(),
        retry_safety: retry_safety.to_owned(),
        uncertain,
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

#[cfg(test)]
mod retry_tests {
    use crate::{
        control::{ActorBoundaries, ControlErrorDetails},
        event_log::CanonicalBoundary,
    };

    use super::actor_failure_retry;

    #[test]
    fn post_apply_terminating_frame_requires_same_action_id() {
        let boundary = CanonicalBoundary {
            next_event_seq: 2,
            event_hash: "a".repeat(64),
            output_bytes: 3,
        };
        let details = ControlErrorDetails {
            current_presented_state_hash: "b".repeat(64),
            boundaries: ActorBoundaries {
                durable: boundary.clone(),
                observed: boundary,
            },
            scope: "action_terminating_screen_frame".to_owned(),
            input_sequence: None,
            screen_frame: None,
        };

        assert_eq!(
            actor_failure_retry("screen_frame_resource_limit", Some(&details)),
            ("safe_same_request_id", false)
        );
        assert_eq!(
            actor_failure_retry("screen_frame_resource_limit", None),
            ("safe_new_request", false)
        );
    }
}
