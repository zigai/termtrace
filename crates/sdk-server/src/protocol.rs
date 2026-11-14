use crate::failure::Failure;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

pub(crate) const PARSE_ERROR: i64 = -32_700;
pub(crate) const INVALID_REQUEST: i64 = -32_600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32_601;
pub(crate) const INVALID_PARAMS: i64 = -32_602;
pub(crate) const INTERNAL_ERROR: i64 = -32_603;
pub(crate) const NOT_INITIALIZED: i64 = -32_002;
pub(crate) const TERMTRACE_ERROR: i64 = -32_000;
pub(crate) const CANCEL: &str = "$/cancel";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Box<RawValue>>,
}

#[derive(Serialize)]
struct Response<'line> {
    jsonrpc: &'static str,
    id: &'line Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'line RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'line Failure>,
}

pub(crate) enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Box<RawValue>,
    },
    Notification {
        method: String,
        params: Box<RawValue>,
    },
}

pub(crate) fn parse(line: &str) -> Result<Incoming, (Value, Failure)> {
    // Params stay raw text, so the envelope and its id parse before any deep params are decoded,
    // and integers past the u64 range stay exact.
    let envelope: Envelope = serde_json::from_str(line).map_err(|error| {
        return serde_json::from_str::<Value>(line).map_or_else(
            |_| {
                return (
                    Value::Null,
                    Failure::protocol(PARSE_ERROR, format!("invalid JSON: {error}")),
                );
            },
            |value| {
                return (
                    value.get("id").cloned().unwrap_or(Value::Null),
                    Failure::protocol(INVALID_REQUEST, format!("invalid request: {error}")),
                );
            },
        );
    })?;

    let id = envelope.id.clone().unwrap_or(Value::Null);

    if envelope.jsonrpc != "2.0" {
        return Err((
            id,
            Failure::protocol(INVALID_REQUEST, "jsonrpc must be \"2.0\"".to_owned()),
        ));
    }

    let params = match envelope.params {
        Some(params) => params,
        None => RawValue::from_string("{}".to_owned())
            .map_err(|error| return (id.clone(), Failure::internal(error.to_string())))?,
    };

    return Ok(match envelope.id {
        Some(id @ (Value::Number(_) | Value::String(_))) => Incoming::Request {
            id,
            method: envelope.method,
            params,
        },
        None => Incoming::Notification {
            method: envelope.method,
            params,
        },
        Some(id) => {
            return Err((
                id,
                Failure::protocol(INVALID_REQUEST, "id must be a number or string".to_owned()),
            ));
        }
    });
}

/// Shared line writer; one response is one line, written and flushed under the lock.
#[derive(Clone)]
pub(crate) struct Responder {
    output: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Responder {
    pub(crate) fn new(output: impl Write + Send + 'static) -> Self {
        return Self {
            output: Arc::new(Mutex::new(Box::new(output))),
        };
    }

    pub(crate) fn respond(
        &self,
        id: &Value,
        outcome: Result<Box<RawValue>, Failure>,
    ) -> io::Result<()> {
        let (result, error) = match &outcome {
            Ok(result) => (Some(&**result), None),
            Err(failure) => (None, Some(failure)),
        };

        let message = Response {
            jsonrpc: "2.0",
            id,
            result,
            error,
        };
        let mut line = serde_json::to_vec(&message).map_err(io::Error::other)?;
        line.push(b'\n');

        let mut output = self
            .output
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner());
        output.write_all(&line)?;
        return output.flush();
    }
}
