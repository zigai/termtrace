use std::collections::BTreeMap;

use super::*;

pub(super) fn append_bounded_text(output: &mut String, value: &str) -> bool {
    let remaining = MAX_COMPACT_SCREEN_BYTES.saturating_sub(output.len());
    if value.len() <= remaining {
        output.push_str(value);
        return false;
    }
    let end = value.floor_char_boundary(remaining);
    output.push_str(value.get(..end).unwrap_or_default());
    return true;
}

pub(super) fn bounded_screen_frame_text(
    frame: &serde_json::Map<String, serde_json::Value>,
) -> Option<(String, bool)> {
    if let Some(viewport_text) = frame
        .get("viewport_text")
        .and_then(serde_json::Value::as_str)
    {
        let mut output = String::new();
        let truncated = append_bounded_text(&mut output, viewport_text);
        return Some((output, truncated));
    }
    let rows = frame
        .get("data")
        .and_then(serde_json::Value::as_object)?
        .get("rows")
        .and_then(serde_json::Value::as_array)?;
    let mut output = String::new();
    for (index, row) in rows.iter().enumerate() {
        let text = row
            .as_object()
            .and_then(|row| return row.get("text"))
            .and_then(serde_json::Value::as_str)?;
        if index != 0 && append_bounded_text(&mut output, "\n") {
            return Some((output, true));
        }
        if append_bounded_text(&mut output, text) {
            return Some((output, true));
        }
    }
    return Some((output, false));
}

pub(super) fn bounded_screen_frame_projection(
    frame: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut projection = serde_json::Map::new();
    for key in [
        "version",
        "state_hash",
        "frame_hash",
        "presented",
        "capture_quality",
    ] {
        copy_value(frame, &mut projection, key, key);
    }
    copy_value(frame, &mut projection, "geometry", "geometry");
    if let Some((screen, truncated)) = bounded_screen_frame_text(frame) {
        projection.insert(
            "viewport_text".to_owned(),
            serde_json::Value::String(screen),
        );
        if truncated {
            projection.insert("screen_truncated".to_owned(), serde_json::Value::Bool(true));
        }
    }
    return serde_json::Value::Object(projection);
}

pub(super) fn bounded_capture_projection(value: &serde_json::Value) -> Option<serde_json::Value> {
    const MAX_METADATA_STRING_BYTES: usize = 4 * 1_024;

    fn is_bounded_scalar(value: &serde_json::Value) -> bool {
        return match value {
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
                true
            }
            serde_json::Value::String(value) => value.len() <= MAX_METADATA_STRING_BYTES,
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => false,
        };
    }

    fn scalar_fields(
        source: &serde_json::Map<String, serde_json::Value>,
    ) -> serde_json::Map<String, serde_json::Value> {
        return source
            .iter()
            .filter(|(_, value)| return is_bounded_scalar(value))
            .map(|(key, value)| return (key.clone(), value.clone()))
            .collect();
    }

    let source = value.as_object()?;
    let mut projection = pagination_projection(value)
        .and_then(|value| return value.as_object().cloned())
        .unwrap_or_default();
    projection.extend(scalar_fields(source));
    for key in [
        "record",
        "boundary",
        "completion",
        "input",
        "observation",
        "postcondition",
        "predicate",
        "frame",
    ] {
        let Some(nested) = source.get(key).and_then(serde_json::Value::as_object) else {
            continue;
        };
        if key == "frame"
            || (key == "observation"
                && nested.contains_key("state_hash")
                && nested.contains_key("data"))
        {
            projection.insert(key.to_owned(), bounded_screen_frame_projection(nested));
            continue;
        }
        let nested = scalar_fields(nested);
        if !nested.is_empty() {
            projection.insert(key.to_owned(), serde_json::Value::Object(nested));
        }
    }
    return (!projection.is_empty()).then_some(serde_json::Value::Object(projection));
}

pub(super) fn selected(fields: &[ResponseField], field: ResponseField) -> bool {
    return fields.contains(&field);
}

pub(super) fn compact_value_with_paths(
    value: serde_json::Value,
    fields: &[ResponseField],
    preserve_canonical_paths: bool,
) -> Option<serde_json::Value> {
    return match value {
        serde_json::Value::Null => None,
        serde_json::Value::Array(values) => {
            let values = values
                .into_iter()
                .filter_map(|value| {
                    return compact_value_with_paths(value, fields, preserve_canonical_paths);
                })
                .collect::<Vec<_>>();
            (!values.is_empty()).then_some(serde_json::Value::Array(values))
        }
        serde_json::Value::Object(values) => {
            let mut compact = serde_json::Map::new();
            for (key, value) in values {
                let keep = match key.as_str() {
                    "details"
                    | "path"
                    | "stdout_log"
                    | "stderr_log"
                    | "pid"
                    | "recorder_pid"
                    | "recorder_process_start_ticks"
                    | "ready" => false,
                    "canonical_session" | "canonical_session_display" | "session_display" => {
                        preserve_canonical_paths
                    }
                    "cursor" => selected(fields, ResponseField::Cursor),
                    "modes" | "terminal_modes" => selected(fields, ResponseField::Modes),
                    "boundary"
                    | "boundaries"
                    | "durability"
                    | "durable_boundary"
                    | "observed_boundary"
                    | "event_hash"
                    | "next_event_seq"
                    | "output_bytes"
                    | "first_intent_event_seq"
                    | "last_result_event_seq" => selected(fields, ResponseField::Boundary),
                    "first_observed_monotonic_ns"
                    | "observed_monotonic_ns"
                    | "stable_for_ns"
                    | "pace_ns"
                    | "timing"
                    | "timestamps" => selected(fields, ResponseField::Timing),
                    _ => true,
                };
                if !keep {
                    continue;
                }
                if let Some(value) =
                    compact_value_with_paths(value, fields, preserve_canonical_paths)
                {
                    compact.insert(key, value);
                }
            }
            (!compact.is_empty()).then_some(serde_json::Value::Object(compact))
        }
        value => Some(value),
    };
}

pub(super) fn value_string(value: &serde_json::Value, key: &str) -> Option<String> {
    return value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
}

pub(super) fn response_type(value: Option<&serde_json::Value>, fallback: &str) -> String {
    return value
        .and_then(|value| {
            return value_string(value, "kind").or_else(|| return value_string(value, "type"));
        })
        .unwrap_or_else(|| return fallback.replace('_', "-"));
}

pub(super) fn nested_string(value: Option<&serde_json::Value>, key: &str) -> Option<String> {
    return value.and_then(|value| {
        return value_string(value, key).or_else(|| {
            return value
                .get("input")
                .and_then(|input| return value_string(input, key));
        });
    });
}
pub(super) fn failure_string(value: Option<&serde_json::Value>, key: &str) -> Option<String> {
    return nested_string(value, key)
        .or_else(|| {
            return value
                .and_then(|value| return value.get("completion"))
                .and_then(|completion| return completion.get("failure"))
                .and_then(|failure| return value_string(failure, key));
        })
        .or_else(|| {
            return value
                .and_then(|value| return value.get("postcondition"))
                .and_then(|postcondition| return value_string(postcondition, key));
        });
}

pub(super) fn session_state(
    value: Option<&serde_json::Value>,
    command_type: &str,
    success: bool,
) -> String {
    if let Some(lifecycle) = value.and_then(|value| return value_string(value, "lifecycle")) {
        return match lifecycle.as_str() {
            "live" => "live",
            "ended" => "ended",
            "completed" => "completed",
            "pending" => "pending",
            _ => "unknown",
        }
        .to_owned();
    }
    if let Some(session_state) = value.and_then(|value| return value_string(value, "session_state"))
    {
        return match session_state.as_str() {
            "live" => "live",
            "ended" => "ended",
            "completed" => "completed",
            "pending" => "pending",
            _ => "unknown",
        }
        .to_owned();
    }
    if let Some(startup_state) = value.and_then(|value| return value_string(value, "startup_state"))
    {
        return if startup_state == "live" {
            "live".to_owned()
        } else {
            "ended".to_owned()
        };
    }
    if value.is_some_and(|value| {
        return response_type(Some(value), command_type) == "completed"
            || value.get("completion").is_some();
    }) {
        return "ended".to_owned();
    }
    if let Some(ended) = value
        .and_then(|value| return value.get("ended"))
        .and_then(serde_json::Value::as_bool)
    {
        return if ended { "ended" } else { "live" }.to_owned();
    }
    if value
        .and_then(|value| return value_string(value, "quality"))
        .is_some_and(|quality| return quality == "completed")
    {
        return "ended".to_owned();
    }
    if success && matches!(command_type, "start" | "restart") {
        return "live".to_owned();
    }
    if success && command_type == "stop" {
        return "ended".to_owned();
    }
    if success && matches!(command_type, "send" | "act") {
        return "live".to_owned();
    }
    return "unknown".to_owned();
}

pub(super) fn copy_value(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    source_key: &str,
    target_key: &str,
) {
    if let Some(value) = source.get(source_key).cloned() {
        target.insert(target_key.to_owned(), value);
    }
}

pub(super) fn compact_postcondition(
    value: &serde_json::Value,
    include_screen: bool,
) -> Option<serde_json::Value> {
    let source = value.as_object()?;
    let mut postcondition = serde_json::Map::new();
    for key in [
        "matched",
        "state",
        "kind",
        "quality",
        "outcome",
        "unchanged",
        "ended",
        "code",
        "message",
        "error",
    ] {
        copy_value(source, &mut postcondition, key, key);
    }
    copy_value(source, &mut postcondition, "state_hash", "state");
    if include_screen {
        copy_value(source, &mut postcondition, "viewport_text", "screen");
        copy_value(source, &mut postcondition, "screen", "screen");
    }
    copy_value(source, &mut postcondition, "geometry", "size");
    copy_value(source, &mut postcondition, "size", "size");
    return (!postcondition.is_empty()).then_some(serde_json::Value::Object(postcondition));
}

pub(super) fn compact_completion(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    include_boundary: bool,
) {
    copy_value(source, target, "exit_code", "exit_code");
    copy_value(source, target, "signal_name", "signal");
    if let Some(failure) = source.get("failure").and_then(serde_json::Value::as_object) {
        copy_value(failure, target, "code", "code");
        copy_value(failure, target, "message", "message");
    }
    if !include_boundary {
        return;
    }
    let mut boundary = serde_json::Map::new();
    copy_value(source, &mut boundary, "event_count", "next_event_seq");
    copy_value(source, &mut boundary, "final_event_hash", "event_hash");
    copy_value(source, &mut boundary, "output_bytes", "output_bytes");
    if !boundary.is_empty() {
        target.insert("boundary".to_owned(), serde_json::Value::Object(boundary));
    }
}

pub(super) fn compact_task_value(
    value: serde_json::Value,
    fields: &[ResponseField],
    command_type: &str,
) -> Option<serde_json::Value> {
    if !matches!(
        command_type,
        "start" | "send" | "act" | "wait" | "status" | "stop"
    ) {
        return compact_value_with_paths(value, fields, command_type == "test");
    }
    let source = value.as_object()?;
    let copy_compacted = |target: &mut serde_json::Map<String, serde_json::Value>,
                          source_key: &str,
                          target_key: &str| {
        if let Some(value) = source.get(source_key).cloned().and_then(|value| {
            return compact_value_with_paths(value, fields, false);
        }) {
            target.insert(target_key.to_owned(), value);
        }
    };
    let mut projected = serde_json::Map::new();
    match command_type {
        "start" => {
            copy_value(source, &mut projected, "alias", "session");
            copy_value(source, &mut projected, "session_id", "session_id");
            copy_value(source, &mut projected, "startup_state", "startup_state");
            copy_value(source, &mut projected, "canonical_session_display", "path");
            if let Some(completion) = source
                .get("completion")
                .and_then(serde_json::Value::as_object)
            {
                compact_completion(
                    completion,
                    &mut projected,
                    selected(fields, ResponseField::Boundary),
                );
            }
            if let Some(postcondition) = source
                .get("postcondition")
                .and_then(|value| return compact_postcondition(value, false))
            {
                projected.insert("postcondition".to_owned(), postcondition);
            }
        }
        "send" => {
            copy_value(source, &mut projected, "atom_count", "atoms");
            copy_value(source, &mut projected, "bytes_written", "bytes");
            copy_value(source, &mut projected, "input_kind", "input");
            copy_value(source, &mut projected, "reason", "reason");
            copy_compacted(&mut projected, "atoms", "outcomes");
        }
        "act" => {
            if let Some(input) = source.get("input").and_then(serde_json::Value::as_object) {
                let mut summary = serde_json::Map::new();
                for key in ["applied", "atoms", "bytes"] {
                    if let Some(value) = input.get(key).cloned().and_then(|value| {
                        return compact_value_with_paths(value, fields, false);
                    }) {
                        summary.insert(key.to_owned(), value);
                    }
                }
                projected.insert("input".to_owned(), serde_json::Value::Object(summary));
            }
            if let Some(observation) = source
                .get("observation")
                .and_then(serde_json::Value::as_object)
            {
                let mut summary = serde_json::Map::new();
                copy_value(observation, &mut summary, "state", "state");
                copy_value(observation, &mut summary, "state_hash", "state");
                copy_value(observation, &mut summary, "quality", "quality");
                copy_value(observation, &mut summary, "capture_quality", "quality");
                copy_value(observation, &mut summary, "scope", "scope");
                copy_value(observation, &mut summary, "screen", "screen");
                if let Some((screen, truncated)) = bounded_screen_frame_text(observation) {
                    summary.insert("screen".to_owned(), serde_json::Value::String(screen));
                    if truncated {
                        summary
                            .insert("screen_truncated".to_owned(), serde_json::Value::Bool(true));
                    }
                }
                projected.insert("observation".to_owned(), serde_json::Value::Object(summary));
            }
            if source.get("ended").and_then(serde_json::Value::as_bool) == Some(true) {
                projected.insert("ended".to_owned(), serde_json::Value::Bool(true));
            }
            if source
                .get("postcondition")
                .and_then(serde_json::Value::as_object)
                .and_then(|postcondition| return postcondition.get("matched"))
                .and_then(serde_json::Value::as_bool)
                == Some(false)
                && let Some(postcondition) = source
                    .get("postcondition")
                    .and_then(|value| return compact_postcondition(value, false))
            {
                projected.insert("postcondition".to_owned(), postcondition);
            }
        }
        "wait" => {
            copy_value(source, &mut projected, "kind", "kind");
            copy_value(source, &mut projected, "scope", "scope");
            copy_value(source, &mut projected, "quality", "quality");
            copy_value(source, &mut projected, "completion_quality", "quality");
            copy_value(source, &mut projected, "state_hash", "state");
            copy_value(source, &mut projected, "lifecycle", "lifecycle");
            copy_value(source, &mut projected, "geometry", "size");
            copy_value(source, &mut projected, "viewport_text", "screen");
            if !projected.contains_key("size")
                && let (Some(cols), Some(rows)) = (source.get("cols"), source.get("rows"))
            {
                projected.insert("size".to_owned(), serde_json::json!([cols, rows]));
            }
            if let Some(frame) = source.get("frame").and_then(serde_json::Value::as_object) {
                copy_value(frame, &mut projected, "state_hash", "state");
                copy_value(frame, &mut projected, "geometry", "size");
                if let Some((screen, truncated)) = bounded_screen_frame_text(frame) {
                    projected.insert("screen".to_owned(), serde_json::Value::String(screen));
                    if truncated {
                        projected
                            .insert("screen_truncated".to_owned(), serde_json::Value::Bool(true));
                    }
                }
            }
            if source.get("predicate").is_some() {
                projected.insert(
                    "scope".to_owned(),
                    serde_json::Value::String("screen".to_owned()),
                );
                copy_value(source, &mut projected, "matched", "matched");
            }
        }
        "status" | "stop" => {
            for key in [
                "kind",
                "lifecycle",
                "pid",
                "enabled",
                "strict",
                "integration_version",
                "default_command_matches",
                "scope",
                "pane_count",
                "selected_panes",
                "excluded_panes",
                "recorded_panes",
                "unrecorded_panes",
                "recording_id",
                "name",
                "entries",
                "compressed_bytes",
                "maximum_entries",
                "maximum_compressed_bytes",
            ] {
                copy_value(source, &mut projected, key, key);
            }
            copy_value(source, &mut projected, "session", "session");
            copy_value(source, &mut projected, "session_display", "session");
            copy_value(source, &mut projected, "diagnostic", "message");
            if let Some(completion) = source
                .get("completion")
                .and_then(serde_json::Value::as_object)
            {
                compact_completion(
                    completion,
                    &mut projected,
                    selected(fields, ResponseField::Boundary),
                );
            }
        }
        _ => return None,
    }
    if selected(fields, ResponseField::Boundary) {
        for key in ["boundary", "boundaries", "durability"] {
            copy_compacted(&mut projected, key, key);
        }
    }
    if selected(fields, ResponseField::Cursor) {
        copy_compacted(&mut projected, "cursor", "cursor");
    }
    if selected(fields, ResponseField::Modes) {
        copy_compacted(&mut projected, "modes", "modes");
        copy_compacted(&mut projected, "terminal_modes", "terminal_modes");
    }
    if selected(fields, ResponseField::Timing) {
        for key in [
            "first_observed_monotonic_ns",
            "observed_monotonic_ns",
            "stable_for_ns",
            "pace_ns",
            "timing",
            "timestamps",
        ] {
            copy_compacted(&mut projected, key, key);
        }
    }
    return Some(serde_json::Value::Object(projected));
}

pub(super) fn response_without_screen(value: &serde_json::Value) -> serde_json::Value {
    return match value {
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(response_without_screen)
                .collect::<Vec<_>>(),
        ),
        serde_json::Value::Object(values) => {
            let values = values
                .iter()
                .filter_map(|(key, value)| {
                    return (key != "screen")
                        .then(|| return (key.clone(), response_without_screen(value)));
                })
                .collect::<serde_json::Map<_, _>>();
            serde_json::Value::Object(values)
        }
        value => value.clone(),
    };
}

pub(super) fn response_size_without_screen(value: &serde_json::Value) -> Result<usize> {
    return serde_json::to_vec(&response_without_screen(value))
        .map(|encoded| return encoded.len().saturating_add(1))
        .into_diagnostic();
}

pub(super) fn enforce_success_budget(
    mut response: serde_json::Value,
    command_type: &str,
    fields: &[ResponseField],
) -> Result<serde_json::Value> {
    if !fields.is_empty() {
        return Ok(response);
    }
    let limit = match command_type {
        "start" => START_RESPONSE_BYTES,
        "send" | "act" => ACTION_RESPONSE_BYTES,
        "wait" | "status" => WAIT_STATUS_RESPONSE_BYTES,
        "stop" => STOP_RESPONSE_BYTES,
        _ => return Ok(response),
    };
    if response_size_without_screen(&response)? <= limit {
        return Ok(response);
    }
    if let Some(postcondition) = response
        .get_mut("postcondition")
        .and_then(serde_json::Value::as_object_mut)
    {
        for key in [
            "message",
            "error",
            "kind",
            "quality",
            "size",
            "state",
            "ended",
            "unchanged",
        ] {
            postcondition.remove(key);
        }
    }
    if response_size_without_screen(&response)? <= limit {
        return Ok(response);
    }
    for key in [
        "message",
        "reason",
        "session_id",
        "session",
        "startup_state",
        "atoms",
        "bytes",
        "quality",
        "size",
        "state",
    ] {
        if let Some(response) = response.as_object_mut() {
            response.remove(key);
        }
        if response_size_without_screen(&response)? <= limit {
            return Ok(response);
        }
    }
    return Ok(response);
}
pub(super) fn bounded_agent_response(response: serde_json::Value) -> Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(&response).into_diagnostic()?;
    if encoded.len() > MAX_AGENT_RESPONSE_BYTES {
        let response_bytes = encoded.len();
        let response_type =
            value_string(&response, "type").unwrap_or_else(|| return "result".to_owned());
        let ok = response
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let applied = if ok {
            response_string_value(response.get("applied"), "applied").and_then(|value| {
                return value
                    .map(|value| return applied_from_str(&value))
                    .transpose();
            })
        } else if response.get("applied").is_some() {
            response_string_value(response.get("applied"), "applied").and_then(|value| {
                return value
                    .map(|value| return applied_from_str(&value))
                    .transpose();
            })
        } else {
            let value = nested_string(Some(&response), "applied")
                .unwrap_or_else(|| return "unknown".to_owned());
            applied_from_str(&value).map(Some)
        };
        let retry = if ok {
            response_string_value(response.get("retry"), "retry").and_then(|value| {
                return value.map(|value| return retry_from_str(&value)).transpose();
            })
        } else if response.get("retry").is_some() {
            response_string_value(response.get("retry"), "retry").and_then(|value| {
                return value.map(|value| return retry_from_str(&value)).transpose();
            })
        } else {
            let value = nested_string(Some(&response), "retry")
                .unwrap_or_else(|| return "same_request_id_only".to_owned());
            retry_from_str(&value).map(Some)
        };
        let session_state = if ok {
            response_string_value(response.get("session_state"), "session_state").and_then(
                |value| {
                    return value
                        .map(|value| return session_state_from_str(&value))
                        .transpose();
                },
            )
        } else if response.get("session_state").is_some() {
            response_string_value(response.get("session_state"), "session_state").and_then(
                |value| {
                    return value
                        .map(|value| return session_state_from_str(&value))
                        .transpose();
                },
            )
        } else {
            let value = nested_string(Some(&response), "session_state")
                .unwrap_or_else(|| return "unknown".to_owned());
            session_state_from_str(&value).map(Some)
        };
        let mut fields = BTreeMap::new();
        for key in [
            "state",
            "state_id",
            "state_hash",
            "event_seq",
            "quality",
            "size",
            "session",
            "path",
        ] {
            if let Some(value) = response.get(key).cloned() {
                fields.insert(key.to_owned(), value);
            }
        }
        if let Some(pagination) = pagination_projection(&response)
            && let Some(source) = pagination.as_object()
        {
            for key in ["page", "output_page", "next_offset", "next_cursor"] {
                if let Some(value) = source.get(key).cloned() {
                    fields.insert(key.to_owned(), value);
                }
            }
        }
        let compact = match (applied, retry, session_state) {
            (Ok(applied), Ok(retry), Ok(session_state)) => typed_envelope(CliResponseEnvelope {
                version: CLI_RESPONSE_VERSION,
                ok,
                response_type,
                code: (!ok).then(|| {
                    return nested_string(Some(&response), "code")
                        .unwrap_or_else(|| return "operation_failed".to_owned());
                }),
                applied,
                retry,
                session_state,
                request_id: response
                    .get("request_id")
                    .filter(|value| return !value.is_null())
                    .cloned(),
                message: None,
                truncated: Some(true),
                response_bytes: Some(u64::try_from(response_bytes).unwrap_or(u64::MAX)),
                fields,
            }),
            (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
                response_encoding_failure(message)
            }
        };
        encoded = serde_json::to_vec(&compact).into_diagnostic()?;
    }
    encoded.push(b'\n');
    return Ok(encoded);
}
