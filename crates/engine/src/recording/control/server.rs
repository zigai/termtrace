//! Authenticated control-socket transport and worker lifecycle.

use super::super::{
    model::RuntimeMessage,
    support::{finish_with_cleanup, poll_ready, terminal_error},
};
use super::transport::{control_response_timeout, control_socket_address, validate_frame};
use crate::{Error, Result};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::{pipe, write as fd_write, Uid},
};
use std::{
    io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _},
    os::fd::{AsFd as _, OwnedFd},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use termtrace_model::{
    protocol::control::{
        ActionPostconditionResult, ControlData, ControlErrorDetails, ControlRequest,
        ControlResponse, ControlResult, ScreenFrameFailureDetails, CONTROL_PROTOCOL_VERSION,
        MAX_CONTROL_MESSAGE_BYTES,
    },
    terminal::observation::ActorBoundaries,
    terminal::screen::{FrameDetail, ScreenFrame, ScreenFrameData, ScreenFrameSource},
};

pub(in crate::recording) struct ControlWorker {
    stop_accepting: OwnedFd,
    cancel: OwnedFd,
    thread: thread::JoinHandle<()>,
}

const MAX_CONTROL_CONNECTIONS: usize = 256;

struct ConnectionPermit<'counter> {
    active: &'counter AtomicUsize,
}

impl<'counter> ConnectionPermit<'counter> {
    fn acquire(active: &'counter AtomicUsize, capacity: usize) -> Option<Self> {
        return active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                return (count < capacity).then_some(count + 1);
            })
            .ok()
            .map(|_| return Self { active });
    }
}

impl Drop for ConnectionPermit<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(in crate::recording) fn spawn_control_server(
    socket_name: &str,
    sender: mpsc::SyncSender<RuntimeMessage>,
) -> Result<ControlWorker> {
    let address = control_socket_address(socket_name)?;
    let listener = UnixListener::bind_addr(&address).map_err(|error| {
        return terminal_error("bind abstract control socket", error.to_string());
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        return terminal_error("configure abstract control socket", error.to_string());
    })?;

    let (stop_accepting_read, stop_accepting) = pipe().map_err(|error| {
        return terminal_error(
            "create control listener cancellation pipe",
            error.to_string(),
        );
    })?;

    let (cancel_read, cancel) = pipe().map_err(|error| {
        return terminal_error(
            "create control request cancellation pipe",
            error.to_string(),
        );
    })?;

    let thread = thread::Builder::new()
        .name("termtrace-control-server".to_owned())
        .spawn(move || {
            if let Err(error) =
                control_server_loop(&listener, &stop_accepting_read, &cancel_read, &sender)
            {
                let _ = sender.try_send(RuntimeMessage::ControlServerFailed(error.to_string()));
            }
        })
        .map_err(|error| return terminal_error("spawn control server", error.to_string()))?;

    tracing::debug!(socket = socket_name, "abstract control socket ready");

    return Ok(ControlWorker {
        stop_accepting,
        cancel,
        thread,
    });
}

fn control_server_loop(
    listener: &UnixListener,
    stop_accepting: &OwnedFd,
    cancel: &OwnedFd,
    sender: &mpsc::SyncSender<RuntimeMessage>,
) -> Result<()> {
    let active = AtomicUsize::new(0);

    return thread::scope(|scope| loop {
        let (listener_ready, stop_accepting_ready) = {
            let mut descriptors = [
                PollFd::new(listener.as_fd(), PollFlags::POLLIN),
                PollFd::new(stop_accepting.as_fd(), PollFlags::POLLIN),
            ];

            match poll(&mut descriptors, PollTimeout::NONE) {
                Ok(_) => (poll_ready(&descriptors[0]), poll_ready(&descriptors[1])),
                Err(Errno::EINTR) => continue,
                Err(error) => {
                    return Err(terminal_error("poll control socket", error.to_string()));
                }
            }
        };

        if !listener_ready && stop_accepting_ready {
            return Ok(());
        }

        if !listener_ready {
            continue;
        }

        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let Some(permit) = ConnectionPermit::acquire(&active, MAX_CONTROL_CONNECTIONS)
                    else {
                        continue;
                    };

                    scope.spawn(move || {
                        let _permit = permit;

                        if let Err(error) = handle_control_connection(stream, sender, cancel) {
                            tracing::warn!(%error, "control connection failed");
                        }
                    });
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    return Err(terminal_error(
                        "accept control connection",
                        error.to_string(),
                    ));
                }
            }
        }

        if stop_accepting_ready {
            return Ok(());
        }
    });
}

fn handle_control_connection(
    mut stream: UnixStream,
    sender: &mpsc::SyncSender<RuntimeMessage>,
    cancel: &OwnedFd,
) -> Result<()> {
    let credentials = getsockopt(&stream, PeerCredentials)
        .map_err(|error| return terminal_error("authenticate control peer", error.to_string()))?;

    if credentials.uid() != Uid::effective().as_raw() {
        return Err(Error::ControlRejected {
            code: "unauthorized_peer".to_owned(),
            message: "control peer uid does not own this session".to_owned(),
        });
    }

    let timeout = Some(Duration::from_secs(10));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| return stream.set_write_timeout(timeout))
        .map_err(|error| {
            return terminal_error("configure control connection", error.to_string());
        })?;

    let mut encoded = Vec::new();
    BufReader::new(stream.try_clone().map_err(|error| {
        return terminal_error("clone control connection", error.to_string());
    })?)
    .take(MAX_CONTROL_MESSAGE_BYTES + 1)
    .read_until(b'\n', &mut encoded)
    .map_err(|error| return terminal_error("read control request", error.to_string()))?;

    // Reachability checks connect and close without submitting a request.
    if encoded.is_empty() {
        return Ok(());
    }

    validate_frame(&encoded)?;
    let request: ControlRequest = serde_json::from_slice(&encoded).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<control-socket>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if request.version != CONTROL_PROTOCOL_VERSION {
        return write_control_response(
            &mut stream,
            &ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: ControlResult::Error {
                    code: "unsupported_version".to_owned(),
                    message: format!("protocol version {} is unsupported", request.version),
                    details: None,
                },
            },
        );
    }

    let response_timeout = control_response_timeout(&request.command, Duration::from_secs(1));
    stream
        .set_write_timeout(Some(response_timeout))
        .map_err(|error| return terminal_error("configure wait response", error.to_string()))?;

    let (response_sender, response_receiver) = mpsc::sync_channel(1);
    let mut message = RuntimeMessage::Control {
        request,
        response: response_sender,
    };
    let deadline = Instant::now() + response_timeout;

    loop {
        match sender.try_send(message) {
            Ok(()) => break,
            Err(mpsc::TrySendError::Full(returned)) => {
                if cancellation_requested(cancel)? {
                    return Ok(());
                }

                if Instant::now() >= deadline {
                    return Err(terminal_error(
                        "dispatch control request",
                        "recording actor is unresponsive",
                    ));
                }

                message = returned;

                thread::sleep(Duration::from_millis(10));
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(terminal_error(
                    "dispatch control request",
                    "recording actor disconnected",
                ));
            }
        }
    }

    let response = loop {
        match response_receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(response) => break response,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancellation_requested(cancel)? {
                    return Ok(());
                }

                if Instant::now() >= deadline {
                    return Err(terminal_error(
                        "wait for control response",
                        "recording actor is unresponsive",
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(terminal_error(
                    "wait for control response",
                    "recording actor disconnected",
                ));
            }
        }
    };

    return write_control_response(&mut stream, &response);
}

fn cancellation_requested(cancel: &OwnedFd) -> Result<bool> {
    let mut descriptors = [PollFd::new(cancel.as_fd(), PollFlags::POLLIN)];
    match poll(&mut descriptors, PollTimeout::ZERO) {
        Ok(_) => return Ok(poll_ready(&descriptors[0])),
        Err(Errno::EINTR) => return Ok(false),
        Err(error) => {
            return Err(terminal_error(
                "poll control cancellation",
                error.to_string(),
            ));
        }
    }
}

fn write_control_response(stream: &mut UnixStream, response: &ControlResponse) -> Result<()> {
    let encoded = serde_json::to_vec(response).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<control-socket>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let response_bytes = u64::try_from(encoded.len().saturating_add(1)).unwrap_or(u64::MAX);
    let encoded = if response_bytes > MAX_CONTROL_MESSAGE_BYTES {
        let (code, message, details) = oversized_response_error(response, response_bytes);

        serde_json::to_vec(&ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: response.request_id,
            result: ControlResult::Error {
                code,
                message,
                details,
            },
        })
        .map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<control-socket>"),
                json_path: "$".to_owned(),
                source,
            };
        })?
    } else {
        encoded
    };

    return stream
        .write_all(&encoded)
        .and_then(|()| return stream.write_all(b"\n"))
        .and_then(|()| return stream.flush())
        .map_err(|error| return terminal_error("write control response", error.to_string()));
}

fn oversized_response_error(
    response: &ControlResponse,
    response_bytes: u64,
) -> (String, String, Option<ControlErrorDetails>) {
    let ControlResult::Ok { data } = &response.result else {
        return (
            "response_too_large".to_owned(),
            format!("complete control response exceeds the {MAX_CONTROL_MESSAGE_BYTES}-byte limit"),
            None,
        );
    };
    let Some((frame, boundaries, scope)) = rich_response_context(data) else {
        return (
            "response_too_large".to_owned(),
            format!("complete control response exceeds the {MAX_CONTROL_MESSAGE_BYTES}-byte limit"),
            None,
        );
    };
    let detail = match &frame.data {
        ScreenFrameData::Text { .. } => FrameDetail::Text,
        ScreenFrameData::Spans { .. } => FrameDetail::Spans,
        ScreenFrameData::Cells { .. } => FrameDetail::Cells,
    };

    return (
        "screen_frame_resource_limit".to_owned(),
        format!("complete control response exceeds the {MAX_CONTROL_MESSAGE_BYTES}-byte limit"),
        Some(ControlErrorDetails {
            current_presented_state_hash: frame.state_hash.clone(),
            boundaries,
            scope: scope.to_owned(),
            input_sequence: None,
            screen_frame: Some(ScreenFrameFailureDetails {
                detail: Some(detail),
                region: Some(frame.requested_region),
                region_reason: None,
                cols: Some(frame.geometry.cols),
                rows: Some(frame.geometry.rows),
                limit_kind: Some("response_bytes".to_owned()),
                requested: Some(response_bytes),
                maximum: Some(MAX_CONTROL_MESSAGE_BYTES),
            }),
        }),
    );
}

fn rich_response_context(
    data: &ControlData,
) -> Option<(&ScreenFrame, ActorBoundaries, &'static str)> {
    let (frame, boundaries, scope) = match data {
        ControlData::ScreenFrame { frame } => (
            frame,
            actor_frame_boundaries(frame)?,
            "screen_frame_response",
        ),
        ControlData::StableSnapshot { frame, .. } => (
            frame,
            actor_frame_boundaries(frame)?,
            "stable_snapshot_response",
        ),
        ControlData::ScreenWait { frame, .. } => (
            frame,
            actor_frame_boundaries(frame)?,
            "screen_wait_response",
        ),
        ControlData::ActionApplied {
            postcondition,
            boundaries,
            ..
        } => {
            let frame = match postcondition {
                ActionPostconditionResult::None { frame, .. }
                | ActionPostconditionResult::Idle { frame, .. }
                | ActionPostconditionResult::Screen { frame, .. } => frame,
            };

            (frame, boundaries.clone(), "action_response")
        }
        _ => return None,
    };
    return Some((frame, boundaries, scope));
}

fn actor_frame_boundaries(frame: &ScreenFrame) -> Option<ActorBoundaries> {
    let ScreenFrameSource::Actor { boundaries, .. } = &frame.source else {
        return None;
    };
    return Some(boundaries.clone());
}

impl ControlWorker {
    pub(in crate::recording) fn begin_shutdown(&self) -> Result<()> {
        return match fd_write(&self.stop_accepting, b"x") {
            Ok(_) | Err(Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error(
                "stop accepting control connections",
                error.to_string(),
            )),
        };
    }

    pub(in crate::recording) fn shutdown_finished(&self) -> bool {
        return self.thread.is_finished();
    }

    pub(in crate::recording) fn stop(self) -> Result<()> {
        let stop_accepting_result = self.begin_shutdown();
        let cancel_result = match fd_write(&self.cancel, b"x") {
            Ok(_) | Err(Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error("cancel control server", error.to_string())),
        };

        let join_result = self
            .thread
            .join()
            .map_err(|_| return terminal_error("join control server", "control thread panicked"));

        let result = finish_with_cleanup(stop_accepting_result, cancel_result);
        return finish_with_cleanup(result, join_result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use termtrace_evidence::event_log::CanonicalBoundary;
    use termtrace_model::{
        terminal::observation::ActorBoundaries, terminal::observation::CaptureQuality,
        terminal::screen::ScreenProjectionRequest,
    };
    use termtrace_terminal::{project_screen_frame, GhosttyVt};
    use uuid::Uuid;

    fn boundaries() -> ActorBoundaries {
        let boundary = CanonicalBoundary {
            next_event_seq: 7,
            event_hash: "a".repeat(64),
            output_bytes: 11,
        };
        return ActorBoundaries {
            durable: boundary.clone(),
            observed: boundary,
        };
    }

    fn actor_frame() -> ScreenFrame {
        let mut terminal = GhosttyVt::new(8, 2, 0).expect("terminal");
        terminal.write(b"READY");
        let state = terminal.snapshot().expect("semantic state");

        return project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            ScreenFrameSource::Actor {
                session_id: Uuid::nil(),
                boundaries: boundaries(),
                observed_monotonic_ns: 13,
            },
            true,
            CaptureQuality::Live,
        )
        .expect("screen frame");
    }

    #[test]
    fn oversized_rich_response_retains_typed_response_limit_details() {
        let frame = actor_frame();
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::nil(),
            result: ControlResult::Ok {
                data: ControlData::ScreenFrame { frame },
            },
        };
        let (code, _, details) =
            oversized_response_error(&response, MAX_CONTROL_MESSAGE_BYTES + 37);

        assert_eq!(code, "screen_frame_resource_limit");

        let details = details.expect("rich response details");
        let screen = details.screen_frame.expect("screen-frame details");

        assert_eq!(screen.limit_kind.as_deref(), Some("response_bytes"));
        assert_eq!(screen.requested, Some(MAX_CONTROL_MESSAGE_BYTES + 37));
        assert_eq!(screen.maximum, Some(MAX_CONTROL_MESSAGE_BYTES));
        assert_eq!(screen.detail, Some(FrameDetail::Spans));
    }

    #[test]
    fn oversized_non_rich_response_is_not_misclassified_as_a_screen_frame() {
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::nil(),
            result: ControlResult::Ok {
                data: ControlData::Acknowledged {
                    boundaries: boundaries(),
                },
            },
        };
        let (code, _, details) = oversized_response_error(&response, MAX_CONTROL_MESSAGE_BYTES + 1);

        assert_eq!(code, "response_too_large");
        assert!(details.is_none());
    }

    #[test]
    fn connection_limiter_never_oversubscribes_or_leaks() {
        let active = AtomicUsize::new(0);
        let barrier = Barrier::new(2);

        thread::scope(|scope| {
            let permit = ConnectionPermit::acquire(&active, 1).expect("first connection");
            scope.spawn(|| {
                assert!(ConnectionPermit::acquire(&active, 1).is_none());

                barrier.wait();
                barrier.wait();

                let _permit = ConnectionPermit::acquire(&active, 1).expect("released capacity");

                assert!(ConnectionPermit::acquire(&active, 1).is_none());
            });

            barrier.wait();

            drop(permit);

            barrier.wait();
        });

        let _permit = ConnectionPermit::acquire(&active, 1).expect("worker released capacity");

        assert!(ConnectionPermit::acquire(&active, 1).is_none());
    }
}
