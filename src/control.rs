//! Versioned, bounded client protocol for controlling a live recording.

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    event::{ByteString, InputKind},
    event_log::CanonicalBoundary,
};

/// Current control protocol version.
pub const CONTROL_PROTOCOL_VERSION: u32 = 4;
/// Relative socket path inside a live session directory.
pub const CONTROL_SOCKET_NAME: &str = "control.sock";
/// Maximum encoded request or response size.
pub const MAX_CONTROL_MESSAGE_BYTES: u64 = 1024 * 1024;

/// Safely supported process-group signals.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSignal {
    /// Interrupt the foreground application (`SIGINT`).
    Interrupt,
    /// Request graceful termination (`SIGTERM`).
    Terminate,
    /// Notify the application that its terminal disconnected (`SIGHUP`).
    Hangup,
    /// Application-defined signal one (`SIGUSR1`).
    User1,
    /// Application-defined signal two (`SIGUSR2`).
    User2,
}

/// One command sent to the live session actor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    /// Verify that the host and protocol are responsive.
    Ping,
    /// Return current process and event counters.
    Status,
    /// Send exact bytes to the child PTY.
    Input {
        /// Raw bytes encoded as Base64 in JSON.
        data: ByteString,
        /// Semantic category retained in the canonical input intent.
        input_kind: InputKind,
        /// Optional human-readable intent.
        intent: Option<String>,
    },
    /// Change the child PTY geometry.
    Resize {
        /// Terminal columns.
        cols: u16,
        /// Terminal rows.
        rows: u16,
        /// Total pixel width.
        pixel_width: u16,
        /// Total pixel height.
        pixel_height: u16,
    },
    /// Send one allow-listed signal to the child process group.
    Signal {
        /// Signal to send.
        signal: ProcessSignal,
    },
    /// Add a named point to the canonical event stream.
    Mark {
        /// Non-empty marker label.
        label: String,
    },
    /// Request graceful child termination.
    Stop,
}

/// Request envelope carried as one JSON line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    /// Protocol version.
    pub version: u32,
    /// Caller-generated correlation identifier.
    pub request_id: Uuid,
    /// Requested operation.
    pub command: ControlCommand,
}

impl ControlRequest {
    /// Construct a request with a fresh time-sortable identifier.
    pub fn new(command: ControlCommand) -> Self {
        return Self {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: Uuid::now_v7(),
            command,
        };
    }
}

/// Successful response payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlData {
    /// No additional data is required.
    Acknowledged {
        /// Durable and observed canonical positions after the operation.
        boundaries: ActorBoundaries,
    },
    /// Live session status.
    Status {
        /// Child process identifier when available.
        pid: Option<u32>,
        /// Durable and observed canonical positions at status evaluation.
        boundaries: ActorBoundaries,
    },
    /// Canonical position of an appended marker.
    Marked {
        /// Normalized marker label.
        label: String,
        /// Canonical event sequence containing the marker.
        event_seq: u64,
        /// Durable and observed canonical positions after the marker.
        boundaries: ActorBoundaries,
    },
}

/// Durable and latest actor-processed positions returned by live operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorBoundaries {
    /// Latest boundary whose file synchronization completed successfully.
    pub durable: CanonicalBoundary,
    /// Latest boundary processed by the session actor, possibly volatile.
    pub observed: CanonicalBoundary,
}

/// Response envelope carried as one JSON line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlResponse {
    /// Protocol version.
    pub version: u32,
    /// Identifier copied from the request.
    pub request_id: Uuid,
    /// Success or structured rejection.
    pub result: ControlResult,
}

/// Successful response or stable rejection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlResult {
    /// Command was accepted and completely applied.
    Ok {
        /// Command-specific data.
        data: ControlData,
    },
    /// Command was rejected or failed.
    Error {
        /// Stable machine-readable code.
        code: String,
        /// Human-readable diagnostic.
        message: String,
    },
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
    let socket = session_directory.as_ref().join(CONTROL_SOCKET_NAME);
    let mut stream = UnixStream::connect(&socket).map_err(|source| {
        return Error::Filesystem {
            path: socket.clone(),
            source,
        };
    })?;
    let timeout = Some(Duration::from_secs(10));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| return stream.set_write_timeout(timeout))
        .map_err(|source| {
            return Error::Filesystem {
                path: socket.clone(),
                source,
            };
        })?;
    serde_json::to_writer(&mut stream, request).map_err(|source| {
        return Error::InvalidJson {
            path: socket.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    stream.write_all(b"\n").map_err(|source| {
        return Error::Filesystem {
            path: socket.clone(),
            source,
        };
    })?;
    stream.shutdown(Shutdown::Write).map_err(|source| {
        return Error::Filesystem {
            path: socket.clone(),
            source,
        };
    })?;

    let mut encoded = Vec::new();
    BufReader::new(stream)
        .take(MAX_CONTROL_MESSAGE_BYTES + 1)
        .read_until(b'\n', &mut encoded)
        .map_err(|source| {
            return Error::Filesystem {
                path: socket.clone(),
                source,
            };
        })?;
    validate_frame(&encoded)?;
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
    match response.result {
        ControlResult::Ok { data } => return Ok(data),
        ControlResult::Error { code, message } => {
            return Err(Error::ControlRejected { code, message });
        }
    }
}

pub(crate) fn validate_frame(encoded: &[u8]) -> Result<()> {
    if encoded.is_empty()
        || encoded.last() != Some(&b'\n')
        || u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "message must be one newline-terminated JSON object no larger than 1 MiB"
                .to_owned(),
        });
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unterminated_and_oversized_frames() {
        assert!(validate_frame(br#"{"version":1}"#).is_err());
        let mut oversized = vec![b'x'; usize::try_from(MAX_CONTROL_MESSAGE_BYTES).unwrap()];
        oversized.push(b'\n');
        assert!(validate_frame(&oversized).is_err());
    }

    #[test]
    fn request_schema_rejects_unknown_fields() {
        let encoded = format!(
            r#"{{"version":1,"request_id":"{}","command":"ping","extra":true}}"#,
            Uuid::now_v7()
        );
        assert!(serde_json::from_str::<ControlRequest>(&encoded).is_err());
    }
}
