use super::*;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::error::ErrorKind as ClapErrorKind;

const RESPONSE_VERSION: u8 = 1;
const MAX_AGENT_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: usize = 256 * 1024;
const START_RESPONSE_BYTES: usize = 300;
const ACTION_RESPONSE_BYTES: usize = 250;
const WAIT_STATUS_RESPONSE_BYTES: usize = 300;
const MAX_COMPACT_SCREEN_BYTES: usize = 16 * 1024;
const STOP_RESPONSE_BYTES: usize = 350;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ResponseMode {
    Auto,
    Human,
    Agent,
    Full,
    Quiet,
}

impl ResponseMode {
    pub(super) fn resolve(self) -> Self {
        if self != Self::Auto {
            return self;
        }
        if io::stdout().is_terminal() {
            return Self::Human;
        }
        return Self::Agent;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ResponseField {
    Cursor,
    Modes,
    Boundary,
    Timing,
}

#[derive(Debug)]
struct ResponseState {
    mode: ResponseMode,
    fields: Vec<ResponseField>,
    command_type: String,
    mutating: bool,
    values: Vec<serde_json::Value>,
    raw: Vec<u8>,
    captured_bytes: usize,
    omitted_items: usize,
}

static RESPONSE_STATE: OnceLock<Mutex<ResponseState>> = OnceLock::new();

pub(super) fn configure_response(
    mode: ResponseMode,
    fields: Vec<ResponseField>,
    command_type: String,
    mutating: bool,
) {
    let _ = RESPONSE_STATE.set(Mutex::new(ResponseState {
        mode,
        fields,
        command_type,
        mutating,
        values: Vec::new(),
        raw: Vec::new(),
        captured_bytes: 0,
        omitted_items: 0,
    }));
}

fn response_mode() -> ResponseMode {
    let Some(state) = RESPONSE_STATE.get() else {
        return ResponseMode::Full;
    };
    return state
        .lock()
        .unwrap_or_else(|poisoned| return poisoned.into_inner())
        .mode;
}

fn progress_enabled(mode: ResponseMode) -> bool {
    return mode != ResponseMode::Quiet;
}

pub(super) fn write_progress(message: impl Display) {
    if !progress_enabled(response_mode()) {
        return;
    }
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "{message}");
}

fn pagination_projection(value: &serde_json::Value) -> Option<serde_json::Value> {
    let source = value.as_object()?;
    let mut projection = serde_json::Map::new();
    for key in [
        "kind",
        "type",
        "page",
        "output_page",
        "next_offset",
        "next_cursor",
    ] {
        if let Some(value) = source.get(key).cloned() {
            projection.insert(key.to_owned(), value);
        }
    }
    if !projection.contains_key("page")
        && !projection.contains_key("output_page")
        && let Some(results) = source.get("results").and_then(serde_json::Value::as_array)
        && let Some(page) = results
            .iter()
            .rev()
            .find_map(pagination_projection)
            .and_then(|value| return value.as_object().cloned())
    {
        for key in ["page", "output_page", "next_offset", "next_cursor"] {
            if let Some(value) = page.get(key).cloned() {
                projection.insert(key.to_owned(), value);
            }
        }
    }
    if !projection.contains_key("page")
        && !projection.contains_key("output_page")
        && !projection.contains_key("next_offset")
        && !projection.contains_key("next_cursor")
    {
        return None;
    }
    return Some(serde_json::Value::Object(projection));
}
fn append_bounded_text(output: &mut String, value: &str) -> bool {
    let remaining = MAX_COMPACT_SCREEN_BYTES.saturating_sub(output.len());
    if value.len() <= remaining {
        output.push_str(value);
        return false;
    }
    let mut end = remaining;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(value.get(..end).unwrap_or_default());
    return true;
}

fn bounded_screen_frame_text(
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

fn bounded_screen_frame_projection(
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

fn bounded_capture_projection(value: &serde_json::Value) -> Option<serde_json::Value> {
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

fn capture_json(value: &impl serde::Serialize) -> Result<()> {
    let value = serde_json::to_value(value).into_diagnostic()?;
    let encoded_bytes = serde_json::to_vec(&value).into_diagnostic()?.len();
    let Some(state) = RESPONSE_STATE.get() else {
        return Ok(());
    };
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| return poisoned.into_inner());
    if state.captured_bytes.saturating_add(encoded_bytes) > MAX_CAPTURE_BYTES {
        state.omitted_items = state.omitted_items.saturating_add(1);
        if let Some(pagination) = bounded_capture_projection(&value) {
            state.captured_bytes = state
                .captured_bytes
                .saturating_add(serde_json::to_vec(&pagination).into_diagnostic()?.len());
            state.values.push(pagination);
        }
        return Ok(());
    }
    state.captured_bytes = state.captured_bytes.saturating_add(encoded_bytes);
    state.values.push(value);
    return Ok(());
}

fn capture_raw(output: &[u8]) {
    let Some(state) = RESPONSE_STATE.get() else {
        return;
    };
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| return poisoned.into_inner());
    capture_raw_into(&mut state, output);
}

fn capture_raw_into(state: &mut ResponseState, output: &[u8]) {
    if let Some(page) = raw_pagination_projection(output) {
        state.values.push(page);
    }
    state.captured_bytes = state.captured_bytes.saturating_add(output.len());
    let remaining = MAX_CAPTURE_BYTES.saturating_sub(state.raw.len());
    let keep = remaining.min(output.len());
    if let Some(prefix) = output.get(..keep) {
        state.raw.extend_from_slice(prefix);
    }
    if keep < output.len() {
        state.omitted_items = state.omitted_items.saturating_add(1);
    }
}

fn raw_pagination_projection(output: &[u8]) -> Option<serde_json::Value> {
    let line = str::from_utf8(output)
        .ok()?
        .lines()
        .rev()
        .find_map(|line| return line.strip_prefix("page: "))?;
    let page = serde_json::from_str::<serde_json::Value>(line).ok()?;
    return Some(serde_json::json!({ "page": page }));
}

fn direct_stdout(output: &[u8]) -> Result<ExitCode> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    match stdout
        .write_all(output)
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => return Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => return Err(error).into_diagnostic(),
    }
}

fn direct_stderr(message: &str) {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    let _ = writeln!(stderr, "termtrace: {message}");
}

fn report_message(error: &miette::Report) -> String {
    let mut message = error.to_string();
    if let Some(help) = error.help() {
        let _ = write!(message, "\nhelp: {help}");
    }
    return message;
}

fn selected(fields: &[ResponseField], field: ResponseField) -> bool {
    return fields.contains(&field);
}

#[cfg(test)]
fn compact_value(value: serde_json::Value, fields: &[ResponseField]) -> Option<serde_json::Value> {
    return compact_value_with_paths(value, fields, false);
}

fn compact_value_with_paths(
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

fn value_string(value: &serde_json::Value, key: &str) -> Option<String> {
    return value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
}

fn response_type(value: Option<&serde_json::Value>, fallback: &str) -> String {
    return value
        .and_then(|value| {
            return value_string(value, "kind").or_else(|| return value_string(value, "type"));
        })
        .unwrap_or_else(|| return fallback.replace('_', "-"));
}

fn nested_string(value: Option<&serde_json::Value>, key: &str) -> Option<String> {
    return value.and_then(|value| {
        return value_string(value, key).or_else(|| {
            return value
                .get("input")
                .and_then(|input| return value_string(input, key));
        });
    });
}
fn failure_string(value: Option<&serde_json::Value>, key: &str) -> Option<String> {
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

fn session_state(value: Option<&serde_json::Value>, command_type: &str, success: bool) -> String {
    if let Some(lifecycle) = value.and_then(|value| return value_string(value, "lifecycle")) {
        return lifecycle;
    }
    if let Some(session_state) = value.and_then(|value| return value_string(value, "session_state"))
    {
        return session_state;
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

fn copy_value(
    source: &serde_json::Map<String, serde_json::Value>,
    target: &mut serde_json::Map<String, serde_json::Value>,
    source_key: &str,
    target_key: &str,
) {
    if let Some(value) = source.get(source_key).cloned() {
        target.insert(target_key.to_owned(), value);
    }
}

fn compact_postcondition(
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

fn compact_completion(
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

fn compact_task_value(
    value: serde_json::Value,
    fields: &[ResponseField],
    command_type: &str,
) -> Option<serde_json::Value> {
    let compacted = compact_value_with_paths(value.clone(), fields, command_type == "test")?;
    if !matches!(
        command_type,
        "start" | "send" | "act" | "wait" | "status" | "stop"
    ) {
        return Some(compacted);
    }
    let source = value.as_object()?;
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
            if let Some(compacted) = compacted.as_object() {
                copy_value(compacted, &mut projected, "atoms", "outcomes");
            }
        }
        "act" => {
            if let Some(compacted) = compacted.as_object() {
                if let Some(input) = compacted
                    .get("input")
                    .and_then(serde_json::Value::as_object)
                {
                    let mut summary = serde_json::Map::new();
                    for key in ["applied", "atoms", "bytes"] {
                        copy_value(input, &mut summary, key, key);
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
                            summary.insert(
                                "screen_truncated".to_owned(),
                                serde_json::Value::Bool(true),
                            );
                        }
                    }
                    projected.insert("observation".to_owned(), serde_json::Value::Object(summary));
                }
                if compacted.get("ended").and_then(serde_json::Value::as_bool) == Some(true) {
                    projected.insert("ended".to_owned(), serde_json::Value::Bool(true));
                }
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
    if selected(fields, ResponseField::Boundary)
        && let Some(compacted) = compacted.as_object()
    {
        for key in ["boundary", "boundaries", "durability"] {
            copy_value(compacted, &mut projected, key, key);
        }
    }
    if let Some(compacted) = compacted.as_object() {
        if selected(fields, ResponseField::Cursor) {
            copy_value(compacted, &mut projected, "cursor", "cursor");
        }
        if selected(fields, ResponseField::Modes) {
            copy_value(compacted, &mut projected, "modes", "modes");
            copy_value(
                compacted,
                &mut projected,
                "terminal_modes",
                "terminal_modes",
            );
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
                copy_value(compacted, &mut projected, key, key);
            }
        }
    }
    return Some(serde_json::Value::Object(projected));
}

fn compact_success(state: &ResponseState) -> serde_json::Value {
    let last = state.values.last();
    let kind = response_type(last, &state.command_type);
    let mut response = serde_json::Map::new();
    response.insert("v".to_owned(), serde_json::json!(RESPONSE_VERSION));
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
    return serde_json::Value::Object(response);
}

fn retry_for_applied(applied: &str) -> &'static str {
    return match applied {
        "all" => "unsafe",
        "partial" => "inspect_before_retry",
        "unknown" => "same_request_id_only",
        _ => "safe_new_request",
    };
}
fn compact_failure(state: &ResponseState, error: Option<&miette::Report>) -> serde_json::Value {
    let last = state.values.last();
    let mut response = serde_json::Map::new();
    response.insert("v".to_owned(), serde_json::json!(RESPONSE_VERSION));
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
    return serde_json::Value::Object(response);
}

fn response_without_screen(value: &serde_json::Value) -> serde_json::Value {
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

fn response_size_without_screen(value: &serde_json::Value) -> Result<usize> {
    return serde_json::to_vec(&response_without_screen(value))
        .map(|encoded| return encoded.len().saturating_add(1))
        .into_diagnostic();
}

fn enforce_success_budget(
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
fn bounded_agent_response(response: serde_json::Value) -> Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(&response).into_diagnostic()?;
    if encoded.len() > MAX_AGENT_RESPONSE_BYTES {
        let response_bytes = encoded.len();
        let response_type =
            value_string(&response, "type").unwrap_or_else(|| return "result".to_owned());
        let ok = response
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let mut compact = if ok {
            serde_json::json!({
                "v": RESPONSE_VERSION,
                "ok": true,
                "type": response_type,
                "truncated": true,
                "response_bytes": response_bytes,
            })
        } else {
            serde_json::json!({
                "v": RESPONSE_VERSION,
                "ok": false,
                "type": response_type,
                "code": nested_string(Some(&response), "code")
                    .unwrap_or_else(|| return "operation_failed".to_owned()),
                "applied": nested_string(Some(&response), "applied")
                    .unwrap_or_else(|| return "unknown".to_owned()),
                "retry": nested_string(Some(&response), "retry")
                    .unwrap_or_else(|| return "same_request_id_only".to_owned()),
                "session_state": nested_string(Some(&response), "session_state")
                    .unwrap_or_else(|| return "unknown".to_owned()),
                "truncated": true,
                "response_bytes": response_bytes,
            })
        };
        if let Some(compact) = compact.as_object_mut() {
            for key in [
                "applied",
                "retry",
                "session_state",
                "request_id",
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
                    compact.insert(key.to_owned(), value);
                }
            }
        }
        if let Some(pagination) = pagination_projection(&response)
            && let (Some(target), Some(source)) = (compact.as_object_mut(), pagination.as_object())
        {
            for key in ["page", "output_page", "next_offset", "next_cursor"] {
                if let Some(value) = source.get(key).cloned() {
                    target.insert(key.to_owned(), value);
                }
            }
        }
        encoded = serde_json::to_vec(&compact).into_diagnostic()?;
    }
    encoded.push(b'\n');
    return Ok(encoded);
}

fn human_screen_summary(value: &serde_json::Value) -> String {
    let state = value_string(value, "state").unwrap_or_else(|| return "unknown".to_owned());
    let size = value.get("size").and_then(serde_json::Value::as_array);
    let cols = size
        .and_then(|size| return size.first())
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let rows = size
        .and_then(|size| return size.get(1))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let quality = value_string(value, "quality").unwrap_or_else(|| return "unknown".to_owned());
    if value
        .get("unchanged")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return format!("Viewport unchanged at state {state} ({cols}×{rows}, {quality})\n");
    }
    let Some(screen) = value.get("screen") else {
        return format!("Viewport state {state} ({cols}×{rows}, {quality}); content omitted\n");
    };
    if let Some(viewport) = screen.as_str() {
        return format!(
            "Viewport state {state} ({cols}×{rows}, {quality})\n{}\n",
            viewport.trim_end_matches('\n')
        );
    }
    let Some(screen) = screen.as_object() else {
        return format!("Viewport state {state} ({cols}×{rows}, {quality}); content unavailable\n");
    };
    let encoding =
        value_string(&serde_json::Value::Object(screen.clone()), "encoding").unwrap_or_default();
    let label = if encoding == "diff_rows" {
        let since = value_string(&serde_json::Value::Object(screen.clone()), "since")
            .unwrap_or_else(|| return "unknown".to_owned());
        format!("Viewport diff from {since} to {state}")
    } else {
        format!("Sparse viewport at state {state}")
    };
    let mut output = format!("{label} ({cols}×{rows}, {quality})\n");
    let changed_rows = screen
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    if changed_rows == 0 {
        output.push_str("  (no nonblank or changed rows)\n");
        return output;
    }
    if let Some(screen_rows) = screen.get("rows").and_then(serde_json::Value::as_array) {
        for row in screen_rows {
            let Some(row) = row.as_array() else {
                continue;
            };
            let number = row
                .first()
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let text = row
                .get(1)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let _ = writeln!(output, "  row {number}: {text}");
        }
    }
    return output;
}

fn human_failure_summary(state: &ResponseState, error: Option<&miette::Report>) -> String {
    let last = state.values.last();
    let code = failure_string(last, "code").unwrap_or_else(|| return "operation_failed".to_owned());
    let message = error.map_or_else(
        || {
            return failure_string(last, "message")
                .or_else(|| return failure_string(last, "error"))
                .or_else(|| return failure_string(last, "diagnostic"))
                .unwrap_or_else(|| return "command failed".to_owned());
        },
        report_message,
    );
    let mut summary = format!("[{code}] {message}");
    if state.mutating {
        let applied = nested_string(last, "applied").unwrap_or_else(|| {
            return if last.is_some() { "unknown" } else { "none" }.to_owned();
        });
        let retry = nested_string(last, "retry")
            .unwrap_or_else(|| return retry_for_applied(&applied).to_owned());
        let current_state = session_state(last, &state.command_type, false);
        let _ = write!(
            summary,
            "\nmutation: applied={applied}, retry={retry}, session_state={current_state}"
        );
    }
    if let Some(postcondition) = last.and_then(|value| return value.get("postcondition")) {
        let matched = postcondition
            .get("matched")
            .and_then(serde_json::Value::as_bool)
            .map_or_else(
                || return "unknown".to_owned(),
                |matched| return matched.to_string(),
            );
        let status = value_string(postcondition, "state")
            .or_else(|| return value_string(postcondition, "quality"))
            .unwrap_or_else(|| return "unknown".to_owned());
        let _ = write!(
            summary,
            "\npostcondition: matched={matched}, status={status}"
        );
    }
    return summary;
}

fn value_u64(value: &serde_json::Value, key: &str) -> u64 {
    return value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
}

fn human_action_summary(value: &serde_json::Value) -> String {
    let input = value.get("input").unwrap_or(&serde_json::Value::Null);
    let applied = value_string(input, "applied").unwrap_or_else(|| return "unknown".to_owned());
    let atoms = value_u64(input, "atoms");
    let bytes = value_u64(input, "bytes");
    let postcondition = value
        .get("postcondition")
        .unwrap_or(&serde_json::Value::Null);
    let postcondition_kind =
        value_string(postcondition, "kind").unwrap_or_else(|| return "unknown".to_owned());
    let outcome =
        value_string(postcondition, "outcome").unwrap_or_else(|| return "unknown".to_owned());
    let matched = postcondition
        .get("matched")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let session = if value
        .get("ended")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        "ended"
    } else {
        "live"
    };
    return format!(
        "Action: mutation applied={applied}, atoms={atoms}, bytes={bytes}; postcondition {postcondition_kind} matched={matched}, outcome={outcome}; session={session}\n"
    );
}

fn human_inspect_summary(value: &serde_json::Value) -> String {
    let action = value.get("action").unwrap_or(&serde_json::Value::Null);
    let status = value_string(action, "status").unwrap_or_else(|| return "unknown".to_owned());
    let bytes = value_u64(action, "bytes_written");
    let cells = value_u64(value, "cells_changed");
    let rows = value
        .get("rows_changed")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let warnings = value
        .get("warnings")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    return format!(
        "Last action: status={status}, bytes={bytes}, changed={cells} cells across {rows} rows, warnings={warnings}\n"
    );
}

fn human_diagnose_summary(value: &serde_json::Value) -> String {
    let findings = value
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if findings.is_empty() {
        return "Diagnostics: no findings for the last action\n".to_owned();
    }
    let codes = findings
        .iter()
        .filter_map(|finding| return value_string(finding, "code"))
        .collect::<Vec<_>>()
        .join(", ");
    return format!("Diagnostics: {} finding(s): {codes}\n", findings.len());
}

fn artifact_path(value: &serde_json::Value) -> String {
    for key in [
        "output_display",
        "file",
        "archive",
        "directory",
        "artifact_directory",
        "manifest",
    ] {
        if let Some(path) = value_string(value, key) {
            return path;
        }
    }
    return "unknown path".to_owned();
}

fn human_artifact_summary(
    value: &serde_json::Value,
    command_type: &str,
    kind: &str,
) -> Option<String> {
    return match command_type {
        "screenshot" => {
            let state =
                value_string(value, "state_hash").unwrap_or_else(|| return "unknown".to_owned());
            Some(format!(
                "Screenshot written to {} (state {state})\n",
                artifact_path(value)
            ))
        }
        "bundle" => Some(format!("Bundle written to {}\n", artifact_path(value))),
        "bug-report" | "bug_report" => {
            Some(format!("Bug report written to {}\n", artifact_path(value)))
        }
        "video" => {
            let renderer =
                value_string(value, "renderer").unwrap_or_else(|| return "unknown".to_owned());
            let fps = value_u64(value, "fps");
            let width = value_u64(value, "width");
            let height = value_u64(value, "height");
            Some(format!(
                "Video written to {} ({width}×{height}, {fps} FPS, {renderer})\n",
                artifact_path(value)
            ))
        }
        _ if matches!(
            kind,
            "semantic_screenshot"
                | "native_screenshot"
                | "semantic_screenshot_batch"
                | "bundle"
                | "bundle_archive"
                | "bug_report"
                | "video"
        ) =>
        {
            Some(format!(
                "{} written to {}\n",
                kind.replace('_', " "),
                artifact_path(value)
            ))
        }
        _ => None,
    };
}

fn human_summary(value: &serde_json::Value, command_type: &str) -> String {
    let kind = response_type(Some(value), command_type);
    if kind == "screen" {
        return human_screen_summary(value);
    }
    if command_type == "status" || kind == "status" {
        let lifecycle = if kind == "completed" {
            "ended".to_owned()
        } else {
            value_string(value, "lifecycle").unwrap_or_else(|| return "live".to_owned())
        };
        let exit = value
            .get("completion")
            .and_then(|completion| return completion.get("exit_code"))
            .and_then(serde_json::Value::as_i64)
            .map_or(String::new(), |exit| return format!(", exit={exit}"));
        let diagnostic = value_string(value, "diagnostic")
            .or_else(|| return value_string(value, "message"))
            .map_or(String::new(), |message| return format!(" — {message}"));
        return format!("Status: session={lifecycle}{exit}{diagnostic}\n");
    }
    if command_type == "act" || kind == "action" {
        return human_action_summary(value);
    }
    if command_type == "inspect" {
        return human_inspect_summary(value);
    }
    if command_type == "diagnose" {
        return human_diagnose_summary(value);
    }
    if let Some(summary) = human_artifact_summary(value, command_type, &kind) {
        return summary;
    }
    if kind == "started" {
        let name = value_string(value, "alias").unwrap_or_else(|| return "session".to_owned());
        let path = value_string(value, "canonical_session_display")
            .or_else(|| return value_string(value, "path"))
            .unwrap_or_default();
        return format!("Started {name} — {path}\n");
    }
    if kind == "stable_snapshot" {
        let state =
            value_string(value, "state_hash").unwrap_or_else(|| return "unknown".to_owned());
        let geometry = value.get("geometry");
        let cols = geometry
            .and_then(|geometry| return geometry.get("cols"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let rows = geometry
            .and_then(|geometry| return geometry.get("rows"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        return format!("Stable at state {state} ({cols}×{rows})\n");
    }
    if kind == "input_sequence_applied" {
        let atoms = value
            .get("atom_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        let bytes = value
            .get("bytes_written")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        return format!("Sent {atoms} inputs, {bytes} bytes\n");
    }
    if kind == "completed" {
        return "Session completed\n".to_owned();
    }
    return format!("{} succeeded\n", kind.replace('_', " "));
}

pub(super) fn finish_response(result: Result<ExitCode>) -> ExitCode {
    let Some(state) = RESPONSE_STATE.get() else {
        return match result {
            Ok(code) => code,
            Err(error) => {
                direct_stderr(&error.to_string());
                ExitCode::FAILURE
            }
        };
    };
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| return poisoned.into_inner());
    let (code, error) = match result {
        Ok(code) => (code, None),
        Err(error) => (ExitCode::FAILURE, Some(error)),
    };
    let success = code == ExitCode::SUCCESS && error.is_none();

    let write_result = match state.mode {
        ResponseMode::Auto => {
            direct_stderr("internal error: auto response mode was not resolved");
            return ExitCode::FAILURE;
        }
        ResponseMode::Full => Ok(ExitCode::SUCCESS),
        ResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResponseMode::Human => {
            if success {
                if !state.raw.is_empty() {
                    direct_stdout(&state.raw)
                } else {
                    let summary = state
                        .values
                        .iter()
                        .map(|value| return human_summary(value, &state.command_type))
                        .collect::<String>();
                    direct_stdout(summary.as_bytes())
                }
            } else {
                direct_stderr(&human_failure_summary(&state, error.as_ref()));
                Ok(ExitCode::SUCCESS)
            }
        }
        ResponseMode::Agent => {
            let response = if success {
                enforce_success_budget(compact_success(&state), &state.command_type, &state.fields)
            } else {
                Ok(compact_failure(&state, error.as_ref()))
            };
            response
                .and_then(bounded_agent_response)
                .and_then(|output| return direct_stdout(&output))
        }
    };
    if let Some(error) = error
        && !matches!(state.mode, ResponseMode::Quiet | ResponseMode::Human)
    {
        direct_stderr(&report_message(&error));
    }
    if let Err(error) = write_result {
        direct_stderr(&error.to_string());
        return ExitCode::FAILURE;
    }
    return code;
}

pub(super) fn write_clap_error(error: clap::Error, mode: ResponseMode) -> ExitCode {
    let success = matches!(
        error.kind(),
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
    );
    let code = if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2))
    };
    let mode = if success && mode == ResponseMode::Auto {
        ResponseMode::Human
    } else {
        mode.resolve()
    };
    match mode {
        ResponseMode::Auto => {
            direct_stderr("internal error: auto response mode was not resolved");
            return ExitCode::FAILURE;
        }
        ResponseMode::Full | ResponseMode::Human => {
            let _ = error.print();
        }
        ResponseMode::Quiet => {}
        ResponseMode::Agent => {
            let response = if success {
                serde_json::json!({
                    "v": RESPONSE_VERSION,
                    "ok": true,
                    "type": "help",
                    "output": error.to_string(),
                })
            } else {
                serde_json::json!({
                    "v": RESPONSE_VERSION,
                    "ok": false,
                    "type": "arguments",
                    "code": "invalid_arguments",
                    "applied": "none",
                    "retry": "safe_new_request",
                    "session_state": "unknown",
                    "message": error.to_string().trim(),
                })
            };
            if let Ok(output) = bounded_agent_response(response) {
                let _ = direct_stdout(&output);
            }
            if !success {
                direct_stderr(error.to_string().trim());
            }
        }
    }
    return code;
}

pub(super) fn stable_enum_label(value: &impl serde::Serialize) -> Result<String> {
    let serialized = serde_json::to_value(value).into_diagnostic()?;
    let Some(label) = serialized.as_str() else {
        return Err(miette::miette!(
            "human output label did not serialize as a string"
        ));
    };
    return Ok(label.to_owned());
}

pub(super) fn optional_display(value: Option<impl Display>) -> String {
    return value.map_or_else(|| return "-".to_owned(), |value| return value.to_string());
}

pub(super) fn id_list(values: &[u64]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    let mut output = String::new();
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "{value}");
    }
    return output;
}

pub(super) fn quoted_text(value: &str) -> Result<String> {
    return serde_json::to_string(value).into_diagnostic();
}

pub(super) fn projection_warning(quality: OutputProjectionQuality) -> Option<&'static str> {
    return match quality {
        OutputProjectionQuality::ExactBytes => None,
        OutputProjectionQuality::LossyLineOriented => Some(
            "projection_quality=lossy_line_oriented; controls were removed; use show/timeline for viewport state",
        ),
        OutputProjectionQuality::UnsuitableForFullScreen => Some(
            "projection_quality=unsuitable_for_full_screen; alternate-screen controls were observed; use show/timeline for viewport state",
        ),
    };
}

pub(super) fn write_projection_warning(message: &str) -> Result<()> {
    if response_mode() == ResponseMode::Quiet {
        return Ok(());
    }
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    writeln!(stderr, "termtrace: {message}").into_diagnostic()?;
    return Ok(());
}

pub(super) struct ResponseOutput {
    mode: ResponseMode,
}

impl io::Write for ResponseOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        return match self.mode {
            ResponseMode::Full => io::stdout().lock().write(buf),
            ResponseMode::Quiet => Ok(buf.len()),
            ResponseMode::Auto => Err(io::Error::other("auto response mode was not resolved")),
            ResponseMode::Human | ResponseMode::Agent => {
                capture_raw(buf);
                Ok(buf.len())
            }
        };
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.mode == ResponseMode::Full {
            return io::stdout().lock().flush();
        }
        return Ok(());
    }
}

pub(super) fn response_output() -> ResponseOutput {
    return ResponseOutput {
        mode: response_mode(),
    };
}

pub(super) fn write_follow_stdout(output: &[u8]) -> Result<bool> {
    return match response_mode() {
        ResponseMode::Full => {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            match stdout
                .write_all(output)
                .and_then(|()| return stdout.flush())
            {
                Ok(()) => Ok(true),
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
                Err(error) => Err(error).into_diagnostic(),
            }
        }
        ResponseMode::Quiet => Ok(true),
        ResponseMode::Auto => Err(miette::miette!("auto response mode was not resolved")),
        ResponseMode::Human | ResponseMode::Agent => {
            capture_raw(output);
            Ok(true)
        }
    };
}

pub(super) fn write_follow_json_line(value: &impl serde::Serialize) -> Result<bool> {
    if response_mode() == ResponseMode::Full {
        let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
        encoded.push(b'\n');
        return write_follow_stdout(&encoded);
    }
    if response_mode() != ResponseMode::Quiet {
        capture_json(value)?;
    }
    return Ok(true);
}

pub(super) fn row_text(row: &RowState) -> String {
    let mut text = String::new();
    for cell in &row.cells {
        if cell.grapheme.is_empty() {
            text.push(' ');
        } else {
            text.push_str(&cell.grapheme);
        }
    }
    return text.trim_end().to_owned();
}

pub(super) fn paginate<T>(items: Vec<T>, page: &PageArgs) -> (Vec<T>, OutputPage, bool) {
    let total = items.len();
    let start = page.offset.min(total);
    let end = if page.all {
        total
    } else {
        start.saturating_add(page.limit).min(total)
    };
    let returned = end.saturating_sub(start);
    let partial = start != 0 || end < total;
    let next_offset = (end < total).then_some(end);
    let selected = items
        .into_iter()
        .skip(start)
        .take(returned)
        .collect::<Vec<_>>();
    return (
        selected,
        OutputPage {
            offset: start,
            limit: (!page.all).then_some(page.limit),
            total: Some(total),
            returned,
            truncated: end < total,
            next_offset,
        },
        partial,
    );
}

pub(super) fn write_json_line(value: &impl serde::Serialize) -> Result<ExitCode> {
    return match response_mode() {
        ResponseMode::Full => {
            let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
            encoded.push(b'\n');
            direct_stdout(&encoded)
        }
        ResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResponseMode::Auto => Err(miette::miette!("auto response mode was not resolved")),
        ResponseMode::Human | ResponseMode::Agent => {
            capture_json(value)?;
            Ok(ExitCode::SUCCESS)
        }
    };
}

pub(super) fn write_stdout(output: impl AsRef<[u8]>) -> Result<ExitCode> {
    return match response_mode() {
        ResponseMode::Full => direct_stdout(output.as_ref()),
        ResponseMode::Quiet => Ok(ExitCode::SUCCESS),
        ResponseMode::Auto => Err(miette::miette!("auto response mode was not resolved")),
        ResponseMode::Human | ResponseMode::Agent => {
            capture_raw(output.as_ref());
            Ok(ExitCode::SUCCESS)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_fields_hide_forensic_tables_by_default() {
        let source = serde_json::json!({
            "state_hash": "abc",
            "cursor": { "column": 2, "row": 3 },
            "modes": { "dec_1049": true },
            "boundary": { "event_hash": "def", "next_event_seq": 9 },
            "observed_monotonic_ns": 42,
            "nullable": null,
        });
        let compact = compact_value(source.clone(), &[]).unwrap_or_default();
        assert_eq!(compact["state_hash"], "abc");
        assert!(compact.get("cursor").is_none());
        assert!(compact.get("modes").is_none());
        assert!(compact.get("boundary").is_none());
        assert!(compact.get("observed_monotonic_ns").is_none());
        assert!(compact.get("nullable").is_none());

        let fields = [
            ResponseField::Cursor,
            ResponseField::Modes,
            ResponseField::Boundary,
            ResponseField::Timing,
        ];
        let expanded = compact_value(source, &fields).unwrap_or_default();
        assert!(expanded.get("cursor").is_some());
        assert!(expanded.get("modes").is_some());
        assert!(expanded.get("boundary").is_some());
        assert!(expanded.get("observed_monotonic_ns").is_some());
    }

    #[test]
    fn oversized_agent_errors_keep_required_retry_contract() {
        let response = serde_json::json!({
            "v": 1,
            "ok": false,
            "type": "action",
            "code": "postcondition_failed",
            "applied": "all",
            "retry": "unsafe",
            "session_state": "live",
            "screen": "x".repeat(MAX_AGENT_RESPONSE_BYTES),
        });
        let encoded = bounded_agent_response(response).unwrap_or_default();
        assert!(encoded.len() <= MAX_AGENT_RESPONSE_BYTES);
        let compact: serde_json::Value =
            serde_json::from_slice(&encoded).unwrap_or(serde_json::Value::Null);
        assert_eq!(compact["ok"], false);
        assert_eq!(compact["applied"], "all");
        assert_eq!(compact["retry"], "unsafe");
        assert_eq!(compact["session_state"], "live");
        assert_eq!(compact["truncated"], true);
    }
    #[test]
    fn oversized_agent_pages_keep_the_next_offset() {
        let response = serde_json::json!({
            "v": 1,
            "ok": true,
            "type": "events",
            "results": [
                {"kind": "event", "payload": "x".repeat(MAX_AGENT_RESPONSE_BYTES)},
                {
                    "kind": "output_page",
                    "page": {
                        "offset": 0,
                        "limit": 100,
                        "total": 250,
                        "returned": 100,
                        "truncated": true,
                        "next_offset": 100
                    }
                }
            ]
        });
        let encoded = bounded_agent_response(response).unwrap_or_default();
        let compact: serde_json::Value =
            serde_json::from_slice(&encoded).unwrap_or(serde_json::Value::Null);
        assert_eq!(compact["truncated"], true);
        assert_eq!(compact["page"]["next_offset"], 100);
        assert_eq!(compact["page"]["returned"], 100);
    }

    #[test]
    fn oversized_raw_pages_keep_the_trailing_resume_cursor() {
        let mut state = response_state("timeline", false, serde_json::Value::Null, Vec::new());
        let output = format!(
            "{}\npage: {{\"offset\":0,\"limit\":600,\"returned\":600,\"truncated\":true,\"next_offset\":580}}\n",
            "x".repeat(MAX_CAPTURE_BYTES)
        );
        capture_raw_into(&mut state, output.as_bytes());

        let encoded =
            bounded_agent_response(compact_success(&state)).expect("bounded agent response");
        let compact: serde_json::Value =
            serde_json::from_slice(&encoded).expect("agent response JSON");
        assert_eq!(compact["truncated"], true);
        assert_eq!(compact["page"]["next_offset"], 580);
        assert_eq!(compact["page"]["returned"], 600);
    }

    #[test]
    fn binary_raw_output_is_base64_encoded_for_agents() {
        let mut state = response_state("output", false, serde_json::Value::Null, Vec::new());
        capture_raw_into(&mut state, b"\xff");

        let response = compact_success(&state);
        assert_eq!(response["output"]["encoding"], "base64");
        assert_eq!(response["output"]["data"], "/w==");
        assert_eq!(response["output"]["bytes"], 1);
        assert_eq!(response["output"]["truncated"], false);
    }

    #[test]
    fn capture_budget_fallback_keeps_only_pagination_metadata() {
        let value = serde_json::json!({
            "kind": "timeline",
            "entries": ["x".repeat(MAX_CAPTURE_BYTES)],
            "output_page": {
                "offset": 40,
                "limit": 20,
                "returned": 20,
                "truncated": true,
                "next_offset": 60
            }
        });
        let projection = pagination_projection(&value).unwrap_or_default();
        assert_eq!(projection["kind"], "timeline");
        assert_eq!(projection["output_page"]["next_offset"], 60);
        assert!(projection.get("entries").is_none());
    }

    #[test]
    fn oversized_capture_projection_keeps_state_identity_and_omits_bulk_state() {
        let value = serde_json::json!({
            "record": {
                "state_id": 13_000,
                "state_hash": "abc",
                "event_seq": 42,
                "presented": true,
                "delta": {
                    "rows": ["x".repeat(MAX_CAPTURE_BYTES)]
                }
            },
            "state": {
                "grid": ["x".repeat(MAX_CAPTURE_BYTES)]
            }
        });
        let projection = bounded_capture_projection(&value).unwrap_or_default();
        assert_eq!(projection["record"]["state_id"], 13_000);
        assert_eq!(projection["record"]["state_hash"], "abc");
        assert_eq!(projection["record"]["event_seq"], 42);
        assert_eq!(projection["record"]["presented"], true);
        assert!(projection["record"].get("delta").is_none());
        assert!(projection.get("state").is_none());
    }

    #[test]
    fn rich_wait_and_action_frames_keep_bounded_agent_identity_and_text() {
        let frame = serde_json::json!({
            "version": 1,
            "state_hash": "a".repeat(64),
            "frame_hash": "b".repeat(64),
            "presented": true,
            "capture_quality": "live",
            "geometry": { "cols": 80, "rows": 24 },
            "data": {
                "detail": "spans",
                "rows": [
                    { "row": 0, "text": "READY" },
                    { "row": 1, "text": "next" }
                ]
            }
        });
        let wait = serde_json::json!({
            "predicate": { "predicate": "contains", "text": "MISSING" },
            "matched": false,
            "quality": "deadline",
            "frame": frame,
        });
        let projected = bounded_capture_projection(&wait).expect("bounded wait projection");
        let compact = compact_task_value(projected, &[], "wait").expect("compact wait");
        assert_eq!(compact["scope"], "screen");
        assert_eq!(compact["matched"], false);
        assert_eq!(compact["state"], "a".repeat(64));
        assert_eq!(compact["screen"], "READY\nnext");

        let action = serde_json::json!({
            "observation": frame,
            "postcondition": {
                "kind": "screen",
                "matched": true,
                "outcome": "matched_unsettled"
            }
        });
        let projected = bounded_capture_projection(&action).expect("bounded action projection");
        let compact = compact_task_value(projected, &[], "act").expect("compact action");
        assert_eq!(compact["observation"]["state"], "a".repeat(64));
        assert_eq!(compact["observation"]["screen"], "READY\nnext");
    }

    fn response_state(
        command_type: &str,
        mutating: bool,
        value: serde_json::Value,
        fields: Vec<ResponseField>,
    ) -> ResponseState {
        let captured_bytes = serde_json::to_vec(&value).expect("response fixture").len();
        return ResponseState {
            mode: ResponseMode::Agent,
            fields,
            command_type: command_type.to_owned(),
            mutating,
            values: vec![value],
            raw: Vec::new(),
            captured_bytes,
            omitted_items: 0,
        };
    }

    #[test]
    fn task_projections_are_allowlisted_and_meet_default_byte_gates() {
        let start = response_state(
            "start",
            true,
            serde_json::json!({
                "kind": "started",
                "startup_state": "completed_success",
                "alias": "fast",
                "canonical_session": "/private/session",
                "canonical_session_display": "/private/session",
                "session_id": "018f47d2-7ad0-7000-8000-000000000000",
                "recorder_pid": 42,
                "recorder_process_start_ticks": 99,
                "stdout_log": "/private/stdout",
                "stderr_log": "/private/stderr",
                "ready": false,
                "completion": {
                    "exit_code": 0,
                    "signal_name": null,
                    "event_count": 4,
                    "final_event_hash": "a".repeat(64),
                    "output_bytes": 12,
                },
                "postcondition": {
                    "matched": true,
                    "state": "b".repeat(64),
                    "screen": "large viewport".repeat(100),
                },
            }),
            Vec::new(),
        );
        let start =
            enforce_success_budget(compact_success(&start), "start", &[]).expect("start budget");
        assert_eq!(start["session_state"], "ended");
        assert!(start.get("canonical_session").is_none());
        assert_eq!(start["path"], "/private/session");
        assert!(start.get("recorder_pid").is_none());
        assert!(start.get("ready").is_none());
        assert!(start.get("boundary").is_none());
        assert!(start["postcondition"].get("screen").is_none());
        assert!(response_size_without_screen(&start).expect("start size") <= START_RESPONSE_BYTES);

        let send = response_state(
            "send",
            true,
            serde_json::json!({
                "kind": "input_sequence_applied",
                "sequence_id": "018f47d2-7ad0-7000-8000-000000000001",
                "atom_count": 3,
                "bytes_written": 9,
                "pace_ns": 10,
                "boundary": {"event_hash": "c".repeat(64)},
            }),
            Vec::new(),
        );
        let send =
            enforce_success_budget(compact_success(&send), "send", &[]).expect("send budget");
        assert_eq!(send["atoms"], 3);
        assert_eq!(send["bytes"], 9);
        assert!(send.get("atom_count").is_none());
        assert!(send.get("pace_ns").is_none());
        assert!(response_size_without_screen(&send).expect("send size") <= ACTION_RESPONSE_BYTES);

        let act = response_state(
            "act",
            true,
            serde_json::json!({
                "type": "action",
                "request_id": "018f47d2-7ad0-7000-8000-000000000002",
                "input": {"applied": "all", "atoms": 1, "bytes": 1, "pace_ns": 0},
                "postcondition": {
                    "kind": "screen",
                    "matched": true,
                    "outcome": "stable",
                },
                "observation": {
                    "state": "d".repeat(64),
                    "quality": "stable",
                    "size": [80, 24],
                    "screen": "viewport".repeat(100),
                },
                "ended": false,
            }),
            Vec::new(),
        );
        let act = enforce_success_budget(compact_success(&act), "act", &[]).expect("act budget");
        assert_eq!(act["input"]["applied"], "all");
        assert!(act.get("postcondition").is_none());
        assert!(act["observation"].get("screen").is_some());
        assert!(response_size_without_screen(&act).expect("act size") <= ACTION_RESPONSE_BYTES);

        for (command, mutating, limit, value) in [
            (
                "wait",
                false,
                WAIT_STATUS_RESPONSE_BYTES,
                serde_json::json!({
                    "kind": "stable_snapshot",
                    "quality": "completed",
                    "state_hash": "e".repeat(64),
                    "viewport_text": "large viewport".repeat(100),
                    "geometry": {"cols": 80, "rows": 24},
                    "modes": {"alternate_screen": false},
                    "observed_monotonic_ns": 7,
                    "boundary": {"event_hash": "f".repeat(64)},
                }),
            ),
            (
                "status",
                false,
                WAIT_STATUS_RESPONSE_BYTES,
                serde_json::json!({
                    "kind": "status",
                    "session": "app",
                    "lifecycle": "live",
                    "pid": 42,
                    "boundary": {"event_hash": "f".repeat(64)},
                }),
            ),
            (
                "stop",
                true,
                STOP_RESPONSE_BYTES,
                serde_json::json!({
                    "kind": "completed",
                    "completion": {
                        "exit_code": 0,
                        "signal_name": null,
                        "event_count": 4,
                        "final_event_hash": "f".repeat(64),
                        "output_bytes": 12,
                    },
                }),
            ),
        ] {
            let state = response_state(command, mutating, value, Vec::new());
            let response = enforce_success_budget(compact_success(&state), command, &[])
                .expect("task response budget");
            assert!(
                response_size_without_screen(&response).expect("task size") <= limit,
                "{command} response exceeded {limit} bytes: {response}"
            );
            let mut gated_response = response.clone();
            if command == "status" {
                assert_eq!(gated_response["pid"], 42);
                gated_response
                    .as_object_mut()
                    .expect("status response")
                    .remove("pid");
            }
            assert!(!contains_forbidden_default_field(&gated_response));
            if command == "wait" {
                assert_eq!(response["session_state"], "ended");
                assert_eq!(response["kind"], "stable_snapshot");
                assert!(response.get("screen").is_some());
            }
            if command == "stop" {
                assert_eq!(response["kind"], "completed");
            }
        }
    }

    fn contains_forbidden_default_field(value: &serde_json::Value) -> bool {
        return match value {
            serde_json::Value::Array(values) => values.iter().any(contains_forbidden_default_field),
            serde_json::Value::Object(values) => {
                values.keys().any(|key| {
                    return matches!(
                        key.as_str(),
                        "boundary"
                            | "boundaries"
                            | "modes"
                            | "terminal_modes"
                            | "timing"
                            | "pid"
                            | "recorder_pid"
                            | "recorder_process_start_ticks"
                            | "stdout_log"
                            | "stderr_log"
                            | "ready"
                    );
                }) || values.values().any(contains_forbidden_default_field)
            }
            _ => false,
        };
    }

    #[test]
    fn boundary_is_only_included_when_requested_and_partial_requires_inspection() {
        let value = serde_json::json!({
            "kind": "input_sequence_applied",
            "sequence_id": "018f47d2-7ad0-7000-8000-000000000003",
            "atom_count": 1,
            "bytes_written": 1,
            "boundary": {"event_hash": "a".repeat(64), "next_event_seq": 3},
        });
        let default = compact_success(&response_state("send", true, value.clone(), Vec::new()));
        assert!(default.get("boundary").is_none());
        let requested = compact_success(&response_state(
            "send",
            true,
            value,
            vec![ResponseField::Boundary],
        ));
        assert!(requested.get("boundary").is_some());
        assert_eq!(retry_for_applied("none"), "safe_new_request");
        assert_eq!(retry_for_applied("unknown"), "same_request_id_only");
        assert_eq!(retry_for_applied("partial"), "inspect_before_retry");
    }
    #[test]
    fn human_screen_renders_text_sparse_and_diff_content() {
        let base = serde_json::json!({
            "type": "screen",
            "state": "current",
            "quality": "stable",
            "size": [80, 24],
        });
        let mut text = base.clone();
        text["screen"] = serde_json::json!("hello\nworld");
        let text = human_screen_summary(&text);
        assert!(text.contains("Viewport state current"));
        assert!(text.contains("hello\nworld"));

        let mut sparse = base.clone();
        sparse["screen"] = serde_json::json!({
            "encoding": "rows",
            "rows": [[3, "visible"]],
            "blank_rows_omitted": 23,
        });
        let sparse = human_screen_summary(&sparse);
        assert!(sparse.contains("Sparse viewport"));
        assert!(sparse.contains("row 3: visible"));

        let mut diff = base;
        diff["screen"] = serde_json::json!({
            "encoding": "diff_rows",
            "since": "prior",
            "rows": [[4, "changed"]],
        });
        let diff = human_screen_summary(&diff);
        assert!(diff.contains("Viewport diff from prior to current"));
        assert!(diff.contains("row 4: changed"));
    }
    #[test]
    fn human_task_summaries_preserve_status_findings_mutation_and_paths() {
        let status = human_summary(&serde_json::json!({"kind": "status", "pid": 42}), "status");
        assert_eq!(status, "Status: session=live\n");

        let action = human_summary(
            &serde_json::json!({
                "type": "action",
                "input": {"applied": "all", "atoms": 2, "bytes": 6},
                "postcondition": {"kind": "screen", "matched": true, "outcome": "stable"},
                "ended": false,
            }),
            "act",
        );
        assert!(action.contains("mutation applied=all, atoms=2, bytes=6"));
        assert!(action.contains("postcondition screen matched=true, outcome=stable"));

        let inspect = human_summary(
            &serde_json::json!({
                "format": "termtrace-last-action",
                "action": {"status": "applied", "bytes_written": 4},
                "cells_changed": 7,
                "rows_changed": [1, 2],
                "warnings": [{"code": "broad_change"}],
            }),
            "inspect",
        );
        assert!(inspect.contains("status=applied"));
        assert!(inspect.contains("changed=7 cells across 2 rows, warnings=1"));

        let diagnose = human_summary(
            &serde_json::json!({
                "format": "termtrace-last-action-diagnostics",
                "findings": [{"code": "stale_cells"}, {"code": "cursor_jump"}],
            }),
            "diagnose",
        );
        assert!(diagnose.contains("2 finding(s): stale_cells, cursor_jump"));

        let screenshot = human_summary(
            &serde_json::json!({
                "kind": "semantic_screenshot",
                "file": "/tmp/screen.png",
                "state_hash": "abc",
            }),
            "screenshot",
        );
        assert!(screenshot.contains("/tmp/screen.png"));
        assert!(screenshot.contains("state abc"));

        let bundle = human_summary(
            &serde_json::json!({"kind": "bundle", "directory": "/tmp/report"}),
            "bundle",
        );
        assert_eq!(bundle, "Bundle written to /tmp/report\n");
        let bug_report = human_summary(
            &serde_json::json!({"kind": "bug_report", "directory": "/tmp/bug"}),
            "bug-report",
        );
        assert_eq!(bug_report, "Bug report written to /tmp/bug\n");
        let video = human_summary(
            &serde_json::json!({
                "kind": "video",
                "output_display": "/tmp/demo.mp4",
                "renderer": "semantic",
                "fps": 30,
                "width": 1280,
                "height": 720,
            }),
            "video",
        );
        assert_eq!(
            video,
            "Video written to /tmp/demo.mp4 (1280×720, 30 FPS, semantic)\n"
        );
        assert!(progress_enabled(ResponseMode::Human));
        assert!(!progress_enabled(ResponseMode::Quiet));
    }
}
