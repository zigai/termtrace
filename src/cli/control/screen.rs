use super::*;

pub(in crate::cli) fn run_screen(
    response: &mut ResponseWriter,
    arguments: ScreenArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if !matches!(
        resolved.lifecycle,
        RegistryLifecycle::Live | RegistryLifecycle::Completed
    ) {
        return write_session_lifecycle(response, &resolved, "screen", ExitCode::FAILURE);
    }
    let lifecycle = resolved.lifecycle;
    let session = resolved.path;
    let observation = observe_session_with_options(&session, StatePoint::LatestPresented, false)
        .into_diagnostic()?;
    let mut output = serde_json::json!({
        "v": 1,
        "ok": true,
        "type": "screen",
        "session": session,
        "lifecycle": lifecycle,
        "state": observation.state_hash,
        "quality": observation.capture_quality,
        "size": [observation.frame.cols, observation.frame.rows],
        "ended": observation.source_completed,
    });
    if arguments.since.as_deref() == Some(observation.state_hash.as_str()) {
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("screen response is not an object"))?
            .insert("unchanged".to_owned(), serde_json::Value::Bool(true));
        return write_json_line(response, &output);
    }
    let screen = match arguments.screen {
        ScreenEncoding::Exact => serde_json::Value::String(observation.frame.viewport_text),
        ScreenEncoding::Text => serde_json::Value::String(
            observation
                .frame
                .viewport_text
                .trim_end_matches(|character: char| {
                    return character == ' ' || character == '\n' || character == '\r';
                })
                .to_owned(),
        ),
        ScreenEncoding::Sparse => sparse_screen(&observation.frame.viewport_text),
        ScreenEncoding::Diff => {
            let since = arguments
                .since
                .ok_or_else(|| miette::miette!("--screen diff requires --since STATE_HASH"))?;
            let point = StatePoint::StateHash {
                state_hash: since.clone(),
            };
            let prior = if session.join("completion.json").is_file() {
                observe_session_with_options(&session, point, false).into_diagnostic()?
            } else {
                let _ = request(&session, ControlCommand::Status).into_diagnostic()?;
                observe_validated_prefix(&session, point, false).into_diagnostic()?
            };
            diff_screen(
                &prior.frame.viewport_text,
                &observation.frame.viewport_text,
                since,
            )
        }
        ScreenEncoding::None => serde_json::Value::Null,
    };
    if arguments.screen != ScreenEncoding::None {
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("screen response is not an object"))?
            .insert("screen".to_owned(), screen);
    }
    return write_json_line(response, &output);
}

pub(super) fn sparse_screen(viewport_text: &str) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = viewport_text
        .lines()
        .enumerate()
        .filter_map(|(row, text)| {
            let text = text.trim_end();
            return (!text.is_empty()).then(|| serde_json::json!([row, text]));
        })
        .collect();
    let total_rows = viewport_text.lines().count();
    return serde_json::json!({
        "encoding": "rows",
        "rows": rows,
        "blank_rows_omitted": total_rows.saturating_sub(rows.len()),
    });
}

pub(super) fn diff_screen(before: &str, after: &str, since: String) -> serde_json::Value {
    let before_rows: Vec<&str> = before.lines().collect();
    let after_rows: Vec<&str> = after.lines().collect();
    let row_count = before_rows.len().max(after_rows.len());
    let rows: Vec<serde_json::Value> = (0..row_count)
        .filter_map(|row| {
            let old = before_rows.get(row).copied().unwrap_or_default();
            let new = after_rows.get(row).copied().unwrap_or_default();
            return (old != new).then(|| serde_json::json!([row, new.trim_end()]));
        })
        .collect();
    return serde_json::json!({
        "encoding": "diff_rows",
        "since": since,
        "rows": rows,
    });
}
