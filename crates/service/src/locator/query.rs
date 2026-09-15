//! Pure immutable rich-frame query model and evaluator.

use regex::Regex;
use std::{error::Error as StdError, fmt, ops::RangeInclusive, sync::Arc};
use termtrace_model::{
    terminal::screen::{
        ScreenAppearance, ScreenFrame, ScreenFrameData, ScreenRegion, MAX_SCREEN_FRAME_CELLS,
        MAX_SCREEN_FRAME_JSON_BYTES,
    },
    terminal::state::{ColorSpec, Rgb, WideKind},
};

const MAX_DIAGNOSTIC_MATCHES: usize = 16;
const MAX_QUERY_MATCHES: usize = 10_000;
const MAX_DIAGNOSTIC_CHARS: usize = 256;
const MAX_COVERED_CELLS: usize = 128;
const MAX_QUERY_MATCHED_CELLS: usize = MAX_SCREEN_FRAME_CELLS as usize;
const MAX_QUERY_TEXT_BYTES: usize = MAX_SCREEN_FRAME_JSON_BYTES as usize;
const MAX_QUERY_CELL_WORK: usize = MAX_QUERY_MATCHED_CELLS * 4;

/// Whether a text query searches or compares the complete selected text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextMatchKind {
    /// Find every nonempty substring occurrence.
    #[default]
    Substring,
    /// Match the complete selected row or region text.
    Exact,
}

/// One host-native text matcher.
#[derive(Clone)]
pub enum TextMatcher {
    /// Literal Unicode text.
    Literal {
        /// Nonempty search text.
        text: Arc<str>,
        /// Substring or exact behavior.
        kind: TextMatchKind,
    },
    /// Rust's native regular-expression engine.
    NativeRegex {
        /// Stable source used in bounded diagnostics.
        source: Arc<str>,
        /// Compiled native expression, never serialized to the wire protocol.
        regex: Arc<Regex>,
    },
}

impl fmt::Debug for TextMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal { text, kind } => {
                return f
                    .debug_struct("Literal")
                    .field("text", text)
                    .field("kind", kind)
                    .finish();
            }
            Self::NativeRegex { source, .. } => {
                return f
                    .debug_struct("NativeRegex")
                    .field("source", source)
                    .finish_non_exhaustive();
            }
        }
    }
}

/// Complete optional appearance constraints applied to every covered logical cell.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StyleFilter {
    /// Source foreground constraint.
    pub foreground: Option<ColorSpec>,
    /// Source background constraint.
    pub background: Option<ColorSpec>,
    /// Source underline color constraint.
    pub underline_color: Option<ColorSpec>,
    /// Resolved foreground constraint, including an explicit unresolved value.
    pub resolved_foreground: Option<Option<Rgb>>,
    /// Resolved background constraint, including an explicit unresolved value.
    pub resolved_background: Option<Option<Rgb>>,
    /// Bold constraint.
    pub bold: Option<bool>,
    /// Italic constraint.
    pub italic: Option<bool>,
    /// Faint constraint.
    pub faint: Option<bool>,
    /// Blink constraint.
    pub blink: Option<bool>,
    /// Inverse constraint.
    pub inverse: Option<bool>,
    /// Invisible constraint.
    pub invisible: Option<bool>,
    /// Strikethrough constraint.
    pub strikethrough: Option<bool>,
    /// Overline constraint.
    pub overline: Option<bool>,
    /// Native underline variant constraint.
    pub underline: Option<i32>,
    /// Selection constraint.
    pub selected: Option<bool>,
    /// Authoritative terminal width-role constraint.
    pub wide_kind: Option<WideKind>,
}

impl StyleFilter {
    pub(crate) fn matches(&self, appearance: &ScreenAppearance, wide_kind: WideKind) -> bool {
        let style = &appearance.style;

        return option_matches(&self.foreground, &style.foreground)
            && option_matches(&self.background, &style.background)
            && option_matches(&self.underline_color, &style.underline_color)
            && option_matches(&self.resolved_foreground, &appearance.resolved_foreground)
            && option_matches(&self.resolved_background, &appearance.resolved_background)
            && option_matches(&self.bold, &style.bold)
            && option_matches(&self.italic, &style.italic)
            && option_matches(&self.faint, &style.faint)
            && option_matches(&self.blink, &style.blink)
            && option_matches(&self.inverse, &style.inverse)
            && option_matches(&self.invisible, &style.invisible)
            && option_matches(&self.strikethrough, &style.strikethrough)
            && option_matches(&self.overline, &style.overline)
            && option_matches(&self.underline, &style.underline)
            && option_matches(&self.selected, &appearance.selected)
            && option_matches(&self.wide_kind, &wide_kind);
    }
}

fn option_matches<T: PartialEq>(expected: &Option<T>, actual: &T) -> bool {
    return expected
        .as_ref()
        .is_none_or(|expected| return expected == actual);
}

/// Immutable query specification composed without observing a session.
#[derive(Clone, Debug, Default)]
pub struct LocatorSpec {
    matcher: Option<TextMatcher>,
    row: Option<u16>,
    region: Option<ScreenRegion>,
    style: Option<StyleFilter>,
    cross_soft_wraps: bool,
}

impl LocatorSpec {
    /// Match a nonempty literal substring.
    pub fn text(text: impl Into<String>) -> Result<Self, QueryError> {
        return Self::literal(text, TextMatchKind::Substring);
    }

    /// Match the complete selected row or region text.
    pub fn exact(text: impl Into<String>) -> Result<Self, QueryError> {
        return Self::literal(text, TextMatchKind::Exact);
    }

    fn literal(text: impl Into<String>, kind: TextMatchKind) -> Result<Self, QueryError> {
        let text = text.into();
        if text.is_empty() {
            return Err(QueryError::InvalidQuery {
                reason: "literal text must not be empty".to_owned(),
            });
        }
        return Ok(Self {
            matcher: Some(TextMatcher::Literal {
                text: Arc::from(text),
                kind,
            }),
            ..Self::default()
        });
    }

    /// Compile a host-native regex source that cannot produce an empty match.
    pub fn native_regex(source: impl Into<String>) -> Result<Self, QueryError> {
        let source = source.into();
        if source.is_empty() {
            return Err(QueryError::InvalidQuery {
                reason: "regex source must not be empty".to_owned(),
            });
        }

        let regex = Regex::new(&source).map_err(|error| {
            return QueryError::InvalidQuery {
                reason: bounded_text(&error.to_string()).0,
            };
        })?;

        return Self::regex(regex);
    }

    /// Use an already compiled host-native regex that cannot produce an empty match.
    pub fn regex(regex: Regex) -> Result<Self, QueryError> {
        if regex_can_match_zero_width(&regex) {
            return Err(QueryError::InvalidQuery {
                reason: "regex can produce a zero-width match".to_owned(),
            });
        }

        let source: Arc<str> = Arc::from(regex.as_str());

        return Ok(Self {
            matcher: Some(TextMatcher::NativeRegex {
                source,
                regex: Arc::new(regex),
            }),
            ..Self::default()
        });
    }

    /// Restrict this query to one absolute row.
    #[must_use]
    pub fn in_row(mut self, row: u16) -> Self {
        self.row = Some(row);

        return self;
    }

    /// Restrict this query to one absolute nonempty half-open region.
    pub fn in_region(mut self, region: ScreenRegion) -> Result<Self, QueryError> {
        if region.top >= region.bottom || region.left >= region.right {
            return Err(QueryError::InvalidQuery {
                reason: "query region must be nonempty".to_owned(),
            });
        }

        self.region = Some(region);

        return Ok(self);
    }

    /// Require a complete cell appearance predicate.
    #[must_use]
    pub fn with_style(mut self, style: StyleFilter) -> Self {
        self.style = Some(style);

        return self;
    }

    /// Permit text matching across explicitly linked native soft-wrap rows.
    #[must_use]
    pub fn across_soft_wraps(mut self, enabled: bool) -> Self {
        self.cross_soft_wraps = enabled;

        return self;
    }

    /// Combine independent scope, text, and appearance clauses.
    pub fn and(self, other: Self) -> Result<Self, QueryError> {
        if self.matcher.is_some() && other.matcher.is_some() {
            return Err(QueryError::InvalidQuery {
                reason: "a composed locator may contain only one text matcher".to_owned(),
            });
        }

        if self.row.is_some() && other.row.is_some() && self.row != other.row {
            return Err(QueryError::InvalidQuery {
                reason: "composed row scopes conflict".to_owned(),
            });
        }

        let region = match (self.region, other.region) {
            (Some(left), Some(right)) => {
                let intersection = ScreenRegion {
                    top: left.top.max(right.top),
                    left: left.left.max(right.left),
                    bottom: left.bottom.min(right.bottom),
                    right: left.right.min(right.right),
                };

                if intersection.top >= intersection.bottom
                    || intersection.left >= intersection.right
                {
                    return Err(QueryError::InvalidQuery {
                        reason: "composed region scopes do not intersect".to_owned(),
                    });
                }

                Some(intersection)
            }
            (Some(region), None) | (None, Some(region)) => Some(region),
            (None, None) => None,
        };

        if self.style.is_some() && other.style.is_some() {
            return Err(QueryError::InvalidQuery {
                reason: "a composed locator may contain only one style filter".to_owned(),
            });
        }

        let row = self.row.or(other.row);
        if let (Some(row), Some(region)) = (row, region)
            && (row < region.top || row >= region.bottom)
        {
            return Err(QueryError::InvalidQuery {
                reason: "composed row and region scopes do not intersect".to_owned(),
            });
        }
        return Ok(Self {
            matcher: self.matcher.or(other.matcher),
            row,
            region,
            style: self.style.or(other.style),
            cross_soft_wraps: self.cross_soft_wraps || other.cross_soft_wraps,
        });
    }

    pub(super) fn region(&self) -> Option<ScreenRegion> {
        return self.region;
    }

    pub(super) fn description(&self) -> QueryDescription {
        let matcher = match &self.matcher {
            Some(TextMatcher::Literal { text, kind }) => {
                format!("literal({kind:?}, {:?})", bounded_text(text).0)
            }
            Some(TextMatcher::NativeRegex { source, .. }) => {
                format!("native_regex({:?})", bounded_text(source).0)
            }
            None => "cells".to_owned(),
        };

        let (text, truncated) = bounded_text(&format!(
            "{matcher}; row={:?}; region={:?}; style={:?}; cross_soft_wraps={}",
            self.row, self.region, self.style, self.cross_soft_wraps
        ));

        return QueryDescription { text, truncated };
    }
}

fn regex_can_match_zero_width(regex: &Regex) -> bool {
    const PROBES: [&str; 9] = ["", "a", "α", "_", "!", "\n", "\r\n", "a!", "!a"];

    return PROBES.iter().any(|probe| {
        return regex
            .find_iter(probe)
            .any(|regex_match| return regex_match.is_empty());
    });
}

/// Bounded normalized query evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryDescription {
    /// Human-readable canonical query.
    pub text: String,
    /// Whether it was truncated at a Unicode boundary.
    pub truncated: bool,
}

/// One absolute half-open range covered by a match.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MatchRange {
    /// Absolute row.
    pub row: u16,
    /// Inclusive absolute terminal column.
    pub start_column: u16,
    /// Exclusive absolute terminal column.
    pub end_column: u16,
}

/// Bounded cell evidence retained by a match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchedCell {
    /// Absolute row.
    pub row: u16,
    /// Absolute terminal column of the authoritative head cell.
    pub column: u16,
    /// Complete grapheme cluster.
    pub grapheme: String,
    /// Authoritative native width role.
    pub wide_kind: WideKind,
    /// Complete source and resolved appearance.
    pub appearance: ScreenAppearance,
}

/// Immutable match tied to the exact frame that produced it.
#[derive(Clone, Debug)]
pub struct ScreenMatch {
    /// Exact immutable frame evidence; identity is never reconstructed from text.
    pub frame: Arc<ScreenFrame>,
    /// Ordered absolute covered ranges.
    pub ranges: Arc<[MatchRange]>,
    /// Matched text with spaces and graphemes preserved.
    pub text: Arc<str>,
    /// Bounded covered logical cells.
    pub covered_cells: Arc<[MatchedCell]>,
    /// Number of additional covered cells omitted from diagnostics.
    pub omitted_cell_count: usize,
    /// Bounded normalized query.
    pub query: QueryDescription,
}

/// Pure query and strict-result failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryError {
    /// Empty, zero-width, conflicting, or otherwise ambiguous query input.
    InvalidQuery {
        /// Bounded reason.
        reason: String,
    },
    /// A strict operation found no match.
    NoMatch {
        /// Bounded normalized query.
        query: QueryDescription,
        /// Exact semantic state identity.
        state_hash: String,
        /// Exact rich-frame projection identity.
        frame_hash: String,
    },
    /// A strict operation found multiple matches.
    MultipleMatches {
        /// Bounded normalized query.
        query: QueryDescription,
        /// Exact total count.
        count: usize,
        /// Exact semantic state identity.
        state_hash: String,
        /// Exact rich-frame projection identity.
        frame_hash: String,
        /// At most the first bounded ranges.
        ranges: Vec<MatchRange>,
        /// Whether additional ranges were omitted.
        truncated: bool,
    },
    /// A zero-based explicit index was absent.
    IndexOutOfBounds {
        /// Requested index.
        index: usize,
        /// Exact total count.
        count: usize,
        /// Bounded normalized query.
        query: QueryDescription,
        /// Exact semantic state identity.
        state_hash: String,
        /// Exact rich-frame projection identity.
        frame_hash: String,
    },
    /// A bounded query result exceeded its public resource ceiling.
    ResourceLimit {
        /// Exact number that would have been returned.
        requested: usize,
        /// Public maximum.
        maximum: usize,
    },
    /// The frame detail could not be losslessly evaluated.
    InsufficientFrameDetail {
        /// Bounded reason.
        reason: String,
    },
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuery { reason } => return write!(f, "invalid terminal query: {reason}"),
            Self::NoMatch {
                query, state_hash, ..
            } => return write!(f, "no match for {} in state {state_hash}", query.text),
            Self::MultipleMatches {
                query,
                count,
                state_hash,
                ..
            } => {
                return write!(
                    f,
                    "expected one match for {}, found {count} in state {state_hash}",
                    query.text
                );
            }
            Self::IndexOutOfBounds {
                index,
                count,
                query,
                state_hash,
                ..
            } => {
                return write!(
                    f,
                    "match index {index} is absent for {} ({count} matches) in state {state_hash}",
                    query.text
                );
            }
            Self::ResourceLimit { requested, maximum } => {
                return write!(
                    f,
                    "query match limit exceeded ({requested} requested, maximum {maximum})"
                );
            }
            Self::InsufficientFrameDetail { reason } => {
                return write!(f, "frame cannot satisfy query: {reason}");
            }
        }
    }
}

impl StdError for QueryError {}

/// Pure result set over one explicit immutable frame.
#[derive(Clone, Debug)]
pub struct QueryResult {
    pub(super) frame: Arc<ScreenFrame>,
    pub(super) spec: LocatorSpec,
    pub(super) matches: Arc<[ScreenMatch]>,
}

impl QueryResult {
    /// Borrow all matches in deterministic row/column order.
    #[must_use]
    pub fn all(&self) -> &[ScreenMatch] {
        return &self.matches;
    }

    /// Exact match count.
    #[must_use]
    pub fn count(&self) -> usize {
        return self.matches.len();
    }

    /// Require exactly one match.
    pub fn single(&self) -> Result<&ScreenMatch, QueryError> {
        match self.all() {
            [] => {
                return Err(QueryError::NoMatch {
                    query: self.spec.description(),
                    state_hash: self.frame.state_hash.clone(),
                    frame_hash: self.frame.frame_hash.clone(),
                });
            }
            [only] => return Ok(only),
            _ => return Err(multiple_matches(&self.spec, &self.frame, &self.matches)),
        }
    }

    /// Explicitly select the first match, rejecting absence but not ambiguity.
    pub fn first(&self) -> Result<&ScreenMatch, QueryError> {
        return self.matches.first().ok_or_else(|| {
            return QueryError::NoMatch {
                query: self.spec.description(),
                state_hash: self.frame.state_hash.clone(),
                frame_hash: self.frame.frame_hash.clone(),
            };
        });
    }

    /// Explicitly select one zero-based ordered match.
    pub fn nth(&self, index: usize) -> Result<&ScreenMatch, QueryError> {
        return self.matches.get(index).ok_or_else(|| {
            return QueryError::IndexOutOfBounds {
                index,
                count: self.matches.len(),
                query: self.spec.description(),
                state_hash: self.frame.state_hash.clone(),
                frame_hash: self.frame.frame_hash.clone(),
            };
        });
    }

    /// Exact immutable frame evaluated by this result.
    #[must_use]
    pub fn frame(&self) -> &Arc<ScreenFrame> {
        return &self.frame;
    }
}

fn multiple_matches(
    spec: &LocatorSpec,
    frame: &ScreenFrame,
    matches: &[ScreenMatch],
) -> QueryError {
    let ranges: Vec<MatchRange> = matches
        .iter()
        .take(MAX_DIAGNOSTIC_MATCHES)
        .filter_map(|screen_match| return screen_match.ranges.first().copied())
        .collect();

    return QueryError::MultipleMatches {
        query: spec.description(),
        count: matches.len(),
        state_hash: frame.state_hash.clone(),
        frame_hash: frame.frame_hash.clone(),
        truncated: matches.len() > ranges.len(),
        ranges,
    };
}

fn bounded_text(text: &str) -> (String, bool) {
    let end = text.floor_char_boundary(MAX_DIAGNOSTIC_CHARS);

    return (
        text.get(..end).unwrap_or_default().to_owned(),
        end != text.len(),
    );
}

// The evaluator is implemented below the public model so its scratch representation never leaks.
struct LogicalCell {
    row: u16,
    column: u16,
    end_column: u16,
    grapheme: String,
    wide_kind: WideKind,
    appearance: ScreenAppearance,
}

struct LogicalRow {
    row: u16,
    soft_wrapped: bool,
    wrap_continuation: bool,
    cells: Vec<LogicalCell>,
}

#[derive(Default)]
struct QueryBudget {
    cell_work: usize,
    matched_cells: usize,
    text_bytes: usize,
}

impl QueryBudget {
    fn charge_work(&mut self, cells: usize) -> Result<(), QueryError> {
        self.cell_work = self.cell_work.saturating_add(cells);

        if self.cell_work > MAX_QUERY_CELL_WORK {
            return Err(QueryError::ResourceLimit {
                requested: self.cell_work,
                maximum: MAX_QUERY_CELL_WORK,
            });
        }
        return Ok(());
    }

    fn charge_match(&mut self, cells: usize, text_bytes: usize) -> Result<(), QueryError> {
        self.matched_cells = self.matched_cells.saturating_add(cells);

        if self.matched_cells > MAX_QUERY_MATCHED_CELLS {
            return Err(QueryError::ResourceLimit {
                requested: self.matched_cells,
                maximum: MAX_QUERY_MATCHED_CELLS,
            });
        }

        self.text_bytes = self.text_bytes.saturating_add(text_bytes);

        if self.text_bytes > MAX_QUERY_TEXT_BYTES {
            return Err(QueryError::ResourceLimit {
                requested: self.text_bytes,
                maximum: MAX_QUERY_TEXT_BYTES,
            });
        }
        return Ok(());
    }
}

/// Host-local immutable query operations for model-owned rich frames.
pub trait ScreenFrameQuery {
    /// Evaluate an immutable specification without I/O.
    fn query(&self, spec: LocatorSpec) -> Result<QueryResult, QueryError>;
}

impl ScreenFrameQuery for ScreenFrame {
    fn query(&self, spec: LocatorSpec) -> Result<QueryResult, QueryError> {
        return query_frame(Arc::new(self.clone()), spec);
    }
}

pub(super) fn query_frame(
    frame: Arc<ScreenFrame>,
    spec: LocatorSpec,
) -> Result<QueryResult, QueryError> {
    if let Some(row) = spec.row
        && row >= frame.geometry.rows
    {
        return Err(QueryError::InvalidQuery {
            reason: format!(
                "row {row} is outside frame geometry with {} rows",
                frame.geometry.rows
            ),
        });
    }

    if let Some(row) = spec.row
        && (row < frame.returned_region.top || row >= frame.returned_region.bottom)
    {
        return Err(QueryError::InsufficientFrameDetail {
            reason: format!(
                "row {row} is not contained by returned frame region {:?}",
                frame.returned_region
            ),
        });
    }

    if let Some(region) = spec.region {
        if region.top >= region.bottom
            || region.left >= region.right
            || region.bottom > frame.geometry.rows
            || region.right > frame.geometry.cols
        {
            return Err(QueryError::InvalidQuery {
                reason: format!(
                    "region {region:?} is outside frame geometry {}x{}",
                    frame.geometry.cols, frame.geometry.rows
                ),
            });
        }

        let returned = frame.returned_region;
        if region.top < returned.top
            || region.left < returned.left
            || region.bottom > returned.bottom
            || region.right > returned.right
        {
            return Err(QueryError::InsufficientFrameDetail {
                reason: format!(
                    "region {region:?} is not contained by returned frame region {returned:?}"
                ),
            });
        }
    }

    if let (Some(row), Some(region)) = (spec.row, spec.region)
        && (row < region.top || row >= region.bottom)
    {
        return Err(QueryError::InvalidQuery {
            reason: "row and region scopes do not intersect".to_owned(),
        });
    }

    let rows = logical_rows(&frame)?;
    let mut matches = evaluate_rows(&frame, rows, &spec)?;
    matches.sort_by_key(|screen_match| {
        return screen_match.ranges.first().copied().unwrap_or(MatchRange {
            row: u16::MAX,
            start_column: u16::MAX,
            end_column: u16::MAX,
        });
    });

    return Ok(QueryResult {
        frame,
        spec,
        matches: Arc::from(matches),
    });
}

fn evaluate_rows(
    frame: &Arc<ScreenFrame>,
    rows: Vec<LogicalRow>,
    spec: &LocatorSpec,
) -> Result<Vec<ScreenMatch>, QueryError> {
    let mut output: Vec<ScreenMatch> = Vec::new();
    let mut budget = QueryBudget::default();
    let grouped_rows = if spec.cross_soft_wraps && spec.matcher.is_some() {
        soft_wrap_groups(rows)
    } else {
        rows
    };

    for row in &grouped_rows {
        let selected: Vec<usize> = row
            .cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| {
                return spec.row.is_none_or(|expected| return expected == cell.row)
                    && cell_in_region(cell, spec.region);
            })
            .map(|(index, _)| return index)
            .collect();
        if selected.is_empty() {
            continue;
        }

        match &spec.matcher {
            Some(TextMatcher::Literal { text, kind }) => {
                evaluate_literal(
                    frame,
                    row,
                    &selected,
                    spec,
                    text,
                    *kind,
                    &mut output,
                    &mut budget,
                )?;
            }
            Some(TextMatcher::NativeRegex { regex, .. }) => {
                evaluate_regex(frame, row, &selected, spec, regex, &mut output, &mut budget)?;
            }
            None => {
                evaluate_style_ranges(frame, row, &selected, spec, &mut output, &mut budget)?;
            }
        }
    }

    return Ok(output);
}

fn soft_wrap_groups(rows: Vec<LogicalRow>) -> Vec<LogicalRow> {
    let mut groups: Vec<LogicalRow> = Vec::new();
    for mut row in rows {
        if let Some(previous) = groups.last_mut()
            && previous.soft_wrapped
            && row.wrap_continuation
        {
            previous.cells.append(&mut row.cells);
            previous.soft_wrapped = row.soft_wrapped;
            continue;
        }

        groups.push(row);
    }

    return groups;
}

fn selected_text(row: &LogicalRow, selected: &[usize]) -> (String, Vec<(usize, usize, usize)>) {
    let mut text = String::new();
    let mut boundaries: Vec<(usize, usize, usize)> = Vec::with_capacity(selected.len());

    for index in selected {
        if let Some(cell) = row.cells.get(*index) {
            let start = text.len();
            text.push_str(&cell.grapheme);
            boundaries.push((start, text.len(), *index));
        }
    }

    return (text, boundaries);
}

fn evaluate_literal(
    frame: &Arc<ScreenFrame>,
    row: &LogicalRow,
    selected: &[usize],
    spec: &LocatorSpec,
    needle: &str,
    kind: TextMatchKind,
    output: &mut Vec<ScreenMatch>,
    budget: &mut QueryBudget,
) -> Result<(), QueryError> {
    let (haystack, boundaries) = selected_text(row, selected);

    if kind == TextMatchKind::Exact {
        if haystack == needle {
            push_byte_match(
                frame,
                row,
                &boundaries,
                spec,
                0,
                haystack.len(),
                output,
                budget,
            )?;
        }

        return Ok(());
    }

    for (start, _, _) in &boundaries {
        if !haystack
            .get(*start..)
            .is_some_and(|suffix| return suffix.starts_with(needle))
        {
            continue;
        }

        push_byte_match(
            frame,
            row,
            &boundaries,
            spec,
            *start,
            start.saturating_add(needle.len()),
            output,
            budget,
        )?;
    }

    return Ok(());
}

fn evaluate_regex(
    frame: &Arc<ScreenFrame>,
    row: &LogicalRow,
    selected: &[usize],
    spec: &LocatorSpec,
    regex: &Regex,
    output: &mut Vec<ScreenMatch>,
    budget: &mut QueryBudget,
) -> Result<(), QueryError> {
    let (haystack, boundaries) = selected_text(row, selected);

    for regex_match in regex.find_iter(&haystack) {
        if regex_match.is_empty() {
            return Err(QueryError::InvalidQuery {
                reason: "regex produced a zero-width match".to_owned(),
            });
        }

        push_byte_match(
            frame,
            row,
            &boundaries,
            spec,
            regex_match.start(),
            regex_match.end(),
            output,
            budget,
        )?;
    }

    return Ok(());
}

fn push_byte_match(
    frame: &Arc<ScreenFrame>,
    row: &LogicalRow,
    boundaries: &[(usize, usize, usize)],
    spec: &LocatorSpec,
    start: usize,
    end: usize,
    output: &mut Vec<ScreenMatch>,
    budget: &mut QueryBudget,
) -> Result<(), QueryError> {
    let boundary_range = matched_boundary_range(boundaries, start, end)?;
    let Some(matched_boundaries) = boundaries.get(boundary_range) else {
        return Ok(());
    };
    budget.charge_work(matched_boundaries.len())?;
    if !matched_boundaries.iter().all(|(_, _, index)| {
        return row
            .cells
            .get(*index)
            .is_some_and(|cell| return style_matches(spec.style.as_ref(), cell));
    }) {
        return Ok(());
    }

    let text_bytes = matched_boundaries
        .iter()
        .filter_map(|(_, _, index)| return row.cells.get(*index))
        .fold(0usize, |total, cell| {
            return total.saturating_add(cell.grapheme.len());
        });
    budget.charge_match(matched_boundaries.len(), text_bytes)?;
    let cells: Vec<&LogicalCell> = matched_boundaries
        .iter()
        .filter_map(|(_, _, index)| return row.cells.get(*index))
        .collect();

    return push_match(output, make_match(frame, spec, &cells));
}

fn matched_boundary_range(
    boundaries: &[(usize, usize, usize)],
    start: usize,
    end: usize,
) -> Result<RangeInclusive<usize>, QueryError> {
    let start_boundary = boundaries
        .binary_search_by_key(&start, |(cell_start, _, _)| return *cell_start)
        .ok();
    let end_boundary = boundaries
        .binary_search_by_key(&end, |(_, cell_end, _)| return *cell_end)
        .ok();

    return match (start_boundary, end_boundary) {
        (Some(start_boundary), Some(end_boundary)) => Ok(start_boundary..=end_boundary),
        _ => Err(QueryError::InvalidQuery {
            reason: "text match is not aligned to complete terminal graphemes".to_owned(),
        }),
    };
}

fn evaluate_style_ranges(
    frame: &Arc<ScreenFrame>,
    row: &LogicalRow,
    selected: &[usize],
    spec: &LocatorSpec,
    output: &mut Vec<ScreenMatch>,
    budget: &mut QueryBudget,
) -> Result<(), QueryError> {
    let mut run: Vec<&LogicalCell> = Vec::new();
    for index in selected {
        let Some(cell) = row.cells.get(*index) else {
            continue;
        };

        budget.charge_work(1)?;

        let contiguous = run
            .last()
            .is_none_or(|previous| return previous.end_column == cell.column);
        if style_matches(spec.style.as_ref(), cell) && contiguous {
            run.push(cell);
            continue;
        }

        if !run.is_empty() {
            charge_and_push_match(frame, spec, &run, output, budget)?;
            run.clear();
        }

        if style_matches(spec.style.as_ref(), cell) {
            run.push(cell);
        }
    }

    if !run.is_empty() {
        charge_and_push_match(frame, spec, &run, output, budget)?;
    }

    return Ok(());
}

fn charge_and_push_match(
    frame: &Arc<ScreenFrame>,
    spec: &LocatorSpec,
    cells: &[&LogicalCell],
    output: &mut Vec<ScreenMatch>,
    budget: &mut QueryBudget,
) -> Result<(), QueryError> {
    let text_bytes = cells.iter().fold(0usize, |total, cell| {
        return total.saturating_add(cell.grapheme.len());
    });
    budget.charge_match(cells.len(), text_bytes)?;

    return push_match(output, make_match(frame, spec, cells));
}

fn push_match(output: &mut Vec<ScreenMatch>, screen_match: ScreenMatch) -> Result<(), QueryError> {
    if output.len() >= MAX_QUERY_MATCHES {
        return Err(QueryError::ResourceLimit {
            requested: output.len().saturating_add(1),
            maximum: MAX_QUERY_MATCHES,
        });
    }

    output.push(screen_match);

    return Ok(());
}

fn style_matches(filter: Option<&StyleFilter>, cell: &LogicalCell) -> bool {
    return filter.is_none_or(|filter| return filter.matches(&cell.appearance, cell.wide_kind));
}

fn cell_in_region(cell: &LogicalCell, region: Option<ScreenRegion>) -> bool {
    return region.is_none_or(|region| {
        return cell.row >= region.top
            && cell.row < region.bottom
            && cell.column >= region.left
            && cell.end_column <= region.right;
    });
}

fn make_match(frame: &Arc<ScreenFrame>, spec: &LocatorSpec, cells: &[&LogicalCell]) -> ScreenMatch {
    let mut ranges: Vec<MatchRange> = Vec::new();
    for cell in cells {
        if let Some(range) = ranges.last_mut()
            && range.row == cell.row
            && range.end_column == cell.column
        {
            range.end_column = cell.end_column;
            continue;
        }

        ranges.push(MatchRange {
            row: cell.row,
            start_column: cell.column,
            end_column: cell.end_column,
        });
    }

    let ranges: Arc<[MatchRange]> = Arc::from(ranges);
    let text: String = cells
        .iter()
        .map(|cell| return cell.grapheme.as_str())
        .collect();
    let covered_cells: Vec<MatchedCell> = cells
        .iter()
        .take(MAX_COVERED_CELLS)
        .map(|cell| {
            return MatchedCell {
                row: cell.row,
                column: cell.column,
                grapheme: cell.grapheme.clone(),
                wide_kind: cell.wide_kind,
                appearance: cell.appearance.clone(),
            };
        })
        .collect();

    return ScreenMatch {
        frame: Arc::clone(frame),
        ranges,
        text: Arc::from(text),
        covered_cells: Arc::from(covered_cells),
        omitted_cell_count: cells.len().saturating_sub(MAX_COVERED_CELLS),
        query: spec.description(),
    };
}

// Kept as one narrow adapter over the authoritative rich-frame representation.
fn logical_rows(frame: &ScreenFrame) -> Result<Vec<LogicalRow>, QueryError> {
    if matches!(&frame.data, ScreenFrameData::Text { .. }) {
        return Err(QueryError::InsufficientFrameDetail {
            reason: "text detail has no authoritative cell-to-column or appearance map".to_owned(),
        });
    }

    let mut rows: Vec<LogicalRow> = Vec::new();

    frame.for_each_cell(|metadata, column, grapheme, wide_kind, appearance| {
        if rows.last().is_none_or(|row| return row.row != metadata.row) {
            rows.push(LogicalRow {
                row: metadata.row,
                soft_wrapped: metadata.soft_wrapped,
                wrap_continuation: metadata.wrap_continuation,
                cells: Vec::new(),
            });
        }

        if wide_kind == WideKind::SpacerTail {
            return;
        }

        let grapheme = if grapheme.is_empty() { " ".to_owned() } else { grapheme.to_owned() };
        let end_column = column.saturating_add(if wide_kind == WideKind::Wide { 2 } else { 1 });

        if let Some(row) = rows.last_mut() {
            row.cells.push(LogicalCell {
                row: metadata.row,
                column,
                end_column,
                grapheme,
                wide_kind,
                appearance: appearance.clone(),
            });
        }
    });

    return Ok(rows);
}

#[cfg(test)]
mod tests;
