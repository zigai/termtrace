//! Authenticated control-socket transport and worker lifecycle.

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

use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::{Uid, pipe, write as fd_write},
};

use crate::{
    Error, Result,
    control::{
        CONTROL_PROTOCOL_VERSION, ControlRequest, ControlResponse, ControlResult,
        MAX_CONTROL_MESSAGE_BYTES, control_response_timeout, control_socket_address,
        validate_frame,
    },
};

use super::super::{
    model::RuntimeMessage,
    support::{finish_with_cleanup, poll_ready, terminal_error},
};

pub(in crate::record) struct ControlWorker {
    cancel: OwnedFd,
    thread: thread::JoinHandle<()>,
}

const MAX_CONTROL_CONNECTIONS: usize = 256;

struct ConnectionPermit<'counter> {
    active: &'counter AtomicUsize,
}

impl Drop for ConnectionPermit<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(in crate::record) fn spawn_control_server(
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
    let (cancel_read, cancel) = pipe().map_err(|error| {
        return terminal_error("create control cancellation pipe", error.to_string());
    })?;
    let thread = thread::Builder::new()
        .name("termtrace-control-server".to_owned())
        .spawn(move || {
            if let Err(error) = control_server_loop(&listener, &cancel_read, &sender) {
                let _ = sender.try_send(RuntimeMessage::ControlServerFailed(error.to_string()));
            }
        })
        .map_err(|error| return terminal_error("spawn control server", error.to_string()))?;
    tracing::debug!(socket = socket_name, "abstract control socket ready");
    return Ok(ControlWorker { cancel, thread });
}

fn control_server_loop(
    listener: &UnixListener,
    cancel: &OwnedFd,
    sender: &mpsc::SyncSender<RuntimeMessage>,
) -> Result<()> {
    let active = AtomicUsize::new(0);
    return thread::scope(|scope| {
        loop {
            let (listener_ready, cancel_ready) = {
                let mut descriptors = [
                    PollFd::new(listener.as_fd(), PollFlags::POLLIN),
                    PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
                ];
                match poll(&mut descriptors, PollTimeout::NONE) {
                    Ok(_) => (poll_ready(&descriptors[0]), poll_ready(&descriptors[1])),
                    Err(Errno::EINTR) => continue,
                    Err(error) => {
                        return Err(terminal_error("poll control socket", error.to_string()));
                    }
                }
            };
            if cancel_ready {
                return Ok(());
            }
            if !listener_ready {
                continue;
            }
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let permit =
                            active.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                                return (count < MAX_CONTROL_CONNECTIONS).then_some(count + 1);
                            });
                        if permit.is_err() {
                            continue;
                        }
                        let active = &active;
                        scope.spawn(move || {
                            let _permit = ConnectionPermit { active };
                            let _result = handle_control_connection(stream, sender, cancel);
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
    serde_json::to_writer(&mut *stream, &response).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<control-socket>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return stream
        .write_all(b"\n")
        .and_then(|()| return stream.flush())
        .map_err(|error| return terminal_error("write control response", error.to_string()));
}

impl ControlWorker {
    pub(in crate::record) fn stop(self) -> Result<()> {
        let cancel_result = match fd_write(&self.cancel, b"x") {
            Ok(_) | Err(Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error("cancel control server", error.to_string())),
        };
        let join_result = self
            .thread
            .join()
            .map_err(|_| return terminal_error("join control server", "control thread panicked"));
        return finish_with_cleanup(cancel_result, join_result);
    }
}
