//! Versioned newline-framed process driver for language-neutral clients.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    io::{BufRead, BufReader, Error as IoError, Read, Result as IoResult, Write},
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
    capability::{NATIVE_SCREENSHOT_CAPABILITY, require_capability},
    client::Terminal,
    control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlErrorDetails, ControlRequest,
        ControlResult, send_request_response,
    },
    diff::{SemanticDiff, diff_states, inspect_point},
    environment::EnvironmentSpec,
    event::{ByteString, EVENT_LOG_VERSION, EventProjection},
    event_log::EventLog,
    inspection::{WaitOutcome, wait_for_completion, wait_for_output},
    observation::{Observation, StatePoint, observe_session_with_options},
    replay::{STATE_FORMAT_VERSION, StateSelection},
    save::{SaveFormat, save},
    session::{
        CaptureTarget, ResourceLimits, SessionCompletion, VerificationLevel, VerificationReport,
        read_completion, read_manifest, validate_completed_log, verify_session,
    },
    session_registry::SessionRegistry,
    timeline::{TimelineEntry, build_timeline},
    visual::{CaptureKind, CaptureOptions, capture, native_capture_target},
};

/// Current JSONL driver protocol version.
pub const DRIVER_PROTOCOL_VERSION: u32 = 3;
const DRIVER_RESPONSE_CACHE: usize = 4096;
const UNBOUNDED_MESSAGE_BYTES: u64 = 0;

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
        /// Deterministic child-environment policy; values travel only in driver stdin.
        #[serde(default)]
        environment: EnvironmentSpec,
        /// Non-truncating runtime resource limits.
        #[serde(default)]
        resource_limits: ResourceLimits,
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
        /// Actor-provided ordering or attempted-prefix details when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<ControlErrorDetails>,
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
        /// Maximum newline-framed request bytes, or zero when requests have no framing cap.
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
pub fn serve(mut input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut driver = Driver::default();
    while let Some(frame) = read_frame(&mut input)? {
        let response = match frame {
            DriverFrame::Request(request) => driver.handle_request(request),
            DriverFrame::Invalid { line, column } => driver_error(
                Uuid::nil(),
                "invalid_request",
                format!(
                    "driver request does not match the protocol schema at line {line} column {column}"
                ),
                None,
            ),
            DriverFrame::Unterminated => driver_error(
                Uuid::nil(),
                "invalid_request",
                "driver request is not newline terminated",
                None,
            ),
        };
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

enum DriverFrame {
    Request(DriverRequest),
    Invalid { line: usize, column: usize },
    Unterminated,
}

fn read_frame(input: &mut impl BufRead) -> Result<Option<DriverFrame>> {
    loop {
        let available = input.fill_buf().map_err(driver_input_error)?;
        if available.is_empty() {
            return Ok(None);
        }
        if available.first() != Some(&b'\n') {
            break;
        }
        input.consume(1);
    }

    let mut frame = NewlineFrameReader::new(input);
    let parsed = {
        let mut buffered = BufReader::new(&mut frame);
        let mut deserializer = serde_json::Deserializer::from_reader(&mut buffered);
        DriverRequest::deserialize(&mut deserializer)
            .and_then(|request| return deserializer.end().map(|()| return request))
    };
    if let Some(source) = frame.take_input_error() {
        return Err(driver_input_error(source));
    }
    frame.drain().map_err(driver_input_error)?;
    if !frame.terminated() {
        return Ok(Some(DriverFrame::Unterminated));
    }
    return Ok(Some(match parsed {
        Ok(request) => DriverFrame::Request(request),
        Err(error) => DriverFrame::Invalid {
            line: error.line(),
            column: error.column(),
        },
    }));
}

fn driver_input_error(source: IoError) -> Error {
    return Error::Filesystem {
        path: PathBuf::from("<driver-stdin>"),
        source,
    };
}

struct NewlineFrameReader<'input, R> {
    input: &'input mut R,
    terminated: bool,
    input_error: Option<IoError>,
}

impl<'input, R> NewlineFrameReader<'input, R> {
    fn new(input: &'input mut R) -> Self {
        return Self {
            input,
            terminated: false,
            input_error: None,
        };
    }

    fn drain(&mut self) -> IoResult<()>
    where
        R: BufRead,
    {
        let mut buffer = [0_u8; 8192];
        while !self.terminated {
            if self.read(&mut buffer)? == 0 {
                break;
            }
        }
        return Ok(());
    }

    fn take_input_error(&mut self) -> Option<IoError> {
        return self.input_error.take();
    }

    fn terminated(&self) -> bool {
        return self.terminated;
    }
}

impl<R: BufRead> Read for NewlineFrameReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.terminated || buf.is_empty() {
            return Ok(0);
        }
        let available = match self.input.fill_buf() {
            Ok(available) => available,
            Err(source) => {
                self.input_error = Some(IoError::new(source.kind(), source.to_string()));
                return Err(source);
            }
        };
        if available.is_empty() {
            return Ok(0);
        }
        let newline = available.iter().position(|byte| return *byte == b'\n');
        let frame_bytes = newline.map_or(available.len(), |index| return index.saturating_add(1));
        let consumed = frame_bytes.min(buf.len());
        let (Some(destination), Some(source)) =
            (buf.get_mut(..consumed), available.get(..consumed))
        else {
            return Err(IoError::other(
                "driver frame buffer bounds are inconsistent",
            ));
        };
        destination.copy_from_slice(source);
        self.input.consume(consumed);
        self.terminated = newline.is_some_and(|index| return consumed > index);
        return Ok(consumed);
    }
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
    fn handle_request(&mut self, request: DriverRequest) -> DriverResponse {
        let method_hash = match serde_json::to_vec(&request.method) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(_) => {
                return driver_error(
                    request.request_id,
                    "invalid_request",
                    "driver request method could not be encoded",
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
        if driver_response_is_cacheable(&response) {
            self.cache(request.request_id, method_hash, response.clone());
        }
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
                    max_message_bytes: UNBOUNDED_MESSAGE_BYTES,
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
        _ => ("operation_failed".to_owned(), error.to_string()),
    };
    return driver_failure(&code, message, evidence);
}

fn driver_control_failure_from_error(
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

fn actor_failure_retry(code: &str, details: Option<&ControlErrorDetails>) -> (&'static str, bool) {
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

fn control_command_mutates(command: &ControlCommand) -> bool {
    return !matches!(
        command,
        ControlCommand::Ping
            | ControlCommand::Status
            | ControlCommand::Snapshot
            | ControlCommand::WaitScreen { .. }
            | ControlCommand::CaptureStable { .. }
    );
}

fn driver_response_is_cacheable(response: &DriverResponse) -> bool {
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

fn driver_failure(
    code: &str,
    message: impl Into<String>,
    evidence: Option<PathBuf>,
) -> Box<DriverResult> {
    return driver_failure_with_retry(code, message, evidence, "safe_new_request", false, None);
}

fn driver_failure_with_retry(
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, io::Cursor};

    use super::*;

    #[test]
    fn blocked_capability_retains_remediation_in_driver_protocol() {
        let result = driver_failure_from_error(
            Error::CapabilityBlocked {
                id: "native-screenshot".to_owned(),
                code: "native_host_tools_missing".to_owned(),
                detail: "missing host tools: ghostty".to_owned(),
                remediation: "install Ghostty GTK".to_owned(),
            },
            None,
        );
        assert!(matches!(
            result.as_ref(),
            DriverResult::Error {
                code,
                message,
                ..
            } if code == "capability_blocked"
                && message.contains("native_host_tools_missing")
                && message.contains("fix: install Ghostty GTK")
                && message.contains("--require native-screenshot --explain")
        ));
    }

    #[test]
    fn large_valid_frame_is_streamed_and_followed_by_another_request() {
        let large_request_id = Uuid::now_v7();
        let environment = EnvironmentSpec {
            set: BTreeMap::from([(
                "LARGE_VALUE".to_owned(),
                ByteString::new(vec![b'x'; 1024 * 1024]),
            )]),
            ..EnvironmentSpec::default()
        };
        environment.validate().expect("valid environment");
        let large_request = serde_json::to_vec(&DriverRequest {
            version: DRIVER_PROTOCOL_VERSION,
            request_id: large_request_id,
            method: DriverMethod::Launch {
                evidence_root: PathBuf::from("/unused"),
                command: Vec::new(),
                cols: 80,
                rows: 24,
                working_directory: None,
                environment,
                resource_limits: ResourceLimits::default(),
            },
        })
        .expect("large request JSON");
        assert!(large_request.len() > 1024 * 1024);

        let hello_request_id = Uuid::now_v7();
        let hello_request = serde_json::to_vec(&DriverRequest {
            version: DRIVER_PROTOCOL_VERSION,
            request_id: hello_request_id,
            method: DriverMethod::Hello,
        })
        .expect("hello request JSON");
        let mut input = vec![b'\n'];
        input.extend_from_slice(&large_request);
        input.push(b'\n');
        input.extend_from_slice(&hello_request);
        input.push(b'\n');

        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output).expect("serve requests");
        let responses = output
            .split(|byte| return *byte == b'\n')
            .filter(|line| return !line.is_empty())
            .map(|line| return serde_json::from_slice::<DriverResponse>(line).expect("response"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].request_id, large_request_id);
        assert!(matches!(
            &responses[0].result,
            DriverResult::Error { code, .. } if code == "invalid_launch"
        ));
        assert_eq!(responses[1].request_id, hello_request_id);
        assert!(matches!(
            &responses[1].result,
            DriverResult::Ok {
                data: DriverData::Hello {
                    max_message_bytes: UNBOUNDED_MESSAGE_BYTES,
                    ..
                }
            }
        ));
    }

    #[test]
    fn large_malformed_frame_is_drained_before_the_next_request() {
        let request_id = Uuid::now_v7();
        let request = serde_json::to_vec(&DriverRequest {
            version: DRIVER_PROTOCOL_VERSION,
            request_id,
            method: DriverMethod::Hello,
        })
        .expect("request JSON");
        let mut input = vec![b'x'; 1024 * 1024 + 99];
        input.push(b'\n');
        input.extend_from_slice(&request);
        input.push(b'\n');

        let mut output = Vec::new();
        serve(Cursor::new(input), &mut output).expect("serve requests");
        let responses = output
            .split(|byte| return *byte == b'\n')
            .filter(|line| return !line.is_empty())
            .map(|line| return serde_json::from_slice::<DriverResponse>(line).expect("response"))
            .collect::<Vec<_>>();

        assert_eq!(responses.len(), 2);
        assert!(matches!(
            &responses[0].result,
            DriverResult::Error { code, .. } if code == "invalid_request"
        ));
        assert_eq!(responses[1].request_id, request_id);
        assert!(matches!(
            responses[1].result,
            DriverResult::Ok {
                data: DriverData::Hello { .. }
            }
        ));
    }

    #[test]
    fn malformed_frames_are_rejected_without_echoing_payload() {
        let request = serde_json::to_vec(&DriverRequest {
            version: DRIVER_PROTOCOL_VERSION,
            request_id: Uuid::now_v7(),
            method: DriverMethod::Hello,
        })
        .expect("request JSON");
        let mut output = Vec::new();

        serve(Cursor::new(request), &mut output).expect("serve unterminated request");

        let response: DriverResponse = serde_json::from_slice(&output).expect("response JSONL");
        assert_eq!(response.request_id, Uuid::nil());
        assert!(matches!(
            response.result,
            DriverResult::Error { code, message, .. }
                if code == "invalid_request"
                    && message.contains("newline")
                    && !message.contains("hello")
        ));

        let malformed = br#"{"version":"SECRET_SENTINEL","request_id":"00000000-0000-0000-0000-000000000001","method":{"method":"hello"}}
"#;
        let mut output = Vec::new();
        serve(Cursor::new(malformed), &mut output).expect("serve malformed request");

        let response: DriverResponse = serde_json::from_slice(&output).expect("response JSONL");
        assert_eq!(response.request_id, Uuid::nil());
        assert!(matches!(
            response.result,
            DriverResult::Error { code, message, .. }
                if code == "invalid_request"
                    && message.contains("protocol schema")
                    && !message.contains("SECRET_SENTINEL")
        ));
    }

    #[test]
    fn retryable_transport_failures_are_not_cached_by_request_id() {
        let response = DriverResponse {
            version: DRIVER_PROTOCOL_VERSION,
            request_id: Uuid::now_v7(),
            result: *driver_failure_with_retry(
                "control_timeout",
                "timed out",
                None,
                "safe_same_request_id",
                true,
                None,
            ),
        };
        assert!(!driver_response_is_cacheable(&response));

        let actor_response = DriverResponse {
            version: DRIVER_PROTOCOL_VERSION,
            request_id: Uuid::now_v7(),
            result: *driver_failure_with_retry(
                "input_sequence_incomplete",
                "prefix applied",
                None,
                "safe_same_request_id",
                true,
                None,
            ),
        };
        assert!(driver_response_is_cacheable(&actor_response));

        let deterministic = driver_error(
            Uuid::now_v7(),
            "unknown_handle",
            "session handle is not attached",
            None,
        );
        assert!(driver_response_is_cacheable(&deterministic));
    }

    #[test]
    fn retry_classification_distinguishes_observation_from_mutation() {
        assert!(!control_command_mutates(&ControlCommand::Snapshot));
        assert!(control_command_mutates(&ControlCommand::InputSequence {
            atoms: Vec::new(),
            pace_ns: 0,
            include_atom_outcomes: false,
        }));
        assert_eq!(
            actor_failure_retry("input_sequence_incomplete", None),
            ("safe_same_request_id", false)
        );
        assert_eq!(
            actor_failure_retry("input_write_failed", None),
            ("safe_same_request_id", true)
        );
    }
}
