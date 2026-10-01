//! Bounded synchronous access to every installed process-driver method.

use nix::{
    errno::Errno,
    fcntl::{fcntl, FcntlArg, OFlag},
    poll::{poll, PollFd, PollFlags, PollTimeout},
};
use std::{
    error::Error as StdError,
    io::{self, Read as _, Write as _},
    os::fd::AsFd,
    path::Path,
    process::{ChildStdin, ChildStdout, Command, Stdio},
    result::Result as StdResult,
    time::{Duration, Instant},
};
use termtrace_model::protocol::{
    control::CONTROL_PROTOCOL_VERSION,
    driver::{
        DriverMethod, DriverPayload, DriverRequest, DriverResponse, DriverResult,
        DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES, DRIVER_PROTOCOL_VERSION,
    },
};
use termtrace_process::{poll_until, SupervisedChild};
use thiserror::Error;
use uuid::Uuid;

const CLOSE_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Failure of the installed process driver's framing or transport.
#[derive(Debug, Error)]
pub enum DriverError {
    /// The installed process driver violated its framing or correlation contract.
    #[error("invalid driver protocol message: {reason}")]
    InvalidProtocol {
        /// Bounded diagnostic without raw response contents.
        reason: String,
        /// Lower-level failure, when it cannot carry raw response contents.
        #[source]
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
    /// Process-driver I/O failed or exceeded its deadline.
    #[error("driver transport failed during `{phase}`: {source}")]
    Transport {
        /// Transport operation that failed.
        phase: &'static str,
        /// Whether any request bytes may have reached the driver.
        request_dispatched: bool,
        /// Underlying transport error, including `TimedOut` deadlines.
        #[source]
        source: io::Error,
    },
}

type Result<T, E = DriverError> = StdResult<T, E>;

/// Owns an installed `termtrace agent` process without linking the recording engine.
///
/// Calls are serialized through a mutable handle. Dropping or closing the driver
/// closes its input and lets it finalize sessions it owns; attached sessions keep
/// their independent recorder ownership. Use `close` to observe cleanup errors.
pub struct Driver {
    /// The driver's process group, until the driver is closed and reaped.
    child: Option<SupervisedChild>,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
}

impl Driver {
    /// Start the installed executable and validate its protocol handshake.
    ///
    /// # Errors
    ///
    /// Returns `DriverError::Transport` when the executable cannot be started or its pipes cannot
    /// be made nonblocking, and `DriverError::InvalidProtocol` when a pipe is missing or the
    /// handshake is invalid or incompatible.
    pub fn spawn(binary: impl AsRef<Path>) -> Result<Self> {
        let mut child = SupervisedChild::spawn(
            Command::new(binary.as_ref())
                .arg("agent")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit()),
        )
        .map_err(|source| return transport("spawn", false, source))?;
        let input = child.child_mut().stdin.take();
        let output = child.child_mut().stdout.take();
        let mut driver = Self {
            child: Some(child),
            input,
            output,
        };

        nonblocking(
            driver
                .input
                .as_ref()
                .ok_or_else(|| return protocol("missing driver input pipe"))?,
        )?;

        nonblocking(
            driver
                .output
                .as_ref()
                .ok_or_else(|| return protocol("missing driver output pipe"))?,
        )?;

        let response = driver.request(DriverMethod::Hello, Duration::from_secs(10))?;
        if !matches!(
            response.result,
            DriverResult::Ok {
                data: DriverPayload::Hello {
                    driver_protocol_version: DRIVER_PROTOCOL_VERSION,
                    control_protocol_version: CONTROL_PROTOCOL_VERSION,
                    ..
                }
            }
        ) {
            return Err(protocol("installed driver handshake is incompatible"));
        }
        return Ok(driver);
    }

    /// Send one method with a fresh correlation ID and a total I/O deadline.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub fn request(&mut self, method: DriverMethod, timeout: Duration) -> Result<DriverResponse> {
        return self.send(
            &DriverRequest {
                version: DRIVER_PROTOCOL_VERSION,
                request_id: Uuid::now_v7(),
                method,
            },
            timeout,
        );
    }

    /// Send a caller-owned request while retaining structured success or rejection.
    ///
    /// After uncertain delivery, retry only according to the returned driver or
    /// actor evidence; never generate a new ID merely because a timeout occurred.
    ///
    /// # Errors
    ///
    /// Returns `DriverError::InvalidProtocol` when the driver is closed, the request version is
    /// incompatible, the deadline overflows, the request exceeds the frame limit, or the response
    /// is malformed or uncorrelated, and `DriverError::Transport` when I/O fails or `timeout`
    /// elapses.
    pub fn send(&mut self, request: &DriverRequest, timeout: Duration) -> Result<DriverResponse> {
        if self.child.is_none() || request.version != DRIVER_PROTOCOL_VERSION {
            return Err(protocol(
                "driver is closed or request version is incompatible",
            ));
        }

        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| return protocol("driver timeout exceeds clock range"))?;

        let mut encoded = serde_json::to_vec(request).map_err(|source| {
            return protocol_with_source("could not encode driver request", source);
        })?;
        encoded.push(b'\n');
        if u64::try_from(encoded.len()).unwrap_or(u64::MAX) > DRIVER_MAX_REQUEST_BYTES {
            return Err(protocol("driver request exceeds frame limit"));
        }

        let result = self.exchange(&encoded, deadline).and_then(|bytes| {
            let response: DriverResponse =
                serde_json::from_slice(&bytes).map_err(undecodable_response)?;

            if response.version != DRIVER_PROTOCOL_VERSION
                || response.request_id != request.request_id
            {
                return Err(protocol("driver response version or request ID differs"));
            }
            return Ok(response);
        });

        if result.is_err() {
            // A late response must not be mistaken for the next request's result.
            let _cleanup = self.close(Duration::from_secs(5));
        }

        return result;
    }

    fn exchange(&mut self, encoded: &[u8], deadline: Instant) -> Result<Vec<u8>> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| return protocol("driver input is closed"))?;

        let mut sent = 0;
        while let Some(remaining) = encoded
            .get(sent..)
            .filter(|remaining| return !remaining.is_empty())
        {
            ready(
                input,
                PollFlags::POLLOUT,
                deadline,
                "write request",
                sent > 0,
            )?;

            match input.write(remaining) {
                Ok(0) => {
                    return Err(transport(
                        "write request",
                        sent > 0,
                        io::Error::from(io::ErrorKind::WriteZero),
                    ));
                }
                Ok(count) => sent += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(source) => return Err(transport("write request", sent > 0, source)),
            }
        }

        let output = self
            .output
            .as_mut()
            .ok_or_else(|| return protocol("driver output is closed"))?;

        let mut response = Vec::new();

        loop {
            ready(output, PollFlags::POLLIN, deadline, "read response", true)?;

            let mut chunk = [0_u8; 8192];

            match output.read(&mut chunk) {
                Ok(0) => {
                    return Err(transport(
                        "read response",
                        true,
                        io::Error::from(io::ErrorKind::UnexpectedEof),
                    ));
                }
                Ok(count) => {
                    let bytes = chunk
                        .get(..count)
                        .ok_or_else(|| return protocol("invalid driver read count"))?;
                    response.extend_from_slice(bytes);
                    if u64::try_from(response.len()).unwrap_or(u64::MAX) > DRIVER_MAX_RESPONSE_BYTES
                    {
                        return Err(protocol("driver response exceeds frame limit"));
                    }

                    if let Some(newline) = response.iter().position(|byte| return *byte == b'\n') {
                        if newline + 1 != response.len() {
                            return Err(protocol("driver sent unsolicited trailing response data"));
                        }
                        return Ok(response);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(source) => return Err(transport("read response", true, source)),
            }
        }
    }

    /// Close input, allow owned-session finalization, and reap the process.
    ///
    /// # Errors
    ///
    /// Returns `DriverError::Transport` when the process cannot be polled or does not exit within
    /// `timeout`, after which it is killed, and `DriverError::InvalidProtocol` when it exits
    /// unsuccessfully.
    pub fn close(&mut self, timeout: Duration) -> Result<()> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };

        self.input.take();

        let exited = match poll_until(Instant::now() + timeout, CLOSE_POLL_INTERVAL, || {
            return child.try_wait();
        }) {
            Ok(exited) => exited,
            Err(source) => {
                self.child = Some(child);

                return Err(transport("close", false, source));
            }
        };

        self.output.take();

        let Some(status) = exited else {
            let _status = child.kill();

            return Err(transport(
                "close",
                false,
                io::Error::from(io::ErrorKind::TimedOut),
            ));
        };
        if !status.success() {
            return Err(protocol("driver exited unsuccessfully during cleanup"));
        }
        return Ok(());
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _cleanup = self.close(Duration::from_secs(5));
    }
}

fn nonblocking(file: &impl AsFd) -> Result<()> {
    let flags = fcntl(file, FcntlArg::F_GETFL).map_err(|error| {
        return transport(
            "configure pipe",
            false,
            io::Error::from_raw_os_error(error as i32),
        );
    })?;
    fcntl(
        file,
        FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
    )
    .map_err(|error| {
        return transport(
            "configure pipe",
            false,
            io::Error::from_raw_os_error(error as i32),
        );
    })?;

    return Ok(());
}

fn ready(
    file: &impl AsFd,
    flags: PollFlags,
    deadline: Instant,
    phase: &'static str,
    dispatched: bool,
) -> Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|value| return !value.is_zero())
            .ok_or_else(|| {
                return transport(phase, dispatched, io::Error::from(io::ErrorKind::TimedOut));
            })?;

        let timeout =
            PollTimeout::try_from(remaining.min(Duration::from_secs(60))).map_err(|source| {
                return protocol_with_source("invalid driver poll duration", source);
            })?;

        let mut descriptors = [PollFd::new(file.as_fd(), flags)];

        match poll(&mut descriptors, timeout) {
            Ok(0) | Err(Errno::EINTR) => {}
            Ok(_) => return Ok(()),
            Err(error) => {
                return Err(transport(
                    phase,
                    dispatched,
                    io::Error::from_raw_os_error(error as i32),
                ));
            }
        }
    }
}

fn protocol(reason: &str) -> DriverError {
    return DriverError::InvalidProtocol {
        reason: reason.to_owned(),
        source: None,
    };
}

fn protocol_with_source(
    reason: &str,
    source: impl Into<Box<dyn StdError + Send + Sync>>,
) -> DriverError {
    return DriverError::InvalidProtocol {
        reason: reason.to_owned(),
        source: Some(source.into()),
    };
}

/// Reject a driver response that does not decode as the response schema.
///
/// `InvalidProtocol` promises a bounded diagnostic without raw response contents, and decoder
/// errors can quote response values, so the decoder's detail is deliberately dropped here.
fn undecodable_response(_: serde_json::Error) -> DriverError {
    return protocol("driver response failed schema validation");
}

fn transport(phase: &'static str, request_dispatched: bool, source: io::Error) -> DriverError {
    return DriverError::Transport {
        phase,
        request_dispatched,
        source,
    };
}
