//! Runner-neutral expectation classification and deterministic frame snapshots.

use crate::locator::{QueryError, QueryResult, ScreenMatch, StyleFilter};
use serde_json::Value;
use std::{error::Error as StdError, fmt};
use termtrace_engine::observation::{
    normalize_screen_frame_cells, normalize_screen_frame_spans, normalize_screen_frame_text,
};
use termtrace_model::{
    terminal::screen::{
        ScreenCell, ScreenCellRow, ScreenFrame, ScreenFrameContent, ScreenFrameError, ScreenRegion,
        ScreenSnapshotOptions, ScreenSpan, ScreenSpanRow, ScreenTextRow, SpanCell,
    },
    terminal::state::{ColorSpec, CursorState, WideKind},
};

const MAX_ASSERTION_CHARS: usize = 512;
const MAX_EXCERPT_ROWS: usize = 8;

/// Deterministic reviewed representation of an already captured exact frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SnapshotProfile {
    /// Geometry, regions, wrap facts, convenience text, and selected cursor/mode facts.
    #[default]
    Text,
    /// Text plus complete lossless appearance spans.
    Spans,
    /// Bounded exact cells including grapheme and authoritative width mapping.
    Cells,
}

/// Stable frame identity included in bounded assertion diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssertionFrameIdentity {
    /// Exact semantic-state hash.
    pub state_hash: String,
    /// Exact rich projection hash.
    pub projection_hash: String,
    /// Whether the actor marked the state presentation eligible.
    pub presented: bool,
}

/// A valid expectation that did not match.
///
/// Only this type is suitable for conversion into a runner's negatable mismatch value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectationMismatchError {
    /// Stable expectation family.
    pub assertion: &'static str,
    /// Bounded expected summary.
    pub expected: String,
    /// Bounded actual summary.
    pub actual: String,
    /// Bounded screen excerpt, never the complete protocol payload.
    pub excerpt: String,
    /// Exact frame identity when a frame was available.
    pub frame: Option<Box<AssertionFrameIdentity>>,
    /// Whether any diagnostic field was truncated.
    pub truncated: bool,
}

impl fmt::Display for ExpectationMismatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(
            f,
            "{}: expected {}; actual {}; frame={:?}\n{}",
            self.assertion, self.expected, self.actual, self.frame, self.excerpt
        );
    }
}

impl StdError for ExpectationMismatchError {}

/// Assertion infrastructure failures that must never become a negatable mismatch.
#[derive(Debug)]
pub enum AssertionError {
    /// Pure query validation or strictness failure.
    Query(QueryError),
    /// Snapshot/detail/resource validation failure.
    Snapshot(ScreenFrameError),
    /// A valid expectation mismatch.
    Mismatch(ExpectationMismatchError),
}

impl fmt::Display for AssertionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query(error) => return error.fmt(f),
            Self::Snapshot(error) => return error.fmt(f),
            Self::Mismatch(error) => return error.fmt(f),
        }
    }
}

impl StdError for AssertionError {}

/// Purely normalize an exact frame with one of the three stable profiles.
///
/// # Errors
/// Returns `ScreenFrameError::Serialization` when `profile` needs span or cell detail that `frame`
/// does not hold, or the normalized frame cannot be serialized.
pub fn normalize_frame(
    frame: &ScreenFrame,
    profile: SnapshotProfile,
    options: ScreenSnapshotOptions,
) -> Result<Value, ScreenFrameError> {
    let mut projected = frame.clone();
    projected.data = data_for_profile(frame, profile)?;
    let mut value = match profile {
        SnapshotProfile::Text => normalize_screen_frame_text(&projected, options),
        SnapshotProfile::Spans => normalize_screen_frame_spans(&projected, options),
        SnapshotProfile::Cells => normalize_screen_frame_cells(&projected, options),
    }?;

    if let Value::Object(fields) = &mut value {
        fields.insert(
            "capture_quality".to_owned(),
            serde_json::json!(frame.capture_quality),
        );
    }

    return Ok(value);
}

/// Produce deterministic pretty JSON suitable for Rust's built-in `assert_eq!` diff.
///
/// # Errors
/// Returns `AssertionError::Snapshot` when the frame cannot be normalized for `profile` or
/// serialized.
pub fn normalized_snapshot(
    frame: &ScreenFrame,
    profile: SnapshotProfile,
    options: ScreenSnapshotOptions,
) -> Result<String, AssertionError> {
    let value = normalize_frame(frame, profile, options).map_err(AssertionError::Snapshot)?;

    return serde_json::to_string_pretty(&value)
        .map_err(ScreenFrameError::serialization)
        .map_err(AssertionError::Snapshot);
}

fn data_for_profile(
    frame: &ScreenFrame,
    profile: SnapshotProfile,
) -> Result<ScreenFrameContent, ScreenFrameError> {
    match (profile, &frame.data) {
        (SnapshotProfile::Text, ScreenFrameContent::Text { rows }) => {
            return Ok(ScreenFrameContent::Text { rows: rows.clone() });
        }
        (SnapshotProfile::Text, ScreenFrameContent::Spans { rows }) => {
            return Ok(ScreenFrameContent::Text {
                rows: rows
                    .iter()
                    .map(|row| {
                        return ScreenTextRow {
                            row: row.row,
                            soft_wrapped: row.soft_wrapped,
                            wrap_continuation: row.wrap_continuation,
                            text: row.text.clone(),
                        };
                    })
                    .collect(),
            });
        }
        (SnapshotProfile::Text, ScreenFrameContent::Cells { rows }) => {
            return Ok(ScreenFrameContent::Text {
                rows: rows
                    .iter()
                    .map(|row| {
                        return ScreenTextRow {
                            row: row.row,
                            soft_wrapped: row.soft_wrapped,
                            wrap_continuation: row.wrap_continuation,
                            text: row.text.clone(),
                        };
                    })
                    .collect(),
            });
        }
        (SnapshotProfile::Spans, ScreenFrameContent::Spans { rows }) => {
            return Ok(ScreenFrameContent::Spans { rows: rows.clone() });
        }
        (SnapshotProfile::Spans, ScreenFrameContent::Cells { rows }) => {
            return Ok(ScreenFrameContent::Spans {
                rows: rows.iter().map(span_row_from_cells).collect(),
            });
        }
        (SnapshotProfile::Cells, ScreenFrameContent::Cells { rows }) => {
            return Ok(ScreenFrameContent::Cells { rows: rows.clone() });
        }
        (SnapshotProfile::Cells, ScreenFrameContent::Spans { rows }) => {
            return Ok(ScreenFrameContent::Cells {
                rows: rows.iter().map(cell_row_from_spans).collect(),
            });
        }
        (SnapshotProfile::Text, ScreenFrameContent::None) => {
            return Ok(ScreenFrameContent::None);
        }
        (
            SnapshotProfile::Spans | SnapshotProfile::Cells,
            ScreenFrameContent::Text { .. } | ScreenFrameContent::None,
        ) => {
            return Err(ScreenFrameError::Serialization);
        }
    }
}

fn span_row_from_cells(row: &ScreenCellRow) -> ScreenSpanRow {
    let mut spans: Vec<ScreenSpan> = Vec::new();
    for cell in &row.cells {
        if let Some(span) = spans.last_mut()
            && span.appearance == cell.appearance
            && span.end_column == cell.column
        {
            span.end_column = cell.column.saturating_add(1);
            span.cell_count = span.cell_count.saturating_add(1);
            append_snapshot_grapheme(&mut span.text, &cell.grapheme, cell.wide_kind);
            span.cells.push(SpanCell {
                grapheme: cell.grapheme.clone(),
                wide_kind: cell.wide_kind,
            });
            continue;
        }

        let mut text = String::new();
        append_snapshot_grapheme(&mut text, &cell.grapheme, cell.wide_kind);
        spans.push(ScreenSpan {
            start_column: cell.column,
            end_column: cell.column.saturating_add(1),
            cell_count: 1,
            text,
            appearance: cell.appearance.clone(),
            cells: vec![SpanCell {
                grapheme: cell.grapheme.clone(),
                wide_kind: cell.wide_kind,
            }],
        });
    }

    return ScreenSpanRow {
        row: row.row,
        soft_wrapped: row.soft_wrapped,
        wrap_continuation: row.wrap_continuation,
        text: row.text.clone(),
        spans,
    };
}

fn cell_row_from_spans(row: &ScreenSpanRow) -> ScreenCellRow {
    let cells = row
        .spans
        .iter()
        .flat_map(|span| {
            return span.cells.iter().enumerate().map(move |(offset, cell)| {
                return ScreenCell {
                    row: row.row,
                    column: span
                        .start_column
                        .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX)),
                    grapheme: cell.grapheme.clone(),
                    wide_kind: cell.wide_kind,
                    appearance: span.appearance.clone(),
                };
            });
        })
        .collect();

    return ScreenCellRow {
        row: row.row,
        soft_wrapped: row.soft_wrapped,
        wrap_continuation: row.wrap_continuation,
        text: row.text.clone(),
        cells,
    };
}

fn append_snapshot_grapheme(text: &mut String, grapheme: &str, wide_kind: WideKind) {
    if matches!(wide_kind, WideKind::SpacerHead | WideKind::SpacerTail) {
        return;
    }

    if grapheme.is_empty() {
        text.push(' ');
        return;
    }

    text.push_str(grapheme);
}

/// Require at least one valid match.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when `result` has no matches.
pub fn assert_exists(result: &QueryResult) -> Result<(), ExpectationMismatchError> {
    if result.count() > 0 {
        return Ok(());
    }
    return Err(mismatch(
        "exists",
        "at least one match",
        "0 matches",
        result.frame(),
    ));
}

/// Require no valid matches.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when `result` has any match.
pub fn assert_not_exists(result: &QueryResult) -> Result<(), ExpectationMismatchError> {
    if result.count() == 0 {
        return Ok(());
    }
    return Err(mismatch(
        "not_exists",
        "0 matches",
        &format!("{} matches", result.count()),
        result.frame(),
    ));
}

/// Require one exact match count.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when `result` does not have exactly `expected` matches.
pub fn assert_count(result: &QueryResult, expected: usize) -> Result<(), ExpectationMismatchError> {
    if result.count() == expected {
        return Ok(());
    }
    return Err(mismatch(
        "count",
        &format!("{expected} matches"),
        &format!("{} matches", result.count()),
        result.frame(),
    ));
}

/// Require literal text to occur in the deterministic presented row stream.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when `expected` is empty or absent from the presented row text.
pub fn assert_screen_text(
    frame: &ScreenFrame,
    expected: &str,
) -> Result<(), ExpectationMismatchError> {
    let text = frame_text(frame);
    if !expected.is_empty() && text.contains(expected) {
        return Ok(());
    }
    return Err(mismatch(
        "screen_text",
        &format!("screen contains {expected:?}"),
        &format!("screen text {text:?}"),
        frame,
    ));
}

/// Require exact text evidence on one already selected match.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when the match text differs from `expected`.
pub fn assert_match_text(
    screen_match: &ScreenMatch,
    expected: &str,
) -> Result<(), ExpectationMismatchError> {
    if screen_match.text.as_ref() == expected {
        return Ok(());
    }
    return Err(mismatch(
        "match_text",
        &format!("text {expected:?}"),
        &format!("text {:?}", screen_match.text),
        &screen_match.frame,
    ));
}

/// Require every covered authoritative head cell to satisfy a complete appearance filter.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when the match covers no authoritative head cells or any
/// covered cell fails `expected`.
pub fn assert_match_style(
    screen_match: &ScreenMatch,
    expected: &StyleFilter,
) -> Result<(), ExpectationMismatchError> {
    let mut covered = 0usize;
    let mut mismatched = 0usize;

    screen_match
        .frame
        .for_each_cell(|metadata, column, _, wide_kind, appearance| {
            if matches!(wide_kind, WideKind::SpacerHead | WideKind::SpacerTail) {
                return;
            }

            let in_match = screen_match.ranges.iter().any(|range| {
                return range.row == metadata.row
                    && column >= range.start_column
                    && column < range.end_column;
            });

            if !in_match {
                return;
            }

            covered += 1;

            if !expected.matches(appearance, wide_kind) {
                mismatched += 1;
            }
        });

    if covered > 0 && mismatched == 0 {
        return Ok(());
    }
    return Err(mismatch(
        "match_style",
        &format!("all covered cells match {expected:?}"),
        &format!("covered {covered}; mismatched {mismatched}"),
        &screen_match.frame,
    ));
}

/// Require the complete exact cursor state.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when the frame's cursor differs from `expected` in any field.
pub fn assert_cursor(
    frame: &ScreenFrame,
    expected: &CursorState,
) -> Result<(), ExpectationMismatchError> {
    if &frame.cursor == expected {
        return Ok(());
    }
    return Err(mismatch(
        "cursor",
        &format!("{expected:?}"),
        &format!("{:?}", frame.cursor),
        frame,
    ));
}

/// Require no native soft-wrap or wrap-continuation facts.
///
/// # Errors
/// Returns an `ExpectationMismatchError` naming the rows that carry soft-wrap or wrap-continuation
/// facts.
pub fn assert_no_wrapped_rows(frame: &ScreenFrame) -> Result<(), ExpectationMismatchError> {
    let wrapped: Vec<u16> = match &frame.data {
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .filter(|row| return row.soft_wrapped || row.wrap_continuation)
            .map(|row| return row.row)
            .collect(),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .filter(|row| return row.soft_wrapped || row.wrap_continuation)
            .map(|row| return row.row)
            .collect(),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .filter(|row| return row.soft_wrapped || row.wrap_continuation)
            .map(|row| return row.row)
            .collect(),
        ScreenFrameContent::None => Vec::new(),
    };

    if wrapped.is_empty() {
        return Ok(());
    }
    return Err(mismatch(
        "no_wrapped_rows",
        "no native wrapped rows",
        &format!("wrapped rows {wrapped:?}"),
        frame,
    ));
}

/// Require a region to contain only unselected blanks with default source colors and no attributes.
///
/// # Errors
/// Returns `AssertionError::Query` when `region` is empty or outside the frame geometry, or the
/// frame lacks span or cell detail covering it, and `AssertionError::Mismatch` when any cell in it
/// is not an unselected blank with default colors and no attributes.
pub fn assert_neutral_region(
    frame: &ScreenFrame,
    region: ScreenRegion,
) -> Result<(), AssertionError> {
    if region.top >= region.bottom || region.left >= region.right {
        return Err(AssertionError::Query(QueryError::InvalidQuery {
            reason: "neutral assertion region must be nonempty".to_owned(),
        }));
    }

    if region.bottom > frame.geometry.rows || region.right > frame.geometry.cols {
        return Err(AssertionError::Query(QueryError::InvalidQuery {
            reason: format!(
                "neutral assertion region {region:?} is outside frame geometry {}x{}",
                frame.geometry.cols, frame.geometry.rows
            ),
        }));
    }

    let returned = frame.returned_region;
    if region.top < returned.top
        || region.left < returned.left
        || region.bottom > returned.bottom
        || region.right > returned.right
    {
        return Err(AssertionError::Query(QueryError::InsufficientFrameDetail {
            reason: format!(
                "neutral assertion region {region:?} is not contained by returned frame region {returned:?}"
            ),
        }));
    }

    if matches!(&frame.data, ScreenFrameContent::Text { .. }) {
        return Err(AssertionError::Query(QueryError::InsufficientFrameDetail {
            reason: "neutral-region assertion requires spans or cells".to_owned(),
        }));
    }

    let mut checked = 0usize;
    let mut non_neutral = 0usize;

    frame.for_each_cell(|metadata, column, grapheme, wide_kind, appearance| {
        if metadata.row < region.top
            || metadata.row >= region.bottom
            || column < region.left
            || column >= region.right
        {
            return;
        }

        checked += 1;
        let style = &appearance.style;
        let neutral_style = style.foreground == ColorSpec::None
            && style.background == ColorSpec::None
            && style.underline_color == ColorSpec::None
            && !style.bold
            && !style.italic
            && !style.faint
            && !style.blink
            && !style.inverse
            && !style.invisible
            && !style.strikethrough
            && !style.overline
            && style.underline == 0;

        let neutral_grapheme = grapheme.is_empty() || grapheme == " ";

        let neutral_width = matches!(
            wide_kind,
            WideKind::Narrow | WideKind::SpacerHead | WideKind::SpacerTail
        );

        if !neutral_style
            || !neutral_grapheme
            || !neutral_width
            || appearance.selected
            || appearance.resolved_foreground.is_some()
            || appearance.resolved_background.is_some()
        {
            non_neutral += 1;
        }
    });

    let expected_cells = usize::from(region.width()) * usize::from(region.height());
    if checked == expected_cells && non_neutral == 0 {
        return Ok(());
    }
    return Err(AssertionError::Mismatch(mismatch(
        "neutral_region",
        &format!("{expected_cells} neutral terminal cells"),
        &format!("checked {checked}; non-neutral {non_neutral}"),
        frame,
    )));
}

/// Require a presentation-eligible frame outside synchronized-output suppression.
///
/// # Errors
/// Returns an `ExpectationMismatchError` when the frame is not presented or synchronized output is
/// active.
pub fn assert_no_presented_transient(frame: &ScreenFrame) -> Result<(), ExpectationMismatchError> {
    if frame.presented && !frame.modes.synchronized_output {
        return Ok(());
    }
    return Err(mismatch(
        "no_presented_transient",
        "presented=true and synchronized_output=false",
        &format!(
            "presented={} synchronized_output={}",
            frame.presented, frame.modes.synchronized_output
        ),
        frame,
    ));
}

fn mismatch(
    assertion: &'static str,
    expected: &str,
    actual: &str,
    frame: &ScreenFrame,
) -> ExpectationMismatchError {
    let (expected, expected_truncated) = bounded(expected);
    let (actual, actual_truncated) = bounded(actual);
    let (excerpt, excerpt_truncated) = bounded(&frame_excerpt(frame));

    return ExpectationMismatchError {
        assertion,
        expected,
        actual,
        excerpt,
        frame: Some(Box::new(AssertionFrameIdentity {
            state_hash: frame.state_hash.clone(),
            projection_hash: frame.frame_hash.clone(),
            presented: frame.presented,
        })),
        truncated: expected_truncated || actual_truncated || excerpt_truncated,
    };
}

fn frame_text(frame: &ScreenFrame) -> String {
    return match &frame.data {
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>()
            .join("\n"),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>()
            .join("\n"),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .map(|row| return row.text.as_str())
            .collect::<Vec<&str>>()
            .join("\n"),
        ScreenFrameContent::None => String::new(),
    };
}

fn frame_excerpt(frame: &ScreenFrame) -> String {
    let rows: Vec<(u16, &str)> = match &frame.data {
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .take(MAX_EXCERPT_ROWS)
            .map(|row| return (row.row, row.text.as_str()))
            .collect(),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .take(MAX_EXCERPT_ROWS)
            .map(|row| return (row.row, row.text.as_str()))
            .collect(),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .take(MAX_EXCERPT_ROWS)
            .map(|row| return (row.row, row.text.as_str()))
            .collect(),
        ScreenFrameContent::None => Vec::new(),
    };

    return rows
        .iter()
        .map(|(row, text)| format!("{row}: {text}"))
        .collect::<Vec<String>>()
        .join("\n");
}

fn bounded(text: &str) -> (String, bool) {
    let end = text.floor_char_boundary(MAX_ASSERTION_CHARS);

    return (
        text.get(..end).unwrap_or_default().to_owned(),
        end != text.len(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_frame() -> ScreenFrame {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/conformance/locators/locator.json"
        ))
        .expect("locator fixture");

        return serde_json::from_value(value.get("frame").cloned().expect("fixture frame"))
            .expect("screen frame");
    }

    #[test]
    fn neutral_region_rejects_uncaptured_and_out_of_geometry_cells() {
        let mut frame = fixture_frame();
        frame.returned_region = ScreenRegion {
            top: 0,
            left: 0,
            right: 1,
            bottom: 1,
        };
        assert!(matches!(
            assert_neutral_region(
                &frame,
                ScreenRegion {
                    top: 0,
                    left: 1,
                    right: 2,
                    bottom: 1,
                }
            ),
            Err(AssertionError::Query(
                QueryError::InsufficientFrameDetail { .. }
            ))
        ));
        assert!(matches!(
            assert_neutral_region(
                &frame,
                ScreenRegion {
                    top: 0,
                    left: 0,
                    right: 13,
                    bottom: 1,
                }
            ),
            Err(AssertionError::Query(QueryError::InvalidQuery { .. }))
        ));
    }
}
