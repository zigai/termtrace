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

fn value_string(value: &serde_json::Value, key: &str) -> Option<String> {
    return value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
}

fn value_u64(value: &serde_json::Value, key: &str) -> u64 {
    return value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
}

pub(crate) fn human_action_summary(value: &serde_json::Value) -> String {
    let input = value.get("input").unwrap_or(&serde_json::Value::Null);
    let effect = input
        .get("effect")
        .and_then(|effect| return value_string(effect, "kind"))
        .unwrap_or_else(|| return "unknown".to_owned());

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
        "Action: mutation effect={effect}, atoms={atoms}, bytes={bytes}; postcondition {postcondition_kind} matched={matched}, outcome={outcome}; session={session}\n"
    );
}

pub(crate) fn human_last_action_summary(report: &LastActionReport) -> Result<String> {
    let status = stable_enum_label(&report.action.status)?;

    return Ok(format!(
        "Last action: status={status}, bytes={}, changed={} cells across {} rows, warnings={}\n",
        report.action.bytes_written,
        report.cells_changed,
        report.rows_changed.len(),
        report.warnings.len()
    ));
}

pub(crate) fn human_diagnostics_summary(report: &LastActionDiagnosticsReport) -> String {
    if report.findings.is_empty() {
        return "Diagnostics: no findings for the last action\n".to_owned();
    }

    let codes = report
        .findings
        .iter()
        .map(|finding| return finding.code.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    return format!(
        "Diagnostics: {} finding(s): {codes}\n",
        report.findings.len()
    );
}
