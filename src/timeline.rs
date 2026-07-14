//! Validated source-time ranges shared by semantic and native replay.

use std::result::Result as StdResult;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    event::Event,
    event_log::EventLog,
    ghostty_vt::DirtyState,
    replay::replay,
    vt_framer::{VtToken, frame_event_log},
};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MINUTE: u64 = 60 * NANOS_PER_SECOND;
const NANOS_PER_HOUR: u64 = 60 * NANOS_PER_MINUTE;

/// One ordered, source-bound timeline item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntry {
    /// Canonical session offset.
    pub monotonic_ns: u64,
    /// Responsible canonical event, absent for the initial semantic state.
    pub event_seq: Option<u64>,
    /// Timeline item payload.
    pub item: TimelineItem,
}

/// Concise canonical or semantic item in a diagnostic timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineItem {
    /// A distinct Ghostty semantic state transition.
    SemanticState {
        /// Dense semantic state identifier.
        state_id: u64,
        /// Byte offset within the responsible output event.
        event_byte_offset: Option<u64>,
        /// Complete normalized semantic state hash.
        state_hash: String,
        /// Whether the state was eligible for presentation.
        presented: bool,
        /// Ghostty's dirty classification.
        ghostty_dirty: DirtyState,
        /// Rows whose semantic cells changed.
        changed_rows: Vec<u16>,
        /// Number of complete semantic cell values changed.
        changed_cells: u64,
        /// Whether cursor state changed.
        cursor_changed: bool,
        /// Whether terminal modes changed.
        modes_changed: bool,
        /// Whether terminal metadata changed.
        metadata_changed: bool,
    },
    /// A non-output canonical event retained verbatim.
    CanonicalEvent {
        /// Exact typed canonical event.
        event: Event,
    },
    /// One lossless explanatory VT frame.
    VtToken {
        /// Exact token and canonical source segments.
        token: VtToken,
    },
}

/// Build a concise ordered timeline from validated canonical events.
///
/// Output byte events are represented by their resulting semantic states;
/// mutation results are omitted from the default concise view. Other events
/// retain their exact typed canonical payload.
///
/// # Errors
///
/// Returns an error when semantic replay fails.
pub fn build_timeline(
    log: &EventLog,
    include_hidden_states: bool,
    include_controls: bool,
    range: Option<SourceRange>,
) -> Result<Vec<TimelineEntry>> {
    let replay = replay(log)?;
    let mut entries = Vec::new();
    for state in replay.states {
        if !include_hidden_states && !state.presented {
            continue;
        }
        if range.is_some_and(|selection| return !selection.contains(state.monotonic_ns)) {
            continue;
        }
        let changed_rows = state
            .delta
            .rows
            .iter()
            .map(|row| return row.row)
            .collect::<Vec<_>>();
        let changed_cells = state.delta.rows.iter().fold(0_u64, |count, row| {
            return count.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
        });
        entries.push(TimelineEntry {
            monotonic_ns: state.monotonic_ns,
            event_seq: state.event_seq,
            item: TimelineItem::SemanticState {
                state_id: state.state_id,
                event_byte_offset: state.event_byte_offset,
                state_hash: state.state_hash,
                presented: state.presented,
                ghostty_dirty: state.ghostty_dirty,
                changed_rows,
                changed_cells,
                cursor_changed: state.delta.cursor.is_some(),
                modes_changed: state.delta.modes.is_some(),
                metadata_changed: state.delta.metadata.is_some(),
            },
        });
    }
    for record in &log.events {
        if range.is_some_and(|selection| return !selection.contains(record.monotonic_ns))
            || !timeline_event(&record.event)
        {
            continue;
        }
        entries.push(TimelineEntry {
            monotonic_ns: record.monotonic_ns,
            event_seq: Some(record.seq),
            item: TimelineItem::CanonicalEvent {
                event: record.event.clone(),
            },
        });
    }
    if include_controls {
        for token in frame_event_log(log)? {
            let Some(first) = token.source.first() else {
                continue;
            };
            if range.is_some_and(|selection| {
                return !token
                    .source
                    .iter()
                    .any(|segment| return selection.contains(segment.monotonic_ns));
            }) {
                continue;
            }
            entries.push(TimelineEntry {
                monotonic_ns: first.monotonic_ns,
                event_seq: Some(first.event_seq),
                item: TimelineItem::VtToken { token },
            });
        }
    }
    entries.sort_by_key(|entry| {
        let state_order = match &entry.item {
            TimelineItem::CanonicalEvent { .. } => 0,
            TimelineItem::VtToken { .. } => 1,
            TimelineItem::SemanticState { .. } => 2,
        };
        return (
            entry.monotonic_ns,
            entry.event_seq.unwrap_or(0),
            state_order,
            match &entry.item {
                TimelineItem::SemanticState {
                    event_byte_offset, ..
                } => event_byte_offset.unwrap_or(0),
                TimelineItem::VtToken { token } => token
                    .source
                    .first()
                    .map_or(0, |segment| return segment.start_byte_offset),
                TimelineItem::CanonicalEvent { .. } => 0,
            },
        );
    });
    return Ok(entries);
}

fn timeline_event(event: &Event) -> bool {
    return !matches!(
        event,
        Event::Output { .. }
            | Event::InputResult { .. }
            | Event::ResizeResult { .. }
            | Event::SignalResult { .. }
    );
}

/// A non-empty half-open interval on the canonical monotonic timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRange {
    /// Inclusive source timestamp in nanoseconds.
    pub start_monotonic_ns: u64,
    /// Exclusive source timestamp in nanoseconds.
    pub end_monotonic_ns: u64,
}

impl SourceRange {
    /// Duration of the selected source interval.
    pub fn duration_ns(self) -> u64 {
        return self.end_monotonic_ns - self.start_monotonic_ns;
    }

    /// Whether a timestamp belongs to this half-open interval.
    pub fn contains(self, monotonic_ns: u64) -> bool {
        return monotonic_ns >= self.start_monotonic_ns && monotonic_ns < self.end_monotonic_ns;
    }

    /// Stable suffix used by range-specific no-replace artifact directories.
    pub fn artifact_suffix(self) -> String {
        return format!(
            "range-{}-{}",
            self.start_monotonic_ns, self.end_monotonic_ns
        );
    }
}

/// Resolve optional CLI bounds against a canonical event-log endpoint.
///
/// `start` is inclusive and defaults to zero. `end` is exclusive and defaults
/// to one nanosecond after the final event so that the final event is included.
/// An entirely absent request preserves legacy full-session behavior.
///
/// # Errors
///
/// Returns an error for reversed, empty, overflowing, or out-of-bounds ranges.
pub fn resolve_source_range(
    start: Option<u64>,
    end: Option<u64>,
    final_monotonic_ns: u64,
) -> Result<Option<SourceRange>> {
    if start.is_none() && end.is_none() {
        return Ok(None);
    }
    // A half-open range ending at the final timestamp would exclude the final
    // event. Advance by one nanosecond so an omitted end still means "through
    // the complete session" without giving ranged replay a special last-event case.
    let source_end = final_monotonic_ns.checked_add(1).ok_or_else(|| {
        return range_error("source timeline ends at u64::MAX and cannot form an exclusive bound");
    })?;
    let start = start.unwrap_or(0);
    let end = end.unwrap_or(source_end);
    if start >= end {
        return Err(range_error(format!(
            "source range must be non-empty with FROM < TO, got {start}..{end}"
        )));
    }
    if start > final_monotonic_ns {
        return Err(range_error(format!(
            "source range starts at {start} ns after the final event at {final_monotonic_ns} ns"
        )));
    }
    if end > source_end {
        return Err(range_error(format!(
            "source range ends at {end} ns after the exclusive source endpoint {source_end} ns"
        )));
    }
    return Ok(Some(SourceRange {
        start_monotonic_ns: start,
        end_monotonic_ns: end,
    }));
}

/// Resolve optional numeric or marker bounds into canonical timestamps.
///
/// Marker labels must identify exactly one canonical marker. Start markers are
/// inclusive and end markers are exclusive, matching numeric range semantics.
///
/// # Errors
///
/// Returns an error for mixed selectors, missing or duplicate marker labels,
/// or an invalid resulting source range.
pub fn resolve_source_selection(
    log: &EventLog,
    start: Option<u64>,
    end: Option<u64>,
    from_marker: Option<&str>,
    to_marker: Option<&str>,
) -> Result<Option<SourceRange>> {
    if start.is_some() && from_marker.is_some() {
        return Err(range_error(
            "choose either `--from` or `--from-marker`, not both",
        ));
    }
    if end.is_some() && to_marker.is_some() {
        return Err(range_error(
            "choose either `--to` or `--to-marker`, not both",
        ));
    }
    let start = match from_marker {
        Some(label) => Some(resolve_marker(log, label)?),
        None => start,
    };
    let end = match to_marker {
        Some(label) => Some(resolve_marker(log, label)?),
        None => end,
    };
    let final_monotonic_ns = log
        .events
        .last()
        .map_or(0, |record| return record.monotonic_ns);
    return resolve_source_range(start, end, final_monotonic_ns);
}

fn resolve_marker(log: &EventLog, label: &str) -> Result<u64> {
    if label.is_empty() {
        return Err(range_error("marker label must not be empty"));
    }
    let matches = log
        .events
        .iter()
        .filter(|record| {
            return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => return Ok(record.monotonic_ns),
        [] => {
            return Err(range_error(format!(
                "marker `{label}` does not exist in the canonical event log"
            )));
        }
        _ => {
            return Err(range_error(format!(
                "marker `{label}` occurs {} times; marker range selectors require a unique label",
                matches.len()
            )));
        }
    }
}

/// Parse a human-readable non-negative source offset into nanoseconds.
///
/// Accepted forms are a decimal number with an optional `ns`, `us`, `ms`,
/// `s`, `m`, or `h` suffix (bare numbers are seconds), plus `MM:SS.fraction`
/// and `HH:MM:SS.fraction` clock notation.
pub fn parse_time_offset_ns(value: &str) -> StdResult<u64, String> {
    if value.is_empty() {
        return Err("time offset cannot be empty".to_owned());
    }
    if value.contains(':') {
        return parse_clock_offset(value);
    }
    let (number, scale) = [
        ("ns", 1),
        ("us", 1_000),
        ("ms", 1_000_000),
        ("s", NANOS_PER_SECOND),
        ("m", NANOS_PER_MINUTE),
        ("h", NANOS_PER_HOUR),
    ]
    .into_iter()
    .find_map(|(suffix, scale)| {
        return value
            .strip_suffix(suffix)
            .map(|number| return (number, scale));
    })
    .unwrap_or((value, NANOS_PER_SECOND));
    return parse_scaled_decimal(number, scale)
        .map_err(|reason| return format!("invalid time offset `{value}`: {reason}"));
}

fn parse_clock_offset(value: &str) -> StdResult<u64, String> {
    let parts = value.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, parse_integer(minutes, "minutes")?, *seconds),
        [hours, minutes, seconds] => (
            parse_integer(hours, "hours")?,
            parse_integer(minutes, "minutes")?,
            *seconds,
        ),
        _ => {
            return Err(format!(
                "invalid time offset `{value}`: use MM:SS or HH:MM:SS"
            ));
        }
    };
    if parts.len() == 3 && minutes >= 60 {
        return Err(format!(
            "invalid time offset `{value}`: minutes must be below 60 in HH:MM:SS"
        ));
    }
    let seconds_ns = parse_scaled_decimal(seconds, NANOS_PER_SECOND)
        .map_err(|reason| return format!("invalid time offset `{value}`: {reason}"))?;
    if seconds_ns >= NANOS_PER_MINUTE {
        return Err(format!(
            "invalid time offset `{value}`: seconds must be below 60"
        ));
    }
    let hours_ns = hours
        .checked_mul(NANOS_PER_HOUR)
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"))?;
    let minutes_ns = minutes
        .checked_mul(NANOS_PER_MINUTE)
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"))?;
    return hours_ns
        .checked_add(minutes_ns)
        .and_then(|total| return total.checked_add(seconds_ns))
        .ok_or_else(|| return format!("invalid time offset `{value}`: value overflows"));
}

fn parse_scaled_decimal(number: &str, scale: u64) -> StdResult<u64, String> {
    let (whole, fraction) = number
        .split_once('.')
        .map_or((number, None), |(whole, fraction)| {
            return (whole, Some(fraction));
        });
    if whole.is_empty() || number.matches('.').count() > 1 {
        return Err("expected a non-negative decimal number".to_owned());
    }
    let whole = parse_integer(whole, "value")?;
    let whole_ns = whole
        .checked_mul(scale)
        .ok_or_else(|| return "value overflows nanoseconds".to_owned())?;
    let Some(fraction) = fraction else {
        return Ok(whole_ns);
    };
    if fraction.is_empty() || fraction.len() > 18 {
        return Err("fraction must contain between 1 and 18 digits".to_owned());
    }
    let numerator = parse_integer(fraction, "fraction")?;
    let exponent =
        u32::try_from(fraction.len()).map_err(|_| return "fraction is too precise".to_owned())?;
    let denominator = 10_u64
        .checked_pow(exponent)
        .ok_or_else(|| return "fraction is too precise".to_owned())?;
    let scaled = u128::from(numerator) * u128::from(scale);
    if scaled % u128::from(denominator) != 0 {
        return Err("fraction is more precise than one nanosecond".to_owned());
    }
    let fraction_ns = u64::try_from(scaled / u128::from(denominator))
        .map_err(|_| return "fraction overflows nanoseconds".to_owned())?;
    return whole_ns
        .checked_add(fraction_ns)
        .ok_or_else(|| return "value overflows nanoseconds".to_owned());
}

fn parse_integer(value: &str, component: &str) -> StdResult<u64, String> {
    return value
        .parse::<u64>()
        .map_err(|_| return format!("{component} must be a non-negative integer"));
}

fn range_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_half_open_bounds() {
        assert_eq!(
            resolve_source_range(None, None, 100).expect("full range"),
            None
        );
        assert_eq!(
            resolve_source_range(Some(20), None, 100).expect("open end"),
            Some(SourceRange {
                start_monotonic_ns: 20,
                end_monotonic_ns: 101,
            })
        );
        assert_eq!(
            resolve_source_range(None, Some(80), 100).expect("open start"),
            Some(SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 80,
            })
        );
    }

    #[test]
    fn rejects_empty_and_out_of_bounds_ranges() {
        assert!(resolve_source_range(Some(10), Some(10), 100).is_err());
        assert!(resolve_source_range(Some(101), None, 100).is_err());
        assert!(resolve_source_range(None, Some(102), 100).is_err());
    }

    #[test]
    fn parses_units_and_clock_offsets_exactly() {
        assert_eq!(parse_time_offset_ns("1.5s"), Ok(1_500_000_000));
        assert_eq!(parse_time_offset_ns("250ms"), Ok(250_000_000));
        assert_eq!(parse_time_offset_ns("2m"), Ok(120_000_000_000));
        assert_eq!(parse_time_offset_ns("01:02.25"), Ok(62_250_000_000));
        assert_eq!(parse_time_offset_ns("01:01:02.25"), Ok(3_662_250_000_000));
        assert_eq!(parse_time_offset_ns("2"), Ok(2_000_000_000));
    }

    #[test]
    fn rejects_invalid_offsets() {
        assert!(parse_time_offset_ns("-1s").is_err());
        assert!(parse_time_offset_ns("1:60").is_err());
        assert!(parse_time_offset_ns("0.1ns").is_err());
        assert!(parse_time_offset_ns("2m30s").is_err());
    }
}
