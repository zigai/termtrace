use super::runtime::Driver;
use std::{
    env,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use termtrace_engine::observation::{replay_endpoint_for_prefix, replay_endpoint_for_session};
use termtrace_engine::Error as EngineError;
use termtrace_media::native_capture::{native_capture_target, CaptureOutcome, NativeReplayWorker};
use termtrace_model::{
    execution::session::CaptureTarget,
    protocol::control::{ControlCommand, ControlData},
    terminal::{observation::StatePoint, replay::ReplayEndpoint},
};
use termtrace_sdk::{request, Error, Result};
use uuid::Uuid;

/// Keep at most one private terminal per language-driver process.
pub(super) struct DriverNativeWorker {
    pub(super) handle: Uuid,
    viewport: Option<CaptureTarget>,
    last: ReplayEndpoint,
    worker: NativeReplayWorker,
}

impl Driver {
    pub(super) fn native_screenshot(
        &mut self,
        handle: Uuid,
        session: &Path,
        point: &StatePoint,
        destination: &Path,
        viewport: Option<CaptureTarget>,
    ) -> Result<CaptureOutcome> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let boundary = if session.join("completion.json").is_file() {
            None
        } else {
            if *point != StatePoint::LatestPresented {
                return Err(EngineError::InvalidReplay {
                    reason: "live native capture requires the latest presented state".to_owned(),
                }
                .into());
            }

            match request(session, ControlCommand::Status) {
                Ok(ControlData::Status { boundaries, .. }) => Some(boundaries.durable),
                Err(_) if session.join("completion.json").is_file() => None,
                Err(error) => return Err(error),
                _ => {
                    return Err(EngineError::InvalidReplay {
                        reason: "live status returned an unexpected response".to_owned(),
                    }
                    .into());
                }
            }
        };

        let endpoint = if let Some(boundary) = &boundary {
            replay_endpoint_for_prefix(session, boundary, &cancelled)?
        } else {
            replay_endpoint_for_session(session, point, &cancelled)?
        };

        if self.native.as_ref().is_some_and(|active| {
            return active.handle != handle
                || active.viewport != viewport
                || (endpoint.event_seq, endpoint.event_byte_offset)
                    < (active.last.event_seq, active.last.event_byte_offset);
        }) {
            self.native = None;
        }

        if self.native.is_none() {
            let helper = env::current_exe().map_err(|source| {
                return Error::Filesystem {
                    path: "<current-executable>".into(),
                    source,
                };
            })?;
            let (target, _) = native_capture_target(&helper, session, viewport)?;

            let worker = NativeReplayWorker::start(
                &helper,
                session,
                target,
                cancelled,
                Duration::from_secs(30),
            )?;

            self.native = Some(DriverNativeWorker {
                handle,
                viewport,
                last: endpoint.clone(),
                worker,
            });
        }

        let active = self.native.as_mut().ok_or_else(|| {
            return EngineError::InvalidReplay {
                reason: "native replay worker unavailable".to_owned(),
            };
        })?;

        let captured = if let Some(boundary) = boundary {
            active.worker.capture_latest_prefix(&boundary, destination)
        } else {
            active.worker.capture(point, destination)
        };
        match captured {
            Ok(capture) => {
                active.last = endpoint;
                return Ok(capture);
            }
            Err(error) => {
                self.native = None;
                return Err(error.into());
            }
        }
    }

    pub(super) fn close_native(&mut self, handle: &Uuid) {
        if self
            .native
            .as_ref()
            .is_some_and(|worker| return worker.handle == *handle)
        {
            self.native = None;
        }
    }
}
