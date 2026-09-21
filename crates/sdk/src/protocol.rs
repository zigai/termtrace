//! Blocking client transport for the versioned live-session control protocol.

use crate::{Error, Result};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::socket::{
        connect, getsockopt, socket,
        sockopt::{PeerCredentials, SocketError},
        AddressFamily, SockFlag, SockType, UnixAddr,
    },
    unistd::getuid,
};
use std::{
    fs::File,
    io::{Error as IoError, ErrorKind, Read as _, Write as _},
    net::Shutdown,
    os::{
        fd::{AsFd as _, AsRawFd as _},
        linux::net::SocketAddrExt as _,
        unix::net::{SocketAddr, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::session::SessionManifest,
    protocol::control::{
        ActionPostcondition, ControlCommand, ControlData, ControlRequest, ControlResponse,
        ControlResult, InputSequenceAtom, ScreenPredicate, CONTROL_PROTOCOL_VERSION,
        MAX_CONTROL_MESSAGE_BYTES,
    },
    terminal::{inspection::ScreenWaitOutcome, screen::ScreenProjectionRequest},
};

const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INPUT_SEQUENCE_PACING: Duration = Duration::from_secs(60);

fn read_control_manifest(session_directory: &Path) -> Result<SessionManifest> {
    let path = session_directory.join("session.json");
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|file| {
            return file
                .take(MAX_CONTROL_MESSAGE_BYTES + 1)
                .read_to_end(&mut bytes);
        })
        .map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES {
        return Err(Error::InvalidControlProtocol {
            reason: "session manifest exceeds the client transport limit".to_owned(),
        });
    }
    return serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path,
            json_path: "$".to_owned(),
            source,
        };
    });
}

pub(crate) fn control_socket_address(name: &str) -> Result<SocketAddr> {
    return SocketAddr::from_abstract_name(name.as_bytes()).map_err(|error| {
        return Error::InvalidControlProtocol {
            reason: format!("invalid abstract control socket name: {error}"),
        };
    });
}

pub(crate) fn connect_session_control(session_directory: &Path) -> Result<UnixStream> {
    return connect_session_control_timing(
        session_directory,
        &ExchangeTiming::Timeout(DEFAULT_CONTROL_TIMEOUT),
    );
}

pub(crate) fn connect_session_control_timing(
    session_directory: &Path,
    timing: &ExchangeTiming,
) -> Result<UnixStream> {
    if !session_directory.join("session.json").is_file() {
        return Err(Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: control_lifecycle_hint(session_directory),
            source: IoError::new(ErrorKind::NotFound, "session manifest is unavailable"),
        });
    }

    let manifest = read_control_manifest(session_directory)?;
    let _ = control_socket_address(&manifest.control_socket)?;
    let _ = timing.remaining(session_directory, "connect", false)?;

    let fd = socket(
        AddressFamily::Unix,
        SockType::Stream,
        SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
        None,
    )
    .map_err(|source| {
        return Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: control_lifecycle_hint(session_directory),
            source: IoError::from_raw_os_error(source as i32),
        };
    })?;

    let unix_addr = UnixAddr::new_abstract(manifest.control_socket.as_bytes()).map_err(|error| {
        return Error::InvalidControlProtocol {
            reason: format!("invalid abstract control socket name: {error}"),
        };
    })?;

    match connect(fd.as_raw_fd(), &unix_addr) {
        Ok(()) => {}
        Err(Errno::EINPROGRESS) => {
            let mut descriptors = [PollFd::new(fd.as_fd(), PollFlags::POLLOUT)];
            loop {
                let remaining = timing.remaining(session_directory, "connect", false)?;
                let poll_timeout = PollTimeout::try_from(remaining.min(Duration::from_secs(60)))
                    .unwrap_or(PollTimeout::NONE);

                match poll(&mut descriptors, poll_timeout) {
                    Ok(0) => {
                        return Err(control_transport_error(
                            session_directory,
                            "connect",
                            remaining,
                            false,
                            IoError::new(ErrorKind::TimedOut, "control connect timed out"),
                        ));
                    }
                    Ok(_) => {
                        let so_error = getsockopt(&fd, SocketError).map_err(|error| {
                            return Error::ControlUnavailable {
                                session: session_directory.to_path_buf(),
                                lifecycle: control_lifecycle_hint(session_directory),
                                source: IoError::from_raw_os_error(error as i32),
                            };
                        })?;
                        if so_error == 0 {
                            break;
                        }
                        return Err(Error::ControlUnavailable {
                            session: session_directory.to_path_buf(),
                            lifecycle: control_lifecycle_hint(session_directory),
                            source: IoError::from_raw_os_error(so_error),
                        });
                    }
                    Err(Errno::EINTR) => {}
                    Err(error) => {
                        return Err(control_transport_error(
                            session_directory,
                            "connect",
                            remaining,
                            false,
                            IoError::from_raw_os_error(error as i32),
                        ));
                    }
                }
            }
        }
        Err(error) => {
            return Err(Error::ControlUnavailable {
                session: session_directory.to_path_buf(),
                lifecycle: control_lifecycle_hint(session_directory),
                source: IoError::from_raw_os_error(error as i32),
            });
        }
    }

    let stream = UnixStream::from(fd);
    let credentials = getsockopt(&stream, PeerCredentials).map_err(|error| {
        return Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: "control_peer_identity_unavailable".to_owned(),
            source: IoError::from_raw_os_error(error as i32),
        };
    })?;

    if credentials.uid() != getuid().as_raw() {
        return Err(Error::ControlUnavailable {
            session: session_directory.to_path_buf(),
            lifecycle: "control_peer_owner_mismatch".to_owned(),
            source: IoError::new(
                ErrorKind::PermissionDenied,
                "control endpoint belongs to a different user",
            ),
        });
    }
    return Ok(stream);
}


/// Return whether the live recorder's abstract control endpoint accepts a connection.
pub fn control_endpoint_is_reachable(session_directory: impl AsRef<Path>) -> bool {
    return connect_session_control(session_directory.as_ref()).is_ok();
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

/// Wait for an exact predicate on the latest presentation-eligible viewport.
pub fn wait_for_screen(
    session_directory: impl AsRef<Path>,
    predicate: ScreenPredicate,
    stable_for: Duration,
    timeout: Duration,
) -> Result<ScreenWaitOutcome> {
    let session_directory = session_directory.as_ref();
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
        return Err(Error::InvalidOperation {
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
    let response = send_request_response(session_directory, request)?;
    match response.result {
        ControlResult::Ok { data } => return Ok(data),
        ControlResult::Error { code, message, .. } => {
            return Err(Error::ControlRejected { code, message });
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
        ExchangeTiming::Timeout(control_response_timeout(
            &request.command,
            Duration::from_secs(3),
        )),
    );
}

pub(crate) enum ExchangeTiming {
    /// Command-derived socket timeout, including any operation-specific slack.
    Timeout(Duration),
    /// Caller-owned total budget, recomputed before request and response I/O.
    Deadline(Instant),
}

impl ExchangeTiming {
    pub(crate) fn deadline(timeout: Duration) -> Result<Self> {
        return Instant::now()
            .checked_add(timeout)
            .map(Self::Deadline)
            .ok_or_else(|| {
                return Error::InvalidDuration {
                    field: "request_timeout",
                    reason: "request timeout exceeds the monotonic clock range".to_owned(),
                };
            });
    }

    pub(crate) fn into_deadline(self) -> Result<Instant> {
        return match self {
            Self::Timeout(timeout) => Instant::now().checked_add(timeout).ok_or_else(|| {
                return Error::InvalidDuration {
                    field: "request_timeout",
                    reason: "request timeout exceeds the monotonic clock range".to_owned(),
                };
            }),
            Self::Deadline(deadline) => Ok(deadline),
        };
    }

    fn remaining(
        &self,
        session: &Path,
        phase: &'static str,
        request_dispatched: bool,
    ) -> Result<Duration> {
        return match self {
            Self::Timeout(timeout) => Ok(*timeout),
            Self::Deadline(deadline) => deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| return !remaining.is_zero())
                .ok_or_else(|| {
                    return control_transport_error(
                        session,
                        phase,
                        Duration::ZERO,
                        request_dispatched,
                        IoError::new(ErrorKind::TimedOut, "control deadline elapsed"),
                    );
                }),
        };
    }
}

pub(crate) fn exchange(
    session_directory: &Path,
    request: &ControlRequest,
    timing: ExchangeTiming,
) -> Result<ControlResponse> {
    let timing = ExchangeTiming::Deadline(timing.into_deadline()?);
    let stream = connect_session_control_timing(session_directory, &timing)?;

    return exchange_stream(session_directory, stream, request, timing);
}

fn exchange_stream(
    session_directory: &Path,
    mut stream: UnixStream,
    request: &ControlRequest,
    timing: ExchangeTiming,
) -> Result<ControlResponse> {
    let timing = ExchangeTiming::Deadline(timing.into_deadline()?);
    let socket = PathBuf::from("<abstract-control-socket>");
    let mut request_frame = serde_json::to_vec(request).map_err(|source| {
        return Error::InvalidJson {
            path: socket.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if u64::try_from(request_frame.len().saturating_add(1)).unwrap_or(u64::MAX)
        > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "request must be no larger than 16 MiB".to_owned(),
        });
    }

    request_frame.push(b'\n');

    let timeout = timing.remaining(session_directory, "write request", false)?;
    stream.set_nonblocking(false).map_err(|source| {
        return control_transport_error(
            session_directory,
            "configure deadlines",
            timeout,
            false,
            source,
        );
    })?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|source| {
            return control_transport_error(
                session_directory,
                "configure deadlines",
                timeout,
                false,
                source,
            );
        })?;
    stream.write_all(&request_frame).map_err(|source| {
        return control_transport_error(session_directory, "write request", timeout, true, source);
    })?;
    stream.shutdown(Shutdown::Write).map_err(|source| {
        return control_transport_error(session_directory, "finish request", timeout, true, source);
    })?;

    stream.set_nonblocking(true).map_err(|source| {
        return control_transport_error(
            session_directory,
            "configure response deadline",
            timeout,
            true,
            source,
        );
    })?;

    let timeout = timing.remaining(session_directory, "read response", true)?;
    let mut encoded = Vec::new();
    let mut chunk = [0_u8; 8192];

    loop {
        let remaining = timing.remaining(session_directory, "read response", true)?;
        let poll_timeout = PollTimeout::try_from(remaining.min(Duration::from_secs(60)))
            .unwrap_or(PollTimeout::NONE);
        let mut descriptors = [PollFd::new(stream.as_fd(), PollFlags::POLLIN)];

        match poll(&mut descriptors, poll_timeout) {
            Ok(0) => {
                return Err(control_transport_error(
                    session_directory,
                    "read response",
                    remaining,
                    true,
                    IoError::new(ErrorKind::TimedOut, "control read timed out"),
                ));
            }
            Ok(_) => {
                match stream.read(&mut chunk) {
                    Ok(0) => {
                        break;
                    }
                    Ok(count) => {
                        let bytes = chunk.get(..count).ok_or_else(|| {
                            return Error::InvalidControlProtocol {
                                reason: "invalid response slice".to_owned(),
                            };
                        })?;
                        if let Some(position) = bytes.iter().position(|&byte| return byte == b'\n') {
                            let line = bytes.get(..=position).ok_or_else(|| {
                                return Error::InvalidControlProtocol {
                                    reason: "invalid response slice".to_owned(),
                                };
                            })?;
                            encoded.extend_from_slice(line);
                            break;
                        }
                        encoded.extend_from_slice(bytes);
                        if u64::try_from(encoded.len()).unwrap_or(u64::MAX)
                            > MAX_CONTROL_MESSAGE_BYTES
                        {
                            break;
                        }
                    }
                    Err(source)
                        if source.kind() == ErrorKind::Interrupted
                            || source.kind() == ErrorKind::WouldBlock => {}
                    Err(source) => {
                        return Err(control_transport_error(
                            session_directory,
                            "read response",
                            remaining,
                            true,
                            source,
                        ));
                    }
                }
            }
            Err(Errno::EINTR) => {}
            Err(error) => {
                return Err(control_transport_error(
                    session_directory,
                    "read response",
                    remaining,
                    true,
                    IoError::from_raw_os_error(error as i32),
                ));
            }
        }
    }

    validate_response_frame(session_directory, timeout, &encoded)?;

    let response: ControlResponse = serde_json::from_slice(&encoded).map_err(|source| {
        return Error::InvalidJson {
            path: socket,
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if response.version != CONTROL_PROTOCOL_VERSION || response.request_id != request.request_id {
        return Err(Error::InvalidControlProtocol {
            reason: "response version or request identifier does not match".to_owned(),
        });
    }
    return Ok(response);
}


pub(crate) fn control_response_timeout(command: &ControlCommand, wait_slack: Duration) -> Duration {
    return match command {
        ControlCommand::WaitScreen { timeout_ns, .. }
        | ControlCommand::CaptureStable { timeout_ns, .. } => {
            Duration::from_nanos(*timeout_ns).saturating_add(wait_slack)
        }
        ControlCommand::InputSequence { atoms, pace_ns, .. } => {
            input_sequence_response_timeout(atoms, *pace_ns)
        }
        ControlCommand::Act {
            atoms,
            pace_ns,
            postcondition,
            ..
        } => {
            let postcondition_timeout = match postcondition {
                ActionPostcondition::None => Duration::ZERO,
                ActionPostcondition::Idle { timeout_ns, .. }
                | ActionPostcondition::Screen { timeout_ns, .. } => {
                    Duration::from_nanos(*timeout_ns)
                }
            };

            input_sequence_response_timeout(atoms, *pace_ns)
                .saturating_add(postcondition_timeout)
                .saturating_add(wait_slack)
        }
        _ => DEFAULT_CONTROL_TIMEOUT,
    };
}

fn input_sequence_response_timeout(atoms: &[InputSequenceAtom], pace_ns: u64) -> Duration {
    let gaps = u64::try_from(atoms.len().saturating_sub(1)).unwrap_or(u64::MAX);
    let pacing = Duration::from_nanos(pace_ns.saturating_mul(gaps)).min(MAX_INPUT_SEQUENCE_PACING);

    return DEFAULT_CONTROL_TIMEOUT.saturating_add(pacing);
}

fn control_lifecycle_hint(session: &Path) -> String {
    if session.join("completion.json").is_file() {
        return "completed".to_owned();
    }

    if !session.join("session.json").is_file() {
        return "not_a_session".to_owned();
    }
    return "control_endpoint_unreachable_or_recorder_starting".to_owned();
}

fn control_transport_error(
    session: &Path,
    phase: &'static str,
    timeout: Duration,
    request_dispatched: bool,
    source: IoError,
) -> Error {
    if matches!(source.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        return Error::ControlTimeout {
            session: session.to_path_buf(),
            phase,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            request_dispatched,
            source,
        };
    }
    return Error::ControlTransport {
        session: session.to_path_buf(),
        phase,
        request_dispatched,
        source,
    };
}

fn validate_response_frame(session: &Path, timeout: Duration, encoded: &[u8]) -> Result<()> {
    if encoded.is_empty() {
        return Err(control_transport_error(
            session,
            "read response",
            timeout,
            true,
            IoError::new(
                ErrorKind::UnexpectedEof,
                "control endpoint closed without a response",
            ),
        ));
    }
    return validate_frame(encoded);
}

fn validate_frame(encoded: &[u8]) -> Result<()> {
    if encoded.is_empty()
        || encoded.last() != Some(&b'\n')
        || u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "message must be one newline-terminated JSON object no larger than 16 MiB"
                .to_owned(),
        });
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use tempfile::TempDir;
    use termtrace_model::protocol::control::ControlCommand;

    #[test]
    fn stop_request_obeys_the_supplied_deadline() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let request = ControlRequest::new(ControlCommand::Stop);
        let error = exchange_stream(
            Path::new("/session"),
            client,
            &request,
            ExchangeTiming::deadline(Duration::from_millis(20)).expect("deadline"),
        )
        .expect_err("withheld stop response must time out");
        assert!(matches!(
            error,
            Error::ControlTimeout {
                phase: "read response",
                request_dispatched: true,
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
            ExchangeTiming::deadline(total_timeout).expect("deadline"),
        )
        .expect_err("slow peer must time out");

        assert!(matches!(
            error,
            Error::ControlTimeout {
                phase: "read response",
                request_dispatched: true,
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
            Instant::now().checked_sub(Duration::from_millis(10)).expect("instant"),
        );
        let error = exchange_stream(Path::new("/session"), client, &request, expired_timing)
            .expect_err("expired deadline must time out");
        assert!(matches!(
            error,
            Error::ControlTimeout {
                phase: "write request",
                request_dispatched: false,
                ..
            }
        ));
        drop(server);
    }

    #[test]
    fn missing_live_actor_has_a_typed_lifecycle_error() {
        let temporary = TempDir::new().expect("temporary directory");
        let error = request(temporary.path(), ControlCommand::Ping).expect_err("missing actor");
        assert!(matches!(
            error,
            Error::ControlUnavailable { lifecycle, .. } if lifecycle == "not_a_session"
        ));
    }

    #[test]
    fn empty_control_response_is_a_transport_failure() {
        let error = validate_response_frame(Path::new("/session"), Duration::from_secs(1), &[])
            .expect_err("empty response must be a transport failure");
        assert!(matches!(
            error,
            Error::ControlTransport {
                phase: "read response",
                request_dispatched: true,
                source,
                ..
            } if source.kind() == ErrorKind::UnexpectedEof
        ));
    }
}
