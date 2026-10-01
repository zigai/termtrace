//! Authenticated control-socket transport and worker lifecycle.

use super::super::{
    model::RuntimeMessage,
    support::{finish_with_cleanup, poll_ready, terminal_error, thread_panicked},
};
use super::transport::{control_socket_address, validate_frame};
use crate::{Error, Result};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::{pipe, write as fd_write, Uid},
};
use std::{
    io::{ErrorKind, Read as _, Write as _},
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
        ActionPostconditionResult, ControlErrorDetails, ControlPayload, ControlRequest,
        ControlResponse, ControlResult, ScreenFrameFailureDetails, CONTROL_PROTOCOL_VERSION,
        MAX_CONTROL_MESSAGE_BYTES,
    },
    protocol::mutation::MutationEffect,
    terminal::observation::ActorBoundaries,
    terminal::screen::{FrameDetail, ScreenFrame, ScreenFrameContent, ScreenFrameSource},
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

pub(in crate::recording) fn spawn(
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
                let reason = error.to_string();

                if let Err(delivery) = sender.try_send(RuntimeMessage::ControlServerFailed(reason))
                {
                    tracing::warn!(
                        %error,
                        %delivery,
                        "control server failure could not be delivered to the recording runtime"
                    );
                }
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

        const MAX_ACCEPT_BATCH: usize = 32;
        let mut accepted_count = 0;
        loop {
            if accepted_count >= MAX_ACCEPT_BATCH {
                break;
            }

            match listener.accept() {
                Ok((stream, _)) => {
                    accepted_count = accepted_count.saturating_add(1);
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

    let request_timeout = Duration::from_secs(10);
    let Some(request) = read_control_request(&mut stream, cancel, request_timeout)? else {
        return Ok(());
    };
    if request.version != CONTROL_PROTOCOL_VERSION {
        return write_control_response(
            &mut stream,
            cancel,
            &ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: ControlResult::Error {
                    code: "unsupported_version".to_owned(),
                    message: format!("protocol version {} is unsupported", request.version),
                    effect: MutationEffect::NotApplied,
                    details: None,
                },
            },
            request_timeout,
            MutationEffect::NotApplied,
        );
    }

    let request_id = request.request_id;
    let response_timeout = request.command.actor_response_timeout();
    let unanswered = request.command.unanswered_effect();
    let success_effect = if request.command.is_mutation() {
        MutationEffect::Applied
    } else {
        MutationEffect::NotApplied
    };

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
                    let response = ControlResponse {
                        version: CONTROL_PROTOCOL_VERSION,
                        request_id,
                        result: ControlResult::Error {
                            code: "actor_unresponsive".to_owned(),
                            message: "recording actor is unresponsive".to_owned(),
                            effect: MutationEffect::NotApplied,
                            details: None,
                        },
                    };

                    if let Err(error) = write_control_response(
                        &mut stream,
                        cancel,
                        &response,
                        response_timeout,
                        MutationEffect::NotApplied,
                    ) {
                        tracing::debug!(%error, "could not report actor failure to control client");
                    }

                    return Err(terminal_error(
                        "dispatch control request",
                        "recording actor is unresponsive",
                    ));
                }

                message = returned;

                let mut descriptors = [PollFd::new(cancel.as_fd(), PollFlags::POLLIN)];
                if poll(&mut descriptors, PollTimeout::from(10_u16)).is_ok()
                    && poll_ready(&descriptors[0])
                {
                    return Ok(());
                }
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                let response = ControlResponse {
                    version: CONTROL_PROTOCOL_VERSION,
                    request_id,
                    result: ControlResult::Error {
                        code: "actor_disconnected".to_owned(),
                        message: "recording actor disconnected".to_owned(),
                        effect: MutationEffect::NotApplied,
                        details: None,
                    },
                };

                if let Err(error) = write_control_response(
                    &mut stream,
                    cancel,
                    &response,
                    response_timeout,
                    MutationEffect::NotApplied,
                ) {
                    tracing::debug!(%error, "could not report actor failure to control client");
                }

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
                    let response = ControlResponse {
                        version: CONTROL_PROTOCOL_VERSION,
                        request_id,
                        result: ControlResult::Error {
                            code: "actor_unresponsive".to_owned(),
                            message: "recording actor is unresponsive".to_owned(),
                            effect: unanswered,
                            details: None,
                        },
                    };

                    if let Err(error) = write_control_response(
                        &mut stream,
                        cancel,
                        &response,
                        response_timeout,
                        MutationEffect::NotApplied,
                    ) {
                        tracing::debug!(%error, "could not report actor failure to control client");
                    }

                    return Err(terminal_error(
                        "wait for control response",
                        "recording actor is unresponsive",
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let response = ControlResponse {
                    version: CONTROL_PROTOCOL_VERSION,
                    request_id,
                    result: ControlResult::Error {
                        code: "actor_disconnected".to_owned(),
                        message: "recording actor disconnected".to_owned(),
                        effect: unanswered,
                        details: None,
                    },
                };

                if let Err(error) = write_control_response(
                    &mut stream,
                    cancel,
                    &response,
                    response_timeout,
                    MutationEffect::NotApplied,
                ) {
                    tracing::debug!(%error, "could not report actor failure to control client");
                }

                return Err(terminal_error(
                    "wait for control response",
                    "recording actor disconnected",
                ));
            }
        }
    };

    return write_control_response(
        &mut stream,
        cancel,
        &response,
        response_timeout,
        success_effect,
    );
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

fn read_control_request(
    stream: &mut UnixStream,
    cancel: &OwnedFd,
    timeout: Duration,
) -> Result<Option<ControlRequest>> {
    stream.set_nonblocking(true).map_err(|error| {
        return terminal_error(
            "configure nonblocking control connection",
            error.to_string(),
        );
    })?;

    let deadline = Instant::now() + timeout;
    let mut encoded = Vec::new();
    let mut chunk = [0u8; 8192];
    let max_bytes = usize::try_from(MAX_CONTROL_MESSAGE_BYTES).unwrap_or(usize::MAX);

    loop {
        if cancellation_requested(cancel)? {
            return Ok(None);
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(terminal_error(
                "read control request",
                "request read deadline exceeded",
            ));
        }

        let remaining = deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(1));

        let poll_timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::NONE);

        let mut descriptors = [
            PollFd::new(stream.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];

        match poll(&mut descriptors, poll_timeout) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => {
                return Err(terminal_error("poll control request", error.to_string()));
            }
        }

        if poll_ready(&descriptors[1]) {
            return Ok(None);
        }

        if !poll_ready(&descriptors[0]) {
            continue;
        }

        match stream.read(&mut chunk) {
            Ok(0) => {
                break;
            }
            Ok(count) => {
                let Some(slice) = chunk.get(..count) else {
                    continue;
                };
                if let Some(newline_index) = slice.iter().position(|b| return *b == b'\n') {
                    if encoded.len().saturating_add(newline_index) > max_bytes {
                        return Err(Error::ControlRejected {
                            code: "request_too_large".to_owned(),
                            message: format!(
                                "control request exceeds the {MAX_CONTROL_MESSAGE_BYTES}-byte limit"
                            ),
                        });
                    }

                    if let Some(sub) = slice.get(..=newline_index) {
                        encoded.extend_from_slice(sub);
                    }

                    break;
                }

                if encoded.len().saturating_add(count) > max_bytes {
                    return Err(Error::ControlRejected {
                        code: "request_too_large".to_owned(),
                        message: format!(
                            "control request exceeds the {MAX_CONTROL_MESSAGE_BYTES}-byte limit"
                        ),
                    });
                }

                encoded.extend_from_slice(slice);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(terminal_error("read control request", error.to_string()));
            }
        }
    }

    if encoded.is_empty() {
        return Ok(None);
    }

    validate_frame(&encoded)?;
    let request: ControlRequest = serde_json::from_slice(&encoded).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<control-socket>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    return Ok(Some(request));
}

fn write_control_response(
    stream: &mut UnixStream,
    cancel: &OwnedFd,
    response: &ControlResponse,
    timeout: Duration,
    success_effect: MutationEffect,
) -> Result<()> {
    let encoded = serde_json::to_vec(response).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<control-socket>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let response_bytes = u64::try_from(encoded.len().saturating_add(1)).unwrap_or(u64::MAX);
    let mut payload = if response_bytes > MAX_CONTROL_MESSAGE_BYTES {
        let (code, message, details) = oversized_response_error(response, response_bytes);
        let effect = match &response.result {
            ControlResult::Ok { .. } => success_effect,
            ControlResult::Error { effect, .. } => *effect,
        };

        serde_json::to_vec(&ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: response.request_id,
            result: ControlResult::Error {
                code,
                message,
                effect,
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
    payload.push(b'\n');
    write_all_with_cancellation(stream, cancel, &payload, timeout)?;

    return Ok(());
}

fn write_all_with_cancellation(
    stream: &mut UnixStream,
    cancel: &OwnedFd,
    mut payload: &[u8],
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;

    while !payload.is_empty() {
        if cancellation_requested(cancel)? {
            return Ok(());
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(terminal_error(
                "write control response",
                "response write deadline exceeded",
            ));
        }

        let remaining = deadline
            .saturating_duration_since(now)
            .max(Duration::from_millis(1));

        let poll_timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::NONE);

        let mut descriptors = [
            PollFd::new(stream.as_fd(), PollFlags::POLLOUT),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];

        match poll(&mut descriptors, poll_timeout) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => {
                return Err(terminal_error("poll control response", error.to_string()));
            }
        }

        if poll_ready(&descriptors[1]) {
            return Ok(());
        }

        let revents = descriptors[0].revents().unwrap_or(PollFlags::empty());
        if !revents.intersects(
            PollFlags::POLLOUT | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
        ) {
            continue;
        }

        match stream.write(payload) {
            Ok(0) => {
                return Err(terminal_error(
                    "write control response",
                    "control peer closed connection prematurely",
                ));
            }
            Ok(count) => {
                let Some(remaining) = payload.get(count..) else {
                    return Ok(());
                };
                payload = remaining;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                return Err(terminal_error("write control response", error.to_string()));
            }
        }
    }

    return Ok(());
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
        ScreenFrameContent::None => FrameDetail::None,
        ScreenFrameContent::Text { .. } => FrameDetail::Text,
        ScreenFrameContent::Spans { .. } => FrameDetail::Spans,
        ScreenFrameContent::Cells { .. } => FrameDetail::Cells,
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
    data: &ControlPayload,
) -> Option<(&ScreenFrame, ActorBoundaries, &'static str)> {
    let (frame, boundaries, scope) = match data {
        ControlPayload::ScreenFrame { frame } => (
            frame,
            actor_frame_boundaries(frame)?,
            "screen_frame_response",
        ),
        ControlPayload::StableSnapshot { frame, .. } => (
            frame,
            actor_frame_boundaries(frame)?,
            "stable_snapshot_response",
        ),
        ControlPayload::ScreenWait { frame, .. } => (
            frame,
            actor_frame_boundaries(frame)?,
            "screen_wait_response",
        ),
        ControlPayload::ActionApplied {
            postcondition,
            boundaries,
            ..
        } => {
            let frame = match postcondition {
                ActionPostconditionResult::None { frame, .. }
                | ActionPostconditionResult::Idle { frame, .. }
                | ActionPostconditionResult::Screen { frame, .. } => frame.as_ref()?,
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
            .map_err(|payload| return thread_panicked("join control server", payload));

        let result = finish_with_cleanup(stop_accepting_result, cancel_result);

        return finish_with_cleanup(result, join_result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use termtrace_evidence::event_log::CanonicalBoundary;
    use termtrace_model::protocol::control::{ActionInputResult, ScreenPostconditionQuality};
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
                data: ControlPayload::ScreenFrame { frame },
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
                data: ControlPayload::Acknowledged {
                    boundaries: boundaries(),
                },
            },
        };
        let (code, _, details) = oversized_response_error(&response, MAX_CONTROL_MESSAGE_BYTES + 1);
        assert_eq!(code, "response_too_large");
        assert!(details.is_none());
    }

    #[test]
    fn oversized_action_applied_response_retains_action_response_scope() {
        let frame = actor_frame();
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::nil(),
            result: ControlResult::Ok {
                data: ControlPayload::ActionApplied {
                    input: ActionInputResult {
                        sequence_id: Uuid::nil(),
                        atom_count: 1,
                        bytes_written: 4,
                        first_intent_event_seq: 1,
                        last_result_event_seq: 2,
                        pace_ns: 0,
                        atoms: Vec::new(),
                    },
                    postcondition: ActionPostconditionResult::Screen {
                        matched: true,
                        quality: ScreenPostconditionQuality::MatchedUnsettled,
                        state_hash: None,
                        frame: Some(frame),
                    },
                    boundaries: boundaries(),
                },
            },
        };
        let (code, _, details) =
            oversized_response_error(&response, MAX_CONTROL_MESSAGE_BYTES + 42);
        assert_eq!(code, "screen_frame_resource_limit");

        let details = details.expect("action response details");

        assert_eq!(details.scope, "action_response");

        let screen = details.screen_frame.expect("screen-frame details");

        assert_eq!(screen.limit_kind.as_deref(), Some("response_bytes"));
        assert_eq!(screen.requested, Some(MAX_CONTROL_MESSAGE_BYTES + 42));
        assert_eq!(screen.maximum, Some(MAX_CONTROL_MESSAGE_BYTES));
        assert_eq!(screen.detail, Some(FrameDetail::Spans));
    }

    #[test]
    fn canvas_4k_lightweight_text_projection_stays_compact_compared_to_rich_spans() {
        let mut terminal = GhosttyVt::new(384, 108, 0).expect("4K terminal");
        terminal.write(b"READY\r\nCOUNT:1:test");
        let state = terminal.snapshot().expect("semantic state");

        let spans_frame = project_screen_frame(
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
        .expect("spans frame");

        let spans_response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::nil(),
            result: ControlResult::Ok {
                data: ControlPayload::ActionApplied {
                    input: ActionInputResult {
                        sequence_id: Uuid::nil(),
                        atom_count: 1,
                        bytes_written: 4,
                        first_intent_event_seq: 1,
                        last_result_event_seq: 2,
                        pace_ns: 0,
                        atoms: Vec::new(),
                    },
                    postcondition: ActionPostconditionResult::Screen {
                        matched: true,
                        quality: ScreenPostconditionQuality::MatchedUnsettled,
                        state_hash: None,
                        frame: Some(spans_frame),
                    },
                    boundaries: boundaries(),
                },
            },
        };
        let spans_bytes = serde_json::to_vec(&spans_response)
            .expect("spans json")
            .len();
        assert!(spans_bytes > 1_500_000);

        let text_frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest {
                detail: FrameDetail::Text,
                region: None,
            },
            ScreenFrameSource::Actor {
                session_id: Uuid::nil(),
                boundaries: boundaries(),
                observed_monotonic_ns: 13,
            },
            true,
            CaptureQuality::Live,
        )
        .expect("text frame");

        let text_response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::nil(),
            result: ControlResult::Ok {
                data: ControlPayload::ActionApplied {
                    input: ActionInputResult {
                        sequence_id: Uuid::nil(),
                        atom_count: 1,
                        bytes_written: 4,
                        first_intent_event_seq: 1,
                        last_result_event_seq: 2,
                        pace_ns: 0,
                        atoms: Vec::new(),
                    },
                    postcondition: ActionPostconditionResult::Screen {
                        matched: true,
                        quality: ScreenPostconditionQuality::MatchedUnsettled,
                        state_hash: None,
                        frame: Some(text_frame),
                    },
                    boundaries: boundaries(),
                },
            },
        };
        let text_bytes = serde_json::to_vec(&text_response).expect("text json").len();
        assert!(text_bytes < 100_000);
        assert!(text_bytes < spans_bytes / 15);
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

    #[test]
    fn incomplete_request_read_is_interrupted_promptly_on_shutdown() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let socket_name = format!("termtrace-test-h1-{}", Uuid::now_v7());
        let worker = spawn(&socket_name, sender).expect("control server");
        let socket = control_socket_address(&socket_name).expect("socket address");
        let mut client = UnixStream::connect_addr(&socket).expect("connect client");
        client.write_all(b"{\"version\":5").expect("partial write");
        client.flush().expect("flush partial write");

        let start = Instant::now();
        worker.stop().expect("stop control server");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "shutdown took {elapsed:?}, expected < 1s"
        );
    }
}
