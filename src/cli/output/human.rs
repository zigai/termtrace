use super::*;

pub(crate) fn human_screen_summary(value: &serde_json::Value) -> String {
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

pub(super) fn human_failure_summary(
    state: &ResponseState,
    error: Option<&miette::Report>,
) -> String {
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

pub(super) fn value_u64(value: &serde_json::Value, key: &str) -> u64 {
    return value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
}

pub(crate) fn human_action_summary(value: &serde_json::Value) -> String {
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

pub(super) fn human_inspect_summary(value: &serde_json::Value) -> String {
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

pub(super) fn human_diagnose_summary(value: &serde_json::Value) -> String {
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

pub(super) fn artifact_path(value: &serde_json::Value) -> String {
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

pub(super) fn human_artifact_summary(
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

pub(crate) fn human_summary(value: &serde_json::Value, command_type: &str) -> String {
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

    if command_type == "inspect" || command_type == "last_action" || command_type == "last-action" {
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
