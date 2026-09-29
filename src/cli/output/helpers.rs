use super::*;

pub(in crate::cli) fn stable_enum_label(value: &impl serde::Serialize) -> Result<String> {
    let serialized = serde_json::to_value(value).into_diagnostic()?;
    let Some(label) = serialized.as_str() else {
        return Err(miette::miette!(
            "human output label did not serialize as a string"
        ));
    };

    return Ok(label.to_owned());
}

pub(in crate::cli) fn optional_display(value: Option<impl Display>) -> String {
    return value.map_or_else(|| return "-".to_owned(), |value| return value.to_string());
}

pub(in crate::cli) fn id_list(values: &[u64]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }

    let mut output = String::new();
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }

        #[expect(
            clippy::let_underscore_must_use,
            reason = "`fmt::Write` for `String` never fails, as documented on `format!`, and `drop_non_drop` rejects dropping the unit result"
        )]
        let _ = write!(output, "{value}");
    }

    return output;
}

pub(in crate::cli) fn quoted_text(value: &str) -> Result<String> {
    return serde_json::to_string(value).into_diagnostic();
}

pub(in crate::cli) fn projection_warning(quality: OutputProjectionQuality) -> Option<&'static str> {
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

pub(in crate::cli) fn write_projection_warning(
    response: &ResponseWriter,
    message: &str,
) -> Result<()> {
    if response.mode() == ResponseMode::Quiet {
        return Ok(());
    }

    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    writeln!(stderr, "termtrace: {message}").into_diagnostic()?;

    return Ok(());
}

pub(in crate::cli) fn row_text(row: &RowState) -> String {
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
