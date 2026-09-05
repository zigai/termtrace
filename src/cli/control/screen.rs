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
    if arguments.since.as_deref() == Some(observation.state_hash.as_str()) {
        let output = ScreenResponse {
            v: 1,
            ok: true,
            response_type: "screen",
            session: &session,
            lifecycle,
            state: &observation.state_hash,
            quality: observation.capture_quality,
            size: [observation.frame.cols, observation.frame.rows],
            ended: observation.source_completed,
            unchanged: Some(true),
            screen: None,
        };
        return write_json_line(response, &output);
    }
    let screen = match arguments.screen {
        ScreenEncoding::Exact => Some(serde_json::Value::String(observation.frame.viewport_text)),
        ScreenEncoding::Text => Some(serde_json::Value::String(
            observation
                .frame
                .viewport_text
                .trim_end_matches(|character: char| {
                    return character == ' ' || character == '\n' || character == '\r';
                })
                .to_owned(),
        )),
        ScreenEncoding::Sparse => Some(sparse_screen(&observation.frame.viewport_text)),
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
            Some(diff_screen(
                &prior.frame.viewport_text,
                &observation.frame.viewport_text,
                since,
            ))
        }
        ScreenEncoding::None => None,
    };
    let output = ScreenResponse {
        v: 1,
        ok: true,
        response_type: "screen",
        session: &session,
        lifecycle,
        state: &observation.state_hash,
        quality: observation.capture_quality,
        size: [observation.frame.cols, observation.frame.rows],
        ended: observation.source_completed,
        unchanged: None,
        screen,
    };
    return write_json_line(response, &output);
}

#[derive(serde::Serialize)]
struct ScreenResponse<'response> {
    v: u32,
    ok: bool,
    #[serde(rename = "type")]
    response_type: &'static str,
    session: &'response Path,
    lifecycle: RegistryLifecycle,
    state: &'response str,
    quality: CaptureQuality,
    size: [u16; 2],
    ended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unchanged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    screen: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
pub(super) struct SparseScreenResponse<'viewport> {
    encoding: &'static str,
    rows: Vec<(usize, &'viewport str)>,
    blank_rows_omitted: usize,
}

#[derive(serde::Serialize)]
pub(super) struct DiffScreenResponse<'viewport> {
    encoding: &'static str,
    since: String,
    rows: Vec<(usize, &'viewport str)>,
}

pub(super) fn sparse_screen(viewport_text: &str) -> serde_json::Value {
    let rows: Vec<(usize, &str)> = viewport_text
        .lines()
        .enumerate()
        .filter_map(|(row, text)| {
            let text = text.trim_end();
            return (!text.is_empty()).then(|| return (row, text));
        })
        .collect();
    let total_rows = viewport_text.lines().count();
    let response = SparseScreenResponse {
        encoding: "rows",
        blank_rows_omitted: total_rows.saturating_sub(rows.len()),
        rows,
    };
    #[expect(
        clippy::expect_used,
        reason = "SparseScreenResponse contains only primitive usize and string slices which cannot fail JSON serialization"
    )]
    return serde_json::to_value(&response).expect("sparse screen serialization");
}

pub(super) fn diff_screen(before: &str, after: &str, since: String) -> serde_json::Value {
    let before_rows: Vec<&str> = before.lines().collect();
    let after_rows: Vec<&str> = after.lines().collect();
    let row_count = before_rows.len().max(after_rows.len());
    let rows: Vec<(usize, &str)> = (0..row_count)
        .filter_map(|row| {
            let old = before_rows.get(row).copied().unwrap_or_default();
            let new = after_rows.get(row).copied().unwrap_or_default();
            return (old != new).then(|| return (row, new.trim_end()));
        })
        .collect();
    let response = DiffScreenResponse {
        encoding: "diff_rows",
        since,
        rows,
    };
    #[expect(
        clippy::expect_used,
        reason = "DiffScreenResponse contains only String and string slices which cannot fail JSON serialization"
    )]
    return serde_json::to_value(&response).expect("diff screen serialization");
}
