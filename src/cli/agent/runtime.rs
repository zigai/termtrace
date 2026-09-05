use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufWriter, Write},
    path::PathBuf,
    process::Child,
};

#[cfg(test)]
use std::result::Result as StdResult;

#[cfg(test)]
use serde::Serialize;
use termtrace_model::protocol::control::ControlData;
use termtrace_sdk::{Error, Result};
use uuid::Uuid;

use super::{
    framing::{DriverFrame, read_frame},
    operations::{driver_error, driver_failure_with_retry, driver_response_is_cacheable},
};
use termtrace_model::protocol::driver::{
    DRIVER_MAX_REQUEST_BYTES, DRIVER_MAX_RESPONSE_BYTES, DRIVER_PROTOCOL_VERSION, DriverData,
    DriverRequest, DriverResponse, DriverResult,
};

pub(super) const DRIVER_RESPONSE_CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Serve requests until clean input EOF.
pub(in crate::cli) fn serve(mut input: impl BufRead, output: impl Write) -> Result<()> {
    let mut output = BufWriter::new(output);
    let mut driver = Driver::default();
    while let Some(frame) = read_frame(&mut input)? {
        let (_response, encoded) = match frame {
            DriverFrame::Request(request) => driver.handle_request(request),
            DriverFrame::Invalid { line, column } => encode_driver_response(&driver_error(
                Uuid::nil(),
                "invalid_request",
                format!(
                    "driver request does not match the protocol schema at line {line} column {column}"
                ),
                None,
            ))?,
            DriverFrame::TooLarge => encode_driver_response(&driver_error(
                Uuid::nil(),
                "request_too_large",
                format!("driver request exceeds the {DRIVER_MAX_REQUEST_BYTES}-byte frame limit"),
                None,
            ))?,
            DriverFrame::Unterminated => encode_driver_response(&driver_error(
                Uuid::nil(),
                "invalid_request",
                "driver request is not newline terminated",
                None,
            ))?,
        };
        output
            .write_all(&encoded)
            .and_then(|()| return output.write_all(b"\n"))
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

fn encode_driver_response(response: &DriverResponse) -> Result<(DriverResponse, Vec<u8>)> {
    let bytes = serde_json::to_vec(response).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<driver-stdout>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok((response.clone(), bytes));
}

#[derive(Debug)]
pub(super) enum SessionOwnership {
    External,
    Launched { recorder: Option<Child> },
}

pub(super) struct AttachedSession {
    pub(super) path: PathBuf,
    pub(super) ownership: SessionOwnership,
}

#[derive(Default)]
pub(super) struct Driver {
    pub(super) sessions: HashMap<Uuid, AttachedSession>,
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
    fn handle_request(&mut self, request: DriverRequest) -> (DriverResponse, Vec<u8>) {
        let method_hash = match serde_json::to_vec(&request.method) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(_) => {
                let err = driver_error(
                    request.request_id,
                    "invalid_request",
                    "driver request method could not be encoded",
                    None,
                );
                let bytes = serde_json::to_vec(&err).unwrap_or_default();
                return (err, bytes);
            }
        };
        if let Some(cached) = self.cached.get(&request.request_id) {
            if cached.method_hash == method_hash {
                let bytes = serde_json::to_vec(&cached.response).unwrap_or_default();
                return (cached.response.clone(), bytes);
            }
            let err = driver_error(
                request.request_id,
                "request_id_collision",
                "driver request identifier was already used for a different method",
                None,
            );
            let bytes = serde_json::to_vec(&err).unwrap_or_default();
            return (err, bytes);
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
        let (response, encoded) = bounded_response_with_bytes(response);
        if driver_response_is_cacheable(&response) {
            self.cache(
                request.request_id,
                method_hash,
                response.clone(),
                encoded.len(),
            );
        }
        return (response, encoded);
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

pub(super) fn bounded_response_with_bytes(response: DriverResponse) -> (DriverResponse, Vec<u8>) {
    let bytes = match serde_json::to_vec(&response) {
        Ok(bytes) => bytes,
        Err(_) => {
            let err = driver_error(
                response.request_id,
                "response_encoding_failed",
                "driver response could not be encoded",
                None,
            );
            let err_bytes = serde_json::to_vec(&err).unwrap_or_default();
            return (err, err_bytes);
        }
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= DRIVER_MAX_RESPONSE_BYTES {
        return (response, bytes);
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
    let fallback = DriverResponse {
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
    let fallback_bytes = serde_json::to_vec(&fallback).unwrap_or_default();
    return (fallback, fallback_bytes);
}

#[cfg(test)]
pub(super) fn bounded_response(response: DriverResponse) -> DriverResponse {
    return bounded_response_with_bytes(response).0;
}

#[cfg(test)]
pub(super) fn serialized_size(value: &impl Serialize) -> StdResult<usize, serde_json::Error> {
    return serde_json::to_vec(value).map(|bytes| return bytes.len());
}
