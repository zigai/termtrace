use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, Error as IoError, Result as IoResult, Write},
    path::PathBuf,
    process::Child,
    result::Result as StdResult,
};

use serde::Serialize;
use termtrace_model::protocol::control::ControlData;
use termtrace_sdk::{Error, Result};
use uuid::Uuid;

use super::{
    framing::{DriverFrame, read_frame},
    operations::{driver_error, driver_failure_with_retry, driver_response_is_cacheable},
    protocol::{
        DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES, DRIVER_PROTOCOL_VERSION,
        DRIVER_RESPONSE_CACHE_BYTES, DriverData, DriverRequest, DriverResponse, DriverResult,
    },
};

/// Serve requests until clean input EOF.
pub(in crate::cli) fn serve(mut input: impl BufRead, mut output: impl Write) -> Result<()> {
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
            DriverFrame::TooLarge => driver_error(
                Uuid::nil(),
                "request_too_large",
                format!("driver request exceeds the {DRIVER_MAX_REQUEST_BYTES}-byte frame limit"),
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

#[derive(Default)]
pub(super) struct Driver {
    pub(super) sessions: HashMap<Uuid, PathBuf>,
    pub(super) launched: HashSet<Uuid>,
    pub(super) recorders: HashMap<Uuid, Child>,
    pub(super) cached: HashMap<Uuid, CachedDriverResponse>,
    order: VecDeque<Uuid>,
    pub(super) cached_bytes: usize,
}

pub(super) struct CachedDriverResponse {
    method_hash: String,
    response: DriverResponse,
    encoded_bytes: usize,
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
        let response = bounded_response(response);
        if driver_response_is_cacheable(&response)
            && let Ok(encoded_bytes) = serialized_size(&response)
        {
            self.cache(
                request.request_id,
                method_hash,
                response.clone(),
                encoded_bytes,
            );
        }
        return response;
    }

    pub(super) fn cache(
        &mut self,
        request_id: Uuid,
        method_hash: String,
        response: DriverResponse,
        encoded_bytes: usize,
    ) {
        self.cached_bytes = self.cached_bytes.saturating_add(encoded_bytes);
        self.cached.insert(
            request_id,
            CachedDriverResponse {
                method_hash,
                response,
                encoded_bytes,
            },
        );
        self.order.push_back(request_id);
        while self.cached_bytes > DRIVER_RESPONSE_CACHE_BYTES {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some(response) = self.cached.remove(&evicted) {
                self.cached_bytes = self.cached_bytes.saturating_sub(response.encoded_bytes);
            }
        }
    }
}

pub(super) fn bounded_response(response: DriverResponse) -> DriverResponse {
    let Ok(encoded_bytes) = serialized_size(&response) else {
        return driver_error(
            response.request_id,
            "response_encoding_failed",
            "driver response could not be encoded",
            None,
        );
    };
    if u64::try_from(encoded_bytes).unwrap_or(u64::MAX) <= DRIVER_MAX_RESPONSE_BYTES {
        return response;
    }
    let rich_screen_response = matches!(
        &response.result,
        DriverResult::Ok {
            data: DriverData::ScreenFrame { .. }
                | DriverData::Control {
                    data: ControlData::ScreenFrame { .. }
                        | ControlData::StableSnapshot { .. }
                        | ControlData::ScreenWait { .. }
                        | ControlData::ActionApplied { .. },
                },
        }
    );
    return DriverResponse {
        version: DRIVER_PROTOCOL_VERSION,
        request_id: response.request_id,
        result: *driver_failure_with_retry(
            if rich_screen_response {
                "screen_frame_resource_limit"
            } else {
                "response_too_large"
            },
            format!(
                "driver response exceeds the {DRIVER_MAX_RESPONSE_BYTES}-byte limit; request a smaller projection or page"
            ),
            None,
            "safe_same_request_id",
            false,
            None,
        ),
    };
}

pub(super) fn serialized_size(value: &impl Serialize) -> StdResult<usize, serde_json::Error> {
    let mut writer = CountingWriter::default();
    serde_json::to_writer(&mut writer, value)?;
    return Ok(writer.bytes);
}

#[derive(Default)]
struct CountingWriter {
    bytes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.bytes = self
            .bytes
            .checked_add(buf.len())
            .ok_or_else(|| return IoError::other("serialized driver response size overflow"))?;
        return Ok(buf.len());
    }

    fn flush(&mut self) -> IoResult<()> {
        return Ok(());
    }
}
