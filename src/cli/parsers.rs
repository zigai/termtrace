use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use std::{ffi::OsStr, path::PathBuf, result::Result as StdResult, time::Duration};
pub(super) use termtrace_engine::timeline::parse_time_offset_ns;
use termtrace_model::{
    execution::provenance::ProvenanceCapture, execution::session::CaptureTarget,
    execution::visual_profile::VisualProfile, protocol::control::ScreenPredicate,
    terminal::analysis::DiagnosticCheck, terminal::screen::ScreenRegion,
};
use termtrace_sdk::defaults;

pub(super) fn parse_row_predicate(value: &str) -> Result<(u16, String)> {
    let (row, text) = value
        .split_once(':')
        .ok_or_else(|| return miette::miette!("row predicate must use ROW:TEXT"))?;

    let row = row
        .parse::<u16>()
        .into_diagnostic()
        .wrap_err("row predicate ROW must be a zero-based integer")?;

    return Ok((row, text.to_owned()));
}

pub(super) fn parse_region_predicate(value: &str) -> Result<(ScreenRegion, String)> {
    let (bounds, text) = value.split_once(':').ok_or_else(|| {
        return miette::miette!("region predicate must use LEFT,TOP,RIGHT,BOTTOM:TEXT");
    })?;

    let values = bounds
        .split(',')
        .map(|item| {
            return item
                .parse::<u16>()
                .into_diagnostic()
                .wrap_err("region bounds must be zero-based integers");
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
        .into_diagnostic()
        .wrap_err("cursor COLUMN must be a zero-based integer")?;

    let row = row
        .parse::<u16>()
        .into_diagnostic()
        .wrap_err("cursor ROW must be a zero-based integer")?;

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

pub(super) fn parse_diagnostic_check(value: &str) -> StdResult<DiagnosticCheck, String> {
    return match value {
        "full-redraw" => Ok(DiagnosticCheck::FullRedraw),
        "transient" | "flicker" => Ok(DiagnosticCheck::Transient),
        "dirty" | "stale-cells" => Ok(DiagnosticCheck::Dirty),
        "cursor" => Ok(DiagnosticCheck::Cursor),
        "modes" => Ok(DiagnosticCheck::Modes),
        "palette" => Ok(DiagnosticCheck::Palette),
        "query-loop" => Ok(DiagnosticCheck::QueryLoop),
        "effects" => Ok(DiagnosticCheck::Effects),
        _ => Err(format!(
            "unknown diagnostic check `{value}`; expected full-redraw, transient, dirty, cursor, modes, palette, query-loop, or effects"
        )),
    };
}

fn parse_duration(value: &str, domain: &str) -> StdResult<Duration, String> {
    let nanoseconds =
        parse_time_offset_ns(value).map_err(|error| return format!("{domain} {error}"))?;

    return Ok(Duration::from_nanos(nanoseconds));
}

pub(super) fn parse_pace_duration(value: &str) -> StdResult<Duration, String> {
    let duration = parse_duration(value, "pace duration")?;
    if duration > Duration::from_secs(60) {
        return Err("pace duration must not exceed 60s".to_owned());
    }
    return Ok(duration);
}

pub(super) fn parse_stability_duration(value: &str) -> StdResult<Duration, String> {
    return parse_duration(value, "stability duration");
}

/// Parse an idle interval; a flag given without a value selects the shared default.
pub(super) fn parse_idle_interval(value: &str) -> StdResult<Duration, String> {
    if value.is_empty() {
        return Ok(defaults::IDLE_STABLE_FOR);
    }
    return parse_stability_duration(value);
}

pub(super) fn parse_wait_deadline_duration(value: &str) -> StdResult<Duration, String> {
    return parse_duration(value, "wait deadline");
}

pub(super) fn parse_media_hold_duration(value: &str) -> StdResult<Duration, String> {
    return parse_duration(value, "media hold duration");
}

/// Delivery-specific rate limits are checked before session access by the video route.
pub(super) fn parse_derivative_video_fps(value: &str) -> StdResult<u16, String> {
    return match value.parse::<u16>() {
        Ok(fps @ 1..=120) => Ok(fps),
        _ => Err("video FPS must be between 1 and 120 (GIF at most 100)".to_owned()),
    };
}

pub(super) fn parse_video_fps(value: &str) -> StdResult<u16, String> {
    return match value.parse::<u16>() {
        Ok(fps @ (24 | 30 | 60)) => Ok(fps),
        _ => Err("video FPS must be 24, 30, or 60".to_owned()),
    };
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
    return Ok(CaptureTarget::new(width, height));
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

pub(super) fn parse_visual_profile(value: &str) -> StdResult<VisualProfile, String> {
    let profile: VisualProfile = serde_json::from_str(value)
        .map_err(|error| return format!("invalid visual profile: {error}"))?;

    return profile
        .normalized()
        .map_err(|error| return error.to_string());
}

pub(super) fn parse_bounded_screen_regex(value: &str) -> StdResult<String, String> {
    let predicate = ScreenPredicate::MatchesRegex {
        pattern: value.to_owned(),
    };

    predicate
        .compile()
        .map_err(|error| return error.to_string())?;

    return Ok(value.to_owned());
}

pub(super) fn parse_video_output_path(value: &str) -> StdResult<PathBuf, String> {
    let path = PathBuf::from(value);
    if !["mp4", "gif", "webm"]
        .iter()
        .any(|ext| return path.extension() == Some(OsStr::new(ext)))
    {
        return Err("video output must use lowercase `.mp4`, `.gif` or `.webm`".to_owned());
    }
    return Ok(path);
}

#[cfg(test)]
mod tests {
    use super::{
        parse_media_hold_duration, parse_pace_duration, parse_stability_duration,
        parse_wait_deadline_duration,
    };
    use std::time::Duration;

    #[test]
    fn duration_parsers_enforce_domain_limits() {
        assert_eq!(parse_pace_duration("60s"), Ok(Duration::from_secs(60)));
        assert!(parse_pace_duration("61s").is_err());
        assert_eq!(parse_stability_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(
            parse_wait_deadline_duration("2m"),
            Ok(Duration::from_secs(120))
        );
        assert_eq!(
            parse_media_hold_duration("01:30"),
            Ok(Duration::from_secs(90))
        );
        assert!(parse_stability_duration("invalid").is_err());
        assert!(parse_wait_deadline_duration("invalid").is_err());
        assert!(parse_media_hold_duration("invalid").is_err());
    }
}

#[cfg(test)]
mod derivative_output_tests {
    #[test]
    fn only_lowercase_supported_video_extensions_are_accepted() {
        assert_eq!(super::parse_derivative_video_fps("25"), Ok(25));
        assert_eq!(super::parse_derivative_video_fps("100"), Ok(100));
        assert!(super::parse_derivative_video_fps("0").is_err());
        assert!(super::parse_derivative_video_fps("121").is_err());
        for path in ["demo.mp4", "demo.gif", "demo.webm"] {
            assert!(super::parse_video_output_path(path).is_ok());
        }

        for path in ["demo.GIF", "demo.avi", "demo", "demo.gif.tmp", "demo.WEBM"] {
            assert!(super::parse_video_output_path(path).is_err());
        }
    }
}
