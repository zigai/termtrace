use super::*;

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
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    writeln!(stderr, "termtrace: {message}").into_diagnostic()?;
    return Ok(());
}

pub(super) fn write_follow_stdout(output: &[u8]) -> Result<bool> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    return match stdout
        .write_all(output)
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error).into_diagnostic(),
    };
}

pub(super) fn write_follow_json_line(value: &impl serde::Serialize) -> Result<bool> {
    let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
    encoded.push(b'\n');
    return write_follow_stdout(&encoded);
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
            total,
            returned,
            truncated: end < total,
            next_offset,
        },
        partial,
    );
}

pub(super) fn write_json_line(value: &impl serde::Serialize) -> Result<ExitCode> {
    let mut encoded = serde_json::to_vec(value).into_diagnostic()?;
    encoded.push(b'\n');
    return write_stdout(encoded);
}

pub(super) fn write_stdout(output: impl AsRef<[u8]>) -> Result<ExitCode> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    match stdout
        .write_all(output.as_ref())
        .and_then(|()| return stdout.flush())
    {
        Ok(()) => return Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            return Ok(ExitCode::SUCCESS);
        }
        Err(error) => return Err(error).into_diagnostic(),
    }
}
