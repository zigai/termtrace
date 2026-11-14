use crate::failure::Failure;
use serde::Deserialize;
use serde_json::{json, Map, Value};
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
    params: Option<Value>,
}

pub(crate) enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

pub(crate) fn parse(line: &str) -> Result<Incoming, (Value, Failure)> {
    let value: Value = serde_json::from_str(line).map_err(|error| {
        return (
            Value::Null,
            Failure::protocol(PARSE_ERROR, format!("invalid JSON: {error}")),
        );
    })?;

    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let envelope: Envelope = serde_json::from_value(value).map_err(|error| {
        return (
            id.clone(),
            Failure::protocol(INVALID_REQUEST, format!("invalid request: {error}")),
        );
    })?;

    if envelope.jsonrpc != "2.0" {
        return Err((
            id,
            Failure::protocol(INVALID_REQUEST, "jsonrpc must be \"2.0\"".to_owned()),
        ));
    }

    let params = envelope
        .params
        .unwrap_or_else(|| return Value::Object(Map::new()));

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

    pub(crate) fn respond(&self, id: &Value, outcome: Result<Value, Failure>) -> io::Result<()> {
        let message = match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(failure) => json!({"jsonrpc": "2.0", "id": id, "error": failure.to_object()}),
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
