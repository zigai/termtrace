use termtrace_evidence::event_log::EventLog;
use termtrace_model::{execution::event::Event, terminal::replay::SourceRange};

use crate::{Error, Result};

/// Resolve optional CLI bounds against a canonical event-log endpoint.
///
/// `start` is inclusive and defaults to zero. `end` is exclusive and defaults
/// to one nanosecond after the final event so that the final event is included.
/// An entirely absent request selects the complete session.
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
pub(super) fn range_error(reason: impl Into<String>) -> Error {
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
}
