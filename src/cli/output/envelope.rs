use super::*;

pub(super) fn typed_envelope(
    response: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    return serde_json::from_value::<CliResponseEnvelope>(serde_json::Value::Object(response))
        .and_then(serde_json::to_value)
        .unwrap_or_else(|error| {
            return serde_json::json!({
                "v": CLI_RESPONSE_VERSION,
                "ok": false,
                "type": "response_encoding",
                "code": "serialization_failed",
                "applied": Applied::None,
                "retry": Retry::SafeNewRequest,
                "session_state": SessionState::Unknown,
                "message": error.to_string(),
            });
        });
}

pub(super) fn compact_success(state: &ResponseState) -> serde_json::Value {
    let last = state.values.last();
    let kind = response_type(last, &state.command_type);
    let mut response = serde_json::Map::new();
    response.insert("v".to_owned(), serde_json::json!(CLI_RESPONSE_VERSION));
    response.insert("ok".to_owned(), serde_json::Value::Bool(true));
    response.insert("type".to_owned(), serde_json::Value::String(kind));
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
    return typed_envelope(response);
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
    let mut response = serde_json::Map::new();
    response.insert("v".to_owned(), serde_json::json!(CLI_RESPONSE_VERSION));
    response.insert("ok".to_owned(), serde_json::Value::Bool(false));
    response.insert(
        "type".to_owned(),
        serde_json::Value::String(response_type(last, &state.command_type)),
    );
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
    return typed_envelope(response);
}
