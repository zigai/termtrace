//! Versioned newline-framed process driver for language-neutral clients.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    io::{BufRead, Write},
    os::unix::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    analysis::{DiagnoseReport, DiagnosticCheck, diagnose},
    artifact::sync_directory,
    bundle::{BundleManifest, BundleProfile, create as create_bundle},
    client::Terminal,
    control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlRequest,
        MAX_CONTROL_MESSAGE_BYTES, send_request,
    },
    diff::{SemanticDiff, diff_states, inspect_point},
    event::{ByteString, EVENT_LOG_VERSION, EventProjection},
    event_log::EventLog,
    inspection::{WaitOutcome, wait_for_completion, wait_for_output},
    observation::{Observation, StatePoint, observe_session_with_options},
    replay::{STATE_FORMAT_VERSION, StateSelection},
    save::{SaveFormat, save},
    session::{
        CaptureTarget, SessionCompletion, VerificationLevel, VerificationReport, read_completion,
        read_manifest, validate_completed_log, verify_session,
    },
    session_registry::SessionRegistry,
    timeline::{TimelineEntry, build_timeline},
    visual::{CaptureKind, CaptureOptions, capture, native_capture_target},
};

/// Current JSONL driver protocol version.
pub const DRIVER_PROTOCOL_VERSION: u32 = 1;
const DRIVER_RESPONSE_CACHE: usize = 4096;

/// Explicit screenshot truth path; no fallback occurs between variants.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriverScreenshotRenderer {
    /// Deterministic semantic PNG.
    Semantic,
    /// Direct native frame after exact endpoint replay.
    Native,
}

/// One correlated driver request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriverRequest {
    /// Driver protocol version.
    pub version: u32,
    /// Caller-owned retry/correlation identifier.
    pub request_id: Uuid,
    /// Typed driver operation.
    pub method: DriverMethod,
}

/// Supported initial high-level driver methods.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverMethod {
    /// Return protocol and evidence capabilities.
    Hello,
    /// Launch a fresh canonical session through the high-level Rust facade.
    Launch {
        /// Owner-only root receiving collision-free evidence.
        evidence_root: PathBuf,
        /// Exact executable and argument bytes.
        command: Vec<ByteString>,
        /// Initial terminal columns.
        cols: u16,
        /// Initial terminal rows.
        rows: u16,
        /// Optional child working directory.
        working_directory: Option<PathBuf>,
    },
    /// Attach a process-local handle to a canonical session or registry name.
    Attach {
        /// Name, `name:` reference, `path:` reference, or canonical path.
        session: PathBuf,
    },
    /// Forward one typed live control command using the driver request ID.
    Control {
        /// Process-local session handle.
        handle: Uuid,
        /// Typed actor command.
        command: ControlCommand,
        /// Optional optimistic presented-state guard.
        expected_presented_state_hash: Option<String>,
    },
    /// Wait for exact bytes in canonical child output history.
    WaitOutput {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact byte sequence to find.
        data: ByteString,
        /// Bounded wait duration.
        timeout_ms: u64,
    },
    /// Read immutable completion evidence for an attached session.
    Completion {
        /// Process-local session handle.
        handle: Uuid,
    },
    /// Wait for and validate immutable completion evidence.
    Complete {
        /// Process-local session handle.
        handle: Uuid,
        /// Bounded wait duration.
        timeout_ms: u64,
    },
    /// Delete completed evidence launched by this driver under explicit policy.
    DiscardCompleted {
        /// Process-local owned session handle.
        handle: Uuid,
    },
    /// Observe one live or completed semantic state point.
    Observe {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact shared state selector.
        point: StatePoint,
        /// Permit explicitly selected hidden synchronized-output state.
        include_hidden: bool,
    },
    /// Build the completed canonical/semantic timeline.
    Timeline {
        /// Process-local session handle.
        handle: Uuid,
        /// Include non-presented semantic states.
        all_states: bool,
        /// Include exact lossless VT frames.
        controls: bool,
    },
    /// Compare two completed semantic endpoints.
    Diff {
        /// Process-local session handle.
        handle: Uuid,
        /// Earlier state point.
        from: StatePoint,
        /// Later state point.
        to: StatePoint,
    },
    /// Run conservative completed-session detectors.
    Diagnose {
        /// Process-local session handle.
        handle: Uuid,
        /// Empty means every detector.
        checks: Vec<DiagnosticCheck>,
        /// Broad repaint threshold from 1 through 100.
        broad_change_percent: u8,
    },
    /// Read bounded canonical event projections.
    Events {
        /// Process-local session handle.
        handle: Uuid,
        /// Inclusive first event sequence.
        from_event_seq: u64,
        /// Maximum returned events, bounded by the driver.
        limit: u16,
        /// Elide exact byte payloads.
        metadata_only: bool,
    },
    /// Publish one semantic or native screenshot artifact.
    Screenshot {
        /// Process-local session handle.
        handle: Uuid,
        /// Exact selected state point.
        point: StatePoint,
        /// No-replace artifact directory.
        destination: PathBuf,
        /// Explicit truth renderer.
        renderer: DriverScreenshotRenderer,
        /// Optional strict native viewport; absent runs the solver.
        viewport: Option<CaptureTarget>,
    },
    /// Verify attached completed evidence without mutation.
    Verify {
        /// Process-local session handle.
        handle: Uuid,
        /// Requested validation depth.
        level: VerificationLevel,
    },
    /// Publish one sensitivity-profiled evidence handoff.
    Bundle {
        /// Process-local session handle.
        handle: Uuid,
        /// New no-replace bundle directory.
        destination: PathBuf,
        /// Explicit sensitivity profile.
        profile: BundleProfile,
    },
    /// Release only the process-local handle.
    Detach {
        /// Process-local session handle.
        handle: Uuid,
    },
}

/// Exactly one correlated driver response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriverResponse {
    /// Driver protocol version.
    pub version: u32,
    /// Identifier copied from the request.
    pub request_id: Uuid,
    /// Typed success or stable error.
    pub result: DriverResult,
}

/// Driver success or failure envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverResult {
    /// Successful method result.
    Ok {
        /// Method-specific data.
        data: DriverData,
    },
    /// Stable structured failure.
    Error {
        /// Machine-readable code.
        code: String,
        /// Human-readable diagnostic.
        message: String,
        /// `safe_same_request_id`, `safe_new_request`, or `unknown`.
        retry_safety: String,
        /// Whether mutation application may be uncertain.
        uncertain: bool,
        /// Canonical evidence path when known.
        evidence: Option<PathBuf>,
    },
}

/// Typed driver result payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DriverData {
    /// Protocol/version handshake.
    Hello {
        /// Driver protocol version.
        driver_protocol_version: u32,
        /// Live actor protocol version.
        control_protocol_version: u32,
        /// Canonical event format version.
        event_log_version: u32,
        /// Semantic state format version.
        state_format_version: u32,
        /// Stable supported methods.
        methods: Vec<String>,
        /// Maximum newline-framed request bytes.
        max_message_bytes: u64,
    },
    /// Attached process-local handle and immutable identity.
    Attached {
        /// Process-local handle.
        handle: Uuid,
        /// Immutable canonical session identity.
        session_id: Uuid,
        /// Canonical path.
        canonical_session: PathBuf,
        /// Whether completion evidence already exists.
        completed: bool,
    },
    /// Result forwarded from the live actor.
    Control {
        /// Exact actor result data.
        data: ControlData,
    },
    /// Exact canonical output wait result.
    OutputWait {
        /// First match and its canonical position.
        outcome: WaitOutcome,
    },
    /// Immutable completion evidence.
    Completion {
        /// Validated completion envelope.
        completion: SessionCompletion,
    },
    /// Truth-labeled semantic observation.
    Observation {
        /// Shared observation envelope.
        observation: Box<Observation>,
    },
    /// Ordered timeline projection.
    Timeline {
        /// Complete requested entries.
        entries: Vec<TimelineEntry>,
    },
    /// Complete semantic endpoint diff.
    Diff {
        /// Source-bound semantic difference.
        difference: Box<SemanticDiff>,
    },
    /// Conservative diagnostic report.
    Diagnose {
        /// Source-bound findings.
        report: DiagnoseReport,
    },
    /// Bounded canonical event projection page.
    Events {
        /// Ordered event projections.
        events: Vec<EventProjection>,
        /// Next event sequence when more source may exist.
        next_event_seq: u64,
    },
    /// Published screenshot artifact handle.
    Artifact {
        /// `semantic_screenshot` or `native_screenshot`.
        artifact_type: String,
        /// Atomically committed artifact directory.
        directory: PathBuf,
        /// Primary PNG member.
        primary: PathBuf,
        /// Manifest path.
        manifest: PathBuf,
        /// Exact truth-layer label.
        truth_layer: String,
    },
    /// Offline canonical verification report.
    Verification {
        /// Stable complete report, including invalid findings.
        report: Box<VerificationReport>,
    },
    /// Published sensitivity-profiled bundle.
    Bundle {
        /// Atomically committed directory.
        directory: PathBuf,
        /// Complete bundle manifest.
        manifest: BundleManifest,
    },
    /// Process-local handle release acknowledgement.
    Detached,
    /// Explicit completed-evidence deletion acknowledgement.
    Discarded {
        /// Confirms that the canonical directory was removed.
        evidence_removed: bool,
    },
}

/// Serve requests until clean input EOF.
pub fn serve(input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut driver = Driver::default();
    for encoded in input.split(b'\n') {
        let mut encoded = encoded.map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("<driver-stdin>"),
                source,
            };
        })?;
        if encoded.is_empty() {
            continue;
        }
        encoded.push(b'\n');
        let response = driver.handle_frame(&encoded);
        serde_json::to_writer(&mut output, &response).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<driver-stdout>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        output
            .write_all(b"\n")
            .and_then(|()| return output.flush())
            .map_err(|source| {
                return Error::Filesystem {
                    path: PathBuf::from("<driver-stdout>"),
                    source,
                };
            })?;
    }
    return Ok(());
}

#[derive(Default)]
struct Driver {
    sessions: HashMap<Uuid, PathBuf>,
    launched: HashSet<Uuid>,
    cached: HashMap<Uuid, CachedDriverResponse>,
    order: VecDeque<Uuid>,
}

struct CachedDriverResponse {
    method_hash: String,
    response: DriverResponse,
}

impl Driver {
    fn handle_frame(&mut self, encoded: &[u8]) -> DriverResponse {
        if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES {
            return driver_error(
                Uuid::nil(),
                "message_too_large",
                "driver request exceeds 1 MiB",
                None,
            );
        }
        let request: DriverRequest = match serde_json::from_slice(encoded) {
            Ok(value) => value,
            Err(error) => {
                return driver_error(Uuid::nil(), "invalid_request", error.to_string(), None);
            }
        };
        let method_hash = match serde_json::to_vec(&request.method) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(error) => {
                return driver_error(
                    request.request_id,
                    "invalid_request",
                    error.to_string(),
                    None,
                );
            }
        };
        if let Some(cached) = self.cached.get(&request.request_id) {
            if cached.method_hash == method_hash {
                return cached.response.clone();
            }
            return driver_error(
                request.request_id,
                "request_id_collision",
                "driver request identifier was already used for a different method",
                None,
            );
        }
        let response = if request.version != DRIVER_PROTOCOL_VERSION {
            driver_error(
                request.request_id,
                "unsupported_version",
                format!("driver protocol version {} is unsupported", request.version),
                None,
            )
        } else {
            self.execute(&request)
        };
        self.cache(request.request_id, method_hash, response.clone());
        return response;
    }

    fn execute(&mut self, request: &DriverRequest) -> DriverResponse {
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
                    max_message_bytes: MAX_CONTROL_MESSAGE_BYTES,
                }),
                DriverMethod::Launch {
                    evidence_root,
                    command,
                    cols,
                    rows,
                    working_directory,
                } => self.launch(
                    evidence_root,
                    command,
                    *cols,
                    *rows,
                    working_directory.as_deref(),
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
                    send_request(path, &control)
                        .map(|data| return DriverData::Control { data })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
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
                    let path = self.sessions.get(handle).ok_or_else(|| {
                        return driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        );
                    })?;
                    wait_for_completion(path, Duration::from_millis(*timeout_ms))
                        .map(|completion| return DriverData::Completion { completion })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
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
                DriverMethod::Timeline {
                    handle,
                    all_states,
                    controls,
                } => {
                    let path = self.session_path(handle)?;
                    let log = completed_driver_log(path)?;
                    build_timeline(&log, *all_states, *controls, None)
                        .map(|entries| return DriverData::Timeline { entries })
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
                                .map(|outcome| {
                                    return DriverData::Artifact {
                                        artifact_type: "semantic_screenshot".to_owned(),
                                        directory: outcome.directory.clone(),
                                        primary: outcome.directory.join("screen.png"),
                                        manifest: outcome.directory.join("save-manifest.json"),
                                        truth_layer: "semantic_rendering".to_owned(),
                                    };
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
                    create_bundle(path, destination, *profile)
                        .map(|manifest| {
                            return DriverData::Bundle {
                                directory: destination.clone(),
                                manifest,
                            };
                        })
                        .map_err(|error| {
                            return driver_failure_from_error(error, Some(path.clone()));
                        })
                }
                DriverMethod::Detach { handle } => {
                    self.launched.remove(handle);
                    if self.sessions.remove(handle).is_none() {
                        Err(driver_failure(
                            "unknown_handle",
                            "session handle is not attached",
                            None,
                        ))
                    } else {
                        Ok(DriverData::Detached)
                    }
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
        let mut builder = terminal.launch(command).geometry(cols, rows);
        if let Some(directory) = working_directory {
            builder = builder.working_directory(directory);
        }
        let session = builder.start().map_err(|error| {
            return driver_failure_from_error(error, Some(evidence_root.to_path_buf()));
        })?;
        let session_id = session.session_id();
        let canonical_session = session.detach();
        let handle = Uuid::now_v7();
        self.sessions.insert(handle, canonical_session.clone());
        self.launched.insert(handle);
        return Ok(DriverData::Attached {
            handle,
            session_id,
            canonical_session,
            completed: false,
        });
    }

    fn cache(&mut self, request_id: Uuid, method_hash: String, response: DriverResponse) {
        self.cached.insert(
            request_id,
            CachedDriverResponse {
                method_hash,
                response,
            },
        );
        self.order.push_back(request_id);
        while self.cached.len() > DRIVER_RESPONSE_CACHE {
            if let Some(evicted) = self.order.pop_front() {
                self.cached.remove(&evicted);
            }
        }
    }
}

fn completed_driver_log(path: &Path) -> StdResult<EventLog, Box<DriverResult>> {
    let log = EventLog::read(path.join("events.jsonl"))
        .map_err(|error| return driver_failure_from_error(error, Some(path.to_path_buf())))?;
    validate_completed_log(path, &log)
        .map_err(|error| return driver_failure_from_error(error, Some(path.to_path_buf())))?;
    return Ok(log);
}

fn resolve_reference(reference: &Path) -> Result<PathBuf> {
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

fn driver_failure_from_error(error: Error, evidence: Option<PathBuf>) -> Box<DriverResult> {
    let code = match &error {
        Error::ControlRejected { code, .. } => code.clone(),
        _ => "operation_failed".to_owned(),
    };
    return driver_failure(&code, error.to_string(), evidence);
}

fn driver_failure(
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
) -> Box<DriverResult> {
    return Box::new(DriverResult::Error {
        code: code.to_owned(),
        message: message.into(),
        retry_safety: "safe_same_request_id".to_owned(),
        uncertain: false,
        evidence,
    });
}

fn driver_error(
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
