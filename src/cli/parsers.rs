use super::*;

pub(super) fn parse_row_predicate(value: &str) -> Result<(u16, String)> {
    let (row, text) = value
        .split_once(':')
        .ok_or_else(|| return miette::miette!("row predicate must use ROW:TEXT"))?;
    let row = row
        .parse::<u16>()
        .map_err(|_| return miette::miette!("row predicate ROW must be a zero-based integer"))?;
    return Ok((row, text.to_owned()));
}

pub(super) fn parse_region_predicate(value: &str) -> Result<(ScreenRegion, String)> {
    let (bounds, text) = value.split_once(':').ok_or_else(|| {
        return miette::miette!("region predicate must use LEFT,TOP,RIGHT,BOTTOM:TEXT");
    })?;
    let values = bounds
        .split(',')
        .map(|item| {
            return item.parse::<u16>().map_err(|_| {
                return miette::miette!("region bounds must be zero-based integers");
            });
        })
        .collect::<Result<Vec<_>>>()?;
    let [left, top, right, bottom] = values.as_slice() else {
        return Err(miette::miette!(
            "region predicate requires exactly four bounds"
        ));
    };
    return Ok((
        ScreenRegion {
            left: *left,
            top: *top,
            right: *right,
            bottom: *bottom,
        },
        text.to_owned(),
    ));
}

pub(super) fn parse_cursor_position(value: &str) -> Result<(u16, u16)> {
    let (column, row) = value
        .split_once(',')
        .ok_or_else(|| return miette::miette!("cursor position must use COLUMN,ROW"))?;
    let column = column
        .parse::<u16>()
        .map_err(|_| return miette::miette!("cursor COLUMN must be a zero-based integer"))?;
    let row = row
        .parse::<u16>()
        .map_err(|_| return miette::miette!("cursor ROW must be a zero-based integer"))?;
    return Ok((column, row));
}

pub(super) fn parse_state_hash(value: &str) -> StdResult<String, String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(value.to_owned());
    }
    return Err("state hash must contain exactly 64 lowercase hexadecimal characters".to_owned());
}

pub(super) fn parse_event_type(value: &str) -> StdResult<String, String> {
    const EVENT_TYPES: &[&str] = &[
        "output",
        "input_intent",
        "input_result",
        "resize_intent",
        "resize_result",
        "signal_intent",
        "signal_result",
        "marker",
        "query_response",
        "lifecycle",
        "exit",
        "host_error",
    ];
    if EVENT_TYPES.contains(&value) {
        return Ok(value.to_owned());
    }
    return Err(format!(
        "unknown event type `{value}`; expected one of {}",
        EVENT_TYPES.join(",")
    ));
}

pub(super) fn parse_capability_id(value: &str) -> StdResult<String, String> {
    if CAPABILITY_IDS.contains(&value) {
        return Ok(value.to_owned());
    }
    return Err(format!(
        "unknown capability `{value}`; expected one of {}",
        CAPABILITY_IDS.join(",")
    ));
}

pub(super) fn parse_diagnostic_check(value: &str) -> StdResult<DiagnosticCheck, String> {
    return match value {
        "full-redraw" => Ok(DiagnosticCheck::FullRedraw),
        "transient" | "flicker" => Ok(DiagnosticCheck::Transient),
        "dirty" | "stale-cells" => Ok(DiagnosticCheck::Dirty),
        "cursor" => Ok(DiagnosticCheck::Cursor),
        "modes" => Ok(DiagnosticCheck::Modes),
        "palette" => Ok(DiagnosticCheck::Palette),
        "query-loop" => Ok(DiagnosticCheck::QueryLoop),
        _ => Err(format!(
            "unknown diagnostic check `{value}`; expected full-redraw, transient, dirty, cursor, modes, palette, or query-loop"
        )),
    };
}

pub(super) fn parse_pace_duration(value: &str) -> StdResult<Duration, String> {
    let nanoseconds = parse_time_offset_ns(value)?;
    if nanoseconds > 60_000_000_000 {
        return Err("pace must not exceed 60s".to_owned());
    }
    return Ok(Duration::from_nanos(nanoseconds));
}

pub(super) fn parse_scenario_jobs(value: &str) -> StdResult<usize, String> {
    let jobs = parse_positive_usize(value)?;
    if jobs > 64 {
        return Err("scenario jobs must be between 1 and 64".to_owned());
    }
    return Ok(jobs);
}

pub(super) fn parse_positive_usize(value: &str) -> StdResult<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_error| return "value must be a positive integer".to_owned())?;
    if parsed == 0 {
        return Err("value must be greater than zero".to_owned());
    }
    return Ok(parsed);
}

pub(super) fn parse_page_limit(value: &str) -> StdResult<usize, String> {
    let limit = parse_positive_usize(value)?;
    if limit > 10_000 {
        return Err("page limit must be between 1 and 10000".to_owned());
    }
    return Ok(limit);
}

pub(super) fn parse_cell(value: &str) -> StdResult<(u16, u16), String> {
    let (column, row) = value
        .split_once(',')
        .ok_or_else(|| return "cell must use zero-based COLUMN,ROW syntax".to_owned())?;
    let column = column
        .parse::<u16>()
        .map_err(|_| return "cell column must be an unsigned 16-bit integer".to_owned())?;
    let row = row
        .parse::<u16>()
        .map_err(|_| return "cell row must be an unsigned 16-bit integer".to_owned())?;
    return Ok((column, row));
}

pub(super) fn parse_provenance_rule(value: &str) -> StdResult<(String, ProvenanceCapture), String> {
    let Some((name, capture)) = value.split_once('=') else {
        return Err("expected NAME=value|hash|name|not-captured".to_owned());
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| return byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err("environment name must contain only ASCII letters, digits, or '_'".to_owned());
    }
    let capture = match capture {
        "value" => ProvenanceCapture::Value,
        "hash" => ProvenanceCapture::Hash,
        "name" => ProvenanceCapture::Name,
        "not-captured" => ProvenanceCapture::NotCaptured,
        _ => return Err("capture must be value, hash, name, or not-captured".to_owned()),
    };
    return Ok((name.to_owned(), capture));
}

pub(super) fn parse_capture_target(value: &str) -> StdResult<CaptureTarget, String> {
    let Some((width, height)) = value.split_once(['x', 'X']) else {
        return Err("viewport must use WIDTHxHEIGHT, for example 1920x1080".to_owned());
    };
    let width = width
        .parse::<u16>()
        .map_err(|error| return format!("invalid viewport width: {error}"))?;
    let height = height
        .parse::<u16>()
        .map_err(|error| return format!("invalid viewport height: {error}"))?;
    if width == 0 || height == 0 {
        return Err("viewport dimensions must be non-zero".to_owned());
    }
    return Ok(CaptureTarget { width, height });
}

pub(super) fn parse_font_size(value: &str) -> StdResult<String, String> {
    let size = value
        .parse::<f32>()
        .map_err(|error| return format!("invalid font size: {error}"))?;
    if !size.is_finite() || size <= 0.0 {
        return Err("font size must be a positive finite number".to_owned());
    }
    return Ok(value.to_owned());
}

pub(super) fn parse_rgb_color(value: &str) -> StdResult<String, String> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err("color must use #RRGGBB syntax".to_owned());
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| return byte.is_ascii_hexdigit()) {
        return Err("color must use #RRGGBB syntax".to_owned());
    }
    return Ok(format!("#{}", hex.to_ascii_lowercase()));
}
