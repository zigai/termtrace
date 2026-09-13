#![expect(
    dead_code,
    reason = "Legacy envelope functions preserved for test fixtures"
)]

use super::*;
use serde::{
    de::value::{self, StrDeserializer},
    Deserialize as _,
};
use std::collections::BTreeMap;

pub(super) fn typed_envelope(response: CliResponseEnvelope) -> serde_json::Value {
    return serde_json::to_value(response).unwrap_or_else(|error| {
        return response_encoding_failure(error.to_string());
    });
}

pub(super) fn response_encoding_failure(message: String) -> serde_json::Value {
    return serde_json::json!({
        "v": CLI_RESPONSE_VERSION,
        "ok": false,
        "type": "response_encoding",
        "code": "serialization_failed",
        "applied": Applied::None,
        "retry": Retry::SafeNewRequest,
        "session_state": SessionState::Unknown,
        "message": message,
    });
}

pub(super) fn applied_from_str(value: &str) -> Result<Applied, String> {
    return Applied::deserialize(StrDeserializer::<value::Error>::new(value))
        .map_err(|error| return format!("invalid applied value `{value}`: {error}"));
}

pub(super) fn retry_from_str(value: &str) -> Result<Retry, String> {
    return Retry::deserialize(StrDeserializer::<value::Error>::new(value))
        .map_err(|error| return format!("invalid retry value `{value}`: {error}"));
}

pub(super) fn session_state_from_str(value: &str) -> Result<SessionState, String> {
    return SessionState::deserialize(StrDeserializer::<value::Error>::new(value))
        .map_err(|error| return format!("invalid session state value `{value}`: {error}"));
}

pub(super) fn response_string_value(
    value: Option<&serde_json::Value>,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let serde_json::Value::String(value) = value else {
        return Err(format!("response field `{key}` must be a string"));
    };

    return Ok(Some(value.clone()));
}

fn take_string(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(value) = fields.remove(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let serde_json::Value::String(value) = value else {
        return Err(format!("response field `{key}` must be a string"));
    };

    return Ok(Some(value));
}

fn take_bool(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    let Some(value) = fields.remove(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let serde_json::Value::Bool(value) = value else {
        return Err(format!("response field `{key}` must be a boolean"));
    };

    return Ok(Some(value));
}

fn take_u64(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<u64>, String> {
    let Some(value) = fields.remove(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }

    let Some(value) = value.as_u64() else {
        return Err(format!(
            "response field `{key}` must be an unsigned integer"
        ));
    };

    return Ok(Some(value));
}

fn envelope_from_fields(
    ok: bool,
    response_type: String,
    mut fields: BTreeMap<String, serde_json::Value>,
) -> Result<CliResponseEnvelope, String> {
    fields.remove("v");
    fields.remove("ok");
    fields.remove("type");
    let code = take_string(&mut fields, "code")?;
    let applied = take_string(&mut fields, "applied")?
        .map(|value| return applied_from_str(&value))
        .transpose()?;
    let retry = take_string(&mut fields, "retry")?
        .map(|value| return retry_from_str(&value))
        .transpose()?;
    let session_state = take_string(&mut fields, "session_state")?
        .map(|value| return session_state_from_str(&value))
        .transpose()?;
    let request_id = fields
        .remove("request_id")
        .filter(|value| return !value.is_null());
    let message = take_string(&mut fields, "message")?;
    let truncated = take_bool(&mut fields, "truncated")?;
    let response_bytes = take_u64(&mut fields, "response_bytes")?;

    return Ok(CliResponseEnvelope {
        version: CLI_RESPONSE_VERSION,
        ok,
        response_type,
        code,
        applied,
        retry,
        session_state,
        request_id,
        message,
        truncated,
        response_bytes,
        fields,
    });
}

fn typed_envelope_from_fields(
    ok: bool,
    response_type: String,
    fields: BTreeMap<String, serde_json::Value>,
) -> serde_json::Value {
    return match envelope_from_fields(ok, response_type, fields) {
        Ok(response) => typed_envelope(response),
        Err(message) => response_encoding_failure(message),
    };
}

pub(super) fn compact_success(state: &ResponseState) -> serde_json::Value {
    let last = state.values.last();
    let kind = response_type(last, &state.command_type);
    let mut response = BTreeMap::new();

    if state.values.len() == 1 {
        if let Some(serde_json::Value::Object(value)) =
            state.values.first().cloned().and_then(|value| {
                return compact_task_value(value, &state.fields, &state.command_type);
            })
        {
            for (key, value) in value {
                response.entry(key).or_insert(value);
            }
        }
    } else if !state.values.is_empty() {
        let values = state
            .values
            .iter()
            .cloned()
            .filter_map(|value| {
                return compact_task_value(value, &state.fields, &state.command_type);
            })
            .collect::<Vec<_>>();
        response.insert("results".to_owned(), serde_json::Value::Array(values));
    }

    if !state.raw.is_empty() {
        if let Ok(text) = str::from_utf8(&state.raw) {
            response.insert(
                "output".to_owned(),
                serde_json::Value::String(text.to_owned()),
            );
        } else {
            response.insert(
                "output".to_owned(),
                serde_json::json!({
                    "encoding": "base64",
                    "data": STANDARD.encode(&state.raw),
                    "bytes": state.raw.len(),
                    "source_bytes": state.captured_bytes,
                    "truncated": state.omitted_items != 0,
                }),
            );
        }
    }

    if state.omitted_items != 0 {
        response.insert("truncated".to_owned(), serde_json::Value::Bool(true));
        response.insert(
            "omitted_items".to_owned(),
            serde_json::json!(state.omitted_items),
        );
    }

    if state.mutating {
        if state.command_type != "act" {
            let applied = nested_string(last, "applied").unwrap_or_else(|| return "all".to_owned());
            response.insert("applied".to_owned(), serde_json::Value::String(applied));
            response.insert(
                "retry".to_owned(),
                serde_json::Value::String("not_needed".to_owned()),
            );
            response.insert(
                "session_state".to_owned(),
                serde_json::Value::String(session_state(last, &state.command_type, true)),
            );
        }

        if let Some(request_id) = last
            .and_then(|value| return value.get("request_id"))
            .or_else(|| return last.and_then(|value| return value.get("sequence_id")))
            .cloned()
        {
            response.insert("request_id".to_owned(), request_id);
        }
    }

    if matches!(state.command_type.as_str(), "wait" | "status")
        && !response.contains_key("session_state")
    {
        response.insert(
            "session_state".to_owned(),
            serde_json::Value::String(session_state(last, &state.command_type, true)),
        );
    }

    return typed_envelope_from_fields(true, kind, response);
}

pub(super) fn retry_for_applied(applied: &str) -> &'static str {
    return match applied {
        "all" => "unsafe",
        "partial" => "inspect_before_retry",
        "unknown" => "same_request_id_only",
        _ => "safe_new_request",
    };
}

pub(super) fn compact_failure(
    state: &ResponseState,
    error: Option<&miette::Report>,
) -> serde_json::Value {
    let last = state.values.last();
    let response_type = response_type(last, &state.command_type);
    let mut response = BTreeMap::new();
    response.insert(
        "code".to_owned(),
        serde_json::Value::String(
            failure_string(last, "code").unwrap_or_else(|| return "operation_failed".to_owned()),
        ),
    );

    if let Some(serde_json::Value::Object(value)) = last.cloned().and_then(|value| {
        return compact_task_value(value, &state.fields, &state.command_type);
    }) {
        for (key, value) in value {
            response.entry(key).or_insert(value);
        }
    }

    if let Some(details) = last
        .and_then(|value| return value.get("details"))
        .and_then(serde_json::Value::as_object)
        && let Some(state_hash) = details.get("current_presented_state_hash").cloned()
    {
        let mut observation = serde_json::Map::new();
        observation.insert("state".to_owned(), state_hash);
        copy_value(details, &mut observation, "scope", "scope");
        response
            .entry("observation".to_owned())
            .or_insert_with(|| return serde_json::Value::Object(observation));
    }

    let applied = nested_string(last, "applied").unwrap_or_else(|| {
        return if state.mutating && last.is_some() {
            "unknown".to_owned()
        } else {
            "none".to_owned()
        };
    });

    let retry = nested_string(last, "retry")
        .unwrap_or_else(|| return retry_for_applied(&applied).to_owned());

    response.insert("applied".to_owned(), serde_json::Value::String(applied));
    response.insert("retry".to_owned(), serde_json::Value::String(retry));
    response.insert(
        "session_state".to_owned(),
        serde_json::Value::String(session_state(last, &state.command_type, false)),
    );

    if let Some(request_id) = last
        .and_then(|value| return value.get("request_id"))
        .or_else(|| return last.and_then(|value| return value.get("sequence_id")))
        .cloned()
    {
        response.insert("request_id".to_owned(), request_id);
    }

    if let Some(error) = error {
        response.insert(
            "message".to_owned(),
            serde_json::Value::String(error.to_string()),
        );
    } else if let Some(message) = failure_string(last, "message")
        .or_else(|| return failure_string(last, "error"))
        .or_else(|| return failure_string(last, "diagnostic"))
    {
        response.insert("message".to_owned(), serde_json::Value::String(message));
    }

    return typed_envelope_from_fields(false, response_type, response);
}
