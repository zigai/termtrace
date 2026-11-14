use super::session::SessionRef;
use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use termtrace_model::terminal::screen::{ScreenFrame, ScreenRegion};
use termtrace_sdk::defaults;
use termtrace_sdk::locator::{
    LocatorCondition, LocatorSpec, LocatorWaitOptions, MatchRange, MatchedCell, QueryDescription,
    QueryError, QueryResult, ScreenFrameQuery as _, ScreenMatch, StyleFilter,
};

/// A locator as the ordered builder calls that composed it.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocatorQuery {
    /// Clauses applied in order to a locator that matches every cell.
    clauses: Vec<LocatorClause>,
}

/// One locator builder call.
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "clause", rename_all = "snake_case", deny_unknown_fields)]
enum LocatorClause {
    /// Match a nonempty literal substring.
    Text {
        /// Text to find.
        text: String,
    },
    /// Match the complete selected row or region text.
    Exact {
        /// Text the row or region must equal.
        text: String,
    },
    /// Match a Rust regex that cannot match empty text.
    Regex {
        /// Regex source.
        pattern: String,
    },
    /// Restrict the query to one absolute row.
    Row {
        /// Zero-based row.
        row: u16,
    },
    /// Restrict the query to one nonempty half-open region.
    Region {
        /// Absolute region.
        region: ScreenRegion,
    },
    /// Require a cell appearance.
    Style {
        /// Appearance predicate.
        style: StyleFilter,
    },
    /// Permit text matching across linked soft-wrapped rows.
    AcrossSoftWraps {
        /// Whether matches may cross soft wraps.
        enabled: bool,
    },
    /// Combine with an independent locator.
    And {
        /// The other locator.
        locator: LocatorQuery,
    },
}

impl LocatorQuery {
    fn spec(&self) -> Result<LocatorSpec, QueryError> {
        let mut spec = LocatorSpec::default();
        for clause in &self.clauses {
            spec = match clause {
                LocatorClause::Text { text } => spec.and(LocatorSpec::text(text.clone())?)?,
                LocatorClause::Exact { text } => spec.and(LocatorSpec::exact(text.clone())?)?,
                LocatorClause::Regex { pattern } => {
                    spec.and(LocatorSpec::native_regex(pattern.clone())?)?
                }
                LocatorClause::Row { row } => spec.in_row(*row),
                LocatorClause::Region { region } => spec.in_region(*region)?,
                LocatorClause::Style { style } => spec.with_style(style.clone()),
                LocatorClause::AcrossSoftWraps { enabled } => spec.across_soft_wraps(*enabled),
                LocatorClause::And { locator } => spec.and(locator.spec()?)?,
            };
        }

        return Ok(spec);
    }
}

/// One match, tied to the frame it was found in.
#[derive(Serialize, JsonSchema)]
struct LocatorMatch {
    /// Ordered absolute covered ranges.
    ranges: Vec<MatchRange>,
    /// Matched text with spaces and graphemes preserved.
    text: String,
    /// Bounded covered cells.
    cells: Vec<MatchedCell>,
    /// Number of covered cells omitted from `cells`.
    omitted_cell_count: usize,
    /// Normalized query.
    query: QueryDescription,
}

impl LocatorMatch {
    fn of(found: &ScreenMatch) -> Self {
        return Self {
            ranges: found.ranges.to_vec(),
            text: found.text.to_string(),
            cells: found.covered_cells.to_vec(),
            omitted_cell_count: found.omitted_cell_count,
            query: found.query.clone(),
        };
    }
}

/// Every match in one frame.
#[derive(Serialize, JsonSchema)]
struct LocatorMatches {
    /// The frame the query ran on.
    frame: ScreenFrame,
    /// Matches in screen order.
    matches: Vec<LocatorMatch>,
}

impl LocatorMatches {
    fn of(frame: &ScreenFrame, matches: &[ScreenMatch]) -> Self {
        return Self {
            frame: frame.clone(),
            matches: matches.iter().map(LocatorMatch::of).collect(),
        };
    }

    fn of_result(result: &QueryResult) -> Self {
        return Self::of(result.frame(), result.all());
    }
}

/// One selected match and its frame.
#[derive(Serialize, JsonSchema)]
struct SelectedMatch {
    /// The frame the query ran on.
    frame: ScreenFrame,
    /// The match.
    #[serde(rename = "match")]
    found: LocatorMatch,
}

impl SelectedMatch {
    fn of(found: &ScreenMatch) -> Self {
        return Self {
            frame: (*found.frame).clone(),
            found: LocatorMatch::of(found),
        };
    }
}

/// Which match a strict selection takes.
#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(tag = "select", rename_all = "snake_case", deny_unknown_fields)]
enum Selection {
    /// The only match; fails on none or several.
    Single,
    /// The first match; fails on none.
    First,
    /// The match at a zero-based index.
    Nth {
        /// Zero-based index.
        index: usize,
    },
}

impl Selection {
    fn apply(self, result: &QueryResult) -> Result<&ScreenMatch, QueryError> {
        return match self {
            Self::Single => result.single(),
            Self::First => result.first(),
            Self::Nth { index } => result.nth(index),
        };
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LocatorParams {
    session: SessionRef,
    locator: LocatorQuery,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectParams {
    session: SessionRef,
    locator: LocatorQuery,
    selection: Selection,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LocatorWaitParams {
    session: SessionRef,
    locator: LocatorQuery,
    /// Required match count; at least one match by default.
    #[serde(default)]
    condition: Option<LocatorCondition>,
    /// Bound on the wait; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Polling interval; the default locator interval when absent.
    #[serde(default)]
    interval_ms: Option<u64>,
    /// How long the condition must hold; zero by default.
    #[serde(default)]
    stable_for_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FrameQueryParams {
    /// Frame to query.
    frame: ScreenFrame,
    locator: LocatorQuery,
    /// Strict selection; every match when absent.
    #[serde(default)]
    selection: Option<Selection>,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "locator.evaluate",
        "Capture the latest presented frame and return every match.",
        evaluate,
    );
    method(
        registry,
        "locator.count",
        "Capture the latest presented frame and count the matches.",
        count,
    );
    method(
        registry,
        "locator.select",
        "Capture the latest presented frame and return one strictly selected match.",
        select,
    );
    method(
        registry,
        "locator.wait_for",
        "Poll until the match count satisfies the condition, optionally for a stable interval.",
        wait_for,
    );
    method(
        registry,
        "frame.query",
        "Run a locator on a frame, optionally selecting one match strictly.",
        frame_query,
    );
}

pub(super) fn spec(locator: &LocatorQuery) -> Result<LocatorSpec, Failure> {
    return locator
        .spec()
        .map_err(|error| return Failure::query(&error));
}

fn evaluate(call: &Call<'_>, params: LocatorParams) -> Result<LocatorMatches, Failure> {
    let spec = spec(&params.locator)?;
    let session = params.session.attach(call)?;

    return session
        .locator(spec)
        .evaluate()
        .map(|result| return LocatorMatches::of_result(&result))
        .map_err(Failure::locator);
}

fn count(call: &Call<'_>, params: LocatorParams) -> Result<usize, Failure> {
    let spec = spec(&params.locator)?;
    let session = params.session.attach(call)?;

    return session.locator(spec).count().map_err(Failure::locator);
}

fn select(call: &Call<'_>, params: SelectParams) -> Result<SelectedMatch, Failure> {
    let spec = spec(&params.locator)?;
    let session = params.session.attach(call)?;
    let result = session.locator(spec).evaluate().map_err(Failure::locator)?;

    return params
        .selection
        .apply(&result)
        .map(SelectedMatch::of)
        .map_err(|error| return Failure::query(&error));
}

fn wait_for(call: &Call<'_>, params: LocatorWaitParams) -> Result<LocatorMatches, Failure> {
    let spec = spec(&params.locator)?;
    let session = params.session.attach(call)?;
    let options = LocatorWaitOptions {
        condition: params.condition.unwrap_or(LocatorCondition::Exists),
        timeout: millis(params.timeout_ms, defaults::WAIT_TIMEOUT),
        interval: millis(params.interval_ms, defaults::LOCATOR_POLL_INTERVAL),
        stable_for: millis(params.stable_for_ms, Duration::ZERO),
        cancel: Some(call.cancellation.clone()),
    };

    return session
        .locator(spec)
        .wait_for(options)
        .map(|result| return LocatorMatches::of(&result.frame, &result.matches))
        .map_err(Failure::locator);
}

fn frame_query(_call: &Call<'_>, params: FrameQueryParams) -> Result<Vec<LocatorMatch>, Failure> {
    let spec = spec(&params.locator)?;
    let result = params
        .frame
        .query(spec)
        .map_err(|error| return Failure::query(&error))?;

    return params.selection.map_or_else(
        || return Ok(result.all().iter().map(LocatorMatch::of).collect()),
        |selection| {
            return selection
                .apply(&result)
                .map(|found| return vec![LocatorMatch::of(found)])
                .map_err(|error| return Failure::query(&error));
        },
    );
}
