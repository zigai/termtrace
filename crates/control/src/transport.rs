//! Blocking client transport for the versioned live-session control protocol.

use crate::{
    phase::{evidence_phase, read_manifest},
    Error, Result,
};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::socket::{
        connect, getsockopt, socket,
        sockopt::{PeerCredentials, SocketError},
        AddressFamily, SockFlag, SockType, UnixAddr, UnixCredentials,
    },
    unistd::getuid,
};
use std::{
    io::{Error as IoError, ErrorKind, Read as _, Write as _},
    net::Shutdown,
    os::{
        fd::{AsFd as _, AsRawFd as _},
        unix::net::UnixStream,
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::session::SessionPhase,
    protocol::control::{
        is_control_frame, ControlCommand, ControlData, ControlRequest, ControlResponse,
        ControlResult, ScreenPredicate, CONTROL_PROTOCOL_VERSION, DEFAULT_CONTROL_RESPONSE_TIMEOUT,
        MAX_CONTROL_MESSAGE_BYTES,
    },
    protocol::mutation::MutationEffect,
    terminal::{inspection::ScreenWaitOutcome, screen::ScreenProjectionRequest},
};

/// Connect to the recorder's abstract control socket and authenticate its owner.
pub(crate) fn connect_recorder(
    session: &Path,
    timing: &ExchangeTiming,
) -> Result<(UnixStream, UnixCredentials)> {
    let manifest = match read_manifest(session) {
        Ok(manifest) => manifest,
        Err(Error::Filesystem { source, .. }) if source.kind() == ErrorKind::NotFound => {
            return Err(unavailable(
                session,
                IoError::new(ErrorKind::NotFound, "session manifest is unavailable"),
            ));
        }
        Err(error) => return Err(error),
    };

    let _ = timing.remaining(session, "connect", MutationEffect::NotApplied)?;

    let fd = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
        None,
    )
    .map_err(|error| return unavailable(session, IoError::from_raw_os_error(error as i32)))?;

    let unix_addr =
        UnixAddr::new_abstract(manifest.control_socket.as_bytes()).map_err(|error| {
            return Error::InvalidProtocol {
                reason: format!("invalid abstract control socket name: {error}"),
            };
        })?;

    match connect(fd.as_raw_fd(), &unix_addr) {
        Ok(()) => {}
        Err(Errno::EINPROGRESS) => {
            let mut descriptors = [PollFd::new(fd.as_fd(), PollFlags::POLLOUT)];

            loop {
                let remaining = timing.remaining(session, "connect", MutationEffect::NotApplied)?;
                let poll_timeout = PollTimeout::try_from(remaining.min(Duration::from_secs(60)))
                    .unwrap_or(PollTimeout::NONE);

                match poll(&mut descriptors, poll_timeout) {
                    Ok(0) => {
                        return Err(transport_error(
                            session,
                            "connect",
                            remaining,
                            MutationEffect::NotApplied,
                            IoError::new(ErrorKind::TimedOut, "control connect timed out"),
                        ));
                    }
                    Ok(_) => {
                        let so_error = getsockopt(&fd, SocketError).map_err(|error| {
                            return unavailable(session, IoError::from_raw_os_error(error as i32));
                        })?;

                        if so_error == 0 {
                            break;
                        }
                        return Err(unavailable(session, IoError::from_raw_os_error(so_error)));
                    }
                    Err(Errno::EINTR) => {}
                    Err(error) => {
                        return Err(transport_error(
                            session,
                            "connect",
                            remaining,
                            MutationEffect::NotApplied,
                            IoError::from_raw_os_error(error as i32),
                        ));
                    }
                }
            }
        }
        Err(error) => {
            return Err(unavailable(
                session,
                IoError::from_raw_os_error(error as i32),
            ));
        }
    }

    let stream = UnixStream::from(fd);
    let credentials = getsockopt(&stream, PeerCredentials).map_err(|error| {
        return Error::Unavailable {
            session: session.to_path_buf(),
            phase: SessionPhase::Stale,
            source: IoError::from_raw_os_error(error as i32),
        };
    })?;

    if credentials.uid() != getuid().as_raw() {
        return Err(Error::Unavailable {
            session: session.to_path_buf(),
            phase: SessionPhase::Stale,
            source: IoError::new(
                ErrorKind::PermissionDenied,
                "control endpoint belongs to a different user",
            ),
        });
    }
    return Ok((stream, credentials));
}

/// Return whether the live recorder's abstract control endpoint accepts a connection.
pub fn control_endpoint_is_reachable(session_directory: impl AsRef<Path>) -> bool {
    return connect_recorder(
        session_directory.as_ref(),
        &ExchangeTiming::Timeout(DEFAULT_CONTROL_RESPONSE_TIMEOUT),
    )
    .is_ok();
}

/// Send one command to a live session and wait for its correlated response.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, correlation, or
/// remote operation failures.
pub fn request(
    session_directory: impl AsRef<Path>,
    command: ControlCommand,
) -> Result<ControlData> {
    return send_request(session_directory, &ControlRequest::new(command));
}

/// Wait for an exact predicate on the live actor's latest presentation-eligible viewport.
///
/// # Errors
///
/// Returns an error when the actor is unreachable or rejects the wait.
pub fn wait_for_screen(
    session_directory: impl AsRef<Path>,
    predicate: ScreenPredicate,
    stable_for: Duration,
    timeout: Duration,
) -> Result<ScreenWaitOutcome> {
    let stable_for_ns = u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX);
    let timeout_ns = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);

    let result = request(
        session_directory,
        ControlCommand::WaitScreen {
            predicate: predicate.clone(),
            stable_for_ns,
            timeout_ns,
            projection: ScreenProjectionRequest::default(),
        },
    )?;

    let ControlData::ScreenWait {
        matched,
        first_match_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        quality,
        frame,
        ..
    } = result
    else {
        return Err(Error::InvalidProtocol {
            reason: "actor returned an unexpected screen-wait response".to_owned(),
        });
    };

    return Ok(ScreenWaitOutcome {
        predicate,
        matched,
        first_match_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        quality,
        frame,
    });
}

/// Send a caller-owned request, preserving its identifier for safe retries.
///
/// Reusing the same request cannot apply a mutation twice; reusing its
/// identifier for a different command is rejected.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, correlation, or
/// remote operation failures.
pub fn send_request(
    session_directory: impl AsRef<Path>,
    request: &ControlRequest,
) -> Result<ControlData> {
    return send_request_response(session_directory, request).and_then(accepted);
}

/// The actor's answer as data, or its rejection as [`Error::Rejected`].
pub(crate) fn accepted(response: ControlResponse) -> Result<ControlData> {
    match response.result {
        ControlResult::Ok { data } => return Ok(data),
        ControlResult::Error {
            code,
            message,
            effect,
            details,
        } => {
            return Err(Error::Rejected {
                code,
                message,
                effect,
                details: details.map(Box::new),
            });
        }
    }
}

/// Send a caller-owned request and retain structured success or rejection data.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, or correlation
/// failures. Actor rejections are returned in the response envelope.
pub fn send_request_response(
    session_directory: impl AsRef<Path>,
    request: &ControlRequest,
) -> Result<ControlResponse> {
    return exchange(
        session_directory.as_ref(),
        request,
        ExchangeTiming::Timeout(request.command.client_response_timeout()),
    );
}

/// Socket deadline or operation timeout configuration.
pub enum ExchangeTiming {
    /// Command-derived socket timeout, including any operation-specific slack.
    Timeout(Duration),
    /// Caller-owned total budget, recomputed before request and response I/O.
    Deadline(Instant),
}

impl ExchangeTiming {
    /// Compute an absolute deadline from the current time.
    ///
    /// # Errors
    ///
    /// Returns an error when the deadline exceeds the monotonic clock range.
    pub fn deadline(timeout: Duration) -> Result<Self> {
        return Self::Timeout(timeout).into_deadline().map(Self::Deadline);
    }

    /// Resolve this timing configuration into an absolute monotonic deadline.
    ///
    /// # Errors
    ///
    /// Returns an error when the deadline exceeds the monotonic clock range.
    pub fn into_deadline(self) -> Result<Instant> {
        return match self {
            Self::Timeout(timeout) => Instant::now().checked_add(timeout).ok_or_else(|| {
                return Error::InvalidRequest {
                    reason: "request timeout exceeds the monotonic clock range".to_owned(),
                };
            }),
            Self::Deadline(deadline) => Ok(deadline),
        };
    }

    fn remaining(
        &self,
        session: &Path,
        step: &'static str,
        effect: MutationEffect,
    ) -> Result<Duration> {
        return match self {
            Self::Timeout(timeout) => Ok(*timeout),
            Self::Deadline(deadline) => deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| return !remaining.is_zero())
                .ok_or_else(|| {
                    return transport_error(
                        session,
                        step,
                        Duration::ZERO,
                        effect,
                        IoError::new(ErrorKind::TimedOut, "control deadline elapsed"),
                    );
                }),
        };
    }
}

/// Exchange one request/response pair over the session control socket.
///
/// # Errors
///
/// Returns an error for connection, timeout, framing, schema, or correlation
/// failures. Actor rejections are returned in the response envelope.
pub fn exchange(
    session_directory: &Path,
    request: &ControlRequest,
    timing: ExchangeTiming,
) -> Result<ControlResponse> {
    let timing = ExchangeTiming::Deadline(timing.into_deadline()?);
    let (stream, _) = connect_recorder(session_directory, &timing)?;

    return exchange_stream(session_directory, stream, request, &timing);
}

fn exchange_stream(
    session_directory: &Path,
    mut stream: UnixStream,
    request: &ControlRequest,
    timing: &ExchangeTiming,
) -> Result<ControlResponse> {
    let unanswered = request.command.unanswered_effect();
    let mut request_frame = serde_json::to_vec(request).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<abstract-control-socket>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if u64::try_from(request_frame.len().saturating_add(1)).unwrap_or(u64::MAX)
        > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidRequest {
            reason: "request must be no larger than 16 MiB".to_owned(),
        });
    }

    request_frame.push(b'\n');

    let timeout = timing.remaining(
        session_directory,
        "write request",
        MutationEffect::NotApplied,
    )?;
    stream.set_nonblocking(false).map_err(|source| {
        return transport_error(
            session_directory,
            "configure deadlines",
            timeout,
            MutationEffect::NotApplied,
            source,
        );
    })?;
    stream.set_write_timeout(Some(timeout)).map_err(|source| {
        return transport_error(
            session_directory,
            "configure deadlines",
            timeout,
            MutationEffect::NotApplied,
            source,
        );
    })?;
    stream.write_all(&request_frame).map_err(|source| {
        return transport_error(
            session_directory,
            "write request",
            timeout,
            unanswered,
            source,
        );
    })?;
    stream.shutdown(Shutdown::Write).map_err(|source| {
        return transport_error(
            session_directory,
            "finish request",
            timeout,
            unanswered,
            source,
        );
    })?;
    stream.set_nonblocking(true).map_err(|source| {
        return transport_error(
            session_directory,
            "configure response deadline",
            timeout,
            unanswered,
            source,
        );
    })?;

    let timeout = timing.remaining(session_directory, "read response", unanswered)?;
    let mut encoded = Vec::new();
    let mut chunk = [0_u8; 8192];

    loop {
        let remaining = timing.remaining(session_directory, "read response", unanswered)?;
        let poll_timeout = PollTimeout::try_from(remaining.min(Duration::from_secs(60)))
            .unwrap_or(PollTimeout::NONE);

        let mut descriptors = [PollFd::new(stream.as_fd(), PollFlags::POLLIN)];

        match poll(&mut descriptors, poll_timeout) {
            Ok(0) => {
                return Err(transport_error(
                    session_directory,
                    "read response",
                    remaining,
                    unanswered,
                    IoError::new(ErrorKind::TimedOut, "control read timed out"),
                ));
            }
            Ok(_) => match stream.read(&mut chunk) {
                Ok(0) => {
                    break;
                }
                Ok(count) => {
                    let bytes = chunk.get(..count).ok_or_else(|| {
                        return Error::InvalidResponse {
                            reason: "invalid response slice".to_owned(),
                            effect: unanswered,
                        };
                    })?;
                    if let Some(position) = bytes.iter().position(|&byte| return byte == b'\n') {
                        let line = bytes.get(..=position).ok_or_else(|| {
                            return Error::InvalidResponse {
                                reason: "invalid response slice".to_owned(),
                                effect: unanswered,
                            };
                        })?;
                        encoded.extend_from_slice(line);
                        break;
                    }

                    encoded.extend_from_slice(bytes);
                    if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES
                    {
                        break;
                    }
                }
                Err(source)
                    if source.kind() == ErrorKind::Interrupted
                        || source.kind() == ErrorKind::WouldBlock => {}
                Err(source) => {
                    return Err(transport_error(
                        session_directory,
                        "read response",
                        remaining,
                        unanswered,
                        source,
                    ));
                }
            },
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(transport_error(
                    session_directory,
                    "read response",
                    remaining,
                    unanswered,
                    IoError::from_raw_os_error(error as i32),
                ));
            }
        }
    }

    validate_response_frame(session_directory, timeout, &encoded, unanswered)?;

    let response: ControlResponse = serde_json::from_slice(&encoded).map_err(|source| {
        return Error::InvalidResponse {
            reason: format!("response does not match the control schema: {source}"),
            effect: unanswered,
        };
    })?;

    if response.version != CONTROL_PROTOCOL_VERSION || response.request_id != request.request_id {
        return Err(Error::InvalidResponse {
            reason: "response version or request identifier does not match".to_owned(),
            effect: unanswered,
        });
    }
    return Ok(response);
}

fn unavailable(session: &Path, source: IoError) -> Error {
    return Error::Unavailable {
        session: session.to_path_buf(),
        phase: evidence_phase(session).phase,
        source,
    };
}

fn transport_error(
    session: &Path,
    step: &'static str,
    timeout: Duration,
    effect: MutationEffect,
    source: IoError,
) -> Error {
    if matches!(source.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        return Error::Timeout {
            session: session.to_path_buf(),
            step,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            effect,
            source,
        };
    }
    return Error::Transport {
        session: session.to_path_buf(),
        step,
        effect,
        source,
    };
}

fn validate_response_frame(
    session: &Path,
    timeout: Duration,
    encoded: &[u8],
    unanswered: MutationEffect,
) -> Result<()> {
    if encoded.is_empty() {
        return Err(transport_error(
            session,
            "read response",
            timeout,
            unanswered,
            IoError::new(
                ErrorKind::UnexpectedEof,
                "control endpoint closed without a response",
            ),
        ));
    }

    if !is_control_frame(encoded) {
        return Err(Error::InvalidResponse {
            reason: "message must be one newline-terminated JSON object no larger than 16 MiB"
                .to_owned(),
            effect: unanswered,
        });
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn stop_request_obeys_the_supplied_deadline() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let request = ControlRequest::new(ControlCommand::Stop);
        let error = exchange_stream(
            Path::new("/session"),
            client,
            &request,
            &ExchangeTiming::deadline(Duration::from_millis(20)).expect("deadline"),
        )
        .expect_err("withheld stop response must time out");
        assert!(matches!(
            error,
            Error::Timeout {
                step: "read response",
                effect: MutationEffect::Unknown,
                ..
            }
        ));

        drop(server);
    }

    #[test]
    fn slow_peer_sending_chunks_obeys_the_total_deadline() {
        let (client, mut server) = UnixStream::pair().expect("socket pair");
        let request = ControlRequest::new(ControlCommand::Stop);
        let total_timeout = Duration::from_millis(50);

        let sender = thread::spawn(move || {
            let mut buf = [0_u8; 1024];
            let _ = server.read(&mut buf);

            for byte in b"{\"version\":1" {
                thread::sleep(Duration::from_millis(30));

                if server.write_all(&[*byte]).is_err() {
                    break;
                }
            }
        });

        let error = exchange_stream(
            Path::new("/session"),
            client,
            &request,
            &ExchangeTiming::deadline(total_timeout).expect("deadline"),
        )
        .expect_err("slow peer must time out");
        assert!(matches!(
            error,
            Error::Timeout {
                step: "read response",
                effect: MutationEffect::Unknown,
                ..
            }
        ));

        let _ = sender.join();
    }

    #[test]
    fn exchange_times_out_when_deadline_is_already_expired() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let request = ControlRequest::new(ControlCommand::Stop);
        let expired_timing = ExchangeTiming::Deadline(
            Instant::now()
                .checked_sub(Duration::from_millis(10))
                .expect("instant"),
        );

        let error = exchange_stream(Path::new("/session"), client, &request, &expired_timing)
            .expect_err("expired deadline must time out");
        assert!(matches!(
            error,
            Error::Timeout {
                step: "write request",
                effect: MutationEffect::NotApplied,
                ..
            }
        ));

        drop(server);
    }

    #[test]
    fn missing_live_actor_reports_the_observed_phase() {
        let temporary = TempDir::new().expect("temporary directory");
        let error = request(temporary.path(), ControlCommand::Ping).expect_err("missing actor");
        assert!(matches!(
            error,
            Error::Unavailable {
                phase: SessionPhase::Incomplete,
                ..
            }
        ));
    }

    #[test]
    fn empty_control_response_is_a_transport_failure() {
        let error = validate_response_frame(
            Path::new("/session"),
            Duration::from_secs(1),
            &[],
            MutationEffect::Unknown,
        )
        .expect_err("empty response must be a transport failure");
        assert!(matches!(
            error,
            Error::Transport {
                step: "read response",
                effect: MutationEffect::Unknown,
                source,
                ..
            } if source.kind() == ErrorKind::UnexpectedEof
        ));
    }
}
