//! Pure immutable rich-frame queries and lazy session-bound locators.

use std::{
    error::Error as StdError,
    fmt,
    ops::RangeInclusive,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use regex::Regex;

use crate::{
    client::{Screen, Session, StableCaptureOptions},
    ghostty_vt::{ColorSpec, Rgb, WideKind},
    screen_frame::{
        FrameDetail, MAX_SCREEN_FRAME_CELLS, MAX_SCREEN_FRAME_JSON_BYTES, ScreenAppearance,
        ScreenFrame, ScreenFrameData, ScreenProjectionRequest, ScreenRegion,
    },
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

    fn description(&self) -> QueryDescription {
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
    frame: Arc<ScreenFrame>,
    spec: LocatorSpec,
    matches: Arc<[ScreenMatch]>,
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
        if self.matches.is_empty() {
            return Err(QueryError::NoMatch {
                query: self.spec.description(),
                state_hash: self.frame.state_hash.clone(),
                frame_hash: self.frame.frame_hash.clone(),
            });
        }
        if self.matches.len() != 1 {
            return Err(multiple_matches(&self.spec, &self.frame, &self.matches));
        }
        return self.matches.first().ok_or_else(|| {
            return QueryError::NoMatch {
                query: self.spec.description(),
                state_hash: self.frame.state_hash.clone(),
                frame_hash: self.frame.frame_hash.clone(),
            };
        });
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
impl Screen<'_> {
    /// Bind an immutable specification to this screen without performing I/O.
    #[must_use]
    pub fn locator(&self, spec: LocatorSpec) -> Locator<'_> {
        return Locator::new(self.session(), spec);
    }

    /// Bind a nonempty literal text query without performing I/O.
    pub fn get_by_text(&self, text: impl Into<String>) -> Result<Locator<'_>, QueryError> {
        return Ok(self.locator(LocatorSpec::text(text)?));
    }

    /// Bind an already compiled host-native regular expression without performing I/O.
    pub fn get_by_regex(&self, regex: Regex) -> Result<Locator<'_>, QueryError> {
        return Ok(self.locator(LocatorSpec::regex(regex)?));
    }

    /// Bind one absolute row scope without performing I/O.
    #[must_use]
    pub fn row(&self, row: u16) -> Locator<'_> {
        return self.locator(LocatorSpec::default().in_row(row));
    }

    /// Bind one exact absolute region without performing I/O.
    pub fn region(&self, region: ScreenRegion) -> Result<Locator<'_>, QueryError> {
        return Ok(self.locator(LocatorSpec::default().in_region(region)?));
    }

    /// Bind a complete appearance filter without performing I/O.
    #[must_use]
    pub fn filter(&self, style: StyleFilter) -> Locator<'_> {
        return self.locator(LocatorSpec::default().with_style(style));
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

/// Lazy session-bound immutable locator.
#[derive(Clone)]
pub struct Locator<'session> {
    session: &'session Session,
    spec: LocatorSpec,
}

impl<'session> Locator<'session> {
    /// Bind an immutable specification without performing I/O.
    #[must_use]
    pub fn new(session: &'session Session, spec: LocatorSpec) -> Self {
        return Self { session, spec };
    }

    /// Compose another pure specification without performing I/O.
    pub fn and(&self, other: LocatorSpec) -> Result<Self, QueryError> {
        return Ok(Self {
            session: self.session,
            spec: self.spec.clone().and(other)?,
        });
    }

    /// Capture exactly one latest-presented frame and evaluate it once.
    pub fn evaluate(&self) -> Result<QueryResult, LocatorError> {
        let frame = self
            .session
            .screen()
            .frame(ScreenProjectionRequest {
                detail: FrameDetail::Spans,
                region: self.spec.region,
            })
            .map_err(LocatorError::Capture)?;
        return query_frame(Arc::new(frame), self.spec.clone()).map_err(LocatorError::Query);
    }

    /// Evaluate all matches after exactly one capture.
    pub fn all(&self) -> Result<Arc<[ScreenMatch]>, LocatorError> {
        return Ok(self.evaluate()?.matches);
    }

    /// Count matches after exactly one capture.
    pub fn count(&self) -> Result<usize, LocatorError> {
        return Ok(self.evaluate()?.count());
    }

    /// Strictly require one match after exactly one capture.
    pub fn single(&self) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .single()
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Explicitly select the first match after exactly one capture.
    pub fn first(&self) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .first()
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Explicitly select one zero-based match after exactly one capture.
    pub fn nth(&self, index: usize) -> Result<ScreenMatch, LocatorError> {
        return self
            .evaluate()?
            .nth(index)
            .cloned()
            .map_err(LocatorError::Query);
    }

    /// Poll exact one-frame evaluations and return the satisfying frame without recapture.
    pub fn wait(
        &self,
        timeout: Duration,
        interval: Duration,
    ) -> Result<LocatorWaitResult, LocatorError> {
        return self.wait_stable(timeout, interval, Duration::ZERO);
    }

    /// Poll until a query remains satisfied in one unchanged semantic state.
    pub fn wait_stable(
        &self,
        timeout: Duration,
        interval: Duration,
        stable_for: Duration,
    ) -> Result<LocatorWaitResult, LocatorError> {
        if timeout.is_zero() || interval.is_zero() {
            return Err(LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator wait timeout and interval must be greater than zero".to_owned(),
            }));
        }
        if stable_for > timeout {
            return Err(LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator stable interval must not exceed its timeout".to_owned(),
            }));
        }
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            return LocatorError::Query(QueryError::InvalidQuery {
                reason: "locator wait timeout exceeds the monotonic clock range".to_owned(),
            });
        })?;
        let mut matching_state: Option<(String, Instant)> = None;
        let mut last_frame: Option<Arc<ScreenFrame>> = None;
        loop {
            let before_capture = Instant::now();
            let Some(remaining) = deadline.checked_duration_since(before_capture) else {
                return Err(LocatorError::Timeout {
                    timeout,
                    frame: last_frame,
                    query: self.spec.description(),
                });
            };
            let capture_timeout = remaining.min(Duration::from_secs(3_600));
            let capture = self.session.screen().capture_stable(StableCaptureOptions {
                stable_for: Duration::from_nanos(1).min(capture_timeout),
                timeout: capture_timeout,
                projection: ScreenProjectionRequest {
                    detail: FrameDetail::Spans,
                    region: self.spec.region,
                },
                allow_incomplete: true,
            });
            let observed_at = Instant::now();
            let capture = match capture {
                Ok(capture) => capture,
                Err(_) if observed_at >= deadline => {
                    return Err(LocatorError::Timeout {
                        timeout,
                        frame: last_frame,
                        query: self.spec.description(),
                    });
                }
                Err(error) => return Err(LocatorError::Capture(error)),
            };
            let frame = Arc::new(capture.frame);
            last_frame = Some(Arc::clone(&frame));
            if observed_at >= deadline {
                return Err(LocatorError::Timeout {
                    timeout,
                    frame: last_frame,
                    query: self.spec.description(),
                });
            }
            let result = query_frame(frame, self.spec.clone()).map_err(LocatorError::Query)?;
            let evaluated_at = Instant::now();
            if evaluated_at >= deadline {
                return Err(LocatorError::Timeout {
                    timeout,
                    frame: Some(result.frame),
                    query: self.spec.description(),
                });
            }
            if !result.matches.is_empty() {
                if stable_for.is_zero() {
                    return Ok(LocatorWaitResult {
                        frame: result.frame,
                        matches: result.matches,
                    });
                }
                match &matching_state {
                    Some((state_hash, since)) if state_hash == &result.frame.state_hash => {
                        if evaluated_at.saturating_duration_since(*since) >= stable_for {
                            return Ok(LocatorWaitResult {
                                frame: result.frame,
                                matches: result.matches,
                            });
                        }
                    }
                    _ => matching_state = Some((result.frame.state_hash.clone(), evaluated_at)),
                }
            } else {
                matching_state = None;
            }
            thread::sleep(interval.min(deadline.saturating_duration_since(evaluated_at)));
        }
    }
}

/// Successful locator wait evidence from the same satisfying frame.
#[derive(Clone, Debug)]
pub struct LocatorWaitResult {
    /// Exact satisfying frame.
    pub frame: Arc<ScreenFrame>,
    /// Matches evaluated from that same frame.
    pub matches: Arc<[ScreenMatch]>,
}

/// Session-bound locator infrastructure and pure-query failures.
#[derive(Debug)]
pub enum LocatorError {
    /// Exact rich frame capture failed.
    Capture(crate::Error),
    /// Pure evaluation failed.
    Query(QueryError),
    /// The monotonic locator deadline elapsed.
    Timeout {
        /// Requested bound.
        timeout: Duration,
        /// Last exact frame captured before the deadline, when one was available.
        frame: Option<Arc<ScreenFrame>>,
        /// Bounded normalized query.
        query: QueryDescription,
    },
}

impl fmt::Display for LocatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capture(error) => return write!(f, "capture locator frame: {error}"),
            Self::Query(error) => return error.fmt(f),
            Self::Timeout { timeout, query, .. } => {
                return write!(f, "locator {} timed out after {timeout:?}", query.text);
            }
        }
    }
}

impl StdError for LocatorError {}

fn bounded_text(text: &str) -> (String, bool) {
    let mut end = text.len().min(MAX_DIAGNOSTIC_CHARS);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    return (
        text.get(..end).unwrap_or_default().to_owned(),
        end != text.len(),
    );
}

// The evaluator is implemented below the public model so its scratch representation never leaks.
#[derive(Clone)]
struct LogicalCell {
    row: u16,
    column: u16,
    end_column: u16,
    grapheme: String,
    wide_kind: WideKind,
    appearance: ScreenAppearance,
}

#[derive(Clone)]
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

impl ScreenFrame {
    /// Evaluate an immutable host-local specification without I/O.
    pub fn query(&self, spec: LocatorSpec) -> Result<QueryResult, QueryError> {
        return query_frame(Arc::new(self.clone()), spec);
    }
}

fn query_frame(frame: Arc<ScreenFrame>, spec: LocatorSpec) -> Result<QueryResult, QueryError> {
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
    let mut matches = evaluate_rows(&frame, &rows, &spec)?;
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
    rows: &[LogicalRow],
    spec: &LocatorSpec,
) -> Result<Vec<ScreenMatch>, QueryError> {
    let mut output: Vec<ScreenMatch> = Vec::new();
    let mut budget = QueryBudget::default();
    let grouped_rows = if spec.cross_soft_wraps && spec.matcher.is_some() {
        soft_wrap_groups(rows)
    } else {
        rows.to_vec()
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

fn soft_wrap_groups(rows: &[LogicalRow]) -> Vec<LogicalRow> {
    let mut groups: Vec<LogicalRow> = Vec::new();
    for row in rows {
        let continues = groups
            .last()
            .is_some_and(|previous| return previous.soft_wrapped && row.wrap_continuation);
        if continues {
            if let Some(previous) = groups.last_mut() {
                previous.cells.extend(row.cells.iter().cloned());
                previous.soft_wrapped = row.soft_wrapped;
            }
            continue;
        }
        groups.push(row.clone());
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
        let grapheme = if grapheme.is_empty() {
            " ".to_owned()
        } else {
            grapheme.to_owned()
        };
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
mod tests {
    use super::*;
    use crate::ghostty_vt::CellStyle;
    use crate::observation::CaptureQuality;
    use crate::screen_frame::SpanCell;

    fn fixture_frame() -> ScreenFrame {
        let value: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/locator-conformance.json"))
                .expect("locator fixture");
        return serde_json::from_value(value.get("frame").cloned().expect("frame"))
            .expect("screen frame");
    }

    #[test]
    fn empty_and_zero_width_text_queries_are_rejected() {
        assert!(matches!(
            LocatorSpec::text(""),
            Err(QueryError::InvalidQuery { .. })
        ));
        for pattern in ["a*", r"\b", r"(?m)^\b", r"(?:x{0})\B"] {
            assert!(
                matches!(
                    LocatorSpec::native_regex(pattern),
                    Err(QueryError::InvalidQuery { .. })
                ),
                "{pattern}"
            );
        }
        assert!(LocatorSpec::native_regex("a+").is_ok());
    }

    #[test]
    fn conflicting_composition_is_rejected_without_observation() {
        let first = LocatorSpec::text("first").unwrap_or_default();
        let second = LocatorSpec::exact("second").unwrap_or_default();
        assert!(matches!(
            first.and(second),
            Err(QueryError::InvalidQuery { .. })
        ));

        let first_row = LocatorSpec::default().in_row(1);
        let second_row = LocatorSpec::default().in_row(2);
        let compiled = Regex::new("ready").expect("valid regex");
        assert!(LocatorSpec::regex(compiled).is_ok());
        assert!(matches!(
            first_row.and(second_row),
            Err(QueryError::InvalidQuery { .. })
        ));

        let row = LocatorSpec::default().in_row(0);
        let disjoint_region = LocatorSpec::default()
            .in_region(ScreenRegion {
                top: 1,
                left: 0,
                right: 2,
                bottom: 2,
            })
            .expect("region");
        assert!(matches!(
            row.and(disjoint_region),
            Err(QueryError::InvalidQuery { .. })
        ));
        let direct = LocatorSpec::default()
            .in_region(ScreenRegion {
                top: 1,
                left: 0,
                right: 2,
                bottom: 2,
            })
            .expect("region")
            .in_row(0);
        assert!(matches!(
            fixture_frame().query(direct),
            Err(QueryError::InvalidQuery { .. })
        ));
    }

    #[test]
    fn spacer_heads_remain_queryable_at_wrap_edges() {
        let mut frame = fixture_frame();
        let ScreenFrameData::Spans { rows } = &mut frame.data else {
            panic!("spans frame");
        };
        let span = rows[0].spans.first_mut().expect("row span");
        let spacer = span.cells.last_mut().expect("last cell");
        spacer.grapheme.clear();
        spacer.wide_kind = WideKind::SpacerHead;

        let row_match = frame
            .query(LocatorSpec::default().in_row(0))
            .expect("row query")
            .single()
            .expect("single row match")
            .clone();
        assert_eq!(
            row_match.ranges.as_ref(),
            &[MatchRange {
                row: 0,
                start_column: 0,
                end_column: 12,
            }]
        );
        let region_match = frame
            .query(
                LocatorSpec::default()
                    .in_region(ScreenRegion {
                        top: 0,
                        left: 11,
                        right: 12,
                        bottom: 1,
                    })
                    .expect("region"),
            )
            .expect("region query")
            .single()
            .expect("single spacer match")
            .clone();
        assert_eq!(region_match.ranges[0].start_column, 11);
        assert_eq!(region_match.ranges[0].end_column, 12);
    }

    #[test]
    fn overlapping_matches_stop_at_aggregate_work_budget() {
        let mut frame = fixture_frame();
        frame.geometry.cols = 16_384;
        frame.geometry.rows = 1;
        frame.requested_region = ScreenRegion {
            top: 0,
            left: 0,
            right: 16_384,
            bottom: 1,
        };
        frame.returned_region = frame.requested_region;
        let ScreenFrameData::Spans { rows } = &mut frame.data else {
            panic!("spans frame");
        };
        rows.truncate(1);
        rows[0].text = "a".repeat(16_384);
        let row_text = rows[0].text.clone();
        let span = rows[0].spans.first_mut().expect("row span");
        span.start_column = 0;
        span.end_column = 16_384;
        span.cell_count = 16_384;
        span.text = row_text;
        span.cells = (0..16_384)
            .map(|_| {
                return SpanCell {
                    grapheme: "a".to_owned(),
                    wide_kind: WideKind::Narrow,
                };
            })
            .collect();
        rows[0].spans.truncate(1);

        let error = frame
            .query(LocatorSpec::text("a".repeat(8_192)).expect("locator"))
            .expect_err("aggregate budget");
        assert!(matches!(error, QueryError::ResourceLimit { .. }));
    }

    #[test]
    fn diagnostics_truncate_only_at_unicode_boundaries() {
        let input = "界".repeat(MAX_DIAGNOSTIC_CHARS);
        let (bounded, truncated) = bounded_text(&input);
        assert!(truncated);
        assert!(bounded.len() <= MAX_DIAGNOSTIC_CHARS);
        assert!(bounded.is_char_boundary(bounded.len()));
    }

    #[test]
    fn grapheme_boundaries_retain_terminal_columns_for_combining_and_wide_cells() {
        let appearance = ScreenAppearance {
            style: CellStyle {
                foreground: ColorSpec::None,
                background: ColorSpec::None,
                underline_color: ColorSpec::None,
                bold: false,
                italic: false,
                faint: false,
                blink: false,
                inverse: false,
                invisible: false,
                strikethrough: false,
                overline: false,
                underline: 0,
            },

            resolved_foreground: None,
            resolved_background: None,
            selected: false,
        };
        let row = LogicalRow {
            row: 3,
            soft_wrapped: false,
            wrap_continuation: false,
            cells: vec![
                LogicalCell {
                    row: 3,
                    column: 4,
                    end_column: 5,
                    grapheme: "e\u{301}".to_owned(),
                    wide_kind: WideKind::Narrow,
                    appearance: appearance.clone(),
                },
                LogicalCell {
                    row: 3,
                    column: 5,
                    end_column: 7,
                    grapheme: "界".to_owned(),
                    wide_kind: WideKind::Wide,
                    appearance,
                },
            ],
        };
        let (text, boundaries) = selected_text(&row, &[0, 1]);
        assert_eq!(text, "e\u{301}界");
        assert_eq!(boundaries, vec![(0, 3, 0), (3, 6, 1)]);
        assert_eq!(row.cells.last().map(|cell| return cell.column), Some(5));
        assert_eq!(row.cells.last().map(|cell| return cell.end_column), Some(7));
    }
    #[test]
    fn composed_regions_intersect_and_partial_graphemes_are_rejected() {
        let outer = LocatorSpec::default()
            .in_region(ScreenRegion {
                top: 0,
                left: 0,
                bottom: 4,
                right: 8,
            })
            .unwrap_or_default();
        let inner = LocatorSpec::default()
            .in_region(ScreenRegion {
                top: 1,
                left: 2,
                bottom: 3,
                right: 6,
            })
            .unwrap_or_default();
        let combined = outer.and(inner).unwrap_or_default();
        assert_eq!(
            combined.region,
            Some(ScreenRegion {
                top: 1,
                left: 2,
                bottom: 3,
                right: 6,
            })
        );

        let boundaries = [(0, 3, 0), (3, 6, 1)];
        assert!(matches!(
            matched_boundary_range(&boundaries, 1, 3),
            Err(QueryError::InvalidQuery { .. })
        ));
    }
    #[derive(serde::Deserialize)]
    struct ConformanceCorpus {
        version: u32,
        frame: ScreenFrame,
        cases: Vec<ConformanceCase>,
    }

    #[derive(serde::Deserialize)]
    struct ConformanceCase {
        name: String,
        query: serde_json::Value,
        #[serde(default)]
        matches: Vec<ExpectedMatch>,
        error: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct ExpectedMatch {
        text: String,
        ranges: Vec<[u16; 3]>,
    }

    fn conformance_spec(value: &serde_json::Value) -> Result<LocatorSpec, QueryError> {
        let kind = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                return QueryError::InvalidQuery {
                    reason: "fixture query kind is missing".to_owned(),
                };
            })?;
        match kind {
            "text" => {
                let text = value
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let exact = value
                    .get("exact")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let cross_soft_wraps = value
                    .get("cross_soft_wraps")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let spec = if exact {
                    LocatorSpec::exact(text)?
                } else {
                    LocatorSpec::text(text)?
                };
                return Ok(spec.across_soft_wraps(cross_soft_wraps));
            }
            "regex" => {
                let pattern = value
                    .get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                return LocatorSpec::native_regex(pattern);
            }
            "style" => {
                let bold = value.get("bold").and_then(serde_json::Value::as_bool);
                let resolved_background = value
                    .get("resolved_background")
                    .and_then(serde_json::Value::as_str)
                    .map(|color| {
                        return match color {
                            "#3b1e1c" => Some(Rgb {
                                r: 59,
                                g: 30,
                                b: 28,
                            }),
                            _ => None,
                        };
                    });
                return Ok(LocatorSpec::default().with_style(StyleFilter {
                    bold,
                    resolved_background,
                    ..StyleFilter::default()
                }));
            }
            "row" => {
                let row = value
                    .get("row")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|row| return u16::try_from(row).ok())
                    .ok_or_else(|| {
                        return QueryError::InvalidQuery {
                            reason: "fixture row is invalid".to_owned(),
                        };
                    })?;
                return Ok(LocatorSpec::default().in_row(row));
            }
            "region" => {
                let coordinate = |name: &str| {
                    return value
                        .get(name)
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|coordinate| return u16::try_from(coordinate).ok())
                        .ok_or_else(|| {
                            return QueryError::InvalidQuery {
                                reason: format!("fixture region {name} is invalid"),
                            };
                        });
                };
                let top = coordinate("top")?;
                let left = coordinate("left")?;
                let width = coordinate("width")?;
                let height = coordinate("height")?;
                return LocatorSpec::default().in_region(ScreenRegion {
                    top,
                    left,
                    right: left.saturating_add(width),
                    bottom: top.saturating_add(height),
                });
            }
            "all" => {
                let specs = value
                    .get("specs")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| {
                        return QueryError::InvalidQuery {
                            reason: "fixture composition is invalid".to_owned(),
                        };
                    })?;
                let mut combined = LocatorSpec::default();
                for spec in specs {
                    combined = combined.and(conformance_spec(spec)?)?;
                }
                return Ok(combined);
            }
            _ => {
                return Err(QueryError::InvalidQuery {
                    reason: "fixture query kind is unknown".to_owned(),
                });
            }
        }
    }

    #[test]
    fn shared_locator_conformance_corpus_matches_public_contract() {
        let corpus: ConformanceCorpus =
            serde_json::from_str(include_str!("../fixtures/locator-conformance.json"))
                .expect("locator conformance fixture");
        assert_eq!(corpus.version, 1);
        assert!(!corpus.frame.presented);
        assert_eq!(corpus.frame.capture_quality, CaptureQuality::Incomplete);

        for test_case in corpus.cases {
            let result = conformance_spec(&test_case.query)
                .and_then(|spec| return corpus.frame.query(spec))
                .map(|query| {
                    return query
                        .all()
                        .iter()
                        .map(|screen_match| {
                            assert_eq!(screen_match.frame.state_hash, "fixture-state-v1");
                            assert_eq!(screen_match.frame.frame_hash, "fixture-frame-v1");
                            assert!(!screen_match.frame.presented);
                            assert!(screen_match.query.text.len() <= MAX_DIAGNOSTIC_CHARS);
                            return ExpectedMatch {
                                text: screen_match.text.to_string(),
                                ranges: screen_match
                                    .ranges
                                    .iter()
                                    .map(|range| {
                                        return [range.row, range.start_column, range.end_column];
                                    })
                                    .collect(),
                            };
                        })
                        .collect::<Vec<_>>();
                });

            if test_case.error.as_deref() == Some("invalid_query") {
                assert!(
                    matches!(result, Err(QueryError::InvalidQuery { .. })),
                    "{}",
                    test_case.name
                );
                continue;
            }
            let actual = result.unwrap_or_else(|error| {
                panic!("{} failed: {error}", test_case.name);
            });
            assert_eq!(actual.len(), test_case.matches.len(), "{}", test_case.name);
            for (actual_match, expected_match) in actual.iter().zip(&test_case.matches) {
                assert_eq!(actual_match.text, expected_match.text, "{}", test_case.name);
                assert_eq!(
                    actual_match.ranges, expected_match.ranges,
                    "{}",
                    test_case.name
                );
            }
        }
    }
}
