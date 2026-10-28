use super::{
    capture::{preserve_failed, DisplayScaling},
    capture_io::{
        check_cancelled, command_output, command_output_in_profile, external_error, read_json,
        secure_permissions, unix_now_ns,
    },
    media::{capture_stable_x11_frame_with_check, probe_screenshot},
    process::{
        ghostty_overrides, owned_command, spawn_xvfb, wait_for_x_socket, DisplayLease, ManagedChild,
    },
    profile::materialize,
    validate_visual_manifest, Error, Result,
};
use crate::helper_protocol::{flag, playback_worker};
use crate::playback::worker::{
    abstract_socket_label, is_same_user_peer, WorkerRequest, WorkerResponse,
};
use serde::Serialize;
use std::{
    env::temp_dir,
    fs,
    io::{self, Read as _, Write as _},
    net::Shutdown,
    os::{
        linux::net::SocketAddrExt as _,
        unix::net::{SocketAddr, UnixStream},
    },
    path::{Path, PathBuf},
    process::Stdio,
    result::Result as StdResult,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use termtrace_engine::observation::{replay_endpoint_for_prefix, replay_endpoint_for_session};
use termtrace_engine::recording::verify_profile_fonts;
use termtrace_evidence::{
    artifact::{
        append_artifact_prefix_root, collect_artifact_members, commit_directory,
        create_private_directory, secure_file, sync_directory, verify_artifact_members,
        write_secure,
    },
    event_log::{CanonicalBoundary, EventLogCursor},
    session::read_manifest,
};
use termtrace_model::execution::layout::{self, SessionLayout};
use termtrace_model::{
    artifact::playback::PlaybackFailure,
    artifact::visual::{
        CaptureOutcome, VisualManifest, GHOSTTY_EFFECTIVE_CONFIG_NAME, VISUAL_MANIFEST_VERSION,
    },
    execution::session::CaptureTarget,
    terminal::{
        observation::StatePoint,
        replay::{ReplayEndpoint, StateSelection},
    },
    value::geometry::TerminalGeometryReport,
};
use termtrace_process::poll_until;

const WORKER: &str = "native replay worker";
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(20);

struct WorkDirectory(PathBuf);

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        // Drop has no error channel; the private work directory is never published.
        drop(fs::remove_dir_all(&self.0));
    }
}

/// An owned private Ghostty display for successive forward replay screenshots.
///
/// Every selection is authenticated against canonical evidence. Requests have bounded
/// response framing and deadlines. A failed advance closes the worker because its
/// terminal may have consumed only part of the requested bytes. Dropping the worker
/// closes the protocol and terminates its owned terminal and display processes.
pub struct NativeReplayWorker {
    stream: UnixStream,
    ghostty: ManagedChild,
    xvfb: ManagedChild,
    lease: DisplayLease,
    work: WorkDirectory,
    session: PathBuf,
    target: CaptureTarget,
    canvas: CaptureTarget,
    effective: Vec<u8>,
    version: String,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
    closed: bool,
}

impl NativeReplayWorker {
    /// Start one terminal using the source session's recorded visual configuration.
    ///
    /// # Errors
    /// Returns source, profile, geometry, process-startup, or cancellation failures.
    pub fn start(
        helper: &Path,
        session: &Path,
        target: CaptureTarget,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self> {
        if timeout.is_zero()
            || timeout > Duration::from_secs(3600)
            || target.width < super::MIN_VIEWPORT_WIDTH
            || target.height < super::MIN_VIEWPORT_HEIGHT
        {
            return Err(failure("invalid replay worker timeout or viewport"));
        }

        check_cancelled(&cancelled, WORKER)?;

        let session =
            fs::canonicalize(session).map_err(|source| return filesystem(session, source))?;
        let manifest = read_manifest(&session)?;
        let work =
            WorkDirectory(temp_dir().join(format!("termtrace-worker-{}", uuid::Uuid::now_v7())));
        create_private_directory(&work.0)?;

        let replay = if let Some(provenance) = &manifest.ghostty_visual {
            if provenance.replay_profile != layout::GHOSTTY_REPLAY_PROFILE {
                return Err(failure("unsupported recorded replay profile path"));
            }

            let path = SessionLayout::new(&session).ghostty_replay_profile();
            let bytes = fs::read(&path).map_err(|source| return filesystem(&path, source))?;
            if blake3::hash(&bytes).to_hex().as_str() != provenance.replay_profile_hash {
                return Err(failure("recorded replay profile hash differs"));
            }

            Some(bytes)
        } else {
            None
        };

        let profile = replay
            .as_deref()
            .map(|bytes| return materialize(&work.0.join("profile"), bytes))
            .transpose()?;

        let effective =
            command_output_in_profile("ghostty", &["+show-config"], profile.as_deref())?;

        let canonical = EventLogCursor::open(SessionLayout::new(&session).event_log())?;
        let expected = manifest
            .ghostty_visual
            .as_ref()
            .map(|value| return value.effective_config_hash.as_str())
            .or_else(|| {
                return canonical
                    .header()
                    .colors
                    .source
                    .strip_prefix("ghostty_show_config_blake3:");
            });

        if expected.is_some_and(|hash| return hash != blake3::hash(&effective).to_hex().as_str()) {
            return Err(failure("recorded effective Ghostty configuration changed"));
        }

        if let Some(replay) = &replay
            && manifest.native_profile_dpi().is_some()
        {
            verify_profile_fonts(replay, &effective)?;
        }

        let version =
            String::from_utf8_lossy(&command_output("ghostty", &["+version"])?).into_owned();

        let canvas = super::geometry::session_canvas(&session, target)?;
        let profile_dpi = manifest.native_profile_dpi();
        let lease = DisplayLease::acquire()?;
        let display = format!(":{}", lease.number);
        let mut xvfb = spawn_xvfb(
            Some(helper),
            &lease,
            canvas.width,
            canvas.height,
            &work.0,
            profile_dpi,
        )?;
        wait_for_x_socket(&lease, &mut xvfb, timeout.min(Duration::from_secs(5)))?;

        if let Some(dpi) = profile_dpi {
            super::window::publish_font_dpi(&display, dpi)?;
        }

        let socket_name = format!("termtrace-replay-{}", uuid::Uuid::now_v7());
        let socket = abstract_socket_label(&socket_name);
        let geometry = work.0.join("geometry.json");
        let error_file = work.0.join("playback-error.json");
        let mut command = owned_command(helper, "dbus-run-session")?;
        command
            .arg("--")
            .arg("ghostty")
            .args(ghostty_overrides(target.padding))
            .arg("-e")
            .arg(helper)
            .arg(playback_worker::SUBCOMMAND)
            .arg(flag(playback_worker::SESSION))
            .arg(&session)
            .arg(flag(playback_worker::SOCKET))
            .arg(&socket_name)
            .arg(flag(playback_worker::GEOMETRY_FILE))
            .arg(&geometry)
            .arg(flag(playback_worker::ERROR_FILE))
            .arg(&error_file)
            .env("DISPLAY", display)
            .env("GDK_BACKEND", "x11")
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(secure_file(
                &work.0.join("ghostty.stdout.log"),
            )?))
            .stderr(Stdio::from(secure_file(
                &work.0.join("ghostty.stderr.log"),
            )?));

        if let Some(profile) = profile {
            command.env("XDG_CONFIG_HOME", profile);
        }

        if let Some(dpi) = profile_dpi {
            let scaling = DisplayScaling::profile(dpi);
            command
                .env("GDK_SCALE", scaling.gdk_scale.to_string())
                .env("GDK_DPI_SCALE", format!("{}", scaling.gdk_dpi_scale));
        }

        let mut ghostty = ManagedChild::spawn_logged("ghostty replay worker", &mut command)?;
        let report: TerminalGeometryReport =
            poll_until(Instant::now() + timeout, STARTUP_POLL_INTERVAL, || {
                check_cancelled(&cancelled, WORKER)?;

                ghostty.check_running()?;
                xvfb.check_running()?;
                if let Ok(failed) = read_json::<PlaybackFailure>(&error_file) {
                    return Err(failure(&format!(
                        "worker startup failed: {}",
                        failed.message
                    )));
                }
                return Ok::<_, Error>(read_json(&geometry).ok());
            })?
            .ok_or_else(|| return failure("worker startup timed out"))?;

        super::geometry::validate_capture(
            manifest.geometry,
            manifest.geometry_provenance.source,
            report,
            target.width,
            target.height,
        )?;

        let address = SocketAddr::from_abstract_name(socket_name.as_bytes())
            .map_err(|source| return filesystem(&socket, source))?;
        let stream = UnixStream::connect_addr(&address)
            .map_err(|source| return filesystem(&socket, source))?;

        if !is_same_user_peer(&stream).map_err(|source| return filesystem(&socket, source))? {
            return Err(failure("replay worker socket is owned by another user"));
        }

        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|source| return filesystem(&socket, source))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|source| return filesystem(&socket, source))?;

        return Ok(Self {
            stream,
            ghostty,
            xvfb,
            lease,
            work,
            session,
            target,
            canvas,
            effective,
            version,
            cancelled,
            timeout,
            closed: false,
        });
    }

    /// Capture a completed source selection at or after the previous endpoint.
    ///
    /// # Errors
    /// Returns selection, replay, acquisition, cancellation, or publication failures.
    pub fn capture(&mut self, point: &StatePoint, destination: &Path) -> Result<CaptureOutcome> {
        let endpoint = replay_endpoint_for_session(&self.session, point, &self.cancelled)?;

        return self.capture_endpoint(&endpoint, destination);
    }

    /// Capture the latest presented state within an explicitly committed live prefix.
    ///
    /// # Errors
    /// Returns prefix, replay, acquisition, cancellation, or publication failures.
    pub fn capture_latest_prefix(
        &mut self,
        boundary: &CanonicalBoundary,
        destination: &Path,
    ) -> Result<CaptureOutcome> {
        let endpoint = replay_endpoint_for_prefix(&self.session, boundary, &self.cancelled)?;

        return self.capture_endpoint(&endpoint, destination);
    }

    /// Capture up to 256 completed selections in one terminal, preserving request order.
    ///
    /// Replay visits selections in canonical order. The entire batch is published
    /// atomically and its manifest maps the original requests to their images.
    ///
    /// # Errors
    /// Returns selection, replay, capture, cancellation, or publication failures.
    pub fn capture_batch(
        &mut self,
        points: &[StatePoint],
        destination: &Path,
    ) -> Result<Vec<CaptureOutcome>> {
        if points.is_empty() || points.len() > 256 {
            return Err(failure("native batches require 1 to 256 selections"));
        }

        if destination.exists() {
            return Err(Error::ArtifactExists {
                path: destination.to_path_buf(),
            });
        }

        let mut endpoints = points
            .iter()
            .enumerate()
            .map(|(index, point)| {
                return replay_endpoint_for_session(&self.session, point, &self.cancelled)
                    .map(|endpoint| return (index, endpoint));
            })
            .collect::<StdResult<Vec<_>, _>>()?;
        endpoints
            .sort_by_key(|(_, endpoint)| return (endpoint.event_seq, endpoint.event_byte_offset));
        let source = endpoints
            .first()
            .map(|(_, endpoint)| return endpoint.clone())
            .ok_or_else(|| return failure("batch source is missing"))?;

        if endpoints.iter().any(|(_, endpoint)| {
            return endpoint.boundary != source.boundary
                || endpoint.session_manifest_hash != source.session_manifest_hash;
        }) {
            return Err(failure("batch source changed during selection"));
        }

        let parent = destination
            .parent()
            .filter(|path| return path.is_dir())
            .ok_or_else(|| return failure("batch destination parent does not exist"))?;

        let staging = parent.join(format!(".visual-batch.{}.tmp", uuid::Uuid::now_v7()));
        create_private_directory(&staging)?;

        let result = (|| {
            let mut entries = Vec::new();
            for (index, endpoint) in endpoints {
                let name = format!("capture-{index:04}");
                let artifact = staging.join(&name);
                create_private_directory(&artifact)?;
                self.capture_staged(&endpoint, &artifact)?;

                entries.push(serde_json::json!({"request_index": index, "directory": name, "endpoint": endpoint}));
            }

            entries.sort_by_key(|entry| {
                return entry
                    .get("request_index")
                    .and_then(serde_json::Value::as_u64);
            });
            write_json(
                &staging.join("batch-manifest.json"),
                &serde_json::json!({
                    "version": 1, "kind": "native_replay_screenshot_batch", "entries": entries,
                    "members": collect_artifact_members(&staging)?,
                }),
            )?;

            write_json(
                &staging.join("manifest.json"),
                &serde_json::json!({
                    "version": 1, "kind": "native_replay_screenshot_batch", "session_id": source.session_id,
                    "source_event_hash": source.boundary.event_hash, "source_range": null,
                    "members": collect_artifact_members(&staging)?,
                }),
            )?;

            sync_directory(&staging)?;

            return Ok(());
        })();
        if let Err(error) = result {
            self.close();

            return Err(preserve_failed(parent, &staging, error));
        }

        if let Err(error) = commit_directory(parent, &staging, destination, false) {
            return Err(preserve_failed(parent, &staging, error.into()));
        }

        sync_directory(parent)?;

        self.catalog(destination, "native_screenshot_batch", &source)?;

        return Ok((0..points.len())
            .map(|index| {
                let directory = destination.join(format!("capture-{index:04}"));

                return CaptureOutcome {
                    screenshot: directory.join("final.png"),
                    directory,
                    video: None,
                };
            })
            .collect());
    }

    /// End playback and terminate all owned processes. Safe to call repeatedly.
    pub fn close(&mut self) {
        if !self.closed {
            // The worker may already have closed its end; closing is idempotent teardown.
            drop(self.stream.shutdown(Shutdown::Both));

            self.ghostty.terminate();
            self.xvfb.terminate();
            self.closed = true;
        }
    }

    fn advance(
        &mut self,
        endpoint: &ReplayEndpoint,
    ) -> Result<(TerminalGeometryReport, (u16, u16))> {
        if self.closed {
            return Err(failure("worker is closed"));
        }

        check_cancelled(&self.cancelled, WORKER)?;

        let mut request = serde_json::to_vec(&WorkerRequest::Advance {
            endpoint: endpoint.clone(),
        })
        .map_err(|error| return failure(&error.to_string()))?;
        request.push(b'\n');
        self.stream
            .write_all(&request)
            .map_err(|error| return failure(&error.to_string()))?;

        let mut response = Vec::new();

        // Each attempt waits in the socket read timeout, so attempts need no extra sleep.
        poll_until(Instant::now() + self.timeout, Duration::ZERO, || {
            check_cancelled(&self.cancelled, WORKER)?;

            self.ghostty.check_running()?;
            self.xvfb.check_running()?;

            let mut buffer = [0_u8; 4096];
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err(failure("worker closed without acknowledging endpoint")),
                Ok(count) => response.extend_from_slice(buffer.get(..count).ok_or_else(|| {
                    return failure("worker socket returned an invalid read length");
                })?),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(failure(&error.to_string())),
            }

            if response.len() > 65536 {
                return Err(failure("worker response exceeds framing limit"));
            }
            return Ok(response.contains(&b'\n').then_some(()));
        })?
        .ok_or_else(|| return failure("worker request timed out"))?;

        match serde_json::from_slice(&response)
            .map_err(|error| return failure(&error.to_string()))?
        {
            WorkerResponse::Ready {
                endpoint: actual,
                geometry,
                window_width,
                window_height,
            } if actual == *endpoint => return Ok((geometry, (window_width, window_height))),
            WorkerResponse::Error { message } => return Err(failure(&message)),
            _ => return Err(failure("worker acknowledgement differs from endpoint")),
        }
    }

    fn capture_endpoint(
        &mut self,
        endpoint: &ReplayEndpoint,
        destination: &Path,
    ) -> Result<CaptureOutcome> {
        if destination.exists() {
            return Err(Error::ArtifactExists {
                path: destination.to_path_buf(),
            });
        }

        let parent = destination
            .parent()
            .filter(|path| return path.is_dir())
            .ok_or_else(|| return failure("capture destination parent does not exist"))?;

        let staging = parent.join(format!(".visual.{}.tmp", uuid::Uuid::now_v7()));
        create_private_directory(&staging)?;

        if let Err(error) = self.capture_staged(endpoint, &staging) {
            self.close();

            // The logs supplement the capture failure reported below; a missing log is not an error.
            for name in ["ghostty.stdout.log", "ghostty.stderr.log"] {
                drop(fs::copy(self.work.0.join(name), staging.join(name)));
            }

            return Err(preserve_failed(parent, &staging, error));
        }

        if let Err(error) = commit_directory(parent, &staging, destination, false) {
            return Err(preserve_failed(parent, &staging, error.into()));
        }

        sync_directory(parent)?;

        self.catalog(destination, "native_screenshot", endpoint)?;

        return Ok(CaptureOutcome {
            directory: destination.to_path_buf(),
            video: None,
            screenshot: destination.join("final.png"),
        });
    }

    fn catalog(&self, destination: &Path, kind: &str, endpoint: &ReplayEndpoint) -> Result<()> {
        let canonical_destination = fs::canonicalize(destination)
            .map_err(|source| return filesystem(destination, source))?;

        if canonical_destination.starts_with(&self.session) {
            let path = destination.join("manifest.json");
            append_artifact_prefix_root(
                &self.session,
                &canonical_destination,
                kind,
                &fs::read(&path).map_err(|source| return filesystem(&path, source))?,
                &endpoint.session_manifest_hash,
                &endpoint.boundary,
            )?;
        }

        return Ok(());
    }

    fn capture_staged(&mut self, endpoint: &ReplayEndpoint, staging: &Path) -> Result<()> {
        let (geometry_report, (window_width, window_height)) = self.advance(endpoint)?;
        let width = window_width.min(self.canvas.width);
        let height = window_height.min(self.canvas.height);
        let crop =
            ((width, height) != (self.canvas.width, self.canvas.height)).then_some((width, height));

        let capture_started_unix_ns = unix_now_ns()?;
        let screenshot = staging.join("final.png");
        let samples = capture_stable_x11_frame_with_check(
            staging,
            &format!(":{}", self.lease.number),
            &screenshot,
            crop,
            self.timeout,
            || return check_cancelled(&self.cancelled, WORKER),
        )?;

        check_cancelled(&self.cancelled, WORKER)?;

        secure_permissions(&screenshot)?;

        write_secure(
            &staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME),
            &self.effective,
        )?;

        write_json(&staging.join("replay-endpoint.json"), endpoint)?;
        super::resources::snapshot_mapped(
            &self.ghostty,
            staging,
            &self.cancelled,
            "after_replay_endpoint",
        )?;

        let visual = VisualManifest {
            version: VISUAL_MANIFEST_VERSION,
            session_id: endpoint.session_id,
            source_event_hash: Some(endpoint.boundary.event_hash.clone()),
            source_range: None,
            state_selection: StateSelection::Presented,
            geometry_report,
            fps: 24,
            width,
            height,
            capture_started_unix_ns,
            replay_origin_unix_ns: 0,
            effective_config_hash: blake3::hash(&self.effective).to_hex().to_string(),
            ghostty_version: self.version.clone(),
            ffmpeg_version: None,
            ghostty_overrides: ghostty_overrides(self.target.padding),
            video_probe: None,
            screenshot_probe: probe_screenshot(&screenshot, width, height)?,
            members: collect_artifact_members(staging)?,
            capture_kind: "screenshot".to_owned(),
            selected_state_id: endpoint.state_id,
            selected_state_hash: Some(endpoint.state_hash.clone()),
            endpoint_event_seq: endpoint.event_seq,
            endpoint_event_byte_offset: endpoint.event_byte_offset,
            presentation_evidence: "post_endpoint_stable_capture".to_owned(),
            frame_samples: samples,
            guarantee_level: "native_capture_environment".to_owned(),
        };

        validate_visual_manifest(&visual)?;
        write_json(&staging.join("manifest.json"), &visual)?;
        verify_artifact_members(staging, &visual.members)?;

        return sync_directory(staging).map_err(Error::from);
    }
}

impl Drop for NativeReplayWorker {
    fn drop(&mut self) {
        self.close();
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    return write_secure(path, &bytes).map_err(Error::from);
}

fn failure(reason: &str) -> Error {
    return external_error(WORKER, reason);
}

fn filesystem(path: &Path, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.to_path_buf(),
        source,
    };
}
